//! アプリ本体（:8080）のルーターの組み立て。画面（Topcoat）と `/mcp`、MCP の OAuth のメタデータを載せ、
//! 外側に Host の検査と CSRF 対策を置く。

use std::{convert::Infallible, sync::Arc};

use axum::{Router, extract::Request, middleware, response::IntoResponse, routing::get};
use galley_core::{
    app::{
        document::{ArchiveDocument, CreateDocument, FindDocument, ListDocuments},
        project::{FindProject, ListProjects},
        revision::{AddRevision, FindRevision, ReadRevisionContent},
    },
    domain::{
        blob::BlobStore, document::DocumentRepository, project::ProjectRepository,
        revision::RevisionRepository, upload::HtmlSizeLimit,
    },
};
use galley_web::AppUrl;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use tower::{Service, ServiceBuilder};
use tower_http::limit::RequestBodyLimitLayer;

use crate::{
    guard::{self, AllowedHosts, McpToken},
    mcp::{GalleyMcp, McpApp},
    oauth::{self, ProtectedResourceMetadata},
};

/// ユースケースに渡すリポジトリとストレージ。実装（adapter）は `main.rs` で作る。
#[derive(Clone)]
pub struct Ports {
    pub projects: Arc<dyn ProjectRepository>,
    pub documents: Arc<dyn DocumentRepository>,
    pub revisions: Arc<dyn RevisionRepository>,
    pub blobs: Arc<dyn BlobStore>,
}

/// MCP から使うユースケースを組み立てる。`html_size_limit` は1ファイルのサイズ上限。
pub fn mcp_app(ports: Ports, app_url: AppUrl, html_size_limit: HtmlSizeLimit) -> Arc<McpApp> {
    let Ports {
        projects,
        documents,
        revisions,
        blobs,
    } = ports;
    Arc::new(McpApp {
        list_projects: ListProjects::new(projects.clone()),
        find_project: FindProject::new(projects.clone()),
        list_documents: ListDocuments::new(documents.clone()),
        find_document: FindDocument::new(documents.clone()),
        find_revision: FindRevision::new(revisions.clone()),
        read_revision: ReadRevisionContent::new(revisions.clone(), blobs.clone()),
        create_document: CreateDocument::new(
            projects,
            documents.clone(),
            blobs.clone(),
            html_size_limit,
        ),
        add_revision: AddRevision::new(documents.clone(), revisions, blobs, html_size_limit),
        archive_document: ArchiveDocument::new(documents),
        app_url,
    })
}

