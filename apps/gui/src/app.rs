use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{
    Editor, Escape as InputEscape, Input, InputEvent, InputState, Textarea, TextareaState,
};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdwork_core::archive::{self, Conflict, ConflictPolicy, ImportMode, ImportOptions, ImportPlan};
use mdwork_core::bookmarks::Bookmarks;
use mdwork_core::config::{
    AppConfig, CursorPos, NotebookSession, PanelArrangement, PanelDock, PanelKind, Session,
    ThemePreference,
};
use mdwork_core::doc::{Eol, TextEncoding};
use mdwork_core::image::{self, ImageHost, ImageKind};
use mdwork_core::notebook::{Entry, EntryKind, Notebook, TrashItem};
use mdwork_core::permissions::{self as perms, Scope, TokenRecord};
use mdwork_core::search::{self, SearchHit, SearchQuery};
use mdwork_core::text::text_stats;
use mdwork_core::workspace_registry::WorkspaceRegistry;
use mdwork_core::{FolderKind, Workspace};
use mdwork_export::{
    ExportEvent, ExportFormat, ExportJob, ExportOptions, ExportRequest, NotebookPrefs, PaperSize,
};
use mdwork_mcp::{AuditEntry, AuditLog, TokenStore};
use mdwork_render::preview::{OutlineItem, PreviewIr};
use rust_i18n::t;
use uuid::Uuid;

use crate::doc::{DiskConflict, Doc, DocEvent, DocOrigin, OpenRequest};
use crate::{disk, preview, theme};

mod bookmarks;
mod documents;
mod export;
mod files;
mod image_settings;
mod lifecycle;
mod mcp_settings;
mod notifications;
mod palette;
mod panels;
mod previewer;
mod prompt;
mod render;
mod searcher;
mod settings;
mod util;
mod workspace;
mod zip;

use util::*;

const PANEL_MIN_WIDTH: f32 = 240.0;
const PANEL_MAX_WIDTH: f32 = 560.0;

const WELCOME_NOTE: &str = r#"# 欢迎使用墨笺

墨笺是一个轻量的 Markdown 笔记本：笔记就是磁盘上的 `.md` 文件，可以直接用资源管理器管理。

## 工作区

工作区是磁盘上的一个普通文件夹，默认是「我的文档」下的 `md-work`。一个工作区里可以放多个笔记本，每个笔记本用子目录保存笔记。

- 点击标题栏左上角的工作区名称，可以在多个工作区之间切换
- 「选择文件夹…」可以把任意文件夹作为工作区，也可以把导出的笔记本 zip 导入进来
- 切换工作区不会移动或删除任何文件；下次启动会自动打开上次的工作区、笔记本和笔记
- 在「设置 → 工作区」里可以给工作区起一个别名，文件夹本身的名称不变

## 已经具备

- 每个子目录都是一个笔记本，复制进工作区即可自动识别
- 打开多个笔记本，每个笔记本记住自己的标签页
- UTF-8 / GB18030 等编码往返保存
- Markdown 语法高亮与软换行
- 自动保存（停止输入 800 毫秒后）
- 标题栏通知中心：普通消息、警告和错误分色显示，可全部已读或清空

## 接下来

选中左侧的笔记开始编辑。按 `Ctrl+S` 立即保存。
"#;

struct NotebookEntry {
    notebook: Notebook,
    tree: Vec<Entry>,
    expanded: HashSet<String>,
    rows: Vec<TreeRow>,
    /// Whether this notebook root is expanded in the unified sidebar tree.
    root_expanded: bool,
}

#[derive(Clone)]
struct TreeRow {
    rel_path: String,
    name: String,
    kind: EntryKind,
    depth: usize,
    expanded: bool,
    created: u64,
    modified: u64,
    note_count: usize,
}

/// Flat virtual-list row for the unified notebook tree (notebook roots + entries).
#[derive(Clone)]
enum SidebarRow {
    Notebook {
        id: Uuid,
        name: String,
        note_count: usize,
        expanded: bool,
    },
    Entry {
        notebook_id: Uuid,
        row: TreeRow,
    },
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

impl SidePanel {
    const ALL: [SidePanel; 5] = [
        SidePanel::Notes,
        SidePanel::Outline,
        SidePanel::Search,
        SidePanel::Bookmarks,
        SidePanel::Trash,
    ];

