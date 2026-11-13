//! MCP server: shared context, token/ACL enforcement, error codes, and
//! note resources.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use mdwork_core::config::AppConfig;
use mdwork_core::dirty::DirtyState;
use mdwork_core::mcp_permissions::{NodeKey, PermLevel, PermissionTree};
use mdwork_core::notebook::Notebook;
use mdwork_core::permissions::{Scope, TokenRecord};
use mdwork_core::workspace::Workspace;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    ErrorCode, ErrorData, ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate, ServerCapabilities, ServerConfig,
};
use rmcp::service::{MaybeSendFuture, RequestContext};
use rmcp::{RoleServer, ServerHandler};
use serde_json::json;
use uuid::Uuid;

use crate::TokenPolicy;
use crate::audit::{AuditEntry, AuditLog};
use crate::auth::TokenStore;
use crate::perms::PermissionStore;

pub const CODE_UNAUTHORIZED: i32 = -32001;
pub const CODE_NOT_FOUND: i32 = -32002;
pub const CODE_FORBIDDEN: i32 = -32003;
pub const CODE_CONFLICT: i32 = -32009;
pub const CODE_DOC_DIRTY: i32 = -32010;
pub const CODE_INVALID_PATH: i32 = -32011;
pub const CODE_EXPORT_FAILED: i32 = -32012;

const MAX_RESOURCES: usize = 500;

pub struct ServerContext {
    pub workspace_root: PathBuf,
    pub tokens: Arc<TokenStore>,
    pub audit: Arc<AuditLog>,
    /// Set for the stdio transport, which carries one token for the process.
    pub stdio_token: Option<TokenRecord>,
    /// Whether a caller must present a valid token; see [`TokenPolicy`].
    pub token_policy: TokenPolicy,
    /// Overrides the GUI dirty-buffer marker; tests and portable runs set it.
    pub dirty_path: Option<PathBuf>,
    /// Overrides the export output folder.
    pub exports_dir: Option<PathBuf>,
    /// Overrides the permission tree file; tests and portable runs set it.
    pub permissions_path: Option<PathBuf>,
}

impl ServerContext {
    /// Resolves the workspace root from `MDWORK_WORKSPACE`, then the GUI
    /// config, then the default `Documents/md-work`.
    pub fn load(
        stdio_token: Option<TokenRecord>,
        token_policy: TokenPolicy,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            workspace_root: resolve_workspace_root(),
            tokens: Arc::new(TokenStore::new(TokenStore::default_path())),
            audit: Arc::new(AuditLog::new(AuditLog::default_dir())),
            stdio_token,
            token_policy,
            dirty_path: None,
            exports_dir: None,
            permissions_path: None,
        })
    }
}

/// Identity behind an MCP call.
///
/// `Local` is only created when a local transport (stdio or loopback HTTP)
/// presents no token and the effective policy allows it. It is fully trusted:
/// every scope, every notebook/note, and no permission-tree gate, because the
/// GUI permission tree defaults to "all hidden" and would otherwise make a
/// token-less CLI useless. A presented token always goes through `Token` with
/// the original ACL and permission-tree checks.
#[derive(Debug, Clone)]
pub(crate) enum Caller {
    Token(TokenRecord),
    Local,
}

impl Caller {
    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    pub(crate) fn id(&self) -> Option<Uuid> {
        match self {
            Self::Token(record) => Some(record.id),
            Self::Local => None,
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Token(record) => &record.name,
            Self::Local => "local (no token)",
        }
    }

    pub(crate) fn scopes(&self) -> Vec<String> {
        match self {
            Self::Token(record) => record
                .scopes
                .iter()
                .map(|scope| scope.as_str().to_string())
                .collect(),
            Self::Local => Scope::ALL
                .iter()
                .map(|scope| scope.as_str().to_string())
                .collect(),
        }
    }

    pub(crate) fn has(&self, scope: Scope) -> bool {
        match self {
            Self::Token(record) => record.has(scope),
            Self::Local => true,
        }
    }

    pub(crate) fn allows_notebook(&self, notebook_id: &Uuid) -> bool {
        match self {
            Self::Token(record) => record.allows_notebook(notebook_id),
            Self::Local => true,
        }
    }

