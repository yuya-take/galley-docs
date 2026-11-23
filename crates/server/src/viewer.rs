//! 資料配信（アプリ本体と別オリジン）。
//!
//! 資料の HTML を、実体の SHA-256 で決まる `GET /b/<hash>` で返す。`GET /b/<hash>?download=<slug>` は
//! ファイルとしてダウンロードさせる（ファイル名は `<slug>.html`）。URL だけで中身が決まるので、AWS の構成では
//! CloudFront が S3 から同じ URL で配る（`docs/design.md` の「資料の配信 URL」）。
//!
//! 前の形の `GET /r/<revision_id>` と `GET /r/<revision_id>/download` は、既に配った URL が壊れないよう、
//! 版から実体のハッシュを引いて `/b/<hash>` に振り向ける。
//!
//! 資料内のスクリプトがアプリを操作したり外部と通信したりしないよう、sandbox 付きの CSP を付ける。
//! 404 やリダイレクトなどを含むすべてのレスポンスに同じヘッダーを付ける（URL を直接開かれても同じ制限が
//! かかるように）。

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, Query, State, rejection::QueryRejection},
    http::{HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use galley_core::{
    app::{
        blob::ReadBlob, document::FindDocumentById, error::AppError, revision::FindRevisionById,
    },
    domain::{
        blob::{BlobContent, BlobHash, HTML_CONTENT_TYPE},
        document::DocumentSlug,
        revision::Revision,
        shared::RevisionId,
    },
};
use serde::Deserialize;

use crate::guard::{AllowedHosts, check_host};

pub const CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; media-src data: blob:; connect-src 'none'; form-action 'none'; base-uri 'none'";

/// 実体は中身のハッシュで決まり、ダウンロードのファイル名も URL に入っているので、URL ごとに応答は
/// 変わらない。ブラウザーに長く持たせる。
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// 資料配信から使うユースケース。
pub struct ViewerApp {
    pub read_blob: ReadBlob,
    /// 前の形の URL（`/r/<revision_id>`）から、実体のハッシュを知るのに使う。
    pub find_revision: FindRevisionById,
    /// 前の形のダウンロードの URL から、ファイル名（資料の slug）を知るのに使う。
    pub find_document: FindDocumentById,
}

/// Host の検査で断ったレスポンスにも同じヘッダーを付けるよう、ヘッダーを一番外に置く。
pub fn router(app: Arc<ViewerApp>, allowed_hosts: Arc<AllowedHosts>) -> Router {
    Router::new()
        .route("/b/{hash}", get(serve_blob))
        .route("/r/{revision_id}", get(redirect_revision))
        .route("/r/{revision_id}/download", get(redirect_download))
        .fallback(not_found)
        .with_state(app)
        .layer(middleware::from_fn_with_state(allowed_hosts, check_host))
        .layer(middleware::map_response(security_headers))
}

/// すべてのレスポンスに付けるヘッダー。
async fn security_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // 資料内のリンクから移動したとき、資料の URL を移動先に渡さない
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    response
}

/// `/b/<hash>` の問い合わせ文字列。ほかの名前は無視する。
#[derive(Debug, Deserialize)]
struct BlobQuery {
    /// ダウンロードさせるときの資料の slug。
    download: Option<String>,
}

/// 実体の HTML を返す。`?download=<slug>` があれば、`<slug>.html` としてダウンロードさせる。
///
/// 画面と別のオリジンなので、`<a download>` ではファイル名を付けられない。そのため
/// `Content-Disposition: attachment` を付けて返す。
async fn serve_blob(
    State(app): State<Arc<ViewerApp>>,
    Path(hash): Path<String>,
    query: Result<Query<BlobQuery>, QueryRejection>,
) -> Response {
    // 64 文字の小文字の16進数だけを受け付けるので、ストレージの置き場所の外は指せない
    let Ok(hash) = BlobHash::parse(&hash) else {
        return ServeError::NotFound.into_response();
    };
    let download = match query {
        Ok(Query(BlobQuery { download: None })) => None,
        Ok(Query(BlobQuery {
            download: Some(slug),
        })) => match DocumentSlug::parse(&slug) {
            Ok(slug) => Some(slug),
            Err(_) => return ServeError::InvalidDownload.into_response(),
        },
        // 同じ名前が2回あるなど
        Err(_) => return ServeError::InvalidDownload.into_response(),
    };
    let content = match read_blob(&app, &hash).await {
        Ok(content) => content.into_bytes(),
        Err(err) => return err.into_response(),
    };
    let headers = [
        (header::CONTENT_TYPE, HTML_CONTENT_TYPE),
        (header::CACHE_CONTROL, CACHE_IMMUTABLE),
    ];
    match download {
        None => (headers, content).into_response(),
        Some(slug) => (
            headers,
            [(
                header::CONTENT_DISPOSITION,
                content_disposition(&slug.file_name()),
            )],
            content,
        )
            .into_response(),
    }
}

