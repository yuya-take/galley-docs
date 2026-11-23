//! 結合テストの DB。環境変数 `GALLEY_TEST_DYNAMODB_ENDPOINT`（例: `http://localhost:8000`）があれば
//! DynamoDB Local、無ければ SQLite で動かす。DynamoDB ではテストごとに新しいテーブルを作る。
//!
//! ```sh
//! docker run --rm -d -p 8000:8000 amazon/dynamodb-local:3.3.2
//! GALLEY_TEST_DYNAMODB_ENDPOINT=http://localhost:8000 \
//!   cargo test -p galley-core --test repository --test revision --test purge --test dynamodb
//! ```

// テストのファイルごとに使う関数が違う
#![allow(dead_code)]

use std::{error::Error, num::NonZeroU32, path::PathBuf, sync::Arc};

use aws_sdk_dynamodb::{
    Client,
    config::{BehaviorVersion, Credentials, Region},
};
use galley_core::{
    adapter::{dynamodb::DynamoDbDatabase, sqlite::SqliteDatabase},
    domain::{
        document::DocumentRepository, project::ProjectRepository, revision::RevisionRepository,
    },
};

pub type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// DynamoDB Local のエンドポイントを渡す環境変数。
const DYNAMODB_ENDPOINT: &str = "GALLEY_TEST_DYNAMODB_ENDPOINT";

/// DynamoDB のテストで、1 回の Query で読む件数。少なくして、ページを分けて読む処理をどのテストでも通す。
const QUERY_PAGE_SIZE: NonZeroU32 = NonZeroU32::new(2).unwrap();

pub enum TestDatabase {
    Sqlite {
        database: SqliteDatabase,
        /// ファイルの DB のとき、その一時ディレクトリ（終わったら消す）。
        dir: Option<Arc<TempDir>>,
    },
    DynamoDb(DynamoDbDatabase),
}

/// テストごとの一時ディレクトリ。終わったら消す。
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new() -> TestResult<Self> {
        let dir =
            TempDir(std::env::temp_dir().join(format!("galley-test-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&dir.0)?;
        Ok(dir)
    }

    fn db_path(&self) -> PathBuf {
        self.0.join("galley.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// DynamoDB Local のエンドポイント。設定が無ければ SQLite で動かす。
pub fn dynamodb_endpoint() -> Option<String> {
    std::env::var(DYNAMODB_ENDPOINT)
        .ok()
        .filter(|endpoint| !endpoint.is_empty())
}

/// DynamoDB Local のクライアント。認証情報とリージョンは DynamoDB Local が見ないので、固定の値にする。
pub fn dynamodb_client(endpoint: &str) -> Client {
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .endpoint_url(endpoint)
        .region(Region::new("ap-northeast-1"))
        .credentials_provider(Credentials::new(
            "local",
            "local",
            None,
            None,
            "galley-test",
        ))
        .build();
    Client::from_conf(config)
}

/// DynamoDB Local に新しいテーブルを作る。
pub async fn new_dynamodb_table(endpoint: &str) -> TestResult<DynamoDbDatabase> {
    let name = format!("galley-test-{}", uuid::Uuid::now_v7());
    let database = DynamoDbDatabase::with_client(dynamodb_client(endpoint), &name)?;
    database.create_table().await?;
    Ok(database)
}

impl TestDatabase {
    /// SQLite はメモリ上の DB、DynamoDB は新しいテーブル。
    pub async fn new() -> TestResult<Self> {
        match dynamodb_endpoint() {
            Some(endpoint) => Self::dynamodb(&endpoint).await,
            None => Ok(Self::Sqlite {
                database: SqliteDatabase::in_memory().await?,
                dir: None,
            }),
        }
    }

    /// ほかの接続から開き直せる DB（同時の書き込みを確かめる）。SQLite はファイルの DB。
    pub async fn file() -> TestResult<Self> {
        match dynamodb_endpoint() {
            Some(endpoint) => Self::dynamodb(&endpoint).await,
            None => {
                let dir = TempDir::new()?;
                let database = SqliteDatabase::open(&dir.db_path()).await?;
                Ok(Self::Sqlite {
                    database,
                    dir: Some(Arc::new(dir)),
                })
            }
        }
    }

    async fn dynamodb(endpoint: &str) -> TestResult<Self> {
        Ok(Self::DynamoDb(
            new_dynamodb_table(endpoint)
                .await?
                .with_query_page_size(QUERY_PAGE_SIZE),
        ))
    }

    /// 同じ DB を別に開く（サーバーと `galley purge` のように、別のプロセスから使う代わり）。
    pub async fn reopen(&self) -> TestResult<Self> {
        match self {
            Self::Sqlite { dir: Some(dir), .. } => Ok(Self::Sqlite {
                database: SqliteDatabase::open(&dir.db_path()).await?,
                dir: Some(dir.clone()),
            }),
            Self::Sqlite { dir: None, .. } => Err("メモリ上の DB は開き直せません".into()),
            Self::DynamoDb(database) => {
                let endpoint = dynamodb_endpoint().ok_or("DynamoDB Local がありません")?;
                Ok(Self::DynamoDb(
                    DynamoDbDatabase::with_client(
                        dynamodb_client(&endpoint),
                        database.table_name(),
                    )?
                    .with_query_page_size(QUERY_PAGE_SIZE),
                ))
            }
        }
    }

    pub fn projects(&self) -> Arc<dyn ProjectRepository> {
        match self {
            Self::Sqlite { database, .. } => Arc::new(database.projects()),
            Self::DynamoDb(database) => Arc::new(database.projects()),
        }
    }

    pub fn documents(&self) -> Arc<dyn DocumentRepository> {
        match self {
            Self::Sqlite { database, .. } => Arc::new(database.documents()),
            Self::DynamoDb(database) => Arc::new(database.documents()),
        }
    }

    pub fn revisions(&self) -> Arc<dyn RevisionRepository> {
        match self {
            Self::Sqlite { database, .. } => Arc::new(database.revisions()),
            Self::DynamoDb(database) => Arc::new(database.revisions()),
        }
    }
}
