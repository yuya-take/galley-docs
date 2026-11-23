//! 項目のキー（`pk`・`sk`）。テーブルの設計は `docs/design.md` の「DynamoDB のテーブル」。
//!
//! ID は UUID の文字列（ハイフン区切り）、版の番号は並び順が番号の順になるよう 10 桁にそろえる。

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use crate::domain::{
    blob::BlobHash,
    document::DocumentSlug,
    project::ProjectSlug,
    revision::RevisionNumber,
    shared::{DocumentId, ProjectId, RevisionId},
};

/// パーティションキーの属性名。
pub(super) const PK: &str = "pk";
/// ソートキーの属性名。
pub(super) const SK: &str = "sk";

/// プロジェクトとその slug の印を置くパーティション。
pub(super) const PROJECTS: &str = "PROJECTS";
/// プロジェクトの `sk` の接頭辞。
pub(super) const PROJECT_PREFIX: &str = "PROJECT#";
/// 資料の `sk` の接頭辞（`PROJECT#<id>` のパーティションの中）。
pub(super) const DOCUMENT_PREFIX: &str = "DOCUMENT#";
/// 版の `sk` の接頭辞（`DOCUMENT#<id>`・`BLOB#<hash>` のパーティションの中）。
pub(super) const REVISION_PREFIX: &str = "REVISION#";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Key {
    pub pk: String,
    pub sk: String,
}

impl Key {
    fn new(pk: impl Into<String>, sk: impl Into<String>) -> Self {
        Self {
            pk: pk.into(),
            sk: sk.into(),
        }
    }

    pub fn to_item(&self) -> HashMap<String, AttributeValue> {
        HashMap::from([
            (PK.to_owned(), AttributeValue::S(self.pk.clone())),
            (SK.to_owned(), AttributeValue::S(self.sk.clone())),
        ])
    }

    /// プロジェクト。
    pub fn project(id: ProjectId) -> Self {
        Self::new(PROJECTS, format!("{PROJECT_PREFIX}{}", id.as_uuid()))
    }

    /// プロジェクトの slug の一意性を守る印。
    pub fn project_slug(slug: &ProjectSlug) -> Self {
        Self::new(PROJECTS, format!("PROJECT_SLUG#{}", slug.as_str()))
    }

    /// 資料。資料の slug は変わらないので、プロジェクトの中で slug を `sk` にして一意性を守る。
    pub fn document(project_id: ProjectId, slug: &DocumentSlug) -> Self {
        Self::new(
            project_documents(project_id),
            format!("{DOCUMENT_PREFIX}{}", slug.as_str()),
        )
    }

    /// 資料の ID から資料のキー（プロジェクトと slug）を引くための印。
    pub fn document_ref(id: DocumentId) -> Self {
        Self::new(document_revisions(id), "DOCUMENT")
    }

    /// 版（資料のパーティションの中で、番号の順に並ぶ）。
    pub fn revision(document_id: DocumentId, number: RevisionNumber) -> Self {
        Self::new(
            document_revisions(document_id),
            format!("{REVISION_PREFIX}{:010}", number.get()),
        )
    }

    /// 版の写し（版の ID から引く）。
    pub fn revision_by_id(id: RevisionId) -> Self {
        Self::new(format!("REVISION#{}", id.as_uuid()), "REVISION")
    }

    /// 版の写し（ブロブから、そのブロブを使っている版を引く）。
    pub fn revision_by_blob(hash: &BlobHash, id: RevisionId) -> Self {
        Self::new(
            blob_partition(hash),
            format!("{REVISION_PREFIX}{}", id.as_uuid()),
        )
    }

    /// ブロブの情報と参照の数。
    pub fn blob(hash: &BlobHash) -> Self {
        Self::new(blob_partition(hash), "BLOB")
    }
}

/// プロジェクトの資料を置くパーティション。
pub(super) fn project_documents(project_id: ProjectId) -> String {
    format!("PROJECT#{}", project_id.as_uuid())
}

/// 資料の版を置くパーティション。
pub(super) fn document_revisions(document_id: DocumentId) -> String {
    format!("DOCUMENT#{}", document_id.as_uuid())
}

/// ブロブと、それを使う版の写しを置くパーティション。
pub(super) fn blob_partition(hash: &BlobHash) -> String {
    format!("BLOB#{}", hash.as_str())
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    #[test]
    fn revision_numbers_sort_by_number() {
        let document = DocumentId::from_uuid(Uuid::nil());
        let key = |n| Key::revision(document, RevisionNumber::new(n).unwrap()).sk;
        assert_eq!(key(9), "REVISION#0000000009");
        assert!(key(9) < key(10));
        assert_eq!(key(u32::MAX), "REVISION#4294967295");
    }

    #[test]
    fn slug_markers_do_not_match_project_prefix() {
        let slug = ProjectSlug::parse("sales").unwrap();
        let marker = Key::project_slug(&slug);
        assert_eq!(marker.pk, PROJECTS);
        assert!(!marker.sk.starts_with(PROJECT_PREFIX));
        assert!(
            Key::project(ProjectId::from_uuid(Uuid::nil()))
                .sk
                .starts_with(PROJECT_PREFIX)
        );
    }

    #[test]
    fn document_ref_shares_partition_with_revisions_but_not_prefix() {
        let id = DocumentId::from_uuid(Uuid::nil());
        let reference = Key::document_ref(id);
        let revision = Key::revision(id, RevisionNumber::FIRST);
        assert_eq!(reference.pk, revision.pk);
        assert!(!reference.sk.starts_with(REVISION_PREFIX));
    }
}
