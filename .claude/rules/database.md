---
paths:
  - "crates/core/src/adapter/**"
  - "crates/core/db/**"
---

# Database Standards

## 構成

- **DB**: SQLite（WAL モード）1ファイル。`/data` ボリュームに置く（既定）。AWS では DynamoDB（`GALLEY_DB=dynamodb://<テーブル名>`。下の「DynamoDB」）
- **ORM**: Toasty
- **資料の実体**: DB には入れず、`object_store` でファイル（既定は `/data/blobs/ab/cd/<hash>.html`、置き場所は `BlobHash::storage_key`）として保存する。DB はハッシュだけ持つ
- 書き込みは直列になるが、資料の更新頻度なら問題にならない前提

## テーブル

| テーブル | 主な列 | メモ |
| --- | --- | --- |
| projects | id, slug, name, description, color, archived_at, created_at | slug は URL に使う。予約語（`RESERVED_PROJECT_SLUGS`：`archive`、`connect`、`api`、`mcp`、`settings` など）は使えない。color は紋章の色（red / blue / green / gold / purple / silver） |
| documents | id, project_id, slug, title, current_revision_id, archived_at, created_at, updated_at | current_revision_id が「現在版」のポインタ。(project_id, slug) は一意。slug の予約語は `settings`（`RESERVED_DOCUMENT_SLUGS`）。updated_at は版を追加したときだけ変わる |
| revisions | id, document_id, number, blob_hash, message, author_name, source, restored_from_number, created_at | 作成後は変更しない。(document_id, number) は一意。restored_from_number は「この版に戻す」で作った版だけが持つ |
| blobs | hash, size, created_at | 実体はファイル |

slug は英小文字・数字・ハイフンだけ。資料の slug はファイル名から作る（`docs/design.md` の「slug の決め方」）。

ユーザーや権限のテーブルは持たない。

## パターン

- **主キー**: UUID v7（時系列でソートできる）
- **論理削除**: `archived_at`（NULL なら有効）。projects と documents だけ。物理削除は `galley purge`（コンテナ内のコマンド）の `DocumentRepository::purge`・`purge_revision` だけが行う
- **不変**: revisions と blobs は INSERT のみ。UPDATE しない。DELETE は上の物理削除だけで、版と資料を消すのと同じトランザクションで、どの版からも参照されなくなった blobs の行を消す
- **過去版に戻す**: 過去版と同じ blob_hash を指す新しいリビジョンを作り、`restored_from_number` に元の版番号を入れる（履歴を書き換えない）
- **時刻**: UTC で保存する
- **外部キー**: Toasty は外部キー制約を作らず、接続ごとの PRAGMA（`foreign_keys` など）も設定できない。参照の整合はユースケースで保つ（存在を確かめてから作る。物理削除は資料とその版、参照の無くなったブロブをまとめて消す）
- **WAL**: `SqliteDatabase::open` で `PRAGMA journal_mode = WAL` を1回実行する（DB ファイルに記録される）。`busy_timeout` は rusqlite の既定（5秒）
- **書き込みは1本ずつ**: 書き込むメソッドは最初に `WriteLock::acquire` で順番待ちする。読んでから書くトランザクションは `begin_write`（`BEGIN IMMEDIATE`）で始める
- **版番号**: 現在版の番号 + 1。`(document_id, number)` の一意制約で重なりを `Conflict` にし、ユースケースが読み直してやり直す（`app::revision` の `MAX_ATTEMPTS` 回）

## 保存の順序

1. ブロブをストレージに書く（同じハッシュがあれば書かない）
2. リビジョンの作成と `current_revision_id` の更新を **1トランザクション** で行う

この順序なら「DB にあるのに実体がない」状態にはならない。参照されないブロブは残りうるので、掃除ジョブで消す。

## マイグレーション

