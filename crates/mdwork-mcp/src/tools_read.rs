//! Read-only MCP tools: notebooks, notes, search, outline, bookmarks.

use std::sync::atomic::AtomicBool;

use mdwork_core::bookmarks::Bookmarks;
use mdwork_core::doc;
use mdwork_core::mcp_permissions::{NodeKey, PermLevel};
use mdwork_core::notebook::EntryKind;
use mdwork_core::permissions::Scope;
use mdwork_core::search::{SearchQuery, search_notebook_filtered};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::ErrorData;
use rmcp::service::RequestContext;
use rmcp::{Json, RoleServer, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::server::{MdworkServer, core_error, invalid_params, invalid_path, require_scope};

const DEFAULT_READ_LIMIT: u64 = 400;
const MAX_READ_LIMIT: u64 = 5_000;
const DEFAULT_SEARCH_HITS: u64 = 200;
const MAX_SEARCH_HITS: u64 = 1_000;

#[derive(Debug, Serialize, JsonSchema)]
struct ServerStatusOut {
    name: String,
    version: String,
    workspace_root: String,
    notebook_count: usize,
    token_name: String,
    scopes: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct NotebookSummary {
    id: String,
    name: String,
    path: String,
    note_count: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ListNotebooksOut {
    notebooks: Vec<NotebookSummary>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct NotebookArgs {
    /// Notebook id as returned by `list_notebooks`.
    notebook_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct NotebookInfoOut {
    id: String,
    name: String,
    path: String,
    created: u64,
    note_count: usize,
    folder_count: usize,
    note_bytes: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ListNotesArgs {
    notebook_id: String,
    /// Optional notebook-relative folder; omit for the whole notebook.
    #[serde(default)]
    folder: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct NoteEntryOut {
    path: String,
    name: String,
    /// `note` or `folder`.
    kind: String,
    size: u64,
    modified: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ListNotesOut {
    notebook_id: String,
    notebook_name: String,
    entries: Vec<NoteEntryOut>,
    total: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadNoteArgs {
    notebook_id: String,
    /// Notebook-relative note path, e.g. `工作/设计.md`.
    path: String,
    /// One-based first line; defaults to 1.
    #[serde(default)]
    offset: Option<u64>,
    /// Maximum lines to return; defaults to 400, capped at 5000.
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct NoteContentOut {
    notebook_id: String,
    notebook_name: String,
    path: String,
    start_line: u64,
    end_line: u64,
    total_lines: u64,
    /// Line content with newlines normalized to `\n`.
    text: String,
    truncated: bool,
    encoding: String,
    eol: String,
    bom: bool,
    /// SHA-256 of the raw file bytes; pass to `write_note` as `expected_sha256`.
    sha256: String,
    size: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GetOutlineArgs {
    notebook_id: String,
    path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct HeadingOut {
    level: u8,
    title: String,
    line: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
struct OutlineOut {
    notebook_id: String,
    path: String,
    headings: Vec<HeadingOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct BookmarkOut {
    path: String,
    title: String,
    heading: Option<String>,
    line: usize,
    created: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ListBookmarksOut {
    notebook_id: String,
    bookmarks: Vec<BookmarkOut>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchArgs {
    /// Plain text or regular expression to find.
    query: String,
    /// Restrict the search to one notebook; omit to search every allowed notebook.
    #[serde(default)]
    notebook_id: Option<String>,
    #[serde(default)]
    case_sensitive: Option<bool>,
    /// Treat `query` as a regular expression.
    #[serde(default)]
    regex: Option<bool>,
    /// Result cap; defaults to 200, capped at 1000.
    #[serde(default)]
    max_hits: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct SearchHitOut {
    notebook_id: String,
    notebook_name: String,
    path: String,
    line: usize,
    column: usize,
    text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
struct SearchOut {
    hits: Vec<SearchHitOut>,
    files_scanned: u64,
    truncated: bool,
}

#[tool_router(router = read_router, vis = "pub")]
impl MdworkServer {
    #[tool(
        description = "Server version, workspace root, and the calling token's scopes",
        annotations(read_only_hint = true)
    )]
    fn server_status(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<ServerStatusOut>, ErrorData> {
        self.exec(&ctx, "server_status", |token, _entry| {
            let notebook_count = self
                .workspace()
                .scan()
                .notebooks
                .iter()
                .filter(|notebook| self.notebook_readable(token, &notebook.marker.id))
                .count();
            Ok(Json(ServerStatusOut {
                name: mdwork_core::APP_NAME.to_string(),
                version: mdwork_core::version().to_string(),
                workspace_root: self.ctx.workspace_root.display().to_string(),
                notebook_count,
                token_name: token.name.clone(),
                scopes: token
                    .scopes
                    .iter()
                    .map(|scope| scope.as_str().to_string())
                    .collect(),
            }))
        })
    }

    #[tool(
        description = "List notebooks this token can access",
        annotations(read_only_hint = true)
    )]
    fn list_notebooks(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<ListNotebooksOut>, ErrorData> {
        self.exec(&ctx, "list_notebooks", |token, _entry| {
            require_scope(token, Scope::Read)?;
            let notebooks = self
                .workspace()
                .scan()
                .notebooks
                .into_iter()
                .filter(|notebook| self.notebook_readable(token, &notebook.marker.id))
                .map(|notebook| {
                    let id = notebook.marker.id;
                    // 只统计可见笔记：隐藏笔记的条数与名字一样属于越权信息。
                    let note_count = notebook
                        .list_entries()
                        .map(|entries| {
                            entries
                                .iter()
                                .filter(|entry| self.note_readable(token, &id, &entry.rel_path))
                                .count()
                        })
                        .unwrap_or(0);
                    NotebookSummary {
                        id: id.to_string(),
                        name: notebook.display_name().to_string(),
                        path: notebook.root.display().to_string(),
                        note_count,
                    }
                })
                .collect();
            Ok(Json(ListNotebooksOut { notebooks }))
        })
    }

    #[tool(
        description = "Notebook metadata and note counts",
        annotations(read_only_hint = true)
    )]
    fn notebook_info(
        &self,
        Parameters(args): Parameters<NotebookArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<NotebookInfoOut>, ErrorData> {
        self.exec(&ctx, "notebook_info", |token, entry| {
            require_scope(token, Scope::Read)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let tree = notebook.list_tree().map_err(core_error)?;
            let id = notebook.marker.id;
            entry.notebook = Some(id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            let visible_notes: Vec<_> = tree
                .iter()
                .filter(|item| {
                    item.kind == EntryKind::Note
                        && self
                            .perms
                            .allows(&NodeKey::entry(id, &item.rel_path), PermLevel::Read)
                })
                .collect();
            let note_count = visible_notes.len();
            let folder_count = tree
                .iter()
                .filter(|item| {
                    item.kind == EntryKind::Folder
                        && visible_notes
                            .iter()
                            .any(|note| note.rel_path.starts_with(&format!("{}/", item.rel_path)))
                })
                .count();
            let note_bytes = visible_notes.iter().map(|item| item.size).sum();
            Ok(Json(NotebookInfoOut {
                id: notebook.marker.id.to_string(),
                name: notebook.display_name().to_string(),
                path: notebook.root.display().to_string(),
                created: notebook.marker.created,
                note_count,
                folder_count,
                note_bytes,
            }))
        })
    }

    #[tool(
        description = "List notes and folders in a notebook, filtered by the token's path ACL",
        annotations(read_only_hint = true)
    )]
    fn list_notes(
        &self,
        Parameters(args): Parameters<ListNotesArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<ListNotesOut>, ErrorData> {
        self.exec(&ctx, "list_notes", |token, entry| {
            require_scope(token, Scope::Read)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let folder = normalize_folder(args.folder.as_deref())?;
            let tree = notebook.list_tree().map_err(core_error)?;
            let id = notebook.marker.id;
            entry.notebook = Some(id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = folder.clone();

            let allowed_notes: Vec<&mdwork_core::Entry> = tree
                .iter()
                .filter(|item| {
                    item.kind == EntryKind::Note && self.note_readable(token, &id, &item.rel_path)
                })
                .collect();
            let entries: Vec<NoteEntryOut> = tree
                .iter()
                .filter(|item| match &folder {
                    Some(prefix) => item.rel_path.starts_with(prefix),
                    None => true,
                })
                .filter(|item| match item.kind {
                    EntryKind::Note => self.note_readable(token, &id, &item.rel_path),
                    EntryKind::Folder => allowed_notes
                        .iter()
                        .any(|note| note.rel_path.starts_with(&format!("{}/", item.rel_path))),
                })
                .map(|item| NoteEntryOut {
                    path: item.rel_path.clone(),
                    name: item.name.clone(),
                    kind: match item.kind {
                        EntryKind::Note => "note",
                        EntryKind::Folder => "folder",
                    }
                    .to_string(),
                    size: item.size,
                    modified: item.modified,
                })
                .collect();
            let total = entries.len();
            Ok(Json(ListNotesOut {
                notebook_id: id.to_string(),
                notebook_name: notebook.display_name().to_string(),
                entries,
                total,
            }))
        })
    }

    #[tool(
        description = "Read a note (optionally a line range) plus its encoding, EOL and sha256",
        annotations(read_only_hint = true)
    )]
    fn read_note(
        &self,
        Parameters(args): Parameters<ReadNoteArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<NoteContentOut>, ErrorData> {
        self.exec(&ctx, "read_note", |token, entry| {
            require_scope(token, Scope::Read)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let rel = self.note_rel(&notebook, token, &args.path)?;
            let path = notebook.note_path(&rel).map_err(core_error)?;
            let bytes = std::fs::read(&path).map_err(|err| crate::server::io_error(&rel, err))?;
            let decoded = doc::decode_with_hint(&bytes, notebook.encoding_hint(&rel));
            entry.notebook = Some(notebook.marker.id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = Some(rel.clone());
            entry.bytes = Some(bytes.len() as u64);

            let lines: Vec<&str> = decoded.text.lines().collect();
            let total_lines = lines.len() as u64;
            let offset = args.offset.unwrap_or(1).max(1);
            let limit = args
                .limit
                .unwrap_or(DEFAULT_READ_LIMIT)
                .clamp(1, MAX_READ_LIMIT);
            let start = (offset - 1).min(total_lines);
            let end = (start + limit).min(total_lines);
            let text = lines[start as usize..end as usize].join("\n");

            Ok(Json(NoteContentOut {
                notebook_id: notebook.marker.id.to_string(),
                notebook_name: notebook.display_name().to_string(),
                path: rel,
                start_line: start + 1,
                end_line: end,
                total_lines,
                text,
                truncated: end < total_lines,
                encoding: decoded.encoding.label().to_string(),
                eol: decoded.eol.label().to_string(),
                bom: decoded.bom,
                sha256: doc::sha256_hex(&bytes),
                size: bytes.len() as u64,
            }))
        })
    }

    #[tool(
        description = "Heading outline of a note (level, title, one-based line)",
        annotations(read_only_hint = true)
    )]
    fn get_outline(
        &self,
        Parameters(args): Parameters<GetOutlineArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<OutlineOut>, ErrorData> {
        self.exec(&ctx, "get_outline", |token, entry| {
            require_scope(token, Scope::Read)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let rel = self.note_rel(&notebook, token, &args.path)?;
            let path = notebook.note_path(&rel).map_err(core_error)?;
            let bytes = std::fs::read(&path).map_err(|err| crate::server::io_error(&rel, err))?;
            entry.notebook = Some(notebook.marker.id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            entry.path = Some(rel.clone());
            entry.bytes = Some(bytes.len() as u64);

            let text = doc::decode_with_hint(&bytes, notebook.encoding_hint(&rel)).text;
            let headings = mdwork_render::preview::PreviewIr::from_markdown(&text)
                .outline()
                .into_iter()
                .map(|item| HeadingOut {
                    level: item.level,
                    title: item.title,
                    line: item.line,
                })
                .collect();
            Ok(Json(OutlineOut {
                notebook_id: notebook.marker.id.to_string(),
                path: rel,
                headings,
            }))
        })
    }

    #[tool(
        description = "List note and heading bookmarks of a notebook",
        annotations(read_only_hint = true)
    )]
    fn list_bookmarks(
        &self,
        Parameters(args): Parameters<NotebookArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<ListBookmarksOut>, ErrorData> {
        self.exec(&ctx, "list_bookmarks", |token, entry| {
            require_scope(token, Scope::Read)?;
            let notebook = self.notebook(token, &args.notebook_id)?;
            let id = notebook.marker.id;
            entry.notebook = Some(id);
            entry.notebook_name = Some(notebook.display_name().to_string());
            let marks = Bookmarks::load(&notebook).map_err(core_error)?;
            let bookmarks = marks
                .entries
                .into_iter()
                .filter(|mark| self.note_readable(token, &id, &mark.rel_path))
                .map(|mark| BookmarkOut {
                    path: mark.rel_path,
                    title: mark.title,
                    heading: mark.heading,
                    line: mark.line,
                    created: mark.created,
                })
                .collect();
            Ok(Json(ListBookmarksOut {
                notebook_id: id.to_string(),
                bookmarks,
            }))
        })
    }

    #[tool(
        description = "Search note content across this token's notebooks (plain text or regex)",
        annotations(read_only_hint = true)
    )]
    fn search_notes(
        &self,
        Parameters(args): Parameters<SearchArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<SearchOut>, ErrorData> {
        self.exec(&ctx, "search_notes", |token, entry| {
            require_scope(token, Scope::Search)?;
            let mut query = SearchQuery::new(args.query.trim());
            if query.is_blank() {
                return Err(invalid_params("query must not be blank"));
            }
            query.case_sensitive = args.case_sensitive.unwrap_or(false);
            query.regex = args.regex.unwrap_or(false);
            let max_hits = args
                .max_hits
                .unwrap_or(DEFAULT_SEARCH_HITS)
                .clamp(1, MAX_SEARCH_HITS) as usize;

            let notebooks = match &args.notebook_id {
                Some(id) => vec![self.notebook(token, id)?],
                None => self
                    .workspace()
                    .scan()
                    .notebooks
                    .into_iter()
                    .filter(|notebook| self.notebook_readable(token, &notebook.marker.id))
                    .collect(),
            };

            let cancel = AtomicBool::new(false);
            let mut hits = Vec::new();
            let mut files = 0u64;
            let mut truncated = false;
            for notebook in notebooks {
                if hits.len() >= max_hits {
                    truncated = true;
                    break;
                }
                let id = notebook.marker.id;
                let outcome = search_notebook_filtered(
                    &notebook,
                    &query,
                    &cancel,
                    max_hits - hits.len(),
                    |rel_path| self.note_readable(token, &id, rel_path),
                )
                .map_err(core_error)?;
                files += outcome.files as u64;
                truncated |= outcome.truncated;
                hits.extend(outcome.hits.into_iter().map(|hit| SearchHitOut {
                    notebook_id: hit.notebook_id.to_string(),
                    notebook_name: hit.notebook_name,
                    path: hit.rel_path,
                    line: hit.line,
                    column: hit.column,
                    text: hit.text,
                }));
            }
            hits.sort_by(|a, b| {
                (&a.notebook_id, &a.path, a.line).cmp(&(&b.notebook_id, &b.path, b.line))
            });
            entry.query = Some(query.text.clone());

            Ok(Json(SearchOut {
                hits,
                files_scanned: files,
                truncated,
            }))
        })
    }
}

fn normalize_folder(raw: Option<&str>) -> Result<Option<String>, ErrorData> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim().replace('\\', "/");
    let trimmed = trimmed.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.split('/').any(|segment| {
        segment.is_empty() || segment == "." || segment == ".." || segment.starts_with('.')
    }) {
        return Err(invalid_path(format!("invalid folder: {raw}")));
    }
    Ok(Some(format!("{trimmed}/")))
}
