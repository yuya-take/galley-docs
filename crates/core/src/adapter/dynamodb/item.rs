//! 項目と domain の型の変換。項目は adapter の外に出さず、domain の型にして返す。
//!
//! 日時は SQLite と同じく小数9桁固定の RFC 3339（UTC）の文字列にする（文字列の順と時刻の順が一致する）。
//! 値が無い（`archived_at` が `None` など）ときは属性を置かない。

use std::collections::BTreeSet;

use aws_sdk_dynamodb::types::AttributeValue;
use jiff::Timestamp;
use uuid::Uuid;

use crate::domain::{
    blob::{Blob, BlobHash},
    document::{Document, DocumentSlug, DocumentTitle},
    error::RepositoryError,
    project::{CrestColor, Project, ProjectDescription, ProjectName, ProjectSlug},
    revision::{AuthorName, Revision, RevisionMessage, RevisionNumber, RevisionSource},
    shared::{DocumentId, ProjectId, RevisionId},
};

use super::{keys::Key, table::Item};

pub(super) fn string(value: &str) -> AttributeValue {
    AttributeValue::S(value.to_owned())
}

pub(super) fn number(value: impl std::fmt::Display) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

pub(super) fn uuid(value: Uuid) -> AttributeValue {
    AttributeValue::S(value.to_string())
}

pub(super) fn timestamp(at: Timestamp) -> AttributeValue {
    AttributeValue::S(format!("{at:.9}"))
}

/// 名前の集合（`author_names`）。キーを名前、値を `true` にしたマップで持つ。
///
/// 文字列の集合（SS）にしないのは、DynamoDB Local（3.3）が、日本語の文字列の集合にすでにある要素を
/// `ADD` すると、以後の `ADD` を「Input collection contains duplicates」で断るため。マップなら
/// `SET author_names.#name = :true` で1人ずつ足せる。
pub(super) fn name_set(names: impl IntoIterator<Item = String>) -> AttributeValue {
    AttributeValue::M(
        names
            .into_iter()
            .map(|name| (name, AttributeValue::Bool(true)))
            .collect(),
    )
}

/// 保存済みの値を domain の型に読み直せなかったときのエラー。
pub(super) fn corrupted(attribute: &str, err: impl std::fmt::Display) -> RepositoryError {
    RepositoryError::Corrupted(format!("{attribute}: {err}"))
}

/// 項目から属性を読む。
pub(super) struct Reader<'a> {
    item: &'a Item,
    /// エラーに出す項目の種類（`projects` など）。
    kind: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(item: &'a Item, kind: &'static str) -> Self {
        Self { item, kind }
    }

    fn corrupted(&self, attribute: &str, err: impl std::fmt::Display) -> RepositoryError {
        corrupted(&format!("{}.{attribute}", self.kind), err)
    }

    pub fn optional_string(&self, attribute: &str) -> Result<Option<&'a str>, RepositoryError> {
        match self.item.get(attribute) {
            None | Some(AttributeValue::Null(_)) => Ok(None),
            Some(AttributeValue::S(value)) => Ok(Some(value)),
            Some(other) => Err(self.corrupted(attribute, format!("{other:?}"))),
        }
    }

    pub fn string(&self, attribute: &str) -> Result<&'a str, RepositoryError> {
        self.optional_string(attribute)?
            .ok_or_else(|| self.corrupted(attribute, "ありません"))
    }

    pub fn optional_number<T>(&self, attribute: &str) -> Result<Option<T>, RepositoryError>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        match self.item.get(attribute) {
            None | Some(AttributeValue::Null(_)) => Ok(None),
            Some(AttributeValue::N(value)) => value
                .parse()
                .map(Some)
                .map_err(|e| self.corrupted(attribute, e)),
            Some(other) => Err(self.corrupted(attribute, format!("{other:?}"))),
        }
    }

    pub fn number<T>(&self, attribute: &str) -> Result<T, RepositoryError>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        self.optional_number(attribute)?
            .ok_or_else(|| self.corrupted(attribute, "ありません"))
    }

    pub fn uuid(&self, attribute: &str) -> Result<Uuid, RepositoryError> {
        self.string(attribute)?
            .parse()
            .map_err(|e| self.corrupted(attribute, e))
    }

    pub fn optional_timestamp(
        &self,
        attribute: &str,
    ) -> Result<Option<Timestamp>, RepositoryError> {
        self.optional_string(attribute)?
            .map(|text| text.parse().map_err(|e| self.corrupted(attribute, e)))
            .transpose()
    }

    pub fn timestamp(&self, attribute: &str) -> Result<Timestamp, RepositoryError> {
        self.optional_timestamp(attribute)?
            .ok_or_else(|| self.corrupted(attribute, "ありません"))
    }

    /// 名前の集合（[`name_set`] で書いたマップのキー）。
    pub fn name_set(&self, attribute: &str) -> Result<BTreeSet<String>, RepositoryError> {
        match self.item.get(attribute) {
            None | Some(AttributeValue::Null(_)) => Ok(BTreeSet::new()),
            Some(AttributeValue::M(names)) => Ok(names.keys().cloned().collect()),
            Some(other) => Err(self.corrupted(attribute, format!("{other:?}"))),
        }
    }

    /// 値を読んで domain の型にする。
    pub fn parse<T, E: std::fmt::Display>(
        &self,
        attribute: &str,
        parse: impl FnOnce(&str) -> Result<T, E>,
    ) -> Result<T, RepositoryError> {
        parse(self.string(attribute)?).map_err(|e| self.corrupted(attribute, e))
    }
}

