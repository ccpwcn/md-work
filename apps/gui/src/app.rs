use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Editor, Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdwork_core::Workspace;
use mdwork_core::archive::{self, Conflict, ConflictPolicy, ImportMode, ImportOptions, ImportPlan};
use mdwork_core::bookmarks::Bookmarks;
use mdwork_core::config::{
    AppConfig, CursorPos, NotebookSession, PanelArrangement, PanelDock, Session, ThemePreference,
};
use mdwork_core::doc::{Eol, TextEncoding};
use mdwork_core::image::{self, ImageHost, ImageKind};
use mdwork_core::notebook::{Entry, EntryKind, Notebook, TrashItem};
use mdwork_core::permissions::{self as perms, Scope, TokenRecord};
use mdwork_core::search::{self, SearchHit, SearchQuery};
use mdwork_core::text::text_stats;
use mdwork_export::{
    ExportEvent, ExportFormat, ExportJob, ExportOptions, ExportRequest, NotebookPrefs, PaperSize,
};
use mdwork_mcp::{AuditEntry, AuditLog, TokenStore};
use mdwork_render::preview::{OutlineItem, PreviewIr};
use rust_i18n::t;
use uuid::Uuid;

use crate::doc::{DiskConflict, Doc, DocOrigin, read_external_doc, read_notebook_doc};
use crate::{preview, theme};

const PANEL_MIN_WIDTH: f32 = 240.0;
const PANEL_MAX_WIDTH: f32 = 560.0;

const WELCOME_NOTE: &str = r#"# 欢迎使用墨笺

墨笺是一个轻量的 Markdown 笔记本：笔记就是磁盘上的 `.md` 文件，可以直接用资源管理器管理。

## 已经具备

- 工作区位于「我的文档」下的 `md-work` 目录
- 每个子目录都是一个笔记本，复制进工作区即可自动识别
- 打开多个笔记本，每个笔记本记住自己的标签页
- UTF-8 / GB18030 等编码往返保存
- Markdown 语法高亮与软换行
- 自动保存（停止输入 800 毫秒后）

## 接下来

选中左侧的笔记开始编辑。按 `Ctrl+S` 立即保存。
"#;

struct NotebookEntry {
    notebook: Notebook,
    tree: Vec<Entry>,
    expanded: HashSet<String>,
    rows: Vec<TreeRow>,
}

#[derive(Clone)]
struct TreeRow {
    rel_path: String,
    name: String,
    kind: EntryKind,
    depth: usize,
    expanded: bool,
}

struct PreviewCache {
    doc: EntityId,
    revision: u64,
    ir: PreviewIr,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    Notes,
    Outline,
    Search,
    Bookmarks,
    Trash,
}

#[derive(Clone)]
struct Renaming {
    notebook_id: Uuid,
    rel_path: String,
    input: Entity<InputState>,
}

#[derive(Clone, PartialEq, Eq)]
enum SearchStatus {
    Idle,
    Running,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchOption {
    Case,
    Regex,
    Filename,
}

struct ExportDialog {
    notebook_id: Option<Uuid>,
    notebook_root: PathBuf,
    notebook_name: String,
    note_rel: String,
    title: String,
    markdown: String,
    format: ExportFormat,
    options: ExportOptions,
    header_input: Entity<InputState>,
    footer_input: Entity<InputState>,
    output_input: Entity<InputState>,
    job: Option<ExportJob>,
    busy: bool,
    stage: Option<String>,
    progress: Option<(usize, usize)>,
    warnings: Vec<String>,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ZipDialogMode {
    Export,
    Import,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImageHostMode {
    Local,
    PicGo,
    Http,
}

struct ImageSettingsDialog {
    mode: ImageHostMode,
    picgo_endpoint: Entity<InputState>,
    http_endpoint: Entity<InputState>,
    http_field: Entity<InputState>,
    http_url_path: Entity<InputState>,
    http_headers: Entity<TextareaState>,
    error: Option<String>,
}

struct ZipDialog {
    mode: ZipDialogMode,
    notebook_id: Option<Uuid>,
    notebook_name: String,
    workspace_root: PathBuf,
    path_input: Entity<InputState>,
    rename_input: Entity<InputState>,
    import_mode: ImportMode,
    policy: ConflictPolicy,
    choices: HashMap<String, ConflictPolicy>,
    plan: Option<ImportPlan>,
    plan_generation: u64,
    plan_loading: bool,
    plan_error: Option<String>,
    confirm_overwrite: bool,
    busy: bool,
    error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Appearance,
    Panels,
    Images,
    Mcp,
    Windows,
}

struct SettingsDialog {
    tab: SettingsTab,
    ui_font: Entity<InputState>,
    note_font: Entity<InputState>,
    ui_size: Entity<InputState>,
    note_size: Entity<InputState>,
}

enum PromptKind {
    NewFolder { notebook_id: Uuid },
    MoveEntry { notebook_id: Uuid, from: String },
    OpenNotebook,
}

struct PromptDialog {
    kind: PromptKind,
    title: String,
    hint: String,
    input: Entity<InputState>,
    error: Option<String>,
}

struct RecoveryDialog {
    entries: Vec<mdwork_core::recovery::Snapshot>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PaletteCommand {
    NewNote,
    NewFolder,
    NewNotebook,
    OpenNotebook,
    Save,
    TogglePreview,
    CloseTab,
    Settings,
    Search,
    Outline,
    Bookmarks,
    Trash,
    NotesPanel,
    ExportPdf,
    ExportPng,
    ExportNotebookZip,
    ImportNotebookZip,
    MoveEntry,
    ThemeSystem,
    ThemeLight,
    ThemeDark,
    ToggleCodeWrap,
    ToggleAutosave,
    SaveAsUtf8,
    SaveAsGb18030,
    SaveAsWindows1252,
    SaveAsUtf16,
    SaveEolLf,
    SaveEolCrlf,
    OpenWorkspaceFolder,
    OpenLogFolder,
    RegisterFileAssoc,
    Rescan,
}

struct PaletteItem {
    label: String,
    hint: String,
    command: PaletteCommand,
}

struct Palette {
    input: Entity<InputState>,
    items: Vec<PaletteItem>,
    filtered: Vec<usize>,
    selected: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum McpTab {
    Server,
    Tokens,
    Audit,
}

struct McpSettingsDialog {
    tab: McpTab,
    enabled: bool,
    port_input: Entity<InputState>,
    tokens: Vec<TokenRecord>,
    selected: Option<Uuid>,
    creating: bool,
    name_input: Entity<InputState>,
    allow_input: Entity<InputState>,
    deny_input: Entity<InputState>,
    scopes: Vec<Scope>,
    notebooks: HashSet<Uuid>,
    /// Plaintext token from the create/rotate call; shown until the dialog closes.
    secret: Option<String>,
    pending_rotate: bool,
    pending_revoke: bool,
    pending_delete: bool,
    audit: Vec<AuditEntry>,
    audit_error: Option<String>,
    message: Option<String>,
    error: Option<String>,
}

pub struct AppShell {
    config: AppConfig,
    session: Session,
    workspace: Option<Workspace>,
    notebooks: Vec<NotebookEntry>,
    active_notebook: Option<Uuid>,
    docs: Vec<Entity<Doc>>,
    active_doc: Option<usize>,
    notice: Option<String>,
    pending: Arc<Mutex<Vec<String>>>,
    changed: Option<Arc<Mutex<Receiver<Vec<PathBuf>>>>>,
    window_handle: gpui_kit::AnyWindowHandle,
    tree_scroll: UniformListScrollHandle,
    flash_rel: Option<String>,
    preview_mode: bool,
    preview_cache: Option<PreviewCache>,
    math: HashMap<u64, Option<PathBuf>>,
    math_pending: HashSet<u64>,
    copied: bool,
    panel: SidePanel,
    /// (mouse x at drag start, sidebar width at drag start)
    panel_resize: Option<(f32, f32)>,
    outline: Vec<OutlineItem>,
    outline_cache: Option<(EntityId, u64)>,
    bookmarks: Bookmarks,
    bookmarks_notebook: Option<Uuid>,
    search_input: Entity<InputState>,
    search_status: SearchStatus,
    search_results: Vec<SearchHit>,
    search_all: bool,
    search_case: bool,
    search_regex: bool,
    search_filename: bool,
    search_cancel: Arc<AtomicBool>,
    search_generation: u64,
    renaming: Option<Renaming>,
    trash: Vec<TrashItem>,
    trash_notebook: Option<Uuid>,
    export: Option<ExportDialog>,
    zip: Option<ZipDialog>,
    image_settings: Option<ImageSettingsDialog>,
    mcp_settings: Option<McpSettingsDialog>,
    settings: Option<SettingsDialog>,
    prompt: Option<PromptDialog>,
    palette: Option<Palette>,
    recovery: Option<RecoveryDialog>,
    scan_issues: Vec<String>,
    dirty: crate::dirty::DirtyRegistry,
    mcp: crate::mcp_server::McpHttp,
    mcp_live: bool,
    selection_sig: Option<(usize, usize)>,
    _watcher: Option<mdwork_core::watch::WorkspaceWatcher>,
    _appearance_subscription: Option<Subscription>,
}

impl AppShell {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        pending: Arc<Mutex<Vec<String>>>,
        initial_path: Option<String>,
    ) -> Self {
        let mut config = AppConfig::load_or_default();
        config.panel_width = if config.panel_width.is_finite() {
            config.panel_width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH)
        } else {
            300.0
        };
        rust_i18n::set_locale(&config.language);
        theme::apply(&config, window, cx);

        let root = config
            .resolve_workspace_root()
            .unwrap_or_else(|_| PathBuf::from("md-work"));
        let existed = root.exists();
        let first_run = !existed;
        let workspace = Workspace::open(&root).ok();
        let mut notice = None;

        if let Some(ws) = &workspace {
            if !existed {
                if let Err(err) = seed_workspace(ws) {
                    notice = Some(format!("初始化工作区失败：{err}"));
                }
            }
        } else {
            notice = Some(format!("无法打开工作区：{}", root.display()));
        }

        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("search.placeholder").to_string())
                .clean_on_escape()
        });

        let mcp_port = config.mcp_port;
        let mcp_enabled = config.mcp_http;

        let session = match Session::load_from(&mdwork_core::paths::session_path()) {
            Ok(session) => session,
            Err(err) => {
                tracing::warn!(error = %err, "读取会话失败，使用默认会话");
                Session::default()
            }
        };

        let mut this = Self {
            config,
            session,
            workspace,
            notebooks: Vec::new(),
            active_notebook: None,
            docs: Vec::new(),
            active_doc: None,
            notice,
            pending,
            changed: None,
            window_handle: window.window_handle(),
            tree_scroll: UniformListScrollHandle::new(),
            flash_rel: None,
            preview_mode: false,
            preview_cache: None,
            math: HashMap::new(),
            math_pending: HashSet::new(),
            copied: false,
            panel: SidePanel::Notes,
            panel_resize: None,
            outline: Vec::new(),
            outline_cache: None,
            bookmarks: Bookmarks::default(),
            bookmarks_notebook: None,
            search_input,
            search_status: SearchStatus::Idle,
            search_results: Vec::new(),
            search_all: true,
            search_case: false,
            search_regex: false,
            search_filename: false,
            search_cancel: Arc::new(AtomicBool::new(false)),
            search_generation: 0,
            renaming: None,
            trash: Vec::new(),
            trash_notebook: None,
            export: None,
            zip: None,
            image_settings: None,
            mcp_settings: None,
            settings: None,
            prompt: None,
            palette: None,
            recovery: None,
            scan_issues: Vec::new(),
            dirty: crate::dirty::DirtyRegistry::new(),
            mcp: crate::mcp_server::McpHttp::new(mcp_port),
            mcp_live: false,
            selection_sig: None,
            _watcher: None,
            _appearance_subscription: None,
        };

