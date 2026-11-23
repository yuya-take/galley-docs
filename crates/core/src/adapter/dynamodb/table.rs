//! テーブルの読み書き。読み出しはすべて強い整合性で行い、Query はページを最後までたどる。

use std::{collections::HashMap, num::NonZeroU32, sync::Arc};

use aws_sdk_dynamodb::{
    Client,
    error::{BuildError, ProvideErrorMetadata, SdkError},
    operation::transact_write_items::TransactWriteItemsError,
    types::{AttributeValue, Delete, Put, TransactWriteItem, Update},
};

use crate::domain::error::RepositoryError;

use super::keys::{Key, PK, SK};

/// テーブルの1項目。
pub(super) type Item = HashMap<String, AttributeValue>;

/// 1 回の TransactWriteItems に入れられる書き込みの数（DynamoDB の上限）。
pub(super) const MAX_TRANSACTION_ITEMS: usize = 100;

#[derive(Debug, Clone)]
pub(super) struct Table {
    client: Client,
    name: Arc<str>,
    /// 1 回の Query で読む件数の上限。`None` なら DynamoDB の上限（1 MB）まで読む。
    page_size: Option<NonZeroU32>,
}

/// Query で読む順番（sk の順）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Order {
    Ascending,
    Descending,
}

/// 条件式・更新式と、その中で使う属性名（`#name`）と値（`:value`）。
///
/// 属性名は DynamoDB の予約語（`name`・`size`・`number` など）とぶつからないよう、必ず `#` の
/// プレースホルダーで渡す。使わない名前や値を渡すと DynamoDB が断るので、式で使うものだけを入れる。
#[derive(Debug, Clone, Default)]
pub(super) struct Expression {
    text: String,
    names: HashMap<String, String>,
    values: HashMap<String, AttributeValue>,
}

impl Expression {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// 項目がまだ無いこと（一意性の印、新しい項目）。
    pub fn not_exists() -> Self {
        Self::new(format!("attribute_not_exists({PK})"))
    }

    /// 項目があること（読んだあとに消されていないこと）。
    pub fn exists() -> Self {
        Self::new(format!("attribute_exists({PK})"))
    }

    pub fn name(mut self, placeholder: &str, attribute: &str) -> Self {
        self.names
            .insert(placeholder.to_owned(), attribute.to_owned());
        self
    }

    pub fn value(mut self, placeholder: &str, value: AttributeValue) -> Self {
        self.values.insert(placeholder.to_owned(), value);
        self
    }
}

/// トランザクションの中の書き込み1つ。
#[derive(Debug, Clone)]
pub(super) enum Write {
    Put {
        item: Item,
        condition: Option<Expression>,
    },
    Delete {
        key: Key,
        condition: Option<Expression>,
    },
    Update {
        key: Key,
        update: Expression,
        condition: Option<Expression>,
    },
}

impl Write {
    pub fn put(item: Item) -> Self {
        Self::Put {
            item,
            condition: None,
        }
    }

    pub fn delete(key: Key) -> Self {
        Self::Delete {
            key,
            condition: None,
        }
    }

    pub fn update(key: Key, update: Expression) -> Self {
        Self::Update {
            key,
            update,
            condition: None,
        }
    }

    pub fn when(self, expression: Expression) -> Self {
        match self {
            Self::Put { item, .. } => Self::Put {
                item,
                condition: Some(expression),
            },
            Self::Delete { key, .. } => Self::Delete {
                key,
                condition: Some(expression),
            },
            Self::Update { key, update, .. } => Self::Update {
                key,
                update,
                condition: Some(expression),
            },
        }
    }
}

/// 式の属性名と値を、DynamoDB に渡す形にする。空なら渡さない（空の指定は DynamoDB が断る）。
fn placeholders(
    expressions: &[&Expression],
) -> (
    Option<HashMap<String, String>>,
    Option<HashMap<String, AttributeValue>>,
) {
    let mut names = HashMap::new();
    let mut values = HashMap::new();
    for expression in expressions {
        names.extend(expression.names.clone());
        values.extend(expression.values.clone());
    }
    (
        (!names.is_empty()).then_some(names),
        (!values.is_empty()).then_some(values),
    )
}

impl Table {
    pub fn new(client: Client, name: &str) -> Self {
        Self {
            client,
            name: Arc::from(name),
            page_size: None,
        }
    }