    pub(crate) fn allows_note(&self, notebook_id: &Uuid, rel_path: &str) -> bool {
        match self {
            Self::Token(record) => record.allows_note(notebook_id, rel_path),
            Self::Local => true,
        }
    }
}

pub fn resolve_workspace_root() -> PathBuf {
    match std::env::var_os("MDWORK_WORKSPACE") {
        Some(raw) if !raw.is_empty() => PathBuf::from(raw),
        _ => AppConfig::load_or_default()
            .resolve_workspace_root()
            .unwrap_or_else(|_| PathBuf::from("md-work")),
    }
}

#[derive(Clone)]
pub struct MdworkServer {
    pub(crate) ctx: Arc<ServerContext>,
    pub(crate) tool_router: ToolRouter<Self>,
    /// 会话级权限快照：建立会话时加载一次，之后的权限变更只影响新会话。
    pub(crate) perms: Arc<PermissionTree>,
}

impl MdworkServer {
    pub fn new(ctx: Arc<ServerContext>) -> Self {
        let store = PermissionStore::new(
            ctx.permissions_path
                .clone()
                .unwrap_or_else(PermissionStore::default_path),
        );
        let loaded = store.load();
        if loaded.tampered {
            tracing::warn!("MCP 权限文件校验失败，本会话按全部隐藏处理");
            log_quietly(
                &ctx.audit,
                &AuditEntry::new("permissions/load", "").error("TAMPERED"),
            );
        }
        let mut tool_router = Self::read_router();
        tool_router.merge(Self::write_router());
        Self {
            ctx,
            tool_router,
            perms: Arc::new(loaded.tree),
        }
    }

    pub fn workspace(&self) -> Workspace {
        Workspace::new(self.ctx.workspace_root.clone())
    }

    /// Resolves the caller: fixed for stdio, per-request for HTTP where the
    /// auth middleware stores the verified caller in the request extensions.
    pub(crate) fn token(&self, ctx: &RequestContext<RoleServer>) -> Result<Caller, ErrorData> {
        if let Some(record) = &self.ctx.stdio_token {
            return Ok(Caller::Token(record.clone()));
        }
        if let Some(parts) = ctx.extensions.get::<axum::http::request::Parts>() {
            return parts
                .extensions
                .get::<Caller>()
                .cloned()
                .ok_or_else(|| unauthorized("missing bearer token"));
        }
        // No HTTP request parts: this is the stdio transport. The middleware
        // always inserts a caller for HTTP, so a missing one is never widened.
        if self.ctx.token_policy.requires_token() {
            Err(unauthorized("missing bearer token"))
        } else {
            Ok(Caller::Local)
        }
    }

    /// Runs a tool body with audit logging. The body fills in notebook, path
    /// and byte counts on `entry` when it knows them.
    pub(crate) fn exec<T>(
        &self,
        ctx: &RequestContext<RoleServer>,
        tool: &'static str,
        body: impl FnOnce(&Caller, &mut AuditEntry) -> Result<T, ErrorData>,
    ) -> Result<T, ErrorData> {
        let started = Instant::now();
        let mut entry = AuditEntry::new(tool, "");
        entry.request_id = ctx.id.to_string();
        entry.client = ctx
            .client_info()
            .map(|info| format!("{} {}", info.name, info.version));

        let token = match self.token(ctx) {
            Ok(token) => token,
            Err(err) => {
                entry.result = "error:UNAUTHORIZED".to_string();
                entry.duration_ms = elapsed_ms(started);
                log_quietly(self.ctx.audit.as_ref(), &entry);
                return Err(err);
            }
        };
        entry.token_id = token.id();
        entry.token_name = token.name().to_string();

        let result = body(&token, &mut entry);
        if let Err(err) = &result {
            entry.result = format!("error:{}", error_label(err));
        }
        entry.duration_ms = elapsed_ms(started);
        log_quietly(self.ctx.audit.as_ref(), &entry);
        result
    }

