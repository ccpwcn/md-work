//! Pluggable theme system (all configuration is TOML).
//!
//! Two kinds of files are involved:
//!
//! * `~/.config/md-work/theme_extension.toml` — the main configuration. It
//!   stores the active theme, the extension loading policy and the error
//!   fallback policy. Every key has a matching control in the settings panel.
//! * `~/.config/md-work/themes/<id>.toml` — one extension theme per file.
//!   Dropping a new file in that directory loads it immediately; JSON files in
//!   Zed's theme format are converted into TOML automatically.
//!
//! The parsed theme is emitted as a JSON value shaped like
//! `gpui_component::ThemeConfig` (the component library's public theme schema).
//! The GUI deserializes that value — the headless core never depends on GPUI.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::config::ThemePreference;
use crate::{paths, toml_io};

/// `~/.config/md-work/theme_extension.toml`.
pub fn main_config_path() -> PathBuf {
    paths::theme_extension_config_path()
}

/// `~/.config/md-work/themes`.
pub fn themes_dir() -> PathBuf {
    paths::extension_themes_dir()
}

const CONFIG_HEADER: &str = "\
# =====================================================================
#  墨笺 md-work · 主题扩展主配置（theme_extension.toml）
#
#  本文件保存软件与主题相关的全部配置；每一项都能在「设置 → 外观」中找到。
#  扩展主题放在同目录的 themes/ 子目录，每个主题一个 TOML 文件：
#
#    [theme]
#    name = \"My Theme\"           # 必填，显示名称
#    author = \"...\"              # 可选
#    mode = \"dark\"               # light | dark，必填
#
#    [colors]
#    background = \"#1e1e2e\"      # 键名与 gpui-component 主题一致
#    foreground = \"#cdd6f4\"
#    \"accent.background\" = \"#313244\"   # 含点的键用引号，也可写成 [colors.accent]
#
#    [radius]                     # 可选
#    normal = 6
#    large = 8
#    shadow = true
#
#    [highlight]                  # 可选，语法高亮（与 Zed 的 style 兼容）
#    \"editor.background\" = \"#1e1e2e\"
#    [highlight.syntax]
#    keyword = \"#bb9af7\"
#    [highlight.syntax.comment]
#    color = \"#565f89\"
#
#  可直接把 Zed 的 .json 主题文件放进 themes/，或在设置里点「导入 Zed 主题」，
#  软件会自动转换为上述 TOML 格式。
# =====================================================================
";

/// A theme that can be activated from `theme_extension.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveTheme {
    /// Follow the OS appearance with the two built-in themes.
    System,
    BuiltinLight,
    BuiltinDark,
    /// `id` is the file stem of `~/.config/md-work/themes/<id>.toml`.
    Extension(String),
}

impl ActiveTheme {
    pub fn as_str(&self) -> String {
        match self {
            Self::System => "system".to_string(),
            Self::BuiltinLight => "builtin:light".to_string(),
            Self::BuiltinDark => "builtin:dark".to_string(),
            Self::Extension(id) => format!("ext:{id}"),
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "system" => Self::System,
            "builtin:light" | "light" => Self::BuiltinLight,
            "builtin:dark" | "dark" => Self::BuiltinDark,
            other => match other.strip_prefix("ext:") {
                Some(id) if !id.trim().is_empty() => Self::Extension(id.trim().to_string()),
                _ => Self::System,
            },
        }
    }

    pub fn is_extension(&self) -> bool {
        matches!(self, Self::Extension(_))
    }
}

impl Serialize for ActiveTheme {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for ActiveTheme {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(deserializer)?))
    }
}

/// How extension theme files are picked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeLoadPolicy {
    /// Watch `themes/` and load new/changed files immediately (default).
    #[default]
    Auto,
    /// Only scan once when the application starts.
    Startup,
    /// Never load extension themes; built-ins only.
    Disabled,
}

impl ThemeLoadPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Startup => "startup",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "auto" => Some(Self::Auto),
            "startup" => Some(Self::Startup),
            "disabled" => Some(Self::Disabled),
            _ => None,
        }
    }

    pub fn watches(self) -> bool {
        matches!(self, Self::Auto)
    }

    pub fn loads(self) -> bool {
        !matches!(self, Self::Disabled)
    }
}

/// What to do when the active extension theme disappears or fails to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeFallbackPolicy {
    /// Fall back to the built-in theme of the same light/dark mode.
    #[default]
    Builtin,
    /// Keep the last successfully loaded theme on screen.
    KeepCurrent,
    /// Fall back to following the OS appearance.
    System,
}

impl ThemeFallbackPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::KeepCurrent => "keep-current",
            Self::System => "system",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "builtin" => Some(Self::Builtin),
            "keep-current" => Some(Self::KeepCurrent),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