        this.reload();
        this.start_watcher();
        let snapshots = mdwork_core::recovery::list(&mdwork_core::paths::recovery_dir());
        if !snapshots.is_empty() {
            this.recovery = Some(RecoveryDialog { entries: snapshots });
        }
        cx.subscribe(&this.search_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_search(cx);
            }
        })
        .detach();
        this.restore_session(window, cx);

        if let Some(path) = initial_path {
            this.open_path(&path, window, cx);
        } else if first_run {
            let welcome = this.notebooks.first().and_then(|entry| {
                let id = entry.notebook.marker.id;
                entry
                    .tree
                    .iter()
                    .find(|item| item.kind == EntryKind::Note)
                    .map(|item| (id, item.rel_path.to_string()))
            });
            if let Some((id, rel_path)) = welcome {
                this.open_note_by_id(id, &rel_path, window, cx);
            }
        }

        this.reveal_active(cx);
        this.spawn_poller(cx);

        if mcp_enabled {
            this.mcp_live = this.mcp.start(mcp_port);
        }

        let weak = cx.entity().downgrade();
        this._appearance_subscription =
            Some(window.observe_window_appearance(move |window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if this.config.theme == ThemePreference::System {
                        theme::apply(&this.config, window, cx);
                    }
                });
            }));

        let weak = cx.entity().downgrade();
        cx.on_window_closed(move |app, _| {
            if let Some(this) = weak.upgrade() {
                this.update(app, |this, cx| this.persist_session(cx));
            }
        })
        .detach();

        this
    }

    fn start_watcher(&mut self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        match mdwork_core::watch::WorkspaceWatcher::start(workspace.root()) {
            Ok((watcher, rx)) => {
                self._watcher = Some(watcher);
                self.changed = Some(Arc::new(Mutex::new(rx)));
            }
            Err(err) => tracing::warn!(error = %err, "文件监视不可用"),
        }
    }

    fn spawn_poller(&self, cx: &mut Context<Self>) {
        let pending = self.pending.clone();
        let changed = self.changed.clone();
        cx.spawn(async move |this, cx| {
            let mut tick: u32 = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                tick = tick.wrapping_add(1);
                let _ = this.update(cx, |this, cx| {
                    let sig = this.active_doc_entity().map(|doc| {
                        let range = doc.read(cx).editor.read(cx).selected_range();
                        (range.start, range.end)
                    });
                    if sig != this.selection_sig {
                        this.selection_sig = sig;
                        cx.notify();
                    }
                });
                if tick % 8 == 0 {
                    let _ = this.update(cx, |this, cx| {
                        let live = this.mcp.running();
                        if live != this.mcp_live {
                            this.mcp_live = live;
                            cx.notify();
                        }
                    });
                }
                let paths: Vec<String> = {
                    let mut guard = pending.lock().expect("pending paths lock");
                    std::mem::take(&mut *guard)
                };
                let disk: Vec<PathBuf> = match &changed {
                    Some(rx) => {
                        let rx = rx.lock().expect("watcher receiver lock");
                        let mut paths = Vec::new();
                        while let Ok(batch) = rx.try_recv() {
                            paths.extend(batch);
                        }
                        paths
                    }
                    None => Vec::new(),
                };
                if paths.is_empty() && disk.is_empty() {
                    continue;
                }
                let Ok(handle) = this.read_with(cx, |this, _| this.window_handle) else {
                    continue;
                };
                let weak = this.clone();
                let _ = handle.update(cx, |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        for path in &paths {
                            this.open_path(path, window, cx);
                        }
                        if !disk.is_empty() {
                            this.handle_external_changes(disk, window, cx);
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn reload(&mut self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.refresh() {
            Ok(scan) => {
                self.scan_issues = scan
                    .issues
                    .iter()
                    .map(|issue| {
                        tracing::warn!(
                            path = %issue.path.display(),
                            kind = ?issue.kind,
                            message = %issue.message,
                            "扫描工作区发现问题"
                        );
                        format!("{}：{}", issue.path.display(), issue.message)
                    })
                    .collect();
                let previous: Vec<(Uuid, HashSet<String>)> = self
                    .notebooks
                    .iter()
                    .map(|entry| (entry.notebook.marker.id, entry.expanded.clone()))
                    .collect();
                self.notebooks = scan
                    .notebooks
                    .into_iter()
                    .map(|notebook| {
                        let mut entry = NotebookEntry {
                            expanded: previous
                                .iter()
                                .find(|(id, _)| *id == notebook.marker.id)
                                .map(|(_, expanded)| expanded.clone())
                                .unwrap_or_default(),
                            notebook,
                            tree: Vec::new(),
                            rows: Vec::new(),
                        };
                        refresh_tree(&mut entry);
                        entry
                    })
                    .collect();
                if self.active_notebook.is_none() {
                    self.active_notebook = self.notebooks.first().map(|e| e.notebook.marker.id);
                }
            }
            Err(err) => self.notice = Some(format!("扫描工作区失败：{err}")),
        }
    }

    fn reload_entries(&mut self) {
        for entry in &mut self.notebooks {
            refresh_tree(entry);
        }
    }

    fn handle_external_changes(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| !is_atomic_temp(path))
            .collect();
        if paths.is_empty() {
            return;
        }
        let open_paths: Vec<PathBuf> = self
            .docs
            .iter()
            .map(|doc| doc.read(cx).path.clone())
            .collect();
        let needs_scan = paths
            .iter()
            .any(|path| !open_paths.iter().any(|open| paths_equal(open, path)));

        for index in 0..self.docs.len() {
            let (path, base_hash, dirty) = {
                let doc = self.docs[index].read(cx);
                (doc.path.clone(), doc.base_hash.clone(), doc.dirty)
            };
            if !paths.iter().any(|changed| paths_equal(changed, &path)) {
                continue;
            }
            let doc = self.docs[index].clone();
            if !path.exists() {
                doc.update(cx, |doc, cx| doc.mark_conflict(DiskConflict::Removed, cx));
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let hash = mdwork_core::doc::sha256_hex(&bytes);
            if hash == base_hash {
                continue;
            }
            let decoded = mdwork_core::doc::decode(&bytes);
            doc.update(cx, |doc, cx| {
                if dirty {
                    doc.mark_conflict(DiskConflict::Modified, cx);
                } else {
                    doc.apply_disk(decoded, hash, window, cx);
                }
            });
        }

        if needs_scan {
            let root = self
                .workspace
                .as_ref()
                .map(|workspace| workspace.root().to_path_buf());
            let notebook_touched = root.is_some_and(|root| {
                paths.iter().any(|path| {
                    path.parent() == Some(root.as_path())
                        || path.file_name().is_some_and(|name| name == "notebook.json")
                })
            });
            if notebook_touched {
                self.reload();
            } else {
                self.reload_entries();
            }
        }
        cx.notify();
    }

    fn reload_doc(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(index).cloned() else {
            return;
        };
        let path = doc.read(cx).path.clone();
        match std::fs::read(&path) {
            Ok(bytes) => {
                let hash = mdwork_core::doc::sha256_hex(&bytes);
                let decoded = mdwork_core::doc::decode(&bytes);
                doc.update(cx, |doc, cx| doc.apply_disk(decoded, hash, window, cx));
            }
            Err(err) => {
                self.notice = Some(format!("无法读取 {}：{err}", path.display()));
                cx.notify();
            }
        }
    }

    fn keep_doc(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(doc) = self.docs.get(index) {
            doc.update(cx, |doc, cx| doc.keep_mine(cx));
        }
        cx.notify();
    }

    fn recreate_doc(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(doc) = self.docs.get(index) {
            doc.update(cx, |doc, cx| doc.save_now(cx));
        }
        cx.notify();
    }

    fn toggle_folder(&mut self, notebook_id: Uuid, rel_path: &str, cx: &mut Context<Self>) {
        let Some(entry) = self
            .notebooks
            .iter_mut()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            return;
        };
        if !entry.expanded.remove(rel_path) {
            entry.expanded.insert(rel_path.to_string());
        }
        let expanded = entry.expanded.clone();
        entry.rows = build_rows(&entry.tree, &expanded);
        cx.notify();
    }

    fn reveal_active(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.active_doc.and_then(|index| self.docs.get(index)) else {
            return;
        };
        let (notebook_id, rel_path) = {
            let doc = active.read(cx);
            let Some(notebook_id) = doc.notebook_id else {
                return;
            };
            (notebook_id, doc.rel_path.clone())
        };

        let Some(entry) = self
            .notebooks
            .iter_mut()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            return;
        };
        let mut changed = false;
        let mut ancestor = rel_path.as_str();
        while let Some(index) = ancestor.rfind('/') {
            ancestor = &ancestor[..index];
            changed |= entry.expanded.insert(ancestor.to_string());
        }
        if changed {
            let expanded = entry.expanded.clone();
            entry.rows = build_rows(&entry.tree, &expanded);
        }
        let Some(row) = entry.rows.iter().position(|row| row.rel_path == rel_path) else {
            return;
        };
        self.tree_scroll.scroll_to_item(row, ScrollStrategy::Center);
        self.flash_rel = Some(rel_path);
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.flash_rel = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Cursors must come from the session on disk: opening tabs rewrites it.
        let saved_cursors = self.session.clone();
        let session = self.session.clone();
        for saved in &session.notebooks {
            let Some(notebook) = self
                .notebooks
                .iter()
                .find(|entry| entry.notebook.marker.id == saved.id)
                .map(|entry| entry.notebook.clone())
            else {
                continue;
            };
            for rel_path in &saved.open_notes {
                let exists = notebook
                    .note_path(rel_path)
                    .map(|path| path.is_file())
                    .unwrap_or(false);
                if exists {
                    self.open_note(&notebook, rel_path, window, cx);
                }
            }
        }

        if let Some(id) = session.active_notebook {
            if self.notebooks.iter().any(|e| e.notebook.marker.id == id) {
                self.active_notebook = Some(id);
            }
        }

        if let Some(id) = self.active_notebook {
            if let Some(saved) = session.notebook(&id) {
                if let Some(active) = &saved.active_note {
                    self.activate_note(&id, active, cx);
                }
            }
        }

        for path in &session.external_notes {
            let path = PathBuf::from(path);
            if path.is_file() {
                self.open_external(&path, window, cx);
            }
        }

        if let Some(path) = &session.active_external {
            let path = PathBuf::from(path);
            if let Some(index) = self.docs.iter().position(|doc| {
                let doc = doc.read(cx);
                doc.notebook_id.is_none() && doc.path == path
            }) {
                self.active_doc = Some(index);
            }
        } else if let Some(id) = session.active_notebook {
            // Opening external notes above makes the last one active; put the
            // focus back on the notebook note that was active at shutdown.
            if let Some(saved) = session.notebook(&id) {
                if let Some(active) = &saved.active_note {
                    self.activate_note(&id, active, cx);
                }
            }
        }

        self.restore_cursor(&saved_cursors, window, cx);
    }

    fn restore_cursor(&mut self, saved: &Session, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_doc else {
            return;
        };
        let Some(doc) = self.docs.get(index).cloned() else {
            return;
        };
        let cursor = {
            let doc = doc.read(cx);
            match doc.notebook_id {
                Some(id) => saved
                    .notebook(&id)
                    .and_then(|entry| entry.cursors.get(&doc.rel_path).copied()),
                None => saved
                    .external_cursors
                    .get(&doc.path.display().to_string())
                    .copied(),
            }
        };
        if let Some(pos) = cursor {
            doc.update(cx, |doc, cx| {
                doc.set_cursor(pos.line, pos.column, window, cx);
            });
        }
    }

    fn open_path(&mut self, raw: &str, window: &mut Window, cx: &mut Context<Self>) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }
        let path = PathBuf::from(trimmed);
        if !path.is_file() {
            self.notice = Some(format!("文件不存在：{}", path.display()));
            cx.notify();
            return;
        }
        let is_markdown = path
            .extension()
            .map(|ext| {
                let ext = ext.to_string_lossy().to_lowercase();
                ext == "md" || ext == "markdown"
            })
            .unwrap_or(false);
        if !is_markdown {
            self.notice = Some(format!("不是 Markdown 文件：{}", path.display()));
            cx.notify();
            return;
        }

        if let Some(workspace) = &self.workspace {
            if let Some(notebook) = workspace.notebook_for_path(&path) {
                if let Ok(rel) = path.strip_prefix(&notebook.root) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    self.active_notebook = Some(notebook.marker.id);
                    self.open_note(&notebook, &rel, window, cx);
                    return;
                }
            }
        }
        self.open_external(&path, window, cx);
    }

    fn open_note(
        &mut self,
        notebook: &Notebook,
        rel_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.docs.iter().position(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id == Some(notebook.marker.id) && doc.rel_path == rel_path
        }) {
            self.active_doc = Some(index);
            self.persist_session(cx);
            self.reveal_active(cx);
            cx.notify();
            return;
        }

        match read_notebook_doc(notebook, rel_path) {
            Ok((path, decoded, hash)) => {
                let origin = DocOrigin::Notebook {
                    id: notebook.marker.id,
                    name: notebook.display_name().to_string(),
                };
                let autosave = self.config.autosave;
                let rel_path = rel_path.to_string();
                let registry = self.dirty.clone();
                let root = Some(notebook.root.clone());
                let doc = cx.new(|cx| {
                    Doc::new(
                        origin, root, rel_path, path, decoded, hash, autosave, registry, window, cx,
                    )
                });
                cx.observe(&doc, |_, _, cx| cx.notify()).detach();
                self.docs.push(doc);
                self.active_doc = Some(self.docs.len() - 1);
                self.notice = None;
                self.persist_session(cx);
                self.reveal_active(cx);
                cx.notify();
            }
            Err(err) => {
                self.notice = Some(format!("无法打开 {rel_path}：{err}"));
                cx.notify();
            }
        }
    }

    fn open_note_by_id(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        self.active_notebook = Some(notebook_id);
        self.open_note(&notebook, rel_path, window, cx);
    }

    fn open_external(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.docs.iter().position(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id.is_none() && doc.path == path
        }) {
            self.active_doc = Some(index);
            self.persist_session(cx);
            cx.notify();
            return;
        }

        match read_external_doc(path) {
            Ok((decoded, hash)) => {
                let file_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let autosave = self.config.autosave;
                let path_buf = path.to_path_buf();
                let registry = self.dirty.clone();
                let doc = cx.new(|cx| {
                    Doc::new(
                        DocOrigin::External,
                        None,
                        file_name,
                        path_buf,
                        decoded,
                        hash,
                        autosave,
                        registry,
                        window,
                        cx,
                    )
                });
                cx.observe(&doc, |_, _, cx| cx.notify()).detach();
                self.docs.push(doc);
                self.active_doc = Some(self.docs.len() - 1);
                self.notice = None;
                self.persist_session(cx);
                cx.notify();
            }
            Err(err) => {
                self.notice = Some(format!("无法打开 {}：{err}", path.display()));
                cx.notify();
            }
        }
    }

    /// Copies an externally opened buffer into the active notebook as a new
    /// note. The original file on disk is left untouched.
    fn save_external_into_notebook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.active_doc else {
            return;
        };
        let Some(doc) = self.docs.get(index).cloned() else {
            return;
        };
        let (text, encoding, bom, eol, file_name) = {
            let doc = doc.read(cx);
            if doc.notebook_id.is_some() {
                return;
            }
            (
                doc.text(cx).to_string(),
                doc.encoding,
                doc.bom,
                doc.eol,
                doc.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "note.md".to_string()),
            )
        };

        let notebook = self
            .active_notebook
            .and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
            })
            .or_else(|| self.notebooks.first())
            .map(|entry| entry.notebook.clone());
        let Some(notebook) = notebook else {
            self.notice = Some(t!("external.no_notebook").to_string());
            cx.notify();
            return;
        };

        let (stem, ext) = match file_name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
                (stem.to_string(), Some(ext.to_string()))
            }
            _ => (file_name.clone(), None),
        };
        let mut rel_path = file_name.clone();
        let mut counter = 2;
        while notebook
            .note_path(&rel_path)
            .map(|path| path.exists())
            .unwrap_or(true)
        {
            rel_path = match &ext {
                Some(ext) => format!("{stem} ({counter}).{ext}"),
                None => format!("{stem} ({counter})"),
            };
            counter += 1;
        }

        let Some(path) = notebook.note_path(&rel_path).ok() else {
            self.notice = Some(format!("存入笔记本失败：{rel_path}"));
            cx.notify();
            return;
        };
        let bytes = match mdwork_core::doc::encode_text(&text, encoding, bom, eol) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.notice = Some(format!("存入笔记本失败：{err}"));
                cx.notify();
                return;
            }
        };
        if let Err(err) = mdwork_core::atomic::write_atomic(&path, &bytes) {
            self.notice = Some(format!("存入笔记本失败：{err}"));
            cx.notify();
            return;
        }
        let hint = mdwork_core::doc::EncodingHint { encoding, bom, eol };
        if let Err(err) =
            mdwork_core::notebook::store_encoding_hint(&notebook.root, &rel_path, hint)
        {
            tracing::warn!(
                path = %rel_path,
                error = %err,
                "写入编码记录失败"
            );
        }
        tracing::info!(
            notebook = %notebook.display_name(),
            path = %rel_path,
            "外部文件已存入笔记本"
        );

        self.docs.remove(index);
        self.active_doc = match self.active_doc {
            Some(active) if active == index => None,
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
        self.active_notebook = Some(notebook.marker.id);
        self.reload_entries();
        self.open_note(&notebook, &rel_path, window, cx);
        self.notice = Some(t!("external.saved", path = rel_path.clone()).to_string());
        cx.notify();
    }

    fn activate_note(&mut self, notebook_id: &Uuid, rel_path: &str, cx: &mut Context<Self>) {
        if let Some(index) = self.docs.iter().position(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id == Some(*notebook_id) && doc.rel_path == rel_path
        }) {
            self.active_doc = Some(index);
            self.active_notebook = Some(*notebook_id);
            self.reveal_active(cx);
        }
    }

    fn activate_doc(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.docs.len() {
            return;
        }
        self.active_doc = Some(index);
        if let Some(id) = self.docs[index].read(cx).notebook_id {
            self.active_notebook = Some(id);
        }
        self.persist_session(cx);
        self.reveal_active(cx);
        cx.notify();
    }

    fn activate_notebook(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.active_notebook = Some(id);
        let preferred = self.docs.iter().position(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id == Some(id)
                && self
                    .session
                    .notebook(&id)
                    .and_then(|saved| saved.active_note.as_deref())
                    .map(|active| active == doc.rel_path)
                    .unwrap_or(false)
        });
        if let Some(index) = preferred {
            self.active_doc = Some(index);
        }
        self.persist_session(cx);
        self.reveal_active(cx);
        cx.notify();
    }

    fn close_doc(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.docs.len() {
            return;
        }
        let doc = self.docs[index].clone();
        let mut save_failed = None;
        doc.update(cx, |doc, cx| {
            if doc.dirty {
                if let Err(err) = doc.save(cx) {
                    save_failed = Some(err.to_string());
                }
            }
        });
        if let Some(err) = save_failed {
            self.notice = Some(format!("保存失败，未关闭：{err}"));
            cx.notify();
            return;
        }

        self.docs.remove(index);
        self.active_doc = match self.active_doc {
            Some(active) if active == index => {
                if self.docs.is_empty() {
                    None
                } else {
                    Some(index.min(self.docs.len() - 1))
                }
            }
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
        self.persist_session(cx);
        cx.notify();
    }

    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_notebook else {
            self.notice = Some(t!("editor.no_notebook").to_string());
            cx.notify();
            return;
        };
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };

        let mut name = "新笔记.md".to_string();
        let mut counter = 2;
        while notebook
            .note_path(&name)
            .map(|p| p.exists())
            .unwrap_or(true)
        {
            name = format!("新笔记 {counter}.md");
            counter += 1;
        }

        match notebook.create_note(&name, "") {
            Ok(rel_path) => {
                self.reload_entries();
                self.open_note(&notebook, &rel_path, window, cx);
            }
            Err(err) => {
                self.notice = Some(format!("新建笔记失败：{err}"));
                cx.notify();
            }
        }
    }

    fn new_notebook(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let mut name = "新笔记本".to_string();
        let mut counter = 2;
        while self
            .notebooks
            .iter()
            .any(|entry| entry.notebook.display_name() == name)
        {
            name = format!("新笔记本 {counter}");
            counter += 1;
        }
        match workspace.create_notebook(&name) {
            Ok(notebook) => {
                self.active_notebook = Some(notebook.marker.id);
                self.reload();
                self.persist_session(cx);
                cx.notify();
            }
            Err(err) => {
                self.notice = Some(format!("新建笔记本失败：{err}"));
                cx.notify();
            }
        }
    }

    fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(notebook_id) = self.active_notebook else {
            self.notice = Some(t!("editor.no_notebook").to_string());
            cx.notify();
            return;
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("新文件夹")
                .clean_on_escape()
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, move |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_prompt(cx);
            }
        })
        .detach();
        self.prompt = Some(PromptDialog {
            kind: PromptKind::NewFolder { notebook_id },
            title: t!("prompt.new_folder").to_string(),
            hint: t!("prompt.new_folder_hint").to_string(),
            input,
            error: None,
        });
        cx.notify();
    }

    fn begin_move(
        &mut self,
        notebook_id: Uuid,
        from: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(from.to_string())
                .clean_on_escape()
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, move |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_prompt(cx);
            }
        })
        .detach();
        self.prompt = Some(PromptDialog {
            kind: PromptKind::MoveEntry {
                notebook_id,
                from: from.to_string(),
            },
            title: t!("prompt.move").to_string(),
            hint: t!("prompt.move_hint").to_string(),
            input,
            error: None,
        });
        cx.notify();
    }

    fn open_external_notebook_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(String::new())
                .placeholder(t!("prompt.open_notebook_placeholder").to_string())
                .clean_on_escape()
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, move |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_prompt(cx);
            }
        })
        .detach();
        self.prompt = Some(PromptDialog {
            kind: PromptKind::OpenNotebook,
            title: t!("prompt.open_notebook").to_string(),
            hint: t!("prompt.open_notebook_hint").to_string(),
            input,
            error: None,
        });
        cx.notify();
    }

    fn close_prompt(&mut self, cx: &mut Context<Self>) {
        if self.prompt.take().is_some() {
            cx.notify();
        }
    }

    fn close_recovery(&mut self, cx: &mut Context<Self>) {
        if self.recovery.take().is_some() {
            cx.notify();
        }
    }

    fn restore_recovery(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.recovery.as_ref() else {
            return;
        };
        let Some(snapshot) = dialog.entries.get(index).cloned() else {
            return;
        };
        if !snapshot.note_path.exists() {
            self.notice = Some(t!("recovery.missing").to_string());
            cx.notify();
            return;
        }
        let snapshot_path = snapshot.note_path.to_string_lossy().into_owned();
        self.open_path(&snapshot_path, window, cx);
        let restored = self
            .docs
            .iter()
            .find(|doc| doc.read(cx).path == snapshot.note_path)
            .cloned();
        let mut disk_changed = false;
        if let Some(doc) = restored {
            disk_changed = snapshot
                .base_sha256
                .as_deref()
                .is_some_and(|base| base != doc.read(cx).base_hash.as_str());
            let text = snapshot.text.clone();
            doc.update(cx, |doc, cx| doc.replace_all(text, window, cx));
        }
        let _ = mdwork_core::recovery::remove(
            &mdwork_core::paths::recovery_dir(),
            &snapshot.note_path,
        );
        let mut empty = false;
        if let Some(dialog) = self.recovery.as_mut() {
            dialog.entries.retain(|entry| entry.note_path != snapshot.note_path);
            empty = dialog.entries.is_empty();
        }
        if empty {
            self.recovery = None;
        }
        self.notice = Some(if disk_changed {
            t!("recovery.disk_changed").to_string()
        } else {
            t!("recovery.restored").to_string()
        });
        cx.notify();
    }

    fn discard_recovery(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(dialog) = self.recovery.as_mut() else {
            return;
        };
        let Some(snapshot) = dialog.entries.get(index).cloned() else {
            return;
        };
        let _ = mdwork_core::recovery::remove(
            &mdwork_core::paths::recovery_dir(),
            &snapshot.note_path,
        );
        dialog.entries.retain(|entry| entry.note_path != snapshot.note_path);
        let empty = dialog.entries.is_empty();
        if empty {
            self.recovery = None;
        }
        cx.notify();
    }

    fn discard_all_recovery(&mut self, cx: &mut Context<Self>) {
        let _ = mdwork_core::recovery::clear(&mdwork_core::paths::recovery_dir());
        self.recovery = None;
        cx.notify();
    }

    fn submit_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let value = prompt.input.read(cx).value().trim().to_string();
        if value.is_empty() {
            self.prompt = Some(PromptDialog {
                error: Some(t!("prompt.empty").to_string()),
                ..prompt
            });
            cx.notify();
            return;
        }
        let result = match &prompt.kind {
            PromptKind::NewFolder { notebook_id } => self
                .notebooks
                .iter()
                .find(|entry| entry.notebook.marker.id == *notebook_id)
                .map(|entry| entry.notebook.clone())
                .ok_or_else(|| "notebook not found".to_string())
                .and_then(|notebook| {
                    notebook
                        .create_folder(&value)
                        .map(|_| ())
                        .map_err(|err| err.to_string())
                }),
            PromptKind::MoveEntry { notebook_id, from } => {
                let notebook = self
                    .notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == *notebook_id)
                    .map(|entry| entry.notebook.clone());
                let Some(notebook) = notebook else {
                    cx.notify();
                    return;
                };
                match notebook.rename_entry(from, &value) {
                    Ok(new_rel) => {
                        self.update_docs_after_rename(*notebook_id, from, &new_rel, cx);
                        if self.bookmarks_notebook == Some(*notebook_id) {
                            self.bookmarks.rekey(from, &new_rel);
                            self.save_bookmarks();
                        }
                        Ok(())
                    }
                    Err(err) => Err(err.to_string()),
                }
            }
            PromptKind::OpenNotebook => {
                let path = PathBuf::from(&value);
                let Some(workspace) = self.workspace.clone() else {
                    cx.notify();
                    return;
                };
                match workspace.register_external(&path) {
                    Ok(notebook) => {
                        self.active_notebook = Some(notebook.marker.id);
                        Ok(())
                    }
                    Err(err) => Err(err.to_string()),
                }
            }
        };
        match result {
            Ok(()) => {
                self.reload();
                self.persist_session(cx);
                self.reveal_active(cx);
            }
            Err(err) => {
                self.prompt = Some(PromptDialog {
                    error: Some(err),
                    ..prompt
                });
            }
        }
        cx.notify();
    }

    fn save_as_encoding(
        &mut self,
        encoding: TextEncoding,
        bom: bool,
        eol: Eol,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            self.notice = Some(t!("encoding.no_doc").to_string());
            cx.notify();
            return;
        };
        let result = doc.update(cx, |doc, cx| doc.save_as_encoding(encoding, bom, eol, cx));
        match result {
            Ok(()) => {
                self.notice = Some(
                    t!(
                        "encoding.saved_as",
                        encoding = encoding.label(),
                        eol = eol.label()
                    )
                    .to_string(),
                );
            }
            Err(err) => self.notice = Some(format!("{}：{err}", t!("encoding.failed"))),
        }
        cx.notify();
    }

    fn set_panel(&mut self, panel: SidePanel, cx: &mut Context<Self>) {
        self.panel = panel;
        cx.notify();
    }

    fn ensure_bookmarks(&mut self, cx: &mut Context<Self>) {
        if self.bookmarks_notebook == self.active_notebook {
            return;
        }
        self.bookmarks_notebook = self.active_notebook;
        self.bookmarks = self
            .active_notebook
            .and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
            })
            .map(|entry| Bookmarks::load(&entry.notebook).unwrap_or_default())
            .unwrap_or_default();
        cx.notify();
    }

    fn save_bookmarks(&mut self) {
        let Some(entry) = self.active_notebook.and_then(|id| {
            self.notebooks
                .iter()
                .find(|entry| entry.notebook.marker.id == id)
        }) else {
            return;
        };
        if let Err(err) = self.bookmarks.save(&entry.notebook) {
            self.notice = Some(format!("保存书签失败：{err}"));
        }
    }

    fn toggle_note_bookmark(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        let (notebook_id, rel_path, title) = {
            let doc = doc.read(cx);
            (doc.notebook_id, doc.rel_path.clone(), doc.title())
        };
        if notebook_id.is_none() {
            self.notice = Some(t!("bookmark.external").to_string());
            cx.notify();
            return;
        }
        self.ensure_bookmarks(cx);
        self.bookmarks.toggle_note(&rel_path, &title);
        self.save_bookmarks();
        cx.notify();
    }

    fn toggle_heading_bookmark(
        &mut self,
        rel_path: &str,
        heading: &str,
        line: usize,
        cx: &mut Context<Self>,
    ) {
        self.ensure_bookmarks(cx);
        let title = Path::new(rel_path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| rel_path.to_string());
        self.bookmarks
            .toggle_heading(rel_path, &title, heading, line);
        self.save_bookmarks();
        cx.notify();
    }

    fn remove_bookmark(&mut self, rel_path: &str, heading: Option<&str>, cx: &mut Context<Self>) {
        self.ensure_bookmarks(cx);
        self.bookmarks.remove(rel_path, heading);
        self.save_bookmarks();
        cx.notify();
    }

    fn begin_rename(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .and_then(|entry| {
                entry
                    .rows
                    .iter()
                    .find(|row| row.rel_path == rel_path)
                    .map(|row| row.name.clone())
            })
        else {
            return;
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(name)
                .clean_on_escape()
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_rename(cx),
            _ => {}
        })
        .detach();
        self.renaming = Some(Renaming {
            notebook_id,
            rel_path: rel_path.to_string(),
            input,
        });
        cx.notify();
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            cx.notify();
        }
    }

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let value = renaming.input.read(cx).value().trim().to_string();
        if value.is_empty() {
            cx.notify();
            return;
        }
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == renaming.notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let target = match renaming.rel_path.rsplit_once('/') {
            Some((parent, _)) => format!("{parent}/{value}"),
            None => value,
        };
        if target.eq_ignore_ascii_case(&renaming.rel_path) {
            cx.notify();
            return;
        }
        match notebook.rename_entry(&renaming.rel_path, &target) {
            Ok(new_rel) => {
                self.update_docs_after_rename(
                    renaming.notebook_id,
                    &renaming.rel_path,
                    &new_rel,
                    cx,
                );
                if let Some(entry) = self
                    .notebooks
                    .iter_mut()
                    .find(|entry| entry.notebook.marker.id == renaming.notebook_id)
                {
                    let from_prefix = format!("{}/", renaming.rel_path);
                    let expanded: HashSet<String> = entry
                        .expanded
                        .drain()
                        .map(|path| {
                            if path == renaming.rel_path {
                                new_rel.clone()
                            } else if let Some(rest) = path.strip_prefix(&from_prefix) {
                                format!("{new_rel}/{rest}")
                            } else {
                                path
                            }
                        })
                        .collect();
                    entry.expanded = expanded;
                }
                if self.bookmarks_notebook == Some(renaming.notebook_id) {
                    self.bookmarks.rekey(&renaming.rel_path, &new_rel);
                    self.save_bookmarks();
                }
                self.reload_entries();
                self.persist_session(cx);
                self.reveal_active(cx);
            }
            Err(err) => {
                self.notice = Some(t!("tree.rename_failed", error = err.to_string()).to_string());
            }
        }
        cx.notify();
    }

    fn update_docs_after_rename(
        &mut self,
        notebook_id: Uuid,
        from: &str,
        to: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let from_prefix = format!("{from}/");
        for doc in &self.docs {
            let rel_path = {
                let doc = doc.read(cx);
                if doc.notebook_id != Some(notebook_id) {
                    continue;
                }
                doc.rel_path.clone()
            };
            let new_rel = if rel_path == from {
                to.to_string()
            } else if let Some(rest) = rel_path.strip_prefix(&from_prefix) {
                format!("{to}/{rest}")
            } else {
                continue;
            };
            let Ok(path) = notebook.to_path(&new_rel) else {
                continue;
            };
            doc.update(cx, |doc, cx| {
                doc.rebase(new_rel, path);
                cx.notify();
            });
        }
    }

    fn delete_entry(&mut self, notebook_id: Uuid, rel_path: String, cx: &mut Context<Self>) {
        let affected: Vec<usize> = self
            .docs
            .iter()
            .enumerate()
            .filter_map(|(index, doc)| {
                let doc = doc.read(cx);
                (doc.notebook_id == Some(notebook_id) && path_under(&doc.rel_path, &rel_path))
                    .then_some(index)
            })
            .collect();
        for index in affected.into_iter().rev() {
            self.close_doc(index, cx);
        }
        let still_open = self.docs.iter().any(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id == Some(notebook_id) && path_under(&doc.rel_path, &rel_path)
        });
        if still_open {
            self.notice = Some(t!("tree.delete_busy").to_string());
            cx.notify();
            return;
        }

        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        match notebook.delete_entry(&rel_path) {
            Ok(item) => {
                if self.trash_notebook == Some(notebook_id) {
                    self.trash_notebook = None;
                }
                if self.bookmarks_notebook == Some(notebook_id) {
                    self.bookmarks.prune(&notebook);
                    self.save_bookmarks();
                }
                self.reload_entries();
                self.persist_session(cx);
                self.notice = Some(format!("{}：{}", t!("trash.moved"), item.rel_path));
            }
            Err(err) => {
                self.notice = Some(t!("tree.delete_failed", error = err.to_string()).to_string());
            }
        }
        cx.notify();
    }

    fn ensure_trash(&mut self, cx: &mut Context<Self>) {
        let target = self.active_notebook;
        if self.trash_notebook == target {
            return;
        }
        self.trash_notebook = target;
        self.trash = target
            .and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
            })
            .and_then(|entry| entry.notebook.list_trash().ok())
            .unwrap_or_default();
        cx.notify();
    }

    fn restore_trash(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let Some(notebook) = self
            .active_notebook
            .and_then(|notebook_id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == notebook_id)
            })
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        match notebook.restore_trash_item(&id) {
            Ok(rel_path) => {
                self.trash.retain(|item| item.id != id);
                self.reload_entries();
                self.persist_session(cx);
                self.notice = Some(format!("{}：{rel_path}", t!("trash.restored")));
            }
            Err(err) => {
                self.notice = Some(t!("trash.failed", error = err.to_string()).to_string());
            }
        }
        cx.notify();
    }

    fn jump_to(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.docs.iter().position(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id == Some(notebook_id) && doc.rel_path == rel_path
        });
        if let Some(index) = existing {
            self.active_doc = Some(index);
            self.active_notebook = Some(notebook_id);
            self.persist_session(cx);
            self.reveal_active(cx);
            let doc = self.docs[index].clone();
            doc.update(cx, |doc, cx| {
                doc.set_cursor(line.saturating_sub(1) as u32, 0, window, cx)
            });
            cx.notify();
            return;
        }

        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let before = self.docs.len();
        self.active_notebook = Some(notebook_id);
        self.open_note(&notebook, rel_path, window, cx);
        if self.docs.len() > before {
            let doc = self.docs[self.docs.len() - 1].clone();
            doc.update(cx, |doc, cx| {
                doc.set_cursor(line.saturating_sub(1) as u32, 0, window, cx)
            });
            self.reveal_active(cx);
            cx.notify();
        }
    }

    fn refresh_outline(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            self.outline.clear();
            self.outline_cache = None;
            return;
        };
        let (id, revision) = {
            let doc = doc.read(cx);
            (doc.editor.entity_id(), doc.revision())
        };
        if self.outline_cache == Some((id, revision)) {
            return;
        }
        let ir = match &self.preview_cache {
            Some(cache) if cache.doc == id && cache.revision == revision => cache.ir.clone(),
            _ => {
                let text = doc.read(cx).text(cx);
                PreviewIr::from_markdown(&text)
            }
        };
        self.outline = ir.outline();
        self.outline_cache = Some((id, revision));
    }

    fn set_search_scope(&mut self, all: bool, cx: &mut Context<Self>) {
        if self.search_all == all {
            return;
        }
        self.search_all = all;
        self.schedule_search(cx);
    }

    fn toggle_search_option(&mut self, option: SearchOption, cx: &mut Context<Self>) {
        match option {
            SearchOption::Case => self.search_case = !self.search_case,
            SearchOption::Regex => self.search_regex = !self.search_regex,
            SearchOption::Filename => self.search_filename = !self.search_filename,
        }
        self.schedule_search(cx);
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation += 1;
        let generation = self.search_generation;
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.search_cancel.clone();

        let mut query = SearchQuery::new(self.search_input.read(cx).value().to_string());
        query.case_sensitive = self.search_case;
        query.regex = self.search_regex;
        query.filename = self.search_filename;
        if query.is_blank() {
            self.search_results.clear();
            self.search_status = SearchStatus::Idle;
            cx.notify();
            return;
        }

        let notebooks: Vec<Notebook> = if self.search_all {
            self.notebooks
                .iter()
                .map(|entry| entry.notebook.clone())
                .collect()
        } else {
            self.active_notebook
                .and_then(|id| {
                    self.notebooks
                        .iter()
                        .find(|entry| entry.notebook.marker.id == id)
                })
                .map(|entry| entry.notebook.clone())
                .into_iter()
                .collect()
        };
        self.search_status = SearchStatus::Running;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(220))
                .await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let mut hits = Vec::new();
                    let mut error = None;
                    for notebook in &notebooks {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        match search::search_notebook(
                            notebook,
                            &query,
                            &cancel,
                            search::DEFAULT_MAX_HITS,
                        ) {
                            Ok(result) => {
                                let stop = result.cancelled;
                                hits.extend(result.hits);
                                if stop {
                                    break;
                                }
                            }
                            Err(err) => {
                                error = Some(err.to_string());
                                break;
                            }
                        }
                    }
                    (hits, error)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                match outcome {
                    (_, Some(err)) => {
                        this.search_results.clear();
                        this.search_status = SearchStatus::Failed(err);
                    }
                    (hits, None) => {
                        this.search_results = hits;
                        this.search_status = SearchStatus::Idle;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_active(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.active_doc else {
            return;
        };
        if let Some(doc) = self.docs.get(index) {
            doc.update(cx, |doc, cx| doc.save_now(cx));
        }
        self.persist_session(cx);
        cx.notify();
    }

    fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_mode = !self.preview_mode;
        if !self.preview_mode {
            if let Some(index) = self.active_doc {
                if let Some(doc) = self.docs.get(index).cloned() {
                    let (line, column) = doc.read(cx).cursor(cx);
                    doc.update(cx, |doc, cx| doc.set_cursor(line, column, window, cx));
                }
            }
        }
        cx.notify();
    }

    fn wrap_selection(
        &mut self,
        prefix: &str,
        suffix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview_mode {
            return;
        }
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        let selection = doc.read(cx).editor.read(cx).selected_value().to_string();
        let (text, caret_back) = if selection.is_empty() {
            (format!("{prefix}{suffix}"), suffix.chars().count() as u32)
        } else {
            (format!("{prefix}{selection}{suffix}"), 0)
        };
        doc.update(cx, |doc, cx| doc.insert_text(&text, window, cx));
        if caret_back > 0 {
            let (line, column) = doc.read(cx).cursor(cx);
            let target = column.saturating_sub(caret_back);
            doc.update(cx, |doc, cx| doc.set_cursor(line, target, window, cx));
        }
        cx.notify();
    }

    fn ensure_preview(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            self.preview_cache = None;
            return;
        };
        let (id, revision) = {
            let doc = doc.read(cx);
            (doc.editor.entity_id(), doc.revision())
        };
        if self
            .preview_cache
            .as_ref()
            .is_some_and(|cache| cache.doc == id && cache.revision == revision)
        {
            return;
        }
        let text = doc.read(cx).text(cx);
        self.preview_cache = Some(PreviewCache {
            doc: id,
            revision,
            ir: PreviewIr::from_markdown(&text),
        });
        self.queue_math(cx);
    }

    /// Rasterises any equation in the current preview that has no cached SVG yet.
    fn queue_math(&mut self, cx: &mut Context<Self>) {
        let Some(cache) = &self.preview_cache else {
            return;
        };
        let mut formulas = Vec::new();
        preview::collect_math(&cache.ir, &mut |tex, display| {
            formulas.push((tex.to_string(), display));
        });

        let fill = preview::fill_hex(cx.theme().foreground);
        let mut pending = Vec::new();
        for (tex, display) in formulas {
            let key = preview::math_key(&tex, display, &fill, preview::MATH_SIZE_PT);
            if self.math.contains_key(&key) || !self.math_pending.insert(key) {
                continue;
            }
            pending.push((key, tex, display));
        }

        for (key, tex, display) in pending {
            let fill = fill.clone();
            cx.spawn(async move |this, cx| {
                let svg = cx
                    .background_executor()
                    .spawn(async move {
                        mdwork_export::math::math_svg(
                            &tex,
                            &mdwork_export::math::MathOptions {
                                fill: &fill,
                                size_pt: preview::MATH_SIZE_PT,
                                display,
                            },
                        )
                        .ok()
                    })
                    .await;
                let path = svg.and_then(|svg| write_math_cache(key, &svg));
                let _ = this.update(cx, |this, cx| {
                    this.math_pending.remove(&key);
                    this.math.insert(key, path);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn toggle_code_wrap(&mut self, cx: &mut Context<Self>) {
        self.config.code_wrap = !self.config.code_wrap;
        if let Err(err) = self.config.save() {
            self.notice = Some(format!("保存设置失败：{err}"));
        }
        cx.notify();
    }

    fn palette_items(&self) -> Vec<PaletteItem> {
        let item = |key: &str, hint: &str, command: PaletteCommand| PaletteItem {
            label: t!(key).to_string(),
            hint: hint.to_string(),
            command,
        };
        vec![
            item("palette.new_note", "Ctrl+N", PaletteCommand::NewNote),
            item("palette.new_folder", "", PaletteCommand::NewFolder),
            item("palette.new_notebook", "", PaletteCommand::NewNotebook),
            item("palette.open_notebook", "", PaletteCommand::OpenNotebook),
            item("palette.save", "Ctrl+S", PaletteCommand::Save),
            item(
                "palette.toggle_preview",
                "Ctrl+T",
                PaletteCommand::TogglePreview,
            ),
            item("palette.close_tab", "", PaletteCommand::CloseTab),
            item("palette.settings", "", PaletteCommand::Settings),
            item("palette.search", "", PaletteCommand::Search),
            item("palette.outline", "", PaletteCommand::Outline),
            item("palette.bookmarks", "", PaletteCommand::Bookmarks),
            item("palette.trash", "", PaletteCommand::Trash),
            item("palette.notes_panel", "", PaletteCommand::NotesPanel),
            item("palette.export_pdf", "", PaletteCommand::ExportPdf),
            item("palette.export_png", "", PaletteCommand::ExportPng),
            item("palette.export_zip", "", PaletteCommand::ExportNotebookZip),
            item("palette.import_zip", "", PaletteCommand::ImportNotebookZip),
            item("palette.move_note", "", PaletteCommand::MoveEntry),
            item("palette.theme_system", "", PaletteCommand::ThemeSystem),
            item("palette.theme_light", "", PaletteCommand::ThemeLight),
            item("palette.theme_dark", "", PaletteCommand::ThemeDark),
            item("palette.code_wrap", "", PaletteCommand::ToggleCodeWrap),
            item("palette.autosave", "", PaletteCommand::ToggleAutosave),
            item("palette.save_utf8", "", PaletteCommand::SaveAsUtf8),
            item("palette.save_gb18030", "", PaletteCommand::SaveAsGb18030),
            item(
                "palette.save_windows1252",
                "",
                PaletteCommand::SaveAsWindows1252,
            ),
            item("palette.save_utf16", "", PaletteCommand::SaveAsUtf16),
            item("palette.eol_lf", "", PaletteCommand::SaveEolLf),
            item("palette.eol_crlf", "", PaletteCommand::SaveEolCrlf),
            item(
                "palette.open_workspace",
                "",
                PaletteCommand::OpenWorkspaceFolder,
            ),
            item("palette.open_logs", "", PaletteCommand::OpenLogFolder),
            item(
                "palette.register_assoc",
                "",
                PaletteCommand::RegisterFileAssoc,
            ),
            item("palette.rescan", "", PaletteCommand::Rescan),
        ]
    }

    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("palette.placeholder").to_string())
                .clean_on_escape()
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.refresh_palette(cx);
            }
            InputEvent::PressEnter { .. } => {
                let handle = this.window_handle;
                let weak = cx.entity().downgrade();
                let _ = handle.update(cx, |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| this.run_palette_selected(window, cx));
                });
            }
            _ => {}
        })
        .detach();
        let items = self.palette_items();
        let filtered = (0..items.len()).collect();
        self.palette = Some(Palette {
            input,
            items,
            filtered,
            selected: 0,
        });
        cx.notify();
    }

    fn close_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            cx.notify();
        }
    }

    fn refresh_palette(&mut self, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        let query = palette.input.read(cx).value().trim().to_lowercase();
        let mut scored: Vec<(i64, usize)> = palette
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let score = fuzzy_score(&item.label.to_lowercase(), &query)?;
                Some((score, index))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        palette.filtered = scored.into_iter().map(|(_, index)| index).collect();
        palette.selected = 0;
        cx.notify();
    }

    fn palette_step(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        if palette.filtered.is_empty() {
            return;
        }
        let len = palette.filtered.len() as i64;
        let next = (palette.selected as i64 + delta).rem_euclid(len);
        palette.selected = next as usize;
        cx.notify();
    }

    fn run_palette_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.palette.as_ref() else {
            return;
        };
        let command = palette
            .filtered
            .get(palette.selected)
            .and_then(|index| palette.items.get(*index))
            .map(|item| item.command);
        let Some(command) = command else {
            return;
        };
        self.close_palette(cx);
        self.run_palette_command(command, window, cx);
    }

    fn run_palette_command(
        &mut self,
        command: PaletteCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            PaletteCommand::NewNote => self.new_note(window, cx),
            PaletteCommand::NewFolder => self.new_folder(window, cx),
            PaletteCommand::NewNotebook => self.new_notebook(cx),
            PaletteCommand::OpenNotebook => self.open_external_notebook_prompt(window, cx),
            PaletteCommand::Save => self.save_active(cx),
            PaletteCommand::TogglePreview => self.toggle_preview(window, cx),
            PaletteCommand::CloseTab => {
                if let Some(index) = self.active_doc {
                    self.close_doc(index, cx);
                }
            }
            PaletteCommand::Settings => self.open_settings(window, cx),
            PaletteCommand::Search => {
                self.set_panel(SidePanel::Search, cx);
                self.search_input
                    .update(cx, |state, cx| state.focus(window, cx));
            }
            PaletteCommand::Outline => self.set_panel(SidePanel::Outline, cx),
            PaletteCommand::Bookmarks => self.set_panel(SidePanel::Bookmarks, cx),
            PaletteCommand::Trash => self.set_panel(SidePanel::Trash, cx),
            PaletteCommand::NotesPanel => self.set_panel(SidePanel::Notes, cx),
            PaletteCommand::ExportPdf => {
                self.open_export_dialog_with(ExportFormat::Pdf, window, cx)
            }
            PaletteCommand::ExportPng => {
                self.open_export_dialog_with(ExportFormat::Png, window, cx)
            }
            PaletteCommand::ExportNotebookZip => {
                if let Some(id) = self.active_notebook {
                    self.open_zip_export(id, window, cx);
                } else {
                    self.notice = Some(t!("zip.no_notebook").to_string());
                    cx.notify();
                }
            }
            PaletteCommand::ImportNotebookZip => self.open_zip_import(window, cx),
            PaletteCommand::MoveEntry => {
                let target = self.active_doc_entity().and_then(|doc| {
                    let doc = doc.read(cx);
                    doc.notebook_id.map(|id| (id, doc.rel_path.clone()))
                });
                if let Some((notebook_id, rel_path)) = target {
                    self.begin_move(notebook_id, &rel_path, window, cx);
                } else {
                    self.notice = Some(t!("tree.move_external").to_string());
                    cx.notify();
                }
            }
            PaletteCommand::ThemeSystem => {
                self.set_theme_preference(ThemePreference::System, window, cx)
            }
            PaletteCommand::ThemeLight => {
                self.set_theme_preference(ThemePreference::Light, window, cx)
            }
            PaletteCommand::ThemeDark => {
                self.set_theme_preference(ThemePreference::Dark, window, cx)
            }
            PaletteCommand::ToggleCodeWrap => self.toggle_code_wrap(cx),
            PaletteCommand::ToggleAutosave => self.set_autosave(!self.config.autosave, cx),
            PaletteCommand::SaveAsUtf8 => {
                let (_, _, eol) = self.current_encoding(cx);
                self.save_as_encoding(TextEncoding::Utf8, false, eol, cx)
            }
            PaletteCommand::SaveAsGb18030 => {
                let (_, _, eol) = self.current_encoding(cx);
                self.save_as_encoding(TextEncoding::Gb18030, false, eol, cx)
            }
            PaletteCommand::SaveAsWindows1252 => {
                let (_, _, eol) = self.current_encoding(cx);
                self.save_as_encoding(TextEncoding::Windows1252, false, eol, cx)
            }
            PaletteCommand::SaveAsUtf16 => {
                let (_, _, eol) = self.current_encoding(cx);
                self.save_as_encoding(TextEncoding::Utf16Le, true, eol, cx)
            }
            PaletteCommand::SaveEolLf => {
                let (encoding, bom, _) = self.current_encoding(cx);
                self.save_as_encoding(encoding, bom, Eol::Lf, cx)
            }
            PaletteCommand::SaveEolCrlf => {
                let (encoding, bom, _) = self.current_encoding(cx);
                self.save_as_encoding(encoding, bom, Eol::Crlf, cx)
            }
            PaletteCommand::OpenWorkspaceFolder => {
                let root = self
                    .workspace
                    .as_ref()
                    .map(|workspace| workspace.root().to_path_buf());
                self.open_path_in_explorer(root, cx);
            }
            PaletteCommand::OpenLogFolder => {
                self.open_path_in_explorer(Some(mdwork_core::paths::logs_dir()), cx)
            }
            PaletteCommand::RegisterFileAssoc => self.register_file_assoc(cx),
            PaletteCommand::Rescan => {
                self.reload();
                self.persist_session(cx);
                cx.notify();
            }
        }
    }

    fn current_encoding(&self, cx: &App) -> (TextEncoding, bool, Eol) {
        self.active_doc_entity()
            .map(|doc| {
                let doc = doc.read(cx);
                (doc.encoding, doc.bom, doc.eol)
            })
            .unwrap_or((TextEncoding::Utf8, false, Eol::Lf))
    }

    fn copy_preview(&mut self, cx: &mut Context<Self>) {
        self.ensure_preview(cx);
        let Some(cache) = &self.preview_cache else {
            return;
        };
        let text = cache.ir.to_plain_text();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn persist_session(&mut self, cx: &App) {
        let mut session = Session::default();
        session.active_notebook = self.active_notebook;

        for entry in &self.notebooks {
            let id = entry.notebook.marker.id;
            let mut saved = NotebookSession {
                id,
                ..Default::default()
            };
            for doc in &self.docs {
                let doc = doc.read(cx);
                if doc.notebook_id == Some(id) {
                    saved.open_notes.push(doc.rel_path.clone());
                    let (line, column) = doc.cursor(cx);
                    saved
                        .cursors
                        .insert(doc.rel_path.clone(), CursorPos { line, column });
                }
            }
            if let Some(index) = self.active_doc {
                if let Some(doc) = self.docs.get(index) {
                    let doc = doc.read(cx);
                    if doc.notebook_id == Some(id) {
                        saved.active_note = Some(doc.rel_path.clone());
                    }
                }
            }
            session.notebooks.push(saved);
        }

        for doc in &self.docs {
            let doc = doc.read(cx);
            if doc.notebook_id.is_none() {
                let path = doc.path.display().to_string();
                let (line, column) = doc.cursor(cx);
                session
                    .external_cursors
                    .insert(path.clone(), CursorPos { line, column });
                session.external_notes.push(path);
            }
        }
        if let Some(index) = self.active_doc {
            if let Some(doc) = self.docs.get(index) {
                let doc = doc.read(cx);
                if doc.notebook_id.is_none() {
                    session.active_external = Some(doc.path.display().to_string());
                }
            }
        }

        if let Err(err) = session.save() {
            tracing::warn!(error = %err, "保存会话失败");
        }
        self.session = session;
    }

    fn active_doc_entity(&self) -> Option<&Entity<Doc>> {
        self.active_doc.and_then(|index| self.docs.get(index))
    }

    fn render_title(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .pr_3()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .child(t!("app.name").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("app.subtitle").to_string()),
                    ),
            )
            .child(div().flex_1())
            .child(
                title_icon(
                    "title-new-note",
                    IconName::SquarePen,
                    t!("sidebar.new_note").to_string(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx))),
            )
            .child(
                title_icon(
                    "title-export",
                    IconName::Download,
                    t!("export.title").to_string(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_export_dialog(window, cx))),
            )
            .child(
                title_icon(
                    "title-zip-export",
                    IconName::Package,
                    t!("zip.export_label").to_string(),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    let Some(id) = this.active_notebook else {
                        this.notice = Some(t!("zip.no_notebook").to_string());
                        cx.notify();
                        return;
                    };
                    this.open_zip_export(id, window, cx);
                })),
            )
            .child(
                title_icon(
                    "title-zip-import",
                    IconName::FolderInput,
                    t!("zip.import_label").to_string(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_zip_import(window, cx))),
            )
            .child(
                title_icon(
                    "title-settings",
                    IconName::Settings,
                    t!("settings.title").to_string(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            )
            .into_any_element()
    }

    fn render_notice(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self.notice.clone().unwrap_or_default();
        div()
            .px_3()
            .py_1()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().list_active)
            .text_color(cx.theme().foreground)
            .text_sm()
            .child(div().flex_1().child(text))
            .child(
                Button::new("btn-dismiss")
                    .ghost()
                    .compact()
                    .label("×")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn open_export_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_export_dialog_with(ExportFormat::Pdf, window, cx);
    }

    fn open_export_dialog_with(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.export.is_some() {
            return;
        }
        let Some(doc_entity) = self.active_doc_entity().cloned() else {
            self.notice = Some(t!("export.no_doc").to_string());
            cx.notify();
            return;
        };
        let (notebook_id, notebook_name, notebook_root, note_rel, title, markdown) = {
            let doc = doc_entity.read(cx);
            let notebook_id = doc.notebook_id;
            let notebook = notebook_id.and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
            });
            let (name, root) = match notebook {
                Some(entry) => (
                    entry.notebook.display_name().to_string(),
                    entry.notebook.root.clone(),
                ),
                None => (
                    String::new(),
                    doc.path.parent().map(Path::to_path_buf).unwrap_or_default(),
                ),
            };
            let note_rel = if notebook_id.is_some() {
                doc.rel_path.clone()
            } else {
                doc.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| doc.rel_path.clone())
            };
            (
                notebook_id,
                name,
                root,
                note_rel,
                doc.title(),
                doc.text(cx).to_string(),
            )
        };

        let prefs = notebook_id
            .map(|id| NotebookPrefs::load(&id))
            .unwrap_or_default();
        let options = prefs.export.clone();
        let dir = prefs
            .export_dir(true)
            .unwrap_or_else(|| notebook_root.clone());
        let stem = Path::new(&note_rel)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "export".to_string());
        let output = dir.join(format!("{stem}.{}", format.extension()));

        let header_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(options.header.clone())
                .placeholder(t!("export.header_placeholder").to_string())
                .clean_on_escape()
        });
        let footer_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(options.footer.clone())
                .placeholder(t!("export.footer_placeholder").to_string())
                .clean_on_escape()
        });
        let output_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(output.display().to_string())
                .clean_on_escape()
        });

        self.export = Some(ExportDialog {
            notebook_id,
            notebook_root,
            notebook_name,
            note_rel,
            title,
            markdown,
            format,
            options,
            header_input,
            footer_input,
            output_input,
            job: None,
            busy: false,
            stage: None,
            progress: None,
            warnings: Vec::new(),
            error: None,
        });
        cx.notify();
    }

    fn close_export_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.export.take() {
            if let Some(job) = dialog.job {
                job.cancel();
            }
        }
        cx.notify();
    }

    fn set_export_option(
        &mut self,
        apply: impl FnOnce(&mut ExportOptions),
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.export.as_mut() {
            apply(&mut dialog.options);
            cx.notify();
        }
    }

    fn set_export_format(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.export.as_mut() else {
            return;
        };
        if dialog.format == format || dialog.busy {
            return;
        }
        dialog.format = format;
        let current = PathBuf::from(dialog.output_input.read(cx).value().to_string());
        let next = ensure_export_extension(current, format);
        dialog.output_input.update(cx, |state, cx| {
            state.set_value(next.display().to_string(), window, cx)
        });
        cx.notify();
    }

    fn pick_export_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.export.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let current = PathBuf::from(dialog.output_input.read(cx).value().to_string());
        let dir = current
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dialog.notebook_root.clone());
        let name = current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("export.{}", dialog.format.extension()));
        let receiver = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = receiver.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    if let Some(dialog) = this.export.as_mut() {
                        let path = ensure_export_extension(path, dialog.format);
                        let display = path.display().to_string();
                        dialog
                            .output_input
                            .update(cx, |state, cx| state.set_value(display, window, cx));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn begin_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.export.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let cli = match cli_path() {
            Ok(path) => path,
            Err(message) => {
                if let Some(dialog) = self.export.as_mut() {
                    dialog.error = Some(message);
                }
                cx.notify();
                return;
            }
        };
        let raw_output = PathBuf::from(dialog.output_input.read(cx).value().to_string());
        if raw_output.file_name().is_none() {
            if let Some(dialog) = self.export.as_mut() {
                dialog.error = Some(t!("export.invalid_output").to_string());
            }
            cx.notify();
            return;
        }
        let output = ensure_export_extension(raw_output, dialog.format);
        let mut options = dialog.options.clone();
        options.header = dialog.header_input.read(cx).value().to_string();
        options.footer = dialog.footer_input.read(cx).value().to_string();
        let request = ExportRequest {
            notebook_root: dialog.notebook_root.clone(),
            note_rel: dialog.note_rel.clone(),
            notebook_name: dialog.notebook_name.clone(),
            title: dialog.title.clone(),
            format: dialog.format,
            output: output.clone(),
            options: options.clone(),
            markdown: Some(dialog.markdown.clone()),
            temp_dir: None,
        };
        let notebook_id = dialog.notebook_id;

        if let Some(id) = notebook_id {
            let mut prefs = NotebookPrefs::load(&id);
            prefs.export = options.clone();
            if let Some(parent) = output.parent() {
                prefs.set_export_dir(dialog.format == ExportFormat::Pdf, parent);
            }
            if let Err(err) = prefs.save(&id) {
                tracing::warn!(error = %err, "保存笔记本偏好失败");
            }
        }

        match ExportJob::start(&cli, &request) {
            Ok(job) => {
                if let Some(dialog) = self.export.as_mut() {
                    dialog.job = Some(job);
                    dialog.busy = true;
                    dialog.stage = Some("read".to_string());
                    dialog.progress = None;
                    dialog.warnings.clear();
                    dialog.error = None;
                    let display = output.display().to_string();
                    dialog
                        .output_input
                        .update(cx, |state, cx| state.set_value(display, window, cx));
                }
                self.spawn_export_poll(cx);
            }
            Err(err) => {
                if let Some(dialog) = self.export.as_mut() {
                    dialog.error = Some(err.to_string());
                }
            }
        }
        cx.notify();
    }

    fn spawn_export_poll(&self, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let keep = weak
                    .update(cx, |this, cx| this.poll_export(cx))
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn poll_export(&mut self, _cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.export.as_mut() else {
            return false;
        };
        let Some(job) = dialog.job.as_ref() else {
            return false;
        };
        let events = job.drain();
        let finished = job.is_finished();
        let mut done = None;
        let mut failed = None;
        for event in events {
            match event {
                ExportEvent::Stage { stage } => dialog.stage = Some(stage),
                ExportEvent::Page { page, pages } => dialog.progress = Some((page, pages)),
                ExportEvent::Warning { message } => dialog.warnings.push(message),
                ExportEvent::Done { paths, pages } => done = Some((paths, pages)),
                ExportEvent::Error { message } => failed = Some(message),
            }
        }

        let mut close_with = None;
        if let Some(message) = failed {
            dialog.busy = false;
            dialog.job = None;
            dialog.error = Some(message);
        } else if let Some((paths, _pages)) = done {
            dialog.busy = false;
            dialog.job = None;
            close_with = Some(if paths.len() == 1 {
                t!("export.done", path = paths[0].clone()).to_string()
            } else {
                t!("export.done_count", count = paths.len()).to_string()
            });
        } else if finished && dialog.job.is_some() {
            dialog.busy = false;
            dialog.job = None;
            dialog.error = Some(t!("export.failed").to_string());
        }

        if let Some(notice) = close_with {
            self.export = None;
            self.notice = Some(notice);
            return false;
        }
        dialog.job.is_some()
    }

    fn open_zip_export(&mut self, notebook_id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        if self.zip.is_some() {
            return;
        }
        let Some(entry) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            self.notice = Some(t!("zip.no_notebook").to_string());
            cx.notify();
            return;
        };
        let notebook_root = entry.notebook.root.clone();
        let notebook_name = entry.notebook.display_name().to_string();
        let stem = notebook_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "notebook".to_string());
        let prefs = NotebookPrefs::load(&notebook_id);
        let dir = prefs
            .last_zip_dir
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .or_else(|| notebook_root.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| notebook_root.clone());
        let output = unique_zip_path(&dir, &stem);

        let path_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(output.display().to_string())
                .clean_on_escape()
        });
        let rename_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(notebook_name.clone())
                .placeholder(t!("zip.rename_placeholder").to_string())
                .clean_on_escape()
        });
        subscribe_zip_path(cx, &path_input);
        subscribe_zip_path(cx, &rename_input);

        self.zip = Some(ZipDialog {
            mode: ZipDialogMode::Export,
            notebook_id: Some(notebook_id),
            notebook_name,
            workspace_root: self
                .workspace
                .as_ref()
                .map(|workspace| workspace.root().to_path_buf())
                .unwrap_or_default(),
            path_input,
            rename_input,
            import_mode: ImportMode::Merge,
            policy: ConflictPolicy::KeepBoth,
            choices: HashMap::new(),
            plan: None,
            plan_generation: 0,
            plan_loading: false,
            plan_error: None,
            confirm_overwrite: false,
            busy: false,
            error: None,
        });
        cx.notify();
    }

    fn open_zip_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.zip.is_some() {
            return;
        }
        let Some(root) = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().to_path_buf())
        else {
            self.notice = Some(t!("zip.no_workspace").to_string());
            cx.notify();
            return;
        };
        let notebook_id = self.active_notebook;
        let notebook_name = self
            .active_notebook
            .and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
                    .map(|entry| entry.notebook.display_name().to_string())
            })
            .unwrap_or_default();
        let dir = notebook_id
            .map(|id| NotebookPrefs::load(&id))
            .and_then(|prefs| prefs.last_import_dir)
            .map(PathBuf::from)
            .filter(|path| path.is_dir());

        let path_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .placeholder(t!("zip.path_placeholder").to_string())
                .clean_on_escape();
            if let Some(dir) = &dir {
                input = input.default_value(dir.display().to_string());
            }
            input
        });
        let rename_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("zip.rename_placeholder").to_string())
                .clean_on_escape()
        });
        subscribe_zip_path(cx, &path_input);
        subscribe_zip_path(cx, &rename_input);

        self.zip = Some(ZipDialog {
            mode: ZipDialogMode::Import,
            notebook_id,
            notebook_name,
            workspace_root: root,
            path_input,
            rename_input,
            import_mode: ImportMode::Merge,
            policy: ConflictPolicy::KeepBoth,
            choices: HashMap::new(),
            plan: None,
            plan_generation: 0,
            plan_loading: false,
            plan_error: None,
            confirm_overwrite: false,
            busy: false,
            error: None,
        });
        cx.notify();
    }

    fn close_zip_dialog(&mut self, cx: &mut Context<Self>) {
        self.zip = None;
        cx.notify();
    }

    fn zip_rename_to(&self, cx: &Context<Self>) -> Option<String> {
        let dialog = self.zip.as_ref()?;
        if dialog.import_mode != ImportMode::Rename {
            return None;
        }
        let value = dialog.rename_input.read(cx).value().trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    }

    fn schedule_zip_plan(&mut self, cx: &mut Context<Self>) {
        let rename_to = self.zip_rename_to(cx);
        let Some(dialog) = self.zip.as_mut() else {
            return;
        };
        if dialog.mode != ZipDialogMode::Import || dialog.busy {
            return;
        }
        dialog.plan_generation = dialog.plan_generation.wrapping_add(1);
        let generation = dialog.plan_generation;
        let raw = dialog.path_input.read(cx).value().trim().to_string();
        if raw.is_empty() {
            dialog.plan = None;
            dialog.plan_error = None;
            dialog.plan_loading = false;
            cx.notify();
            return;
        }
        let path = PathBuf::from(&raw);
        if !path.is_file() {
            dialog.plan = None;
            dialog.plan_error = None;
            dialog.plan_loading = false;
            cx.notify();
            return;
        }
        let mode = dialog.import_mode;
        let root = dialog.workspace_root.clone();
        dialog.plan_loading = true;
        dialog.plan_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(180))
                .await;
            let result = cx
                .background_executor()
                .spawn(
                    async move { archive::plan_import(&root, &path, mode, rename_to.as_deref()) },
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(dialog) = this.zip.as_mut() else {
                    return;
                };
                if dialog.plan_generation != generation {
                    return;
                }
                dialog.plan_loading = false;
                match result {
                    Ok(plan) => {
                        dialog
                            .choices
                            .retain(|path, _| plan.conflicts.iter().any(|item| &item.path == path));
                        dialog.plan = Some(plan);
                        dialog.plan_error = None;
                    }
                    Err(err) => {
                        dialog.plan = None;
                        dialog.plan_error = Some(err.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_zip_import_mode(&mut self, mode: ImportMode, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_mut() else {
            return;
        };
        if dialog.import_mode == mode || dialog.busy {
            return;
        }
        dialog.import_mode = mode;
        dialog.confirm_overwrite = false;
        dialog.error = None;
        cx.notify();
        self.schedule_zip_plan(cx);
    }

    fn set_zip_policy(&mut self, policy: ConflictPolicy, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_mut() else {
            return;
        };
        dialog.policy = policy;
        dialog.choices.clear();
        cx.notify();
    }

    fn set_zip_choice(&mut self, path: &str, policy: ConflictPolicy, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_mut() else {
            return;
        };
        if policy == dialog.policy {
            dialog.choices.remove(path);
        } else {
            dialog.choices.insert(path.to_string(), policy);
        }
        cx.notify();
    }

    fn pick_zip_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let mode = dialog.mode;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("zip.pick_prompt").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = picked else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(dialog) = this.zip.as_mut() else {
                    return;
                };
                let display = path.display().to_string();
                dialog
                    .path_input
                    .update(cx, |state, cx| state.set_value(display, window, cx));
                match mode {
                    ZipDialogMode::Import => this.schedule_zip_plan(cx),
                    ZipDialogMode::Export => {
                        let path = ensure_zip_extension(path);
                        let display = path.display().to_string();
                        if let Some(dialog) = this.zip.as_mut() {
                            dialog
                                .path_input
                                .update(cx, |state, cx| state.set_value(display, window, cx));
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn pick_zip_export_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let current = PathBuf::from(dialog.path_input.read(cx).value().to_string());
        let dir = current
            .parent()
            .map(Path::to_path_buf)
            .filter(|path| path.is_dir())
            .unwrap_or_else(|| dialog.workspace_root.clone());
        let name = current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{}.zip", dialog.notebook_name));
        let receiver = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(path))) => path,
                _ => return,
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.zip.is_some() {
                    let path = ensure_zip_extension(path);
                    if let Some(dialog) = this.zip.as_mut() {
                        dialog.path_input.update(cx, |state, cx| {
                            state.set_value(path.display().to_string(), window, cx)
                        });
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn begin_zip_export(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let Some(notebook_id) = dialog.notebook_id else {
            return;
        };
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let output = ensure_zip_extension(PathBuf::from(
            dialog.path_input.read(cx).value().trim().to_string(),
        ));
        if output.file_name().is_none() {
            if let Some(dialog) = self.zip.as_mut() {
                dialog.error = Some(t!("zip.invalid_path").to_string());
            }
            cx.notify();
            return;
        }
        if let Some(dialog) = self.zip.as_mut() {
            dialog.busy = true;
            dialog.error = None;
        }
        cx.notify();
        self.spawn_zip_export(notebook, output, notebook_id, cx);
    }

    fn spawn_zip_export(
        &mut self,
        notebook: Notebook,
        output: PathBuf,
        notebook_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let target = output.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { archive::export_zip(&notebook, &target) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let message = match result {
                    Ok(summary) => {
                        this.zip = None;
                        let mut prefs = NotebookPrefs::load(&notebook_id);
                        prefs.last_zip_dir =
                            output.parent().map(|parent| parent.display().to_string());
                        if let Err(err) = prefs.save(&notebook_id) {
                            tracing::warn!(error = %err, "保存笔记本偏好失败");
                        }
                        t!(
                            "zip.export_done",
                            path = output.display().to_string(),
                            files = summary.files
                        )
                        .to_string()
                    }
                    Err(err) => {
                        if let Some(dialog) = this.zip.as_mut() {
                            dialog.busy = false;
                            dialog.error = Some(err.to_string());
                        }
                        return cx.notify();
                    }
                };
                this.notice = Some(message);
                cx.notify();
            });
        })
        .detach();
    }

    fn begin_zip_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_ref() else {
            return;
        };
        if dialog.busy || dialog.mode != ZipDialogMode::Import {
            return;
        }
        let zip_path = PathBuf::from(dialog.path_input.read(cx).value().trim().to_string());
        if !zip_path.is_file() {
            if let Some(dialog) = self.zip.as_mut() {
                dialog.error = Some(t!("zip.invalid_path").to_string());
            }
            cx.notify();
            return;
        }
        if dialog.import_mode == ImportMode::Overwrite && !dialog.confirm_overwrite {
            if let Some(dialog) = self.zip.as_mut() {
                dialog.confirm_overwrite = true;
            }
            cx.notify();
            return;
        }
        let options = ImportOptions {
            mode: dialog.import_mode,
            policy: dialog.policy,
            choices: dialog.choices.clone(),
            rename_to: self.zip_rename_to(cx),
        };
        let root = dialog.workspace_root.clone();
        let notebook_id = dialog.notebook_id;

        for doc in &self.docs {
            doc.update(cx, |doc, cx| {
                if doc.dirty {
                    doc.save_now(cx);
                }
            });
        }
        let unsaved = self
            .docs
            .iter()
            .find(|doc| doc.read(cx).dirty)
            .map(|doc| doc.read(cx).path.display().to_string());
        if let Some(path) = unsaved {
            if let Some(dialog) = self.zip.as_mut() {
                dialog.error = Some(t!("zip.save_failed", path = path).to_string());
            }
            cx.notify();
            return;
        }

        if let Some(dialog) = self.zip.as_mut() {
            dialog.busy = true;
            dialog.error = None;
        }
        cx.notify();
        self.spawn_zip_import(root, zip_path, options, notebook_id, window, cx);
    }

    fn spawn_zip_import(
        &mut self,
        root: PathBuf,
        zip_path: PathBuf,
        options: ImportOptions,
        notebook_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = zip_path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { archive::import_zip(&root, &target, &options) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(outcome) => {
                        let root = outcome.notebook.root.clone();
                        let name = outcome.notebook.display_name().to_string();
                        let new_id = outcome.notebook.marker.id;
                        this.zip = None;
                        if let Some(id) = notebook_id {
                            let mut prefs = NotebookPrefs::load(&id);
                            prefs.last_import_dir =
                                zip_path.parent().map(|parent| parent.display().to_string());
                            if let Err(err) = prefs.save(&id) {
                                tracing::warn!(error = %err, "保存笔记本偏好失败");
                            }
                        }
                        this.reload();
                        this.reload_docs_under(&root, window, cx);
                        this.active_notebook = Some(new_id);
                        this.persist_session(cx);
                        this.notice = Some(
                            t!(
                                "zip.import_done",
                                name = name,
                                imported = outcome.imported,
                                same = outcome.skipped_identical,
                                both = outcome.kept_both
                            )
                            .to_string(),
                        );
                    }
                    Err(err) => {
                        if let Some(dialog) = this.zip.as_mut() {
                            dialog.busy = false;
                            dialog.error = Some(err.to_string());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh open documents that live in a notebook folder that was just
    /// rewritten, and drop tabs whose files no longer exist.
    fn reload_docs_under(&mut self, root: &Path, window: &mut Window, cx: &mut Context<Self>) {
        for index in (0..self.docs.len()).rev() {
            let doc = self.docs[index].clone();
            let path = doc.read(cx).path.clone();
            if !path.starts_with(root) {
                continue;
            }
            if path.exists() {
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let hash = mdwork_core::doc::sha256_hex(&bytes);
                        let decoded = mdwork_core::doc::decode(&bytes);
                        doc.update(cx, |doc, cx| {
                            doc.apply_disk(decoded, hash, window, cx);
                        });
                    }
                    Err(err) => {
                        doc.update(cx, |doc, cx| {
                            doc.error = Some(err.to_string());
                            cx.notify();
                        });
                    }
                }
            } else {
                self.docs.remove(index);
                self.active_doc = match self.active_doc {
                    Some(active) if active == index => {
                        if self.docs.is_empty() {
                            None
                        } else {
                            Some(index.min(self.docs.len() - 1))
                        }
                    }
                    Some(active) if active > index => Some(active - 1),
                    other => other,
                };
            }
        }
        cx.notify();
    }

    fn render_export_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.export.as_ref() else {
            return div().into_any_element();
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let overlay = cx.theme().overlay;
        let danger = cx.theme().danger;
        let label = |text: String| div().w(px(64.)).text_xs().text_color(muted).child(text);
        let format_button =
            |format: ExportFormat, label_key: &'static str, cx: &mut Context<Self>| {
                let active = dialog.format == format;
                Button::new(format!("export-format-{}", format.extension()))
                    .ghost()
                    .compact()
                    .label(t!(label_key).to_string())
                    .when(active, |button| button.primary())
                    .disabled(dialog.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_export_format(format, window, cx)
                    }))
            };
        let checkbox = |id: &'static str,
                        label_key: &'static str,
                        checked: bool,
                        apply: fn(&mut ExportOptions, bool),
                        cx: &mut Context<Self>| {
            Checkbox::new(id)
                .label(t!(label_key).to_string())
                .checked(checked)
                .disabled(dialog.busy)
                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                    this.set_export_option(|options| apply(options, *checked), cx)
                }))
        };
        let cycle = |id: &'static str,
                     text: String,
                     apply: fn(&mut ExportOptions),
                     cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .compact()
                .label(text)
                .disabled(dialog.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.set_export_option(apply, cx)))
        };

        let paper_label = match dialog.options.paper {
            PaperSize::A4 => "A4",
            PaperSize::A5 => "A5",
            PaperSize::Letter => "Letter",
            PaperSize::Custom => "Custom",
        };
        let status = if let Some(error) = &dialog.error {
            Some((error.clone(), danger))
        } else if dialog.busy {
            let stage = dialog
                .stage
                .as_deref()
                .map(stage_label)
                .unwrap_or_else(|| t!("export.busy").to_string());
            let text = match dialog.progress {
                Some((page, pages)) => {
                    format!(
                        "{stage} · {}",
                        t!("export.page", page = page, pages = pages)
                    )
                }
                None => stage,
            };
            Some((text, muted))
        } else if !dialog.warnings.is_empty() {
            Some((
                t!("export.warnings", count = dialog.warnings.len()).to_string(),
                muted,
            ))
        } else {
            None
        };

        let output_value = dialog.output_input.read(cx).value().to_string();
        let can_start = !dialog.busy && !output_value.trim().is_empty();

        div()
            .id("export-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .child(
                div()
                    .id("export-dialog")
                    .v_flex()
                    .w(px(620.))
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_base()
                                    .font_bold()
                                    .child(t!("export.title").to_string()),
                            )
                            .child(
                                Button::new("export-close")
                                    .ghost()
                                    .compact()
                                    .label("×")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.close_export_dialog(cx)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("export.format").to_string()))
                            .child(format_button(ExportFormat::Pdf, "export.pdf", cx))
                            .child(format_button(ExportFormat::Png, "export.png", cx))
                            .child(div().flex_1())
                            .child(label(t!("export.paper").to_string()))
                            .child(
                                Button::new("export-paper")
                                    .ghost()
                                    .compact()
                                    .label(paper_label.to_string())
                                    .disabled(dialog.busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_export_option(
                                            |options| {
                                                options.paper = match options.paper {
                                                    PaperSize::A4 => PaperSize::A5,
                                                    PaperSize::A5 => PaperSize::Letter,
                                                    PaperSize::Letter | PaperSize::Custom => {
                                                        PaperSize::A4
                                                    }
                                                }
                                            },
                                            cx,
                                        )
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_3()
                            .when(dialog.format == ExportFormat::Png, |row| {
                                row.child(
                                    Button::new("export-dpi")
                                        .ghost()
                                        .compact()
                                        .label(
                                            t!("export.dpi", dpi = dialog.options.png_dpi)
                                                .to_string(),
                                        )
                                        .disabled(dialog.busy)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.set_export_option(
                                                |options| {
                                                    options.png_dpi = match options.png_dpi {
                                                        0..=120 => 144,
                                                        121..=168 => 192,
                                                        169..=240 => 300,
                                                        _ => 96,
                                                    }
                                                },
                                                cx,
                                            )
                                        })),
                                )
                            })
                            .child(checkbox(
                                "export-cover",
                                "export.cover",
                                dialog.options.cover,
                                |options, value| options.cover = value,
                                cx,
                            ))
                            .child(checkbox(
                                "export-toc",
                                "export.toc",
                                dialog.options.toc,
                                |options, value| options.toc = value,
                                cx,
                            ))
                            .child(checkbox(
                                "export-page-numbers",
                                "export.page_numbers",
                                dialog.options.page_numbers,
                                |options, value| options.page_numbers = value,
                                cx,
                            ))
                            .child(checkbox(
                                "export-code-wrap",
                                "export.code_wrap",
                                dialog.options.code_wrap,
                                |options, value| options.code_wrap = value,
                                cx,
                            ))
                            .child(checkbox(
                                "export-code-highlight",
                                "export.code_highlight",
                                dialog.options.code_highlight,
                                |options, value| options.code_highlight = value,
                                cx,
                            ))
                            .when(dialog.format == ExportFormat::Png, |row| {
                                row.child(checkbox(
                                    "export-page-files",
                                    "export.page_files",
                                    dialog.options.png_page_files,
                                    |options, value| options.png_page_files = value,
                                    cx,
                                ))
                            }),
                    )
                    .child(
                        div()
                            .h_flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(cycle(
                                "export-margin",
                                t!("export.margin", mm = dialog.options.margin_top_mm as i32)
                                    .to_string(),
                                |options| {
                                    let next = match options.margin_top_mm as i32 {
                                        10 => 15.0,
                                        15 => 20.0,
                                        20 => 25.0,
                                        25 => 30.0,
                                        _ => 10.0,
                                    };
                                    options.margin_top_mm = next;
                                    options.margin_bottom_mm = next;
                                    options.margin_left_mm = next;
                                    options.margin_right_mm = next;
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-font-size",
                                t!(
                                    "export.font_size",
                                    pt = format_pt(dialog.options.font_size_pt)
                                )
                                .to_string(),
                                |options| {
                                    options.font_size_pt = match options.font_size_pt as i32 {
                                        9 => 10.0,
                                        10 => 11.0,
                                        11 => 12.0,
                                        12 => 13.0,
                                        13 => 14.0,
                                        _ => 9.0,
                                    };
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-line-height",
                                t!(
                                    "export.line_height",
                                    value = format!("{:.2}", dialog.options.line_height)
                                )
                                .to_string(),
                                |options| {
                                    options.line_height =
                                        match (options.line_height * 100.0).round() as i32 {
                                            140 => 1.5,
                                            150 => 1.65,
                                            165 => 1.8,
                                            180 => 2.0,
                                            _ => 1.4,
                                        };
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-toc-depth",
                                t!("export.toc_depth", depth = dialog.options.toc_depth)
                                    .to_string(),
                                |options| {
                                    options.toc_depth = match options.toc_depth {
                                        1 => 2,
                                        2 => 3,
                                        3 => 4,
                                        4 => 5,
                                        _ => 1,
                                    };
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-image-width",
                                t!(
                                    "export.image_width",
                                    pct = dialog.options.image_max_width_pct
                                )
                                .to_string(),
                                |options| {
                                    options.image_max_width_pct = match options.image_max_width_pct
                                    {
                                        60 => 75,
                                        61..=75 => 90,
                                        76..=90 => 100,
                                        _ => 60,
                                    };
                                },
                                cx,
                            ))
                            .child(cycle(
                                "export-page-format",
                                format!(
                                    "{}: {}",
                                    t!("export.page_format"),
                                    dialog.options.page_number_format
                                ),
                                |options| {
                                    options.page_number_format =
                                        match options.page_number_format.as_str() {
                                            "1" => "1 / 1".to_string(),
                                            "1 / 1" => "- 1 -".to_string(),
                                            "- 1 -" => "1 / 2".to_string(),
                                            _ => "1".to_string(),
                                        };
                                },
                                cx,
                            ))
                            .child(checkbox(
                                "export-justify",
                                "export.justify",
                                dialog.options.justify,
                                |options, value| options.justify = value,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("export.header").to_string()))
                            .child(Input::new(&dialog.header_input).w_full().small()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("export.footer").to_string()))
                            .child(Input::new(&dialog.footer_input).w_full().small()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("export.output").to_string()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&dialog.output_input).w_full().small()),
                            )
                            .child(
                                Button::new("export-browse")
                                    .ghost()
                                    .compact()
                                    .label(t!("export.browse").to_string())
                                    .disabled(dialog.busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_export_output(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .min_h(px(24.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .when_some(status, |element, (text, color)| {
                                        element.text_color(color).child(text)
                                    }),
                            )
                            .when(dialog.busy, |row| {
                                row.child(
                                    Button::new("export-cancel")
                                        .ghost()
                                        .compact()
                                        .label(t!("export.cancel").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_export_dialog(cx)
                                        })),
                                )
                            })
                            .when(!dialog.busy, |row| {
                                row.child(
                                    Button::new("export-dismiss")
                                        .ghost()
                                        .compact()
                                        .label(t!("export.close").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.close_export_dialog(cx)
                                        })),
                                )
                                .child(
                                    Button::new("export-start")
                                        .primary()
                                        .compact()
                                        .label(t!("export.start").to_string())
                                        .disabled(!can_start)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.begin_export(window, cx)
                                        })),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_zip_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.zip.as_ref() else {
            return div().into_any_element();
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let overlay = cx.theme().overlay;
        let danger = cx.theme().danger;
        let label = |text: String| {
            div()
                .w(px(64.))
                .flex_shrink_0()
                .text_xs()
                .text_color(muted)
                .child(text)
        };

        let title = match dialog.mode {
            ZipDialogMode::Export => t!("zip.export_title").to_string(),
            ZipDialogMode::Import => t!("zip.import_title").to_string(),
        };

        let import_mode_button = |mode: ImportMode, key: &'static str, cx: &mut Context<Self>| {
            let active = dialog.import_mode == mode;
            Button::new(format!("zip-mode-{key}"))
                .ghost()
                .compact()
                .label(t!(key).to_string())
                .when(active, |button| button.primary())
                .disabled(dialog.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.set_zip_import_mode(mode, cx)))
        };

        let policy_button = |policy: ConflictPolicy,
                             key: &'static str,
                             path: Option<String>,
                             cx: &mut Context<Self>| {
            let active = match &path {
                Some(path) => dialog.choices.get(path).copied().unwrap_or(dialog.policy) == policy,
                None => dialog.policy == policy,
            };
            let id = format!(
                "zip-policy-{key}-{policy:?}-{}",
                path.as_deref().unwrap_or("")
            );
            Button::new(id)
                .ghost()
                .compact()
                .label(t!(key).to_string())
                .when(active, |button| button.primary())
                .disabled(dialog.busy)
                .on_click(cx.listener(move |this, _, _, cx| match &path {
                    Some(path) => this.set_zip_choice(path, policy, cx),
                    None => this.set_zip_policy(policy, cx),
                }))
        };

        let mut body: Vec<AnyElement> = Vec::new();

        match dialog.mode {
            ZipDialogMode::Export => {
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("zip.notebook").to_string()))
                        .child(div().text_sm().child(dialog.notebook_name.clone()))
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("zip.file").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.path_input).w_full().small()),
                        )
                        .child(
                            Button::new("zip-export-browse")
                                .ghost()
                                .compact()
                                .label(t!("zip.browse").to_string())
                                .disabled(dialog.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_zip_export_path(window, cx)
                                })),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("zip.export_hint").to_string())
                        .into_any_element(),
                );
            }
            ZipDialogMode::Import => {
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("zip.file").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.path_input).w_full().small()),
                        )
                        .child(
                            Button::new("zip-import-browse")
                                .ghost()
                                .compact()
                                .label(t!("zip.browse").to_string())
                                .disabled(dialog.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_zip_path(window, cx)
                                })),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("zip.mode").to_string()))
                        .child(import_mode_button(ImportMode::Merge, "zip.mode.merge", cx))
                        .child(import_mode_button(
                            ImportMode::Overwrite,
                            "zip.mode.overwrite",
                            cx,
                        ))
                        .child(import_mode_button(
                            ImportMode::Rename,
                            "zip.mode.rename",
                            cx,
                        ))
                        .into_any_element(),
                );
                if dialog.import_mode == ImportMode::Rename {
                    body.push(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("zip.rename_to").to_string()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&dialog.rename_input).w_full().small()),
                            )
                            .into_any_element(),
                    );
                }

                let mut preview: Vec<AnyElement> = Vec::new();
                if dialog.plan_loading {
                    preview.push(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("zip.checking").to_string())
                            .into_any_element(),
                    );
                } else if let Some(error) = &dialog.plan_error {
                    preview.push(
                        div()
                            .text_xs()
                            .text_color(danger)
                            .child(t!("zip.preview_failed", message = error.clone()).to_string())
                            .into_any_element(),
                    );
                } else if let Some(plan) = &dialog.plan {
                    let folder_state = if plan.target_exists {
                        t!("zip.target_exists").to_string()
                    } else {
                        t!("zip.target_new").to_string()
                    };
                    preview.push(
                        div()
                            .h_flex()
                            .gap_2()
                            .text_xs()
                            .child(div().text_color(muted).child(t!("zip.target").to_string()))
                            .child(div().child(plan.folder_name.clone()))
                            .child(div().text_color(muted).child(folder_state))
                            .into_any_element(),
                    );
                    preview.push(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(
                                t!(
                                    "zip.summary",
                                    new = plan.new_files,
                                    same = plan.identical_files,
                                    conflicts = plan.conflicts.len(),
                                    size = format_bytes(plan.total_bytes)
                                )
                                .to_string(),
                            )
                            .into_any_element(),
                    );
                    if dialog.import_mode == ImportMode::Overwrite && plan.target_exists {
                        preview.push(
                            div()
                                .text_xs()
                                .text_color(danger)
                                .child(t!("zip.overwrite_warning").to_string())
                                .into_any_element(),
                        );
                    }
                    if dialog.import_mode == ImportMode::Merge && !plan.conflicts.is_empty() {
                        preview.push(
                            div()
                                .h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(t!("zip.default_policy").to_string()),
                                )
                                .child(policy_button(
                                    ConflictPolicy::KeepExisting,
                                    "zip.policy.keep_existing",
                                    None,
                                    cx,
                                ))
                                .child(policy_button(
                                    ConflictPolicy::UseImported,
                                    "zip.policy.use_imported",
                                    None,
                                    cx,
                                ))
                                .child(policy_button(
                                    ConflictPolicy::KeepBoth,
                                    "zip.policy.keep_both",
                                    None,
                                    cx,
                                ))
                                .into_any_element(),
                        );
                        let mut rows: Vec<AnyElement> = Vec::new();
                        for conflict in &plan.conflicts {
                            let Conflict { path, .. } = conflict;
                            rows.push(
                                div()
                                    .h_flex()
                                    .items_center()
                                    .gap_1()
                                    .py_1()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .child(path.clone()),
                                    )
                                    .child(policy_button(
                                        ConflictPolicy::KeepExisting,
                                        "zip.policy.short_keep",
                                        Some(path.clone()),
                                        cx,
                                    ))
                                    .child(policy_button(
                                        ConflictPolicy::UseImported,
                                        "zip.policy.short_use",
                                        Some(path.clone()),
                                        cx,
                                    ))
                                    .child(policy_button(
                                        ConflictPolicy::KeepBoth,
                                        "zip.policy.short_both",
                                        Some(path.clone()),
                                        cx,
                                    ))
                                    .into_any_element(),
                            );
                        }
                        preview.push(
                            div()
                                .v_flex()
                                .max_h(px(150.))
                                .id("zip-conflicts")
                                .overflow_y_scroll()
                                .gap_1()
                                .children(rows)
                                .into_any_element(),
                        );
                    }
                }
                body.push(
                    div()
                        .v_flex()
                        .gap_1()
                        .w_full()
                        .p_2()
                        .rounded_md()
                        .bg(cx.theme().list_hover)
                        .children(preview)
                        .into_any_element(),
                );
            }
        }

        let status = if let Some(error) = &dialog.error {
            Some((error.clone(), danger))
        } else if dialog.busy {
            Some((
                match dialog.mode {
                    ZipDialogMode::Export => t!("zip.exporting").to_string(),
                    ZipDialogMode::Import => t!("zip.importing").to_string(),
                },
                muted,
            ))
        } else {
            None
        };

        let can_start = !dialog.busy
            && match dialog.mode {
                ZipDialogMode::Export => !dialog.path_input.read(cx).value().trim().is_empty(),
                ZipDialogMode::Import => dialog.plan.is_some(),
            };
        let dialog_mode = dialog.mode;
        let start_label = match dialog_mode {
            ZipDialogMode::Export => t!("zip.export_start").to_string(),
            ZipDialogMode::Import => match dialog.import_mode {
                ImportMode::Merge => t!("zip.import_merge").to_string(),
                ImportMode::Overwrite if dialog.confirm_overwrite => {
                    t!("zip.confirm_overwrite").to_string()
                }
                ImportMode::Overwrite => t!("zip.import_overwrite").to_string(),
                ImportMode::Rename => t!("zip.import_rename").to_string(),
            },
        };

        let mut footer = div().h_flex().items_center().gap_2().min_h(px(24.)).child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .when_some(status, |element, (text, color)| {
                    element.text_color(color).child(text)
                }),
        );
        if dialog.busy {
            footer = footer.child(
                Button::new("zip-cancel")
                    .ghost()
                    .compact()
                    .label(t!("export.cancel").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.close_zip_dialog(cx))),
            );
        } else {
            let start_mode = dialog_mode;
            footer = footer
                .child(
                    Button::new("zip-dismiss")
                        .ghost()
                        .compact()
                        .label(t!("export.close").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.close_zip_dialog(cx))),
                )
                .child(
                    Button::new("zip-start")
                        .primary()
                        .compact()
                        .label(start_label)
                        .disabled(!can_start)
                        .on_click(cx.listener(move |this, _, window, cx| match start_mode {
                            ZipDialogMode::Export => this.begin_zip_export(cx),
                            ZipDialogMode::Import => this.begin_zip_import(window, cx),
                        })),
                );
        }

        div()
            .id("zip-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .child(
                div()
                    .id("zip-dialog")
                    .v_flex()
                    .w(px(660.))
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().text_base().font_bold().child(title))
                            .child(
                                Button::new("zip-close")
                                    .ghost()
                                    .compact()
                                    .label("×")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.close_zip_dialog(cx)),
                                    ),
                            ),
                    )
                    .children(body)
                    .child(footer),
            )
            .into_any_element()
    }

    fn open_image_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.image_settings.is_some() {
            return;
        }
        let (mode, picgo, http_endpoint, field, url_path, headers) = match &self.config.image_host {
            ImageHost::Local => (
                ImageHostMode::Local,
                image::default_picgo_endpoint(),
                String::new(),
                image::default_field(),
                image::default_url_path(),
                String::new(),
            ),
            ImageHost::PicGo { endpoint } => (
                ImageHostMode::PicGo,
                endpoint.clone(),
                String::new(),
                image::default_field(),
                image::default_url_path(),
                String::new(),
            ),
            ImageHost::Http {
                endpoint,
                field,
                url_path,
                headers,
            } => (
                ImageHostMode::Http,
                image::default_picgo_endpoint(),
                endpoint.clone(),
                field.clone(),
                url_path.clone(),
                headers.clone(),
            ),
        };
        let picgo_endpoint = cx.new(|cx| InputState::new(window, cx).default_value(picgo));
        let http_endpoint = cx.new(|cx| InputState::new(window, cx).default_value(http_endpoint));
        let http_field = cx.new(|cx| InputState::new(window, cx).default_value(field));
        let http_url_path = cx.new(|cx| InputState::new(window, cx).default_value(url_path));
        let http_headers = cx.new(|cx| TextareaState::new(window, cx).default_value(headers));
        self.image_settings = Some(ImageSettingsDialog {
            mode,
            picgo_endpoint,
            http_endpoint,
            http_field,
            http_url_path,
            http_headers,
            error: None,
        });
        cx.notify();
    }

    fn set_image_host_mode(&mut self, mode: ImageHostMode, cx: &mut Context<Self>) {
        let Some(dialog) = self.image_settings.as_mut() else {
            return;
        };
        if dialog.mode == mode {
            return;
        }
        dialog.mode = mode;
        dialog.error = None;
        cx.notify();
    }

    fn save_image_settings(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.image_settings.as_ref() else {
            return;
        };
        let host = match dialog.mode {
            ImageHostMode::Local => ImageHost::Local,
            ImageHostMode::PicGo => ImageHost::PicGo {
                endpoint: dialog.picgo_endpoint.read(cx).value().trim().to_string(),
            },
            ImageHostMode::Http => ImageHost::Http {
                endpoint: dialog.http_endpoint.read(cx).value().trim().to_string(),
                field: dialog.http_field.read(cx).value().trim().to_string(),
                url_path: dialog.http_url_path.read(cx).value().trim().to_string(),
                headers: dialog.http_headers.read(cx).value().to_string(),
            },
        };
        if let Err(err) = image::validate_host(&host) {
            if let Some(dialog) = self.image_settings.as_mut() {
                dialog.error = Some(err.to_string());
            }
            cx.notify();
            return;
        }
        self.config.image_host = host;
        match self.config.save() {
            Ok(()) => {
                self.image_settings = None;
            }
            Err(err) => {
                if let Some(dialog) = self.image_settings.as_mut() {
                    dialog.error = Some(format!("保存设置失败：{err}"));
                }
            }
        }
        cx.notify();
    }

    fn render_image_settings_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.image_settings.as_ref() else {
            return empty_hint(t!("image.loading").to_string(), cx.theme().muted_foreground);
        };
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let label = |text: String| {
            div()
                .w(px(96.))
                .flex_shrink_0()
                .text_xs()
                .text_color(muted)
                .child(text)
        };
        let mode_button = |mode: ImageHostMode, key: &'static str, cx: &mut Context<Self>| {
            let active = dialog.mode == mode;
            Button::new(match mode {
                ImageHostMode::Local => "image-mode-local",
                ImageHostMode::PicGo => "image-mode-picgo",
                ImageHostMode::Http => "image-mode-http",
            })
            .ghost()
            .compact()
            .label(t!(key).to_string())
            .when(active, |button| button.primary())
            .on_click(cx.listener(move |this, _, _, cx| this.set_image_host_mode(mode, cx)))
        };

        let mut body: Vec<AnyElement> = Vec::new();
        body.push(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(label(t!("image.mode").to_string()))
                .child(mode_button(ImageHostMode::Local, "image.mode.local", cx))
                .child(mode_button(ImageHostMode::PicGo, "image.mode.picgo", cx))
                .child(mode_button(ImageHostMode::Http, "image.mode.http", cx))
                .into_any_element(),
        );
        match dialog.mode {
            ImageHostMode::Local => {
                body.push(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("image.local_hint").to_string())
                        .into_any_element(),
                );
            }
            ImageHostMode::PicGo => {
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("image.endpoint").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.picgo_endpoint).w_full().small()),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("image.picgo_hint").to_string())
                        .into_any_element(),
                );
            }
            ImageHostMode::Http => {
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("image.endpoint").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.http_endpoint).w_full().small()),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("image.field").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.http_field).w_full().small()),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("image.url_path").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.http_url_path).w_full().small()),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_start()
                        .gap_2()
                        .child(label(t!("image.headers").to_string()))
                        .child(
                            div()
                                .v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(Textarea::new(&dialog.http_headers).h(px(64.)).w_full())
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(t!("image.headers_hint").to_string()),
                                ),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("image.http_hint").to_string())
                        .into_any_element(),
                );
            }
        }

        let mut footer = div().h_flex().items_center().gap_2().min_h(px(24.)).child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(danger)
                .when_some(dialog.error.clone(), |element, error| element.child(error)),
        );
        footer = footer.child(
            Button::new("image-save")
                .primary()
                .compact()
                .label(t!("image.save").to_string())
                .on_click(cx.listener(|this, _, _, cx| this.save_image_settings(cx))),
        );

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .children(body)
            .child(footer)
            .into_any_element()
    }

    fn mcp_tab_button(&self, tab: McpTab, key: &'static str, cx: &mut Context<Self>) -> Button {
        let active = self
            .mcp_settings
            .as_ref()
            .is_some_and(|dialog| dialog.tab == tab);
        Button::new(match tab {
            McpTab::Server => "mcp-tab-server",
            McpTab::Tokens => "mcp-tab-tokens",
            McpTab::Audit => "mcp-tab-audit",
        })
        .ghost()
        .compact()
        .label(t!(key).to_string())
        .when(active, |button| button.primary())
        .on_click(cx.listener(move |this, _, _, cx| this.set_mcp_tab(tab, cx)))
    }

    fn render_mcp_settings_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return empty_hint(t!("mcp.loading").to_string(), cx.theme().muted_foreground);
        };
        let tab = dialog.tab;
        let has_error = dialog.error.is_some();
        let message = dialog.error.clone().or_else(|| dialog.message.clone());
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;

        let body = match tab {
            McpTab::Server => self.render_mcp_server_tab(cx),
            McpTab::Tokens => self.render_mcp_tokens_tab(cx),
            McpTab::Audit => self.render_mcp_audit_tab(cx),
        };

        let message_color = if has_error { danger } else { muted };
        let footer = div().h_flex().items_center().gap_2().min_h(px(24.)).child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(message_color)
                .when_some(message, |element, text| element.child(text)),
        );

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(self.mcp_tab_button(McpTab::Server, "mcp.tab.server", cx))
                    .child(self.mcp_tab_button(McpTab::Tokens, "mcp.tab.tokens", cx))
                    .child(self.mcp_tab_button(McpTab::Audit, "mcp.tab.audit", cx)),
            )
            .child(body)
            .child(footer)
            .into_any_element()
    }

    fn render_mcp_server_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let running = self.mcp.running();
        let mcp_error = self.mcp.error.clone();
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().popover_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;

        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            Checkbox::new("mcp-enabled")
                .label(t!("mcp.enable").to_string())
                .checked(dialog.enabled)
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    if let Some(dialog) = this.mcp_settings.as_mut() {
                        dialog.enabled = *checked;
                        dialog.message = None;
                        dialog.error = None;
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        rows.push(
            div()
                .text_xs()
                .text_color(muted)
                .child(t!("mcp.enable_hint").to_string())
                .into_any_element(),
        );
        rows.push(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(96.))
                        .text_xs()
                        .text_color(muted)
                        .child(t!("mcp.port").to_string()),
                )
                .child(
                    div()
                        .w(px(110.))
                        .child(Input::new(&dialog.port_input).w_full().small()),
                )
                .child(
                    Button::new("mcp-apply")
                        .primary()
                        .compact()
                        .label(t!("mcp.apply").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.apply_mcp_server(cx))),
                )
                .into_any_element(),
        );
        let status_text = if running {
            t!("mcp.running", port = self.mcp.port).to_string()
        } else {
            t!("mcp.stopped").to_string()
        };
        rows.push(
            div()
                .text_xs()
                .text_color(if running { foreground } else { muted })
                .child(status_text)
                .into_any_element(),
        );
        if !running {
            if let Some(error) = mcp_error {
                rows.push(
                    div()
                        .text_xs()
                        .text_color(danger)
                        .child(error)
                        .into_any_element(),
                );
            }
        }

        let cli = cli_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "md-work-cli.exe".to_string());
        let port = self.config.mcp_port.max(1);
        let sample_token = dialog
            .secret
            .clone()
            .unwrap_or_else(|| t!("mcp.token_placeholder").to_string());
        let snippet = serde_json::json!({
            "mcpServers": {
                "md-work": {
                    "command": cli,
                    "args": ["mcp", "serve", "--stdio"],
                    "env": { "MDWORK_TOKEN": sample_token }
                }
            }
        });
        let snippet = serde_json::to_string_pretty(&snippet).unwrap_or_default();
        rows.push(
            div()
                .text_xs()
                .font_bold()
                .child(t!("mcp.clients").to_string())
                .into_any_element(),
        );
        rows.push(
            div()
                .text_xs()
                .text_color(muted)
                .child(
                    t!(
                        "mcp.snippet_hint",
                        url = format!("http://127.0.0.1:{port}/mcp")
                    )
                    .to_string(),
                )
                .into_any_element(),
        );
        let snippet_copy = snippet.clone();
        rows.push(
            div()
                .v_flex()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .child(div().font_family("Consolas").text_xs().child(snippet))
                .child(
                    Button::new("mcp-copy-snippet")
                        .ghost()
                        .compact()
                        .label(t!("mcp.copy").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.copy_mcp_text(snippet_copy.clone(), cx)
                        })),
                )
                .into_any_element(),
        );

        div()
            .id("mcp-server-body")
            .v_flex()
            .gap_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }

    fn render_mcp_tokens_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let list = self.render_mcp_token_list(cx);
        let editor = self.render_mcp_token_editor(cx);
        div()
            .h_flex()
            .gap_3()
            .flex_1()
            .min_h_0()
            .child(list)
            .child(editor)
            .into_any_element()
    }

    fn render_mcp_token_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().list_hover;
        let active_bg = cx.theme().list_active;
        let transparent = cx.theme().transparent;
        let mut rows: Vec<AnyElement> = Vec::new();
        if dialog.tokens.is_empty() {
            rows.push(empty_hint(t!("mcp.no_tokens").to_string(), muted));
        }
        for record in &dialog.tokens {
            let id = record.id;
            let active = dialog.selected == Some(id);
            let status = if record.revoked {
                t!("mcp.status.revoked").to_string()
            } else if !record.is_active(perms::now_unix()) {
                t!("mcp.status.expired").to_string()
            } else {
                t!("mcp.status.active").to_string()
            };
            let scopes_label = record
                .scopes
                .iter()
                .map(|scope| scope_label(*scope))
                .collect::<Vec<_>>()
                .join(" · ");
            rows.push(
                div()
                    .id(format!("mcp-token-{id}"))
                    .v_flex()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if active { active_bg } else { transparent })
                    .hover(move |style| style.bg(hover))
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.select_mcp_token(id, window, cx)
                        }),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(record.name.clone()),
                            )
                            .child(div().text_xs().text_color(muted).child(status)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(scopes_label),
                    )
                    .into_any_element(),
            );
        }

        div()
            .id("mcp-token-list")
            .w(px(230.))
            .min_h_0()
            .v_flex()
            .gap_1()
            .overflow_y_scroll()
            .child(
                Button::new("mcp-new-token")
                    .compact()
                    .label(t!("mcp.create").to_string())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.begin_new_mcp_token(window, cx)),
                    ),
            )
            .children(rows)
            .into_any_element()
    }

    fn render_mcp_token_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;
        let editing = dialog.creating || dialog.selected.is_some();
        let mut editor: Vec<AnyElement> = Vec::new();
        if !editing {
            editor.push(empty_hint(t!("mcp.select_or_new").to_string(), muted));
        } else {
            editor.push(mcp_field(
                t!("mcp.name").to_string(),
                Input::new(&dialog.name_input)
                    .w_full()
                    .small()
                    .into_any_element(),
                muted,
            ));
            let mut scope_row = div().h_flex().flex_wrap().gap_3();
            for scope in Scope::ALL {
                let checked = dialog.scopes.contains(&scope);
                scope_row = scope_row.child(
                    Checkbox::new(format!("mcp-scope-{}", scope.as_str()))
                        .label(scope_label(scope))
                        .checked(checked)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.toggle_mcp_scope(scope, *checked, cx)
                        })),
                );
            }
            editor.push(mcp_field(
                t!("mcp.scopes").to_string(),
                scope_row.into_any_element(),
                muted,
            ));

            let known: HashSet<Uuid> = self
                .notebooks
                .iter()
                .map(|entry| entry.notebook.marker.id)
                .collect();
            let mut notebook_rows = div().v_flex().gap_1();
            for entry in &self.notebooks {
                let id = entry.notebook.marker.id;
                let checked = dialog.notebooks.contains(&id);
                notebook_rows = notebook_rows.child(
                    Checkbox::new(format!("mcp-notebook-{id}"))
                        .label(entry.notebook.display_name().to_string())
                        .checked(checked)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.toggle_mcp_notebook(id, *checked, cx)
                        })),
                );
            }
            for id in dialog.notebooks.iter().filter(|id| !known.contains(id)) {
                let id = *id;
                notebook_rows = notebook_rows.child(
                    Checkbox::new(format!("mcp-notebook-{id}"))
                        .label(t!("mcp.unknown_notebook", id = id.to_string()).to_string())
                        .checked(true)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.toggle_mcp_notebook(id, *checked, cx)
                        })),
                );
            }
            editor.push(mcp_field(
                t!("mcp.notebooks").to_string(),
                notebook_rows.into_any_element(),
                muted,
            ));
            editor.push(mcp_field(
                t!("mcp.allow").to_string(),
                Input::new(&dialog.allow_input)
                    .w_full()
                    .small()
                    .into_any_element(),
                muted,
            ));
            editor.push(mcp_field(
                t!("mcp.deny").to_string(),
                Input::new(&dialog.deny_input)
                    .w_full()
                    .small()
                    .into_any_element(),
                muted,
            ));
            editor.push(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("mcp.globs_hint").to_string())
                    .into_any_element(),
            );

            if let Some(secret) = dialog.secret.clone() {
                let copy_value = secret.clone();
                editor.push(
                    div()
                        .v_flex()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .child(
                            div()
                                .h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .font_bold()
                                        .child(t!("mcp.secret_title").to_string()),
                                )
                                .child(
                                    Button::new("mcp-copy-secret")
                                        .ghost()
                                        .compact()
                                        .label(t!("mcp.copy").to_string())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.copy_mcp_text(copy_value.clone(), cx)
                                        })),
                                ),
                        )
                        .child(div().font_family("Consolas").text_xs().child(secret))
                        .child(
                            div()
                                .text_xs()
                                .text_color(danger)
                                .child(t!("mcp.secret_warning").to_string()),
                        )
                        .into_any_element(),
                );
            }

            let has_selected = dialog.selected.is_some();
            let selected_revoked = dialog
                .tokens
                .iter()
                .find(|record| Some(record.id) == dialog.selected)
                .is_some_and(|record| record.revoked);
            let mut actions = div().h_flex().gap_2();
            actions = actions.child(
                Button::new("mcp-save-token")
                    .primary()
                    .compact()
                    .label(t!("mcp.save").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.save_mcp_token(cx))),
            );
            if has_selected {
                let rotate_label = if dialog.pending_rotate {
                    t!("mcp.confirm_rotate").to_string()
                } else {
                    t!("mcp.rotate").to_string()
                };
                actions = actions.child(
                    Button::new("mcp-rotate-token")
                        .ghost()
                        .compact()
                        .disabled(selected_revoked)
                        .label(rotate_label)
                        .on_click(cx.listener(|this, _, _, cx| this.rotate_mcp_token(cx))),
                );
                let revoke_label = if dialog.pending_revoke {
                    t!("mcp.confirm_revoke").to_string()
                } else {
                    t!("mcp.revoke").to_string()
                };
                actions = actions.child(
                    Button::new("mcp-revoke-token")
                        .ghost()
                        .compact()
                        .disabled(selected_revoked)
                        .label(revoke_label)
                        .on_click(cx.listener(|this, _, _, cx| this.revoke_mcp_token(cx))),
                );
                let delete_label = if dialog.pending_delete {
                    t!("mcp.confirm_delete").to_string()
                } else {
                    t!("mcp.delete").to_string()
                };
                actions = actions.child(
                    Button::new("mcp-delete-token")
                        .ghost()
                        .compact()
                        .label(delete_label)
                        .on_click(cx.listener(|this, _, _, cx| this.delete_mcp_token(cx))),
                );
            }
            editor.push(actions.into_any_element());
        }

        div()
            .id("mcp-token-editor")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .v_flex()
            .gap_2()
            .overflow_y_scroll()
            .children(editor)
            .into_any_element()
    }

    fn render_mcp_audit_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("mcp.audit_hint").to_string()),
                )
                .child(
                    Button::new("mcp-audit-refresh")
                        .ghost()
                        .compact()
                        .label(t!("mcp.audit_refresh").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_mcp_audit(cx))),
                )
                .into_any_element(),
        );
        if let Some(error) = dialog.audit_error.clone() {
            rows.push(
                div()
                    .text_xs()
                    .text_color(danger)
                    .child(error)
                    .into_any_element(),
            );
        }
        if dialog.audit.is_empty() && dialog.audit_error.is_none() {
            rows.push(empty_hint(t!("mcp.audit_empty").to_string(), muted));
        }
        for entry in &dialog.audit {
            let when = age_label(entry.time);
            let result_color = if entry.result == "ok" { muted } else { danger };
            let target = entry.path.clone().or_else(|| {
                entry
                    .query
                    .as_deref()
                    .map(|query| t!("mcp.audit_query", query = query).to_string())
            });
            let location = match (&entry.notebook_name, target) {
                (Some(notebook), Some(target)) => format!("{notebook} · {target}"),
                (Some(notebook), None) => notebook.clone(),
                (None, Some(target)) => target,
                (None, None) => String::new(),
            };
            rows.push(
                div()
                    .v_flex()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(muted).child(when))
                            .child(div().text_xs().font_bold().child(entry.tool.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(entry.token_name.clone()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(result_color)
                                    .child(entry.result.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .child(format!("{location} · {} ms", entry.duration_ms)),
                    )
                    .into_any_element(),
            );
        }
        div()
            .id("mcp-audit-body")
            .v_flex()
            .gap_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }

    fn open_mcp_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let port = self.config.mcp_port.to_string();
        let enabled = self.config.mcp_http;
        let port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(port)
                .placeholder("11026")
        });
        let name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("mcp.name_placeholder").to_string())
        });
        let allow_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("mcp.allow_placeholder").to_string())
        });
        let deny_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("mcp.deny_placeholder").to_string())
        });
        let (tokens, error) = match token_store().list() {
            Ok(tokens) => (tokens, None),
            Err(err) => (
                Vec::new(),
                Some(t!("mcp.token_error", message = err.to_string()).to_string()),
            ),
        };
        let (audit, audit_error) = load_audit_entries();
        self.mcp_settings = Some(McpSettingsDialog {
            tab: McpTab::Server,
            enabled,
            port_input,
            tokens,
            selected: None,
            creating: false,
            name_input,
            allow_input,
            deny_input,
            scopes: Vec::new(),
            notebooks: HashSet::new(),
            secret: None,
            pending_rotate: false,
            pending_revoke: false,
            pending_delete: false,
            audit,
            audit_error,
            message: None,
            error,
        });
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        self.open_image_settings(window, cx);
        self.open_mcp_settings(window, cx);
        let ui_font = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(self.config.ui_font.clone())
                .placeholder("Segoe UI / Microsoft YaHei UI")
        });
        let note_font = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(self.config.note_font.clone())
                .placeholder("Consolas / 霞鹜文楷")
        });
        let ui_size = cx.new(|cx| {
            InputState::new(window, cx).default_value(format!("{:.0}", self.config.ui_font_size))
        });
        let note_size = cx.new(|cx| {
            InputState::new(window, cx).default_value(format!("{:.0}", self.config.note_font_size))
        });
        self.settings = Some(SettingsDialog {
            tab: SettingsTab::Appearance,
            ui_font,
            note_font,
            ui_size,
            note_size,
        });
        cx.notify();
    }

    /// Copies the font fields into the config, applies them and persists.
    fn apply_typography(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings.as_ref() else {
            return;
        };
        let ui_font = dialog.ui_font.read(cx).value().trim().to_string();
        let note_font = dialog.note_font.read(cx).value().trim().to_string();
        let ui_size = parse_font_size(&dialog.ui_size.read(cx).value(), self.config.ui_font_size);
        let note_size = parse_font_size(
            &dialog.note_size.read(cx).value(),
            self.config.note_font_size,
        );
        if self.config.ui_font == ui_font
            && self.config.note_font == note_font
            && self.config.ui_font_size == ui_size
            && self.config.note_font_size == note_size
        {
            return;
        }
        self.config.ui_font = ui_font;
        self.config.note_font = note_font;
        self.config.ui_font_size = ui_size;
        self.config.note_font_size = note_size;
        theme::apply(&self.config, window, cx);
        if let Err(err) = self.config.save() {
            self.notice = Some(format!("保存设置失败：{err}"));
        }
        cx.refresh_windows();
    }

    fn set_autosave(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.config.autosave == enabled {
            return;
        }
        self.config.autosave = enabled;
        if let Err(err) = self.config.save() {
            self.notice = Some(format!("保存设置失败：{err}"));
        }
        for doc in &self.docs {
            doc.update(cx, |doc, cx| doc.set_autosave(enabled, cx));
        }
        cx.notify();
    }

    fn register_file_assoc(&mut self, cx: &mut Context<Self>) {
        match crate::file_assoc::ensure_registered() {
            Ok(()) => {
                self.notice = Some(t!("settings.windows_register_ok").to_string());
                tracing::info!("已注册「用墨笺打开」右键菜单");
            }
            Err(err) => {
                self.notice = Some(format!("{}: {err}", t!("settings.windows_register_failed")));
                tracing::warn!(error = %err, "注册文件关联失败");
            }
        }
        cx.notify();
    }

    fn open_windows_default_apps(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = std::process::Command::new("explorer.exe")
            .arg("ms-settings:defaultapps")
            .spawn()
        {
            self.notice = Some(format!(
                "{}: {err}",
                t!("settings.windows_default_apps_failed")
            ));
        }
        cx.notify();
    }

    fn open_path_in_explorer(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(path) = path else {
            return;
        };
        if !path.exists() {
            let _ = std::fs::create_dir_all(&path);
        }
        if let Err(err) = std::process::Command::new("explorer.exe")
            .arg(&path)
            .spawn()
        {
            self.notice = Some(format!(
                "{}: {err}",
                t!("settings.windows_open_folder_failed")
            ));
        }
        cx.notify();
    }

    fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_typography(window, cx);
        self.settings = None;
        self.image_settings = None;
        self.mcp_settings = None;
        cx.notify();
    }

    fn set_settings_tab(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        if let Some(dialog) = self.settings.as_mut() {
            if dialog.tab == tab {
                return;
            }
            dialog.tab = tab;
        }
        cx.notify();
    }

    fn set_theme_preference(
        &mut self,
        preference: ThemePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.config.theme == preference {
            return;
        }
        let previous = self.config.theme;
        self.config.theme = preference;
        theme::apply(&self.config, window, cx);
        if let Err(err) = self.config.save() {
            self.notice = Some(format!("保存设置失败：{err}"));
        }
        tracing::info!(from = ?previous, to = ?preference, "切换主题");
        cx.notify();
    }

    fn set_language(&mut self, language: &str, cx: &mut Context<Self>) {
        if self.config.language == language {
            return;
        }
        let previous = self.config.language.clone();
        self.config.language = language.to_string();
        rust_i18n::set_locale(&self.config.language);
        if let Err(err) = self.config.save() {
            self.notice = Some(format!("保存设置失败：{err}"));
        }
        tracing::info!(from = %previous, to = %self.config.language, "切换语言");
        cx.refresh_windows();
    }

    fn set_panel_dock(&mut self, dock: PanelDock, cx: &mut Context<Self>) {
        if self.config.panel_dock == dock {
            return;
        }
        self.config.panel_dock = dock;
        if let Err(err) = self.config.save() {
            self.notice = Some(format!("保存设置失败：{err}"));
        }
        cx.notify();
    }

    fn set_panel_arrangement(&mut self, arrangement: PanelArrangement, cx: &mut Context<Self>) {
        if self.config.panel_arrangement == arrangement {
            return;
        }
        self.config.panel_arrangement = arrangement;
        if let Err(err) = self.config.save() {
            self.notice = Some(format!("保存设置失败：{err}"));
        }
        cx.notify();
    }

    fn set_panel_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
        if (self.config.panel_width - width).abs() < 0.5 {
            return;
        }
        self.config.panel_width = width;
        if let Err(err) = self.config.save() {
            self.notice = Some(format!("保存设置失败：{err}"));
        }
        cx.notify();
    }

    fn begin_panel_resize(&mut self, mouse_x: f32, cx: &mut Context<Self>) {
        self.panel_resize = Some((mouse_x, self.config.panel_width));
        cx.notify();
    }

    fn update_panel_resize(&mut self, mouse_x: f32, cx: &mut Context<Self>) {
        let Some((origin, start_width)) = self.panel_resize else {
            return;
        };
        let delta = if self.config.panel_dock == PanelDock::Right {
            origin - mouse_x
        } else {
            mouse_x - origin
        };
        let width = (start_width + delta).clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
        if (self.config.panel_width - width).abs() < 0.5 {
            return;
        }
        self.config.panel_width = width;
        cx.notify();
    }

    fn end_panel_resize(&mut self, cx: &mut Context<Self>) {
        if self.panel_resize.take().is_none() {
            return;
        }
        self.config.panel_width = self.config.panel_width.round();
        if let Err(err) = self.config.save() {
            self.notice = Some(format!("保存设置失败：{err}"));
        }
        cx.notify();
    }

    fn set_mcp_tab(&mut self, tab: McpTab, cx: &mut Context<Self>) {
        if let Some(dialog) = self.mcp_settings.as_mut() {
            if dialog.tab == tab {
                return;
            }
            dialog.tab = tab;
            dialog.message = None;
            dialog.error = None;
        }
        cx.notify();
    }

    fn reload_mcp_tokens(dialog: &mut McpSettingsDialog) {
        match token_store().list() {
            Ok(tokens) => {
                dialog.tokens = tokens;
                if let Some(id) = dialog.selected {
                    if !dialog.tokens.iter().any(|record| record.id == id) {
                        dialog.selected = None;
                        dialog.creating = false;
                        dialog.secret = None;
                    }
                }
                dialog.error = None;
            }
            Err(err) => {
                dialog.error = Some(t!("mcp.token_error", message = err.to_string()).to_string());
            }
        }
    }

    fn select_mcp_token(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.mcp_settings.as_mut() else {
            return;
        };
        let Some(record) = dialog.tokens.iter().find(|record| record.id == id).cloned() else {
            return;
        };
        dialog.selected = Some(id);
        dialog.creating = false;
        dialog.secret = None;
        dialog.pending_rotate = false;
        dialog.pending_revoke = false;
        dialog.pending_delete = false;
        dialog.message = None;
        dialog.error = None;
        dialog.scopes = record.scopes.clone();
        dialog.notebooks = record.notebooks.iter().copied().collect();
        let allow = record.note_allow.join(", ");
        let deny = record.note_deny.join(", ");
        dialog.name_input.update(cx, |state, cx| {
            state.set_value(record.name.clone(), window, cx)
        });
        dialog
            .allow_input
            .update(cx, |state, cx| state.set_value(allow, window, cx));
        dialog
            .deny_input
            .update(cx, |state, cx| state.set_value(deny, window, cx));
        cx.notify();
    }

    fn begin_new_mcp_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_notebook;
        let Some(dialog) = self.mcp_settings.as_mut() else {
            return;
        };
        dialog.selected = None;
        dialog.creating = true;
        dialog.secret = None;
        dialog.pending_rotate = false;
        dialog.pending_revoke = false;
        dialog.pending_delete = false;
        dialog.message = None;
        dialog.error = None;
        dialog.scopes = vec![Scope::Read, Scope::Search];
        dialog.notebooks = active.into_iter().collect();
        for input in [&dialog.name_input, &dialog.allow_input, &dialog.deny_input] {
            input.update(cx, |state, cx| state.set_value(String::new(), window, cx));
        }
        cx.notify();
    }

    fn save_mcp_token(&mut self, cx: &mut Context<Self>) {
        let Some((name, allow, deny, scopes, notebooks, creating, selected)) =
            self.mcp_settings.as_ref().map(|dialog| {
                (
                    dialog.name_input.read(cx).value().trim().to_string(),
                    split_globs(&dialog.allow_input.read(cx).value()),
                    split_globs(&dialog.deny_input.read(cx).value()),
                    Scope::ALL
                        .into_iter()
                        .filter(|scope| dialog.scopes.contains(scope))
                        .collect::<Vec<_>>(),
                    dialog.notebooks.iter().copied().collect::<Vec<_>>(),
                    dialog.creating,
                    dialog.selected,
                )
            })
        else {
            return;
        };

        let validation = if name.is_empty() {
            Some(t!("mcp.err.name").to_string())
        } else if scopes.is_empty() {
            Some(t!("mcp.err.scopes").to_string())
        } else if notebooks.is_empty() {
            Some(t!("mcp.err.notebooks").to_string())
        } else {
            None
        };
        if let Some(message) = validation {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.error = Some(message);
                dialog.message = None;
            }
            cx.notify();
            return;
        }

        let store = token_store();
        let outcome: Result<String, String> = if creating {
            store
                .create(&name, scopes, notebooks, allow, deny)
                .map(|(record, wire)| {
                    if let Some(dialog) = self.mcp_settings.as_mut() {
                        dialog.selected = Some(record.id);
                        dialog.creating = false;
                        dialog.secret = Some(wire);
                        dialog.pending_rotate = false;
                        dialog.pending_revoke = false;
                        dialog.pending_delete = false;
                    }
                    t!("mcp.created").to_string()
                })
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())
        } else if let Some(id) = selected {
            store
                .list()
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())
                .and_then(|list| {
                    list.into_iter()
                        .find(|record| record.id == id)
                        .ok_or_else(|| t!("mcp.err.select").to_string())
                })
                .and_then(|mut record| {
                    record.name = name;
                    record.scopes = scopes;
                    record.notebooks = notebooks;
                    record.note_allow = allow;
                    record.note_deny = deny;
                    store
                        .update(record)
                        .map(|()| t!("mcp.saved").to_string())
                        .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())
                })
        } else {
            Err(t!("mcp.err.select").to_string())
        };

        if let Some(dialog) = self.mcp_settings.as_mut() {
            match outcome {
                Ok(message) => {
                    Self::reload_mcp_tokens(dialog);
                    dialog.message = Some(message);
                    dialog.error = None;
                }
                Err(error) => {
                    dialog.error = Some(error);
                    dialog.message = None;
                }
            }
        }
        cx.notify();
    }

    fn rotate_mcp_token(&mut self, cx: &mut Context<Self>) {
        let Some((id, armed)) = self
            .mcp_settings
            .as_ref()
            .and_then(|dialog| dialog.selected.map(|id| (id, dialog.pending_rotate)))
        else {
            return;
        };
        if !armed {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.pending_rotate = true;
                dialog.message = None;
                dialog.error = None;
            }
            cx.notify();
            return;
        }
        let store = token_store();
        let outcome: Result<String, String> = store
            .list()
            .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())
            .and_then(|list| {
                list.into_iter()
                    .find(|record| record.id == id)
                    .ok_or_else(|| t!("mcp.err.select").to_string())
            })
            .and_then(|mut record| {
                let secret = perms::new_secret();
                record.hash = perms::hash_secret(&secret);
                let wire = perms::format_token(&record.id, &secret);
                store
                    .update(record)
                    .map(|()| wire)
                    .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())
            });
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.pending_rotate = false;
            match outcome {
                Ok(wire) => {
                    Self::reload_mcp_tokens(dialog);
                    dialog.secret = Some(wire);
                    dialog.message = Some(t!("mcp.rotated").to_string());
                    dialog.error = None;
                }
                Err(error) => {
                    dialog.error = Some(error);
                    dialog.message = None;
                }
            }
        }
        cx.notify();
    }

    fn revoke_mcp_token(&mut self, cx: &mut Context<Self>) {
        let Some((id, armed)) = self
            .mcp_settings
            .as_ref()
            .and_then(|dialog| dialog.selected.map(|id| (id, dialog.pending_revoke)))
        else {
            return;
        };
        if !armed {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.pending_revoke = true;
                dialog.message = None;
                dialog.error = None;
            }
            cx.notify();
            return;
        }
        let outcome = token_store()
            .revoke(&id)
            .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string());
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.pending_revoke = false;
            match outcome {
                Ok(()) => {
                    Self::reload_mcp_tokens(dialog);
                    dialog.secret = None;
                    dialog.message = Some(t!("mcp.revoked").to_string());
                    dialog.error = None;
                }
                Err(error) => {
                    dialog.error = Some(error);
                    dialog.message = None;
                }
            }
        }
        cx.notify();
    }

    fn delete_mcp_token(&mut self, cx: &mut Context<Self>) {
        let Some((id, armed)) = self
            .mcp_settings
            .as_ref()
            .and_then(|dialog| dialog.selected.map(|id| (id, dialog.pending_delete)))
        else {
            return;
        };
        if !armed {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.pending_delete = true;
                dialog.message = None;
                dialog.error = None;
            }
            cx.notify();
            return;
        }
        let outcome = token_store()
            .delete(&id)
            .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string());
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.pending_delete = false;
            match outcome {
                Ok(()) => {
                    dialog.selected = None;
                    dialog.creating = false;
                    dialog.secret = None;
                    Self::reload_mcp_tokens(dialog);
                    dialog.message = Some(t!("mcp.deleted").to_string());
                    dialog.error = None;
                }
                Err(error) => {
                    dialog.error = Some(error);
                    dialog.message = None;
                }
            }
        }
        cx.notify();
    }

    fn toggle_mcp_scope(&mut self, scope: Scope, enabled: bool, cx: &mut Context<Self>) {
        if let Some(dialog) = self.mcp_settings.as_mut() {
            if enabled {
                if !dialog.scopes.contains(&scope) {
                    dialog.scopes.push(scope);
                }
            } else {
                dialog.scopes.retain(|item| *item != scope);
            }
            dialog.error = None;
        }
        cx.notify();
    }

    fn toggle_mcp_notebook(&mut self, notebook_id: Uuid, enabled: bool, cx: &mut Context<Self>) {
        if let Some(dialog) = self.mcp_settings.as_mut() {
            if enabled {
                dialog.notebooks.insert(notebook_id);
            } else {
                dialog.notebooks.remove(&notebook_id);
            }
            dialog.error = None;
        }
        cx.notify();
    }

    fn apply_mcp_server(&mut self, cx: &mut Context<Self>) {
        let Some((raw_port, enabled)) = self.mcp_settings.as_ref().map(|dialog| {
            (
                dialog.port_input.read(cx).value().trim().to_string(),
                dialog.enabled,
            )
        }) else {
            return;
        };
        let port = raw_port.parse::<u16>().ok().filter(|port| *port > 0);
        let Some(port) = port else {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.error = Some(t!("mcp.err.port").to_string());
                dialog.message = None;
            }
            cx.notify();
            return;
        };

        self.config.mcp_port = port;
        self.config.mcp_http = enabled;
        let mut error = self
            .config
            .save()
            .err()
            .map(|err| format!("{} {err}", t!("mcp.save_failed")));
        let mut message = None;
        self.mcp.stop();
        self.mcp_live = false;
        if enabled {
            if self.mcp.start(port) {
                self.mcp_live = true;
                message = Some(t!("mcp.started", port = port).to_string());
            } else if error.is_none() {
                error = self.mcp.error.clone();
            }
        } else {
            message = Some(t!("mcp.stopped").to_string());
        }
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.error = error;
            dialog.message = message;
        }
        cx.notify();
    }

    fn refresh_mcp_audit(&mut self, cx: &mut Context<Self>) {
        let (audit, error) = load_audit_entries();
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.audit = audit;
            dialog.audit_error = error;
        }
        cx.notify();
    }

    fn copy_mcp_text(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.message = Some(t!("mcp.copied").to_string());
            dialog.error = None;
        }
        cx.notify();
    }

    fn settings_nav(
        &self,
        tab: SettingsTab,
        icon: IconName,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .settings
            .as_ref()
            .is_some_and(|dialog| dialog.tab == tab);
        let color = if active {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        };
        let hover = cx.theme().list_hover;
        div()
            .id(match tab {
                SettingsTab::Appearance => "settings-nav-appearance",
                SettingsTab::Panels => "settings-nav-panels",
                SettingsTab::Images => "settings-nav-images",
                SettingsTab::Mcp => "settings-nav-mcp",
                SettingsTab::Windows => "settings-nav-windows",
            })
            .h_flex()
            .h(px(32.))
            .px_3()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .text_color(if active {
                cx.theme().foreground
            } else {
                cx.theme().muted_foreground
            })
            .bg(if active {
                cx.theme().list_active
            } else {
                cx.theme().transparent
            })
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| this.set_settings_tab(tab, cx)))
            .child(Icon::new(icon).small().text_color(color))
            .child(t!(key).to_string())
            .into_any_element()
    }

    fn render_settings_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let dangerish = cx.theme().border;

        let theme_button =
            |preference: ThemePreference, key: &'static str, cx: &mut Context<Self>| {
                let active = self.config.theme == preference;
                let id = match preference {
                    ThemePreference::System => "settings-theme-system",
                    ThemePreference::Light => "settings-theme-light",
                    ThemePreference::Dark => "settings-theme-dark",
                };
                Button::new(id)
                    .ghost()
                    .compact()
                    .label(t!(key).to_string())
                    .when(active, |button| button.primary())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_theme_preference(preference, window, cx)
                    }))
            };

        let language_button =
            |language: &'static str, label: &'static str, cx: &mut Context<Self>| {
                let active = self.config.language == language;
                Button::new(match language {
                    "zh-CN" => "settings-language-zh",
                    _ => "settings-language-en",
                })
                .ghost()
                .compact()
                .label(label)
                .when(active, |button| button.primary())
                .on_click(cx.listener(move |this, _, _, cx| this.set_language(language, cx)))
            };

        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.theme").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.theme_hint").to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(theme_button(ThemePreference::System, "theme.system", cx))
                    .child(theme_button(ThemePreference::Light, "theme.light", cx))
                    .child(theme_button(ThemePreference::Dark, "theme.dark", cx)),
            )
            .child(div().my_2().h(px(1.)).bg(dangerish))
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.language").to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(language_button("zh-CN", "中文", cx))
                    .child(language_button("en", "English", cx)),
            )
            .child(div().my_2().h(px(1.)).bg(dangerish))
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.typography").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.typography_hint").to_string()),
            )
            .child({
                let (ui_font, note_font, ui_size, note_size) = match self.settings.as_ref() {
                    Some(dialog) => (
                        dialog.ui_font.clone(),
                        dialog.note_font.clone(),
                        dialog.ui_size.clone(),
                        dialog.note_size.clone(),
                    ),
                    None => return div().into_any_element(),
                };
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t!("settings.ui_font").to_string()),
                                    )
                                    .child(Input::new(&ui_font).small()),
                            )
                            .child(
                                div()
                                    .w(px(72.))
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t!("settings.size").to_string()),
                                    )
                                    .child(Input::new(&ui_size).small()),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t!("settings.note_font").to_string()),
                                    )
                                    .child(Input::new(&note_font).small()),
                            )
                            .child(
                                div()
                                    .w(px(72.))
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t!("settings.size").to_string()),
                                    )
                                    .child(Input::new(&note_size).small()),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("settings-font-apply")
                                    .primary()
                                    .compact()
                                    .label(t!("settings.apply_fonts").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.apply_typography(window, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t!("settings.fonts_empty_hint").to_string()),
                            ),
                    )
                    .into_any_element()
            })
            .child(div().my_2().h(px(1.)).bg(dangerish))
            .child(
                Checkbox::new("settings-autosave")
                    .label(t!("settings.autosave").to_string())
                    .checked(self.config.autosave)
                    .on_click(
                        cx.listener(|this, checked: &bool, _, cx| this.set_autosave(*checked, cx)),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.autosave_hint").to_string()),
            )
            .into_any_element()
    }

    fn render_palette(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(palette) = self.palette.as_ref() else {
            return div().into_any_element();
        };
        let input = palette.input.clone();
        let muted = cx.theme().muted_foreground;
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let active = cx.theme().list_active;
        let hover = cx.theme().list_hover;

        let rows: Vec<(usize, String, String, bool)> = palette
            .filtered
            .iter()
            .take(10)
            .enumerate()
            .map(|(row, index)| {
                let item = &palette.items[*index];
                (
                    row,
                    item.label.clone(),
                    item.hint.clone(),
                    row == palette.selected,
                )
            })
            .collect();
        let shell = cx.entity();
        let empty = rows.is_empty();

        let mut list = div().v_flex().gap_0p5();
        for (row, label, hint, selected) in rows {
            let click_shell = shell.clone();
            list = list.child(
                div()
                    .id(format!("palette-row-{row}"))
                    .h(px(32.))
                    .w_full()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if selected {
                        active
                    } else {
                        cx.theme().transparent
                    })
                    .hover(move |style| style.bg(hover))
                    .on_click(move |_, window, cx| {
                        click_shell.update(cx, |this, cx| {
                            if let Some(palette) = this.palette.as_mut() {
                                palette.selected = row;
                            }
                            this.run_palette_selected(window, cx);
                        });
                    })
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(label))
                    .child(div().text_xs().text_color(muted).child(hint)),
            );
        }
        if empty {
            list = list.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("palette.empty").to_string()),
            );
        }

        let key_shell = shell.clone();
        div()
            .id("palette-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(96.))
            .bg(overlay)
            .occlude()
            .on_mouse_down(MouseButton::Left, {
                let shell = shell.clone();
                move |_, _, cx| {
                    shell.update(cx, |this, cx| this.close_palette(cx));
                }
            })
            .on_key_down(move |event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.clone();
                key_shell.update(cx, |this, cx| match key.as_str() {
                    "escape" => this.close_palette(cx),
                    "up" => this.palette_step(-1, cx),
                    "down" => this.palette_step(1, cx),
                    _ => {}
                });
            })
            .child(
                div()
                    .id("palette-dialog")
                    .v_flex()
                    .gap_2()
                    .w(px(640.))
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Input::new(&input).w_full())
                    .child(list),
            )
            .into_any_element()
    }

    fn render_recovery(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.recovery.as_ref() else {
            return div().into_any_element();
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let rows: Vec<(usize, String, String)> = dialog
            .entries
            .iter()
            .take(8)
            .enumerate()
            .map(|(index, snapshot)| {
                let title = snapshot.display_name();
                let notebook = snapshot
                    .notebook_name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| t!("recovery.external").to_string());
                let subtitle = format!(
                    "{} · {}",
                    notebook,
                    format_age(snapshot.age_seconds(now))
                );
                (index, title, subtitle)
            })
            .collect();
        let more = dialog.entries.len().saturating_sub(rows.len());
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().list_hover;
        let shell = cx.entity();

        let mut list = div().v_flex().gap_1();
        for (index, title, subtitle) in rows {
            let restore_shell = shell.clone();
            let discard_shell = shell.clone();
            list = list.child(
                div()
                    .id(format!("recovery-row-{index}"))
                    .h_flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .hover(move |style| style.bg(hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .v_flex()
                            .child(div().text_sm().truncate().child(title))
                            .child(div().text_xs().text_color(muted).truncate().child(subtitle)),
                    )
                    .child(
                        Button::new(format!("recovery-restore-{index}"))
                            .primary()
                            .compact()
                            .label(t!("recovery.restore").to_string())
                            .on_click(move |_, window, cx| {
                                restore_shell.update(cx, |this, cx| {
                                    this.restore_recovery(index, window, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new(format!("recovery-discard-{index}"))
                            .ghost()
                            .compact()
                            .label(t!("recovery.discard").to_string())
                            .on_click(move |_, _, cx| {
                                discard_shell
                                    .update(cx, |this, cx| this.discard_recovery(index, cx));
                            }),
                    ),
            );
        }
        if more > 0 {
            list = list.child(
                div()
                    .px_3()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("recovery.more", count = more).to_string()),
            );
        }

        div()
            .id("recovery-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .w(px(560.))
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_bold()
                                    .child(t!("recovery.title").to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t!("recovery.desc").to_string()),
                            ),
                    )
                    .child(list)
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("recovery-discard-all")
                                    .ghost()
                                    .compact()
                                    .label(t!("recovery.discard_all").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.discard_all_recovery(cx)),
                                    ),
                            )
                            .child(
                                Button::new("recovery-close")
                                    .compact()
                                    .label(t!("common.close").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| this.close_recovery(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_prompt(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prompt) = self.prompt.as_ref() else {
            return div().into_any_element();
        };
        let input = prompt.input.clone();
        let title = prompt.title.clone();
        let hint = prompt.hint.clone();
        let error = prompt.error.clone();
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;

        div()
            .id("prompt-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .on_mouse_down(MouseButton::Left, {
                let shell = cx.entity();
                move |_, _, cx| {
                    shell.update(cx, |this, cx| this.close_prompt(cx));
                }
            })
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .w(px(520.))
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().text_base().font_bold().child(title))
                    .child(div().text_xs().text_color(muted).child(hint))
                    .child(Input::new(&input).w_full())
                    .children(error.map(|error| div().text_xs().text_color(danger).child(error)))
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("prompt-cancel")
                                    .ghost()
                                    .compact()
                                    .label(t!("common.cancel").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| this.close_prompt(cx))),
                            )
                            .child(
                                Button::new("prompt-ok")
                                    .primary()
                                    .compact()
                                    .label(t!("common.ok").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| this.submit_prompt(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_settings_windows(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let workspace = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().display().to_string())
            .unwrap_or_default();
        let registered = crate::file_assoc::is_registered();

        let mut rows = div().v_flex().gap_3();

        rows =
            rows.child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .child(t!("settings.windows_assoc").to_string()),
                    )
                    .child(div().text_xs().text_color(muted).child(if registered {
                        t!("settings.windows_assoc_on").to_string()
                    } else {
                        t!("settings.windows_assoc_off").to_string()
                    }))
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("settings-register-assoc")
                                    .compact()
                                    .label(t!("settings.windows_register").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.register_file_assoc(cx)),
                                    ),
                            )
                            .child(
                                Button::new("settings-default-apps")
                                    .ghost()
                                    .compact()
                                    .label(t!("settings.windows_default_apps").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_windows_default_apps(cx)
                                    })),
                            ),
                    ),
            );

        rows = rows.child(div().h(px(1.)).bg(border)).child(
            div()
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_bold()
                        .child(t!("settings.windows_workspace").to_string()),
                )
                .child(div().text_xs().text_color(muted).child(workspace.clone()))
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("settings-open-workspace")
                                .compact()
                                .label(t!("settings.windows_open_workspace").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_path_in_explorer(
                                        this.workspace
                                            .as_ref()
                                            .map(|workspace| workspace.root().to_path_buf()),
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new("settings-open-logs")
                                .ghost()
                                .compact()
                                .label(t!("settings.windows_open_logs").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_path_in_explorer(
                                        Some(mdwork_core::paths::logs_dir()),
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new("settings-open-crashes")
                                .ghost()
                                .compact()
                                .label(t!("settings.windows_open_crashes").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_path_in_explorer(
                                        Some(mdwork_core::paths::crashes_dir()),
                                        cx,
                                    )
                                })),
                        ),
                ),
        );

        rows.into_any_element()
    }

    fn render_settings_panels(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;

        let dock_button =
            |dock: PanelDock, icon: IconName, key: &'static str, cx: &mut Context<Self>| {
                let active = self.config.panel_dock == dock;
                Button::new(match dock {
                    PanelDock::Left => "settings-dock-left",
                    PanelDock::Right => "settings-dock-right",
                })
                .ghost()
                .compact()
                .icon(icon)
                .label(t!(key).to_string())
                .when(active, |button| button.primary())
                .on_click(cx.listener(move |this, _, _, cx| this.set_panel_dock(dock, cx)))
            };

        let arrangement_button = |arrangement: PanelArrangement,
                                  icon: IconName,
                                  key: &'static str,
                                  cx: &mut Context<Self>| {
            let active = self.config.panel_arrangement == arrangement;
            Button::new(match arrangement {
                PanelArrangement::Horizontal => "settings-arrangement-horizontal",
                PanelArrangement::Vertical => "settings-arrangement-vertical",
            })
            .ghost()
            .compact()
            .icon(icon)
            .label(t!(key).to_string())
            .when(active, |button| button.primary())
            .on_click(
                cx.listener(move |this, _, _, cx| this.set_panel_arrangement(arrangement, cx)),
            )
        };

        let width_button =
            |width: f32, id: &'static str, key: &'static str, cx: &mut Context<Self>| {
                let active = (self.config.panel_width - width).abs() < 1.0;
                Button::new(id)
                    .ghost()
                    .compact()
                    .label(t!(key).to_string())
                    .when(active, |button| button.primary())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_panel_width(width, cx)))
            };

        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.panel_dock").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.panel_dock_hint").to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(dock_button(
                        PanelDock::Left,
                        IconName::PanelLeft,
                        "settings.dock.left",
                        cx,
                    ))
                    .child(dock_button(
                        PanelDock::Right,
                        IconName::PanelRight,
                        "settings.dock.right",
                        cx,
                    )),
            )
            .child(div().my_2().h(px(1.)).bg(border))
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.panel_arrangement").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.panel_arrangement_hint").to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(arrangement_button(
                        PanelArrangement::Horizontal,
                        IconName::Rows3,
                        "settings.arrangement.horizontal",
                        cx,
                    ))
                    .child(arrangement_button(
                        PanelArrangement::Vertical,
                        IconName::Columns3,
                        "settings.arrangement.vertical",
                        cx,
                    )),
            )
            .child(div().my_2().h(px(1.)).bg(border))
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.panel_width").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.panel_width_hint").to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(width_button(
                        240.0,
                        "settings-width-narrow",
                        "settings.width.narrow",
                        cx,
                    ))
                    .child(width_button(
                        300.0,
                        "settings-width-standard",
                        "settings.width.standard",
                        cx,
                    ))
                    .child(width_button(
                        380.0,
                        "settings-width-wide",
                        "settings.width.wide",
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.settings.as_ref() else {
            return div().into_any_element();
        };
        let tab = dialog.tab;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let overlay = cx.theme().overlay;
        let muted = cx.theme().muted_foreground;

        let content = match tab {
            SettingsTab::Appearance => self.render_settings_appearance(cx),
            SettingsTab::Panels => self.render_settings_panels(cx),
            SettingsTab::Images => self.render_image_settings_page(cx),
            SettingsTab::Mcp => self.render_mcp_settings_page(cx),
            SettingsTab::Windows => self.render_settings_windows(cx),
        };
        let title = match tab {
            SettingsTab::Appearance => t!("settings.tab.appearance"),
            SettingsTab::Panels => t!("settings.tab.panels"),
            SettingsTab::Images => t!("settings.tab.images"),
            SettingsTab::Mcp => t!("settings.tab.mcp"),
            SettingsTab::Windows => t!("settings.tab.windows"),
        };

        div()
            .id("settings-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .child(
                div()
                    .id("settings-dialog")
                    .v_flex()
                    .w(px(880.))
                    .h(px(600.))
                    .gap_4()
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_base()
                                    .font_bold()
                                    .child(t!("settings.title").to_string()),
                            )
                            .child(
                                Button::new("settings-close")
                                    .ghost()
                                    .compact()
                                    .icon(IconName::X)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_settings(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .flex_1()
                            .min_h_0()
                            .gap_4()
                            .child(
                                div()
                                    .v_flex()
                                    .w(px(150.))
                                    .h_full()
                                    .flex_shrink_0()
                                    .gap_1()
                                    .child(self.settings_nav(
                                        SettingsTab::Appearance,
                                        IconName::Palette,
                                        "settings.tab.appearance",
                                        cx,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Panels,
                                        IconName::LayoutPanelLeft,
                                        "settings.tab.panels",
                                        cx,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Images,
                                        IconName::Image,
                                        "settings.tab.images",
                                        cx,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Mcp,
                                        IconName::ServerCog,
                                        "settings.tab.mcp",
                                        cx,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Windows,
                                        IconName::Monitor,
                                        "settings.tab.windows",
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .mt_auto()
                                            .px_2()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(
                                                t!(
                                                    "settings.version",
                                                    version = env!("CARGO_PKG_VERSION")
                                                )
                                                .to_string(),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .gap_2()
                                    .child(
                                        div().text_xs().text_color(muted).child(title.to_string()),
                                    )
                                    .child(content),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_bookmarks(cx);

        let vertical = self.config.panel_arrangement == PanelArrangement::Vertical;
        let dock_right = self.config.panel_dock == PanelDock::Right;
        let border = cx.theme().border;
        let sidebar_border = cx.theme().sidebar_border;

        let body = match self.panel {
            SidePanel::Notes => self.render_notes_panel(cx),
            SidePanel::Outline => self.render_outline_panel(cx),
            SidePanel::Search => self.render_search_panel(cx),
            SidePanel::Bookmarks => self.render_bookmarks_panel(cx),
            SidePanel::Trash => self.render_trash_panel(cx),
        };

        let workspace_path = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().display().to_string())
            .unwrap_or_default();

        let mut content = div().v_flex().flex_1().min_w_0().min_h_0();
        if !vertical {
            content = content.child(self.panel_tab_row(cx));
        }
        let content = content.child(body).child(
            div()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(sidebar_border)
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(workspace_path),
        );

        let mut root = div()
            .w(px(self.config.panel_width))
            .h_full()
            .bg(cx.theme().sidebar)
            .border_color(border);
        root = if dock_right {
            root.border_l_1()
        } else {
            root.border_r_1()
        };

        if vertical {
            let rail = self.panel_rail(cx);
            let mut row = div().h_flex().size_full();
            row = if dock_right {
                row.child(content).child(rail)
            } else {
                row.child(rail).child(content)
            };
            root.child(row).into_any_element()
        } else {
            root.v_flex().child(content).into_any_element()
        }
    }

    fn render_panel_handle(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let dragging = self.panel_resize.is_some();
        let idle = cx.theme().background;
        let hover = cx.theme().border;
        let active = cx.theme().primary;
        div()
            .id("panel-resize-handle")
            .w(px(4.))
            .h_full()
            .flex_none()
            .cursor(CursorStyle::ResizeLeftRight)
            .bg(if dragging { active } else { idle })
            .hover(move |style| style.bg(hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    if event.click_count >= 2 {
                        this.set_panel_width(300.0, cx);
                    } else {
                        this.begin_panel_resize(f32::from(event.position.x), cx);
                    }
                }),
            )
            .into_any_element()
    }

    fn panel_icon(panel: SidePanel) -> IconName {
        match panel {
            SidePanel::Notes => IconName::NotebookPen,
            SidePanel::Outline => IconName::ListTree,
            SidePanel::Search => IconName::Search,
            SidePanel::Bookmarks => IconName::Bookmark,
            SidePanel::Trash => IconName::Trash,
        }
    }

    fn panel_tab_row(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h(px(34.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().sidebar_border)
            .child(self.panel_tab(
                SidePanel::Notes,
                t!("panel.notes").to_string(),
                "panel-notes",
                cx,
            ))
            .child(self.panel_tab(
                SidePanel::Outline,
                t!("panel.outline").to_string(),
                "panel-outline",
                cx,
            ))
            .child(self.panel_tab(
                SidePanel::Search,
                t!("panel.search").to_string(),
                "panel-search",
                cx,
            ))
            .child(self.panel_tab(
                SidePanel::Bookmarks,
                t!("panel.bookmarks").to_string(),
                "panel-bookmarks",
                cx,
            ))
            .child(self.panel_tab(
                SidePanel::Trash,
                t!("panel.trash").to_string(),
                "panel-trash",
                cx,
            ))
            .into_any_element()
    }

    fn panel_rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut rail = div()
            .v_flex()
            .w(px(40.))
            .h_full()
            .flex_shrink_0()
            .items_center()
            .py_2()
            .gap_1()
            .border_color(cx.theme().sidebar_border);
        rail = if self.config.panel_dock == PanelDock::Right {
            rail.border_l_1()
        } else {
            rail.border_r_1()
        };
        for panel in [
            SidePanel::Notes,
            SidePanel::Outline,
            SidePanel::Search,
            SidePanel::Bookmarks,
            SidePanel::Trash,
        ] {
            let active = self.panel == panel;
            let label = match panel {
                SidePanel::Notes => t!("panel.notes"),
                SidePanel::Outline => t!("panel.outline"),
                SidePanel::Search => t!("panel.search"),
                SidePanel::Bookmarks => t!("panel.bookmarks"),
                SidePanel::Trash => t!("panel.trash"),
            };
            rail = rail.child(
                Button::new(match panel {
                    SidePanel::Notes => "rail-notes",
                    SidePanel::Outline => "rail-outline",
                    SidePanel::Search => "rail-search",
                    SidePanel::Bookmarks => "rail-bookmarks",
                    SidePanel::Trash => "rail-trash",
                })
                .ghost()
                .compact()
                .icon(Self::panel_icon(panel))
                .tooltip(label.to_string())
                .when(active, |button| button.primary())
                .on_click(cx.listener(move |this, _, _, cx| this.set_panel(panel, cx))),
            );
        }
        rail.into_any_element()
    }

    fn panel_tab(
        &self,
        panel: SidePanel,
        label: String,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.panel == panel;
        let hover = cx.theme().list_hover;
        let color = if active {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        };
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .h(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(color)
            .bg(if active {
                cx.theme().list_active
            } else {
                cx.theme().transparent
            })
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| this.set_panel(panel, cx)))
            .child(
                Icon::new(Self::panel_icon(panel))
                    .xsmall()
                    .text_color(color),
            )
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    }

    fn panel_header(&self, title: String, cx: &mut Context<Self>) -> AnyElement {
        let notebook = self
            .active_notebook
            .and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
            })
            .map(|entry| entry.notebook.display_name().to_string())
            .unwrap_or_default();
        div()
            .px_3()
            .pt_2()
            .pb_1()
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_xs().font_bold().child(title))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(notebook),
            )
            .into_any_element()
    }

    fn render_notes_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex_1().min_h_0().v_flex().gap_1().px_2().pb_2();

        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .pt_1()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("sidebar.notebooks").to_string()),
                )
                .child(
                    Button::new("btn-new-notebook")
                        .ghost()
                        .compact()
                        .label("+")
                        .tooltip(t!("sidebar.new_notebook").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.new_notebook(cx))),
                )
                .child(
                    Button::new("btn-rescan")
                        .ghost()
                        .compact()
                        .label("↻")
                        .tooltip(t!("sidebar.refresh").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reload();
                            this.persist_session(cx);
                            cx.notify();
                        })),
                ),
        );

        if !self.scan_issues.is_empty() {
            let issues = self.scan_issues.clone();
            body = body.child(
                div()
                    .v_flex()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .bg(cx.theme().list_hover)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(cx.theme().danger)
                                    .child(
                                        t!("sidebar.scan_issues", count = issues.len()).to_string(),
                                    ),
                            )
                            .child(
                                Button::new("btn-dismiss-scan-issues")
                                    .ghost()
                                    .compact()
                                    .label("×")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.scan_issues.clear();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .children(issues.into_iter().take(4).map(|issue| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(issue)
                    })),
            );
        }

        if self.notebooks.is_empty() {
            body = body.child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("sidebar.empty").to_string()),
            );
        }

        for entry in &self.notebooks {
            let notebook_id = entry.notebook.marker.id;
            let is_active = self.active_notebook == Some(notebook_id);
            let note_count = entry
                .tree
                .iter()
                .filter(|item| item.kind == EntryKind::Note)
                .count();
            let count_label = format!("{} {}", note_count, t!("sidebar.notes"));
            let hover = cx.theme().list_hover;
            let shell = cx.entity();
            let export_shell = shell.clone();
            let import_shell = shell;
            let export_label = t!("zip.export_label").to_string();
            let import_label = t!("zip.import_label").to_string();

            body = body.child(
                div()
                    .id(format!("notebook-{notebook_id}"))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if is_active {
                        cx.theme().list_active
                    } else {
                        cx.theme().transparent
                    })
                    .hover(move |style| style.bg(hover))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.activate_notebook(notebook_id, cx)),
                    )
                    .context_menu(move |menu, _, _| {
                        let export_shell = export_shell.clone();
                        let import_shell = import_shell.clone();
                        menu.item(PopupMenuItem::new(export_label.clone()).on_click(
                            move |_, window, cx| {
                                export_shell.update(cx, |this, cx| {
                                    this.open_zip_export(notebook_id, window, cx)
                                });
                            },
                        ))
                        .item(
                            PopupMenuItem::new(import_label.clone()).on_click(
                                move |_, window, cx| {
                                    import_shell
                                        .update(cx, |this, cx| this.open_zip_import(window, cx));
                                },
                            ),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .font_bold()
                            .truncate()
                            .child(entry.notebook.display_name().to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(count_label),
                    ),
            );
        }

        let active_entry = self.active_notebook.and_then(|id| {
            self.notebooks
                .iter()
                .find(|entry| entry.notebook.marker.id == id)
        });
        if let Some(entry) = active_entry {
            body = body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .mt_1()
                        .child(div().flex_1())
                        .child(
                            Button::new("btn-sidebar-new-folder")
                                .ghost()
                                .compact()
                                .label("🗀")
                                .tooltip(t!("sidebar.new_folder").to_string())
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.new_folder(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("btn-sidebar-open-notebook")
                                .ghost()
                                .compact()
                                .label("…")
                                .tooltip(t!("sidebar.open_notebook").to_string())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_external_notebook_prompt(window, cx)
                                })),
                        )
                        .child(
                            Button::new("btn-sidebar-new-note")
                                .ghost()
                                .compact()
                                .label("＋")
                                .tooltip(t!("sidebar.new_note").to_string())
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.new_note(window, cx)),
                                ),
                        ),
                )
                .child(self.render_tree(entry.notebook.marker.id, entry, cx));
        }

        body.into_any_element()
    }

    fn render_outline_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.refresh_outline(cx);
        let items = Arc::new(self.outline.clone());
        let count = items.len();
        let has_doc = self.active_doc.is_some();
        let target = self.active_doc_entity().and_then(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id.map(|id| (id, doc.rel_path.clone()))
        });
        let bookmarks = self.bookmarks.clone();
        let shell = cx.entity();
        let hover = cx.theme().list_hover;
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;

        let body: AnyElement = if !has_doc {
            empty_hint(t!("outline.no_doc").to_string(), muted)
        } else if items.is_empty() {
            empty_hint(t!("outline.empty").to_string(), muted)
        } else {
            uniform_list("outline-list", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let item = &items[index];
                        let level = item.level.clamp(1, 6);
                        let line = item.line;
                        let title = item.title.clone();
                        let (notebook_id, rel_path) = match &target {
                            Some((id, rel)) => (Some(*id), Some(rel.clone())),
                            None => (None, None),
                        };
                        let starred = rel_path
                            .as_deref()
                            .map(|rel| bookmarks.has_heading(rel, &title))
                            .unwrap_or(false);
                        let row_shell = shell.clone();
                        let star_shell = shell.clone();
                        let jump_rel = rel_path.clone();
                        let star_rel = rel_path.clone();
                        let star_title = title.clone();

                        div()
                            .id(format!("outline-row-{index}"))
                            .h(px(26.))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_1()
                            .pl(px(8.0 + (level - 1) as f32 * 12.0))
                            .pr_1()
                            .rounded_md()
                            .hover(move |style| style.bg(hover))
                            .child(
                                div()
                                    .id(format!("outline-jump-{index}"))
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        if let (Some(id), Some(rel)) =
                                            (notebook_id, jump_rel.clone())
                                        {
                                            row_shell.update(cx, |this, cx| {
                                                this.jump_to(id, &rel, line, window, cx)
                                            });
                                        }
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .text_color(foreground)
                                            .child(title),
                                    ),
                            )
                            .children(star_rel.map(|rel| {
                                Button::new(format!("outline-star-{index}"))
                                    .ghost()
                                    .compact()
                                    .label(if starred { "★" } else { "☆" })
                                    .tooltip(t!("outline.bookmark").to_string())
                                    .on_click(move |_, _, cx| {
                                        star_shell.update(cx, |this, cx| {
                                            this.toggle_heading_bookmark(
                                                &rel,
                                                &star_title,
                                                line,
                                                cx,
                                            )
                                        });
                                    })
                            }))
                    })
                    .collect::<Vec<_>>()
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .child(self.panel_header(t!("panel.outline").to_string(), cx))
            .child(body)
            .into_any_element()
    }

    fn render_search_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let all = self.search_all;
        let results = Arc::new(self.search_results.clone());
        let count = results.len();
        let has_query = !self.search_input.read(cx).value().is_empty();
        let status = match &self.search_status {
            SearchStatus::Running => t!("search.running").to_string(),
            SearchStatus::Failed(err) => err.clone(),
            SearchStatus::Idle => {
                if !has_query {
                    t!("search.hint").to_string()
                } else if count == 0 {
                    t!("search.empty").to_string()
                } else {
                    t!("search.count", count = count).to_string()
                }
            }
        };
        let failed = matches!(self.search_status, SearchStatus::Failed(_));
        let status_color = if failed {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        let muted = cx.theme().muted_foreground;
        let running = matches!(self.search_status, SearchStatus::Running);
        let foreground = cx.theme().foreground;
        let hover = cx.theme().list_hover;

        let current_button = div()
            .id("scope-current")
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(if all { muted } else { foreground })
            .bg(if all {
                cx.theme().transparent
            } else {
                cx.theme().list_active
            })
            .on_click(cx.listener(|this, _, _, cx| this.set_search_scope(false, cx)))
            .child(t!("search.current").to_string());
        let all_button = div()
            .id("scope-all")
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(if all { foreground } else { muted })
            .bg(if all {
                cx.theme().list_active
            } else {
                cx.theme().transparent
            })
            .on_click(cx.listener(|this, _, _, cx| this.set_search_scope(true, cx)))
            .child(t!("search.all").to_string());

        let toggle = |id: &'static str,
                      key: &'static str,
                      active: bool,
                      option: SearchOption,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_xs()
                .text_color(if active { foreground } else { muted })
                .bg(if active {
                    cx.theme().list_active
                } else {
                    cx.theme().transparent
                })
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_search_option(option, cx)))
                .child(t!(key).to_string())
        };

        let mut toolbar = div()
            .px_2()
            .py_2()
            .v_flex()
            .gap_1()
            .child(Input::new(&self.search_input).cleanable(true).w_full())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(current_button)
                    .child(all_button)
                    .child(toggle(
                        "search-opt-case",
                        "search.opt.case",
                        self.search_case,
                        SearchOption::Case,
                        cx,
                    ))
                    .child(toggle(
                        "search-opt-regex",
                        "search.opt.regex",
                        self.search_regex,
                        SearchOption::Regex,
                        cx,
                    ))
                    .child(toggle(
                        "search-opt-filename",
                        "search.opt.filename",
                        self.search_filename,
                        SearchOption::Filename,
                        cx,
                    ))
                    .child(div().flex_1())
                    .children(running.then(|| {
                        Button::new("btn-search-cancel")
                            .ghost()
                            .compact()
                            .label("×")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.search_cancel.store(true, Ordering::Relaxed);
                                this.search_status = SearchStatus::Idle;
                                cx.notify();
                            }))
                    })),
            )
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(status_color)
                    .truncate()
                    .child(status),
            );

        let shell = cx.entity();

        let body: AnyElement = if results.is_empty() {
            div().flex_1().into_any_element()
        } else {
            uniform_list("search-results", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let hit = &results[index];
                        let shell = shell.clone();
                        let rel_path = hit.rel_path.clone();
                        let notebook_id = hit.notebook_id;
                        let line = hit.line;
                        let notebook_name = hit.notebook_name.clone();
                        let file_name = Path::new(&hit.rel_path)
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or_else(|| hit.rel_path.clone());

                        div()
                            .id(format!("search-row-{index}"))
                            .h(px(44.))
                            .w_full()
                            .px_2()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .on_click(move |_, window, cx| {
                                shell.update(cx, |this, cx| {
                                    this.jump_to(notebook_id, &rel_path, line, window, cx)
                                });
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().text_sm().truncate().child(file_name))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(format!(":{}", hit.line)),
                                    )
                                    .children(all.then(|| {
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .truncate()
                                            .child(notebook_name)
                                    })),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(hit.text.clone()),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        toolbar = toolbar.child(body);
        toolbar.flex_1().min_h_0().into_any_element()
    }

    fn render_bookmarks_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let notebook_id = self.active_notebook;
        let entries = Arc::new(self.bookmarks.entries.clone());
        let count = entries.len();
        let shell = cx.entity();
        let hover = cx.theme().list_hover;
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        let body: AnyElement = if entries.is_empty() {
            empty_hint(t!("bookmark.empty").to_string(), muted)
        } else {
            uniform_list("bookmark-list", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let entry = &entries[index];
                        let rel_path = entry.rel_path.clone();
                        let heading = entry.heading.clone();
                        let subtitle = match &entry.heading {
                            Some(heading) => heading.clone(),
                            None => entry.rel_path.clone(),
                        };
                        let line = entry.line;
                        let row_shell = shell.clone();
                        let remove_shell = shell.clone();
                        let remove_rel = rel_path.clone();
                        let remove_heading = heading.clone();
                        let title = match (&entry.heading, &entry.title) {
                            (Some(_), note_title) => note_title.clone(),
                            (None, note_title) => note_title.clone(),
                        };

                        div()
                            .id(format!("bookmark-row-{index}"))
                            .h(px(46.))
                            .w_full()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded_md()
                            .hover(move |style| style.bg(hover))
                            .child(
                                div()
                                    .id(format!("bookmark-jump-{index}"))
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        if let Some(id) = notebook_id {
                                            row_shell.update(cx, |this, cx| {
                                                this.jump_to(id, &rel_path, line, window, cx)
                                            });
                                        }
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .v_flex()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_sm()
                                                    .text_color(foreground)
                                                    .child(title),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(subtitle),
                                            ),
                                    ),
                            )
                            .child(
                                Button::new(format!("bookmark-remove-{index}"))
                                    .ghost()
                                    .compact()
                                    .label("×")
                                    .tooltip(t!("bookmark.remove").to_string())
                                    .on_click(move |_, _, cx| {
                                        remove_shell.update(cx, |this, cx| {
                                            this.remove_bookmark(
                                                &remove_rel,
                                                remove_heading.as_deref(),
                                                cx,
                                            )
                                        });
                                    }),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .child(self.panel_header(t!("panel.bookmarks").to_string(), cx))
            .child(body)
            .into_any_element()
    }

    fn render_trash_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_trash(cx);
        let items = Arc::new(self.trash.clone());
        let count = items.len();
        let shell = cx.entity();
        let hover = cx.theme().list_hover;
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        let body: AnyElement = if items.is_empty() {
            empty_hint(t!("trash.empty").to_string(), muted)
        } else {
            uniform_list("trash-list", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let item = &items[index];
                        let id = item.id;
                        let name = if item.kind == EntryKind::Folder {
                            format!("{}/", item.rel_path)
                        } else {
                            item.rel_path.clone()
                        };
                        let age = age_label(item.deleted_at);
                        let restore_shell = shell.clone();

                        div()
                            .id(format!("trash-row-{index}"))
                            .h(px(46.))
                            .w_full()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded_md()
                            .hover(move |style| style.bg(hover))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .v_flex()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_sm()
                                            .text_color(foreground)
                                            .child(name),
                                    )
                                    .child(div().text_xs().text_color(muted).child(age)),
                            )
                            .child(
                                Button::new(format!("trash-restore-{index}"))
                                    .ghost()
                                    .compact()
                                    .label("↩")
                                    .tooltip(t!("trash.restore").to_string())
                                    .on_click(move |_, _, cx| {
                                        restore_shell
                                            .update(cx, |this, cx| this.restore_trash(id, cx));
                                    }),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .child(self.panel_header(t!("panel.trash").to_string(), cx))
            .child(body)
            .into_any_element()
    }

    fn render_conflict(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let index = self.active_doc?;
        let (conflict, title) = {
            let doc = self.docs.get(index)?.read(cx);
            (doc.conflict?, doc.title())
        };
        let message = match conflict {
            DiskConflict::Modified => t!("conflict.modified", title = title).to_string(),
            DiskConflict::Removed => t!("conflict.removed", title = title).to_string(),
        };
        let primary = match conflict {
            DiskConflict::Modified => (t!("conflict.reload").to_string(), "btn-conflict-reload"),
            DiskConflict::Removed => (t!("conflict.recreate").to_string(), "btn-conflict-recreate"),
        };
        let secondary = match conflict {
            DiskConflict::Modified => (t!("conflict.keep").to_string(), "btn-conflict-keep"),
            DiskConflict::Removed => (t!("conflict.close").to_string(), "btn-conflict-close"),
        };

        Some(
            div()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().list_active)
                .text_sm()
                .child(div().flex_1().min_w_0().child(message))
                .child(
                    Button::new(primary.1)
                        .primary()
                        .compact()
                        .label(primary.0)
                        .on_click(cx.listener(move |this, _, window, cx| match conflict {
                            DiskConflict::Modified => this.reload_doc(index, window, cx),
                            DiskConflict::Removed => this.recreate_doc(index, cx),
                        })),
                )
                .child(
                    Button::new(secondary.1)
                        .ghost()
                        .compact()
                        .label(secondary.0)
                        .on_click(cx.listener(move |this, _, _, cx| match conflict {
                            DiskConflict::Modified => this.keep_doc(index, cx),
                            DiskConflict::Removed => this.close_doc(index, cx),
                        })),
                )
                .into_any_element(),
        )
    }

    fn active_rel_path(&self, cx: &App) -> Option<String> {
        let doc = self.active_doc_entity()?;
        let doc = doc.read(cx);
        doc.notebook_id?;
        Some(doc.rel_path.clone())
    }

    fn render_tree(
        &self,
        notebook_id: Uuid,
        entry: &NotebookEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if entry.rows.is_empty() {
            return div()
                .py_2()
                .px_2()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t!("sidebar.empty").to_string())
                .into_any_element();
        }

        let rows = Arc::new(entry.rows.clone());
        let active_rel = self.active_rel_path(cx);
        let flash_rel = self.flash_rel.clone();
        let renaming = self.renaming.clone();
        let shell = cx.entity();
        let active_bg = cx.theme().list_active;
        let flash_bg = cx.theme().list_hover;
        let hover_bg = cx.theme().list_hover;
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let count = rows.len();

        uniform_list(
            format!("tree-{notebook_id}"),
            count,
            move |range, _window, _cx| {
                range
                    .map(|index| {
                        let row = &rows[index];
                        let shell = shell.clone();
                        let active = active_rel.as_deref() == Some(row.rel_path.as_str());
                        let flashing = flash_rel.as_deref() == Some(row.rel_path.as_str());
                        let background = if active {
                            active_bg
                        } else if flashing {
                            flash_bg
                        } else {
                            transparent_black()
                        };
                        let is_folder = row.kind == EntryKind::Folder;
                        let chevron = if is_folder {
                            if row.expanded { "▾" } else { "▸" }
                        } else {
                            ""
                        };
                        let color = if is_folder || active {
                            foreground
                        } else {
                            muted
                        };
                        let rel_path = row.rel_path.clone();
                        let label = row.name.clone();
                        let depth = row.depth as f32;

                        if let Some(renaming) = renaming.as_ref().filter(|renaming| {
                            renaming.notebook_id == notebook_id && renaming.rel_path == rel_path
                        }) {
                            let input = renaming.input.clone();
                            let cancel_shell = shell.clone();
                            return div()
                                .id(format!("tree-rename-{index}"))
                                .h(px(28.))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_1()
                                .pl(px(10.0 + depth * 14.0))
                                .pr_1()
                                .rounded_md()
                                .bg(active_bg)
                                .child(Input::new(&input).small().w_full())
                                .child(
                                    Button::new(format!("tree-rename-cancel-{index}"))
                                        .ghost()
                                        .compact()
                                        .label("×")
                                        .on_click(move |_, _, cx| {
                                            cancel_shell
                                                .update(cx, |this, cx| this.cancel_rename(cx));
                                        }),
                                )
                                .into_any_element();
                        }

                        let rename_shell = shell.clone();
                        let delete_shell = shell.clone();
                        let move_shell = shell.clone();
                        let rename_rel = rel_path.clone();
                        let delete_rel = rel_path.clone();
                        let move_rel = rel_path.clone();
                        let rename_label = t!("tree.rename").to_string();
                        let delete_label = t!("tree.delete").to_string();
                        let move_label = t!("tree.move").to_string();

                        div()
                            .id(format!("tree-row-{index}"))
                            .h(px(26.))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_1()
                            .pl(px(10.0 + depth * 14.0))
                            .pr_2()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(background)
                            .hover(move |style| style.bg(hover_bg))
                            .on_click(move |_, window, cx| {
                                shell.update(cx, |this, cx| {
                                    if is_folder {
                                        this.toggle_folder(notebook_id, &rel_path, cx);
                                    } else {
                                        this.open_note_by_id(notebook_id, &rel_path, window, cx);
                                    }
                                });
                            })
                            .child(
                                div()
                                    .w(px(12.))
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(chevron),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(color)
                                    .truncate()
                                    .child(label),
                            )
                            .context_menu(move |menu, _, _| {
                                let rename_shell = rename_shell.clone();
                                let delete_shell = delete_shell.clone();
                                let move_shell = move_shell.clone();
                                let rename_rel = rename_rel.clone();
                                let delete_rel = delete_rel.clone();
                                let move_rel = move_rel.clone();
                                menu.item(PopupMenuItem::new(rename_label.clone()).on_click(
                                    move |_, window, cx| {
                                        rename_shell.update(cx, |this, cx| {
                                            this.begin_rename(notebook_id, &rename_rel, window, cx)
                                        });
                                    },
                                ))
                                .item(PopupMenuItem::new(move_label.clone()).on_click(
                                    move |_, window, cx| {
                                        move_shell.update(cx, |this, cx| {
                                            this.begin_move(notebook_id, &move_rel, window, cx)
                                        });
                                    },
                                ))
                                .separator()
                                .item(
                                    PopupMenuItem::new(delete_label.clone()).on_click(
                                        move |_, _, cx| {
                                            delete_shell.update(cx, |this, cx| {
                                                this.delete_entry(
                                                    notebook_id,
                                                    delete_rel.clone(),
                                                    cx,
                                                )
                                            });
                                        },
                                    ),
                                )
                            })
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            },
        )
        .track_scroll(&self.tree_scroll)
        .flex_1()
        .min_h_0()
        .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut bar = div()
            .h(px(36.))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background);

        for (index, doc) in self.docs.iter().enumerate() {
            let doc = doc.read(cx);
            if doc.notebook_id.is_some() && doc.notebook_id != self.active_notebook {
                continue;
            }
            let active = self.active_doc == Some(index);
            let title = if doc.notebook_id.is_none() {
                format!("外部 · {}", doc.rel_path)
            } else {
                doc.title()
            };
            let dirty = doc.dirty;
            let doc_label = if doc.conflict.is_some() {
                format!("⚠ {title}")
            } else if dirty {
                format!("● {title}")
            } else {
                title
            };
            let label_color = if active {
                cx.theme().foreground
            } else {
                cx.theme().muted_foreground
            };
            let hover = cx.theme().list_hover;

            bar = bar.child(
                div()
                    .id(format!("tab-{index}"))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if active {
                        cx.theme().list_active
                    } else {
                        cx.theme().transparent
                    })
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.activate_doc(index, cx)))
                    .child(
                        div()
                            .text_sm()
                            .text_color(label_color)
                            .truncate()
                            .max_w(px(180.))
                            .child(doc_label),
                    )
                    .child(
                        Button::new(format!("close-{index}"))
                            .ghost()
                            .compact()
                            .label("×")
                            .on_click(cx.listener(move |this, _, _, cx| this.close_doc(index, cx))),
                    ),
            );
        }

        if let Some(index) = self.active_doc {
            let toggle_label = if self.preview_mode {
                t!("preview.edit").to_string()
            } else {
                t!("preview.show").to_string()
            };
            let (bookmark_target, is_external) = {
                let doc = self.docs[index].read(cx);
                (
                    doc.notebook_id.map(|id| (id, doc.rel_path.clone())),
                    doc.notebook_id.is_none(),
                )
            };
            bar = bar.child(div().flex_1());
            if is_external {
                bar = bar.child(
                    Button::new("btn-adopt-external")
                        .ghost()
                        .compact()
                        .label(t!("external.save_into").to_string())
                        .tooltip(t!("external.save_into_hint").to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_external_into_notebook(window, cx)
                        })),
                );
            }
            if let Some((_, rel_path)) = &bookmark_target {
                let starred = self.bookmarks.note(rel_path).is_some();
                bar = bar.child(
                    Button::new("btn-bookmark-note")
                        .ghost()
                        .compact()
                        .label(if starred { "★" } else { "☆" })
                        .tooltip(t!("bookmark.toggle_note").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_note_bookmark(cx))),
                );
            }
            bar = bar.child(
                Button::new("btn-toggle-preview")
                    .ghost()
                    .compact()
                    .label(toggle_label)
                    .tooltip("Ctrl+T".to_string())
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_preview(window, cx))),
            );
            if self.preview_mode {
                let copy_label = if self.copied {
                    t!("preview.copied").to_string()
                } else {
                    t!("preview.copy").to_string()
                };
                let wrap_label = if self.config.code_wrap {
                    t!("preview.wrap").to_string()
                } else {
                    t!("preview.nowrap").to_string()
                };
                bar = bar
                    .child(
                        Button::new("btn-toggle-wrap")
                            .ghost()
                            .compact()
                            .label(wrap_label)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_code_wrap(cx))),
                    )
                    .child(
                        Button::new("btn-copy-preview")
                            .ghost()
                            .compact()
                            .label(copy_label)
                            .on_click(cx.listener(|this, _, _, cx| this.copy_preview(cx))),
                    );
            }
        }

        bar.into_any_element()
    }

    fn render_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(doc) = self.active_doc_entity().cloned() else {
            let message = if self.active_notebook.is_some() {
                t!("editor.empty").to_string()
            } else {
                t!("editor.no_notebook").to_string()
            };
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(message)
                .into_any_element();
        };

        if self.preview_mode {
            self.ensure_preview(cx);
            if let Some(cache) = &self.preview_cache {
                let base = doc.read(cx).path.parent().map(|path| path.to_path_buf());
                return preview::render(
                    &cache.ir,
                    base.as_deref(),
                    &self.math,
                    self.config.code_wrap,
                    &self.config.note_font,
                    self.config.note_font_size,
                    cx,
                );
            }
        }

        let editor = doc.read(cx).editor.clone();
        let notebook_root = {
            let doc = doc.read(cx);
            doc.notebook_id.and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
                    .map(|entry| entry.notebook.root.clone())
            })
        };
        let note_path = doc.read(cx).path.clone();
        let host = self.config.image_host.clone();
        let shell = cx.entity().downgrade();

        let paste_root = notebook_root.clone();
        let paste_note = note_path.clone();
        let paste_host = host.clone();
        let paste_shell = shell.clone();
        let paste_doc = doc.clone();

        let drop_root = notebook_root;
        let drop_note = note_path;
        let drop_host = host;
        let drop_shell = shell;
        let drop_doc = doc.clone();

        div()
            .size_full()
            .min_h_0()
            .id("editor-drop")
            .on_drop::<ExternalPaths>(move |paths, window, cx| {
                let Some(root) = drop_root.clone() else {
                    return;
                };
                let images: Vec<PastedImage> = paths
                    .paths()
                    .iter()
                    .filter(|path| ImageKind::from_extension(path).is_some())
                    .map(|path| PastedImage::File(path.clone()))
                    .collect();
                if images.is_empty() {
                    return;
                }
                spawn_image_insert(
                    drop_shell.clone(),
                    drop_doc.clone(),
                    root,
                    drop_note.clone(),
                    drop_host.clone(),
                    images,
                    window,
                    cx,
                );
            })
            .child(
                Editor::new(&editor)
                    .h_full()
                    .w_full()
                    .on_paste(move |item, window, cx| {
                        let Some(root) = paste_root.clone() else {
                            return false;
                        };
                        let images = clipboard_images(item);
                        if images.is_empty() {
                            return false;
                        }
                        spawn_image_insert(
                            paste_shell.clone(),
                            paste_doc.clone(),
                            root,
                            paste_note.clone(),
                            paste_host.clone(),
                            images,
                            window,
                            cx,
                        );
                        true
                    }),
            )
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut status = div()
            .h(px(26.))
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .text_xs()
            .text_color(cx.theme().muted_foreground);

        if let Some(doc) = self.active_doc_entity() {
            let doc = doc.read(cx);
            let text = doc.text(cx);
            let stats = text_stats(&text);
            let saved_label = if doc.dirty {
                t!("status.unsaved").to_string()
            } else {
                t!("status.saved").to_string()
            };
            let notebook_label = if doc.notebook_name.is_empty() {
                t!("status.external").to_string()
            } else {
                doc.notebook_name.clone()
            };
            let (line, column) = doc.cursor(cx);
            let cursor_label =
                t!("status.cursor", line = line + 1, column = column + 1).to_string();
            status = status
                .child(notebook_label)
                .child(cursor_label)
                .child(t!("status.lines", count = stats.lines).to_string())
                .child(t!("status.words", count = stats.words).to_string())
                .child(t!("status.chars", count = stats.chars).to_string());
            let selection = doc.editor.read(cx).selected_value();
            if !selection.trim().is_empty() {
                let selected = text_stats(&selection);
                status = status.child(
                    t!(
                        "status.selection",
                        chars = selected.chars,
                        words = selected.words
                    )
                    .to_string(),
                );
            }
            status = status
                .child(doc.encoding.label())
                .child(doc.eol.label())
                .child(saved_label)
                .child(if self.preview_mode {
                    t!("status.mode.preview").to_string()
                } else {
                    t!("status.mode.edit").to_string()
                });
            if let Some(err) = &doc.error {
                status = status.child(
                    div()
                        .text_color(cx.theme().danger)
                        .child(format!("{}: {err}", t!("status.error"))),
                );
            }
            if doc.autosave {
                status = status.child(t!("status.autosave").to_string());
            }
        } else {
            status = status.child(t!("status.no_doc").to_string());
        }

        let mcp_status: AnyElement = if self.mcp_live {
            div()
                .child(t!("status.mcp.on", port = self.mcp.port).to_string())
                .into_any_element()
        } else if self.config.mcp_http && self.mcp.error.is_some() {
            div()
                .text_color(cx.theme().danger)
                .child(t!("status.mcp.error").to_string())
                .into_any_element()
        } else {
            div()
                .child(t!("status.mcp.off").to_string())
                .into_any_element()
        };
        status = status.child(div().flex_1()).child(mcp_status);

        status.into_any_element()
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self
            .active_doc_entity()
            .map(|doc| doc.read(cx).title())
            .unwrap_or_default();
        let app_name = t!("app.name");
        if title.is_empty() {
            window.set_window_title(&app_name);
        } else {
            window.set_window_title(&format!("{app_name} — {title}"));
        }

        let notice = if self.notice.is_some() {
            Some(self.render_notice(cx))
        } else {
            None
        };

        let sidebar = self.render_sidebar(cx);
        let panel_handle = self.render_panel_handle(cx);
        let editor = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .v_flex()
            .child(self.render_tabs(cx))
            .children(self.render_conflict(cx))
            .child(div().flex_1().min_h_0().child(self.render_editor(cx)))
            .into_any_element();
        let mut main = div().flex_1().min_h_0().flex();
        main = if self.config.panel_dock == PanelDock::Right {
            main.child(editor).child(panel_handle).child(sidebar)
        } else {
            main.child(sidebar).child(panel_handle).child(editor)
        };

        if self.panel_resize.is_some() {
            let shell = cx.entity().downgrade();
            window.on_mouse_event(move |event: &MouseMoveEvent, _phase, _window, cx| {
                let x = f32::from(event.position.x);
                if let Some(shell) = shell.upgrade() {
                    let _ = shell.update(cx, |this, cx| this.update_panel_resize(x, cx));
                }
            });
            let shell = cx.entity().downgrade();
            window.on_mouse_event(move |_event: &MouseUpEvent, _phase, _window, cx| {
                if let Some(shell) = shell.upgrade() {
                    let _ = shell.update(cx, |this, cx| this.end_panel_resize(cx));
                }
            });
        }

        let mut root = div()
            .v_flex()
            .relative()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &crate::SaveDoc, _window, cx| {
                if this.palette.is_some() || this.prompt.is_some() {
                    return;
                }
                this.save_active(cx)
            }))
            .on_action(cx.listener(|this, _: &crate::TogglePreview, window, cx| {
                if this.palette.is_some() || this.prompt.is_some() {
                    return;
                }
                this.toggle_preview(window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::OpenPalette, window, cx| {
                if this.palette.is_some() {
                    this.close_palette(cx);
                    return;
                }
                this.open_palette(window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::NewNote, window, cx| {
                if this.palette.is_some() || this.prompt.is_some() {
                    return;
                }
                this.new_note(window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::ToggleBold, window, cx| {
                if this.palette.is_some() || this.prompt.is_some() {
                    return;
                }
                this.wrap_selection("**", "**", window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::ToggleItalic, window, cx| {
                if this.palette.is_some() || this.prompt.is_some() {
                    return;
                }
                this.wrap_selection("*", "*", window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::InsertLink, window, cx| {
                if this.palette.is_some() || this.prompt.is_some() {
                    return;
                }
                this.wrap_selection("[", "]()", window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::PastePlain, window, cx| {
                if this.palette.is_some() || this.prompt.is_some() || this.preview_mode {
                    return;
                }
                let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
                    return;
                };
                if text.is_empty() {
                    return;
                }
                if let Some(doc) = this.active_doc_entity().cloned() {
                    doc.update(cx, |doc, cx| doc.insert_text(&text, window, cx));
                    cx.notify();
                }
            }))
            .child(gpui_kit::component::TitleBar::new().child(self.render_title(cx)))
            .children(notice)
            .child(main)
            .child(self.render_status(cx));

        if self.export.is_some() {
            root = root.child(self.render_export_dialog(cx));
        }
        if self.zip.is_some() {
            root = root.child(self.render_zip_dialog(cx));
        }
        if self.settings.is_some() {
            root = root.child(self.render_settings(cx));
        }
        if self.recovery.is_some() {
            root = root.child(self.render_recovery(cx));
        }
        if self.prompt.is_some() {
            root = root.child(self.render_prompt(cx));
        }
        if self.palette.is_some() {
            root = root.child(self.render_palette(cx));
        }
        root
    }
}

/// Subsequence match with a bonus for word starts and consecutive runs.
fn fuzzy_score(label: &str, query: &str) -> Option<i64> {
    if query.trim().is_empty() {
        return Some(0);
    }
    let mut score = 0i64;
    let mut pos = 0usize;
    let mut previous: Option<usize> = None;
    for ch in query.chars().filter(|ch| !ch.is_whitespace()) {
        let found = label[pos..].find(ch)?;
        let index = pos + found;
        let boundary = index == 0
            || matches!(
                label.as_bytes().get(index.wrapping_sub(1)),
                Some(b' ' | b'.' | b'-' | b'/' | b':' | b'_')
            );
        score += if boundary { 8 } else { 1 };
        if previous.is_some_and(|last| index == last + 1) {
            score += 4;
        }
        previous = Some(index);
        pos = index + ch.len_utf8();
    }
    Some(score)
}

fn parse_font_size(raw: &str, fallback: f32) -> f32 {
    raw.trim()
        .parse::<f32>()
        .ok()
        .filter(|size| size.is_finite())
        .map(|size| size.clamp(10.0, 32.0))
        .unwrap_or(fallback.clamp(10.0, 32.0))
}

fn format_pt(value: f32) -> String {
    if (value - value.round()).abs() < f32::EPSILON {
        format!("{}", value.round() as i32)
    } else {
        format!("{value:.1}")
    }
}

fn format_age(seconds: u64) -> String {
    if seconds < 60 {
        t!("recovery.age_now").to_string()
    } else if seconds < 3600 {
        t!("recovery.age_minutes", count = seconds / 60).to_string()
    } else if seconds < 86_400 {
        t!("recovery.age_hours", count = seconds / 3600).to_string()
    } else {
        t!("recovery.age_days", count = seconds / 86_400).to_string()
    }
}

fn mcp_field(label: String, control: AnyElement, muted: gpui_kit::Hsla) -> AnyElement {
    div()
        .v_flex()
        .gap_1()
        .child(div().text_xs().text_color(muted).child(label))
        .child(control)
        .into_any_element()
}

fn empty_hint(text: String, color: gpui_kit::Hsla) -> AnyElement {
    div()
        .flex_1()
        .p_3()
        .text_sm()
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn title_icon(id: &'static str, icon: IconName, tooltip: String) -> Button {
    Button::new(id)
        .ghost()
        .compact()
        .icon(icon)
        .tooltip(tooltip)
}

fn write_math_cache(key: u64, svg: &str) -> Option<PathBuf> {
    let dir = mdwork_core::paths::cache_dir().join("math");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let path = dir.join(format!("{key:016x}.svg"));
    mdwork_core::atomic::write_atomic(&path, svg.as_bytes())
        .ok()
        .map(|_| path)
}

fn seed_workspace(workspace: &Workspace) -> mdwork_core::Result<()> {
    let notebook = workspace.create_notebook("我的笔记本")?;
    notebook.create_note("欢迎使用墨笺.md", WELCOME_NOTE)?;
    Ok(())
}

pub(crate) fn cli_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| t!("export.missing_cli").to_string())?;
    let candidate = dir.join(format!("md-work-cli{}", std::env::consts::EXE_SUFFIX));
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(t!("export.missing_cli").to_string())
    }
}

fn ensure_export_extension(path: PathBuf, format: ExportFormat) -> PathBuf {
    let extension = format.extension();
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|current| current.eq_ignore_ascii_case(extension))
    {
        path
    } else {
        path.with_extension(extension)
    }
}

fn ensure_zip_extension(path: PathBuf) -> PathBuf {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|current| current.eq_ignore_ascii_case("zip"))
    {
        path
    } else {
        path.with_extension("zip")
    }
}

fn unique_zip_path(dir: &Path, stem: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{stem}.zip"));
    let mut counter = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({counter}).zip"));
        counter += 1;
    }
    candidate
}

fn subscribe_zip_path(cx: &mut Context<AppShell>, input: &Entity<InputState>) {
    cx.subscribe(input, |this, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            this.schedule_zip_plan(cx);
        }
    })
    .detach();
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.2} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

enum PastedImage {
    Bytes { bytes: Vec<u8>, kind: ImageKind },
    File(PathBuf),
}

struct StoredImages {
    markdown: String,
    failed_uploads: Vec<String>,
    error: Option<String>,
}

/// Picks the image payload out of a paste event: bitmap data wins over files,
/// and non-image files fall through to the default text paste.
fn clipboard_images(item: &ClipboardItem) -> Vec<PastedImage> {
    let mut bitmaps = Vec::new();
    let mut files = Vec::new();
    for entry in &item.entries {
        match entry {
            ClipboardEntry::Image(image) => {
                let kind = match image.format {
                    ImageFormat::Png => Some(ImageKind::Png),
                    ImageFormat::Jpeg => Some(ImageKind::Jpeg),
                    ImageFormat::Gif => Some(ImageKind::Gif),
                    ImageFormat::Webp => Some(ImageKind::Webp),
                    ImageFormat::Bmp => Some(ImageKind::Bmp),
                    ImageFormat::Svg => Some(ImageKind::Svg),
                    _ => ImageKind::sniff(&image.bytes),
                };
                if let Some(kind) = kind {
                    if image.bytes.len() as u64 <= image::IMAGE_MAX_BYTES {
                        bitmaps.push(PastedImage::Bytes {
                            bytes: image.bytes.clone(),
                            kind,
                        });
                    }
                }
            }
            ClipboardEntry::ExternalPaths(paths) => {
                for path in paths.paths() {
                    if ImageKind::from_extension(path).is_some() {
                        files.push(PastedImage::File(path.clone()));
                    }
                }
            }
            ClipboardEntry::String(_) => {}
        }
    }
    if bitmaps.is_empty() { files } else { bitmaps }
}

fn store_images(
    notebook_root: &Path,
    note_path: &Path,
    host: &ImageHost,
    images: Vec<PastedImage>,
) -> StoredImages {
    let mut links = Vec::new();
    let mut failed_uploads = Vec::new();
    let mut error = None;
    for image in images {
        let (bytes, kind) = match image {
            PastedImage::Bytes { bytes, kind } => (bytes, kind),
            PastedImage::File(path) => {
                let Some(kind) = ImageKind::from_extension(&path) else {
                    continue;
                };
                match std::fs::read(&path) {
                    Ok(bytes) => (bytes, kind),
                    Err(err) => {
                        error = Some(format!("{}: {err}", path.display()));
                        continue;
                    }
                }
            }
        };
        let asset = match image::save_asset(notebook_root, &bytes, kind) {
            Ok(path) => path,
            Err(err) => {
                error = Some(err.to_string());
                continue;
            }
        };
        let mut url = image::markdown_url(note_path, &asset);
        if !host.is_local() {
            match image::upload(host, &asset, &bytes, kind) {
                Ok(remote) => {
                    let _ = std::fs::remove_file(&asset);
                    url = remote;
                }
                Err(err) => failed_uploads.push(err.to_string()),
            }
        }
        links.push(format!("![]({url})"));
    }
    StoredImages {
        markdown: links.join("\n"),
        failed_uploads,
        error,
    }
}

fn spawn_image_insert(
    shell: WeakEntity<AppShell>,
    doc: Entity<Doc>,
    notebook_root: PathBuf,
    note_path: PathBuf,
    host: ImageHost,
    images: Vec<PastedImage>,
    window: &mut Window,
    cx: &mut App,
) {
    let doc = doc.downgrade();
    let task = window.spawn(cx, async move |cx| {
        let stored = cx
            .background_executor()
            .spawn(async move { store_images(&notebook_root, &note_path, &host, images) })
            .await;
        let notice = if let Some(error) = &stored.error {
            Some(t!("image.failed", message = error.clone()).to_string())
        } else {
            stored
                .failed_uploads
                .first()
                .map(|error| t!("image.upload_failed", message = error.clone()).to_string())
        };
        let markdown = stored.markdown;
        if !markdown.is_empty() {
            let _ = doc.update_in(cx, |doc, window, cx| doc.insert_text(&markdown, window, cx));
        }
        let _ = shell.update(cx, |this, cx| {
            if let Some(notice) = notice {
                this.notice = Some(notice);
            }
            cx.notify();
        });
    });
    task.detach();
}

fn stage_label(stage: &str) -> String {
    match stage {
        "read" => t!("export.stage.read").to_string(),
        "compile" => t!("export.stage.compile").to_string(),
        "write" => t!("export.stage.write").to_string(),
        "render" => t!("export.stage.render").to_string(),
        other => other.to_string(),
    }
}

fn is_atomic_temp(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') && name.ends_with(".tmp"))
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

fn path_under(rel: &str, prefix: &str) -> bool {
    if rel.eq_ignore_ascii_case(prefix) {
        return true;
    }
    rel.len() > prefix.len()
        && rel.as_bytes()[prefix.len()] == b'/'
        && rel[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn token_store() -> TokenStore {
    TokenStore::new(TokenStore::default_path())
}

fn split_globs(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c == ';' || c == '\n' || c == '\r')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn load_audit_entries() -> (Vec<AuditEntry>, Option<String>) {
    match AuditLog::new(AuditLog::default_dir()).recent(100) {
        Ok(entries) => (entries, None),
        Err(err) => (
            Vec::new(),
            Some(t!("mcp.audit_error", message = err.to_string()).to_string()),
        ),
    }
}

fn scope_label(scope: Scope) -> String {
    let key = match scope {
        Scope::Read => "mcp.scope.read",
        Scope::Write => "mcp.scope.write",
        Scope::Search => "mcp.scope.search",
        Scope::Delete => "mcp.scope.delete",
        Scope::DeletePermanent => "mcp.scope.delete_permanent",
        Scope::Export => "mcp.scope.export",
        Scope::Admin => "mcp.scope.admin",
    };
    t!(key).to_string()
}

fn age_label(deleted_at: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(deleted_at);
    let elapsed = now.saturating_sub(deleted_at);
    if elapsed < 60 {
        t!("trash.just_now").to_string()
    } else if elapsed < 3600 {
        t!("trash.minutes_ago", n = elapsed / 60).to_string()
    } else if elapsed < 86_400 {
        t!("trash.hours_ago", n = elapsed / 3600).to_string()
    } else {
        t!("trash.days_ago", n = elapsed / 86_400).to_string()
    }
}

fn refresh_tree(entry: &mut NotebookEntry) {
    entry.tree = entry.notebook.list_tree().unwrap_or_default();
    entry.rows = build_rows(&entry.tree, &entry.expanded);
}

fn build_rows(tree: &[Entry], expanded: &HashSet<String>) -> Vec<TreeRow> {
    let mut by_parent: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for item in tree {
        let parent = match item.rel_path.rfind('/') {
            Some(index) => &item.rel_path[..index],
            None => "",
        };
        by_parent.entry(parent).or_default().push(item);
    }
    for items in by_parent.values_mut() {
        items.sort_by(|a, b| {
            let a_folder = a.kind == EntryKind::Folder;
            let b_folder = b.kind == EntryKind::Folder;
            b_folder
                .cmp(&a_folder)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }

    let mut rows = Vec::with_capacity(tree.len());
    push_rows("", 0, &by_parent, expanded, &mut rows);
    rows
}

fn push_rows(
    parent: &str,
    depth: usize,
    by_parent: &BTreeMap<&str, Vec<&Entry>>,
    expanded: &HashSet<String>,
    rows: &mut Vec<TreeRow>,
) {
    let Some(items) = by_parent.get(parent) else {
        return;
    };
    for item in items {
        let is_folder = item.kind == EntryKind::Folder;
        let has_children = is_folder && by_parent.contains_key(item.rel_path.as_str());
        let is_open = has_children && expanded.contains(&item.rel_path);
        rows.push(TreeRow {
            rel_path: item.rel_path.clone(),
            name: item.name.clone(),
            kind: item.kind,
            depth,
            expanded: is_open,
        });
        if is_open {
            push_rows(&item.rel_path, depth + 1, by_parent, expanded, rows);
        }
    }
}