    /// Looks up a notebook the caller may access.
    pub(crate) fn notebook(
        &self,
        token: &Caller,
        notebook_id: &str,
    ) -> Result<Notebook, ErrorData> {
        let id = Uuid::parse_str(notebook_id.trim())
            .map_err(|_| invalid_params(format!("invalid notebook id: {notebook_id}")))?;
        if !token.allows_notebook(&id) {
            return Err(forbidden("notebook is not allowed for this token"));
        }
        if !token.is_local() && !self.perms.allows(&NodeKey::notebook(id), PermLevel::Read) {
            return Err(forbidden("notebook is hidden by the permission tree"));
        }
        self.workspace()
            .find_by_id(&id)
            .ok_or_else(|| not_found(format!("notebook not found: {id}")))
    }

    /// Normalizes and ACL-checks a note path; rejects escapes before touching disk.
    pub(crate) fn note_rel(
        &self,
        notebook: &Notebook,
        token: &Caller,
        path: &str,
    ) -> Result<String, ErrorData> {
        let rel = notebook.normalize_note_rel(path).map_err(core_error)?;
        if !token.allows_note(&notebook.marker.id, &rel) {
            return Err(forbidden("note is not allowed for this token"));
        }
        // 缺失路径放行给下游报 NOT_FOUND；存在的隐藏笔记一律 FORBIDDEN。
        if !token.is_local()
            && self.note_exists(notebook, &rel)?
            && !self
                .perms
                .allows(&NodeKey::entry(notebook.marker.id, &rel), PermLevel::Read)
        {
            return Err(forbidden("note is hidden by the permission tree"));
        }
        Ok(rel)
    }

    /// 路径在磁盘上是否已存在（新建与缺失走不同的授权/报错路径）。
    /// 入参须已规范化和 ACL 检查；不能用 note_path——它会给文件夹补上 .md，
    /// 导致父链上的文件夹永远被当作“不存在”而跳过其授权。
    fn note_exists(&self, notebook: &Notebook, rel: &str) -> Result<bool, ErrorData> {
        Ok(notebook.to_path(rel).map_err(core_error)?.exists())
    }

    /// 权限树门禁：有效权限（祖先链取最小）须达到指定等级；本地可信身份不受限。
    pub(crate) fn require_perm(
        &self,
        token: &Caller,
        key: &NodeKey,
        level: PermLevel,
    ) -> Result<(), ErrorData> {
        if token.is_local() || self.perms.allows(key, level) {
            Ok(())
        } else {
            Err(forbidden(format!(
                "permission tree requires '{}' on {key}",
                level.as_str()
            )))
        }
    }

    /// 修改级授权：已存在节点查自身，新建节点沿父链找最近存在祖先。
    pub(crate) fn require_modify(
        &self,
        token: &Caller,
        notebook: &Notebook,
        rel: &str,
    ) -> Result<(), ErrorData> {
        let mut target = NodeKey::entry(notebook.marker.id, rel);
        while let Some(path) = &target.path {
            if self.note_exists(notebook, path)? {
                break;
            }
            target = target.parent().expect("非笔记本节点必有父节点");
        }
        self.require_perm(token, &target, PermLevel::Modify)
    }

    /// 删除级授权：节点存在才检查；缺失放行由下游报 NOT_FOUND。
    pub(crate) fn require_delete(
        &self,
        token: &Caller,
        notebook: &Notebook,
        rel: &str,
    ) -> Result<(), ErrorData> {
        if self.note_exists(notebook, rel)? {
            self.require_perm(
                token,
                &NodeKey::entry(notebook.marker.id, rel),
                PermLevel::Delete,
            )?;
        }
        Ok(())
    }

    /// 重命名/移动后把权限记录迁移到新路径。
    ///
    /// 权限表按路径存储，不迁移时新路径因无记录而默认隐藏、旧路径残留授权
    /// 可能被后来同名的新笔记继承。当前会话的 `perms` 快照不变，与新会话一致。
    pub(crate) fn rekey_permissions(&self, notebook_id: Uuid, from: &str, to: &str) {
        let store = PermissionStore::new(
            self.ctx
                .permissions_path
                .clone()
                .unwrap_or_else(PermissionStore::default_path),
        );
        let loaded = store.load();
        if loaded.tampered {
            return;
        }
        let mut tree = loaded.tree;
        if tree.rekey(
            &NodeKey::entry(notebook_id, from),
            &NodeKey::entry(notebook_id, to),
        ) == 0
        {
            return;
        }
        if let Err(err) = store.save(&tree) {
            tracing::warn!(error = %err, "迁移 MCP 权限记录失败");
        }
    }

