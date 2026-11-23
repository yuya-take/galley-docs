//! DynamoDB の実装（AWS で動かすとき。`GALLEY_DB=dynamodb://<テーブル名>`）。
//!
//! テーブルは1つで、キーは文字列の `pk`・`sk`、GSI は持たない。キーの設計は `docs/design.md` の
//! 「DynamoDB のテーブル」。どの読み出しも強い整合性で行うので、書いた直後に読んでも見える。
//!
//! 一意性（プロジェクトの slug、資料の slug、版の番号）は条件付き書き込みで守り、まとめて保存するもの
//! （資料と第1版、版の追加）は TransactWriteItems で1回に書く。条件に合わなかったか、同時の
//! トランザクションとぶつかって取り消されたときは [`RepositoryError::Conflict`] を返し、ユースケースの
//! やり直しに任せる。テーブルはアプリでは作らない（AWS では Terraform で作る）。
//!
//! [`RepositoryError::Conflict`]: crate::domain::error::RepositoryError::Conflict

mod document;
mod item;
mod keys;
mod list;
mod project;
mod purge;
mod revision;
mod table;

use std::num::NonZeroU32;

use aws_sdk_dynamodb::{
    Client,
    config::BehaviorVersion,
    types::{AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType},
};

pub use document::DynamoDbDocumentRepository;
pub use project::DynamoDbProjectRepository;
pub use revision::DynamoDbRevisionRepository;

use crate::domain::error::RepositoryError;

use self::{
    keys::{PK, SK},
    table::{Table, backend_error},
};

#[derive(Debug, thiserror::Error)]
pub enum OpenDynamoDbError {
    #[error("DynamoDB のテーブル名「{0}」は使えません（3〜255 文字の英数字・「_」「-」「.」）")]
    InvalidTableName(String),
    #[error(
        "DynamoDB のリージョンがわかりません。AWS_REGION（または AWS のプロファイルの region）を指定してください"
    )]
    MissingRegion,
}

/// DynamoDB のテーブル。複製しても同じクライアントを使う。
#[derive(Debug, Clone)]
pub struct DynamoDbDatabase {
    table: Table,
}

impl DynamoDbDatabase {
    /// AWS の標準の設定（環境変数、`~/.aws` のプロファイル・SSO、Lambda や ECS のロール）で接続する。
    /// DynamoDB Local などに向けるときは `AWS_ENDPOINT_URL_DYNAMODB` を指定する。
    pub async fn connect(table_name: &str) -> Result<Self, OpenDynamoDbError> {
        validate_table_name(table_name)?;
        let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
        if config.region().is_none() {
            return Err(OpenDynamoDbError::MissingRegion);
        }
        Self::with_client(Client::new(&config), table_name)
    }

    /// 作ったクライアントで使う（テストで DynamoDB Local に向けるときなど）。
    pub fn with_client(client: Client, table_name: &str) -> Result<Self, OpenDynamoDbError> {
        validate_table_name(table_name)?;
        Ok(Self {
            table: Table::new(client, table_name),
        })
    }

    /// 1 回の Query で読む件数の上限を決める。既定は DynamoDB の上限（1 MB）まで。
    /// テストで、ページを分けて読む処理を少ない件数で確かめるのに使う。
    pub fn with_query_page_size(self, page_size: NonZeroU32) -> Self {
        Self {
            table: self.table.with_page_size(page_size),
        }
    }

    pub fn table_name(&self) -> &str {
        self.table.name()
    }

    /// テーブルを作る（オンデマンド）。テストと、手元の DynamoDB Local で使う。
    /// AWS のテーブルは Terraform で作るので、サーバーの起動では呼ばない。
    pub async fn create_table(&self) -> Result<(), RepositoryError> {
        let key = |name: &str, key_type| {
            KeySchemaElement::builder()
                .attribute_name(name)
                .key_type(key_type)
                .build()
        };
        let attribute = |name: &str| {
            AttributeDefinition::builder()
                .attribute_name(name)
                .attribute_type(ScalarAttributeType::S)
                .build()
        };
        let build_error = |err| RepositoryError::Backend(Box::new(err));
        self.table
            .client()
            .create_table()
            .table_name(self.table.name())
            .billing_mode(BillingMode::PayPerRequest)
            .key_schema(key(PK, KeyType::Hash).map_err(build_error)?)
            .key_schema(key(SK, KeyType::Range).map_err(build_error)?)
            .attribute_definitions(attribute(PK).map_err(build_error)?)
            .attribute_definitions(attribute(SK).map_err(build_error)?)
            .send()
            .await
            .map_err(backend_error)?;
        Ok(())
    }

    pub fn projects(&self) -> DynamoDbProjectRepository {
        DynamoDbProjectRepository::new(self.table.clone())
    }

    pub fn documents(&self) -> DynamoDbDocumentRepository {
        DynamoDbDocumentRepository::new(self.table.clone())
    }

    pub fn revisions(&self) -> DynamoDbRevisionRepository {
        DynamoDbRevisionRepository::new(self.table.clone())
    }
}

/// DynamoDB のテーブル名の規則（3〜255 文字、英数字と `_` `-` `.`）。
fn validate_table_name(name: &str) -> Result<(), OpenDynamoDbError> {
    let is_valid = (3..=255).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    if is_valid {
        Ok(())
    } else {
        Err(OpenDynamoDbError::InvalidTableName(name.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_names_follow_dynamodb_rules() {
        for name in ["galley", "galley-prod", "galley_v1.0", "abc"] {
            assert!(validate_table_name(name).is_ok(), "{name}");
        }
        for name in ["", "ab", "galley/prod", "ギャレー", &"a".repeat(256)] {
            assert!(validate_table_name(name).is_err(), "{name}");
        }
    }
}
