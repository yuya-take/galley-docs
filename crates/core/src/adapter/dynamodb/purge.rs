//! 資料と版の物理削除（`galley purge`）。
//!
//! ブロブの参照は、ブロブの項目の `ref_count`（版を足すと +1）で数える。消すときは `ref_count` を読み、
//! 読んだ値のままであることを条件に、減らすか（ほかの版が使っている）、ブロブの項目を消す（どの版も
//! 使わなくなる）。読んでから書くまでの間に同じブロブを使う版が足されると条件に合わず、読み直して
//! やり直す。版の追加・戻す操作の側は、ブロブの項目と同じトランザクションで版を書くので、消したあとの
//! ブロブを指す版はできない（戻す操作は `Conflict` になり、新しい登録はブロブの項目を作り直す）。
//!
//! 1 回のトランザクションは 100 件までなので、版が多い資料は古い版から分けて消し、最後に現在版と資料を
//! 消す。途中で止まっても、古い版を [`purge_revision`] で消したのと同じ状態になり、やり直せる。

use std::collections::BTreeSet;

use crate::domain::{
    blob::BlobHash,
    document::{Document, PurgedRevisions},
    error::RepositoryError,
    revision::Revision,
    shared::{DocumentId, RevisionId},
};

use super::{
    document::load_document,
    item::{self, REF_COUNT},
    keys::Key,
    revision::load_revisions,
    table::{Expression, MAX_TRANSACTION_ITEMS, Order, Table, Write},
};

/// 同時の書き込みで取り消されたとき、読み直してやり直す回数。
const MAX_ATTEMPTS: usize = 5;

/// 資料を消すトランザクションで、版とブロブのほかに書くもの（資料と資料の印）。
const DOCUMENT_WRITES: usize = 2;

/// 資料とそのすべての版を消す。資料が無ければ何も消さない。
pub(super) async fn purge_document(
    table: &Table,
    id: DocumentId,
) -> Result<PurgedRevisions, RepositoryError> {
    let mut deleted: Vec<Revision> = Vec::new();
    let mut unreferenced: Vec<BlobHash> = Vec::new();
    let mut attempt = 1;
    loop {
        let Some(stored) = load_document(table, id).await? else {
            break;
        };
        let document = stored.document;
        // 計画を見せたあとに足された版も、読み直して一緒に消す
        let revisions = load_revisions(table, id, Order::Ascending, None).await?;
        let (targets, is_last) = next_batch(&document, revisions);
        let blobs = blob_changes(table, &targets).await?;
        let mut writes: Vec<Write> = targets.iter().flat_map(revision_deletes).collect();
        writes.extend(blobs.writes);
        if is_last {
            // 読んだあとに版が足されていれば現在版が変わっているので、読み直す
            writes.push(
                Write::delete(Key::document(document.project_id, &document.slug))
                    .when(same_current_revision(&document)),
            );
            writes.push(Write::delete(Key::document_ref(id)));
        }
        match table.transact(writes).await {
            Ok(()) => {
                deleted.extend(targets);
                for hash in blobs.unreferenced {
                    if !unreferenced.contains(&hash) {
                        unreferenced.push(hash);
                    }
                }
                if is_last {
                    break;
                }
            }
            Err(RepositoryError::Conflict(_)) if attempt < MAX_ATTEMPTS => attempt += 1,
            Err(err) => return Err(err),
        }
    }
    Ok(purged(deleted, &unreferenced))
}

