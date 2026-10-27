//! MCP server: tools, transports, token storage, and audit log.

mod audit;
mod auth;
mod process;
mod server;
mod tools_read;
mod tools_write;

pub use audit::{AuditEntry, AuditLog};
pub use auth::TokenStore;
pub use server::{
    MdworkServer, ServerContext, CODE_CONFLICT, CODE_DOC_DIRTY, CODE_EXPORT_FAILED, CODE_FORBIDDEN,
    CODE_INVALID_PATH, CODE_NOT_FOUND, CODE_UNAUTHORIZED,
};
pub use server::resolve_workspace_root;

use std::sync::Arc;

use anyhow::{Context as _, Result};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use mdwork_core::permissions::TokenRecord;
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

pub async fn serve_stdio() -> Result<()> {
    let raw = std::env::var("MDWORK_TOKEN")
        .context("stdio transport requires MDWORK_TOKEN in the environment")?;
    let mut ctx = ServerContext::load(None)?;
    let token = ctx
        .tokens
        .verify(&raw)
        .with_context(|| format!("MDWORK_TOKEN rejected for {}", ctx.tokens.path().display()))?;
    ctx.stdio_token = Some(token);
    let service = MdworkServer::new(Arc::new(ctx))
        .serve(rmcp::transport::stdio())
        .await
        .context("start MCP stdio transport")?;
    service.waiting().await.context("MCP stdio loop")?;
    Ok(())
}

pub async fn serve_http(port: u16) -> Result<()> {
    serve_http_with_ctx(Arc::new(ServerContext::load(None)?), port).await
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
    let router = axum::Router::new().nest_service("/mcp", service).layer(
        middleware::from_fn_with_state(ctx.clone(), require_bearer_token),
    );
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            anyhow::bail!(
                "port {port} is already in use; stop the other md-work MCP server or pick another --port"
            )
        }
        Err(err) => return Err(err).with_context(|| format!("bind 127.0.0.1:{port}")),
    };
    eprintln!("md-work MCP listening on http://127.0.0.1:{port}/mcp");
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

/// Rejects every HTTP request without a valid `Authorization: Bearer mdt_...`
/// header and hands the verified record to the MCP layer through the request
/// extensions.
async fn require_bearer_token(
    State(ctx): State<Arc<ServerContext>>,
    mut request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let verified: Option<TokenRecord> = presented.and_then(|raw| ctx.tokens.verify(raw.trim()).ok());
    match verified {
        Some(record) => {
            request.extensions_mut().insert(record);
            next.run(request).await
        }
        None => {
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
    }
}