    /// 令牌 ACL 与权限树的交集：笔记本是否至少可读。
    pub(crate) fn notebook_readable(&self, token: &Caller, id: &Uuid) -> bool {
        token.allows_notebook(id)
            && (token.is_local() || self.perms.allows(&NodeKey::notebook(*id), PermLevel::Read))
    }

    /// 令牌 ACL 与权限树的交集：笔记是否至少可读。
    pub(crate) fn note_readable(&self, token: &Caller, id: &Uuid, rel: &str) -> bool {
        token.allows_note(id, rel)
            && (token.is_local()
                || self
                    .perms
                    .allows(&NodeKey::entry(*id, rel), PermLevel::Read))
    }

    /// True while the GUI still holds unsaved edits for this note. Stale
    /// markers from a crashed GUI are ignored via a pid liveness probe.
    pub(crate) fn note_is_dirty(&self, notebook_id: &Uuid, rel_path: &str) -> bool {
        let state = match &self.ctx.dirty_path {
            Some(path) => DirtyState::load_from(path).unwrap_or_default(),
            None => DirtyState::load(),
        };
        if state.pid == 0 || !crate::process::alive(state.pid) {
            return false;
        }
        state.is_dirty(notebook_id, rel_path)
    }

    fn list_resources_sync(
        &self,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.exec(ctx, "resources/list", |token, _entry| {
            require_scope(token, Scope::Read)?;
            let mut resources = Vec::new();
            'outer: for notebook in self.workspace().scan().notebooks {
                let id = notebook.marker.id;
                if !self.notebook_readable(token, &id) {
                    continue;
                }
                for entry in notebook.list_entries().unwrap_or_default() {
                    if !self.note_readable(token, &id, &entry.rel_path) {
                        continue;
                    }
                    resources.push(
                        Resource::new(note_uri(&id, &entry.rel_path), entry.rel_path.clone())
                            .with_title(entry.name.clone())
                            .with_mime_type("text/markdown")
                            .with_size(entry.size),
                    );
                    if resources.len() >= MAX_RESOURCES {
                        break 'outer;
                    }
                }
            }
            Ok(ListResourcesResult::with_all_items(resources))
        })
    }

    fn list_resource_templates_sync(
        &self,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        self.exec(ctx, "resources/templates", |token, _entry| {
            require_scope(token, Scope::Read)?;
            let template =
                ResourceTemplate::new("mdwork://notebook/{notebook_id}/note/{note_path}", "note")
                    .with_title("MoJiān note")
                    .with_description("A Markdown note; note_path is percent-encoded.")
                    .with_mime_type("text/markdown");
            Ok(ListResourceTemplatesResult::with_all_items(vec![template]))
        })
    }

    fn read_resource_sync(
        &self,
        request: &ReadResourceRequestParams,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let uri = request.uri.clone();
        self.exec(ctx, "resources/read", |token, entry| {
            require_scope(token, Scope::Read)?;
            let (notebook_id, raw_path) = parse_note_uri(&uri)
                .ok_or_else(|| invalid_params(format!("unsupported resource uri: {uri}")))?;
            let notebook = self.notebook(token, &notebook_id.to_string())?;
            let rel = self.note_rel(&notebook, token, &raw_path)?;
            let path = notebook.note_path(&rel).map_err(core_error)?;
            let bytes = std::fs::read(&path).map_err(|err| io_error(&rel, err))?;
            let decoded = mdwork_core::doc::decode(&bytes);
            entry.notebook = Some(notebook_id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = Some(rel.clone());
            entry.bytes = Some(bytes.len() as u64);
            Ok(ReadResourceResult::new(vec![
                ResourceContents::text(decoded.text, uri.clone()).with_mime_type("text/markdown"),
            ]))
        })
    }
}