/// Contents of `theme_extension.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeExtensionConfig {
    /// Currently activated theme.
    pub active: ActiveTheme,
    /// How extension theme files are loaded.
    pub load_policy: ThemeLoadPolicy,
    /// What happens when a theme fails to load.
    pub fallback: ThemeFallbackPolicy,
}

impl Default for ThemeExtensionConfig {
    fn default() -> Self {
        Self {
            active: ActiveTheme::System,
            load_policy: ThemeLoadPolicy::Auto,
            fallback: ThemeFallbackPolicy::Builtin,
        }
    }
}

impl ThemeExtensionConfig {
    /// Loads the config, tolerating broken individual keys. A missing file is
    /// created from the legacy `theme.toml` mode (so existing users keep their
    /// light/dark/system choice).
    pub fn load(path: &Path, legacy_mode: Option<ThemePreference>) -> (Self, Vec<String>) {
        let mut config = Self::default();
        let mut issues = Vec::new();
        let existed = path.is_file();
        if !existed {
            if let Some(mode) = legacy_mode {
                config.active = match mode {
                    ThemePreference::Light => ActiveTheme::BuiltinLight,
                    ThemePreference::Dark => ActiveTheme::BuiltinDark,
                    ThemePreference::System => ActiveTheme::System,
                };
            }
        }
        match fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<toml::Table>(&text) {
                Ok(table) => {
                    if let Some(raw) = table.get("active").and_then(toml::Value::as_str) {
                        let value = raw.trim();
                        let valid = matches!(
                            value,
                            "system" | "light" | "dark" | "builtin:light" | "builtin:dark"
                        ) || value
                            .strip_prefix("ext:")
                            .is_some_and(|id| !id.trim().is_empty());
                        if !valid {
                            issues.push(format!("active：未知值 {raw}，已使用 system"));
                        }
                        config.active = ActiveTheme::parse(raw);
                    } else if table.contains_key("active") {
                        issues.push("active：需要字符串，已使用 system".to_string());
                    }
                    if let Some(raw) = table.get("load_policy").and_then(toml::Value::as_str) {
                        match ThemeLoadPolicy::parse(raw) {
                            Some(policy) => config.load_policy = policy,
                            None => issues.push(format!(
                                "load_policy：未知值 {raw}，已使用 {}",
                                config.load_policy.as_str()
                            )),
                        }
                    } else if table.contains_key("load_policy") {
                        issues.push("load_policy：需要字符串，已使用 auto".to_string());
                    }
                    if let Some(raw) = table.get("fallback").and_then(toml::Value::as_str) {
                        match ThemeFallbackPolicy::parse(raw) {
                            Some(policy) => config.fallback = policy,
                            None => issues.push(format!(
                                "fallback：未知值 {raw}，已使用 {}",
                                config.fallback.as_str()
                            )),
                        }
                    } else if table.contains_key("fallback") {
                        issues.push("fallback：需要字符串，已使用 builtin".to_string());
                    }
                }
                Err(err) => issues.push(format!("解析失败：{err}，已使用默认值")),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => issues.push(format!("读取失败：{err}，已使用默认值")),
        }
        if !existed {
            let _ = config.save(path);
        }
        (config, issues)
    }

    /// Reloads a config that changed on disk without rewriting it.
    pub fn reload(path: &Path) -> (Self, Vec<String>) {
        Self::load(path, None)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        toml_io::save_text(path, &self.to_toml_text())
            .map_err(|err| format!("写入 {} 失败：{err}", path.display()))
    }

    pub fn to_toml_text(&self) -> String {
        format!(
            "{CONFIG_HEADER}\n\
             # 当前激活的主题：system（跟随系统，内置浅/深）| builtin:light | builtin:dark | ext:<id>\n\
             active = {}\n\n\
             # 扩展主题加载策略：auto（监听 themes/，新增或修改立即载入）| startup（仅启动时加载）| disabled（只使用内置主题）\n\
             load_policy = {}\n\n\
             # 错误回退策略：builtin（回退到同模式内置主题）| keep-current（保留上一次有效主题）| system（跟随系统）\n\
             fallback = {}\n",
            toml_quote(&self.active.as_str()),
            toml_quote(self.load_policy.as_str()),
            toml_quote(self.fallback.as_str()),
        )
    }
}

/// Optional typography carried by an extension theme.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeTypography {
    pub ui_font: Option<String>,
    pub note_font: Option<String>,
    pub ui_font_size: Option<f32>,
    pub note_font_size: Option<f32>,
}

/// A successfully parsed extension theme.
#[derive(Debug, Clone)]
pub struct ExtensionTheme {
    /// File stem; also the `ext:<id>` used in `theme_extension.toml`.
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    /// `"light"` or `"dark"`.
    pub mode: String,
    /// `"native"`, `"zed"`, …
    pub source: String,
    pub path: PathBuf,
    pub typography: ThemeTypography,
    /// `gpui_component::ThemeConfig`-shaped JSON.
    pub config: Value,
}

