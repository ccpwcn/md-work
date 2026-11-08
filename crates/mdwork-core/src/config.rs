//! Application configuration, split into three user-readable TOML files.
//!
//! * `core.toml` — `format`, workspace root, language, MCP port. Marked as
//!   read-only for users because mistakes there break startup assumptions.
//! * `runtime.toml` — editable runtime preferences (autosave, panels, image
//!   host, crash reporting).
//! * `theme.toml` — appearance: mode, fonts, colour overrides.
//!
//! A legacy single-file `config.json` is migrated once. Individual keys are
//! parsed by hand so a broken value only disables that key (and is reported in
//! [`AppConfig::issues`]) instead of discarding the whole configuration.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::image::ImageHost;
use crate::paths::{
    CORE_CONFIG_FILE, THEME_CONFIG_FILE, config_path, core_config_path, default_workspace_root,
    legacy_config_path, runtime_config_path, session_path, theme_config_path,
};
use crate::toml_io;
use crate::{Error, Result};

pub const CONFIG_FORMAT: &str = "md-work/config@1";
pub const SESSION_FORMAT: &str = "md-work/session@1";
pub const DEFAULT_CRASH_REPO: &str = "ccpwcn/md-work";
pub const DEFAULT_GITEE_API: &str = "https://gitee.com/api/v5";
/// Environment variable that overrides the crash-report token from disk.
pub const CRASH_TOKEN_ENV: &str = "MDWORK_GITEE_TOKEN";
pub const CORE_CONFIG_WARNING: &str = "此配置不可修改，否则可能导致软件运行异常！";

const PANEL_WIDTH_MIN: f32 = 200.0;
const PANEL_WIDTH_MAX: f32 = 800.0;
const FONT_SIZE_MIN: f32 = 8.0;
const FONT_SIZE_MAX: f32 = 48.0;
const WINDOW_AXIS_MIN: f32 = -32_768.0;
const WINDOW_AXIS_MAX: f32 = 32_768.0;
const WINDOW_EXTENT_MIN: f32 = 1.0;
const WINDOW_EXTENT_MAX: f32 = 16_384.0;

const CORE_HEADER: &str = "\
# =====================================================================
#  墨笺 md-work · 核心配置（core.toml）
#
#  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
#  此配置不可修改，否则可能导致软件运行异常！
#  !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
#
#  请勿手工编辑本文件；如需调整，请使用软件内的“设置”。
# =====================================================================
";

const RUNTIME_HEADER: &str = "\
# =====================================================================
#  墨笺 md-work · 运行配置（runtime.toml）
#
#  保存界面与运行时偏好，可以手动编辑；删除后软件会自动重建。
#  修改后重新启动软件即可生效。
# =====================================================================
";

const THEME_HEADER: &str = "\
# =====================================================================
#  墨笺 md-work · 主题配置（theme.toml）
#
#  控制主题模式、字体与颜色，可以手动编辑；删除后软件会自动重建。
# =====================================================================
";

const SESSION_HEADER: &str = "\
# 墨笺 md-work · 会话状态（自动生成，可安全删除）
";

const COLOR_KEYS: [&str; 13] = [
    "background",
    "foreground",
    "primary",
    "primary_foreground",
    "accent",
    "accent_foreground",
    "border",
    "muted",
    "muted_foreground",
    "sidebar",
    "sidebar_foreground",
    "popover",
    "popover_foreground",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreference {
    Light,
    Dark,
    #[default]
    System,
}

impl ThemePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            ThemePreference::Light => "light",
            ThemePreference::Dark => "dark",
            ThemePreference::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelDock {
    #[default]
    Left,
    Right,
}

impl PanelDock {
    pub fn as_str(self) -> &'static str {
        match self {
            PanelDock::Left => "left",
            PanelDock::Right => "right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelArrangement {
    #[default]
    Horizontal,
    Vertical,
}

impl PanelArrangement {
    pub fn as_str(self) -> &'static str {
        match self {
            PanelArrangement::Horizontal => "horizontal",
            PanelArrangement::Vertical => "vertical",
        }
    }
}

/// Whether the main window is restored or maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowMode {
    #[default]
    Windowed,
    Maximized,
}

impl WindowMode {
    pub fn as_str(self) -> &'static str {
        match self {
            WindowMode::Windowed => "windowed",
            WindowMode::Maximized => "maximized",
        }
    }
}

/// Normal-window rectangle plus maximized state.
///
/// `x`, `y`, `width` and `height` are logical pixels of the restored window.
/// While maximized they are the rectangle the window returns to, not the
/// screen-sized frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub state: WindowMode,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WindowPlacement {
    /// Rounds to whole logical pixels. Returns `None` when a value is
    /// non-finite or outside the range persisted in `runtime.toml`.
    pub fn new(state: WindowMode, x: f32, y: f32, width: f32, height: f32) -> Option<Self> {
        let placement = Self {
            state,
            x: x.round(),
            y: y.round(),
            width: width.round(),
            height: height.round(),
        };
        let axis = (WINDOW_AXIS_MIN..=WINDOW_AXIS_MAX).contains(&placement.x)
            && (WINDOW_AXIS_MIN..=WINDOW_AXIS_MAX).contains(&placement.y);
        let extent = (WINDOW_EXTENT_MIN..=WINDOW_EXTENT_MAX).contains(&placement.width)
            && (WINDOW_EXTENT_MIN..=WINDOW_EXTENT_MAX).contains(&placement.height);
        (axis && extent).then_some(placement)
    }
}

/// The five sidebar panels that can be docked independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelKind {
    Notes,
    Outline,
    Search,
    Bookmarks,
    Trash,
}

impl PanelKind {
    pub const ALL: [PanelKind; 5] = [
        PanelKind::Notes,
        PanelKind::Outline,
        PanelKind::Search,
        PanelKind::Bookmarks,
        PanelKind::Trash,
    ];

    pub fn key(self) -> &'static str {
        match self {
            PanelKind::Notes => "notes",
            PanelKind::Outline => "outline",
            PanelKind::Search => "search",
            PanelKind::Bookmarks => "bookmarks",
            PanelKind::Trash => "trash",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim();
        Self::ALL.into_iter().find(|panel| panel.key() == key)
    }
}

/// Per-panel docking. `panel_dock` on [`AppConfig`] is the uniform fallback
/// used when every panel shares the same side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelDocks {
    pub notes: PanelDock,
    pub outline: PanelDock,
    pub search: PanelDock,
    pub bookmarks: PanelDock,
    pub trash: PanelDock,
}