#[rmcp::tool_handler(router = self.tool_router)]
impl ServerHandler for MdworkServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_instructions(
            "MoJiān (墨笺) notebook server. When a token is presented, calls use its notebook and path ACL plus the permission tree; local connections without a token have full workspace access. \
             Notes are UTF-8/GBK text; write_note expects the sha256 returned by read_note. \
             Deletes go to the notebook trash unless delete:permanent is granted.",
        )
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, ErrorData>> + MaybeSendFuture + '_ {
        std::future::ready(self.list_resources_sync(&ctx))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, ErrorData>> + MaybeSendFuture + '_
    {
        std::future::ready(self.list_resource_templates_sync(&ctx))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResponse, ErrorData>> + MaybeSendFuture + '_ {
        std::future::ready(
            self.read_resource_sync(&request, &ctx)
                .map(ReadResourceResponse::from),
        )
    }
}

pub(crate) fn require_scope(token: &Caller, scope: Scope) -> Result<(), ErrorData> {
    if token.has(scope) {
        Ok(())
    } else {
        Err(forbidden(format!(
            "token lacks the '{}' scope",
            scope.as_str()
        )))
    }
}

pub(crate) fn error(code: i32, label: &str, message: impl Into<String>) -> ErrorData {
    ErrorData::new(
        ErrorCode(code),
        message.into(),
        Some(json!({ "code": label })),
    )
}

pub(crate) fn unauthorized(message: impl Into<String>) -> ErrorData {
    error(CODE_UNAUTHORIZED, "UNAUTHORIZED", message)
}

pub(crate) fn forbidden(message: impl Into<String>) -> ErrorData {
    error(CODE_FORBIDDEN, "FORBIDDEN", message)
}

pub(crate) fn not_found(message: impl Into<String>) -> ErrorData {
    error(CODE_NOT_FOUND, "NOT_FOUND", message)
}

pub(crate) fn conflict(message: impl Into<String>) -> ErrorData {
    error(CODE_CONFLICT, "CONFLICT", message)
}

pub(crate) fn invalid_path(message: impl Into<String>) -> ErrorData {
    error(CODE_INVALID_PATH, "INVALID_PATH", message)
}

pub(crate) fn doc_dirty(rel_path: &str) -> ErrorData {
    error(
        CODE_DOC_DIRTY,
        "DOC_DIRTY",
        format!("note has unsaved edits in md-work: {rel_path}; save it there first"),
    )
}

/// `expected_sha256` mismatch: the file changed after it was read.
pub(crate) fn hash_conflict(rel_path: &str, expected: &str, actual: &str) -> ErrorData {
    ErrorData::new(
        ErrorCode(CODE_CONFLICT),
        format!("note changed since it was read: {rel_path}"),
        Some(json!({
            "code": "CONFLICT",
            "expected_sha256": expected,
            "actual_sha256": actual,
        })),
    )
}

pub(crate) fn export_failed(message: impl Into<String>) -> ErrorData {
    error(CODE_EXPORT_FAILED, "EXPORT_FAILED", message)
}

pub(crate) fn invalid_params(message: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(message.into(), Some(json!({ "code": "INVALID_PARAMS" })))
}

pub(crate) fn core_error(err: mdwork_core::Error) -> ErrorData {
    use mdwork_core::Error as CoreError;
    let message = err.to_string();
    match err {
        CoreError::NotFound(_) | CoreError::NotNotebook(_) => not_found(message),
        CoreError::Exists(_) => conflict(message),
        CoreError::InvalidPath(_) | CoreError::ReservedName(_) | CoreError::Escape(_) => {
            invalid_path(message)
        }
        _ => ErrorData::internal_error(message, Some(json!({ "code": "INTERNAL" }))),
    }
}

pub(crate) fn io_error(path: &str, err: std::io::Error) -> ErrorData {
    if err.kind() == std::io::ErrorKind::NotFound {
        not_found(format!("note not found: {path}"))
    } else {
        ErrorData::internal_error(
            format!("{path}: {err}"),
            Some(json!({ "code": "INTERNAL" })),
        )
    }
}