/// キーと属性から項目を作る。
fn item(key: Key, attributes: impl IntoIterator<Item = (&'static str, AttributeValue)>) -> Item {
    let mut item = key.to_item();
    item.extend(
        attributes
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value)),
    );
    item
}

/// 値があるときだけ属性を置く。
fn optional(
    name: &'static str,
    value: Option<AttributeValue>,
) -> impl Iterator<Item = (&'static str, AttributeValue)> {
    value.map(|value| (name, value)).into_iter()
}

// ---- プロジェクト ----

pub(super) fn project_item(project: &Project) -> Item {
    item(
        Key::project(project.id),
        [
            ("id", uuid(project.id.as_uuid())),
            ("slug", string(project.slug.as_str())),
            ("name", string(project.name.as_str())),
            ("description", string(project.description.as_str())),
            ("color", string(project.color.as_str())),
            ("created_at", timestamp(project.created_at)),
        ]
        .into_iter()
        .chain(optional("archived_at", project.archived_at.map(timestamp))),
    )
}

pub(super) fn project_slug_item(project: &Project) -> Item {
    item(
        Key::project_slug(&project.slug),
        [("project_id", uuid(project.id.as_uuid()))],
    )
}

impl TryFrom<&Item> for Project {
    type Error = RepositoryError;

    fn try_from(item: &Item) -> Result<Self, Self::Error> {
        let r = Reader::new(item, "projects");
        Ok(Self {
            id: ProjectId::from_uuid(r.uuid("id")?),
            slug: r.parse("slug", ProjectSlug::parse)?,
            name: r.parse("name", ProjectName::parse)?,
            description: r.parse("description", ProjectDescription::parse)?,
            color: r.parse("color", CrestColor::parse)?,
            archived_at: r.optional_timestamp("archived_at")?,
            created_at: r.timestamp("created_at")?,
        })
    }
}

// ---- 資料 ----

/// 資料の項目。資料の一覧のために、現在版の番号・変更メモ・更新者と、版を登録した人の集合も持つ。
/// 現在版は版を追加したときにだけ変わるので、版の追加と同じトランザクションで書けば食い違わない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StoredDocument {
    pub document: Document,
    pub current_number: RevisionNumber,
    pub current_message: RevisionMessage,
    pub current_author: AuthorName,
    /// この資料に版を登録したことがある人（「自分が更新」の絞り込み）。
    pub author_names: BTreeSet<String>,
}

pub(super) fn document_item(document: &Document, first: &Revision) -> Item {
    item(
        Key::document(document.project_id, &document.slug),
        [
            ("id", uuid(document.id.as_uuid())),
            ("project_id", uuid(document.project_id.as_uuid())),
            ("slug", string(document.slug.as_str())),
            ("title", string(document.title.as_str())),
            (
                "current_revision_id",
                uuid(document.current_revision_id.as_uuid()),
            ),
            ("current_revision_number", number(first.number.get())),
            ("current_revision_message", string(first.message.as_str())),
            (
                "current_revision_author_name",
                string(first.author_name.as_str()),
            ),
            (
                "author_names",
                name_set([first.author_name.as_str().to_owned()]),
            ),
            ("created_at", timestamp(document.created_at)),
            ("updated_at", timestamp(document.updated_at)),
        ]
        .into_iter()
        .chain(optional("archived_at", document.archived_at.map(timestamp))),
    )
}

