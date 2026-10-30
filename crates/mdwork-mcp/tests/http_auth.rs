//! Streamable HTTP auth: every request needs a valid bearer token.

use std::sync::Arc;
use std::time::Duration;

use mdwork_core::permissions::Scope;
use mdwork_mcp::{AuditLog, ServerContext, TokenStore, serve_http_with_ctx};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

fn temp_ctx() -> (std::path::PathBuf, Arc<ServerContext>, String) {
    let root = std::env::temp_dir().join(format!("mdwork-mcp-http-{}", Uuid::new_v4()));
    let tokens = TokenStore::new(root.join("tokens.bin"));
    let (_, wire) = tokens
        .create("http 测试", vec![Scope::Admin], vec![], vec![], vec![])
        .unwrap();
    let ctx = Arc::new(ServerContext {
        workspace_root: root.clone(),
        tokens: Arc::new(tokens),
        audit: Arc::new(AuditLog::new(root.join("audit"))),
        stdio_token: None,
        dirty_path: Some(root.join("dirty.json")),
        exports_dir: Some(root.join("exports")),
    });
    (root, ctx, wire)
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.local_addr().unwrap().port()
}

async fn request(port: u16, authorization: Option<&str>, body: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect");
    let auth = authorization
        .map(|value| format!("Authorization: {value}\r\n"))
        .unwrap_or_default();
    let raw = format!(
        "POST /mcp HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         {auth}Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(raw.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut response)).await;
    String::from_utf8_lossy(&response).into_owned()
}

fn initialize_body() -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "auth-test", "version": "0" }
        }
    })
    .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn http_requires_a_valid_bearer_token() {
    let (root, ctx, wire) = temp_ctx();
    let port = free_port();
    let server = tokio::spawn(async move { serve_http_with_ctx(ctx, port).await });

    // Wait until the listener accepts connections.
    let mut ready = false;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ready, "server did not start");

    let missing = request(port, None, &initialize_body()).await;
    assert!(missing.starts_with("HTTP/1.1 401"), "{missing}");
    assert!(missing.to_lowercase().contains("www-authenticate"));

    let bad = request(port, Some("Bearer mdt_bogus_secret"), &initialize_body()).await;
    assert!(bad.starts_with("HTTP/1.1 401"), "{bad}");

    let good = request(port, Some(&format!("Bearer {wire}")), &initialize_body()).await;
    assert!(
        !good.starts_with("HTTP/1.1 401"),
        "valid token rejected: {good}"
    );

    server.abort();
    let _ = std::fs::remove_dir_all(root);
}
