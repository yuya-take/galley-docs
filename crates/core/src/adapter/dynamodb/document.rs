use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;

use crate::domain::{
    blob::{Blob, BlobHash},
    document::{
        Document, DocumentListItem, DocumentQuery, DocumentRepository, DocumentSlug,
        PurgedRevisions,
    },
    error::RepositoryError,
    project::Project,
    revision::Revision,
    shared::{DocumentId, ProjectId, RevisionId},
};

use super::{
    item::{self, Reader, StoredDocument},
    keys::{DOCUMENT_PREFIX, Key, project_documents},
    list, purge,
    table::{Expression, Order, Table, Write},
};

#[derive(Debug, Clone)]
pub struct DynamoDbDocumentRepository {
    table: Table,
}

impl DynamoDbDocumentRepository {
    pub(super) fn new(table: Table) -> Self {
        Self { table }
    }
}

/// 資料の ID から資料を読む（資料の印でキーを引いてから、資料を読む）。
pub(super) async fn load_document(
    table: &Table,
    id: DocumentId,
) -> Result<Option<StoredDocument>, RepositoryError> {
    let Some(reference) = table.get(&Key::document_ref(id)).await? else {
        return Ok(None);
    };
    let key = item::document_key_from_ref(&reference)?;
    table
        .get(&key)
        .await?
        .as_ref()
        .map(StoredDocument::try_from)
        .transpose()
}

/// 版の写し3つを書く。資料のパーティションの写しは、同じ番号の版が無いことを条件にする
/// （同時に追加された版と番号が重なれば `Conflict`）。
fn revision_writes(revision: &Revision) -> impl Iterator<Item = Write> {
    let [by_number, by_id, by_blob] = item::revision_items(revision);
    [
        Write::put(by_number).when(Expression::not_exists()),
        Write::put(by_id),
        Write::put(by_blob),
    ]
    .into_iter()
}

/// ブロブの情報を書き、参照の数を 1 増やす。同じハッシュがあれば大きさと作成日時は変えない。
fn blob_write(blob: &Blob) -> Write {
    Write::update(Key::blob(&blob.hash), item::blob_upsert(blob))
}

/// 戻す操作で、戻す元の版のブロブの参照を 1 増やす。ブロブの情報が消されていれば `Conflict`
/// （戻す元の版が物理削除され、実体も消される）。
fn reuse_blob_write(hash: &BlobHash) -> Write {
    Write::update(
        Key::blob(hash),
        Expression::new("ADD #ref_count :one")
            .name("#ref_count", item::REF_COUNT)
            .value(":one", item::number(1)),
    )
    .when(Expression::exists())
}

#[async_trait]
impl DocumentRepository for DynamoDbDocumentRepository {
    /// 資料（slug が使われていれば `Conflict`）、資料の印、第1版の写し、ブロブを1トランザクションで書く。
    async fn insert_with_first_revision(
        &self,
        document: &Document,
        revision: &Revision,
        blob: &Blob,
    ) -> Result<(), RepositoryError> {
        let mut writes = vec![
            Write::put(item::document_item(document, revision)).when(Expression::not_exists()),
            Write::put(item::document_ref_item(document)).when(Expression::not_exists()),
        ];
        writes.extend(revision_writes(revision));
        writes.push(blob_write(blob));
        self.table.transact(writes).await
    }

