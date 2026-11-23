//! 資料配信の URL（アプリの iframe に入れる `src`）。
//!
//! 資料はアプリと別のオリジンから配信する。既定はアプリと同じホスト名で、資料配信のポート（8081）。
//! リバースプロキシで資料に別のホスト名を割り当てるときは `GALLEY_VIEWER_URL` で指定する。

use std::fmt;

use galley_core::domain::{blob::BlobHash, document::DocumentSlug};

use crate::host::{base_url, hostname};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerUrl {
    /// 指定した URL（`https://galley-docs.example.com` など。末尾の `/` は除く）。
    Fixed(String),
    /// アプリへのリクエストと同じホスト名で、このポートにする。
    SameHost { port: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "資料配信の URL「{0}」は http(s)://<ホスト名>[:ポート] の形で指定してください（パスは付けない）"
)]
pub struct ViewerUrlError(String);

impl ViewerUrl {
    /// `http(s)://<ホスト名>[:ポート]` を読む。パス・クエリ・ユーザー情報は受け付けない。
    pub fn parse(value: &str) -> Result<Self, ViewerUrlError> {
        base_url(value)
            .map(Self::Fixed)
            .ok_or_else(|| ViewerUrlError(value.to_owned()))
    }

    /// 指定した URL のホスト名（`host[:port]`）。`SameHost` なら `None`。
    pub fn fixed_authority(&self) -> Option<&str> {
        match self {
            Self::Fixed(base) => base.split_once("://").map(|(_, authority)| authority),
            Self::SameHost { .. } => None,
        }
    }

    /// 版の HTML の URL（`/b/<実体のハッシュ>`）。`app_host` はアプリへのリクエストの Host ヘッダー
    /// （`SameHost` のときに使う）。Host ヘッダーがホスト名として正しくなければ `None`。
    ///
    /// URL は中身だけで決まるので、AWS の構成では CloudFront が S3 から同じ URL で配る。
    pub fn blob_url(&self, app_host: &str, hash: &BlobHash) -> Option<String> {
        Some(format!("{}/b/{hash}", self.base(app_host)?))
    }

    /// 版の HTML をファイル（`<資料の slug>.html`）としてダウンロードする URL
    /// （`/b/<実体のハッシュ>?download=<資料の slug>`。資料ビューアの「HTML をダウンロード」）。
    /// slug は英小文字・数字・ハイフンだけなので、そのまま問い合わせ文字列に入れられる。
    pub fn download_url(
        &self,
        app_host: &str,
        hash: &BlobHash,
        slug: &DocumentSlug,
    ) -> Option<String> {
        Some(format!(
            "{}?download={slug}",
            self.blob_url(app_host, hash)?
        ))
    }

    fn base(&self, app_host: &str) -> Option<String> {
        match self {
            Self::Fixed(base) => Some(base.clone()),
            Self::SameHost { port } => Some(format!("http://{}:{port}", hostname(app_host)?)),
        }
    }
}

impl fmt::Display for ViewerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fixed(base) => f.write_str(base),
            Self::SameHost { port } => write!(f, "http://<アプリと同じホスト名>:{port}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash() -> BlobHash {
        BlobHash::of(b"<!doctype html>")
    }

    fn slug() -> DocumentSlug {
        DocumentSlug::generated(0x3f9a_1c2e)
    }

    #[test]
    fn same_host_uses_viewer_port() {
        let url = ViewerUrl::SameHost { port: 8081 };
        let hash = hash();
        assert_eq!(
            url.blob_url("galley.internal:8080", &hash),
            Some(format!("http://galley.internal:8081/b/{hash}"))
        );
        assert_eq!(
            url.blob_url("192.168.1.10", &hash),
            Some(format!("http://192.168.1.10:8081/b/{hash}"))
        );
        assert_eq!(
            url.blob_url("[::1]:8080", &hash),
            Some(format!("http://[::1]:8081/b/{hash}"))
        );
        assert_eq!(
            url.blob_url("Galley.Internal:8080", &hash),
            Some(format!("http://galley.internal:8081/b/{hash}"))
        );
        assert_eq!(
            url.download_url("galley.internal:8080", &hash, &slug()),
            Some(format!(
                "http://galley.internal:8081/b/{hash}?download=doc-3f9a1c2e"
            ))
        );
    }

    #[test]
    fn same_host_rejects_odd_host_headers() {
        let url = ViewerUrl::SameHost { port: 8081 };
        for host in ["", "evil.com/x", "a b", "\"><script>", "[::1"] {
            assert_eq!(url.blob_url(host, &hash()), None, "{host:?}");
            assert_eq!(url.download_url(host, &hash(), &slug()), None, "{host:?}");
        }
    }

    #[test]
    fn fixed_url_is_used_as_is() {
        let url = ViewerUrl::parse("https://Galley-Docs.example.com/");
        assert_eq!(
            url,
            Ok(ViewerUrl::Fixed(
                "https://galley-docs.example.com".to_owned()
            ))
        );
        let hash = hash();
        assert_eq!(
            url.ok().and_then(|u| u.blob_url("ignored", &hash)),
            Some(format!("https://galley-docs.example.com/b/{hash}"))
        );
        assert!(ViewerUrl::parse("http://localhost:18081").is_ok());
        assert_eq!(
            ViewerUrl::parse("http://docs.internal:8081")
                .ok()
                .as_ref()
                .and_then(ViewerUrl::fixed_authority),
            Some("docs.internal:8081")
        );
        assert_eq!(ViewerUrl::SameHost { port: 1 }.fixed_authority(), None);
        assert!(ViewerUrl::parse("http://[::1]:8081").is_ok());
    }

    #[test]
    fn parse_rejects_paths_and_other_schemes() {
        for value in [
            "galley-docs.example.com",
            "ftp://example.com",
            "https://example.com/docs",
            "https://user@example.com",
            "https://example.com?x=1",
            "https://example.com:99999",
            "https://",
        ] {
            assert!(ViewerUrl::parse(value).is_err(), "{value}");
        }
    }
}