#[derive(Debug, Clone)]
pub struct ThemeFailure {
    pub path: PathBuf,
    pub name: String,
    pub error: String,
}

#[derive(Debug, Default)]
pub struct ThemeScan {
    pub themes: Vec<ExtensionTheme>,
    pub failures: Vec<ThemeFailure>,
}

/// Scans `~/.config/md-work/themes`. Zed `.json` files are converted to TOML
/// first, then every `.toml` file is parsed. A broken file never blocks the
/// others — it is reported as a [`ThemeFailure`] instead.
pub fn scan_themes(dir: &Path) -> ThemeScan {
    let mut scan = ThemeScan::default();
    if let Err(err) = fs::create_dir_all(dir) {
        scan.failures.push(ThemeFailure {
            path: dir.to_path_buf(),
            name: "themes".to_string(),
            error: format!("无法创建主题目录：{err}"),
        });
        return scan;
    }
    import_zed_files_in_dir(dir, &mut scan);

    let mut files: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_file() && has_extension(path, "toml"))
            .collect(),
        Err(err) => {
            scan.failures.push(ThemeFailure {
                path: dir.to_path_buf(),
                name: "themes".to_string(),
                error: format!("无法读取主题目录：{err}"),
            });
            Vec::new()
        }
    };
    files.sort();

    let mut seen = HashSet::new();
    for path in files {
        let stem = file_stem(&path);
        let id = slugify(&stem);
        if !seen.insert(id.clone()) {
            scan.failures.push(ThemeFailure {
                path,
                name: stem,
                error: format!("主题 id「{id}」重复"),
            });
            continue;
        }
        match load_theme_file(&path) {
            Ok(mut theme) => {
                theme.id = id;
                scan.themes.push(theme);
            }
            Err(error) => scan.failures.push(ThemeFailure {
                path,
                name: stem,
                error,
            }),
        }
    }
    scan
}

pub fn load_theme_file(path: &Path) -> Result<ExtensionTheme, String> {
    let text = fs::read_to_string(path).map_err(|err| format!("读取失败：{err}"))?;
    parse_theme_toml(path, &text)
}

/// Parses an extension theme TOML file into a vendor-compatible JSON config.
pub fn parse_theme_toml(path: &Path, text: &str) -> Result<ExtensionTheme, String> {
    let root: toml::Table = toml::from_str(text).map_err(|err| format!("TOML 解析失败：{err}"))?;
    let theme = root
        .get("theme")
        .and_then(|value| value.as_table())
        .ok_or_else(|| "缺少 [theme] 小节".to_string())?;
    let name = theme
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("缺少 [theme] 的 name".to_string());
    }
    let mode = theme
        .get("mode")
        .and_then(|value| value.as_str())
        .unwrap_or("light")
        .trim()
        .to_ascii_lowercase();
    if mode != "light" && mode != "dark" {
        return Err(format!(
            "[theme] 的 mode 只能是 light 或 dark，实际为 {mode}"
        ));
    }

    let mut colors = BTreeMap::new();
    if let Some(table) = root.get("colors").and_then(|value| value.as_table()) {
        collect_color_strings(table, "", &mut colors);
    }
    if !colors.contains_key("background") && !colors.contains_key("foreground") {
        return Err("[colors] 至少需要 background 或 foreground".to_string());
    }

    let mut config: Map<String, Value> = Map::new();
    config.insert("name".to_string(), Value::String(name.clone()));
    config.insert("mode".to_string(), Value::String(mode.clone()));
    config.insert(
        "colors".to_string(),
        Value::Object(
            colors
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        ),
    );

    if let Some(radius) = root.get("radius").and_then(|value| value.as_table()) {
        if let Some(value) = radius.get("normal").and_then(|value| value.as_integer()) {
            config.insert(
                "radius".to_string(),
                Value::Number(serde_json::Number::from(value.clamp(0, 64))),
            );
        }
        if let Some(value) = radius.get("large").and_then(|value| value.as_integer()) {
            config.insert(
                "radius.lg".to_string(),
                Value::Number(serde_json::Number::from(value.clamp(0, 64))),
            );
        }
        if let Some(value) = radius.get("shadow").and_then(|value| value.as_bool()) {
            config.insert("shadow".to_string(), Value::Bool(value));
        }
    }

    if let Some(highlight) = root.get("highlight").and_then(|value| value.as_table()) {
        let mut style: Map<String, Value> = Map::new();
        collect_highlight_strings(highlight, "", &mut style);
        let syntax = highlight
            .get("syntax")
            .and_then(|value| value.as_table())
            .map(build_syntax)
            .unwrap_or_default();
        style.insert("syntax".to_string(), Value::Object(syntax));
        config.insert("highlight".to_string(), Value::Object(style));
    }

    let typography = root
        .get("typography")
        .and_then(|value| value.as_table())
        .map(|table| ThemeTypography {
            ui_font: non_empty_str(table, "ui_font"),
            note_font: non_empty_str(table, "note_font"),
            ui_font_size: positive_size(table, "ui_font_size"),
            note_font_size: positive_size(table, "note_font_size"),
        })
        .unwrap_or_default();

    Ok(ExtensionTheme {
        id: slugify(&file_stem(path)),
        name,
        author: non_empty_str(theme, "author").unwrap_or_default(),
        description: non_empty_str(theme, "description").unwrap_or_default(),
        mode,
        source: non_empty_str(theme, "source").unwrap_or_else(|| "native".to_string()),
        path: path.to_path_buf(),
        typography,
        config: Value::Object(config),
    })
}

