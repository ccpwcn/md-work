//! MCP server: tools, transports, token storage, and audit log.

mod audit;
mod auth;
mod perms;
mod process;
mod server;
mod tools_read;
mod tools_write;

pub use audit::{AuditEntry, AuditLog};
pub use auth::TokenStore;
pub use perms::{LoadedPermissions, PermissionStore};
pub use server::resolve_workspace_root;
pub use server::{
    CODE_CONFLICT, CODE_DOC_DIRTY, CODE_EXPORT_FAILED, CODE_FORBIDDEN, CODE_INVALID_PATH,
    CODE_NOT_FOUND, CODE_UNAUTHORIZED, MdworkServer, ServerContext,
};

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

use crate::server::Caller;

/// Whether a caller must present a valid MCP token.
///
/// Both transports are local-only (process pipes or `127.0.0.1`), so the
/// default is [`TokenPolicy::Optional`]: a connection without a token is a
/// locally trusted caller with full workspace access. Presenting a token is
/// always verified strictly — an invalid token never silently falls back to
/// the trusted identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenPolicy {
    /// A valid token is mandatory; otherwise startup / requests are rejected.
    Required,
    /// Tokens are optional; an absent token means "local trusted caller".
    #[default]
    Optional,
}

impl TokenPolicy {
    pub fn requires_token(self) -> bool {
        matches!(self, Self::Required)
    }

    pub fn from_require_token(require: bool) -> Self {
        if require {
            Self::Required
        } else {
            Self::Optional
        }
    }
}

/// Reads the stdio token, enforcing the policy when none was provided.
/// An explicitly empty value counts as "not provided"; non-Unicode values are
/// always an error.
fn stdio_token_from_env(
    value: Result<String, std::env::VarError>,
    policy: TokenPolicy,
) -> Result<Option<String>> {
    match value {
        Ok(raw) if !raw.trim().is_empty() => Ok(Some(raw)),
        Ok(_) => {
            tracing::warn!("MDWORK_TOKEN 为空，按未提供令牌处理");
            if policy.requires_token() {
                bail!("stdio transport requires MDWORK_TOKEN in the environment");
            }
            Ok(None)
        }
        Err(std::env::VarError::NotPresent) => {
            if policy.requires_token() {
                bail!(
                    "stdio transport requires MDWORK_TOKEN in the environment (use --no-require-token or MDWORK_REQUIRE_TOKEN=0 to allow local connections without a token)"
                );
            }
            Ok(None)
        }
        Err(err) => Err(err).context("读取 MDWORK_TOKEN 失败"),
    }
}

pub async fn serve_stdio(policy: TokenPolicy) -> Result<()> {
    let raw = stdio_token_from_env(std::env::var("MDWORK_TOKEN"), policy)?;
    let mut ctx = ServerContext::load(None, policy)?;
    if let Some(raw) = raw {
        let token = ctx.tokens.verify(&raw).with_context(|| {
            format!("MDWORK_TOKEN rejected for {}", ctx.tokens.path().display())
        })?;
        ctx.stdio_token = Some(token);
    } else {
        tracing::warn!(
            "MDWORK_TOKEN 未设置，stdio 以本地可信模式运行（不限制 scopes、笔记本与权限树）"
        );
    }
    let service = MdworkServer::new(Arc::new(ctx))
        .serve(rmcp::transport::stdio())
        .await
        .context("start MCP stdio transport")?;
    service.waiting().await.context("MCP stdio loop")?;
    Ok(())
}

pub async fn serve_http(port: u16, policy: TokenPolicy) -> Result<()> {
    serve_http_with_ctx(Arc::new(ServerContext::load(None, policy)?), port).await
}

/// Serves Streamable HTTP with the given context; tests inject temp paths here.
pub async fn serve_http_with_ctx(ctx: Arc<ServerContext>, port: u16) -> Result<()> {
    let factory = {
        let ctx = ctx.clone();
        move || Ok(MdworkServer::new(ctx.clone()))
    };
    let service = StreamableHttpService::<MdworkServer, LocalSessionManager>::new(
        factory,
        Default::default(),
        StreamableHttpServerConfig::default(),
    );
    let router =
        axum::Router::new()
            .nest_service("/mcp", service)
            .layer(middleware::from_fn_with_state(
                ctx.clone(),
                authenticate_request,
            ));
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            anyhow::bail!(
                "port {port} is already in use; stop the other md-work MCP server or pick another --port"
            )
        }
        Err(err) => return Err(err).with_context(|| format!("bind 127.0.0.1:{port}")),
    };
    eprintln!(
        "md-work MCP listening on http://127.0.0.1:{port}/mcp (token {})",
        if ctx.token_policy.requires_token() {
            "required"
        } else {
            "optional"
        }
    );
    let parent_pid = std::env::var("MDWORK_PARENT_PID")
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok());
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let ctrl_c = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            let parent_gone = async {
                match parent_pid {
                    Some(pid) => loop {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        if !process::alive(pid) {
                            break;
                        }
                    },
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                _ = ctrl_c => {},
                _ = parent_gone => {},
            }
        })
        .await
        .context("serve MCP over HTTP")
}

/// Resolves the caller for each HTTP request. A presented token is always
/// verified (invalid tokens get 401); an absent token is accepted only when
/// [`TokenPolicy::Optional`] is in effect.
async fn authenticate_request(
    State(ctx): State<Arc<ServerContext>>,
    mut request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let caller = match presented {
        Some(raw) => match ctx.tokens.verify(raw.trim()) {
            Ok(record) => Caller::Token(record),
            Err(err) => {
                tracing::warn!(error = %err, "MCP bearer token 校验失败");
                return unauthorized_response();
            }
        },
        None => {
            if ctx.token_policy.requires_token() {
                return unauthorized_response();
            }
            Caller::Local
        }
    };
    request.extensions_mut().insert(caller);
    next.run(request).await
}

fn unauthorized_response() -> Response {
    let mut response = Response::new(axum::body::Body::from(
        "401 Unauthorized: a valid md-work MCP bearer token is required\n",
    ));
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"md-work\""),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_token_is_optional_unless_policy_requires_it() {
        let missing = || Err(std::env::VarError::NotPresent);
        assert_eq!(
            stdio_token_from_env(missing(), TokenPolicy::Optional).unwrap(),
            None
        );
        assert!(stdio_token_from_env(missing(), TokenPolicy::Required).is_err());

        let empty = || Ok("  ".to_string());
        assert_eq!(
            stdio_token_from_env(empty(), TokenPolicy::Optional).unwrap(),
            None
        );
        assert!(stdio_token_from_env(empty(), TokenPolicy::Required).is_err());

        for policy in [TokenPolicy::Optional, TokenPolicy::Required] {
            assert_eq!(
                stdio_token_from_env(Ok("mdt_x_y".to_string()), policy).unwrap(),
                Some("mdt_x_y".to_string())
            );
        }
    }

    #[test]
    fn token_policy_defaults_to_optional() {
        assert_eq!(TokenPolicy::default(), TokenPolicy::Optional);
        assert!(!TokenPolicy::default().requires_token());
        assert!(TokenPolicy::from_require_token(true).requires_token());
    }
}