/// 版を1つ消す。版が無ければ何も消さない。資料の「版を登録した人」も、残る版から数え直す。
pub(super) async fn purge_revision(
    table: &Table,
    id: RevisionId,
) -> Result<PurgedRevisions, RepositoryError> {
    let mut attempt = 1;
    loop {
        let Some(item) = table.get(&Key::revision_by_id(id)).await? else {
            return Ok(PurgedRevisions::default());
        };
        let revision = Revision::try_from(&item)?;
        let mut writes: Vec<Write> = revision_deletes(&revision).collect();
        if let Some(stored) = load_document(table, revision.document_id).await? {
            let document = stored.document;
            if document.current_revision_id == revision.id {
                return Err(RepositoryError::Conflict(format!(
                    "最新の版（第{}版）は消せません",
                    revision.number
                )));
            }
            let authors: BTreeSet<String> =
                load_revisions(table, document.id, Order::Ascending, None)
                    .await?
                    .into_iter()
                    .filter(|other| other.id != revision.id)
                    .map(|other| other.author_name.as_str().to_owned())
                    .collect();
            let update = if authors.is_empty() {
                Expression::new("REMOVE #author_names")
            } else {
                Expression::new("SET #author_names = :authors")
                    .value(":authors", item::name_set(authors))
            }
            .name("#author_names", "author_names");
            // 数えたあとに版が足されていれば、その版の更新者が抜けるので読み直す
            writes.push(
                Write::update(Key::document(document.project_id, &document.slug), update)
                    .when(same_current_revision(&document)),
            );
        }
        let blobs = blob_changes(table, std::slice::from_ref(&revision)).await?;
        writes.extend(blobs.writes);
        match table.transact(writes).await {
            Ok(()) => return Ok(purged(vec![revision], &blobs.unreferenced)),
            Err(RepositoryError::Conflict(_)) if attempt < MAX_ATTEMPTS => attempt += 1,
            Err(err) => return Err(err),
        }
    }
}

/// 次のトランザクションで消す版と、それで資料も消すか。
///
/// すべての版と資料が1回に収まればまとめて消す。収まらなければ、現在版を除いた古い版から、収まる分だけ。
fn next_batch(document: &Document, revisions: Vec<Revision>) -> (Vec<Revision>, bool) {
    if writes_for(&revisions) + DOCUMENT_WRITES <= MAX_TRANSACTION_ITEMS {
        return (revisions, true);
    }
    let mut batch = Vec::new();
    for revision in revisions {
        if revision.id == document.current_revision_id {
            continue;
        }
        batch.push(revision);
        if writes_for(&batch) > MAX_TRANSACTION_ITEMS {
            batch.pop();
            break;
        }
    }
    (batch, false)
}

/// 版を消すのに要る書き込みの数（版の写し3つと、ブロブごとに1つ）。
fn writes_for(revisions: &[Revision]) -> usize {
    let hashes: BTreeSet<&str> = revisions.iter().map(|r| r.blob_hash.as_str()).collect();
    revisions.len() * 3 + hashes.len()
}

/// 版の写し3つを消す。資料のパーティションの写しは、まだあることを条件にする（同じ版を同時に
/// 消す別の `galley purge` と重なったときに、ブロブの参照を二重に減らさない）。
fn revision_deletes(revision: &Revision) -> impl Iterator<Item = Write> {
    let [by_number, by_id, by_blob] = item::revision_keys(revision);
    [
        Write::delete(by_number).when(Expression::exists()),
        Write::delete(by_id),
        Write::delete(by_blob),
    ]
    .into_iter()
}

fn same_current_revision(document: &Document) -> Expression {
    Expression::new("#current_revision_id = :current_revision_id")
        .name("#current_revision_id", "current_revision_id")
        .value(
            ":current_revision_id",
            item::uuid(document.current_revision_id.as_uuid()),
        )
}

/// 消す版のブロブの参照を減らす書き込みと、どの版からも使われなくなるブロブ。
struct BlobChanges {
    writes: Vec<Write>,
    unreferenced: Vec<BlobHash>,
}

async fn blob_changes(
    table: &Table,
    revisions: &[Revision],
) -> Result<BlobChanges, RepositoryError> {
    // ブロブごとに、消す版のうち何個が使っているか（順番は最初に出てきた順）
    let mut uses: Vec<(&BlobHash, u64)> = Vec::new();
    for revision in revisions {
        match uses
            .iter_mut()
            .find(|(hash, _)| *hash == &revision.blob_hash)
        {
            Some((_, count)) => *count += 1,
            None => uses.push((&revision.blob_hash, 1)),
        }
    }
    let mut changes = BlobChanges {
        writes: Vec::new(),
        unreferenced: Vec::new(),
    };
    for (hash, count) in uses {
        let Some(blob) = table.get(&Key::blob(hash)).await? else {
            tracing::warn!(%hash, "ブロブの情報がありません。参照の数は直しません");
            continue;
        };
        let ref_count = item::ref_count(&blob)?;
        let unchanged = Expression::new("#ref_count = :read")
            .name("#ref_count", REF_COUNT)
            .value(":read", item::number(ref_count));
        if ref_count <= count {
            if ref_count < count {
                tracing::warn!(
                    %hash,
                    ref_count,
                    count,
                    "ブロブの参照の数が、消す版の数より少なくなっています"
                );
            }
            changes
                .writes
                .push(Write::delete(Key::blob(hash)).when(unchanged));
            changes.unreferenced.push(hash.clone());
        } else {
            changes.writes.push(
                Write::update(
                    Key::blob(hash),
                    Expression::new("SET #ref_count = :remaining")
                        .name("#ref_count", REF_COUNT)
                        .value(":remaining", item::number(ref_count - count)),
                )
                .when(unchanged),
            );
        }
    }
    Ok(changes)
}

