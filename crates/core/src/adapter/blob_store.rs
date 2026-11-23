//! object_store による資料の実体の保存。
//!
//! 既定はローカルのディレクトリ。`s3://` の URL を渡すと S3 互換のストレージに置く。
//! 置き場所は [`BlobHash::storage_key`] で決まり、ハッシュから作るのでディレクトリの外は指さない。
//!
//! S3 に書くときは `Content-Type: text/html; charset=utf-8` を付ける。AWS の構成では CloudFront が
//! S3 から直接配るので、S3 のオブジェクトの Content-Type がそのまま応答に出るため。

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use object_store::{
    Attribute, Attributes, ObjectStore, ObjectStoreExt, PutOptions, PutPayload,
    aws::AmazonS3Builder, local::LocalFileSystem, memory::InMemory, path::Path as StorePath,
    prefix::PrefixStore,
};

use crate::domain::{
    blob::{BlobContent, BlobHash, BlobStore, HTML_CONTENT_TYPE},
    error::StorageError,
};

#[derive(Debug, thiserror::Error)]
pub enum OpenBlobStoreError {
    #[error("保存先のディレクトリを作れません: {0}")]
    CreateDir(#[from] std::io::Error),
    #[error("保存先「{0}」は s3://<バケット>/<プレフィックス> の形で指定してください")]
    InvalidUrl(String),
    #[error("保存先を開けません: {0}")]
    Store(#[from] object_store::Error),
}

/// object_store で資料の実体を読み書きする。複製しても同じ保存先を使う。
#[derive(Debug, Clone)]
pub struct ObjectStoreBlobStore {
    store: Arc<dyn ObjectStore>,
    /// 書くときにオブジェクトに付ける属性（Content-Type）。ローカルのディレクトリは属性を持てないので空。
    attributes: Attributes,
}

impl ObjectStoreBlobStore {
    /// ローカルのディレクトリに置く。ディレクトリが無ければ作る。
    pub fn local(root: &Path) -> Result<Self, OpenBlobStoreError> {
        std::fs::create_dir_all(root)?;
        let store = LocalFileSystem::new_with_prefix(root)?;
        // LocalFileSystem は属性を付けた書き込みを受け付けない（NotImplemented になる）
        Ok(Self {
            store: Arc::new(store),
            attributes: Attributes::new(),
        })
    }

    /// S3 互換のストレージ（`s3://<バケット>/<プレフィックス>`、プレフィックスは省略可）に置く。
    ///
    /// 認証情報・リージョン・エンドポイントは `AWS_ACCESS_KEY_ID`、`AWS_REGION`、
    /// `AWS_ENDPOINT` などの環境変数から読む。
    pub fn s3(url: &str) -> Result<Self, OpenBlobStoreError> {
        let (bucket, prefix) =
            parse_s3_url(url).ok_or_else(|| OpenBlobStoreError::InvalidUrl(url.to_owned()))?;
        let s3 = AmazonS3Builder::from_env()
            .with_bucket_name(bucket)
            .build()?;
        let store: Arc<dyn ObjectStore> = if prefix.is_empty() {
            Arc::new(s3)
        } else {
            Arc::new(PrefixStore::new(s3, prefix))
        };
        Ok(Self {
            store,
            attributes: html_attributes(),
        })
    }

    /// メモリ上に置く。テスト用。S3 と同じく Content-Type を付ける。
    pub fn in_memory() -> Self {
        Self {
            store: Arc::new(InMemory::new()),
            attributes: html_attributes(),
        }
    }
}

/// S3 などのオブジェクトに付ける属性。
fn html_attributes() -> Attributes {
    Attributes::from_iter([(Attribute::ContentType, HTML_CONTENT_TYPE)])
}

/// `s3://<バケット>/<プレフィックス>` をバケットとプレフィックスに分ける。
fn parse_s3_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("s3://")?;
    let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
    if bucket.is_empty() {
        return None;
    }
    Some((bucket, prefix.trim_matches('/')))
}

fn location(hash: &BlobHash) -> StorePath {
    StorePath::from(hash.storage_key())
}

fn backend_error(err: object_store::Error) -> StorageError {
    StorageError::Backend(Box::new(err))
}

#[async_trait]
impl BlobStore for ObjectStoreBlobStore {
    async fn put(&self, content: &BlobContent) -> Result<(), StorageError> {
        let location = location(content.hash());
        // 同じハッシュなら中身も同じなので、既にあれば書かない。
        // 確認と書き込みの間に別の書き込みが入っても、同じ中身で上書きされるだけで壊れない
        match self.store.head(&location).await {
            Ok(_) => return Ok(()),
            Err(object_store::Error::NotFound { .. }) => {}
            Err(err) => return Err(backend_error(err)),
        }
        let payload = PutPayload::from(content.as_bytes().to_vec());
        let options = PutOptions {
            attributes: self.attributes.clone(),
            ..PutOptions::default()
        };
        self.store
            .put_opts(&location, payload, options)
            .await
            .map_err(backend_error)?;
        Ok(())
    }

    async fn get(&self, hash: &BlobHash) -> Result<Option<BlobContent>, StorageError> {
        let result = match self.store.get(&location(hash)).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(err) => return Err(backend_error(err)),
        };
        let bytes = result.bytes().await.map_err(backend_error)?;
        // Bytes が他と共有されていなければ、コピーせずに Vec にする
        BlobContent::verify(hash, bytes.into())
            .map(Some)
            .map_err(|err| StorageError::Corrupted(err.to_string()))
    }

    /// 実体を消す。ローカルのときは空になったディレクトリ（`ab/cd/`）は残す（登録と同時に消すと、
    /// 登録側が書き込むディレクトリを消してしまうため）。
    async fn delete(&self, hash: &BlobHash) -> Result<(), StorageError> {
        match self.store.delete(&location(hash)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(err) => Err(backend_error(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    /// オブジェクトに付いた Content-Type。
    async fn content_type(
        store: &ObjectStoreBlobStore,
        hash: &BlobHash,
    ) -> Result<Option<String>, Box<dyn Error>> {
        let result = store.store.get(&location(hash)).await?;
        Ok(result
            .attributes
            .get(&Attribute::ContentType)
            .map(|value| value.to_string()))
    }

    #[tokio::test]
    async fn object_store_put_attaches_html_content_type() -> Result<(), Box<dyn Error>> {
        let store = ObjectStoreBlobStore::in_memory();
        let content = BlobContent::new(b"<!doctype html><p>a</p>".to_vec());
        store.put(&content).await?;
        assert_eq!(
            content_type(&store, content.hash()).await?.as_deref(),
            Some("text/html; charset=utf-8")
        );
        Ok(())
    }

    #[test]
    fn s3_store_attaches_html_content_type() -> Result<(), Box<dyn Error>> {
        let store = ObjectStoreBlobStore::s3("s3://galley/blobs")?;
        assert_eq!(
            store
                .attributes
                .get(&Attribute::ContentType)
                .map(|v| v.to_string()),
            Some(HTML_CONTENT_TYPE.to_owned())
        );
        Ok(())
    }

    #[test]
    fn parse_s3_url_splits_bucket_and_prefix() {
        assert_eq!(parse_s3_url("s3://galley"), Some(("galley", "")));
        assert_eq!(parse_s3_url("s3://galley/"), Some(("galley", "")));
        assert_eq!(
            parse_s3_url("s3://galley/team/blobs/"),
            Some(("galley", "team/blobs"))
        );
        assert_eq!(parse_s3_url("s3:///blobs"), None);
        assert_eq!(parse_s3_url("https://example.com/galley"), None);
    }
}
