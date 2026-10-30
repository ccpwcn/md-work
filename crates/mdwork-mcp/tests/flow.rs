//! End-to-end MCP flow over an in-memory transport: token auth, ACL, path
//! sandboxing, write conflicts, dirty-buffer protection and export.

use std::sync::Arc;
use std::time::Duration;

use mdwork_core::dirty::{DirtyDoc, DirtyState};
use mdwork_core::notebook::Notebook;
use mdwork_core::permissions::{Scope, TokenRecord};
use mdwork_mcp::{
    AuditLog, CODE_CONFLICT, CODE_DOC_DIRTY, CODE_FORBIDDEN, CODE_INVALID_PATH, CODE_NOT_FOUND,
    MdworkServer, ServerContext, TokenStore,
};
use rmcp::model::{CallToolRequestParams, ErrorCode, ErrorData, ReadResourceRequestParams};
use rmcp::service::{RunningService, ServiceError};
use rmcp::{Peer, RoleClient, ServiceExt};
use serde_json::{Value, json};
use uuid::Uuid;

fn temp_root() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mdwork-mcp-flow-{}", Uuid::new_v4()))
}

fn context(root: &std::path::Path, token: TokenRecord) -> Arc<ServerContext> {
    Arc::new(ServerContext {
        workspace_root: root.to_path_buf(),
        tokens: Arc::new(TokenStore::new(root.join("tokens.bin"))),
        audit: Arc::new(AuditLog::new(root.join("audit"))),
        stdio_token: Some(token),
        dirty_path: Some(root.join("dirty.json")),
        exports_dir: Some(root.join("exports")),
    })
}

/// Connects an in-memory client; callers drop the client service to close the
/// connection and then await the returned server task.
async fn connect(
    ctx: Arc<ServerContext>,
) -> (
    Peer<RoleClient>,
    RunningService<RoleClient, ()>,
    tokio::task::JoinHandle<()>,
) {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    let server_task = tokio::spawn(async move {
        let service = MdworkServer::new(ctx)
            .serve(server_io)
            .await
            .expect("serve");
        let _ = service.waiting().await;
    });
    let client = ().serve(client_io).await.expect("mcp client");
    let peer = client.peer().clone();
    (peer, client, server_task)
}

async fn call(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Result<Value, ServiceError> {
    let arguments = args.as_object().expect("object args").clone();
    let result = peer
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(arguments))
        .await?;
    Ok(result.structured_content.unwrap_or(Value::Null))
}

async fn expect_ok(peer: &Peer<RoleClient>, tool: &str, args: Value) -> Value {
    call(peer, tool, args)
        .await
        .unwrap_or_else(|err| panic!("{tool} failed: {err}"))
}