/// Stable content hash used to avoid duplicate reload/notifications.
pub fn content_signature(path: &Path) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match fs::read(path) {
        Ok(bytes) => {
            bytes.hash(&mut hasher);
            hasher.finish()
        }
        Err(_) => 0,
    }
}

// ---------------------------------------------------------------------------
// Zed theme import
// ---------------------------------------------------------------------------

/// One generated TOML document from a Zed theme JSON file.
#[derive(Debug, Clone)]
pub struct ZedThemeDoc {
    pub stem: String,
    pub text: String,
    pub name: String,
}

/// Converts a Zed theme JSON document into one TOML document per theme variant.
pub fn zed_json_to_toml_documents(json: &str) -> Result<Vec<ZedThemeDoc>, String> {
    let root: Value = serde_json::from_str(json).map_err(|err| format!("JSON 解析失败：{err}"))?;
    let author = root
        .get("author")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let set_name = root
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let variants: Vec<&Value> = match root.get("themes").and_then(Value::as_array) {
        Some(themes) => themes.iter().collect(),
        None => vec![&root],
    };
    if variants.is_empty() {
        return Err("未找到 themes 数组".to_string());
    }

    let mut docs = Vec::new();
    for (index, variant) in variants.iter().enumerate() {
        let style = variant
            .get("style")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("第 {} 个主题缺少 style", index + 1))?;
        let name = variant
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if set_name.is_empty() {
                    format!("Zed Theme {index}")
                } else {
                    set_name.clone()
                }
            });
        let appearance = variant
            .get("appearance")
            .and_then(Value::as_str)
            .unwrap_or("dark");
        let mode = if appearance.eq_ignore_ascii_case("light") {
            "light"
        } else {
            "dark"
        };

        let mut out = String::new();
        out.push_str("# 由 Zed 主题自动转换而来，可继续手工编辑。\n");
        out.push_str("[theme]\n");
        out.push_str(&format!("name = {}\n", toml_quote(&name)));
        if !author.is_empty() {
            out.push_str(&format!("author = {}\n", toml_quote(&author)));
        }
        out.push_str(&format!("mode = {}\n", toml_quote(mode)));
        out.push_str("source = \"zed\"\n\n");

        let mut colors: BTreeMap<String, String> = BTreeMap::new();
        for (zed_key, mdwork_key) in ZED_COLOR_MAP {
            if colors.contains_key(*mdwork_key) {
                continue;
            }
            if let Some(color) = style
                .get(*zed_key)
                .and_then(Value::as_str)
                .and_then(normalize_color)
            {
                colors.insert((*mdwork_key).to_string(), color);
            }
        }
        if let Some(accent) = style
            .get("accents")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(Value::as_str)
            .and_then(normalize_color)
        {
            colors
                .entry("primary.background".to_string())
                .or_insert(accent);
        }
        out.push_str("[colors]\n");
        if colors.is_empty() {
            return Err(format!("主题「{name}」没有任何可识别的颜色"));
        }
        for (key, value) in &colors {
            out.push_str(&format!("{} = {}\n", toml_key(key), toml_quote(value)));
        }

        let mut highlight: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in style {
            let Some(color) = value.as_str().and_then(normalize_color) else {
                continue;
            };
            let mapped = match key.as_str() {
                "editor.highlighted_line.background" => "editor.active_line.background",
                "editor.gutter.background" => "editor.gutter.background",
                "editor.active_line.background" => "editor.active_line.background",
                other if other.starts_with("editor.") => other,
                other @ ("error" | "warning" | "info" | "success" | "hint" | "conflict"
                | "created" | "modified" | "predictive" | "hidden") => other,
                _ => continue,
            };
            highlight.insert(mapped.to_string(), color);
        }
        if !highlight.is_empty() {
            out.push_str("\n[highlight]\n");
            for (key, value) in &highlight {
                out.push_str(&format!("{} = {}\n", toml_key(key), toml_quote(value)));
            }
        }

        if let Some(syntax) = style.get("syntax").and_then(Value::as_object) {
            let mut syntax_lines = Vec::new();
            for (key, value) in syntax {
                match zed_syntax_value(key, value) {
                    Some(rendered) => {
                        syntax_lines.push(format!("{} = {rendered}", toml_key(key)));
                    }
                    None => continue,
                }
            }
            if !syntax_lines.is_empty() {
                out.push_str("\n[highlight.syntax]\n");
                for line in syntax_lines {
                    out.push_str(&line);
                    out.push('\n');
                }
            }
        }

        let stem_base = slugify(&format!("{name}-{mode}"));
        docs.push(ZedThemeDoc {
            stem: if stem_base.is_empty() {
                format!("zed-theme-{index}")
            } else {
                stem_base
            },
            text: out,
            name,
        });
    }
    Ok(docs)
}

