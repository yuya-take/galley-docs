//! MCP の OAuth の「保護されたリソースのメタデータ」（RFC 9728）。
//!
//! MCP クライアントは、`/.well-known/oauth-protected-resource/mcp` を読んで、どの認可サーバー
//! （AWS では Cognito）でログインすればよいかを知る。アプリはメタデータを返すだけで、トークンは確かめない。
//! トークンは前段（AWS では API Gateway の JWT オーソライザー）が確かめる。

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::guard::McpToken;

/// 保護されたリソースのメタデータ。`MCP_OAUTH_*` を設定したときだけ作る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OAuthConfigError {
    #[error(
        "MCP_OAUTH_RESOURCE と MCP_OAUTH_AUTHORIZATION_SERVERS は両方を設定してください（どちらか一方だけでは OAuth を案内できません）"
    )]
    Incomplete,
    #[error(
        "{name} の「{value}」は URL として使えません（https://galley.example.com/mcp のように書く）"
    )]
    InvalidUrl { name: &'static str, value: String },
}

impl ProtectedResourceMetadata {
    /// 環境変数の値から作る。`resource` と `authorization_servers` が両方とも空なら `None`
    /// （Docker の既定。メタデータは 404 になる）。`scopes` は省略できる。
    pub fn parse(
        resource: Option<&str>,
        authorization_servers: Option<&str>,
        scopes: Option<&str>,
    ) -> Result<Option<Self>, OAuthConfigError> {
        let resource = resource.map(str::trim).filter(|v| !v.is_empty());
        let authorization_servers = split_list(authorization_servers);
        let resource = match (resource, authorization_servers.is_empty()) {
            (None, true) => return Ok(None),
            (Some(resource), false) => resource,
            _ => return Err(OAuthConfigError::Incomplete),
        };
        validate_url("MCP_OAUTH_RESOURCE", resource)?;
        for server in &authorization_servers {
            validate_url("MCP_OAUTH_AUTHORIZATION_SERVERS", server)?;
        }
        Ok(Some(Self {
            resource: resource.to_owned(),
            authorization_servers,
            scopes_supported: split_list(scopes),
        }))
    }

    /// `MCP_OAUTH_RESOURCE`・`MCP_OAUTH_AUTHORIZATION_SERVERS`・`MCP_OAUTH_SCOPES` を読む。
    ///
    /// アプリはトークンを確かめないので、`MCP_TOKEN` も無ければ、`/mcp` を前段で守っていない限り
    /// 誰でも使える。AWS ではこの組み合わせが正しい（JWT オーソライザーが確かめる）ので、止めずに警告する。
    pub fn from_env(mcp_token: &McpToken) -> Result<Option<Arc<Self>>, OAuthConfigError> {
        let var = |name: &str| std::env::var(name).ok();
        let metadata = Self::parse(
            var("MCP_OAUTH_RESOURCE").as_deref(),
            var("MCP_OAUTH_AUTHORIZATION_SERVERS").as_deref(),
            var("MCP_OAUTH_SCOPES").as_deref(),
        )?;
        let Some(metadata) = metadata else {
            return Ok(None);
        };
        tracing::info!(
            resource = metadata.resource,
            authorization_servers = metadata.authorization_servers.join(", "),
            "MCP の OAuth のメタデータを返します"
        );
        if !mcp_token.is_required() {
            tracing::warn!(
                "MCP_OAUTH_* を設定していて MCP_TOKEN が無いため、アプリは /mcp のトークンを確かめません。\
                 /mcp のトークンは前段（API Gateway の JWT オーソライザーなど）で確かめる前提です。\
                 前段が無いと /mcp を誰でも使えます"
            );
        }
        Ok(Some(Arc::new(metadata)))
    }
}