    fn kind(self) -> PanelKind {
        match self {
            SidePanel::Notes => PanelKind::Notes,
            SidePanel::Outline => PanelKind::Outline,
            SidePanel::Search => PanelKind::Search,
            SidePanel::Bookmarks => PanelKind::Bookmarks,
            SidePanel::Trash => PanelKind::Trash,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NoticeKind {
    Info,
    Success,
    Warning,
    Error,
}

struct NoticeItem {
    kind: NoticeKind,
    text: String,
    read: bool,
    created: std::time::Instant,
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
    Workspace,
    Appearance,
    Panels,
    Images,
    Mcp,
    Windows,
}

struct SettingsDialog {
    tab: SettingsTab,
    workspace_name: Entity<InputState>,
    ui_font: Entity<InputState>,
    note_font: Entity<InputState>,
    ui_size: Entity<InputState>,
    note_size: Entity<InputState>,
}

#[derive(Clone)]
enum PromptKind {
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

/// Confirmation shown before opening a folder that is not empty and is not a
/// md-work workspace (requirement: default is "no", the user must opt in).
struct WorkspaceConfirm {
    path: PathBuf,
    kind: FolderKind,
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
    /// Merged notebook roots + entry rows for the Notes sidebar `uniform_list`.
    sidebar_rows: Vec<SidebarRow>,
    /// Row index whose trailing `…` dropdown is open (keeps hover actions visible).
    sidebar_row_menu: Option<usize>,
    active_notebook: Option<Uuid>,
    docs: Vec<Entity<Doc>>,
    active_doc: Option<usize>,
    notices: Vec<NoticeItem>,
    notice_panel: bool,
    workspace_registry: WorkspaceRegistry,
    workspace_menu: bool,
    /// Workspace roots that were missing on the last presence probe; the
    /// switcher must not hit the filesystem while rendering.
    missing_workspaces: HashSet<PathBuf>,
    pending: Arc<Mutex<Vec<String>>>,
    changed: Option<Arc<Mutex<Receiver<Vec<PathBuf>>>>>,
    window_handle: gpui_kit::AnyWindowHandle,
    tree_scroll: UniformListScrollHandle,
    /// Briefly highlight this notebook entry after reveal/scroll.
    flash_rel: Option<(Uuid, String)>,
    preview_mode: bool,
    preview_cache: Option<PreviewCache>,
    math: HashMap<u64, Option<PathBuf>>,
    math_pending: HashSet<u64>,
    image_paths: HashMap<u64, Option<PathBuf>>,
    images_pending: HashSet<u64>,
    copied: bool,
    /// Active panel of the left sidebar (only panels docked left are shown).
    left_panel: SidePanel,
    /// Active panel of the right sidebar (only panels docked right are shown).
    right_panel: SidePanel,
    /// (dock being dragged, mouse x at drag start, that dock's width at drag start)
    panel_resize: Option<(PanelDock, f32, f32)>,
    outline: Vec<OutlineItem>,
    outline_cache: Option<(EntityId, u64)>,
    bookmarks: Bookmarks,
    bookmarks_notebook: Option<Uuid>,
    bookmarks_loading: Option<Uuid>,
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
    renaming_notebook: Option<(Uuid, Entity<InputState>)>,
    trash: Vec<TrashItem>,
    trash_notebook: Option<Uuid>,
    export: Option<ExportDialog>,
    zip: Option<ZipDialog>,
    image_settings: Option<ImageSettingsDialog>,
    mcp_settings: Option<McpSettingsDialog>,
    settings: Option<SettingsDialog>,
    prompt: Option<PromptDialog>,
    workspace_confirm: Option<WorkspaceConfirm>,
    palette: Option<Palette>,
    recovery: Option<RecoveryDialog>,
    scan_issues: Vec<String>,
    dirty: crate::dirty::DirtyRegistry,
    mcp: crate::mcp_server::McpHttp,
    mcp_live: bool,
    selection_sig: Option<(usize, usize)>,
    initializing: bool,
    reload_generation: u64,
    entries_generation: u64,
    trash_generation: u64,
    switch_generation: u64,
    opening: HashSet<PathBuf>,
    pending_close: HashSet<EntityId>,
    pending_delete: Option<(Uuid, String)>,
    pending_jump: Option<(PathBuf, u32)>,
    pending_recovery: Option<mdwork_core::recovery::Snapshot>,
    /// Paths (CLI single-instance forwards) that arrived before the first
    /// workspace scan finished.
    deferred_paths: Vec<String>,
    pending_extra_paths: Vec<String>,
    pending_startup_path: Option<String>,
    startup_first_run: bool,
    pending_welcome: Option<(Uuid, String)>,
    /// CLI executable resolved once at startup; rendering and exporting must
    /// not stat the filesystem.
    cli: Result<PathBuf, String>,
    _watcher: Option<mdwork_core::watch::WorkspaceWatcher>,
    _appearance_subscription: Option<Subscription>,
}

/// Everything that must be read before the first frame. Built in `main` before
/// the event loop starts, so the window appears with the right theme and does
/// not scan the workspace while the UI is already responding.
pub struct Startup {
    config: AppConfig,
    registry: WorkspaceRegistry,
    root: PathBuf,
    root_exists: bool,
    session: Session,
    notices: Vec<(NoticeKind, String)>,
    config_existed: bool,
    cli: Result<PathBuf, String>,
}

impl Startup {
    pub fn load() -> Self {
        let config_existed = mdwork_core::paths::config_path().is_file();
        let mut config = AppConfig::load_or_default();
        if config.workspace_root.trim().is_empty() {
            if let Some(root) = crate::install_hint::first_run_workspace() {
                config.workspace_root = root;
            }
        }
        config.panel_width = if config.panel_width.is_finite() {
            config.panel_width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH)
        } else {
            300.0
        };
        for dock in [PanelDock::Left, PanelDock::Right] {
            let width = config.width_for_dock(dock);
            if width > 0.0 {
                config.set_width_for_dock(
                    dock,
                    if width.is_finite() {
                        width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH)
                    } else {
                        0.0
                    },
                );
            }
        }
        rust_i18n::set_locale(&config.language);

        let registry = WorkspaceRegistry::load();
        let registry_existed = mdwork_core::paths::workspaces_path().is_file();
        let configured = config
            .resolve_workspace_root()
            .unwrap_or_else(|_| PathBuf::from("md-work"));
        let mut notices = Vec::new();
        let root = match registry.active_path() {
            Some(active) if active.is_dir() => active,
            Some(active) => {
                notices.push((
                    NoticeKind::Warning,
                    format!(
                        "上次使用的工作区已不可用：{}，已回到默认位置",
                        active.display()
                    ),
                ));
                configured
            }
            None => configured,
        };
        let root_exists = root.exists();
        let session = load_workspace_session(&root, !registry_existed);
        let cli = cli_path();
        if let Some(summary) = config.issue_summary() {
            notices.push((NoticeKind::Warning, summary));
        }
        Self {
            config,
            registry,
            root,
            root_exists,
            session,
            notices,
            config_existed,
            cli,
        }
    }

    /// Uploads pending crash records once, off the UI thread.
    pub fn report_crashes(&self) {
        let config = self.config.crash_report.clone();
        std::thread::spawn(move || {
            let summary = mdwork_core::crash::report_pending(&config);
            tracing::info!(
                reported = summary.reported,
                failed = summary.failed,
                skipped = %summary.skipped,
                "崩溃日志上报"
            );
        });
    }
}

/// Result of the first background workspace scan.
struct Initialized {
    root: PathBuf,
    workspace: Option<Workspace>,
    failures: Vec<(NoticeKind, String)>,
    entries: Vec<NotebookEntry>,
    issues: Vec<String>,
    snapshots: Vec<mdwork_core::recovery::Snapshot>,
}

struct Switched {
    root: PathBuf,
    workspace: Option<Workspace>,
    failures: Vec<(NoticeKind, String)>,
    entries: Vec<NotebookEntry>,
    issues: Vec<String>,
    session: Session,
}

/// Outcome of re-reading a file that changed on disk.
enum DiskRead {
    Missing,
    Skip,
    Changed(mdwork_core::doc::DecodedDoc, String),
}

impl AppShell {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        pending: Arc<Mutex<Vec<String>>>,
        initial_path: Option<String>,
        startup: Startup,
    ) -> Self {
        let Startup {
            config,
            registry,
            root,
            root_exists,
            session,
            notices: startup_notices,
            config_existed,
            cli,
        } = startup;

        theme::apply(&config, window, cx);

        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("search.placeholder").to_string())
                .clean_on_escape()
        });