/// Writes converted TOML files into `dir` and returns their paths.
pub fn import_zed_theme(json_path: &Path, dir: &Path) -> Result<Vec<PathBuf>, String> {
    let json = fs::read_to_string(json_path)
        .map_err(|err| format!("读取 {} 失败：{err}", json_path.display()))?;
    write_zed_documents(&json, dir)
}

pub fn write_zed_documents(json: &str, dir: &Path) -> Result<Vec<PathBuf>, String> {
    let docs = zed_json_to_toml_documents(json)?;
    fs::create_dir_all(dir).map_err(|err| format!("创建主题目录失败：{err}"))?;
    let mut written = Vec::new();
    for doc in docs {
        let path = unique_theme_path(dir, &doc.stem);
        toml_io::save_text(&path, &doc.text)
            .map_err(|err| format!("写入 {} 失败：{err}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

/// Converts Zed `.json` files that were dropped straight into `themes/`.
fn import_zed_files_in_dir(dir: &Path, scan: &mut ThemeScan) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && has_extension(path, "json"))
        .collect();
    files.sort();
    for json_path in files {
        let stem = file_stem(&json_path);
        let base = format!("{}-zed", slugify(&stem));
        let json_mtime = fs::metadata(&json_path)
            .and_then(|meta| meta.modified())
            .ok();
        // Skip reconversion while any generated TOML is newer than the JSON;
        // otherwise editing a Zed file would rewrite its conversion on every scan.
        let already_converted = fs::read_dir(dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                has_extension(&entry.path(), "toml") && name.starts_with(&base)
            })
            .filter_map(|entry| {
                fs::metadata(entry.path())
                    .and_then(|meta| meta.modified())
                    .ok()
            })
            .any(|toml_mtime| match json_mtime {
                Some(json_mtime) => toml_mtime >= json_mtime,
                None => true,
            });
        if already_converted {
            continue;
        }
        let json = match fs::read_to_string(&json_path) {
            Ok(json) => json,
            Err(err) => {
                scan.failures.push(ThemeFailure {
                    path: json_path,
                    name: stem,
                    error: format!("读取 Zed 主题失败：{err}"),
                });
                continue;
            }
        };
        let docs = match zed_json_to_toml_documents(&json) {
            Ok(docs) => docs,
            Err(err) => {
                scan.failures.push(ThemeFailure {
                    path: json_path,
                    name: stem,
                    error: format!("Zed 主题转换失败：{err}"),
                });
                continue;
            }
        };
        for (index, doc) in docs.iter().enumerate() {
            let stem = if docs.len() == 1 {
                format!("{}-zed", slugify(&file_stem(&json_path)))
            } else {
                format!("{}-zed-{}", slugify(&file_stem(&json_path)), index + 1)
            };
            let path = dir.join(format!("{stem}.toml"));
            if let Err(err) = toml_io::save_text(&path, &doc.text) {
                scan.failures.push(ThemeFailure {
                    path,
                    name: doc.name.clone(),
                    error: format!("写入转换结果失败：{err}"),
                });
            }
        }
    }
}

/// A commented example theme, created on demand from the settings panel so
/// developers have a starting point without shipping a fake built-in theme.
pub fn example_theme_toml() -> &'static str {
    "\
# 墨笺 md-work 扩展主题示例 —— 复制到 ~/.config/md-work/themes/ 即可自动载入。
# 文件名（不含 .toml）就是主题 id，可在 theme_extension.toml 里写成 active = \"ext:my-theme\"。
[theme]
name = \"My Theme\"
author = \"Your Name\"
description = \"一个暗色示例主题\"
mode = \"dark\"

[colors]
background = \"#181825\"
foreground = \"#cdd6f4\"
\"muted.foreground\" = \"#a6adc8\"
border = \"#313244\"
\"accent.background\" = \"#313244\"
\"accent.foreground\" = \"#cdd6f4\"
\"primary.background\" = \"#89b4fa\"
\"primary.foreground\" = \"#1e1e2e\"
\"popover.background\" = \"#1e1e2e\"
\"popover.foreground\" = \"#cdd6f4\"
\"sidebar.background\" = \"#11111b\"
\"sidebar.foreground\" = \"#cdd6f4\"
\"list.hover.background\" = \"#313244\"
\"list.active.background\" = \"#45475a\"

[radius]
normal = 6
large = 8
shadow = true

[highlight]
\"editor.background\" = \"#181825\"
\"editor.foreground\" = \"#cdd6f4\"
\"editor.line_number\" = \"#6c7086\"

[highlight.syntax]
keyword = \"#cba6f7\"
string = \"#a6e3a1\"
function = \"#89b4fa\"
number = \"#fab387\"
comment = \"#6c7086\"
"
}

