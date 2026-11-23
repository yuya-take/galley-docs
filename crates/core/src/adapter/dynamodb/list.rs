//! 資料の一覧。SQLite では結合と EXISTS の SQL で書いているものを、Query とアプリの中の処理にする。
//!
//! プロジェクトを1回の Query で読み、対象のプロジェクトごとの資料の Query を並行して行ってから、
//! 絞り込み・並べ替え・件数の制限をこのモジュールで行う。資料の項目は現在版の番号・変更メモ・更新者と、
//! 版を登録した人の集合を持つので、版を読み直さずに一覧の1行を作れる。

use std::{cmp::Ordering, collections::HashMap};

use futures_util::future::try_join_all;

use crate::domain::{
    document::{DocumentListItem, DocumentOrder, DocumentQuery},
    error::RepositoryError,
    project::Project,
    shared::{ArchiveFilter, ProjectId},
};

use super::{
    item::StoredDocument,
    project::{load_documents, load_projects},
    table::Table,
};

pub(super) async fn list(
    table: &Table,
    query: &DocumentQuery,
) -> Result<Vec<DocumentListItem>, RepositoryError> {
    let projects = load_projects(table).await?;
    let targets = target_projects(&projects, query);
    let documents: Vec<StoredDocument> = try_join_all(
        targets
            .iter()
            .map(|project| load_documents(table, project.id)),
    )
    .await?
    .into_iter()
    .flatten()
    .collect();
    let projects: HashMap<ProjectId, &Project> = targets
        .into_iter()
        .map(|project| (project.id, project))
        .collect();
    Ok(select(documents, query)
        .into_iter()
        .filter_map(|stored| {
            let project = projects.get(&stored.document.project_id)?;
            Some(list_item(stored, project))
        })
        .collect())
}

/// 資料を読むプロジェクト。プロジェクトを指定しなければ全プロジェクトだが、アーカイブしていない資料の
/// 一覧には、アーカイブしたプロジェクトの資料を含めない（SQLite の実装と同じ）。
fn target_projects<'a>(projects: &'a [Project], query: &DocumentQuery) -> Vec<&'a Project> {
    projects
        .iter()
        .filter(|project| match query.project_id {
            Some(id) => project.id == id,
            None => query.archive == ArchiveFilter::Archived || !project.is_archived(),
        })
        .collect()
}

/// 条件で絞り込み、並べ替えて、件数を制限する。
fn select(documents: Vec<StoredDocument>, query: &DocumentQuery) -> Vec<StoredDocument> {
    let title = query
        .title_contains
        .as_deref()
        .filter(|text| !text.is_empty())
        .map(str::to_ascii_lowercase);
    let mut selected: Vec<StoredDocument> = documents
        .into_iter()
        .filter(|stored| {
            let document = &stored.document;
            let archive_matches = match query.archive {
                ArchiveFilter::Active => !document.is_archived(),
                ArchiveFilter::Archived => document.is_archived(),
            };
            archive_matches
                && query.project_id.is_none_or(|id| document.project_id == id)
                && title.as_deref().is_none_or(|text| {
                    // SQLite の LIKE と同じく、ASCII の英字だけ大文字と小文字を区別しない
                    document.title.as_str().to_ascii_lowercase().contains(text)
                })
                && query
                    .updated_since
                    .is_none_or(|since| document.updated_at >= since)
                && query
                    .author_name
                    .as_ref()
                    .is_none_or(|name| stored.author_names.contains(name.as_str()))
        })
        .collect();
    selected.sort_by(|a, b| compare(query.order, a, b));
    if let Some(limit) = query.limit {
        // u32 は usize に収まる（Galley は 32 ビット以上で動かす）
        selected.truncate(limit as usize);
    }
    selected
}

/// 並び順。同じ値のときは ID で並べる（SQLite の実装と同じ。資料名はバイト列の順）。
fn compare(order: DocumentOrder, a: &StoredDocument, b: &StoredDocument) -> Ordering {
    let (a, b) = (&a.document, &b.document);
    match order {
        DocumentOrder::UpdatedDesc => (b.updated_at, b.id).cmp(&(a.updated_at, a.id)),
        DocumentOrder::UpdatedAsc => (a.updated_at, a.id).cmp(&(b.updated_at, b.id)),
        DocumentOrder::TitleAsc => (a.title.as_str(), a.id).cmp(&(b.title.as_str(), b.id)),
        DocumentOrder::TitleDesc => (b.title.as_str(), b.id).cmp(&(a.title.as_str(), a.id)),
    }
}