/// 資料の ID から資料のキーを引くための印。プロジェクトと slug は変わらない。
pub(super) fn document_ref_item(document: &Document) -> Item {
    item(
        Key::document_ref(document.id),
        [
            ("document_id", uuid(document.id.as_uuid())),
            ("project_id", uuid(document.project_id.as_uuid())),
            ("slug", string(document.slug.as_str())),
        ],
    )
}

/// 資料の印から、資料のキーを読む。
pub(super) fn document_key_from_ref(item: &Item) -> Result<Key, RepositoryError> {
    let r = Reader::new(item, "document_refs");
    Ok(Key::document(
        ProjectId::from_uuid(r.uuid("project_id")?),
        &r.parse("slug", DocumentSlug::parse)?,
    ))
}

impl TryFrom<&Item> for StoredDocument {
    type Error = RepositoryError;

    fn try_from(item: &Item) -> Result<Self, Self::Error> {
        let r = Reader::new(item, "documents");
        Ok(Self {
            document: Document {
                id: DocumentId::from_uuid(r.uuid("id")?),
                project_id: ProjectId::from_uuid(r.uuid("project_id")?),
                slug: r.parse("slug", DocumentSlug::parse)?,
                title: r.parse("title", DocumentTitle::parse)?,
                current_revision_id: RevisionId::from_uuid(r.uuid("current_revision_id")?),
                archived_at: r.optional_timestamp("archived_at")?,
                created_at: r.timestamp("created_at")?,
                updated_at: r.timestamp("updated_at")?,
            },
            current_number: RevisionNumber::new(r.number("current_revision_number")?)
                .map_err(|e| r.corrupted("current_revision_number", e))?,
            current_message: r.parse("current_revision_message", RevisionMessage::parse)?,
            current_author: r.parse("current_revision_author_name", AuthorName::parse)?,
            author_names: r.name_set("author_names")?,
        })
    }
}

// ---- 版 ----

/// 版の項目。版は作ったあと変わらないので、同じ属性の写しを3つのキーに置く。
pub(super) fn revision_items(revision: &Revision) -> [Item; 3] {
    [
        Key::revision(revision.document_id, revision.number),
        Key::revision_by_id(revision.id),
        Key::revision_by_blob(&revision.blob_hash, revision.id),
    ]
    .map(|key| revision_item(key, revision))
}

/// 版の写しのキー（[`revision_items`] と同じ順）。
pub(super) fn revision_keys(revision: &Revision) -> [Key; 3] {
    [
        Key::revision(revision.document_id, revision.number),
        Key::revision_by_id(revision.id),
        Key::revision_by_blob(&revision.blob_hash, revision.id),
    ]
}

fn revision_item(key: Key, revision: &Revision) -> Item {
    item(
        key,
        [
            ("id", uuid(revision.id.as_uuid())),
            ("document_id", uuid(revision.document_id.as_uuid())),
            ("number", number(revision.number.get())),
            ("blob_hash", string(revision.blob_hash.as_str())),
            ("message", string(revision.message.as_str())),
            ("author_name", string(revision.author_name.as_str())),
            ("source", string(revision.source.as_str())),
            ("created_at", timestamp(revision.created_at)),
        ]
        .into_iter()
        .chain(optional(
            "restored_from_number",
            revision.restored_from.map(|n| number(n.get())),
        )),
    )
}

impl TryFrom<&Item> for Revision {
    type Error = RepositoryError;

    fn try_from(item: &Item) -> Result<Self, Self::Error> {
        let r = Reader::new(item, "revisions");
        Ok(Self {
            id: RevisionId::from_uuid(r.uuid("id")?),
            document_id: DocumentId::from_uuid(r.uuid("document_id")?),
            number: RevisionNumber::new(r.number("number")?)
                .map_err(|e| r.corrupted("number", e))?,
            blob_hash: r.parse("blob_hash", BlobHash::parse)?,
            message: r.parse("message", RevisionMessage::parse)?,
            author_name: r.parse("author_name", AuthorName::parse)?,
            source: r.parse("source", RevisionSource::parse)?,
            restored_from: r
                .optional_number("restored_from_number")?
                .map(RevisionNumber::new)
                .transpose()
                .map_err(|e| r.corrupted("restored_from_number", e))?,
            created_at: r.timestamp("created_at")?,
        })
    }
}

