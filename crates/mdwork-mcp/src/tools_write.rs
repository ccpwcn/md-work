//! Write MCP tools: note create/update/append/move/delete, bookmarks, export.
//!
//! Every write is atomic, honors the token's notebook and path ACL, and
//! refuses to clobber a note the GUI still holds unsaved edits for.

use std::fs;
use std::path::{Path, PathBuf};

use mdwork_core::TextEncoding;
use mdwork_core::bookmarks::Bookmarks;
use mdwork_core::doc::{self, EncodingHint, Eol};
use mdwork_core::permissions::Scope;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::ErrorData;
use rmcp::service::RequestContext;
use rmcp::{Json, RoleServer, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::{
    MdworkServer, conflict, core_error, doc_dirty, export_failed, hash_conflict, invalid_params,
    io_error, not_found, require_scope,
};

#[derive(Debug, Deserialize, JsonSchema)]
struct WriteNoteArgs {
    notebook_id: String,
    /// Notebook-relative note path, e.g. `工作/设计.md`.
    path: String,
    /// Full replacement text. The note's encoding, BOM and EOL are preserved.
    text: String,
    /// `sha256` from `read_note`; a mismatch fails with CONFLICT instead of overwriting.
    #[serde(default)]
    expected_sha256: Option<String>,
    /// Create the note (and parent folders) when missing; defaults to true.
    #[serde(default)]
    create: Option<bool>,
    /// Validate everything and report the outcome without touching disk.
    #[serde(default)]
    dry_run: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct WriteNoteOut {
    notebook_id: String,
    path: String,
    created: bool,
    dry_run: bool,
    bytes_before: u64,
    bytes_after: u64,
    sha256_after: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AppendNoteArgs {
    notebook_id: String,
    path: String,
    /// Text appended after a single separating newline when needed.
    text: String,
    #[serde(default)]
    expected_sha256: Option<String>,
    /// Create the note when missing; defaults to true.
    #[serde(default)]
    create: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct AppendNoteOut {
    notebook_id: String,
    path: String,
    created: bool,
    bytes_before: u64,
    bytes_after: u64,
    sha256_after: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct MoveNoteArgs {
    notebook_id: String,
    /// Current notebook-relative path.
    from: String,
    /// New notebook-relative path; must not already exist.
    to: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct MoveNoteOut {
    notebook_id: String,
    from: String,
    to: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DeleteNoteArgs {
    notebook_id: String,
    path: String,
    /// Permanently delete instead of moving to the notebook trash; requires
    /// the `delete:permanent` scope.
    #[serde(default)]
    permanent: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct DeleteNoteOut {
    notebook_id: String,
    path: String,
    permanent: bool,
    /// Trash item id when the note was moved to the trash.
    trash_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AddBookmarkArgs {
    notebook_id: String,
    path: String,
    /// Heading title; omit to bookmark the note itself.
    #[serde(default)]
    heading: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct AddBookmarkOut {
    notebook_id: String,
    path: String,
    heading: Option<String>,
    line: usize,
    /// False when the bookmark already existed.
    created: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ExportNoteArgs {
    notebook_id: String,
    path: String,
    /// `pdf` (default) or `png`. PNG output may contain several page files.
    #[serde(default)]
    format: Option<String>,
    /// Output file name without folders; defaults to `<notebook> - <note>`.
    #[serde(default)]
    output_name: Option<String>,
    /// Document title override.
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ExportNoteOut {
    notebook_id: String,
    path: String,
    format: String,
    files: Vec<String>,
    pages: usize,
    /// True when the GUI has unsaved edits, so the export used the saved copy.
    dirty: bool,
    output_dir: String,
    warnings: Vec<String>,
}

#[tool_router(router = write_router, vis = "pub")]
impl MdworkServer {
    #[tool(
        description = "Create or replace a note (atomic); pass expected_sha256 to avoid overwriting concurrent edits",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    fn write_note(
        &self,
        Parameters(args): Parameters<WriteNoteArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<WriteNoteOut>, ErrorData> {
        self.exec(&ctx, "write_note", |token, entry| {
            require_scope(token, Scope::Write)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let id = notebook.marker.id;
            let rel = self.note_rel(&notebook, token, &args.path)?;
            entry.notebook = Some(id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = Some(rel.clone());

            if self.note_is_dirty(&id, &rel) {
                return Err(doc_dirty(&rel));
            }
            let path = notebook.note_path(&rel).map_err(core_error)?;
            let existing = read_existing(&path, &rel)?;
            let (encoding, bom, eol, bytes_before, sha_before) = match &existing {
                Some(bytes) => {
                    let decoded = doc::decode_with_hint(bytes, notebook.encoding_hint(&rel));
                    (
                        decoded.encoding,
                        decoded.bom,
                        decoded.eol,
                        bytes.len() as u64,
                        doc::sha256_hex(bytes),
                    )
                }
                None => {
                    if path.exists() {
                        return Err(conflict(format!("a folder already exists at {rel}")));
                    }
                    if args.create == Some(false) {
                        return Err(not_found(format!("note not found: {rel}")));
                    }
                    (TextEncoding::Utf8, false, Eol::Lf, 0, String::new())
                }
            };
            check_expected(&rel, args.expected_sha256.as_deref(), &sha_before)?;

            let encoded = doc::encode_text(&args.text, encoding, bom, eol).map_err(core_error)?;
            let dry_run = args.dry_run.unwrap_or(false);
            if !dry_run {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|err| io_error(&rel, err))?;
                }
                mdwork_core::atomic::write_atomic(&path, &encoded)
                    .map_err(|err| io_error(&rel, err))?;
                if let Err(err) =
                    notebook.remember_encoding(&rel, EncodingHint { encoding, bom, eol })
                {
                    tracing::warn!(path = %rel, error = %err, "写入编码记录失败");
                }
            }
            entry.bytes = Some(encoded.len() as u64);

            Ok(Json(WriteNoteOut {
                notebook_id: id.to_string(),
                path: rel,
                created: existing.is_none(),
                dry_run,
                bytes_before,
                bytes_after: encoded.len() as u64,
                sha256_after: doc::sha256_hex(&encoded),
            }))
        })
    }

    #[tool(
        description = "Append text to a note, preserving its encoding and EOL style",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    fn append_note(
        &self,
        Parameters(args): Parameters<AppendNoteArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<AppendNoteOut>, ErrorData> {
        self.exec(&ctx, "append_note", |token, entry| {
            require_scope(token, Scope::Write)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let id = notebook.marker.id;
            let rel = self.note_rel(&notebook, token, &args.path)?;
            entry.notebook = Some(id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = Some(rel.clone());

            if self.note_is_dirty(&id, &rel) {
                return Err(doc_dirty(&rel));
            }
            let path = notebook.note_path(&rel).map_err(core_error)?;
            let existing = read_existing(&path, &rel)?;
            let (mut text, encoding, bom, eol, bytes_before, sha_before) = match &existing {
                Some(bytes) => {
                    let decoded = doc::decode_with_hint(bytes, notebook.encoding_hint(&rel));
                    (
                        decoded.text,
                        decoded.encoding,
                        decoded.bom,
                        decoded.eol,
                        bytes.len() as u64,
                        doc::sha256_hex(bytes),
                    )
                }
                None => {
                    if path.exists() {
                        return Err(conflict(format!("a folder already exists at {rel}")));
                    }
                    if args.create == Some(false) {
                        return Err(not_found(format!("note not found: {rel}")));
                    }
                    (
                        String::new(),
                        TextEncoding::Utf8,
                        false,
                        Eol::Lf,
                        0,
                        String::new(),
                    )
                }
            };
            check_expected(&rel, args.expected_sha256.as_deref(), &sha_before)?;

            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&args.text);

            let encoded = doc::encode_text(&text, encoding, bom, eol).map_err(core_error)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|err| io_error(&rel, err))?;
            }
            mdwork_core::atomic::write_atomic(&path, &encoded)
                .map_err(|err| io_error(&rel, err))?;
            if let Err(err) = notebook.remember_encoding(&rel, EncodingHint { encoding, bom, eol })
            {
                tracing::warn!(path = %rel, error = %err, "写入编码记录失败");
            }
            entry.bytes = Some(encoded.len() as u64);

            Ok(Json(AppendNoteOut {
                notebook_id: id.to_string(),
                path: rel,
                created: existing.is_none(),
                bytes_before,
                bytes_after: encoded.len() as u64,
                sha256_after: doc::sha256_hex(&encoded),
            }))
        })
    }

    #[tool(
        description = "Move or rename a note, keeping its bookmarks attached",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    fn move_note(
        &self,
        Parameters(args): Parameters<MoveNoteArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<MoveNoteOut>, ErrorData> {
        self.exec(&ctx, "move_note", |token, entry| {
            require_scope(token, Scope::Write)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let id = notebook.marker.id;
            let from = self.note_rel(&notebook, token, &args.from)?;
            let to = self.note_rel(&notebook, token, &args.to)?;
            entry.notebook = Some(id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = Some(from.clone());

            if self.note_is_dirty(&id, &from) {
                return Err(doc_dirty(&from));
            }
            if self.note_is_dirty(&id, &to) {
                return Err(doc_dirty(&to));
            }
            let hint = notebook.encoding_hint(&from);
            let moved = notebook.rename_entry(&from, &to).map_err(core_error)?;
            if let Some(hint) = hint {
                let _ = notebook.remember_encoding(&moved, hint);
            }
            let _ = notebook.forget_encoding(&from);

            let mut marks = Bookmarks::load(&notebook).map_err(core_error)?;
            marks.rekey(&from, &moved);
            marks.save(&notebook).map_err(core_error)?;

            Ok(Json(MoveNoteOut {
                notebook_id: id.to_string(),
                from,
                to: moved,
            }))
        })
    }

    #[tool(
        description = "Delete a note: to the notebook trash by default, permanently with the delete:permanent scope",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false
        )
    )]
    fn delete_note(
        &self,
        Parameters(args): Parameters<DeleteNoteArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<DeleteNoteOut>, ErrorData> {
        self.exec(&ctx, "delete_note", |token, entry| {
            require_scope(token, Scope::Delete)?;
            let permanent = args.permanent.unwrap_or(false);
            if permanent {
                require_scope(token, Scope::DeletePermanent)?;
            }
            let notebook = self.notebook(token, &args.notebook_id)?;
            let id = notebook.marker.id;
            let rel = self.note_rel(&notebook, token, &args.path)?;
            entry.notebook = Some(id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = Some(rel.clone());

            if self.note_is_dirty(&id, &rel) {
                return Err(doc_dirty(&rel));
            }

            let trash_id = if permanent {
                let path = notebook.note_path(&rel).map_err(core_error)?;
                if !path.is_file() {
                    return Err(not_found(format!("note not found: {rel}")));
                }
                let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                fs::remove_file(&path).map_err(|err| io_error(&rel, err))?;
                entry.bytes = Some(size);
                None
            } else {
                let item = notebook.delete_entry(&rel).map_err(core_error)?;
                Some(item.id.to_string())
            };

            let mut marks = Bookmarks::load(&notebook).map_err(core_error)?;
            marks.prune(&notebook);
            marks.save(&notebook).map_err(core_error)?;
            if permanent {
                let _ = notebook.forget_encoding(&rel);
            }

            Ok(Json(DeleteNoteOut {
                notebook_id: id.to_string(),
                path: rel,
                permanent,
                trash_id,
            }))
        })
    }

    #[tool(
        description = "Bookmark a note or one of its headings (idempotent)",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    fn add_bookmark(
        &self,
        Parameters(args): Parameters<AddBookmarkArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<AddBookmarkOut>, ErrorData> {
        self.exec(&ctx, "add_bookmark", |token, entry| {
            require_scope(token, Scope::Write)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let id = notebook.marker.id;
            let rel = self.note_rel(&notebook, token, &args.path)?;
            entry.notebook = Some(id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = Some(rel.clone());

            let path = notebook.note_path(&rel).map_err(core_error)?;
            let Some(bytes) = read_existing(&path, &rel)? else {
                return Err(not_found(format!("note not found: {rel}")));
            };
            let text = doc::decode_with_hint(&bytes, notebook.encoding_hint(&rel)).text;
            let note_title = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| rel.clone());

            let heading = args
                .heading
                .as_deref()
                .map(str::trim)
                .filter(|heading| !heading.is_empty());
            let mut marks = Bookmarks::load(&notebook).map_err(core_error)?;
            if let Some(heading) = heading {
                if let Some(existing) = marks
                    .entries
                    .iter()
                    .find(|mark| mark.rel_path == rel && mark.heading.as_deref() == Some(heading))
                {
                    return Ok(Json(AddBookmarkOut {
                        notebook_id: id.to_string(),
                        path: rel,
                        heading: Some(heading.to_string()),
                        line: existing.line,
                        created: false,
                    }));
                }
                let outline = mdwork_render::preview::PreviewIr::from_markdown(&text).outline();
                let Some(item) = outline.iter().find(|item| item.title == heading) else {
                    return Err(not_found(format!("heading not found in {rel}: {heading}")));
                };
                let line = item.line;
                marks.toggle_heading(&rel, &note_title, heading, line);
                marks.save(&notebook).map_err(core_error)?;
                Ok(Json(AddBookmarkOut {
                    notebook_id: id.to_string(),
                    path: rel,
                    heading: Some(heading.to_string()),
                    line,
                    created: true,
                }))
            } else {
                if let Some(existing) = marks.note(&rel) {
                    return Ok(Json(AddBookmarkOut {
                        notebook_id: id.to_string(),
                        path: rel,
                        heading: None,
                        line: existing.line,
                        created: false,
                    }));
                }
                marks.toggle_note(&rel, &note_title);
                marks.save(&notebook).map_err(core_error)?;
                Ok(Json(AddBookmarkOut {
                    notebook_id: id.to_string(),
                    path: rel,
                    heading: None,
                    line: 1,
                    created: true,
                }))
            }
        })
    }

    #[tool(
        description = "Export a note to PDF/PNG under the app exports folder and return the file paths",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false
        )
    )]
    fn export_note(
        &self,
        Parameters(args): Parameters<ExportNoteArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<ExportNoteOut>, ErrorData> {
        self.exec(&ctx, "export_note", |token, entry| {
            require_scope(token, Scope::Export)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let id = notebook.marker.id;
            let rel = self.note_rel(&notebook, token, &args.path)?;
            entry.notebook = Some(id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = Some(rel.clone());

            let path = notebook.note_path(&rel).map_err(core_error)?;
            let Some(bytes) = read_existing(&path, &rel)? else {
                return Err(not_found(format!("note not found: {rel}")));
            };
            entry.bytes = Some(bytes.len() as u64);
            let dirty = self.note_is_dirty(&id, &rel);

            let format = match args.format.as_deref().map(|raw| raw.trim().to_lowercase()) {
                None => mdwork_export::ExportFormat::Pdf,
                Some(raw) if raw == "pdf" => mdwork_export::ExportFormat::Pdf,
                Some(raw) if raw == "png" => mdwork_export::ExportFormat::Png,
                Some(raw) => {
                    return Err(invalid_params(format!(
                        "unsupported export format: {raw} (use pdf or png)"
                    )));
                }
            };
            let dir = self
                .ctx
                .exports_dir
                .clone()
                .unwrap_or_else(mdwork_core::paths::exports_dir);
            fs::create_dir_all(&dir).map_err(|err| io_error("exports", err))?;

            let stem = args
                .output_name
                .as_deref()
                .map(|name| sanitize_stem(strip_known_ext(name.trim())))
                .unwrap_or_else(|| {
                    let note_stem = Path::new(&rel)
                        .file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_else(|| rel.clone());
                    sanitize_stem(&format!("{} - {note_stem}", notebook.display_name()))
                });
            let output = unique_output(&dir, &stem, format.extension());

            let request = mdwork_export::ExportRequest {
                notebook_root: notebook.root.clone(),
                note_rel: rel.clone(),
                notebook_name: notebook.display_name().to_string(),
                title: args.title.clone().unwrap_or_default(),
                format,
                output,
                options: Default::default(),
                markdown: None,
                // The exporter owns and deletes its temp dir, so keep the
                // default one next to the output instead of reusing exports/.
                temp_dir: None,
            };
            let mut warnings = Vec::new();
            let outcome = mdwork_export::run(&request, &mut |event| {
                if let mdwork_export::ExportEvent::Warning { message } = event {
                    warnings.push(message);
                }
            })
            .map_err(|err| export_failed(err.to_string()))?;

            Ok(Json(ExportNoteOut {
                notebook_id: id.to_string(),
                path: rel,
                format: format.extension().to_string(),
                files: outcome
                    .paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect(),
                pages: outcome.pages,
                dirty,
                output_dir: dir.display().to_string(),
                warnings,
            }))
        })
    }
}

fn read_existing(path: &Path, rel: &str) -> Result<Option<Vec<u8>>, ErrorData> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_error(rel, err)),
    }
}

fn check_expected(rel: &str, expected: Option<&str>, actual: &str) -> Result<(), ErrorData> {
    let Some(expected) = expected.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    if expected.eq_ignore_ascii_case(actual) {
        Ok(())
    } else {
        Err(hash_conflict(rel, &expected.to_ascii_lowercase(), actual))
    }
}

fn strip_known_ext(name: &str) -> &str {
    let lower = name.to_lowercase();
    for ext in [".pdf", ".png"] {
        if lower.ends_with(ext) {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

fn sanitize_stem(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if cleaned.is_empty() {
        "export".to_string()
    } else {
        cleaned.chars().take(80).collect()
    }
}

fn unique_output(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = dir.join(format!("{stem} ({n}).{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem} - {}.{ext}", Uuid::new_v4().simple()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use mdwork_core::notebook::Notebook;
    use mdwork_core::permissions::TokenRecord;

    use crate::audit::AuditLog;
    use crate::auth::TokenStore;
    use crate::server::ServerContext;

    fn server() -> (MdworkServer, Notebook, TokenRecord) {
        let root = std::env::temp_dir().join(format!("mdwork-mcp-write-{}", Uuid::new_v4()));
        let notebook = Notebook::create(&root.join("nb"), "写测试").unwrap();
        let token = TokenRecord::new(
            "写令牌",
            "unused".to_string(),
            vec![Scope::Admin],
            vec![notebook.marker.id],
        );
        let ctx = Arc::new(ServerContext {
            workspace_root: root.clone(),
            tokens: Arc::new(TokenStore::new(root.join("tokens.bin"))),
            audit: Arc::new(AuditLog::new(root.join("audit"))),
            stdio_token: None,
            dirty_path: Some(root.join("dirty.toml")),
            exports_dir: Some(root.join("exports")),
        });
        (MdworkServer::new(ctx), notebook, token)
    }

    #[test]
    fn write_tools_are_registered() {
        let (server, _notebook, _) = server();
        let names: Vec<String> = server
            .tool_router
            .list_all()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        for expected in [
            "write_note",
            "append_note",
            "move_note",
            "delete_note",
            "add_bookmark",
            "export_note",
        ] {
            assert!(names.iter().any(|name| name == expected), "{names:?}");
        }
        let delete = server.tool_router.get("delete_note").unwrap();
        assert_eq!(
            delete
                .annotations
                .as_ref()
                .and_then(|annotations| annotations.destructive_hint),
            Some(true)
        );
        let write = server.tool_router.get("write_note").unwrap();
        let schema = serde_json::to_value(&write.input_schema).unwrap();
        assert!(schema["properties"].get("expected_sha256").is_some());
        assert!(schema["properties"].get("dry_run").is_some());
        let _ = std::fs::remove_dir_all(server.ctx.workspace_root.clone());
    }

    #[test]
    fn helper_paths_stay_inside_exports() {
        let dir = std::env::temp_dir().join(format!("mdwork-exports-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let first = unique_output(&dir, "笔记 (1)", "pdf");
        assert_eq!(first.file_name().unwrap(), "笔记 (1).pdf");
        fs::write(&first, b"x").unwrap();
        let second = unique_output(&dir, "笔记 (1)", "pdf");
        assert_eq!(second.file_name().unwrap(), "笔记 (1) (2).pdf");
        assert_eq!(sanitize_stem("a/b:c*"), "a_b_c_");
        assert_eq!(strip_known_ext("report.PDF"), "report");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn expected_hash_mismatch_reports_both_hashes() {
        let err = check_expected("a.md", Some("ABC"), "def").unwrap_err();
        let data = err.data.unwrap();
        assert_eq!(data["code"], "CONFLICT");
        assert_eq!(data["expected_sha256"], "abc");
        assert_eq!(data["actual_sha256"], "def");
        assert!(check_expected("a.md", Some("DEF"), "def").is_ok());
        assert!(check_expected("a.md", None, "def").is_ok());
    }
}
