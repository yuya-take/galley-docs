use std::sync::Arc;

use crate::{
    app::error::AppError,
    domain::blob::{BlobContent, BlobHash, BlobStore},
};

/// ハッシュで実体を読む。資料配信の `/b/<hash>` が使う。
///
/// DB は見ず、ストレージにあれば返す。AWS の構成では CloudFront が S3 から直接配るので、それと同じ
/// 振る舞いにそろえる。読んだ中身がハッシュと一致するかは [`BlobStore::get`] が確かめる。
pub struct ReadBlob {
    blobs: Arc<dyn BlobStore>,
}

impl ReadBlob {
    pub fn new(blobs: Arc<dyn BlobStore>) -> Self {
        Self { blobs }
    }

    /// 実体が無ければ `None`。
    pub async fn execute(&self, hash: &BlobHash) -> Result<Option<BlobContent>, AppError> {
        Ok(self.blobs.get(hash).await?)
    }
}