impl Default for PanelDocks {
    fn default() -> Self {
        Self::uniform(PanelDock::Left)
    }
}

impl PanelDocks {
    pub fn uniform(dock: PanelDock) -> Self {
        Self {
            notes: dock,
            outline: dock,
            search: dock,
            bookmarks: dock,
            trash: dock,
        }
    }

    pub fn get(&self, panel: PanelKind) -> PanelDock {
        match panel {
            PanelKind::Notes => self.notes,
            PanelKind::Outline => self.outline,
            PanelKind::Search => self.search,
            PanelKind::Bookmarks => self.bookmarks,
            PanelKind::Trash => self.trash,
        }
    }

    pub fn set(&mut self, panel: PanelKind, dock: PanelDock) {
        match panel {
            PanelKind::Notes => self.notes = dock,
            PanelKind::Outline => self.outline = dock,
            PanelKind::Search => self.search = dock,
            PanelKind::Bookmarks => self.bookmarks = dock,
            PanelKind::Trash => self.trash = dock,
        }
    }

    pub fn all(&self) -> [PanelDock; 5] {
        [
            self.notes,
            self.outline,
            self.search,
            self.bookmarks,
            self.trash,
        ]
    }

    /// Returns the shared dock when all panels agree.
    pub fn uniform_value(&self) -> Option<PanelDock> {
        let all = self.all();
        let first = all[0];
        all.iter().all(|dock| *dock == first).then_some(first)
    }
}

/// Colour overrides for the built-in light and dark themes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeColors {
    pub light: ColorOverrides,
    pub dark: ColorOverrides,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorOverrides {
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub primary: Option<String>,
    pub primary_foreground: Option<String>,
    pub accent: Option<String>,
    pub accent_foreground: Option<String>,
    pub border: Option<String>,
    pub muted: Option<String>,
    pub muted_foreground: Option<String>,
    pub sidebar: Option<String>,
    pub sidebar_foreground: Option<String>,
    pub popover: Option<String>,
    pub popover_foreground: Option<String>,
}

impl ColorOverrides {
    fn slot(&mut self, key: &str) -> Option<&mut Option<String>> {
        Some(match key {
            "background" => &mut self.background,
            "foreground" => &mut self.foreground,
            "primary" => &mut self.primary,
            "primary_foreground" => &mut self.primary_foreground,
            "accent" => &mut self.accent,
            "accent_foreground" => &mut self.accent_foreground,
            "border" => &mut self.border,
            "muted" => &mut self.muted,
            "muted_foreground" => &mut self.muted_foreground,
            "sidebar" => &mut self.sidebar,
            "sidebar_foreground" => &mut self.sidebar_foreground,
            "popover" => &mut self.popover,
            "popover_foreground" => &mut self.popover_foreground,
            _ => return None,
        })
    }

    fn entries(&self) -> [(&'static str, &Option<String>); 13] {
        [
            ("background", &self.background),
            ("foreground", &self.foreground),
            ("primary", &self.primary),
            ("primary_foreground", &self.primary_foreground),
            ("accent", &self.accent),
            ("accent_foreground", &self.accent_foreground),
            ("border", &self.border),
            ("muted", &self.muted),
            ("muted_foreground", &self.muted_foreground),
            ("sidebar", &self.sidebar),
            ("sidebar_foreground", &self.sidebar_foreground),
            ("popover", &self.popover),
            ("popover_foreground", &self.popover_foreground),
        ]
    }
}

/// Automatic crash reporting through the Gitee issue API.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrashReportConfig {
    pub enabled: bool,
    /// `owner/repo` on Gitee.
    pub repo: String,
    /// Access token; prefer [`CRASH_TOKEN_ENV`] so it never touches disk.
    pub token: String,
    pub api_base: String,
}

impl Default for CrashReportConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            repo: DEFAULT_CRASH_REPO.to_string(),
            token: String::new(),
            api_base: DEFAULT_GITEE_API.to_string(),
        }
    }
}

impl std::fmt::Debug for CrashReportConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrashReportConfig")
            .field("enabled", &self.enabled)
            .field("repo", &self.repo)
            .field(
                "token",
                &if self.token.is_empty() {
                    "<unset>"
                } else {
                    "<redacted>"
                },
            )
            .field("api_base", &self.api_base)
            .finish()
    }
}

impl CrashReportConfig {
    /// Token from the environment first, then from the runtime config file.
    pub fn resolved_token(&self) -> Option<String> {
        if let Ok(raw) = std::env::var(CRASH_TOKEN_ENV) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        let token = self.token.trim();
        (!token.is_empty()).then(|| token.to_string())
    }

    /// Whether crash reporting can actually be attempted.
    pub fn usable(&self) -> bool {
        self.enabled && !self.repo.trim().is_empty() && self.resolved_token().is_some()
    }
}

/// A single configuration key that could not be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub file: PathBuf,
    pub key: String,
    pub message: String,
}

impl ConfigIssue {
    pub fn summary(&self) -> String {
        let key = if self.key.is_empty() {
            "文件"
        } else {
            self.key.as_str()
        };
        format!("{} 的 {key}：{}", self.file.display(), self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub format: String,
    pub workspace_root: String,
    pub language: String,
    pub theme: ThemePreference,
    /// Uniform fallback for panel docking; see [`AppConfig::panel_docks`].
    pub panel_dock: PanelDock,
    pub panel_docks: PanelDocks,
    pub panel_arrangement: PanelArrangement,
    /// Sidebar width in logical pixels when both sides share one width.
    pub panel_width: f32,
    /// Explicit width for the left sidebar; `0.0` follows `panel_width`.
    pub left_width: f32,
    /// Explicit width for the right sidebar; `0.0` follows `panel_width`.
    pub right_width: f32,
    pub ui_font: String,
    pub note_font: String,
    pub ui_font_size: f32,
    pub note_font_size: f32,
    pub autosave: bool,
    pub code_wrap: bool,
    /// 用户已关闭的随包更新说明所属版本。升级后与当前版本不一致时，
    /// 启动时会自动以预览模式打开一次新版本的 changelog。
    pub shown_changelog_version: String,
    /// Where pasted/dropped images go; `Local` keeps them in the notebook.
    pub image_host: ImageHost,
    /// Keeps the loopback MCP server running while md-work is open.
    pub mcp_http: bool,
    pub mcp_port: u16,
    pub crash_report: CrashReportConfig,
    pub theme_colors: ThemeColors,
    /// Last main-window placement. `None` leaves size and state to the toolkit.
    pub window: Option<WindowPlacement>,
    /// Problems found while loading; skipped when saving.
    #[serde(skip)]
    pub issues: Vec<ConfigIssue>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            format: CONFIG_FORMAT.to_string(),
            workspace_root: String::new(),
            language: "zh-CN".to_string(),
            theme: ThemePreference::System,
            panel_dock: PanelDock::Left,
            panel_docks: PanelDocks::default(),
            panel_arrangement: PanelArrangement::Horizontal,
            panel_width: 300.0,
            left_width: 0.0,
            right_width: 0.0,
            ui_font: String::new(),
            note_font: String::new(),
            ui_font_size: 15.0,
            note_font_size: 16.0,
            autosave: true,
            code_wrap: true,
            shown_changelog_version: String::new(),
            image_host: ImageHost::Local,
            mcp_http: false,
            mcp_port: 11026,
            crash_report: CrashReportConfig::default(),
            theme_colors: ThemeColors::default(),
            window: None,
            issues: Vec::new(),
        }
    }
}