    pub fn with_page_size(mut self, page_size: NonZeroU32) -> Self {
        self.page_size = Some(page_size);
        self
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 1項目を強い整合性で読む。
    pub async fn get(&self, key: &Key) -> Result<Option<Item>, RepositoryError> {
        let output = self
            .client
            .get_item()
            .table_name(self.name())
            .set_key(Some(key.to_item()))
            .consistent_read(true)
            .send()
            .await
            .map_err(backend_error)?;
        Ok(output.item)
    }

    /// `pk` が同じ項目のうち、`sk` が `prefix` で始まるものを `sk` の順に読む。
    /// 1 回の Query は 1 MB までしか返さないので、`LastEvaluatedKey` をたどって最後まで読む。
    /// `limit` を指定すれば、その件数まで。
    pub async fn query(
        &self,
        pk: &str,
        prefix: &str,
        order: Order,
        limit: Option<usize>,
    ) -> Result<Vec<Item>, RepositoryError> {
        let mut items = Vec::new();
        let mut start = None;
        loop {
            let remaining = limit.map(|limit| limit.saturating_sub(items.len()));
            if remaining == Some(0) {
                break;
            }
            let page_limit = [
                self.page_size.map(NonZeroU32::get),
                remaining.map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
            ]
            .into_iter()
            .flatten()
            .min()
            .map(|n| i32::try_from(n).unwrap_or(i32::MAX));
            let output = self
                .client
                .query()
                .table_name(self.name())
                .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
                .expression_attribute_names("#pk", PK)
                .expression_attribute_names("#sk", SK)
                .expression_attribute_values(":pk", AttributeValue::S(pk.to_owned()))
                .expression_attribute_values(":prefix", AttributeValue::S(prefix.to_owned()))
                .scan_index_forward(order == Order::Ascending)
                .consistent_read(true)
                .set_limit(page_limit)
                .set_exclusive_start_key(start.take())
                .send()
                .await
                .map_err(backend_error)?;
            items.extend(output.items.unwrap_or_default());
            match output.last_evaluated_key {
                Some(key) if !key.is_empty() => start = Some(key),
                _ => break,
            }
        }
        if let Some(limit) = limit {
            items.truncate(limit);
        }
        Ok(items)
    }

    /// 1項目を条件付きで更新する。条件に合わなければ `false` を返す（何も変えない）。
    pub async fn update_if(
        &self,
        key: &Key,
        update: Expression,
        condition: Expression,
    ) -> Result<bool, RepositoryError> {
        let (names, values) = placeholders(&[&update, &condition]);
        let result = self
            .client
            .update_item()
            .table_name(self.name())
            .set_key(Some(key.to_item()))
            .update_expression(update.text)
            .condition_expression(condition.text)
            .set_expression_attribute_names(names)
            .set_expression_attribute_values(values)
            .send()
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(err)
                if err
                    .as_service_error()
                    .is_some_and(|e| e.is_conditional_check_failed_exception()) =>
            {
                Ok(false)
            }
            Err(err) => Err(backend_error(err)),
        }
    }

    /// 書き込みを1トランザクションで行う。条件に合わなかったか、同時のトランザクションとぶつかって
    /// 取り消されたときは [`RepositoryError::Conflict`] を返す（呼び出し側が読み直してやり直す）。
    pub async fn transact(&self, writes: Vec<Write>) -> Result<(), RepositoryError> {
        if writes.len() > MAX_TRANSACTION_ITEMS {
            return Err(RepositoryError::Backend(
                format!(
                    "1 回のトランザクションに書き込みが多すぎます（{} 件）",
                    writes.len()
                )
                .into(),
            ));
        }
        let items = writes
            .into_iter()
            .map(|write| self.transact_item(write))
            .collect::<Result<Vec<_>, BuildError>>()
            .map_err(|err| RepositoryError::Backend(Box::new(err)))?;
        self.client
            .transact_write_items()
            .set_transact_items(Some(items))
            .send()
            .await
            .map_err(transaction_error)?;
        Ok(())
    }

    fn transact_item(&self, write: Write) -> Result<TransactWriteItem, BuildError> {
        let item = match write {
            Write::Put { item, condition } => {
                let (names, values) = placeholders(&condition.iter().collect::<Vec<_>>());
                let put = Put::builder()
                    .table_name(self.name())
                    .set_item(Some(item))
                    .set_condition_expression(condition.map(|c| c.text))
                    .set_expression_attribute_names(names)
                    .set_expression_attribute_values(values)
                    .build()?;
                TransactWriteItem::builder().put(put)
            }
            Write::Delete { key, condition } => {
                let (names, values) = placeholders(&condition.iter().collect::<Vec<_>>());
                let delete = Delete::builder()
                    .table_name(self.name())
                    .set_key(Some(key.to_item()))
                    .set_condition_expression(condition.map(|c| c.text))
                    .set_expression_attribute_names(names)
                    .set_expression_attribute_values(values)
                    .build()?;
                TransactWriteItem::builder().delete(delete)
            }
            Write::Update {
                key,
                update,
                condition,
            } => {
                let mut expressions = vec![&update];
                expressions.extend(condition.iter());
                let (names, values) = placeholders(&expressions);
                let update = Update::builder()
                    .table_name(self.name())
                    .set_key(Some(key.to_item()))
                    .update_expression(update.text.clone())
                    .set_condition_expression(condition.map(|c| c.text))
                    .set_expression_attribute_names(names)
                    .set_expression_attribute_values(values)
                    .build()?;
                TransactWriteItem::builder().update(update)
            }
        };
        Ok(item.build())
    }
}

/// SDK のエラーを domain のエラーに変換する。
pub(super) fn backend_error<E>(err: E) -> RepositoryError
where
    aws_sdk_dynamodb::Error: From<E>,
{
    RepositoryError::Backend(Box::new(aws_sdk_dynamodb::Error::from(err)))
}

/// トランザクションが取り消された理由のうち、読み直してやり直せば通りうるもの。
/// `ConditionalCheckFailed` は一意性の印や版の番号が先に使われた・読んだあとに変わった、
/// `TransactionConflict` は同じ項目を書く別のトランザクションと重なった。
const RETRYABLE_CANCELLATIONS: [&str; 2] = ["ConditionalCheckFailed", "TransactionConflict"];

fn transaction_error<R>(err: SdkError<TransactWriteItemsError, R>) -> RepositoryError
where
    R: std::fmt::Debug + Send + Sync + 'static,
{
    if let Some(TransactWriteItemsError::TransactionCanceledException(cancelled)) =
        err.as_service_error()
    {
        let codes: Vec<&str> = cancelled
            .cancellation_reasons()
            .iter()
            .filter_map(|reason| reason.code())
            .filter(|code| *code != "None")
            .collect();
        if codes
            .iter()
            .any(|code| RETRYABLE_CANCELLATIONS.contains(code))
        {
            return RepositoryError::Conflict(format!(
                "{}（{}）",
                cancelled
                    .message()
                    .unwrap_or("トランザクションが取り消されました"),
                codes.join(", ")
            ));
        }
    }
    backend_error(err)
}