// ---------------------------------------------------------------------------
// Directory watcher
// ---------------------------------------------------------------------------

const WATCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// Keeps the theme directory watch alive; dropping it stops the watch.
pub struct ThemeWatcher {
    _inner: notify::RecommendedWatcher,
}

impl ThemeWatcher {
    /// Watches both the config root (for `theme_extension.toml`) and `themes/`.
    pub fn start(root: &Path, themes_dir: &Path) -> crate::Result<(Self, Receiver<Vec<PathBuf>>)> {
        fs::create_dir_all(root)?;
        fs::create_dir_all(themes_dir)?;
        let (raw_tx, raw_rx) = channel::<Vec<PathBuf>>();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    return;
                };
                if event.kind.is_access() {
                    return;
                }
                let _ = raw_tx.send(event.paths);
            })?;
        watcher.watch(root, RecursiveMode::NonRecursive)?;
        watcher.watch(themes_dir, RecursiveMode::NonRecursive)?;

        let (tx, rx) = channel::<Vec<PathBuf>>();
        std::thread::spawn(move || {
            while let Ok(mut batch) = raw_rx.recv() {
                let deadline = Instant::now() + WATCH_DEBOUNCE;
                loop {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match raw_rx.recv_timeout(deadline - now) {
                        Ok(more) => batch.extend(more),
                        Err(_) => break,
                    }
                }
                batch.retain(|path| has_extension(path, "toml") || has_extension(path, "json"));
                if !batch.is_empty() && tx.send(batch).is_err() {
                    break;
                }
            }
        });
        Ok((Self { _inner: watcher }, rx))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Zed `style` key -> gpui-component `ThemeConfigColors` key. Earlier entries
/// win, so `editor.background` (the editor surface) beats the chrome colour.
const ZED_COLOR_MAP: &[(&str, &str)] = &[
    ("editor.background", "background"),
    ("terminal.background", "background"),
    ("panel.background", "sidebar.background"),
    ("background", "sidebar.background"),
    ("editor.gutter.background", "sidebar.background"),
    ("editor.foreground", "foreground"),
    ("text", "foreground"),
    ("text.accent", "accent.foreground"),
    ("accent", "accent.background"),
    ("element.background", "accent.background"),
    ("border", "border"),
    ("editor.indent_guide", "border"),
    ("border.focused", "ring"),
    ("border.selected", "ring"),
    ("text.muted", "muted.foreground"),
    ("icon.muted", "muted.foreground"),
    ("element.hover", "muted.background"),
    ("editor.active_line.background", "muted.background"),
    ("editor.highlighted_line.background", "muted.background"),
    ("ghost_element.hover", "list.hover.background"),
    ("element.hover", "list.hover.background"),
    ("element.active", "list.active.background"),
    ("element.selected", "list.active.background"),
    ("ghost_element.selected", "list.active.background"),
    ("surface.background", "popover.background"),
    ("elevated_surface.background", "popover.background"),
    ("panel.background", "popover.background"),
    ("text", "popover.foreground"),
    ("editor.foreground", "popover.foreground"),
    ("tab.active_background", "tab.active.background"),
    ("tab.inactive_background", "tab.background"),
    ("tab_bar.background", "tab_bar.background"),
    ("title_bar.background", "title_bar.background"),
    ("status_bar.background", "status_bar.background"),
    ("panel.focused_border", "sidebar.border"),
    ("border.focused", "sidebar.border"),
    ("scrollbar.thumb.background", "scrollbar.thumb.background"),
    ("scrollbar.track.background", "scrollbar.background"),
    ("search.match_background", "selection.background"),
    (
        "editor.document_highlight.read_background",
        "selection.background",
    ),
    ("drop_target.background", "drop_target.background"),
    ("error", "danger.background"),
    ("conflict", "danger.background"),
    ("warning", "warning.background"),
    ("modified", "warning.background"),
    ("success", "success.background"),
    ("created", "success.background"),
    ("info", "info.background"),
    ("hint", "info.background"),
    ("text.placeholder", "input.border"),
];

const NO_ITALIC: &[&str] = &["emphasis", "emphasis.strong", "link_text", "link_uri"];

