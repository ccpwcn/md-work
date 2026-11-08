//! MCP 笔记树权限集成测试：默认隐藏、四级矩阵、祖先门禁、双门禁交集、
//! 新建默认隐藏、会话快照隔离、篡改失效关闭。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mdwork_core::mcp_permissions::{GrantMode, NodeKey, PermLevel, PermissionTree};
use mdwork_core::notebook::Notebook;
use mdwork_core::permissions::{Scope, TokenRecord};
use mdwork_mcp::{
    AuditLog, CODE_FORBIDDEN, MdworkServer, PermissionStore, ServerContext, TokenStore,
};
use rmcp::model::{CallToolRequestParams, ErrorCode, ErrorData};
use rmcp::service::{RunningService, ServiceError};
use rmcp::{Peer, RoleClient, ServiceExt};
use serde_json::{Value, json};
use uuid::Uuid;

fn temp_root() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mdwork-mcp-perms-{}", Uuid::new_v4()))
}

fn context(root: &Path, token: TokenRecord) -> Arc<ServerContext> {
    Arc::new(ServerContext {
        workspace_root: root.to_path_buf(),
        tokens: Arc::new(TokenStore::new(root.join("tokens.bin"))),
        audit: Arc::new(AuditLog::new(root.join("audit"))),
        stdio_token: Some(token),
        dirty_path: Some(root.join("dirty.toml")),
        exports_dir: Some(root.join("exports")),
        permissions_path: Some(root.join("permissions.toml")),
    })
}

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

async fn disconnect(
    peer: Peer<RoleClient>,
    client: RunningService<RoleClient, ()>,
    server_task: tokio::task::JoinHandle<()>,
) {
    drop(peer);
    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(10), server_task).await;
}

fn setup_notebook(root: &Path) -> Notebook {
    let notebook = Notebook::create(&root.join("权限笔记"), "权限笔记").unwrap();
    std::fs::create_dir_all(notebook.root.join("工作")).unwrap();
    std::fs::write(notebook.root.join("工作/设计.md"), "# 设计\n正文 ALPHA\n").unwrap();
    std::fs::write(notebook.root.join("根.md"), "# 根\n首页 GAMMA\n").unwrap();
    notebook
}

fn full_token(notebook: &Notebook) -> TokenRecord {
    TokenRecord::new(
        "全量令牌",
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
    )
}

/// 联动授权整棵笔记本树到指定等级。
fn tree_with(notebook: &Notebook, level: PermLevel) -> PermissionTree {
    let mut tree = PermissionTree::default();
    let descendants: Vec<NodeKey> = notebook
        .list_tree()
        .unwrap()
        .iter()
        .map(|entry| NodeKey::entry(notebook.marker.id, &entry.rel_path))
        .collect();
    tree.set_level(&NodeKey::notebook(notebook.marker.id), level, &descendants);
    tree
}

fn save_tree(root: &Path, tree: &PermissionTree) {
    PermissionStore::new(root.join("permissions.toml"))
        .save(tree)
        .unwrap();
}