- 置き場所は `crates/core/db/`（`history.toml`、`migrations/*.sql`、`snapshots/*.toml`）。設定はリポジトリのルートの `Toasty.toml`
- モデル（`crates/core/src/adapter/sqlite/model.rs`）を変えたら、リポジトリのルートで `cargo run -p galley-migrate -- migration generate --name <内容>` を実行し、できた SQL を確認して3種類のファイルをまとめてコミットする
- マイグレーションは `toasty::embed_migrations!` でバイナリに埋め込み、起動時に自動で適用する（Docker イメージ1つで動かすため）。適用済みかどうかは `__toasty_migrations` テーブルで判定する
- 一度リリースしたマイグレーションは書き換えない。変更は新しいマイグレーションで行う
- 列の削除・型変更は、既存データの移行を同じマイグレーションに含める

## Toasty の注意点（0.11）

- 結合・`EXISTS`・集計が要るクエリは `toasty::sql::query` の生の SQL で書く。値は必ずプレースホルダー（`?1`、`?2` …）で渡し、文字列に埋め込まない
- 生の SQL で `Uuid` や `Timestamp` を渡したり型を指定して受け取ったりすると、SQLite ドライバーがエラーではなくパニック（`todo!()`）になる。`adapter/sqlite/raw.rs` の変換を使い、UUID はバイト列、日時は文字列（小数9桁固定の RFC 3339）で扱う
- SQLite ドライバーは接続ごとのタスクの中で同期的に SQLite を呼ぶので、ロックを待つ接続が tokio のワーカーを塞ぐ。同時の書き込みが重なると、ロックを持つ接続が進めずに「database is locked」になる。書き込みは必ず `WriteLock` を通す
- `TransactionMode` は `toasty` から再公開されていないので `toasty-core` から使う
- 一意制約の違反は Toasty が区別しないので、SQLite のメッセージ（`UNIQUE constraint failed`）で `RepositoryError::Conflict` に変換する

## DynamoDB

AWS で使う（`crates/core/src/adapter/dynamodb/`）。テーブルの形と読み書きの方法は `docs/design.md` の「DynamoDB のテーブル」に書き、変えたらそこも直す（Terraform がその形でテーブルを作る）。

- **テーブル**: 1つ（`pk`・`sk` とも文字列、GSI なし、オンデマンド）。アプリはテーブルを作らない（`DynamoDbDatabase::create_table` はテストと手元の DynamoDB Local 用）。マイグレーションは無いので、項目の形を変えるときは古い形も読めるようにする
- **読み出し**: すべて強い整合性で読む。Query は `Table::query` を使い、`LastEvaluatedKey` を最後までたどる
- **一意性**: `attribute_not_exists(pk)` の条件付き書き込み。まとめて保存するものは `Table::transact`（TransactWriteItems、100 件まで）で1回に書く。取り消し（条件の失敗・`TransactionConflict`）は `RepositoryError::Conflict` になり、ユースケースがやり直す
- **式の属性名**: DynamoDB の予約語（`name`・`size`・`number`・`source` など）とぶつかるので、条件式・更新式の属性名は必ず `#` のプレースホルダーで渡す（`Expression::name`）。使わないプレースホルダーを渡すと DynamoDB が断る
- **条件の無い更新は項目を作る**: `UpdateItem` は項目が無ければ作ってしまうので、既存の項目を変えるときは `Expression::exists()` などの条件を付ける
- **版は不変**: 版の写し（資料と番号、版の ID、ブロブ）は、同じトランザクションで書いて同じトランザクションで消す。資料の項目の `current_revision_*` は版の追加と同じトランザクションで書く
- **ブロブの参照**: `ref_count` を版の追加で増やし、物理削除で読んだ値を条件に減らす（楽観ロック）
- **集合の属性**: 文字列の集合（SS）は使わず、キーを値にしたマップにする（DynamoDB Local 3.3 は、日本語の文字列の集合への `ADD` を正しく扱えない）
- **テスト**: `crates/core/tests` の結合テストは `GALLEY_TEST_DYNAMODB_ENDPOINT` があれば DynamoDB Local で動く（`tests/common`）。リポジトリを変えたら、SQLite と DynamoDB Local の両方で流す