async fn expect_code(peer: &Peer<RoleClient>, tool: &str, args: Value, code: i32) -> ErrorData {
    match call(peer, tool, args).await {
        Err(ServiceError::McpError(err)) => {
            assert_eq!(err.code, ErrorCode(code), "unexpected code: {err:?}");
            err
        }
        other => panic!("{tool} should have failed with {code}: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn write_flow_honors_acl_conflicts_and_dirty_buffers() {
    let root = temp_root();
    let notebook = Notebook::create(&root.join("集成笔记"), "集成笔记").unwrap();
    let token = TokenRecord::new(
        "集成令牌",
        "unused".to_string(),
        vec![
            Scope::Read,
            Scope::Write,
            Scope::Search,
            Scope::Delete,
            Scope::DeletePermanent,
            Scope::Export,
        ],
        vec![notebook.marker.id],
    );
    let ctx = context(&root, token);
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    let server_ctx = ctx.clone();
    let server_task = tokio::spawn(async move {
        let service = MdworkServer::new(server_ctx)
            .serve(server_io)
            .await
            .expect("serve");
        let _ = service.waiting().await;
    });
    let client = ().serve(client_io).await.expect("mcp client");
    let peer = client.peer().clone();
    let nb = notebook.marker.id.to_string();

    let created = expect_ok(
        &peer,
        "write_note",
        json!({
            "notebook_id": nb,
            "path": "工作/设计.md",
            "text": "# 设计\n正文\n",
        }),
    )
    .await;
    assert_eq!(created["path"], "工作/设计.md");
    assert_eq!(created["created"], true);
    assert_eq!(created["bytes_before"], 0);
    let sha = created["sha256_after"].as_str().unwrap().to_string();
    assert_eq!(
        std::fs::read_to_string(notebook.root.join("工作/设计.md")).unwrap(),
        "# 设计\n正文\n"
    );

    let read = expect_ok(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md" }),
    )
    .await;
    assert_eq!(read["sha256"], sha.as_str());
    assert_eq!(read["text"], "# 设计\n正文");

    let conflict = expect_code(
        &peer,
        "write_note",
        json!({
            "notebook_id": nb,
            "path": "工作/设计.md",
            "text": "覆盖",
            "expected_sha256": "0000",
        }),
        CODE_CONFLICT,
    )
    .await;
    assert_eq!(conflict.data.unwrap()["actual_sha256"], sha.as_str());

    let dirty_path = root.join("dirty.json");
    DirtyState::with_notes(
        std::process::id(),
        vec![DirtyDoc {
            notebook_id: notebook.marker.id,
            rel_path: "工作/设计.md".to_string(),
        }],
    )
    .save_to(&dirty_path)
    .unwrap();
    expect_code(
        &peer,
        "write_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "text": "脏覆盖" }),
        CODE_DOC_DIRTY,
    )
    .await;

    // A marker left behind by a dead process must not block writes.
    DirtyState::with_notes(
        0xFFFF_FFF0,
        vec![DirtyDoc {
            notebook_id: notebook.marker.id,
            rel_path: "工作/设计.md".to_string(),
        }],
    )
    .save_to(&dirty_path)
    .unwrap();
    let updated = expect_ok(
        &peer,
        "write_note",
        json!({
            "notebook_id": nb,
            "path": "工作/设计.md",
            "text": "# 设计\n正文\n",
            "expected_sha256": sha,
        }),
    )
    .await;
    assert_eq!(updated["created"], false);
    let _ = std::fs::remove_file(&dirty_path);

    expect_ok(
        &peer,
        "append_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "text": "## 后续\n补充\n" }),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(notebook.root.join("工作/设计.md")).unwrap(),
        "# 设计\n正文\n## 后续\n补充\n"
    );

    let bookmark = expect_ok(
        &peer,
        "add_bookmark",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "heading": "设计" }),
    )
    .await;
    assert_eq!(bookmark["created"], true);
    assert_eq!(bookmark["line"], 1);
    let again = expect_ok(
        &peer,
        "add_bookmark",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "heading": "设计" }),
    )
    .await;
    assert_eq!(again["created"], false);

    let exported = expect_ok(
        &peer,
        "export_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "format": "pdf" }),
    )
    .await;
    let files = exported["files"].as_array().unwrap();
    assert!(!files.is_empty(), "{exported}");
    for file in files {
        assert!(std::path::Path::new(file.as_str().unwrap()).is_file());
    }
    assert_eq!(exported["dirty"], false);

    let moved = expect_ok(
        &peer,
        "move_note",
        json!({ "notebook_id": nb, "from": "工作/设计.md", "to": "归档/设计稿.md" }),
    )
    .await;
    assert_eq!(moved["to"], "归档/设计稿.md");
    assert!(!notebook.root.join("工作/设计.md").exists());
    assert!(notebook.root.join("归档/设计稿.md").is_file());
    let marks = expect_ok(&peer, "list_bookmarks", json!({ "notebook_id": nb })).await;
    assert_eq!(marks["bookmarks"][0]["path"], "归档/设计稿.md");

    let trashed = expect_ok(
        &peer,
        "delete_note",
        json!({ "notebook_id": nb, "path": "归档/设计稿.md" }),
    )
    .await;
    assert_eq!(trashed["permanent"], false);
    assert!(trashed["trash_id"].is_string());
    assert_eq!(notebook.list_trash().unwrap().len(), 1);
    let marks = expect_ok(&peer, "list_bookmarks", json!({ "notebook_id": nb })).await;
    assert_eq!(marks["bookmarks"].as_array().unwrap().len(), 0);
    expect_code(
        &peer,
        "delete_note",
        json!({ "notebook_id": nb, "path": "归档/设计稿.md", "permanent": true }),
        CODE_NOT_FOUND,
    )
    .await;

    expect_code(
        &peer,
        "write_note",
        json!({ "notebook_id": nb, "path": "../escape.md", "text": "x" }),
        CODE_INVALID_PATH,
    )
    .await;

    let other = Notebook::create(&root.join("别的笔记本"), "别的").unwrap();
    expect_code(
        &peer,
        "write_note",
        json!({
            "notebook_id": other.marker.id.to_string(),
            "path": "a.md",
            "text": "x",
        }),
        CODE_FORBIDDEN,
    )
    .await;

    drop(peer);
    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn read_tools_filter_acl_enforce_scopes_and_audit_without_bodies() {
    let root = temp_root();
    let notebook = Notebook::create(&root.join("工作笔记"), "工作笔记").unwrap();
    std::fs::create_dir_all(notebook.root.join("notes")).unwrap();
    std::fs::write(
        notebook.root.join("notes/design.md"),
        "# 设计\n\n## 细节\n正文 ALPHA\n仅正文标记 BODYSECRET\n",
    )
    .unwrap();
    std::fs::write(
        notebook.root.join("notes/private.md"),
        "# 私密\n秘密 BETA\n",
    )
    .unwrap();
    std::fs::write(notebook.root.join("root.md"), "# 根\n首页 GAMMA\n").unwrap();
    let _other = Notebook::create(&root.join("别的笔记本"), "别的").unwrap();

    let mut token = TokenRecord::new(
        "只读令牌",
        "unused".to_string(),
        vec![Scope::Read, Scope::Search],
        vec![notebook.marker.id],
    );
    token.note_allow = vec!["notes/**".to_string(), "*.md".to_string()];
    token.note_deny = vec!["notes/private*".to_string()];
    let ctx = context(&root, token);
    let (peer, client, server_task) = connect(ctx).await;
    let nb = notebook.marker.id.to_string();

    let status = expect_ok(&peer, "server_status", json!({})).await;
    assert_eq!(status["token_name"], "只读令牌");
    assert_eq!(status["notebook_count"], 1);
    let scopes = status["scopes"].as_array().unwrap();
    assert!(scopes.contains(&json!("read")) && scopes.contains(&json!("search")));

    let notebooks = expect_ok(&peer, "list_notebooks", json!({})).await;
    assert_eq!(notebooks["notebooks"].as_array().unwrap().len(), 1);
    assert_eq!(notebooks["notebooks"][0]["name"], "工作笔记");

    let info = expect_ok(&peer, "notebook_info", json!({ "notebook_id": nb })).await;
    assert_eq!(info["note_count"], 3);
    assert_eq!(info["folder_count"], 1);

    let listing = expect_ok(&peer, "list_notes", json!({ "notebook_id": nb })).await;
    let paths: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"notes/design.md"), "{paths:?}");
    assert!(paths.contains(&"root.md"), "{paths:?}");
    assert!(paths.contains(&"notes"), "{paths:?}");
    assert!(!paths.contains(&"notes/private.md"), "{paths:?}");

    let ranged = expect_ok(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "notes/design.md", "offset": 4, "limit": 1 }),
    )
    .await;
    assert_eq!(ranged["text"], "正文 ALPHA");
    assert_eq!(ranged["start_line"], 4);
    assert_eq!(ranged["end_line"], 4);
    assert_eq!(ranged["total_lines"], 5);
    assert_eq!(ranged["encoding"], "UTF-8");
    assert_eq!(ranged["eol"], "LF");
    assert_eq!(ranged["truncated"], true);
    assert_eq!(ranged["sha256"].as_str().unwrap().len(), 64);

    expect_code(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "notes/private.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    expect_code(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "../escape.md" }),
        CODE_INVALID_PATH,
    )
    .await;
    expect_code(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "notes/missing.md" }),
        CODE_NOT_FOUND,
    )
    .await;

    let outline = expect_ok(
        &peer,
        "get_outline",
        json!({ "notebook_id": nb, "path": "notes/design.md" }),
    )
    .await;
    let headings = outline["headings"].as_array().unwrap();
    assert_eq!(headings.len(), 2);
    assert_eq!(headings[0]["title"], "设计");
    assert_eq!(headings[0]["level"], 1);
    assert_eq!(headings[1]["title"], "细节");
    assert_eq!(headings[1]["line"], 3);

    let marks = expect_ok(&peer, "list_bookmarks", json!({ "notebook_id": nb })).await;
    assert_eq!(marks["bookmarks"].as_array().unwrap().len(), 0);

    let hits = expect_ok(&peer, "search_notes", json!({ "query": "ALPHA" })).await;
    assert_eq!(hits["hits"].as_array().unwrap().len(), 1, "{hits}");
    assert_eq!(hits["hits"][0]["path"], "notes/design.md");
    assert_eq!(hits["hits"][0]["line"], 4);
    assert_eq!(hits["files_scanned"], 2);

    let denied = expect_ok(&peer, "search_notes", json!({ "query": "BETA" })).await;
    assert_eq!(denied["hits"].as_array().unwrap().len(), 0, "{denied}");

    let regex = expect_ok(
        &peer,
        "search_notes",
        json!({ "query": "^## ", "regex": true }),
    )
    .await;
    assert_eq!(regex["hits"].as_array().unwrap().len(), 1);
    assert_eq!(regex["hits"][0]["line"], 3);

    expect_code(
        &peer,
        "write_note",
        json!({ "notebook_id": nb, "path": "notes/design.md", "text": "x" }),
        CODE_FORBIDDEN,
    )
    .await;
    expect_code(
        &peer,
        "append_note",
        json!({ "notebook_id": nb, "path": "notes/design.md", "text": "x" }),
        CODE_FORBIDDEN,
    )
    .await;

    let resources = peer.list_resources(None).await.expect("list resources");
    let uris: Vec<&str> = resources
        .resources
        .iter()
        .map(|item| item.uri.as_str())
        .collect();
    assert_eq!(uris.len(), 2, "{uris:?}");
    let design_uri = format!(
        "mdwork://notebook/{}/note/notes/design.md",
        notebook.marker.id.simple()
    );
    assert!(uris.contains(&design_uri.as_str()), "{uris:?}");
    let read = peer
        .read_resource(ReadResourceRequestParams::new(design_uri.clone()))
        .await
        .expect("read resource");
    let rmcp::model::ResourceContents::TextResourceContents { text, .. } = &read.contents[0] else {
        panic!("expected text resource")
    };
    assert!(text.contains("ALPHA"));
    let private_uri = format!(
        "mdwork://notebook/{}/note/notes/private.md",
        notebook.marker.id.simple()
    );
    assert!(
        peer.read_resource(ReadResourceRequestParams::new(private_uri))
            .await
            .is_err()
    );

    let audit = AuditLog::new(root.join("audit"));
    let recent = audit.recent(100).unwrap();
    assert!(
        recent
            .iter()
            .any(|entry| entry.tool == "read_note" && entry.result == "ok")
    );
    assert!(recent.iter().any(|entry| {
        entry.tool == "search_notes"
            && entry.query.as_deref() == Some("ALPHA")
            && entry.path.is_none()
    }));
    assert!(recent.iter().any(|entry| entry.result == "error:FORBIDDEN"));
    let mut raw = String::new();
    for file in std::fs::read_dir(audit.dir()).unwrap().flatten() {
        raw.push_str(&std::fs::read_to_string(file.path()).unwrap());
    }
    assert!(
        !raw.contains("BODYSECRET"),
        "audit must not record note bodies"
    );

    drop(peer);
    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;

    // A token with only `search` may search but not read or list.
    let search_only = TokenRecord::new(
        "只搜索",
        "unused".to_string(),
        vec![Scope::Search],
        vec![notebook.marker.id],
    );
    let (peer, client, server_task) = connect(context(&root, search_only)).await;
    expect_code(&peer, "list_notebooks", json!({}), CODE_FORBIDDEN).await;
    expect_code(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "notes/design.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    let hits = expect_ok(&peer, "search_notes", json!({ "query": "GAMMA" })).await;
    assert_eq!(hits["hits"].as_array().unwrap().len(), 1, "{hits}");
    drop(peer);
    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;

    let _ = std::fs::remove_dir_all(&root);
}
