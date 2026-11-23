use async_trait::async_trait;

use crate::domain::{
    blob::BlobHash,
    error::RepositoryError,
    revision::{Revision, RevisionNumber, RevisionRepository},
    shared::{DocumentId, RevisionId},
};

use super::{
    keys::{Key, REVISION_PREFIX, blob_partition, document_revisions},
    table::{Order, Table},
};

#[derive(Debug, Clone)]
pub struct DynamoDbRevisionRepository {
    table: Table,
}

impl DynamoDbRevisionRepository {
    pub(super) fn new(table: Table) -> Self {
        Self { table }
    }
}

/// 資料の版を番号の順に読む。
pub(super) async fn load_revisions(
    table: &Table,
    document_id: DocumentId,
    order: Order,
    limit: Option<usize>,
) -> Result<Vec<Revision>, RepositoryError> {
    table
        .query(
            &document_revisions(document_id),
            REVISION_PREFIX,
            order,
            limit,
        )
        .await?
        .iter()
        .map(Revision::try_from)
        .collect()
}

/// ブロブを使っている版を読む（ブロブのパーティションにある版の写し）。
pub(super) async fn load_blob_users(
    table: &Table,
    hash: &BlobHash,
) -> Result<Vec<Revision>, RepositoryError> {
    table
        .query(
            &blob_partition(hash),
            REVISION_PREFIX,
            Order::Ascending,
            None,
        )
        .await?
        .iter()
        .map(Revision::try_from)
        .collect()
}

#[async_trait]
impl RevisionRepository for DynamoDbRevisionRepository {
    async fn find_by_id(&self, id: RevisionId) -> Result<Option<Revision>, RepositoryError> {
        self.table
            .get(&Key::revision_by_id(id))
            .await?
            .as_ref()
            .map(Revision::try_from)
            .transpose()
    }

    async fn find_by_number(
        &self,
        document_id: DocumentId,
        number: RevisionNumber,
    ) -> Result<Option<Revision>, RepositoryError> {
        self.table
            .get(&Key::revision(document_id, number))
            .await?
            .as_ref()
            .map(Revision::try_from)
            .transpose()
    }

    async fn list_by_document(
        &self,
        document_id: DocumentId,
        limit: Option<u32>,
    ) -> Result<Vec<Revision>, RepositoryError> {
        // u32 は usize に収まる（Galley は 32 ビット以上で動かす）
        let limit = limit.map(|limit| limit as usize);
        load_revisions(&self.table, document_id, Order::Descending, limit).await
    }

    async fn list_by_blob_hashes(
        &self,
        hashes: &[BlobHash],
    ) -> Result<Vec<Revision>, RepositoryError> {
        let mut seen: Vec<&BlobHash> = Vec::new();
        let mut revisions = Vec::new();
        for hash in hashes {
            if seen.contains(&hash) {
                continue;
            }
            seen.push(hash);
            revisions.extend(load_blob_users(&self.table, hash).await?);
        }
        // SQLite の実装と同じく、番号の新しい順にする
        revisions.sort_by(|a, b| b.number.cmp(&a.number).then(b.id.cmp(&a.id)));
        Ok(revisions)
    }
}