    /// 版の写し、資料の現在版、ブロブの参照を1トランザクションで書く。
    ///
    /// 資料が消されていれば `Conflict`。資料の現在版の番号が新しい版の番号より小さいことも条件にする
    /// （古い番号の版で現在版を巻き戻さない）。
    async fn insert_revision(
        &self,
        document: &Document,
        revision: &Revision,
        blob: Option<&Blob>,
    ) -> Result<(), RepositoryError> {
        let update = Expression::new(
            "SET #current_revision_id = :revision_id, #current_revision_number = :number, \
             #current_revision_message = :message, #current_revision_author_name = :author, \
             #updated_at = :updated_at, #author_names.#author_key = :true",
        )
        .name("#current_revision_id", "current_revision_id")
        .name("#current_revision_number", "current_revision_number")
        .name("#current_revision_message", "current_revision_message")
        .name(
            "#current_revision_author_name",
            "current_revision_author_name",
        )
        .name("#updated_at", "updated_at")
        .name("#author_names", "author_names")
        .value(":revision_id", item::uuid(revision.id.as_uuid()))
        .value(":number", item::number(revision.number.get()))
        .value(":message", item::string(revision.message.as_str()))
        .value(":author", item::string(revision.author_name.as_str()))
        .value(":updated_at", item::timestamp(document.updated_at))
        .name("#author_key", revision.author_name.as_str())
        .value(":true", AttributeValue::Bool(true));
        let condition = Expression::new(format!(
            "{} AND #current_revision_number < :number",
            Expression::exists().text()
        ))
        .name("#current_revision_number", "current_revision_number")
        .value(":number", item::number(revision.number.get()));

        let mut writes = vec![
            Write::update(Key::document(document.project_id, &document.slug), update)
                .when(condition),
        ];
        writes.extend(revision_writes(revision));
        writes.push(match blob {
            Some(blob) => blob_write(blob),
            None => reuse_blob_write(&revision.blob_hash),
        });
        self.table.transact(writes).await
    }

    /// 資料名とアーカイブの状態を書く。資料が無ければ何もしない（SQLite の実装と同じ。条件を付けないと
    /// DynamoDB は属性の欠けた項目を作ってしまう）。
    async fn update(&self, document: &Document) -> Result<(), RepositoryError> {
        let update = match document.archived_at {
            Some(at) => Expression::new("SET #title = :title, #archived_at = :archived_at")
                .value(":archived_at", item::timestamp(at)),
            None => Expression::new("SET #title = :title REMOVE #archived_at"),
        }
        .name("#title", "title")
        .name("#archived_at", "archived_at")
        .value(":title", item::string(document.title.as_str()));
        self.table
            .update_if(
                &Key::document(document.project_id, &document.slug),
                update,
                Expression::exists(),
            )
            .await?;
        Ok(())
    }

    async fn find_by_id(&self, id: DocumentId) -> Result<Option<Document>, RepositoryError> {
        Ok(load_document(&self.table, id)
            .await?
            .map(|stored| stored.document))
    }

    async fn find_with_project(
        &self,
        id: DocumentId,
    ) -> Result<Option<(Document, Project)>, RepositoryError> {
        let Some(document) = self.find_by_id(id).await? else {
            return Ok(None);
        };
        let project = self
            .table
            .get(&Key::project(document.project_id))
            .await?
            .ok_or_else(|| {
                item::corrupted(
                    "documents.project_id",
                    format!("資料 {} のプロジェクトがありません", document.id),
                )
            })?;
        Ok(Some((document, Project::try_from(&project)?)))
    }

    async fn find_by_slug(
        &self,
        project_id: ProjectId,
        slug: &DocumentSlug,
    ) -> Result<Option<Document>, RepositoryError> {
        Ok(self
            .table
            .get(&Key::document(project_id, slug))
            .await?
            .as_ref()
            .map(StoredDocument::try_from)
            .transpose()?
            .map(|stored| stored.document))
    }

    /// 資料の `sk` は `DOCUMENT#<slug>` なので、slug の前方一致は `sk` の前方一致の Query で読める。
    async fn slugs_with_prefix(
        &self,
        project_id: ProjectId,
        prefix: &str,
    ) -> Result<Vec<DocumentSlug>, RepositoryError> {
        self.table
            .query(
                &project_documents(project_id),
                &format!("{DOCUMENT_PREFIX}{prefix}"),
                Order::Ascending,
                None,
            )
            .await?
            .iter()
            .map(|item| Reader::new(item, "documents").parse("slug", DocumentSlug::parse))
            .collect()
    }

    async fn list(&self, query: &DocumentQuery) -> Result<Vec<DocumentListItem>, RepositoryError> {
        list::list(&self.table, query).await
    }

    async fn purge(&self, id: DocumentId) -> Result<PurgedRevisions, RepositoryError> {
        purge::purge_document(&self.table, id).await
    }

    async fn purge_revision(&self, id: RevisionId) -> Result<PurgedRevisions, RepositoryError> {
        purge::purge_revision(&self.table, id).await
    }
}