        let mcp_port = config.mcp_port;
        let mcp_enabled = config.mcp_http;

        let mut this = Self {
            config,
            session,
            workspace: None,
            notebooks: Vec::new(),
            sidebar_rows: Vec::new(),
            sidebar_row_menu: None,
            active_notebook: None,
            docs: Vec::new(),
            active_doc: None,
            notices: startup_notices
                .into_iter()
                .map(|(kind, text)| NoticeItem {
                    kind,
                    text,
                    read: false,
                    created: Instant::now(),
                })
                .collect(),
            notice_panel: false,
            workspace_registry: registry,
            workspace_menu: false,
            missing_workspaces: HashSet::new(),
            pending,
            changed: None,
            window_handle: window.window_handle(),
            tree_scroll: UniformListScrollHandle::new(),
            flash_rel: None,
            preview_mode: false,
            preview_cache: None,
            math: HashMap::new(),
            math_pending: HashSet::new(),
            image_paths: HashMap::new(),
            images_pending: HashSet::new(),
            copied: false,
            left_panel: SidePanel::Notes,
            right_panel: SidePanel::Notes,
            panel_resize: None,
            outline: Vec::new(),
            outline_cache: None,
            bookmarks: Bookmarks::default(),
            bookmarks_notebook: None,
            bookmarks_loading: None,
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
            renaming_notebook: None,
            trash: Vec::new(),
            trash_notebook: None,
            export: None,
            zip: None,
            image_settings: None,
            mcp_settings: None,
            settings: None,
            prompt: None,
            workspace_confirm: None,
            palette: None,
            recovery: None,
            scan_issues: Vec::new(),
            dirty: crate::dirty::DirtyRegistry::new(),
            mcp: crate::mcp_server::McpHttp::new(mcp_port),
            mcp_live: false,
            selection_sig: None,
            initializing: true,
            reload_generation: 0,
            entries_generation: 0,
            trash_generation: 0,
            switch_generation: 0,
            opening: HashSet::new(),
            pending_close: HashSet::new(),
            pending_delete: None,
            pending_jump: None,
            pending_recovery: None,
            deferred_paths: Vec::new(),
            pending_extra_paths: Vec::new(),
            pending_startup_path: initial_path,
            startup_first_run: !root_exists,
            pending_welcome: None,
            cli,
            _watcher: None,
            _appearance_subscription: None,
        };