impl AppConfig {
    /// Loads the merged configuration at the standard location. Missing files
    /// are recreated so the three-file layout always exists after startup.
    /// Errors never abort startup: bad keys fall back to defaults and are
    /// recorded in [`AppConfig::issues`].
    pub fn load_or_default() -> Self {
        let core = core_config_path();
        let runtime = runtime_config_path();
        let theme = theme_config_path();
        let legacy = legacy_config_path();
        match Self::load_files(&core, &runtime, &theme, Some(&legacy)) {
            Ok(config) => {
                for issue in &config.issues {
                    tracing::warn!(
                        file = %issue.file.display(),
                        key = %issue.key,
                        message = %issue.message,
                        "配置项有误，已使用默认值"
                    );
                }
                if let Err(err) = Self::materialize(&core, &runtime, &theme, Some(&legacy), &config)
                {
                    tracing::warn!(error = %err, "写入配置文件失败");
                }
                config
            }
            Err(err) => {
                tracing::warn!(error = %err, "配置读取失败，使用内置默认值");
                let mut config = Self::default();
                config.panel_docks = PanelDocks::uniform(config.panel_dock);
                config
            }
        }
    }

    /// Loads a configuration anchored at `path` (the runtime file). The core
    /// and theme files are read from the same directory. A `.json` path is
    /// treated as the legacy single-file configuration.
    pub fn load_from(path: &Path) -> Result<Self> {
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            let mut config = Self::read_legacy(path)?;
            if config.panel_docks == PanelDocks::default() {
                config.panel_docks = PanelDocks::uniform(config.panel_dock);
            }
            return Ok(config);
        }
        let dir = config_dir_of(path);
        let mut issues = Vec::new();
        let mut config = Self::default();
        Self::apply_core(&mut config, &dir.join(CORE_CONFIG_FILE), &mut issues)?;
        Self::apply_runtime(&mut config, path, &mut issues)?;
        Self::apply_theme(&mut config, &dir.join(THEME_CONFIG_FILE), &mut issues)?;
        config.issues = issues;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&config_path())
    }

    /// Writes all three files. `path` is the runtime file; core/theme are its
    /// siblings so GUI callers can keep using `save_to(&config_path())`.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let dir = config_dir_of(path);
        let result = (|| {
            toml_io::save_text(&dir.join(CORE_CONFIG_FILE), &self.core_toml_text())?;
            toml_io::save_text(path, &self.runtime_toml_text())?;
            toml_io::save_text(&dir.join(THEME_CONFIG_FILE), &self.theme_toml_text())?;
            Ok(())
        })();
        match &result {
            Ok(()) => tracing::info!(
                path = %path.display(),
                theme = ?self.theme,
                language = %self.language,
                ui_font = %self.ui_font,
                note_font = %self.note_font,
                ui_font_size = self.ui_font_size,
                note_font_size = self.note_font_size,
                autosave = self.autosave,
                code_wrap = self.code_wrap,
                panel_dock = ?self.panel_dock,
                panel_arrangement = ?self.panel_arrangement,
                image_host = ?self.image_host,
                mcp_http = self.mcp_http,
                mcp_port = self.mcp_port,
                workspace_root = %self.workspace_root,
                "保存设置"
            ),
            Err(err) => tracing::error!(
                path = %path.display(),
                error = %err,
                "保存设置失败"
            ),
        }
        result
    }

    pub fn resolve_workspace_root(&self) -> Result<PathBuf> {
        let raw = self.workspace_root.trim();
        if raw.is_empty() {
            default_workspace_root()
        } else {
            Ok(PathBuf::from(raw))
        }
    }

    pub fn set_workspace_root(&mut self, root: &Path) {
        self.workspace_root = root.display().to_string();
    }

    pub fn panel_dock_for(&self, panel: PanelKind) -> PanelDock {
        self.panel_docks.get(panel)
    }

    pub fn set_panel_dock(&mut self, panel: PanelKind, dock: PanelDock) -> bool {
        if self.panel_docks.get(panel) == dock {
            return false;
        }
        self.panel_docks.set(panel, dock);
        self.panel_dock = self.panel_docks.uniform_value().unwrap_or(self.panel_dock);
        true
    }

    pub fn width_for_dock(&self, dock: PanelDock) -> f32 {
        let explicit = match dock {
            PanelDock::Left => self.left_width,
            PanelDock::Right => self.right_width,
        };
        if explicit > 0.0 {
            explicit
        } else {
            self.panel_width
        }
    }

    pub fn set_width_for_dock(&mut self, dock: PanelDock, width: f32) {
        if !width.is_finite() {
            return;
        }
        let width = width.clamp(PANEL_WIDTH_MIN, PANEL_WIDTH_MAX);
        match dock {
            PanelDock::Left => self.left_width = width,
            PanelDock::Right => self.right_width = width,
        }
    }

    /// One-line-per-issue message suitable for a startup notice.
    pub fn issue_summary(&self) -> Option<String> {
        if self.issues.is_empty() {
            return None;
        }
        let mut text = String::from("配置文件存在错误，已忽略相关项并继续运行：");
        for issue in &self.issues {
            text.push('\n');
            text.push_str("• ");
            text.push_str(&issue.summary());
        }
        Some(text)
    }

    fn load_files(
        core: &Path,
        runtime: &Path,
        theme: &Path,
        legacy: Option<&Path>,
    ) -> Result<Self> {
        let all_missing = !core.is_file() && !runtime.is_file() && !theme.is_file();
        let mut config = Self::default();
        if all_missing && let Some(legacy) = legacy.filter(|path| path.is_file()) {
            match Self::read_legacy(legacy) {
                Ok(mut loaded) => {
                    if loaded.panel_docks == PanelDocks::default() {
                        loaded.panel_docks = PanelDocks::uniform(loaded.panel_dock);
                    }
                    config = loaded;
                }
                Err(err) => {
                    tracing::warn!(
                        path = %legacy.display(),
                        error = %err,
                        "旧版配置读取失败，使用默认值"
                    );
                }
            }
        }
        let mut issues = Vec::new();
        Self::apply_core(&mut config, core, &mut issues)?;
        Self::apply_runtime(&mut config, runtime, &mut issues)?;
        Self::apply_theme(&mut config, theme, &mut issues)?;
        config.issues = issues;
        Ok(config)
    }

    /// Writes files that do not exist yet and archives a migrated JSON file.
    fn materialize(
        core: &Path,
        runtime: &Path,
        theme: &Path,
        legacy: Option<&Path>,
        config: &AppConfig,
    ) -> Result<()> {
        if !core.is_file() {
            toml_io::save_text(core, &config.core_toml_text())?;
            tracing::info!(path = %core.display(), "已生成核心配置");
        }
        if !runtime.is_file() {
            toml_io::save_text(runtime, &config.runtime_toml_text())?;
            tracing::info!(path = %runtime.display(), "已生成运行配置");
        }
        if !theme.is_file() {
            toml_io::save_text(theme, &config.theme_toml_text())?;
            tracing::info!(path = %theme.display(), "已生成主题配置");
        }
        if let Some(legacy) = legacy.filter(|path| path.is_file())
            && core.is_file()
            && runtime.is_file()
            && theme.is_file()
        {
            toml_io::archive_legacy(legacy);
            tracing::info!(path = %legacy.display(), "已归档旧版 JSON 配置");
        }
        Ok(())
    }

    fn read_legacy(path: &Path) -> Result<Self> {
        let bytes = fs::read(path)?;
        let config = serde_json::from_slice(&bytes).map_err(|source| Error::Json {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(config)
    }

    fn apply_core(
        config: &mut AppConfig,
        path: &Path,
        issues: &mut Vec<ConfigIssue>,
    ) -> Result<()> {
        let Some(root) = read_config_table(path, issues)? else {
            return Ok(());
        };
        let mut loader = Loader::new(path, issues);
        if let Some(app) = loader.table(&root, "app") {
            loader.string(app, "app", "format", &mut config.format);
            loader.string(app, "app", "workspace_root", &mut config.workspace_root);
            loader.string(app, "app", "language", &mut config.language);
            loader.u16(app, "app", "mcp_port", 1, u16::MAX, &mut config.mcp_port);
        }
        Ok(())
    }

    fn apply_runtime(
        config: &mut AppConfig,
        path: &Path,
        issues: &mut Vec<ConfigIssue>,
    ) -> Result<()> {
        let Some(root) = read_config_table(path, issues)? else {
            return Ok(());
        };
        let mut loader = Loader::new(path, issues);
        if let Some(general) = loader.table(&root, "general") {
            loader.boolean(general, "general", "autosave", &mut config.autosave);
            loader.boolean(general, "general", "code_wrap", &mut config.code_wrap);
            loader.boolean(general, "general", "mcp_http", &mut config.mcp_http);
            loader.string(
                general,
                "general",
                "shown_changelog_version",
                &mut config.shown_changelog_version,
            );
        }
        apply_window(&mut loader, &root, config);
        match loader.table(&root, "panels") {
            Some(panels) => {
                loader.parsed(
                    panels,
                    "panels",
                    "arrangement",
                    &mut config.panel_arrangement,
                );
                loader.parsed(panels, "panels", "dock", &mut config.panel_dock);
                loader.f32(
                    panels,
                    "panels",
                    "width",
                    PANEL_WIDTH_MIN,
                    PANEL_WIDTH_MAX,
                    &mut config.panel_width,
                );
                loader.f32_zero(
                    panels,
                    "panels",
                    "left_width",
                    PANEL_WIDTH_MAX,
                    &mut config.left_width,
                );
                loader.f32_zero(
                    panels,
                    "panels",
                    "right_width",
                    PANEL_WIDTH_MAX,
                    &mut config.right_width,
                );
                let fallback = config.panel_dock;
                for panel in PanelKind::ALL {
                    let key = panel.key();
                    let dock = if panels.contains_key(key) {
                        let mut dock = config.panel_docks.get(panel);
                        loader.parsed(panels, "panels", key, &mut dock);
                        dock
                    } else {
                        fallback
                    };
                    config.panel_docks.set(panel, dock);
                }
            }
            None => config.panel_docks = PanelDocks::uniform(config.panel_dock),
        }
        if let Some(images) = loader.table(&root, "images")
            && let Some(value) = images.get("host")
        {
            match value.clone().try_into::<ImageHost>() {
                Ok(host) => config.image_host = host,
                Err(err) => loader.issue("images.host", format!("图床配置无效：{err}")),
            }
        }
        if let Some(crash) = loader.table(&root, "crash_report") {
            loader.boolean(
                crash,
                "crash_report",
                "enabled",
                &mut config.crash_report.enabled,
            );
            loader.string(crash, "crash_report", "repo", &mut config.crash_report.repo);
            loader.string(
                crash,
                "crash_report",
                "token",
                &mut config.crash_report.token,
            );
            loader.string(
                crash,
                "crash_report",
                "api_base",
                &mut config.crash_report.api_base,
            );
        }
        Ok(())
    }

    fn apply_theme(
        config: &mut AppConfig,
        path: &Path,
        issues: &mut Vec<ConfigIssue>,
    ) -> Result<()> {
        let Some(root) = read_config_table(path, issues)? else {
            return Ok(());
        };
        let mut loader = Loader::new(path, issues);
        if let Some(appearance) = loader.table(&root, "appearance") {
            loader.parsed(appearance, "appearance", "mode", &mut config.theme);
            loader.string(appearance, "appearance", "ui_font", &mut config.ui_font);
            loader.string(appearance, "appearance", "note_font", &mut config.note_font);
            loader.f32(
                appearance,
                "appearance",
                "ui_font_size",
                FONT_SIZE_MIN,
                FONT_SIZE_MAX,
                &mut config.ui_font_size,
            );
            loader.f32(
                appearance,
                "appearance",
                "note_font_size",
                FONT_SIZE_MIN,
                FONT_SIZE_MAX,
                &mut config.note_font_size,
            );
        }
        if let Some(colors) = loader.table(&root, "colors") {
            for (name, palette) in [
                ("light", &mut config.theme_colors.light),
                ("dark", &mut config.theme_colors.dark),
            ] {
                let section = format!("colors.{name}");
                if let Some(table) = loader.table(colors, name) {
                    loader.palette(table, &section, palette);
                }
            }
        }
        Ok(())
    }

    fn core_toml_text(&self) -> String {
        let mut out = String::new();
        out.push_str(CORE_HEADER);
        out.push_str("\n[app]\n");
        out.push_str("# 配置格式版本，仅供程序识别，请勿修改。\n");
        out.push_str(&format!("format = {}\n", toml_string(&self.format)));
        out.push_str("# 工作区根目录；留空表示使用“文档/md-work”。\n");
        out.push_str(&format!(
            "workspace_root = {}\n",
            toml_string(&self.workspace_root)
        ));
        out.push_str("# 界面语言。\n");
        out.push_str(&format!("language = {}\n", toml_string(&self.language)));
        out.push_str("# 本地 MCP 服务端口。\n");
        out.push_str(&format!("mcp_port = {}\n", self.mcp_port));
        out
    }

    fn runtime_toml_text(&self) -> String {
        let mut out = String::new();
        out.push_str(RUNTIME_HEADER);
        out.push_str("\n[general]\n");
        out.push_str("# 编辑时自动保存。\n");
        out.push_str(&format!("autosave = {}\n", self.autosave));
        out.push_str("# 预览区代码块自动换行。\n");
        out.push_str(&format!("code_wrap = {}\n", self.code_wrap));
        out.push_str("# 应用运行期间保持本地 MCP 服务。\n");
        out.push_str(&format!("mcp_http = {}\n", self.mcp_http));
        out.push_str("# 已关闭的随包更新说明版本；升级后首次启动会再次自动打开。\n");
        out.push_str(&format!(
            "shown_changelog_version = {}\n",
            toml_string(&self.shown_changelog_version)
        ));

        if let Some(window) = self.window {
            out.push_str("\n[window]\n");
            out.push_str("# 主窗口形态，程序在窗口变化后写回；删除本节则下次启动使用默认窗口。\n");
            out.push_str("# state：windowed（普通）| maximized（最大化）\n");
            out.push_str(
                "# x、y、width、height 是普通状态下的位置和大小（逻辑像素）。最大化时这是还原后的矩形。\n",
            );
            out.push_str(&format!("state = \"{}\"\n", window.state.as_str()));
            out.push_str(&format!("x = {}\n", f32_literal(window.x)));
            out.push_str(&format!("y = {}\n", f32_literal(window.y)));
            out.push_str(&format!("width = {}\n", f32_literal(window.width)));
            out.push_str(&format!("height = {}\n", f32_literal(window.height)));
        }

        out.push_str("\n[panels]\n");
        out.push_str("# 侧边栏排布：horizontal（横向）| vertical（纵向）\n");
        out.push_str(&format!(
            "arrangement = \"{}\"\n",
            self.panel_arrangement.as_str()
        ));
        out.push_str("# 侧边栏宽度（逻辑像素）。\n");
        out.push_str(&format!("width = {}\n", f32_literal(self.panel_width)));
        if self.left_width > 0.0 {
            out.push_str("# 左侧栏独立宽度。\n");
            out.push_str(&format!("left_width = {}\n", f32_literal(self.left_width)));
        }
        if self.right_width > 0.0 {
            out.push_str("# 右侧栏独立宽度。\n");
            out.push_str(&format!(
                "right_width = {}\n",
                f32_literal(self.right_width)
            ));
        }
        out.push_str("# 侧边栏停靠：left（左）| right（右）\n");
        match self.panel_docks.uniform_value() {
            Some(dock) => {
                out.push_str(&format!("dock = \"{}\"\n", dock.as_str()));
            }
            None => {
                out.push_str(&format!("dock = \"{}\"\n", self.panel_dock.as_str()));
                out.push_str("# 以下为各面板的独立停靠设置。\n");
                for panel in PanelKind::ALL {
                    out.push_str(&format!(
                        "{} = \"{}\"\n",
                        panel.key(),
                        self.panel_docks.get(panel).as_str()
                    ));
                }
            }
        }

        out.push_str("\n[images]\n# 粘贴/拖入图片的存放位置。\n[images.host]\n");
        out.push_str(&image_host_toml(&self.image_host));

        out.push_str("\n[crash_report]\n");
        out.push_str("# 启动时若存在崩溃日志且网络可用，自动向 Gitee 提交 Issue。\n");
        out.push_str(&format!("enabled = {}\n", self.crash_report.enabled));
        out.push_str("# Gitee 仓库，格式为 owner/repo。\n");
        out.push_str(&format!(
            "repo = {}\n",
            toml_string(&self.crash_report.repo)
        ));
        out.push_str(&format!(
            "# 访问令牌；建议改用环境变量 {CRASH_TOKEN_ENV}，避免令牌写入磁盘。\n"
        ));
        out.push_str(&format!(
            "token = {}\n",
            toml_string(&self.crash_report.token)
        ));
        out.push_str("# Gitee OpenAPI 地址。\n");
        out.push_str(&format!(
            "api_base = {}\n",
            toml_string(&self.crash_report.api_base)
        ));
        out
    }

    fn theme_toml_text(&self) -> String {
        let mut out = String::new();
        out.push_str(THEME_HEADER);
        out.push_str("\n[appearance]\n");
        out.push_str("# 主题模式：light（浅色）| dark（深色）| system（跟随系统）\n");
        out.push_str(&format!("mode = \"{}\"\n", self.theme.as_str()));
        out.push_str("# 界面字体；留空使用系统默认字体。\n");
        out.push_str(&format!("ui_font = {}\n", toml_string(&self.ui_font)));
        out.push_str("# 笔记正文字体；留空跟随界面字体。\n");
        out.push_str(&format!("note_font = {}\n", toml_string(&self.note_font)));
        out.push_str("# 界面字号。\n");
        out.push_str(&format!(
            "ui_font_size = {}\n",
            f32_literal(self.ui_font_size)
        ));
        out.push_str("# 笔记正文字号。\n");
        out.push_str(&format!(
            "note_font_size = {}\n",
            f32_literal(self.note_font_size)
        ));

        for (name, palette) in [
            ("light", &self.theme_colors.light),
            ("dark", &self.theme_colors.dark),
        ] {
            out.push_str(&format!("\n[colors.{name}]\n"));
            out.push_str(&format!(
                "# 可选颜色覆盖（#RRGGBB），留空使用内置“{name}”配色。\n"
            ));
            out.push_str(&format!("# 可设置：{}\n", COLOR_KEYS.join(", ")));
            for (key, value) in palette.entries() {
                if let Some(value) = value {
                    out.push_str(&format!("{key} = {}\n", toml_string(value)));
                }
            }
        }
        out
    }
}

fn apply_window(loader: &mut Loader<'_>, root: &toml::Table, config: &mut AppConfig) {
    let Some(window) = loader.table(root, "window") else {
        return;
    };
    let before = loader.issues.len();
    if !window.contains_key("state") {
        loader.issue("window.state", "缺少窗口状态");
    }
    let mut state = WindowMode::Windowed;
    loader.parsed(window, "window", "state", &mut state);
    let mut x = f32::NAN;
    let mut y = f32::NAN;
    let mut width = f32::NAN;
    let mut height = f32::NAN;
    for (key, slot, min, max) in [
        ("x", &mut x, WINDOW_AXIS_MIN, WINDOW_AXIS_MAX),
        ("y", &mut y, WINDOW_AXIS_MIN, WINDOW_AXIS_MAX),
        ("width", &mut width, WINDOW_EXTENT_MIN, WINDOW_EXTENT_MAX),
        ("height", &mut height, WINDOW_EXTENT_MIN, WINDOW_EXTENT_MAX),
    ] {
        if !window.contains_key(key) {
            loader.issue(format!("window.{key}"), "缺少窗口位置或大小");
        }
        loader.f32(window, "window", key, min, max, slot);
    }
    if loader.issues.len() == before {
        config.window = WindowPlacement::new(state, x, y, width, height);
    }
}

fn config_dir_of(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn read_config_table(path: &Path, issues: &mut Vec<ConfigIssue>) -> Result<Option<toml::Table>> {
    match toml_io::read_table(path) {
        Ok(table) => Ok(table),
        Err(Error::Toml { source, .. }) => {
            tracing::warn!(path = %path.display(), error = %source, "配置文件语法错误，已忽略整个文件");
            issues.push(ConfigIssue {
                file: path.to_path_buf(),
                key: String::new(),
                message: format!("TOML 语法错误：{source}"),
            });
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

struct Loader<'a> {
    file: &'a Path,
    issues: &'a mut Vec<ConfigIssue>,
}

impl<'a> Loader<'a> {
    fn new(file: &'a Path, issues: &'a mut Vec<ConfigIssue>) -> Self {
        Self { file, issues }
    }

    fn issue(&mut self, key: impl Into<String>, message: impl Into<String>) {
        self.issues.push(ConfigIssue {
            file: self.file.to_path_buf(),
            key: key.into(),
            message: message.into(),
        });
    }

    fn table<'t>(&mut self, parent: &'t toml::Table, section: &str) -> Option<&'t toml::Table> {
        match parent.get(section) {
            None => None,
            Some(toml::Value::Table(table)) => Some(table),
            Some(other) => {
                self.issue(
                    section,
                    format!("应为表（table），实际为 {}", other.type_str()),
                );
                None
            }
        }
    }

    fn string(&mut self, tbl: &toml::Table, section: &str, key: &str, target: &mut String) {
        let Some(value) = tbl.get(key) else {
            return;
        };
        match value.as_str() {
            Some(text) => *target = text.to_string(),
            None => self.issue(
                format!("{section}.{key}"),
                format!("应为字符串，实际为 {}", value.type_str()),
            ),
        }
    }

    fn boolean(&mut self, tbl: &toml::Table, section: &str, key: &str, target: &mut bool) {
        let Some(value) = tbl.get(key) else {
            return;
        };
        match value.as_bool() {
            Some(flag) => *target = flag,
            None => self.issue(
                format!("{section}.{key}"),
                format!("应为 true 或 false，实际为 {}", value.type_str()),
            ),
        }
    }

    fn u16(
        &mut self,
        tbl: &toml::Table,
        section: &str,
        key: &str,
        min: u16,
        max: u16,
        target: &mut u16,
    ) {
        let Some(value) = tbl.get(key) else {
            return;
        };
        match value.as_integer() {
            Some(number) if (i64::from(min)..=i64::from(max)).contains(&number) => {
                *target = number as u16;
            }
            Some(_) => self.issue(
                format!("{section}.{key}"),
                format!("超出允许范围 {min}..={max}"),
            ),
            None => self.issue(
                format!("{section}.{key}"),
                format!("应为整数，实际为 {}", value.type_str()),
            ),
        }
    }

    fn f32(
        &mut self,
        tbl: &toml::Table,
        section: &str,
        key: &str,
        min: f32,
        max: f32,
        target: &mut f32,
    ) {
        let Some(value) = tbl.get(key) else {
            return;
        };
        match number(value) {
            Some(number) if (min..=max).contains(&number) => *target = number,
            Some(_) => self.issue(
                format!("{section}.{key}"),
                format!("超出允许范围 {min}..={max}"),
            ),
            None => self.issue(
                format!("{section}.{key}"),
                format!("应为数字，实际为 {}", value.type_str()),
            ),
        }
    }

    /// Like [`Loader::f32`] but `0.0` is also accepted ("follow the shared
    /// width").
    fn f32_zero(
        &mut self,
        tbl: &toml::Table,
        section: &str,
        key: &str,
        max: f32,
        target: &mut f32,
    ) {
        let Some(value) = tbl.get(key) else {
            return;
        };
        match number(value) {
            Some(0.0) => *target = 0.0,
            Some(number) if (PANEL_WIDTH_MIN..=max).contains(&number) => *target = number,
            Some(_) => self.issue(
                format!("{section}.{key}"),
                format!("应为 0 或 {PANEL_WIDTH_MIN}..={max}"),
            ),
            None => self.issue(
                format!("{section}.{key}"),
                format!("应为数字，实际为 {}", value.type_str()),
            ),
        }
    }

    fn parsed<T: DeserializeOwned>(
        &mut self,
        tbl: &toml::Table,
        section: &str,
        key: &str,
        target: &mut T,
    ) {
        let Some(value) = tbl.get(key) else {
            return;
        };
        match value.clone().try_into::<T>() {
            Ok(parsed) => *target = parsed,
            Err(err) => self.issue(format!("{section}.{key}"), format!("取值无效：{err}")),
        }
    }

    fn string_option(
        &mut self,
        tbl: &toml::Table,
        section: &str,
        key: &str,
        target: &mut Option<String>,
    ) {
        let Some(value) = tbl.get(key) else {
            return;
        };
        let Some(text) = value.as_str() else {
            self.issue(
                format!("{section}.{key}"),
                format!("应为颜色字符串，实际为 {}", value.type_str()),
            );
            return;
        };
        let text = text.trim();
        if text.is_empty() {
            *target = None;
        } else if is_hex_color(text) {
            *target = Some(text.to_string());
        } else {
            self.issue(
                format!("{section}.{key}"),
                format!("颜色格式应为 #RRGGBB，实际为“{text}”"),
            );
        }
    }

    fn palette(&mut self, tbl: &toml::Table, section: &str, target: &mut ColorOverrides) {
        for key in COLOR_KEYS {
            if let Some(slot) = target.slot(key) {
                self.string_option(tbl, section, key, slot);
            }
        }
    }
}

fn number(value: &toml::Value) -> Option<f32> {
    let raw = value
        .as_float()
        .or_else(|| value.as_integer().map(|number| number as f64))?;
    raw.is_finite().then_some(raw as f32)
}

fn is_hex_color(value: &str) -> bool {
    let Some(hex) = value.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn f32_literal(value: f32) -> String {
    if value.is_finite() {
        format!("{value:?}")
    } else {
        "0.0".to_string()
    }
}

/// TOML literal string when possible so Windows paths stay readable.
fn toml_string(value: &str) -> String {
    let needs_basic = value.contains('\'') || value.chars().any(|ch| ch.is_control() && ch != '\t');
    if !needs_basic {
        return format!("'{value}'");
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04X}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn image_host_toml(host: &ImageHost) -> String {
    match host {
        ImageHost::Local => "kind = \"local\"\n".to_string(),
        ImageHost::PicGo { endpoint } => {
            format!("kind = \"picgo\"\nendpoint = {}\n", toml_string(endpoint))
        }
        ImageHost::Http {
            endpoint,
            field,
            url_path,
            headers,
        } => format!(
            "kind = \"http\"\nendpoint = {}\nfield = {}\nurl_path = {}\nheaders = {}\n",
            toml_string(endpoint),
            toml_string(field),
            toml_string(url_path),
            toml_string(headers)
        ),
    }
}

/// Zero-based caret position inside a note.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CursorPos {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotebookSession {
    pub id: Uuid,
    pub open_notes: Vec<String>,
    pub active_note: Option<String>,
    pub cursors: BTreeMap<String, CursorPos>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub format: String,
    pub active_notebook: Option<Uuid>,
    /// Kept before the table-valued fields: TOML requires plain values before
    /// tables.
    pub external_notes: Vec<String>,
    pub active_external: Option<String>,
    pub external_cursors: BTreeMap<String, CursorPos>,
    pub notebooks: Vec<NotebookSession>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            format: SESSION_FORMAT.to_string(),
            active_notebook: None,
            notebooks: Vec::new(),
            external_notes: Vec::new(),
            active_external: None,
            external_cursors: BTreeMap::new(),
        }
    }
}

impl Session {
    pub fn load_or_default() -> Self {
        Self::load_from(&session_path()).unwrap_or_default()
    }

    /// Reads `path` as TOML, migrating a legacy `.json` sibling once.
    pub fn load_from(path: &Path) -> Result<Self> {
        match toml_io::load_with_legacy::<Session>(path)? {
            Some(session) => Ok(session),
            None => Ok(Self::default()),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&session_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        toml_io::save_with_header(path, SESSION_HEADER, self)
    }

    pub fn notebook(&self, id: &Uuid) -> Option<&NotebookSession> {
        self.notebooks.iter().find(|entry| entry.id == *id)
    }

    pub fn notebook_mut(&mut self, id: &Uuid) -> &mut NotebookSession {
        if let Some(index) = self.notebooks.iter().position(|entry| entry.id == *id) {
            return &mut self.notebooks[index];
        }
        self.notebooks.push(NotebookSession {
            id: *id,
            ..Default::default()
        });
        self.notebooks.last_mut().expect("pushed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mdwork-config-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_config_uses_defaults() {
        let dir = temp_dir();
        let config = AppConfig::load_from(&dir.join("runtime.toml")).unwrap();
        assert_eq!(config, AppConfig::default());
        assert_eq!(config.language, "zh-CN");
        assert!(config.resolve_workspace_root().is_ok());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn config_round_trip() {
        let dir = temp_dir();
        let path = dir.join("runtime.toml");
        let mut config = AppConfig {
            language: "en".to_string(),
            ..Default::default()
        };
        config.theme = ThemePreference::Dark;
        config.set_workspace_root(Path::new(r"D:\notes"));
        config.set_panel_dock(PanelKind::Outline, PanelDock::Right);
        config.set_width_for_dock(PanelDock::Right, 340.0);
        config.theme_colors.dark.primary = Some("#3366ff".to_string());
        config.crash_report.token = "secret".to_string();
        config.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded, config);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shown_changelog_version_round_trip() {
        let dir = temp_dir();
        let path = dir.join("runtime.toml");
        let config = AppConfig {
            shown_changelog_version: "1.2.4".to_string(),
            ..Default::default()
        };
        config.save_to(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("shown_changelog_version = '1.2.4'"));
        assert_eq!(
            AppConfig::load_from(&path).unwrap().shown_changelog_version,
            "1.2.4"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn three_files_are_written_with_headers() {
        let dir = temp_dir();
        let config = AppConfig {
            workspace_root: r"D:\work".to_string(),
            ..Default::default()
        };
        config.save_to(&dir.join("runtime.toml")).unwrap();
        let core = fs::read_to_string(dir.join("core.toml")).unwrap();
        let runtime = fs::read_to_string(dir.join("runtime.toml")).unwrap();
        let theme = fs::read_to_string(dir.join("theme.toml")).unwrap();
        assert!(core.contains(CORE_CONFIG_WARNING));
        assert!(core.contains(r"workspace_root = 'D:\work'"));
        assert!(runtime.contains("[panels]"));
        assert!(runtime.contains("dock = \"left\""));
        assert!(theme.contains("[appearance]"));
        assert!(theme.contains("[colors.light]"));
        assert!(!runtime.contains("[window]"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn window_placement_round_trip() {
        let dir = temp_dir();
        let path = dir.join("runtime.toml");
        let config = AppConfig {
            window: WindowPlacement::new(WindowMode::Maximized, 120.4, 80.6, 1280.0, 800.0),
            ..Default::default()
        };
        config.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(
            loaded.window,
            Some(WindowPlacement {
                state: WindowMode::Maximized,
                x: 120.0,
                y: 81.0,
                width: 1280.0,
                height: 800.0,
            })
        );
        assert!(loaded.issues.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn broken_window_keeps_the_default_frame() {
        let dir = temp_dir();
        let path = dir.join("runtime.toml");
        fs::write(
            &path,
            "[general]\nautosave = false\n\n[window]\nstate = \"huge\"\nwidth = 10\n",
        )
        .unwrap();
        let config = AppConfig::load_from(&path).unwrap();
        assert!(!config.autosave);
        assert!(config.window.is_none());
        let keys: Vec<&str> = config
            .issues
            .iter()
            .map(|issue| issue.key.as_str())
            .collect();
        assert!(keys.contains(&"window.state"));
        assert!(keys.contains(&"window.x"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn uniform_docks_use_shared_key_and_per_panel_uses_keys() {
        let dir = temp_dir();
        let path = dir.join("runtime.toml");
        let mut config = AppConfig {
            panel_docks: PanelDocks::uniform(PanelDock::Right),
            ..Default::default()
        };
        config.save_to(&path).unwrap();
        let runtime = fs::read_to_string(&path).unwrap();
        assert!(runtime.contains("dock = \"right\""));
        assert!(!runtime.contains("notes = \""));
        assert_eq!(
            AppConfig::load_from(&path).unwrap().panel_docks,
            config.panel_docks
        );

        config.set_panel_dock(PanelKind::Trash, PanelDock::Left);
        config.save_to(&path).unwrap();
        let runtime = fs::read_to_string(&path).unwrap();
        assert!(runtime.contains("trash = \"left\""));
        assert!(runtime.contains("notes = \"right\""));
        assert_eq!(
            AppConfig::load_from(&path).unwrap().panel_docks,
            config.panel_docks
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_items_are_reported_and_defaults_kept() {
        let dir = temp_dir();
        let path = dir.join("runtime.toml");
        fs::write(
            &path,
            "[general]\nautosave = \"yes\"\ncode_wrap = false\n\n[panels]\ndock = \"middle\"\nwidth = 9999.0\n\n[images.host]\nkind = \"nope\"\n",
        )
        .unwrap();
        let config = AppConfig::load_from(&path).unwrap();
        assert!(config.autosave);
        assert!(!config.code_wrap);
        assert_eq!(config.panel_dock, PanelDock::Left);
        assert_eq!(config.panel_width, 300.0);
        assert!(config.image_host.is_local());
        let keys: Vec<&str> = config
            .issues
            .iter()
            .map(|issue| issue.key.as_str())
            .collect();
        assert!(keys.contains(&"general.autosave"));
        assert!(keys.contains(&"panels.dock"));
        assert!(keys.contains(&"panels.width"));
        assert!(keys.contains(&"images.host"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_json_is_migrated_to_three_files() {
        let dir = temp_dir();
        let core = dir.join("core.toml");
        let runtime = dir.join("runtime.toml");
        let theme = dir.join("theme.toml");
        let legacy = dir.join("config.json");
        let legacy_config = AppConfig {
            language: "en".to_string(),
            theme: ThemePreference::Dark,
            panel_dock: PanelDock::Right,
            ..Default::default()
        };
        fs::write(&legacy, serde_json::to_vec_pretty(&legacy_config).unwrap()).unwrap();

        let loaded = AppConfig::load_files(&core, &runtime, &theme, Some(&legacy)).unwrap();
        assert_eq!(loaded.language, "en");
        assert_eq!(loaded.theme, ThemePreference::Dark);
        assert_eq!(loaded.panel_docks, PanelDocks::uniform(PanelDock::Right));

        AppConfig::materialize(&core, &runtime, &theme, Some(&legacy), &loaded).unwrap();
        assert!(core.is_file() && runtime.is_file() && theme.is_file());
        assert!(!legacy.exists());
        assert!(dir.join("config.json.migrated").is_file());

        let reloaded = AppConfig::load_files(&core, &runtime, &theme, Some(&legacy)).unwrap();
        assert_eq!(reloaded.language, "en");
        assert_eq!(reloaded.theme, ThemePreference::Dark);
        assert_eq!(reloaded.panel_docks, PanelDocks::uniform(PanelDock::Right));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn image_host_round_trip() {
        let dir = temp_dir();
        let path = dir.join("runtime.toml");
        let config = AppConfig {
            image_host: ImageHost::Http {
                endpoint: "https://img.example.com/upload".to_string(),
                field: "file".to_string(),
                url_path: "/url".to_string(),
                headers: "Authorization: Bearer x".to_string(),
            },
            ..Default::default()
        };
        config.save_to(&path).unwrap();
        let loaded = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.image_host, config.image_host);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn session_round_trip() {
        let dir = temp_dir();
        let path = dir.join("session.toml");
        let mut session = Session::default();
        let id = Uuid::new_v4();
        session.active_notebook = Some(id);
        session.external_notes.push(r"D:\misc\note.md".to_string());
        let entry = session.notebook_mut(&id);
        entry.open_notes.push("a.md".to_string());
        entry.active_note = Some("a.md".to_string());
        entry
            .cursors
            .insert("a.md".to_string(), CursorPos { line: 3, column: 4 });
        session.save_to(&path).unwrap();
        let loaded = Session::load_from(&path).unwrap();
        assert_eq!(loaded, session);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn crash_report_token_prefers_config_but_redacts_debug() {
        let mut config = CrashReportConfig {
            token: "abc".to_string(),
            ..Default::default()
        };
        assert_eq!(config.resolved_token().as_deref(), Some("abc"));
        config.token.clear();
        assert!(config.resolved_token().is_none());
        let debug = format!("{config:?}");
        assert!(!debug.contains("abc"));
    }
}