fn zed_syntax_value(key: &str, value: &Value) -> Option<String> {
    let mut font_style = None;
    let mut font_weight = None;
    let color = match value {
        Value::String(raw) => normalize_color(raw)?,
        Value::Object(map) => {
            let color = map
                .get("color")
                .and_then(Value::as_str)
                .and_then(normalize_color)?;
            if let Some(raw) = map.get("font_style").and_then(Value::as_str) {
                font_style = Some(match raw {
                    "italic" | "underline" => raw.to_string(),
                    _ => "normal".to_string(),
                });
            }
            if let Some(weight) = map.get("font_weight").and_then(Value::as_u64) {
                let normalized = ((weight + 50) / 100 * 100).clamp(100, 900);
                font_weight = Some(normalized);
            }
            color
        }
        _ => return None,
    };
    // Source text is a plain Markdown document: italic emphasis would flicker
    // as `_` / `*` markers are typed. The built-in themes follow the same rule.
    let style = if NO_ITALIC.contains(&key) {
        Some("normal".to_string())
    } else {
        font_style
    };
    if style.is_none() && font_weight.is_none() {
        return Some(toml_quote(&color));
    }
    let mut parts = vec![format!("color = {}", toml_quote(&color))];
    if let Some(style) = style {
        parts.push(format!("font_style = {}", toml_quote(&style)));
    }
    if let Some(weight) = font_weight {
        parts.push(format!("font_weight = {weight}"));
    }
    Some(format!("{{ {} }}", parts.join(", ")))
}

fn collect_color_strings(table: &toml::Table, prefix: &str, out: &mut BTreeMap<String, String>) {
    for (key, value) in table {
        let dotted = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::String(text) => {
                out.insert(dotted, text.clone());
            }
            toml::Value::Table(nested) => collect_color_strings(nested, &dotted, out),
            _ => {}
        }
    }
}

fn collect_highlight_strings(table: &toml::Table, prefix: &str, out: &mut Map<String, Value>) {
    for (key, value) in table {
        if prefix.is_empty() && key == "syntax" {
            continue;
        }
        let dotted = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::String(text) => {
                if let Some(color) = normalize_color(text) {
                    out.insert(dotted, Value::String(color));
                }
            }
            toml::Value::Table(nested) => collect_highlight_strings(nested, &dotted, out),
            _ => {}
        }
    }
}

fn build_syntax(table: &toml::Table) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in table {
        let mut style = Map::new();
        match value {
            toml::Value::String(raw) => {
                let Some(color) = normalize_color(raw) else {
                    continue;
                };
                style.insert("color".to_string(), Value::String(color));
            }
            toml::Value::Table(fields) => {
                let Some(color) = fields
                    .get("color")
                    .and_then(|value| value.as_str())
                    .and_then(normalize_color)
                else {
                    continue;
                };
                style.insert("color".to_string(), Value::String(color));
                if let Some(raw) = fields.get("font_style").and_then(|value| value.as_str()) {
                    let raw = match raw {
                        "italic" | "underline" => raw,
                        _ => "normal",
                    };
                    style.insert("font_style".to_string(), Value::String(raw.to_string()));
                }
                if let Some(weight) = fields
                    .get("font_weight")
                    .and_then(|value| value.as_integer())
                {
                    let normalized = ((weight as i64 + 50) / 100 * 100).clamp(100, 900);
                    style.insert(
                        "font_weight".to_string(),
                        Value::Number(serde_json::Number::from(normalized)),
                    );
                }
            }
            _ => continue,
        }
        if NO_ITALIC.contains(&key.as_str()) {
            style.insert(
                "font_style".to_string(),
                Value::String("normal".to_string()),
            );
        }
        out.insert(key.clone(), Value::Object(style));
    }
    out
}

fn normalize_color(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let hex = raw.strip_prefix('#')?;
    let valid = matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit());
    valid.then(|| format!("#{}", hex.to_ascii_lowercase()))
}

fn non_empty_str(table: &toml::Table, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn positive_size(table: &toml::Table, key: &str) -> Option<f32> {
    let value = table.get(key)?;
    let size = match value {
        toml::Value::Float(size) => *size,
        toml::Value::Integer(size) => *size as f64,
        _ => return None,
    };
    (8.0..=48.0).contains(&size).then(|| size as f32)
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(expected))
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "theme".to_string())
}

pub fn slugify(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn unique_theme_path(dir: &Path, stem: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}.toml"));
    let mut index = 1;
    while path.exists() {
        path = dir.join(format!("{stem}-{index}.toml"));
        index += 1;
    }
    path
}

