//! DynamoDB だけの性質を確かめる。`GALLEY_TEST_DYNAMODB_ENDPOINT`（DynamoDB Local）が無ければ何もしない。
//! どちらの DB でも同じテストは `repository.rs`・`revision.rs`・`purge.rs`（DynamoDB では、1 回の Query で
//! 2 件ずつ読んで、ページをたどる処理を通している）。

mod common;

use std::sync::Arc;

use common::{TestResult, dynamodb_endpoint, new_dynamodb_table};
use galley_core::{
    adapter::{blob_store::ObjectStoreBlobStore, dynamodb::DynamoDbDatabase},
    app::{
        document::{CreateDocument, CreateDocumentInput, PurgeDocument, SlugSource},
        project::{CreateProject, ProjectInput},
        revision::RevisionInput,
    },
    domain::{
        blob::{Blob, BlobHash},
        document::DocumentRepository,
        revision::{AuthorName, NewRevision, RevisionMessage, RevisionRepository, RevisionSource},
        upload::HtmlSizeLimit,
    },
};
use jiff::Timestamp;

/// DynamoDB Local が無ければ `None`（テストを飛ばす）。
async fn database() -> TestResult<Option<DynamoDbDatabase>> {
    match dynamodb_endpoint() {
        Some(endpoint) => Ok(Some(new_dynamodb_table(&endpoint).await?)),
        None => Ok(None),
    }
}

/// 1 回の Query は 1 MB までしか返さない。1 MB を超える版のある資料でも、ページをたどってすべて読み、
/// 物理削除もできる（ページの大きさは DynamoDB の既定のまま）。
#[tokio::test]
async fn queries_follow_pages_beyond_one_megabyte() -> TestResult {
    let Some(database) = database().await? else {
        return Ok(());
    };
    let blobs = Arc::new(ObjectStoreBlobStore::in_memory());
    let project = CreateProject::new(Arc::new(database.projects()))
        .execute(&ProjectInput {
            slug: "sales".to_owned(),
            name: "営業部".to_owned(),
            description: String::new(),
            color: "blue".to_owned(),
        })
        .await?;
    let created = CreateDocument::new(
        Arc::new(database.projects()),
        Arc::new(database.documents()),
        blobs.clone(),
        HtmlSizeLimit::DEFAULT,
    )
    .execute(CreateDocumentInput {
        project_id: project.id,
        title: None,
        slug: SlugSource::FileName("plan.html".to_owned()),
        revision: RevisionInput {
            html: b"<title>plan</title>".to_vec(),
            message: "初版".to_owned(),
            author_name: "佐藤".to_owned(),
            source: RevisionSource::Web,
        },
    })
    .await?;

    // 変更メモを上限（500 文字）の日本語にすると、版の項目は 1 つ 1.5 KB を超える。
    // 版の写しはパーティションごとに置くので、資料のパーティションだけで 1 MB を超える
    let documents = database.documents();
    let message = RevisionMessage::parse(&"版".repeat(500))?;
    let mut document = created.document.clone();
    let mut current = created.revision.clone();
    const REVISIONS: usize = 800;
    for n in 2..=REVISIONS {
        let hash = BlobHash::of(format!("{n}").as_bytes());
        let revision = document.add_revision(
            &current,
            NewRevision {
                blob_hash: hash.clone(),
                message: message.clone(),
                author_name: AuthorName::parse("佐藤")?,
                source: RevisionSource::Mcp,
            },
            Timestamp::now(),
        )?;
        let blob = Blob {
            hash,
            size: 1,
            created_at: Timestamp::now(),
        };
        documents
            .insert_revision(&document, &revision, Some(&blob))
            .await?;
        current = revision;
    }

    let revisions = database
        .revisions()
        .list_by_document(document.id, None)
        .await?;
    assert_eq!(revisions.len(), REVISIONS);
    assert_eq!(revisions[0].id, current.id);
    let size: usize = revisions.iter().map(|r| r.message.as_str().len()).sum();
    assert!(size > 1024 * 1024, "{size} バイト");

    let purge = PurgeDocument::new(
        Arc::new(database.projects()),
        Arc::new(database.documents()),
        Arc::new(database.revisions()),
        blobs,
    );
    let plan = purge.plan("sales", "plan", None).await?;
    assert_eq!(plan.revisions.len(), REVISIONS);
    let outcome = purge.execute(&plan).await?;
    assert_eq!(outcome.purged.numbers.len(), REVISIONS);
    assert_eq!(documents.find_by_id(document.id).await?, None);
    assert!(
        database
            .revisions()
            .list_by_document(document.id, None)
            .await?
            .is_empty()
    );
    Ok(())
}