fn load_tree(root: &Path) -> PermissionTree {
    PermissionStore::new(root.join("permissions.toml"))
        .load()
        .tree
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_by_default_and_invisible_everywhere() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    let (peer, client, server_task) = connect(context(&root, full_token(&notebook))).await;
    let nb = notebook.marker.id.to_string();

    let notebooks = expect_ok(&peer, "list_notebooks", json!({})).await;
    assert_eq!(notebooks["notebooks"].as_array().unwrap().len(), 0);
    let status = expect_ok(&peer, "server_status", json!({})).await;
    assert_eq!(status["notebook_count"], 0);

    expect_code(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    expect_code(
        &peer,
        "list_notes",
        json!({ "notebook_id": nb }),
        CODE_FORBIDDEN,
    )
    .await;
    expect_code(
        &peer,
        "write_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "text": "x" }),
        CODE_FORBIDDEN,
    )
    .await;
    let hits = expect_ok(&peer, "search_notes", json!({ "query": "ALPHA" })).await;
    assert_eq!(hits["hits"].as_array().unwrap().len(), 0, "{hits}");
    let resources = peer.list_resources(None).await.expect("list resources");
    assert!(resources.resources.is_empty());

    disconnect(peer, client, server_task).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_allows_reads_but_no_writes() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    save_tree(&root, &tree_with(&notebook, PermLevel::Read));
    let (peer, client, server_task) = connect(context(&root, full_token(&notebook))).await;
    let nb = notebook.marker.id.to_string();

    let read = expect_ok(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md" }),
    )
    .await;
    assert!(read["text"].as_str().unwrap().contains("ALPHA"));
    let listing = expect_ok(&peer, "list_notes", json!({ "notebook_id": nb })).await;
    let paths: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"工作/设计.md"), "{paths:?}");
    assert!(paths.contains(&"根.md"), "{paths:?}");
    let hits = expect_ok(&peer, "search_notes", json!({ "query": "ALPHA" })).await;
    assert_eq!(hits["hits"].as_array().unwrap().len(), 1);
    expect_ok(
        &peer,
        "get_outline",
        json!({ "notebook_id": nb, "path": "工作/设计.md" }),
    )
    .await;
    let exported = expect_ok(
        &peer,
        "export_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "format": "pdf" }),
    )
    .await;
    assert!(!exported["files"].as_array().unwrap().is_empty());

    for (tool, args) in [
        (
            "write_note",
            json!({ "notebook_id": nb, "path": "工作/设计.md", "text": "x" }),
        ),
        (
            "append_note",
            json!({ "notebook_id": nb, "path": "工作/设计.md", "text": "x" }),
        ),
        (
            "move_note",
            json!({ "notebook_id": nb, "from": "工作/设计.md", "to": "移动.md" }),
        ),
        (
            "delete_note",
            json!({ "notebook_id": nb, "path": "工作/设计.md" }),
        ),
        (
            "add_bookmark",
            json!({ "notebook_id": nb, "path": "工作/设计.md" }),
        ),
        (
            "write_note",
            json!({ "notebook_id": nb, "path": "新建.md", "text": "x" }),
        ),
    ] {
        expect_code(&peer, tool, args, CODE_FORBIDDEN).await;
    }

    disconnect(peer, client, server_task).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn modify_allows_writes_but_not_delete_and_checks_move_target() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    save_tree(&root, &tree_with(&notebook, PermLevel::Modify));
    let (peer, client, server_task) = connect(context(&root, full_token(&notebook))).await;
    let nb = notebook.marker.id.to_string();

    expect_ok(
        &peer,
        "write_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "text": "# 设计\n改写\n" }),
    )
    .await;
    expect_ok(
        &peer,
        "append_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "text": "追加\n" }),
    )
    .await;
    // 新建沿父链向上找最近存在祖先：笔记本为修改级，允许创建。
    let created = expect_ok(
        &peer,
        "write_note",
        json!({ "notebook_id": nb, "path": "新文件夹/草稿.md", "text": "草稿\n" }),
    )
    .await;
    assert_eq!(created["created"], true);
    expect_ok(
        &peer,
        "add_bookmark",
        json!({ "notebook_id": nb, "path": "工作/设计.md" }),
    )
    .await;
    expect_code(
        &peer,
        "delete_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    disconnect(peer, client, server_task).await;

    // 新建的草稿默认隐藏，先授权再移动（模拟用户在权限页联动放行整支）。
    let mut tree = load_tree(&root);
    tree.set_level(
        &NodeKey::entry(notebook.marker.id, "新文件夹"),
        PermLevel::Modify,
        &[NodeKey::entry(notebook.marker.id, "新文件夹/草稿.md")],
    );
    save_tree(&root, &tree);
    let (peer2, client2, task2) = connect(context(&root, full_token(&notebook))).await;
    expect_ok(
        &peer2,
        "move_note",
        json!({ "notebook_id": nb, "from": "新文件夹/草稿.md", "to": "归档/草稿.md" }),
    )
    .await;
    disconnect(peer2, client2, task2).await;

    // 移动后的新路径同样默认隐藏，授权后再测移动目标文件夹只有只读的情形。
    std::fs::create_dir_all(notebook.root.join("只读夹")).unwrap();
    let mut tree = load_tree(&root);
    tree.set_level(
        &NodeKey::entry(notebook.marker.id, "归档"),
        PermLevel::Modify,
        &[NodeKey::entry(notebook.marker.id, "归档/草稿.md")],
    );
    tree.set_level(
        &NodeKey::entry(notebook.marker.id, "只读夹"),
        PermLevel::Read,
        &[],
    );
    save_tree(&root, &tree);
    let (peer3, client3, task3) = connect(context(&root, full_token(&notebook))).await;
    expect_code(
        &peer3,
        "move_note",
        json!({ "notebook_id": nb, "from": "归档/草稿.md", "to": "只读夹/草稿.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    // 只读夹内新建同样被拒。
    expect_code(
        &peer3,
        "write_note",
        json!({ "notebook_id": nb, "path": "只读夹/新.md", "text": "x" }),
        CODE_FORBIDDEN,
    )
    .await;

    disconnect(peer3, client3, task3).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_allows_trash_and_permanent_still_needs_scope() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    save_tree(&root, &tree_with(&notebook, PermLevel::Delete));
    let (peer, client, server_task) = connect(context(&root, full_token(&notebook))).await;
    let nb = notebook.marker.id.to_string();

    let trashed = expect_ok(
        &peer,
        "delete_note",
        json!({ "notebook_id": nb, "path": "根.md" }),
    )
    .await;
    assert_eq!(trashed["permanent"], false);
    assert!(trashed["trash_id"].is_string());
    let permanent = expect_ok(
        &peer,
        "delete_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md", "permanent": true }),
    )
    .await;
    assert_eq!(permanent["permanent"], true);
    disconnect(peer, client, server_task).await;

    // 树权限为删除，但令牌没有 delete:permanent：永久删除仍被拒。
    // 磁盘新建的笔记默认隐藏，先授权（模拟用户在权限页放行）。
    std::fs::write(notebook.root.join("再建.md"), "x").unwrap();
    let mut tree = load_tree(&root);
    tree.set_level(
        &NodeKey::entry(notebook.marker.id, "再建.md"),
        PermLevel::Delete,
        &[],
    );
    save_tree(&root, &tree);
    let mut limited = full_token(&notebook);
    limited.scopes = vec![Scope::Read, Scope::Write, Scope::Delete];
    let (peer, client, server_task) = connect(context(&root, limited)).await;
    expect_code(
        &peer,
        "delete_note",
        json!({ "notebook_id": nb, "path": "再建.md", "permanent": true }),
        CODE_FORBIDDEN,
    )
    .await;
    expect_ok(
        &peer,
        "delete_note",
        json!({ "notebook_id": nb, "path": "再建.md" }),
    )
    .await;

    disconnect(peer, client, server_task).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_ancestor_overrides_child_grant() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    let mut tree = tree_with(&notebook, PermLevel::Delete);
    // 单独授权：文件夹保持隐藏，笔记单独给删除级；祖先链取最小 → 仍不可见。
    tree.mode = GrantMode::Single;
    tree.set_level(
        &NodeKey::entry(notebook.marker.id, "工作"),
        PermLevel::Hidden,
        &[],
    );
    save_tree(&root, &tree);
    assert_eq!(
        tree.level(&NodeKey::entry(notebook.marker.id, "工作/设计.md")),
        PermLevel::Delete
    );
    let (peer, client, server_task) = connect(context(&root, full_token(&notebook))).await;
    let nb = notebook.marker.id.to_string();

    expect_code(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    let listing = expect_ok(&peer, "list_notes", json!({ "notebook_id": nb })).await;
    let paths: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"根.md"), "{paths:?}");
    assert!(!paths.contains(&"工作"), "{paths:?}");
    assert!(!paths.contains(&"工作/设计.md"), "{paths:?}");
    let hits = expect_ok(&peer, "search_notes", json!({ "query": "ALPHA" })).await;
    assert_eq!(hits["hits"].as_array().unwrap().len(), 0, "{hits}");
    let hits = expect_ok(&peer, "search_notes", json!({ "query": "GAMMA" })).await;
    assert_eq!(hits["hits"].as_array().unwrap().len(), 1, "{hits}");

    disconnect(peer, client, server_task).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn newly_created_notes_default_to_hidden() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    save_tree(&root, &tree_with(&notebook, PermLevel::Modify));
    let (peer, client, server_task) = connect(context(&root, full_token(&notebook))).await;
    let nb = notebook.marker.id.to_string();

    let created = expect_ok(
        &peer,
        "write_note",
        json!({ "notebook_id": nb, "path": "新笔记.md", "text": "新内容 NEWSecret\n" }),
    )
    .await;
    assert_eq!(created["created"], true);
    // 新建笔记默认隐藏：创建者会话也不能读回、列举或搜索到。
    expect_code(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "新笔记.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    let listing = expect_ok(&peer, "list_notes", json!({ "notebook_id": nb })).await;
    let paths: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(!paths.contains(&"新笔记.md"), "{paths:?}");
    let hits = expect_ok(&peer, "search_notes", json!({ "query": "NEWSecret" })).await;
    assert_eq!(hits["hits"].as_array().unwrap().len(), 0, "{hits}");
    disconnect(peer, client, server_task).await;

    // 用户在权限页授权后，新会话可读。
    let mut tree = load_tree(&root);
    tree.set_level(
        &NodeKey::entry(notebook.marker.id, "新笔记.md"),
        PermLevel::Read,
        &[],
    );
    save_tree(&root, &tree);
    let (peer, client, server_task) = connect(context(&root, full_token(&notebook))).await;
    let read = expect_ok(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "新笔记.md" }),
    )
    .await;
    assert!(read["text"].as_str().unwrap().contains("NEWSecret"));

    disconnect(peer, client, server_task).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn permission_changes_apply_to_new_sessions_only() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    save_tree(&root, &tree_with(&notebook, PermLevel::Read));
    let (peer_a, client_a, task_a) = connect(context(&root, full_token(&notebook))).await;
    let nb = notebook.marker.id.to_string();
    expect_ok(
        &peer_a,
        "read_note",
        json!({ "notebook_id": nb, "path": "根.md" }),
    )
    .await;
    expect_code(
        &peer_a,
        "write_note",
        json!({ "notebook_id": nb, "path": "根.md", "text": "x" }),
        CODE_FORBIDDEN,
    )
    .await;

    // 升级为删除：已连接的会话 A 不受影响，新会话 B 立即使用新权限。
    save_tree(&root, &tree_with(&notebook, PermLevel::Delete));
    expect_ok(
        &peer_a,
        "read_note",
        json!({ "notebook_id": nb, "path": "根.md" }),
    )
    .await;
    expect_code(
        &peer_a,
        "write_note",
        json!({ "notebook_id": nb, "path": "根.md", "text": "x" }),
        CODE_FORBIDDEN,
    )
    .await;
    let (peer_b, client_b, task_b) = connect(context(&root, full_token(&notebook))).await;
    expect_ok(
        &peer_b,
        "write_note",
        json!({ "notebook_id": nb, "path": "根.md", "text": "乙改写\n" }),
    )
    .await;

    // 收回为全隐藏：B 继续使用旧快照，新会话 C 全部不可见。
    save_tree(&root, &PermissionTree::default());
    expect_ok(
        &peer_b,
        "read_note",
        json!({ "notebook_id": nb, "path": "根.md" }),
    )
    .await;
    let (peer_c, client_c, task_c) = connect(context(&root, full_token(&notebook))).await;
    expect_code(
        &peer_c,
        "read_note",
        json!({ "notebook_id": nb, "path": "根.md" }),
        CODE_FORBIDDEN,
    )
    .await;

    disconnect(peer_a, client_a, task_a).await;
    disconnect(peer_b, client_b, task_b).await;
    disconnect(peer_c, client_c, task_c).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn tampered_file_fails_closed_for_new_sessions_and_audits() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    save_tree(&root, &tree_with(&notebook, PermLevel::Delete));
    let (peer_a, client_a, task_a) = connect(context(&root, full_token(&notebook))).await;
    let nb = notebook.marker.id.to_string();
    expect_ok(
        &peer_a,
        "read_note",
        json!({ "notebook_id": nb, "path": "根.md" }),
    )
    .await;

    // 外部程序篡改权限文件：老会话不中断，新会话失效关闭。
    std::fs::write(root.join("permissions.toml"), "forged = true").unwrap();
    expect_ok(
        &peer_a,
        "read_note",
        json!({ "notebook_id": nb, "path": "根.md" }),
    )
    .await;
    let (peer_b, client_b, task_b) = connect(context(&root, full_token(&notebook))).await;
    expect_code(
        &peer_b,
        "read_note",
        json!({ "notebook_id": nb, "path": "根.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    let notebooks = expect_ok(&peer_b, "list_notebooks", json!({})).await;
    assert_eq!(notebooks["notebooks"].as_array().unwrap().len(), 0);

    let audit = AuditLog::new(root.join("audit"));
    assert!(
        audit
            .recent(100)
            .unwrap()
            .iter()
            .any(|entry| entry.tool == "permissions/load" && entry.result == "error:TAMPERED")
    );

    disconnect(peer_a, client_a, task_a).await;
    disconnect(peer_b, client_b, task_b).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn tree_permission_intersects_token_acl() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    save_tree(&root, &tree_with(&notebook, PermLevel::Delete));

    // 树全放行，但令牌 deny 工作/*：双门禁取交集。
    let mut token = full_token(&notebook);
    token.note_deny = vec!["工作/*".to_string()];
    let (peer, client, server_task) = connect(context(&root, token)).await;
    let nb = notebook.marker.id.to_string();
    expect_code(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "工作/设计.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    expect_ok(
        &peer,
        "read_note",
        json!({ "notebook_id": nb, "path": "根.md" }),
    )
    .await;
    let hits = expect_ok(&peer, "search_notes", json!({ "query": "ALPHA" })).await;
    assert_eq!(hits["hits"].as_array().unwrap().len(), 0, "{hits}");
    disconnect(peer, client, server_task).await;

    // 树为删除级，但令牌只有 read scope：写入仍被拒。
    let read_only = TokenRecord::new(
        "只读令牌",
        "unused".to_string(),
        vec![Scope::Read],
        vec![notebook.marker.id],
    );
    let (peer, client, server_task) = connect(context(&root, read_only)).await;
    expect_code(
        &peer,
        "write_note",
        json!({ "notebook_id": nb, "path": "根.md", "text": "x" }),
        CODE_FORBIDDEN,
    )
    .await;

    disconnect(peer, client, server_task).await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread")]
async fn ungranted_notebook_stays_invisible() {
    let root = temp_root();
    let notebook = setup_notebook(&root);
    let other = Notebook::create(&root.join("未授权笔记本"), "未授权").unwrap();
    std::fs::write(other.root.join("秘密.md"), "秘密 OTHERSecret\n").unwrap();
    save_tree(&root, &tree_with(&notebook, PermLevel::Delete));

    let mut token = full_token(&notebook);
    token.notebooks.push(other.marker.id);
    let (peer, client, server_task) = connect(context(&root, token)).await;
    let notebooks = expect_ok(&peer, "list_notebooks", json!({})).await;
    let names: Vec<&str> = notebooks["notebooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["权限笔记"], "{names:?}");
    let status = expect_ok(&peer, "server_status", json!({})).await;
    assert_eq!(status["notebook_count"], 1);
    expect_code(
        &peer,
        "read_note",
        json!({ "notebook_id": other.marker.id.to_string(), "path": "秘密.md" }),
        CODE_FORBIDDEN,
    )
    .await;
    let hits = expect_ok(&peer, "search_notes", json!({ "query": "OTHERSecret" })).await;
    assert_eq!(hits["hits"].as_array().unwrap().len(), 0, "{hits}");
    let resources = peer.list_resources(None).await.expect("list resources");
    assert_eq!(resources.resources.len(), 2);

    disconnect(peer, client, server_task).await;
    let _ = std::fs::remove_dir_all(&root);
}