pub fn toml_quote(raw: &str) -> String {
    let mut out = String::from("\"");
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
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

fn toml_key(key: &str) -> String {
    if key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        key.to_string()
    } else {
        toml_quote(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrips_through_toml() {
        let config = ThemeExtensionConfig {
            active: ActiveTheme::Extension("my-dark".to_string()),
            load_policy: ThemeLoadPolicy::Startup,
            fallback: ThemeFallbackPolicy::KeepCurrent,
        };
        let parsed: toml::Table = toml::from_str(&config.to_toml_text()).unwrap();
        assert_eq!(parsed["active"].as_str(), Some("ext:my-dark"));
        assert_eq!(parsed["load_policy"].as_str(), Some("startup"));
        assert_eq!(parsed["fallback"].as_str(), Some("keep-current"));
    }

    #[test]
    fn native_theme_parses_dotted_and_nested_colors() {
        let text = r##"
[theme]
name = "Test Dark"
mode = "dark"

[colors]
background = "#101010"
foreground = "#eeeeee"
"accent.background" = "#202020"
[colors.sidebar]
background = "#303030"

[highlight]
"editor.background" = "#101010"
[highlight.syntax]
keyword = "#ff00ff"
[highlight.syntax.comment]
color = "#808080"
font_style = "italic"
"##;
        let theme = parse_theme_toml(Path::new("test.toml"), text).unwrap();
        assert_eq!(theme.id, "test");
        assert_eq!(theme.mode, "dark");
        let colors = theme.config["colors"].as_object().unwrap();
        assert_eq!(colors["accent.background"], "#202020");
        assert_eq!(colors["sidebar.background"], "#303030");
        let syntax = theme.config["highlight"]["syntax"].as_object().unwrap();
        assert_eq!(syntax["keyword"]["color"], "#ff00ff");
        assert_eq!(syntax["comment"]["font_style"], "italic");
    }

    #[test]
    fn broken_theme_reports_a_clear_error() {
        let error =
            parse_theme_toml(Path::new("bad.toml"), "[theme]\nmode = \"dark\"\n").unwrap_err();
        assert!(error.contains("name"), "{error}");
    }

    #[test]
    fn scanning_a_theme_directory_reports_success_and_failure() {
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mdwork-theme-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("config");
        let themes = dir.join("themes");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&themes).unwrap();

        let config_path = root.join("theme_extension.toml");
        let config = ThemeExtensionConfig {
            active: ActiveTheme::Extension("good".to_string()),
            load_policy: ThemeLoadPolicy::Auto,
            fallback: ThemeFallbackPolicy::KeepCurrent,
        };
        config.save(&config_path).unwrap();
        let (loaded, issues) = ThemeExtensionConfig::load(&config_path, None);
        assert!(issues.is_empty());
        assert_eq!(loaded, config);

        fs::write(
            themes.join("good.toml"),
            "[theme]\nname = \"Good\"\nmode = \"light\"\n[colors]\nbackground = \"#ffffff\"\n",
        )
        .unwrap();
        fs::write(themes.join("bad.toml"), "[theme]\nmode = \"dark\"\n").unwrap();
        let scan = scan_themes(&themes);
        assert_eq!(scan.themes.len(), 1);
        assert_eq!(scan.themes[0].id, "good");
        assert_eq!(scan.failures.len(), 1);
        assert!(scan.failures[0].error.contains("name"));

        let zed = r##"{
            "name": "Zed",
            "themes": [{
                "name": "Zed Light",
                "appearance": "light",
                "style": {
                    "editor.background": "#fafafa",
                    "editor.foreground": "#111111",
                    "syntax": { "keyword": "#0000ff" }
                }
            }]
        }"##;
        fs::write(themes.join("zed.json"), zed).unwrap();
        let second = scan_themes(&themes);
        assert_eq!(second.themes.len(), 2, "Zed JSON 应自动转换为 TOML 并载入");
        assert!(
            second
                .themes
                .iter()
                .any(|theme| theme.name == "Zed Light" && theme.source == "zed")
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn zed_theme_converts_to_loadable_toml() {
        let zed = r##"{
            "name": "Example",
            "author": "Someone",
            "themes": [{
                "name": "Example Dark",
                "appearance": "dark",
                "style": {
                    "background": "#111111",
                    "editor.background": "#1e1e2e",
                    "editor.foreground": "#cdd6f4",
                    "border": "#313244",
                    "error": "#ff0000",
                    "syntax": {
                        "keyword": "#cba6f7",
                        "comment": { "color": "#6c7086", "font_style": "italic" },
                        "emphasis": { "color": "#ffffff", "font_style": "italic" }
                    }
                }
            }]
        }"##;
        let docs = zed_json_to_toml_documents(zed).unwrap();
        assert_eq!(docs.len(), 1);
        let theme = parse_theme_toml(Path::new("example-dark.toml"), &docs[0].text).unwrap();
        assert_eq!(theme.name, "Example Dark");
        assert_eq!(theme.mode, "dark");
        assert_eq!(theme.source, "zed");
        assert_eq!(theme.config["colors"]["background"], "#1e1e2e");
        let syntax = theme.config["highlight"]["syntax"].as_object().unwrap();
        assert_eq!(syntax["emphasis"]["font_style"], "normal");
    }
}