        // Always materialise config/runtime.toml on first run so later installer
        // upgrades never rely on, or overwrite, the first-run registry hint.
        let root_text = root.display().to_string();
        if !config_existed || this.config.workspace_root != root_text {
            this.config.set_workspace_root(&root);
            this.save_config(cx);
        }

        cx.subscribe(&this.search_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.schedule_search(cx);
            }
        })
        .detach();

        this.start_initialization(root, root_exists, cx);
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

        self.ensure_bookmarks(cx);
        let has_left = !self.panels_in_dock(PanelDock::Left).is_empty();
        let has_right = !self.panels_in_dock(PanelDock::Right).is_empty();
        // The workspace-path footer lives on the left sidebar, or on the right
        // one when nothing is docked left, so it is never shown twice.
        let left_sidebar = if has_left {
            self.render_dock(PanelDock::Left, true, cx)
        } else {
            None
        };
        let right_sidebar = if has_right {
            self.render_dock(PanelDock::Right, !has_left, cx)
        } else {
            None
        };
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
        if let Some(sidebar) = left_sidebar {
            main = main
                .child(sidebar)
                .child(self.render_panel_handle(PanelDock::Left, cx));
        }
        main = main.child(editor);
        if let Some(sidebar) = right_sidebar {
            main = main
                .child(self.render_panel_handle(PanelDock::Right, cx))
                .child(sidebar);
        }

        let mut root = div()
            .v_flex()
            .relative()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            // Input Escape propagates when clean_on_escape is off; cancel
            // in-place rename / notebook rename instead of leaving focus stuck.
            .on_action(cx.listener(|this, _: &InputEscape, _window, cx| {
                if this.renaming.is_some() {
                    this.cancel_rename(cx);
                } else if this.renaming_notebook.is_some() {
                    this.cancel_notebook_rename(cx);
                }
            }))
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
            .child(main)
            .child(self.render_status(cx));

        if self.panel_resize.is_some() {
            root = root.child(self.render_panel_resize_listeners(cx));
        }

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
        if self.workspace_confirm.is_some() {
            root = root.child(self.render_workspace_confirm(cx));
        }
        if self.palette.is_some() {
            root = root.child(self.render_palette(cx));
        }
        root
    }
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

enum PastedImage {
    Bytes { bytes: Vec<u8>, kind: ImageKind },
    File(PathBuf),
}

struct StoredImages {
    markdown: String,
    failed_uploads: Vec<String>,
    error: Option<String>,
}