// ---- ブロブ ----

/// ブロブの情報を、無ければ作り、参照の数を 1 増やす更新式。同じハッシュがあれば大きさと
/// 作成日時は変えない（SQLite の「同じハッシュがあれば保存しない」と同じ）。
pub(super) fn blob_upsert(blob: &Blob) -> super::table::Expression {
    super::table::Expression::new(
        "SET #hash = :hash, #size = if_not_exists(#size, :size), \
         #created_at = if_not_exists(#created_at, :created_at) \
         ADD #ref_count :one",
    )
    .name("#hash", "hash")
    .name("#size", "size")
    .name("#created_at", "created_at")
    .name("#ref_count", REF_COUNT)
    .value(":hash", string(blob.hash.as_str()))
    .value(":size", number(blob.size))
    .value(":created_at", timestamp(blob.created_at))
    .value(":one", number(1))
}

/// ブロブを使う版の数。版を足すと増え、物理削除で減る。物理削除はこの値を読んでから、
/// 読んだ値のままであることを条件に書く（楽観ロック）。
pub(super) const REF_COUNT: &str = "ref_count";

/// ブロブの項目から参照の数を読む。
pub(super) fn ref_count(item: &Item) -> Result<u64, RepositoryError> {
    Reader::new(item, "blobs").number(REF_COUNT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision() -> Revision {
        Revision {
            id: RevisionId::generate(),
            document_id: DocumentId::generate(),
            number: RevisionNumber::new(3).unwrap(),
            blob_hash: BlobHash::parse(&"a".repeat(64)).unwrap(),
            message: RevisionMessage::parse("第1版の内容に戻す").unwrap(),
            author_name: AuthorName::parse("佐藤").unwrap(),
            source: RevisionSource::Mcp,
            restored_from: Some(RevisionNumber::FIRST),
            created_at: Timestamp::from_second(1_790_000_000).unwrap(),
        }
    }

    #[test]
    fn revision_copies_round_trip() {
        let revision = revision();
        for item in revision_items(&revision) {
            assert_eq!(Revision::try_from(&item).unwrap(), revision);
        }
        let keys: Vec<Key> = revision_keys(&revision).into();
        let items: Vec<Key> = revision_items(&revision)
            .iter()
            .map(|item| Key {
                pk: Reader::new(item, "t").string("pk").unwrap().to_owned(),
                sk: Reader::new(item, "t").string("sk").unwrap().to_owned(),
            })
            .collect();
        assert_eq!(keys, items);
    }

    #[test]
    fn project_round_trips_with_and_without_archive() {
        let mut project = Project::create(
            crate::domain::project::ProjectProfile {
                slug: ProjectSlug::parse("sales").unwrap(),
                name: ProjectName::parse("営業部").unwrap(),
                description: ProjectDescription::parse("").unwrap(),
                color: CrestColor::parse("blue").unwrap(),
            },
            Timestamp::from_second(1_790_000_000).unwrap(),
        );
        let item = project_item(&project);
        assert!(!item.contains_key("archived_at"));
        assert_eq!(Project::try_from(&item).unwrap(), project);

        project.archive(Timestamp::from_second(1_790_000_100).unwrap());
        assert_eq!(Project::try_from(&project_item(&project)).unwrap(), project);
    }

    #[test]
    fn timestamps_keep_nanoseconds_and_sort_as_text() {
        let earlier = Timestamp::from_second(1_790_000_000).unwrap();
        let later = earlier + jiff::SignedDuration::from_nanos(1);
        let (AttributeValue::S(a), AttributeValue::S(b)) = (timestamp(earlier), timestamp(later))
        else {
            panic!("文字列ではありません");
        };
        assert!(a < b);
        assert_eq!(b.parse::<Timestamp>().unwrap(), later);
    }

    #[test]
    fn missing_attribute_is_corrupted() {
        let mut item = project_item(&Project::create(
            crate::domain::project::ProjectProfile {
                slug: ProjectSlug::parse("sales").unwrap(),
                name: ProjectName::parse("営業部").unwrap(),
                description: ProjectDescription::parse("").unwrap(),
                color: CrestColor::parse("blue").unwrap(),
            },
            Timestamp::UNIX_EPOCH,
        ));
        item.remove("name");
        assert!(matches!(
            Project::try_from(&item),
            Err(RepositoryError::Corrupted(message)) if message.starts_with("projects.name")
        ));
    }
}