fn list_item(stored: StoredDocument, project: &Project) -> DocumentListItem {
    let StoredDocument {
        document,
        current_number,
        current_message,
        current_author,
        ..
    } = stored;
    DocumentListItem {
        id: document.id,
        slug: document.slug,
        title: document.title,
        project_id: project.id,
        project_slug: project.slug.clone(),
        project_name: project.name.clone(),
        project_color: project.color,
        revision_number: current_number,
        revision_message: current_message,
        author_name: current_author,
        archived_at: document.archived_at,
        updated_at: document.updated_at,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use jiff::Timestamp;

    use super::*;
    use crate::domain::{
        document::{Document, DocumentSlug, DocumentTitle},
        revision::{AuthorName, RevisionMessage, RevisionNumber},
        shared::{DocumentId, RevisionId},
    };

    fn stored(project_id: ProjectId, title: &str, second: i64, authors: &[&str]) -> StoredDocument {
        let at = Timestamp::from_second(1_790_000_000 + second).unwrap();
        StoredDocument {
            document: Document {
                id: DocumentId::generate(),
                project_id,
                slug: DocumentSlug::parse("doc").unwrap(),
                title: DocumentTitle::parse(title).unwrap(),
                current_revision_id: RevisionId::generate(),
                archived_at: None,
                created_at: at,
                updated_at: at,
            },
            current_number: RevisionNumber::FIRST,
            current_message: RevisionMessage::parse("初版").unwrap(),
            current_author: AuthorName::parse(authors[0]).unwrap(),
            author_names: authors
                .iter()
                .map(|a| (*a).to_owned())
                .collect::<BTreeSet<_>>(),
        }
    }

    fn titles(documents: &[StoredDocument]) -> Vec<&str> {
        documents
            .iter()
            .map(|d| d.document.title.as_str())
            .collect()
    }

    #[test]
    fn title_filter_ignores_ascii_case_only() {
        let project = ProjectId::generate();
        let documents = vec![
            stored(project, "Sales Plan", 1, &["佐藤"]),
            stored(project, "営業計画（ＳＡＬＥＳ）", 2, &["佐藤"]),
            stored(project, "100%_done", 3, &["佐藤"]),
        ];
        let query = |text: &str| DocumentQuery {
            title_contains: Some(text.to_owned()),
            order: DocumentOrder::UpdatedAsc,
            ..DocumentQuery::default()
        };
        assert_eq!(
            titles(&select(documents.clone(), &query("sales"))),
            ["Sales Plan"]
        );
        // 全角の英字は大文字と小文字を区別する（SQLite の LIKE と同じ）
        assert_eq!(
            titles(&select(documents.clone(), &query("ｓａｌｅｓ"))),
            Vec::<&str>::new()
        );
        // `%` `_` はワイルドカードではなく文字として扱う
        assert_eq!(
            titles(&select(documents.clone(), &query("%_"))),
            ["100%_done"]
        );
        assert_eq!(titles(&select(documents, &query(""))).len(), 3);
    }

    #[test]
    fn filters_by_author_and_updated_since_then_orders_and_limits() {
        let project = ProjectId::generate();
        let documents = vec![
            stored(project, "A", 1, &["佐藤"]),
            stored(project, "B", 2, &["田中", "佐藤"]),
            stored(project, "C", 3, &["田中"]),
            stored(project, "D", 4, &["佐藤"]),
        ];
        let query = DocumentQuery {
            author_name: Some(AuthorName::parse("佐藤").unwrap()),
            updated_since: Some(Timestamp::from_second(1_790_000_002).unwrap()),
            ..DocumentQuery::default()
        };
        assert_eq!(titles(&select(documents.clone(), &query)), ["D", "B"]);
        let limited = DocumentQuery {
            order: DocumentOrder::TitleDesc,
            limit: Some(2),
            ..DocumentQuery::default()
        };
        assert_eq!(titles(&select(documents, &limited)), ["D", "C"]);
    }

    #[test]
    fn archived_documents_are_listed_separately() {
        let project = ProjectId::generate();
        let mut archived = stored(project, "古い資料", 1, &["佐藤"]);
        archived.document.archived_at = Some(Timestamp::from_second(1_790_000_009).unwrap());
        let documents = vec![archived, stored(project, "新しい資料", 2, &["佐藤"])];
        assert_eq!(
            titles(&select(documents.clone(), &DocumentQuery::default())),
            ["新しい資料"]
        );
        let query = DocumentQuery {
            archive: ArchiveFilter::Archived,
            ..DocumentQuery::default()
        };
        assert_eq!(titles(&select(documents, &query)), ["古い資料"]);
    }

    #[test]
    fn same_updated_at_is_ordered_by_id() {
        let project = ProjectId::generate();
        let first = stored(project, "先", 1, &["佐藤"]);
        let mut second = stored(project, "後", 1, &["佐藤"]);
        // UUID v7 なので後から作った ID のほうが大きい
        second.document.updated_at = first.document.updated_at;
        let documents = vec![first, second];
        assert_eq!(
            titles(&select(documents, &DocumentQuery::default())),
            ["後", "先"]
        );
    }
}