fn error_label(err: &ErrorData) -> String {
    err.data
        .as_ref()
        .and_then(|data| data.get("code"))
        .and_then(|value| value.as_str())
        .unwrap_or("ERROR")
        .to_string()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

fn log_quietly(audit: &AuditLog, entry: &AuditEntry) {
    let _ = audit.append(entry);
}

fn note_uri(notebook_id: &Uuid, rel_path: &str) -> String {
    format!(
        "mdwork://notebook/{}/note/{}",
        notebook_id.simple(),
        encode_uri_path(rel_path)
    )
}

fn parse_note_uri(uri: &str) -> Option<(Uuid, String)> {
    let rest = uri.strip_prefix("mdwork://notebook/")?;
    let (id, rest) = rest.split_once("/note/")?;
    let id = Uuid::parse_str(id).ok()?;
    let path = decode_uri_path(rest);
    (!path.is_empty()).then_some((id, path))
}

fn encode_uri_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.as_bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/');
        if keep {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn decode_uri_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_uris_round_trip_unicode_and_spaces() {
        let id = Uuid::new_v4();
        let uri = note_uri(&id, "工作/设计 稿.md");
        assert!(uri.starts_with("mdwork://notebook/"));
        assert!(!uri.contains(' '));
        let (parsed_id, path) = parse_note_uri(&uri).unwrap();
        assert_eq!(parsed_id, id);
        assert_eq!(path, "工作/设计 稿.md");
    }

    #[test]
    fn note_uri_rejects_other_shapes() {
        assert!(parse_note_uri("mdwork://notebook/x/note/a.md").is_none());
        assert!(parse_note_uri("file:///C:/a.md").is_none());
        assert!(
            parse_note_uri("mdwork://notebook/00000000-0000-0000-0000-000000000000/note/")
                .is_none()
        );
    }

    #[test]
    fn core_errors_map_to_tool_codes() {
        let err = core_error(mdwork_core::Error::InvalidPath("../x.md".to_string()));
        assert_eq!(err.code, ErrorCode(CODE_INVALID_PATH));
        let err = core_error(mdwork_core::Error::NotFound("a.md".to_string()));
        assert_eq!(err.code, ErrorCode(CODE_NOT_FOUND));
        let err = core_error(mdwork_core::Error::Exists("a.md".to_string()));
        assert_eq!(err.code, ErrorCode(CODE_CONFLICT));
        assert_eq!(error_label(&err), "CONFLICT");
    }

    #[test]
    fn read_tools_are_registered_with_schemas() {
        let dir = std::env::temp_dir().join(format!("mdwork-mcp-tools-{}", Uuid::new_v4()));
        let ctx = Arc::new(ServerContext {
            workspace_root: dir.clone(),
            tokens: Arc::new(TokenStore::new(dir.join("tokens.bin"))),
            audit: Arc::new(AuditLog::new(dir.join("audit"))),
            stdio_token: None,
            token_policy: crate::TokenPolicy::Required,
            dirty_path: Some(dir.join("dirty.toml")),
            exports_dir: Some(dir.join("exports")),
            permissions_path: Some(dir.join("permissions.toml")),
        });
        let server = MdworkServer::new(ctx);
        let names: Vec<String> = server
            .tool_router
            .list_all()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        for expected in [
            "server_status",
            "list_notebooks",
            "notebook_info",
            "list_notes",
            "read_note",
            "get_outline",
            "list_bookmarks",
            "search_notes",
        ] {
            assert!(names.iter().any(|name| name == expected), "{names:?}");
        }

        let read_note = server.tool_router.get("read_note").unwrap();
        assert_eq!(
            read_note
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.read_only_hint),
            Some(true)
        );
        let schema = serde_json::to_value(&read_note.input_schema).unwrap();
        assert_eq!(schema["required"][0], "notebook_id");
        assert!(schema["properties"].get("offset").is_some());
        assert!(schema["properties"].get("limit").is_some());

        let search = server.tool_router.get("search_notes").unwrap();
        let schema = serde_json::to_value(&search.input_schema).unwrap();
        assert!(schema["properties"].get("regex").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