/// カンマ区切りの値を、空の要素を除いて分ける。
fn split_list(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `http(s)://<ホスト>...` の形で、空白とフラグメントを含まないものに限る（RFC 9728 の `resource` の条件）。
fn validate_url(name: &'static str, value: &str) -> Result<(), OAuthConfigError> {
    let host = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"));
    let ok = host.is_some_and(|rest| {
        !rest.is_empty()
            && !rest.starts_with('/')
            && !rest.contains('#')
            && !rest.chars().any(char::is_whitespace)
    });
    if ok {
        Ok(())
    } else {
        Err(OAuthConfigError::InvalidUrl {
            name,
            value: value.to_owned(),
        })
    }
}

/// `GET /.well-known/oauth-protected-resource[/mcp]`。設定が無ければ 404。
pub async fn protected_resource_metadata(
    State(metadata): State<Option<Arc<ProtectedResourceMetadata>>>,
) -> Response {
    match metadata {
        Some(metadata) => Json(metadata.as_ref()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Not Found",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_means_no_metadata() {
        assert_eq!(ProtectedResourceMetadata::parse(None, None, None), Ok(None));
        assert_eq!(
            ProtectedResourceMetadata::parse(Some(" "), Some(" , "), Some("openid")),
            Ok(None)
        );
    }

    #[test]
    fn parses_lists_and_trims() -> Result<(), OAuthConfigError> {
        let metadata = ProtectedResourceMetadata::parse(
            Some(" https://galley.example.com/mcp "),
            Some("https://cognito-idp.ap-northeast-1.amazonaws.com/pool, https://auth.example.com"),
            Some("openid, galley/mcp ,"),
        )?
        .ok_or(OAuthConfigError::Incomplete)?;
        assert_eq!(metadata.resource, "https://galley.example.com/mcp");
        assert_eq!(
            metadata.authorization_servers,
            [
                "https://cognito-idp.ap-northeast-1.amazonaws.com/pool",
                "https://auth.example.com"
            ]
        );
        assert_eq!(metadata.scopes_supported, ["openid", "galley/mcp"]);
        Ok(())
    }

    #[test]
    fn scopes_are_omitted_when_empty() -> Result<(), Box<dyn std::error::Error>> {
        let metadata = ProtectedResourceMetadata::parse(
            Some("https://galley.example.com/mcp"),
            Some("https://auth.example.com"),
            None,
        )?
        .ok_or("メタデータが無い")?;
        let json = serde_json::to_value(&metadata)?;
        assert_eq!(
            json,
            serde_json::json!({
                "resource": "https://galley.example.com/mcp",
                "authorization_servers": ["https://auth.example.com"],
            })
        );
        Ok(())
    }

    #[test]
    fn one_side_only_is_an_error() {
        assert_eq!(
            ProtectedResourceMetadata::parse(Some("https://galley.example.com/mcp"), None, None),
            Err(OAuthConfigError::Incomplete)
        );
        assert_eq!(
            ProtectedResourceMetadata::parse(None, Some("https://auth.example.com"), None),
            Err(OAuthConfigError::Incomplete)
        );
    }

    #[test]
    fn rejects_non_urls() {
        for resource in [
            "galley.example.com/mcp",
            "https://",
            "https:///mcp",
            "https://galley.example.com/mcp#x",
            "https://galley example.com/mcp",
            "ftp://galley.example.com/mcp",
        ] {
            assert!(
                matches!(
                    ProtectedResourceMetadata::parse(
                        Some(resource),
                        Some("https://auth.example.com"),
                        None
                    ),
                    Err(OAuthConfigError::InvalidUrl {
                        name: "MCP_OAUTH_RESOURCE",
                        ..
                    })
                ),
                "{resource}"
            );
        }
        assert!(matches!(
            ProtectedResourceMetadata::parse(
                Some("https://galley.example.com/mcp"),
                Some("https://auth.example.com,cognito"),
                None
            ),
            Err(OAuthConfigError::InvalidUrl {
                name: "MCP_OAUTH_AUTHORIZATION_SERVERS",
                ..
            })
        ));
    }
}