/// 消した版の番号（新しい順）と、参照が無くなったブロブ（消した版の新しい順に、最初に出てきた順）。
/// SQLite の実装と同じ順にそろえる。
fn purged(mut deleted: Vec<Revision>, unreferenced: &[BlobHash]) -> PurgedRevisions {
    deleted.sort_by_key(|revision| std::cmp::Reverse(revision.number));
    let mut unreferenced_blobs: Vec<BlobHash> = Vec::new();
    for revision in &deleted {
        if unreferenced.contains(&revision.blob_hash)
            && !unreferenced_blobs.contains(&revision.blob_hash)
        {
            unreferenced_blobs.push(revision.blob_hash.clone());
        }
    }
    PurgedRevisions {
        numbers: deleted.iter().map(|revision| revision.number).collect(),
        unreferenced_blobs,
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::*;
    use crate::domain::{
        document::{DocumentSlug, DocumentTitle},
        revision::{AuthorName, RevisionMessage, RevisionNumber, RevisionSource},
        shared::ProjectId,
    };

    fn revisions(count: u32) -> (Document, Vec<Revision>) {
        let document_id = DocumentId::generate();
        let revisions: Vec<Revision> = (1..=count)
            .map(|n| Revision {
                id: RevisionId::generate(),
                document_id,
                number: RevisionNumber::new(n).unwrap(),
                blob_hash: BlobHash::of(format!("{n}").as_bytes()),
                message: RevisionMessage::parse("更新").unwrap(),
                author_name: AuthorName::parse("佐藤").unwrap(),
                source: RevisionSource::Web,
                restored_from: None,
                created_at: Timestamp::UNIX_EPOCH,
            })
            .collect();
        let document = Document {
            id: document_id,
            project_id: ProjectId::generate(),
            slug: DocumentSlug::parse("plan").unwrap(),
            title: DocumentTitle::parse("計画").unwrap(),
            current_revision_id: revisions.last().unwrap().id,
            archived_at: None,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
        };
        (document, revisions)
    }

    #[test]
    fn small_document_is_purged_at_once() {
        // 24 版 × 4 + 2 = 98 件
        let (document, all) = revisions(24);
        let (batch, is_last) = next_batch(&document, all.clone());
        assert!(is_last);
        assert_eq!(batch, all);
    }

    #[test]
    fn large_document_is_split_from_oldest_and_keeps_current() {
        let (document, all) = revisions(30);
        let (batch, is_last) = next_batch(&document, all.clone());
        assert!(!is_last);
        // 1 版につき写し3つとブロブ1つ。100 件に収まる 25 版
        assert_eq!(batch.len(), 25);
        assert_eq!(batch[0].number.get(), 1);
        assert!(batch.iter().all(|r| r.id != document.current_revision_id));
        assert!(writes_for(&batch) <= MAX_TRANSACTION_ITEMS);
    }

    #[test]
    fn purged_numbers_and_blobs_are_newest_first() {
        let (_, mut all) = revisions(3);
        all[2].blob_hash = all[0].blob_hash.clone();
        let unreferenced = vec![all[1].blob_hash.clone(), all[0].blob_hash.clone()];
        let result = purged(all.clone(), &unreferenced);
        let numbers: Vec<u32> = result.numbers.iter().map(|n| n.get()).collect();
        assert_eq!(numbers, [3, 2, 1]);
        assert_eq!(
            result.unreferenced_blobs,
            [all[0].blob_hash.clone(), all[1].blob_hash.clone()]
        );
    }
}