/// `/mcp` と OAuth のメタデータ以外はすべて画面（`web`）に渡す。Host の検査を一番外に置く。
///
/// MCP はセッションを持たない（リクエストごとに処理を作り、応答は JSON で返す）。AWS では Lambda の
/// 実行環境がリクエストごとに入れ替わりうるため。Docker の構成も同じ動きにする。
///
/// `oauth` が `None`（`MCP_OAUTH_*` が未設定）なら、メタデータは 404 を返す。
///
/// `html_size_limit` には `mcp_app` に渡したものと同じ1ファイルのサイズ上限を渡す。MCP のリクエストの
/// 大きさの上限は、ここから決める（[`HtmlSizeLimit::json_body_limit`]。JSON のエスケープで増える分の余裕）。
pub fn app_router<W>(
    web: W,
    mcp_app: Arc<McpApp>,
    allowed_hosts: &Arc<AllowedHosts>,
    mcp_token: Arc<McpToken>,
    oauth: Option<Arc<ProtectedResourceMetadata>>,
    html_size_limit: HtmlSizeLimit,
) -> Router
where
    W: Service<Request, Error = Infallible> + Clone + Send + Sync + 'static,
    W::Response: IntoResponse,
    W::Future: Send + 'static,
{
    let mcp = StreamableHttpService::new(
        move || Ok(GalleyMcp::new(mcp_app.clone())),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(allowed_hosts.for_rmcp())
            .with_legacy_session_mode(false)
            .with_json_response(true),
    );
    let mcp = ServiceBuilder::new()
        .layer(middleware::from_fn_with_state(
            mcp_token,
            guard::check_mcp_token,
        ))
        .layer(RequestBodyLimitLayer::new(
            html_size_limit.json_body_limit(),
        ))
        .service(mcp);
    // クライアントがトークンを取る前に読むものなので、MCP_TOKEN の確認はかけない
    let metadata = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::protected_resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(oauth::protected_resource_metadata),
        )
        .with_state(oauth);
    Router::new()
        .nest_service("/mcp", mcp)
        .merge(metadata)
        .fallback_service(web)
        .layer(middleware::from_fn(guard::check_csrf))
        .layer(middleware::from_fn_with_state(
            allowed_hosts.clone(),
            guard::check_host,
        ))
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use axum::{body::Body, http::StatusCode};
    use galley_core::{
        adapter::{blob_store::ObjectStoreBlobStore, sqlite::SqliteDatabase},
        app::{
            document::PurgeDocument,
            project::{ArchiveProject, CreateProject, ProjectInput},
        },
    };
    use http::Request as HttpRequest;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    struct Mcp {
        router: Router,
        /// MCP の外から状態を変える（プロジェクトをアーカイブするなど）ときに使う。
        database: SqliteDatabase,
    }

    impl Mcp {
        async fn new(token: Option<&str>) -> TestResult<Self> {
            Self::with_limit(token, HtmlSizeLimit::DEFAULT).await
        }

        async fn with_limit(
            token: Option<&str>,
            html_size_limit: HtmlSizeLimit,
        ) -> TestResult<Self> {
            Self::build(token, html_size_limit, None).await
        }

        async fn with_oauth(oauth: ProtectedResourceMetadata) -> TestResult<Self> {
            Self::build(None, HtmlSizeLimit::DEFAULT, Some(Arc::new(oauth))).await
        }

        async fn build(
            token: Option<&str>,
            html_size_limit: HtmlSizeLimit,
            oauth: Option<Arc<ProtectedResourceMetadata>>,
        ) -> TestResult<Self> {
            let database = SqliteDatabase::in_memory().await?;
            CreateProject::new(Arc::new(database.projects()))
                .execute(&ProjectInput {
                    slug: "sales".to_owned(),
                    name: "営業部".to_owned(),
                    description: "営業の資料".to_owned(),
                    color: "blue".to_owned(),
                })
                .await?;
            let web = tower::service_fn(|_: Request| async {
                Ok::<_, Infallible>(StatusCode::NOT_FOUND)
            });
            let router = app_router(
                web,
                mcp_app(
                    Ports {
                        projects: Arc::new(database.projects()),
                        documents: Arc::new(database.documents()),
                        revisions: Arc::new(database.revisions()),
                        blobs: Arc::new(ObjectStoreBlobStore::in_memory()),
                    },
                    AppUrl::FromRequest,
                    html_size_limit,
                ),
                &Arc::new(AllowedHosts::parse(None)?),
                Arc::new(McpToken::from_env(token.map(str::to_owned))),
                oauth,
                html_size_limit,
            );
            Ok(Self { router, database })
        }

        async fn post(
            &self,
            body: &Value,
            headers: &[(&str, &str)],
        ) -> TestResult<(StatusCode, http::HeaderMap, String)> {
            let mut request = HttpRequest::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost:8080")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream");
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            let bytes = serde_json::to_vec(body)?;
            let response = self
                .router
                .clone()
                .oneshot(
                    request
                        .header("content-length", bytes.len())
                        .body(Body::from(bytes))?,
                )
                .await?;
            let status = response.status();
            let headers = response.headers().clone();
            let body =
                String::from_utf8(response.into_body().collect().await?.to_bytes().to_vec())?;
            Ok((status, headers, body))
        }

        async fn get(&self, uri: &str, host: &str) -> TestResult<(StatusCode, String)> {
            let response = self
                .router
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri(uri)
                        .header("host", host)
                        .header("accept", "application/json, text/event-stream")
                        .body(Body::empty())?,
                )
                .await?;
            let status = response.status();
            let body =
                String::from_utf8(response.into_body().collect().await?.to_bytes().to_vec())?;
            Ok((status, body))
        }
    }

    impl Mcp {
        /// JSON-RPC の応答を読む。セッションなしで `json_response` にしているので、SSE ではなく JSON で届く。
        fn message(body: &str) -> TestResult<Value> {
            Ok(serde_json::from_str(body).map_err(|e| format!("JSON でない（{e}）: {body}"))?)
        }

        async fn connect(token: Option<&str>) -> TestResult<Self> {
            Self::connect_with(Self::new(token).await?, token).await
        }

        /// クライアントと同じく `initialize` と `notifications/initialized` を送る。
        /// セッションは作らないので、応答に `mcp-session-id` は無い。
        async fn connect_with(mcp: Self, token: Option<&str>) -> TestResult<Self> {
            let auth = token.map(|t| format!("Bearer {t}"));
            let headers: Vec<(&str, &str)> =
                auth.iter().map(|a| ("authorization", a.as_str())).collect();
            let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}});
            let (status, response_headers, body) = mcp.post(&init, &headers).await?;
            assert_eq!(status, StatusCode::OK, "{body}");
            let info = Self::message(&body)?;
            assert_eq!(info["result"]["serverInfo"]["name"], "galley");
            assert!(
                !response_headers.contains_key("mcp-session-id"),
                "{response_headers:?}"
            );
            let (status, _, _) = mcp
                .post(
                    &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                    &headers,
                )
                .await?;
            assert_eq!(status, StatusCode::ACCEPTED);
            Ok(mcp)
        }

        /// ツールを呼び、結果のテキストとエラーかどうかを返す。
        async fn call(
            &self,
            tool: &str,
            arguments: Value,
            headers: &[(&str, &str)],
        ) -> TestResult<(String, bool)> {
            let request = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":arguments}});
            let (status, _, body) = self.post(&request, headers).await?;
            assert_eq!(status, StatusCode::OK, "{body}");
            let message = Self::message(&body)?;
            let result = &message["result"];
            let text = result["content"][0]["text"]
                .as_str()
                .ok_or_else(|| format!("テキストがない: {message}"))?;
            Ok((text.to_owned(), result["isError"] == true))
        }
    }

    const AUTHOR: (&str, &str) = ("x-author-name", "佐藤");
    const HTML_V1: &str = "<!doctype html><title>計画</title><p>第1版";
    const HTML_V2: &str = "<!doctype html><title>計画</title><p>第2版";

    #[tokio::test]
    async fn lists_tools_with_single_html_rule() -> TestResult {
        let mcp = Mcp::connect(None).await?;
        let (_, _, body) = mcp
            .post(&json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}), &[])
            .await?;
        let tools = Mcp::message(&body)?;
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .ok_or("tools がない")?
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for name in [
            "list_projects",
            "list_documents",
            "get_document",
            "create_document",
            "update_document",
            "archive_document",
        ] {
            assert!(names.contains(&name), "{name} がない: {names:?}");
        }
        assert!(body.contains("1ファイルで完結した HTML"));
        Ok(())
    }

    #[tokio::test]
    async fn create_update_and_read_document() -> TestResult {
        let mcp = Mcp::connect(None).await?;

        let (text, is_error) = mcp.call("list_projects", json!({}), &[]).await?;
        assert!(!is_error, "{text}");
        let projects: Value = serde_json::from_str(&text)?;
        assert_eq!(projects[0]["slug"], "sales");

        let (text, is_error) = mcp
            .call(
                "create_document",
                json!({"project":"sales","title":"事業計画","html":HTML_V1,"slug":"plan"}),
                &[AUTHOR],
            )
            .await?;
        assert!(!is_error, "{text}");
        let created: Value = serde_json::from_str(&text)?;
        assert_eq!(created["slug"], "plan");
        assert_eq!(created["revision_number"], 1);
        assert_eq!(created["url"], "http://localhost:8080/sales/plan");
        assert_eq!(created["message"], "「事業計画」を第1版として登録しました");

        let (text, is_error) = mcp
            .call(
                "update_document",
                json!({"project":"sales","document":"plan","html":HTML_V2,"message":"数字を更新"}),
                &[AUTHOR],
            )
            .await?;
        assert!(!is_error, "{text}");
        assert_eq!(serde_json::from_str::<Value>(&text)?["revision_number"], 2);

        let (text, _) = mcp
            .call("list_documents", json!({"project":"sales"}), &[])
            .await?;
        let documents: Value = serde_json::from_str(&text)?;
        assert_eq!(documents[0]["revision_number"], 2);
        assert_eq!(documents[0]["author_name"], "佐藤");

        let (text, _) = mcp
            .call(
                "get_document",
                json!({"project":"sales","document":"plan"}),
                &[],
            )
            .await?;
        let latest: Value = serde_json::from_str(&text)?;
        assert_eq!(latest["html"], HTML_V2);
        assert_eq!(latest["message"], "数字を更新");
        assert_eq!(latest["url"], "http://localhost:8080/sales/plan");
        assert_eq!(latest["latest_revision_number"], 2);

        let (text, _) = mcp
            .call(
                "get_document",
                json!({"project":"sales","document":"plan","revision":1}),
                &[],
            )
            .await?;
        let first: Value = serde_json::from_str(&text)?;
        assert_eq!(first["html"], HTML_V1);
        assert_eq!(first["revision_number"], 1);
        assert_eq!(first["url"], "http://localhost:8080/sales/plan/v/1");
        Ok(())
    }

    /// 版を物理削除（`galley purge`）して番号が欠けても、ほかの版は読めて、消した版は見つからない。
    #[tokio::test]
    async fn get_document_after_revision_is_purged() -> TestResult {
        let mcp = Mcp::connect(None).await?;
        let (text, is_error) = mcp
            .call(
                "create_document",
                json!({"project":"sales","title":"計画","html":HTML_V1,"slug":"plan"}),
                &[AUTHOR],
            )
            .await?;
        assert!(!is_error, "{text}");
        for html in [HTML_V2, "<!doctype html><title>計画</title><p>第3版"] {
            let (text, is_error) = mcp
                .call(
                    "update_document",
                    json!({"project":"sales","document":"plan","html":html,"message":"更新"}),
                    &[AUTHOR],
                )
                .await?;
            assert!(!is_error, "{text}");
        }
        // DB の版だけを消せば足りるので、ストレージは MCP と別の空のものを渡す
        let purge = PurgeDocument::new(
            Arc::new(mcp.database.projects()),
            Arc::new(mcp.database.documents()),
            Arc::new(mcp.database.revisions()),
            Arc::new(ObjectStoreBlobStore::in_memory()),
        );
        let plan = purge.plan("sales", "plan", Some(2)).await?;
        purge.execute(&plan).await?;

        let get = |revision: Option<u32>| {
            let arguments = match revision {
                Some(n) => json!({"project":"sales","document":"plan","revision":n}),
                None => json!({"project":"sales","document":"plan"}),
            };
            mcp.call("get_document", arguments, &[])
        };
        let (text, is_error) = get(Some(2)).await?;
        assert!(is_error, "{text}");
        let (text, is_error) = get(Some(1)).await?;
        assert!(!is_error, "{text}");
        assert_eq!(serde_json::from_str::<Value>(&text)?["html"], HTML_V1);
        let (text, _) = get(None).await?;
        let latest: Value = serde_json::from_str(&text)?;
        assert_eq!(latest["revision_number"], 3);
        assert_eq!(latest["latest_revision_number"], 3);

        let (text, _) = mcp
            .call("list_documents", json!({"project":"sales"}), &[])
            .await?;
        assert_eq!(
            serde_json::from_str::<Value>(&text)?[0]["revision_number"],
            3
        );
        let (text, _) = mcp
            .call(
                "update_document",
                json!({"project":"sales","document":"plan","html":HTML_V2,"message":"戻す"}),
                &[AUTHOR],
            )
            .await?;
        assert_eq!(serde_json::from_str::<Value>(&text)?["revision_number"], 4);
        Ok(())
    }

    #[tokio::test]
    async fn registration_requires_author_name() -> TestResult {
        let mcp = Mcp::connect(None).await?;
        let (text, is_error) = mcp
            .call(
                "create_document",
                json!({"project":"sales","title":"計画","html":HTML_V1}),
                &[],
            )
            .await?;
        assert!(is_error);
        assert!(text.contains("X-Author-Name"), "{text}");
        Ok(())
    }

    #[tokio::test]
    async fn external_resources_are_reported_for_ai() -> TestResult {
        let mcp = Mcp::connect(None).await?;
        let html =
            "<!doctype html>\n<script src=\"https://cdn.jsdelivr.net/npm/chart.js@4\"></script>";
        let (text, is_error) = mcp
            .call(
                "create_document",
                json!({"project":"sales","title":"グラフ","html":html}),
                &[AUTHOR],
            )
            .await?;
        assert!(is_error);
        assert!(
            text.starts_with("外部リソースを含まない単一 HTML にしてください"),
            "{text}"
        );
        assert!(text.contains("2 行目（スクリプト）"), "{text}");
        assert!(text.contains("chart.js を外部から読み込まず"), "{text}");
        Ok(())
    }

    #[tokio::test]
    async fn unknown_project_or_document_is_a_tool_error() -> TestResult {
        let mcp = Mcp::connect(None).await?;
        let (text, is_error) = mcp
            .call("list_documents", json!({"project":"nope"}), &[])
            .await?;
        assert!(is_error);
        assert_eq!(text, "プロジェクトが見つかりません");
        let (text, is_error) = mcp
            .call(
                "update_document",
                json!({"project":"sales","document":"nope","html":HTML_V1}),
                &[AUTHOR],
            )
            .await?;
        assert!(is_error);
        assert_eq!(text, "資料が見つかりません");
        Ok(())
    }

    #[tokio::test]
    async fn update_in_archived_project_is_a_tool_error() -> TestResult {
        let mcp = Mcp::connect(None).await?;
        let (text, is_error) = mcp
            .call(
                "create_document",
                json!({"project":"sales","title":"事業計画","html":HTML_V1,"slug":"plan"}),
                &[AUTHOR],
            )
            .await?;
        assert!(!is_error, "{text}");

        // 資料はアーカイブせず、プロジェクトだけアーカイブする
        let projects = Arc::new(mcp.database.projects());
        let sales = FindProject::new(projects.clone()).execute("sales").await?;
        ArchiveProject::new(projects).execute(sales.id).await?;

        let (text, is_error) = mcp
            .call(
                "update_document",
                json!({"project":"sales","document":"plan","html":HTML_V2,"message":"数字を更新"}),
                &[AUTHOR],
            )
            .await?;
        assert!(is_error, "{text}");
        // AI が利用者に「プロジェクトをアーカイブから戻す」よう伝えられる文言
        assert!(text.contains("アーカイブしたプロジェクト"), "{text}");
        assert!(text.contains("新しい版の追加もできません"), "{text}");
        assert!(
            text.contains("プロジェクトをアーカイブから戻して"),
            "{text}"
        );

        // 版は増えていない
        let (text, is_error) = mcp
            .call(
                "get_document",
                json!({"project":"sales","document":"plan"}),
                &[],
            )
            .await?;
        assert!(!is_error, "{text}");
        let latest: Value = serde_json::from_str(&text)?;
        assert_eq!(latest["latest_revision_number"], 1);
        assert_eq!(latest["html"], HTML_V1);
        Ok(())
    }

    #[tokio::test]
    async fn archive_document_hides_it_from_the_list() -> TestResult {
        let mcp = Mcp::connect(None).await?;
        for (title, slug) in [("事業計画", "plan"), ("メモ", "memo")] {
            let (text, is_error) = mcp
                .call(
                    "create_document",
                    json!({"project":"sales","title":title,"html":HTML_V1,"slug":slug}),
                    &[AUTHOR],
                )
                .await?;
            assert!(!is_error, "{text}");
        }

        // 更新者名（X-Author-Name）は無くてよい
        let (text, is_error) = mcp
            .call(
                "archive_document",
                json!({"project":"sales","document":"plan"}),
                &[],
            )
            .await?;
        assert!(!is_error, "{text}");
        let archived: Value = serde_json::from_str(&text)?;
        assert_eq!(archived["project"], "sales");
        assert_eq!(archived["slug"], "plan");
        assert_eq!(archived["title"], "事業計画");
        assert_eq!(
            archived["message"],
            "「事業計画」をアーカイブしました。Galley の画面のアーカイブから戻せます"
        );

        let (text, _) = mcp
            .call("list_documents", json!({"project":"sales"}), &[])
            .await?;
        let documents: Value = serde_json::from_str(&text)?;
        let slugs: Vec<&str> = documents
            .as_array()
            .ok_or("一覧が配列でない")?
            .iter()
            .filter_map(|d| d["slug"].as_str())
            .collect();
        assert_eq!(slugs, ["memo"]);

        let (text, is_error) = mcp
            .call(
                "get_document",
                json!({"project":"sales","document":"plan"}),
                &[],
            )
            .await?;
        assert!(!is_error, "{text}");
        assert_eq!(serde_json::from_str::<Value>(&text)?["archived"], true);

        // アーカイブ済みの資料
        let (text, is_error) = mcp
            .call(
                "archive_document",
                json!({"project":"sales","document":"plan"}),
                &[],
            )
            .await?;
        assert!(is_error);
        assert_eq!(text, "この資料はすでにアーカイブしています");
        Ok(())
    }

    #[tokio::test]
    async fn archive_document_rejects_missing_or_archived_project() -> TestResult {
        let mcp = Mcp::connect(None).await?;
        let (text, is_error) = mcp
            .call(
                "archive_document",
                json!({"project":"sales","document":"nope"}),
                &[],
            )
            .await?;
        assert!(is_error);
        assert_eq!(text, "資料が見つかりません");
        let (text, is_error) = mcp
            .call(
                "archive_document",
                json!({"project":"nope","document":"plan"}),
                &[],
            )
            .await?;
        assert!(is_error);
        assert_eq!(text, "プロジェクトが見つかりません");

        let (text, is_error) = mcp
            .call(
                "create_document",
                json!({"project":"sales","title":"事業計画","html":HTML_V1,"slug":"plan"}),
                &[AUTHOR],
            )
            .await?;
        assert!(!is_error, "{text}");
        let projects = Arc::new(mcp.database.projects());
        let sales = FindProject::new(projects.clone()).execute("sales").await?;
        ArchiveProject::new(projects).execute(sales.id).await?;

        let (text, is_error) = mcp
            .call(
                "archive_document",
                json!({"project":"sales","document":"plan"}),
                &[],
            )
            .await?;
        assert!(is_error, "{text}");
        assert!(
            text.starts_with("アーカイブしたプロジェクトの資料は、1件ずつはアーカイブできません"),
            "{text}"
        );

        // 資料はアーカイブしていない
        let (text, _) = mcp
            .call(
                "get_document",
                json!({"project":"sales","document":"plan"}),
                &[],
            )
            .await?;
        assert_eq!(serde_json::from_str::<Value>(&text)?["archived"], false);
        Ok(())
    }

    #[tokio::test]
    async fn token_is_required_when_configured() -> TestResult {
        let mcp = Mcp::new(Some("s3cret")).await?;
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}});
        let (status, headers, _) = mcp.post(&init, &[]).await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            headers.get("www-authenticate").map(|v| v.as_bytes()),
            Some(&b"Bearer"[..])
        );
        let (status, _, _) = mcp
            .post(&init, &[("authorization", "Bearer wrong")])
            .await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let mcp = Mcp::connect(Some("s3cret")).await?;
        let (_, is_error) = mcp
            .call(
                "list_projects",
                json!({}),
                &[("authorization", "Bearer s3cret")],
            )
            .await?;
        assert!(!is_error);
        Ok(())
    }

    fn create_request(html: &str) -> Value {
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_document","arguments":{"project":"sales","title":"大","html":html}}})
    }

    #[tokio::test]
    async fn oversized_request_is_rejected() -> TestResult {
        let mcp = Mcp::connect(None).await?;
        let html = "a".repeat(HtmlSizeLimit::DEFAULT.json_body_limit());
        let (status, _, _) = mcp.post(&create_request(&html), &[AUTHOR]).await?;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        Ok(())
    }

    /// 起動時に設定した上限に、HTML の検査とリクエストの大きさの上限が揃う。
    #[tokio::test]
    async fn configured_size_limit_is_applied() -> TestResult {
        let limit = HtmlSizeLimit::parse("1KB")?;
        let mcp = Mcp::connect_with(Mcp::with_limit(None, limit).await?, None).await?;

        let fits = format!("<p>{}", "a".repeat(1024 - 3));
        let (text, is_error) = mcp
            .call(
                "create_document",
                json!({"project":"sales","title":"小","html":fits}),
                &[AUTHOR],
            )
            .await?;
        assert!(!is_error, "{text}");

        // リクエストの上限（1KB * 2 + 1MiB）より小さければ、ツールのエラーとして上限を伝える
        let (text, is_error) = mcp
            .call(
                "create_document",
                json!({"project":"sales","title":"大","html":format!("{fits}a")}),
                &[AUTHOR],
            )
            .await?;
        assert!(is_error, "{text}");
        assert!(text.contains("上限は 1024 バイト"), "{text}");

        let html = "a".repeat(limit.json_body_limit());
        let (status, _, _) = mcp.post(&create_request(&html), &[AUTHOR]).await?;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        Ok(())
    }

    /// Lambda では、リクエストごとに別の実行環境に当たりうる。initialize を送った環境とは
    /// 別のところでも、ツールを呼べる。
    #[tokio::test]
    async fn tools_work_without_initialize() -> TestResult {
        let mcp = Mcp::new(None).await?;
        let (status, headers, body) = mcp
            .post(&json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}), &[])
            .await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(!headers.contains_key("mcp-session-id"), "{headers:?}");
        assert_eq!(
            headers.get("content-type").map(|v| v.as_bytes()),
            Some(&b"application/json"[..])
        );
        let tools = Mcp::message(&body)?;
        assert_eq!(
            tools["result"]["tools"].as_array().map(Vec::len),
            Some(6),
            "{tools}"
        );

        let (text, is_error) = mcp
            .call(
                "create_document",
                json!({"project":"sales","title":"事業計画","html":HTML_V1,"slug":"plan"}),
                &[AUTHOR],
            )
            .await?;
        assert!(!is_error, "{text}");
        let (text, is_error) = mcp
            .call(
                "get_document",
                json!({"project":"sales","document":"plan"}),
                &[],
            )
            .await?;
        assert!(!is_error, "{text}");
        assert_eq!(serde_json::from_str::<Value>(&text)?["html"], HTML_V1);
        Ok(())
    }

    /// セッションが無いので、通知を受け取る GET（SSE）と、セッションを閉じる DELETE は受け付けない。
    #[tokio::test]
    async fn get_and_delete_are_not_allowed() -> TestResult {
        let mcp = Mcp::new(None).await?;
        let (status, _) = mcp.get("/mcp", "localhost:8080").await?;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        let response = mcp
            .router
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri("/mcp")
                    .header("host", "localhost:8080")
                    .header("mcp-session-id", "anything")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        Ok(())
    }

    const METADATA_PATHS: [&str; 2] = [
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-protected-resource",
    ];

    #[tokio::test]
    async fn oauth_metadata_is_not_found_without_config() -> TestResult {
        let mcp = Mcp::new(None).await?;
        for path in METADATA_PATHS {
            let (status, _) = mcp.get(path, "localhost:8080").await?;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn oauth_metadata_is_returned_when_configured() -> TestResult {
        let oauth = ProtectedResourceMetadata::parse(
            Some("https://galley.example.com/mcp"),
            Some("https://cognito-idp.ap-northeast-1.amazonaws.com/ap-northeast-1_pool"),
            Some("openid,galley/mcp"),
        )?
        .ok_or("メタデータが無い")?;
        let mcp = Mcp::with_oauth(oauth).await?;
        for path in METADATA_PATHS {
            let (status, body) = mcp.get(path, "localhost:8080").await?;
            assert_eq!(status, StatusCode::OK, "{path}: {body}");
            assert_eq!(
                serde_json::from_str::<Value>(&body)?,
                json!({
                    "resource": "https://galley.example.com/mcp",
                    "authorization_servers": [
                        "https://cognito-idp.ap-northeast-1.amazonaws.com/ap-northeast-1_pool"
                    ],
                    "scopes_supported": ["openid", "galley/mcp"],
                }),
                "{path}"
            );
            // Host の検査はメタデータにも効く
            let (status, _) = mcp.get(path, "evil.example.com").await?;
            assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        }
        Ok(())
    }

    /// MCP_TOKEN を設定していても、メタデータはトークンなしで読める（トークンを取る前に読むもの）。
    #[tokio::test]
    async fn oauth_metadata_does_not_require_mcp_token() -> TestResult {
        let oauth = ProtectedResourceMetadata::parse(
            Some("https://galley.example.com/mcp"),
            Some("https://auth.example.com"),
            None,
        )?
        .ok_or("メタデータが無い")?;
        let mcp = Mcp::build(
            Some("s3cret"),
            HtmlSizeLimit::DEFAULT,
            Some(Arc::new(oauth)),
        )
        .await?;
        let (status, _) = mcp.get(METADATA_PATHS[0], "localhost:8080").await?;
        assert_eq!(status, StatusCode::OK);
        Ok(())
    }
}