/// 前の形の `/r/<revision_id>` を `/b/<hash>` に振り向ける。版の実体は変わらないので、恒久的な
/// リダイレクトにして長くキャッシュさせる。
async fn redirect_revision(
    State(app): State<Arc<ViewerApp>>,
    Path(revision_id): Path<String>,
) -> Response {
    match find_revision(&app, &revision_id).await {
        Ok(revision) => (
            [(header::CACHE_CONTROL, CACHE_IMMUTABLE)],
            Redirect::permanent(&blob_path(&revision.blob_hash)),
        )
            .into_response(),
        Err(err) => err.into_response(),
    }
}

/// 前の形の `/r/<revision_id>/download` を `/b/<hash>?download=<slug>` に振り向ける。資料の slug は
/// 変えられるので、一時的なリダイレクトにしてキャッシュさせない（`no-store` は [`security_headers`]）。
async fn redirect_download(
    State(app): State<Arc<ViewerApp>>,
    Path(revision_id): Path<String>,
) -> Response {
    let revision = match find_revision(&app, &revision_id).await {
        Ok(revision) => revision,
        Err(err) => return err.into_response(),
    };
    let document = match app.find_document.execute(revision.document_id).await {
        Ok(document) => document,
        Err(err) => {
            tracing::error!(revision_id, "版の資料を読めません: {err}");
            return ServeError::Failed.into_response();
        }
    };
    let path = format!(
        "{}?download={}",
        blob_path(&revision.blob_hash),
        document.slug
    );
    Redirect::temporary(&path).into_response()
}

/// 実体の URL のパス。
fn blob_path(hash: &BlobHash) -> String {
    format!("/b/{hash}")
}

/// `Content-Disposition` の値。ファイル名は資料の slug（英小文字・数字・ハイフン）と `.html` なので、
/// そのまま引用符で囲める。
fn content_disposition(file_name: &str) -> String {
    format!("attachment; filename=\"{file_name}\"")
}

/// 資料を返せなかったとき。
enum ServeError {
    /// ハッシュや ID が正しくない・見つからない（404）。
    NotFound,
    /// ダウンロードのファイル名（資料の slug）が正しくない（400）。
    InvalidDownload,
    /// DB やストレージから読めない（500）。原因はログに残す。
    Failed,
}

impl IntoResponse for ServeError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => text(StatusCode::NOT_FOUND, "資料が見つかりません"),
            Self::InvalidDownload => text(
                StatusCode::BAD_REQUEST,
                "ダウンロードのファイル名が正しくありません",
            ),
            Self::Failed => text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "資料を読み込めませんでした",
            ),
        }
    }
}

/// 実体を読む。
async fn read_blob(app: &ViewerApp, hash: &BlobHash) -> Result<BlobContent, ServeError> {
    match app.read_blob.execute(hash).await {
        Ok(Some(content)) => Ok(content),
        Ok(None) => Err(ServeError::NotFound),
        Err(err) => {
            tracing::error!(%hash, "資料の実体を読めません: {err}");
            Err(ServeError::Failed)
        }
    }
}

