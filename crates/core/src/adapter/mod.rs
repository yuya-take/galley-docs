//! domain のトレイトの実装（DB、ストレージ）。DB は SQLite（既定）と DynamoDB（AWS）。組み立ては `galley-server` の `main.rs` で行う。

pub mod blob_store;
pub mod dynamodb;
pub mod sqlite;
