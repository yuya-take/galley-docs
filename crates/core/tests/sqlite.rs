//! SQLite（Toasty）だけの性質を確かめる。どちらの DB でも同じテストは `repository.rs`。

use std::{error::Error, sync::Arc};

use galley_core::{
    adapter::sqlite::SqliteDatabase,
    app::project::{CreateProject, FindProject, ProjectInput},
};

type TestResult = Result<(), Box<dyn Error>>;

fn project_input(slug: &str, name: &str) -> ProjectInput {
    ProjectInput {
        slug: slug.to_owned(),
        name: name.to_owned(),
        description: String::new(),
        color: "blue".to_owned(),
    }
}

#[tokio::test]
async fn file_database_uses_wal_and_reapplies_migrations_safely() -> TestResult {
    let dir = std::env::temp_dir().join(format!("galley-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("galley.db");

    let database = SqliteDatabase::open(&path).await?;
    let sales = CreateProject::new(Arc::new(database.projects()))
        .execute(&project_input("sales", "営業部"))
        .await?;
    drop(database);

    // 2回目に開いてもマイグレーションは重ねて適用されず、データも残る
    let database = SqliteDatabase::open(&path).await?;
    let found = FindProject::new(Arc::new(database.projects()))
        .execute("sales")
        .await?;
    assert_eq!(found.id, sales.id);
    assert_eq!(database.journal_mode().await?, "wal");

    drop(database);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
