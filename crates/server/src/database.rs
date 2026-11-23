//! DB を選んで開く。`GALLEY_DB` が無ければ SQLite（`<GALLEY_DATA_DIR>/galley.db`）、
//! `dynamodb://<テーブル名>` なら DynamoDB。サーバーと `galley purge` の両方が使う。

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, bail};
use galley_core::{
    adapter::{dynamodb::DynamoDbDatabase, sqlite::SqliteDatabase},
    domain::{
        document::DocumentRepository, project::ProjectRepository, revision::RevisionRepository,
    },
};

/// DB を選ぶ環境変数。
const DB_ENV: &str = "GALLEY_DB";
const DYNAMODB_SCHEME: &str = "dynamodb://";

/// どの DB を使うか。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseConfig {
    /// SQLite のファイル（既定）。
    Sqlite { path: PathBuf },
    /// DynamoDB のテーブル。認証情報とリージョンは AWS の標準の設定（環境変数、プロファイル、ロール）から取る。
    DynamoDb { table: String },
}

impl DatabaseConfig {
    /// `GALLEY_DB` を読む。無ければ `data_dir` の SQLite。
    pub fn from_env(data_dir: &Path) -> anyhow::Result<Self> {
        match std::env::var(DB_ENV) {
            Ok(value) if !value.is_empty() => Self::parse(&value),
            _ => Ok(Self::Sqlite {
                path: data_dir.join("galley.db"),
            }),
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value.strip_prefix(DYNAMODB_SCHEME) {
            Some(table) if !table.is_empty() && !table.contains('/') => Ok(Self::DynamoDb {
                table: table.to_owned(),
            }),
            _ => bail!(
                "{DB_ENV}「{value}」は {DYNAMODB_SCHEME}<テーブル名> の形で指定してください（SQLite を使うときは指定しません）"
            ),
        }
    }
}

impl fmt::Display for DatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite { path } => write!(f, "sqlite {}", path.display()),
            Self::DynamoDb { table } => write!(f, "dynamodb {table}"),
        }
    }
}

/// ユースケースに渡すリポジトリ。
#[derive(Clone)]
pub struct Repositories {
    pub projects: Arc<dyn ProjectRepository>,
    pub documents: Arc<dyn DocumentRepository>,
    pub revisions: Arc<dyn RevisionRepository>,
}

/// DB を開く。SQLite は無ければ作り、マイグレーションを適用する。DynamoDB のテーブルは作らない
/// （AWS では Terraform で作る）。
pub async fn open(config: &DatabaseConfig) -> anyhow::Result<Repositories> {
    match config {
        DatabaseConfig::Sqlite { path } => {
            let database = SqliteDatabase::open(path).await?;
            Ok(Repositories {
                projects: Arc::new(database.projects()),
                documents: Arc::new(database.documents()),
                revisions: Arc::new(database.revisions()),
            })
        }
        DatabaseConfig::DynamoDb { table } => {
            let database = DynamoDbDatabase::connect(table)
                .await
                .with_context(|| format!("DynamoDB のテーブル「{table}」に接続できません"))?;
            Ok(Repositories {
                projects: Arc::new(database.projects()),
                documents: Arc::new(database.documents()),
                revisions: Arc::new(database.revisions()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamodb_url_names_the_table() {
        assert_eq!(
            DatabaseConfig::parse("dynamodb://galley-prod").unwrap(),
            DatabaseConfig::DynamoDb {
                table: "galley-prod".to_owned()
            }
        );
    }

    #[test]
    fn other_values_are_rejected() {
        for value in [
            "dynamodb://",
            "dynamodb://galley/prod",
            "sqlite://galley.db",
            "galley",
        ] {
            assert!(DatabaseConfig::parse(value).is_err(), "{value}");
        }
    }
}
