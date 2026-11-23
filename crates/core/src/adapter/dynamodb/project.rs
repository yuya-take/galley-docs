use std::collections::HashMap;

use async_trait::async_trait;
use futures_util::future::try_join_all;

use crate::domain::{
    error::RepositoryError,
    project::{Project, ProjectRepository, ProjectSlug, ProjectSummary},
    shared::{ArchiveFilter, ProjectId},
};

use super::{
    item::{self, Reader, StoredDocument},
    keys::{DOCUMENT_PREFIX, Key, PROJECT_PREFIX, PROJECTS, project_documents},
    table::{Expression, Order, Table, Write},
};

#[derive(Debug, Clone)]
pub struct DynamoDbProjectRepository {
    table: Table,
}

impl DynamoDbProjectRepository {
    pub(super) fn new(table: Table) -> Self {
        Self { table }
    }
}

/// すべてのプロジェクトを読む（作成順に並べる）。プロジェクトは1つのパーティションにあるので、
/// 1 回の Query（ページはたどる）で読める。
pub(super) async fn load_projects(table: &Table) -> Result<Vec<Project>, RepositoryError> {
    let mut projects = table
        .query(PROJECTS, PROJECT_PREFIX, Order::Ascending, None)
        .await?
        .iter()
        .map(Project::try_from)
        .collect::<Result<Vec<_>, _>>()?;
    projects.sort_by_key(|project| (project.created_at, project.id));
    Ok(projects)
}

/// プロジェクトの資料をすべて読む（アーカイブした資料も含む）。
pub(super) async fn load_documents(
    table: &Table,
    project_id: ProjectId,
) -> Result<Vec<StoredDocument>, RepositoryError> {
    table
        .query(
            &project_documents(project_id),
            DOCUMENT_PREFIX,
            Order::Ascending,
            None,
        )
        .await?
        .iter()
        .map(StoredDocument::try_from)
        .collect()
}

#[async_trait]
impl ProjectRepository for DynamoDbProjectRepository {
    /// プロジェクトと slug の印を1トランザクションで書く。slug が使われていれば `Conflict`。
    async fn insert(&self, project: &Project) -> Result<(), RepositoryError> {
        self.table
            .transact(vec![
                Write::put(item::project_item(project)).when(Expression::not_exists()),
                Write::put(item::project_slug_item(project)).when(Expression::not_exists()),
            ])
            .await
    }

    /// slug が変わったときは、古い印を消して新しい印を作るのを、プロジェクトの保存と同じトランザクションで
    /// 行う。新しい slug がほかのプロジェクトに使われていれば `Conflict`。読んでから保存するまでの間に
    /// slug が変えられていたときも `Conflict` にする（古い印を正しく消せないため）。
    /// プロジェクトが無ければ何もしない（SQLite の実装と同じ）。
    async fn update(&self, project: &Project) -> Result<(), RepositoryError> {
        let Some(current) = self.table.get(&Key::project(project.id)).await? else {
            return Ok(());
        };
        let current_slug = Reader::new(&current, "projects").parse("slug", ProjectSlug::parse)?;
        let same_slug = Expression::new("#slug = :current_slug")
            .name("#slug", "slug")
            .value(":current_slug", item::string(current_slug.as_str()));
        let mut writes = vec![Write::put(item::project_item(project)).when(same_slug)];
        if current_slug != project.slug {
            writes.push(
                Write::delete(Key::project_slug(&current_slug)).when(
                    Expression::new("#project_id = :project_id")
                        .name("#project_id", "project_id")
                        .value(":project_id", item::uuid(project.id.as_uuid())),
                ),
            );
            writes
                .push(Write::put(item::project_slug_item(project)).when(Expression::not_exists()));
        }
        self.table.transact(writes).await
    }

    async fn find_by_id(&self, id: ProjectId) -> Result<Option<Project>, RepositoryError> {
        self.table
            .get(&Key::project(id))
            .await?
            .as_ref()
            .map(Project::try_from)
            .transpose()
    }

    /// slug の印からプロジェクトの ID を引いて読む。
    async fn find_by_slug(&self, slug: &ProjectSlug) -> Result<Option<Project>, RepositoryError> {
        let Some(marker) = self.table.get(&Key::project_slug(slug)).await? else {
            return Ok(None);
        };
        let id = ProjectId::from_uuid(Reader::new(&marker, "project_slugs").uuid("project_id")?);
        // 印を読んだあとに slug が変えられていれば、その slug のプロジェクトはもう無い
        Ok(self
            .find_by_id(id)
            .await?
            .filter(|project| &project.slug == slug))
    }

    /// 資料の件数は、プロジェクトごとの資料の Query を並行して数える。
    async fn list(&self, filter: ArchiveFilter) -> Result<Vec<ProjectSummary>, RepositoryError> {
        let projects: Vec<Project> = load_projects(&self.table)
            .await?
            .into_iter()
            .filter(|project| match filter {
                ArchiveFilter::Active => !project.is_archived(),
                ArchiveFilter::Archived => project.is_archived(),
            })
            .collect();

        let counts: HashMap<ProjectId, u64> =
            try_join_all(projects.iter().map(|project| async move {
                let documents = load_documents(&self.table, project.id).await?;
                let count = documents
                    .iter()
                    .filter(|stored| !stored.document.is_archived())
                    .count();
                Ok::<_, RepositoryError>((project.id, count as u64))
            }))
            .await?
            .into_iter()
            .collect();

        Ok(projects
            .into_iter()
            .map(|project| ProjectSummary {
                document_count: counts.get(&project.id).copied().unwrap_or(0),
                project,
            })
            .collect())
    }
}