/// 前の形の URL の版を探す。
async fn find_revision(app: &ViewerApp, revision_id: &str) -> Result<Revision, ServeError> {
    let id = RevisionId::parse(revision_id).map_err(|_| ServeError::NotFound)?;
    match app.find_revision.execute(id).await {
        Ok(revision) => Ok(revision),
        Err(AppError::RevisionNotFound) => Err(ServeError::NotFound),
        Err(err) => {
            tracing::error!(%id, "版を読めません: {err}");
            Err(ServeError::Failed)
        }
    }
}

async fn not_found() -> Response {
    ServeError::NotFound.into_response()
}

fn text(status: StatusCode, body: &'static str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use axum::body::Body;
    use galley_core::{
        adapter::{blob_store::ObjectStoreBlobStore, sqlite::SqliteDatabase},
        app::{
            document::{CreateDocument, CreateDocumentInput, SlugSource},
            project::{CreateProject, ProjectInput},
            revision::RevisionInput,
        },
        domain::{revision::RevisionSource, upload::HtmlSizeLimit},
    };
    use http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    const HTML: &str =
        "<!doctype html><title>計画</title><script>document.body.append('ok')</script>";

    /// 資料を1つ登録した資料配信と、その版。資料の slug は `keikaku`。
    async fn viewer() -> Result<(Router, Revision), Box<dyn Error>> {
        let database = SqliteDatabase::in_memory().await?;
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
            slug: SlugSource::FileName("keikaku.html".to_owned()),
            revision: RevisionInput {
                html: HTML.as_bytes().to_vec(),
                message: String::new(),
                author_name: "佐藤".to_owned(),
                source: RevisionSource::Web,
            },
        })
        .await?;
        let app = Arc::new(ViewerApp {
            read_blob: ReadBlob::new(blobs),
            find_revision: FindRevisionById::new(Arc::new(database.revisions())),
            find_document: FindDocumentById::new(Arc::new(database.documents())),
        });
        let allowed = Arc::new(AllowedHosts::parse(None)?);
        Ok((router(app, allowed), created.revision))
    }

    async fn send(router: &Router, method: &str, uri: &str) -> Result<Response, Box<dyn Error>> {
        send_to(router, method, uri, "localhost:8081").await
    }

    async fn send_to(
        router: &Router,
        method: &str,
        uri: &str,
        host: &str,
    ) -> Result<Response, Box<dyn Error>> {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, host)
            .body(Body::empty())?;
        Ok(router.clone().oneshot(request).await?)
    }

    fn header_bytes(response: &Response, name: header::HeaderName) -> Option<&[u8]> {
        response.headers().get(name).map(|v| v.as_bytes())
    }

    #[track_caller]
    fn assert_security_headers(response: &Response) {
        assert_eq!(
            header_bytes(response, header::CONTENT_SECURITY_POLICY),
            Some(CSP.as_bytes())
        );
        assert_eq!(
            header_bytes(response, header::X_CONTENT_TYPE_OPTIONS),
            Some(&b"nosniff"[..])
        );
        assert_eq!(
            header_bytes(response, header::REFERRER_POLICY),
            Some(&b"no-referrer"[..])
        );
    }

    #[tokio::test]
    async fn serves_blob_html_with_headers() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        let response = send(&router, "GET", &format!("/b/{}", revision.blob_hash)).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_security_headers(&response);
        assert_eq!(
            header_bytes(&response, header::CONTENT_TYPE),
            Some(&b"text/html; charset=utf-8"[..])
        );
        assert_eq!(
            header_bytes(&response, header::CACHE_CONTROL),
            Some(CACHE_IMMUTABLE.as_bytes())
        );
        assert_eq!(header_bytes(&response, header::CONTENT_DISPOSITION), None);
        let body = response.into_body().collect().await?.to_bytes();
        assert_eq!(body, HTML.as_bytes());
        Ok(())
    }

    #[tokio::test]
    async fn ignores_other_query_parameters() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        let response = send(&router, "GET", &format!("/b/{}?v=1", revision.blob_hash)).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header_bytes(&response, header::CONTENT_DISPOSITION), None);
        Ok(())
    }

    #[tokio::test]
    async fn download_attaches_html_named_after_slug() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        let uri = format!("/b/{}?download=keikaku", revision.blob_hash);
        let response = send(&router, "GET", &uri).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_security_headers(&response);
        assert_eq!(
            header_bytes(&response, header::CONTENT_TYPE),
            Some(&b"text/html; charset=utf-8"[..])
        );
        assert_eq!(
            header_bytes(&response, header::CONTENT_DISPOSITION),
            Some(&b"attachment; filename=\"keikaku.html\""[..])
        );
        // ファイル名は URL に入っているので、URL ごとの応答は変わらない
        assert_eq!(
            header_bytes(&response, header::CACHE_CONTROL),
            Some(CACHE_IMMUTABLE.as_bytes())
        );
        let body = response.into_body().collect().await?.to_bytes();
        assert_eq!(body, HTML.as_bytes());
        Ok(())
    }

    #[tokio::test]
    async fn download_rejects_names_other_than_slugs() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        for query in [
            "download=",
            "download",
            "download=Keikaku",
            "download=keikaku.html",
            "download=a%22b",
            "download=a%0D%0ASet-Cookie:x",
            "download=..%2Fkeikaku",
            "download=settings",
            "download=keikaku&download=other",
        ] {
            let uri = format!("/b/{}?{query}", revision.blob_hash);
            let response = send(&router, "GET", &uri).await?;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
            assert_security_headers(&response);
            assert_eq!(
                header_bytes(&response, header::CONTENT_DISPOSITION),
                None,
                "{query}"
            );
            assert_eq!(
                header_bytes(&response, header::CACHE_CONTROL),
                Some(&b"no-store"[..]),
                "{query}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejects_anything_but_exact_hashes() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        let hash = revision.blob_hash.as_str();
        let unknown = BlobHash::of(b"not stored");
        for uri in [
            // 形は正しいが、保存していない
            format!("/b/{unknown}"),
            format!("/b/{}", hash.to_uppercase()),
            format!("/b/{}", &hash[..63]),
            format!("/b/{hash}0"),
            format!("/b/{hash}.html"),
            format!("/b/{hash}%00"),
            format!("/b/{hash}/"),
            format!("/b/{hash}/x"),
            format!("/b/{}/{}/{hash}.html", &hash[..2], &hash[2..4]),
            format!("/b/..%2F{}%2F{}%2F{hash}", &hash[..2], &hash[2..4]),
            "/b/..%2F..%2F..%2Fetc%2Fpasswd".to_owned(),
            "/b/%2e%2e".to_owned(),
            "/b/".to_owned(),
            "/b".to_owned(),
            format!("/{}/{}/{hash}.html", &hash[..2], &hash[2..4]),
        ] {
            let response = send(&router, "GET", &uri).await?;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert_security_headers(&response);
        }
        Ok(())
    }

    #[tokio::test]
    async fn old_revision_url_redirects_to_blob() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        let response = send(&router, "GET", &format!("/r/{}", revision.id)).await?;
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_security_headers(&response);
        let location = format!("/b/{}", revision.blob_hash);
        assert_eq!(
            header_bytes(&response, header::LOCATION),
            Some(location.as_bytes())
        );
        // 版の実体は変わらないので、振り向け先も変わらない
        assert_eq!(
            header_bytes(&response, header::CACHE_CONTROL),
            Some(CACHE_IMMUTABLE.as_bytes())
        );

        let response = send(&router, "GET", &location).await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await?.to_bytes();
        assert_eq!(body, HTML.as_bytes());
        Ok(())
    }

    #[tokio::test]
    async fn old_download_url_redirects_with_slug() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        let response = send(&router, "GET", &format!("/r/{}/download", revision.id)).await?;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_security_headers(&response);
        let location = format!("/b/{}?download=keikaku", revision.blob_hash);
        assert_eq!(
            header_bytes(&response, header::LOCATION),
            Some(location.as_bytes())
        );
        // 資料の slug は変えられるので、振り向け先をキャッシュさせない
        assert_eq!(
            header_bytes(&response, header::CACHE_CONTROL),
            Some(&b"no-store"[..])
        );

        let response = send(&router, "GET", &location).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_bytes(&response, header::CONTENT_DISPOSITION),
            Some(&b"attachment; filename=\"keikaku.html\""[..])
        );
        Ok(())
    }

    #[tokio::test]
    async fn head_returns_headers_without_body() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        let response = send(&router, "HEAD", &format!("/b/{}", revision.blob_hash)).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_security_headers(&response);
        let body = response.into_body().collect().await?.to_bytes();
        assert!(body.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn every_error_response_has_headers() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        let (id, hash) = (revision.id, revision.blob_hash);
        let cases = [
            (
                "GET",
                format!("/r/{}", RevisionId::generate()),
                StatusCode::NOT_FOUND,
            ),
            (
                "GET",
                format!("/r/{}", id.to_string().to_uppercase()),
                StatusCode::NOT_FOUND,
            ),
            ("GET", "/r/not-an-id".to_owned(), StatusCode::NOT_FOUND),
            ("GET", "/r/".to_owned(), StatusCode::NOT_FOUND),
            ("GET", "/".to_owned(), StatusCode::NOT_FOUND),
            ("GET", "/r/a/b".to_owned(), StatusCode::NOT_FOUND),
            (
                "GET",
                format!("/r/{}/download", RevisionId::generate()),
                StatusCode::NOT_FOUND,
            ),
            (
                "GET",
                "/r/not-an-id/download".to_owned(),
                StatusCode::NOT_FOUND,
            ),
            ("GET", format!("/r/{id}/download/x"), StatusCode::NOT_FOUND),
            (
                "POST",
                format!("/r/{id}/download"),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            ("POST", format!("/r/{id}"), StatusCode::METHOD_NOT_ALLOWED),
            ("POST", format!("/b/{hash}"), StatusCode::METHOD_NOT_ALLOWED),
            (
                "DELETE",
                format!("/b/{hash}"),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            ("PUT", format!("/b/{hash}"), StatusCode::METHOD_NOT_ALLOWED),
        ];
        for (method, uri, status) in cases {
            let response = send(&router, method, &uri).await?;
            assert_eq!(response.status(), status, "{method} {uri}");
            assert_security_headers(&response);
            assert_eq!(
                header_bytes(&response, header::CACHE_CONTROL),
                Some(&b"no-store"[..]),
                "{method} {uri}"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unknown_hosts_with_headers() -> Result<(), Box<dyn Error>> {
        let (router, revision) = viewer().await?;
        for uri in [
            format!("/b/{}", revision.blob_hash),
            format!("/r/{}", revision.id),
        ] {
            let response = send_to(&router, "GET", &uri, "evil.example.com").await?;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
            assert_security_headers(&response);
        }
        Ok(())
    }

    /// テストごとの一時ディレクトリ。終わったら消す。
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn does_not_serve_blobs_that_fail_hash_check() -> Result<(), Box<dyn Error>> {
        let dir = TempDir(
            std::env::temp_dir().join(format!("galley-viewer-test-{}", RevisionId::generate())),
        );
        let hash = BlobHash::of(HTML.as_bytes());
        // ハッシュの置き場所に、別の中身を書いておく（保存先が壊れた・書き換えられた）
        let path = dir.0.join(hash.storage_key());
        let parent = path.parent().ok_or("親ディレクトリがない")?;
        std::fs::create_dir_all(parent)?;
        std::fs::write(&path, b"<!doctype html><p>tampered</p>")?;

        let database = SqliteDatabase::in_memory().await?;
        let app = Arc::new(ViewerApp {
            read_blob: ReadBlob::new(Arc::new(ObjectStoreBlobStore::local(&dir.0)?)),
            find_revision: FindRevisionById::new(Arc::new(database.revisions())),
            find_document: FindDocumentById::new(Arc::new(database.documents())),
        });
        let router = router(app, Arc::new(AllowedHosts::parse(None)?));
        let response = send(&router, "GET", &format!("/b/{hash}")).await?;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_security_headers(&response);
        assert_eq!(
            header_bytes(&response, header::CACHE_CONTROL),
            Some(&b"no-store"[..])
        );
        let body = response.into_body().collect().await?.to_bytes();
        assert_eq!(body, "資料を読み込めませんでした".as_bytes());
        Ok(())
    }
}
