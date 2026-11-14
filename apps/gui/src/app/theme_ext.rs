//! AppShell integration for the pluggable theme system.
//!
//! Keeps `theme_extension.toml`, the scanned extension themes, the directory
//! watcher and the resulting notification messages together, and renders the
//! matching controls in 「设置 → 外观」.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gpui_kit::Axis;
use gpui_kit::base::{ScrollableMask, Scrollbar, ScrollbarMode, Selectable as _};
use mdwork_core::config::ThemePreference;
use mdwork_core::theme_ext::{
    ActiveTheme, ExtensionTheme, ThemeExtensionConfig, ThemeFailure, ThemeFallbackPolicy,
    ThemeLoadPolicy, ThemeWatcher, content_signature, example_theme_toml, import_zed_theme,
    main_config_path, scan_themes, themes_dir,
};
use rust_i18n::t;

use super::util::*;
use super::*;

/// Group id for the three built-in themes in the settings list.
const BUILTIN_GROUP: &str = "builtin";
/// Fixed row height and viewport height of the theme list.
const THEME_ROW_HEIGHT: f32 = 34.;
const THEME_LIST_HEIGHT: f32 = 238.;

/// One row of the theme list. Groups and options share a fixed row height.
#[derive(Clone)]
enum ThemeRow {
    Group {
        id: String,
        label: String,
        count: usize,
        expanded: bool,
    },
    Option {
        label: String,
        detail: String,
        icon: IconName,
        target: ActiveTheme,
        selected: bool,
    },
}

/// Runtime state of the theme extension system.
pub struct ThemeExtensionState {
    pub config: ThemeExtensionConfig,
    pub themes: Vec<ExtensionTheme>,
    pub failures: Vec<ThemeFailure>,
    /// Content hashes of every theme file seen in the last scan.
    signatures: HashMap<PathBuf, u64>,
    /// Remembered display names, used to report removed files.
    names: HashMap<PathBuf, String>,
    config_signature: u64,
    /// Last successfully loaded active extension theme (for `keep-current`).
    last_good: Option<ExtensionTheme>,
    /// Collapsed theme groups, keyed by `collection_id`.
    pub collapsed: HashSet<String>,
    notices: Vec<(NoticeKind, String)>,
}

impl ThemeExtensionState {
    pub fn load(legacy_mode: ThemePreference) -> Self {
        let path = main_config_path();
        let (config, issues) = ThemeExtensionConfig::load(&path, Some(legacy_mode));
        let mut state = Self {
            config,
            themes: Vec::new(),
            failures: Vec::new(),
            signatures: HashMap::new(),
            names: HashMap::new(),
            config_signature: content_signature(&path),
            last_good: None,
            collapsed: HashSet::new(),
            notices: Vec::new(),
        };
        for issue in issues {
            state.push(
                NoticeKind::Warning,
                t!("theme_ext.config_issue", message = issue).to_string(),
            );
        }
        state.scan(true);
        state.ensure_active();
        state
    }

    fn push(&mut self, kind: NoticeKind, text: String) {
        self.notices.push((kind, text));
    }

    pub fn take_notices(&mut self) -> Vec<(NoticeKind, String)> {
        std::mem::take(&mut self.notices)
    }

    /// The extension theme that should be on screen right now, if any.
    pub fn active_definition(&self) -> Option<ExtensionTheme> {
        let ActiveTheme::Extension(id) = &self.config.active else {
            return None;
        };
        if !self.config.load_policy.loads() {
            return None;
        }
        self.themes
            .iter()
            .find(|theme| &theme.id == id)
            .cloned()
            .or_else(|| {
                (self.config.fallback == ThemeFallbackPolicy::KeepCurrent)
                    .then(|| self.last_good.clone())
                    .flatten()
            })
    }

    /// Re-scans the themes directory, emitting a notification for every new,
    /// changed, broken or removed theme file.
    pub fn rescan(&mut self) {
        self.scan(false);
    }

    fn scan(&mut self, initial: bool) {
        if !self.config.load_policy.loads() {
            self.themes.clear();
            self.failures.clear();
            self.signatures.clear();
            self.names.clear();
            return;
        }
        let report = scan_themes(&themes_dir());
        // A file that parses as TOML but cannot be applied to GPUI is a failed
        // load as far as the user is concerned: report it as such rather than
        // claiming success and silently falling back to a built-in theme.
        let mut themes = Vec::new();
        let mut failures = report.failures;
        for theme in report.themes {
            match serde_json::from_value::<gpui_kit::component::ThemeConfig>(theme.config.clone()) {
                Ok(_) => themes.push(theme),
                Err(err) => failures.push(ThemeFailure {
                    path: theme.path.clone(),
                    name: theme.name.clone(),
                    error: format!("主题格式与组件库不兼容：{err}"),
                }),
            }
        }
        let mut signatures = HashMap::new();
        let mut names = HashMap::new();
        let mut file_counts: HashMap<PathBuf, usize> = HashMap::new();

        for theme in &themes {
            signatures.insert(theme.path.clone(), content_signature(&theme.path));
            *file_counts.entry(theme.path.clone()).or_insert(0) += 1;
            names
                .entry(theme.path.clone())
                .or_insert_with(|| theme.collection.clone());
            if self.config.active == ActiveTheme::Extension(theme.id.clone()) {
                self.last_good = Some(theme.clone());
            }
        }
        // One notification per file: a Zed file can contain many variants.
        for (path, label) in &names {
            let count = file_counts.get(path).copied().unwrap_or(1);
            let changed = self.signatures.get(path) != signatures.get(path);
            if initial || changed {
                let name = if count > 1 {
                    format!("{label} · {}", t!("theme_ext.group_count", count = count))
                } else {
                    label.clone()
                };
                self.push(
                    NoticeKind::Success,
                    t!("theme_ext.found_ok", name = name).to_string(),
                );
            }
        }
        for failure in &failures {
            let signature = content_signature(&failure.path);
            let changed = self.signatures.get(&failure.path) != Some(&signature);
            if initial || changed {
                self.push(
                    NoticeKind::Error,
                    t!(
                        "theme_ext.found_fail",
                        name = failure.name.clone(),
                        message = failure.error.clone()
                    )
                    .to_string(),
                );
            }
            signatures.insert(failure.path.clone(), signature);
            names.insert(failure.path.clone(), failure.name.clone());
        }
        if !initial {
            let removed: Vec<String> = self
                .names
                .iter()
                .filter(|(path, _)| !names.contains_key(*path))
                .map(|(_, name)| name.clone())
                .collect();
            for name in removed {
                self.push(
                    NoticeKind::Info,
                    t!("theme_ext.removed", name = name).to_string(),
                );
            }
        }

        self.signatures = signatures;
        self.names = names;
        self.themes = themes;
        self.failures = failures;
    }

    /// Applies the configured error fallback when the active theme vanished.
    /// Returns the new active theme when it changed on disk.
    pub fn ensure_active(&mut self) -> Option<ActiveTheme> {
        let ActiveTheme::Extension(id) = self.config.active.clone() else {
            return None;
        };
        let available =
            self.config.load_policy.loads() && self.themes.iter().any(|theme| theme.id == id);
        if available
            || (self.config.fallback == ThemeFallbackPolicy::KeepCurrent
                && self.last_good.is_some())
        {
            return None;
        }
        let next = match self.config.fallback {
            ThemeFallbackPolicy::Builtin => {
                if self
                    .last_good
                    .as_ref()
                    .is_some_and(|theme| theme.mode == "dark")
                {
                    ActiveTheme::BuiltinDark
                } else {
                    ActiveTheme::BuiltinLight
                }
            }
            _ => ActiveTheme::System,
        };
        self.config.active = next.clone();
        self.save_config();
        self.push(
            NoticeKind::Warning,
            t!("theme_ext.fallback_applied").to_string(),
        );
        Some(next)
    }

    /// Re-reads `theme_extension.toml` after an external edit. Returns true
    /// when any setting actually changed.
    pub fn reload_main_config(&mut self) -> bool {
        let path = main_config_path();
        let signature = content_signature(&path);
        if signature == self.config_signature {
            return false;
        }
        self.config_signature = signature;
        let (config, issues) = ThemeExtensionConfig::reload(&path);
        for issue in issues {
            self.push(
                NoticeKind::Warning,
                t!("theme_ext.config_issue", message = issue).to_string(),
            );
        }
        let changed = config != self.config;
        self.config = config;
        if changed {
            self.push(
                NoticeKind::Info,
                t!("theme_ext.config_reloaded").to_string(),
            );
        }
        changed
    }

    pub fn save_config(&mut self) {
        if let Err(err) = self.config.save(&main_config_path()) {
            self.push(
                NoticeKind::Error,
                t!("theme_ext.save_failed", message = err).to_string(),
            );
        }
        self.config_signature = content_signature(&main_config_path());
    }
}

/// 「设置 → 外观」主题列表的独立滚动区。
///
/// 裸 `overflow_y_scroll()` 放在 flex 列中时，固定高度的主题行会被一起压缩
/// 进 `max_h` 里，列表没有可滚动余量；滚轮事件还会冒泡到外层的设置页滚动
/// 容器，表现为「滚轮翻的是外观面板，主题列表纹丝不动」。这里固定视口高度、
/// 让内容层不被 flex 压缩，并用垂直 `ScrollableMask` 在捕获阶段消费滚轮：
/// 先翻主题，滚到边界后才把事件让给外观页（与平台滚动容器一致）。
fn theme_options_scroll_area(rows: Vec<AnyElement>, scroll: &ScrollHandle) -> AnyElement {
    let content = div()
        .v_flex()
        .w_full()
        .gap_0p5()
        // 给常驻滚动条留出右侧空间，避免盖住主题名与选中勾。
        .pr_3()
        .flex_none()
        .h_auto()
        .min_h_full()
        .children(rows);

    div()
        .id("settings-theme-options")
        .relative()
        .w_full()
        .h(px(THEME_LIST_HEIGHT))
        .flex_none()
        .child(
            div()
                .id("settings-theme-options-viewport")
                .size_full()
                .v_flex()
                .track_scroll(scroll)
                .overflow_y_scroll()
                .lock_scroll_axis()
                .child(content),
        )
        .child(ScrollableMask::new(Axis::Vertical, scroll))
        // 用 handle 记录的真实视口 bounds。`viewport_from_layout()` 会让覆盖层
        // 按自身绝对定位布局绘制，在 auto 高度的容器里会掉到内容之后、跑出卡片。
        .child(Scrollbar::vertical(scroll).mode(ScrollbarMode::Always))
        .into_any_element()
}

impl AppShell {
    fn sync_theme_preference(&mut self) {
        self.config.theme =
            preference_for_active(&self.theme_ext.config.active, &self.theme_ext.themes);
    }

    fn drain_theme_notices(&mut self, cx: &mut Context<Self>) {
        for (kind, text) in self.theme_ext.take_notices() {
            self.notify_with(kind, text, cx);
        }
    }

    /// Starts (or restarts) the directory watcher according to the load policy.
    pub(super) fn start_theme_watcher(&mut self, cx: &mut Context<Self>) {
        self._theme_watcher = None;
        self.theme_changed = None;
        if !self.theme_ext.config.load_policy.watches() {
            return;
        }
        let root = mdwork_core::paths::user_config_root();
        match ThemeWatcher::start(&root, &themes_dir()) {
            Ok((watcher, rx)) => {
                self._theme_watcher = Some(watcher);
                self.theme_changed = Some(Arc::new(Mutex::new(rx)));
            }
            Err(err) => {
                tracing::warn!(error = %err, "主题目录监视不可用");
                self.notify_warning(
                    t!("theme_ext.watch_failed", message = err.to_string()).to_string(),
                    cx,
                );
            }
        }
    }

    /// Applies the currently selected theme immediately.
    pub(super) fn apply_active_theme(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let extension = self.theme_ext.active_definition();
        crate::theme::apply(&self.config, extension.as_ref(), window, cx);
    }

    pub(super) fn set_active_theme(
        &mut self,
        active: ActiveTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.theme_ext.config.active == active {
            return;
        }
        if active.is_extension()
            && !self
                .theme_ext
                .themes
                .iter()
                .any(|theme| active == ActiveTheme::Extension(theme.id.clone()))
        {
            self.notify_warning(t!("theme_ext.not_found").to_string(), cx);
            return;
        }
        let previous = self.theme_ext.config.active.clone();
        self.theme_ext.config.active = active.clone();
        self.theme_ext.save_config();
        self.sync_theme_preference();
        self.save_config(cx);
        self.apply_active_theme(Some(window), cx);
        self.drain_theme_notices(cx);
        tracing::info!(from = %previous.as_str(), to = %active.as_str(), "切换主题");
        cx.notify();
    }

    pub(super) fn set_theme_load_policy(
        &mut self,
        policy: ThemeLoadPolicy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.theme_ext.config.load_policy == policy {
            return;
        }
        self.theme_ext.config.load_policy = policy;
        self.theme_ext.save_config();
        self.start_theme_watcher(cx);
        self.theme_ext.rescan();
        if let Some(active) = self.theme_ext.ensure_active() {
            self.theme_ext.config.active = active;
            self.sync_theme_preference();
            self.save_config(cx);
        }
        self.apply_active_theme(Some(window), cx);
        self.drain_theme_notices(cx);
        cx.notify();
    }

    pub(super) fn set_theme_fallback(
        &mut self,
        policy: ThemeFallbackPolicy,
        cx: &mut Context<Self>,
    ) {
        if self.theme_ext.config.fallback == policy {
            return;
        }
        self.theme_ext.config.fallback = policy;
        self.theme_ext.save_config();
        self.drain_theme_notices(cx);
        cx.notify();
    }

    /// Collapses / expands one theme group in the settings list.
    pub(super) fn toggle_theme_group(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.theme_ext.collapsed.insert(id.clone()) {
            self.theme_ext.collapsed.remove(&id);
        }
        cx.notify();
    }

    /// Expands or collapses every theme group at once.
    pub(super) fn set_all_theme_groups(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if collapsed {
            let mut ids: HashSet<String> = self
                .theme_ext
                .themes
                .iter()
                .map(|theme| theme.collection_id.clone())
                .collect();
            ids.insert(BUILTIN_GROUP.to_string());
            self.theme_ext.collapsed = ids;
        } else {
            self.theme_ext.collapsed.clear();
        }
        cx.notify();
    }

    /// Handles a batch of paths from the theme directory watcher.
    pub(super) fn handle_theme_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let config_path = main_config_path();
        let mut config_touched = false;
        let mut themes_touched = false;
        for path in paths {
            if path == config_path {
                config_touched = true;
            } else {
                themes_touched = true;
            }
        }
        let mut policy_changed = false;
        if config_touched && self.theme_ext.reload_main_config() {
            policy_changed = true;
            self.sync_theme_preference();
            self.save_config(cx);
        }
        if themes_touched || policy_changed {
            self.theme_ext.rescan();
            if let Some(active) = self.theme_ext.ensure_active() {
                self.theme_ext.config.active = active;
                self.sync_theme_preference();
                self.save_config(cx);
            }
        }
        if policy_changed {
            self.start_theme_watcher(cx);
        }
        self.apply_active_theme(Some(window), cx);
        self.drain_theme_notices(cx);
        cx.notify();
    }

    /// Manual rescan from the settings panel.
    pub(super) fn rescan_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme_ext.rescan();
        if let Some(active) = self.theme_ext.ensure_active() {
            self.theme_ext.config.active = active;
            self.sync_theme_preference();
            self.save_config(cx);
        }
        self.apply_active_theme(Some(window), cx);
        self.drain_theme_notices(cx);
        cx.notify();
    }

    /// Opens a native file picker and converts the chosen Zed `.json` theme.
    pub(super) fn import_zed_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("theme_ext.import_prompt").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = picked else {
                return;
            };
            let result = import_zed_theme(&path, &themes_dir());
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(imported) => {
                    this.notify_success(
                        t!("theme_ext.import_ok", count = imported.theme_count).to_string(),
                        cx,
                    );
                    this.rescan_themes(window, cx);
                }
                Err(err) => {
                    this.notify_error(t!("theme_ext.import_failed", message = err).to_string(), cx);
                }
            });
        })
        .detach();
    }

    /// Writes a commented example theme so developers have a starting point.
    pub(super) fn create_example_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = themes_dir();
        let mut path = dir.join("my-theme.toml");
        let mut index = 1;
        while path.exists() {
            path = dir.join(format!("my-theme-{index}.toml"));
            index += 1;
        }
        match mdwork_core::toml_io::save_text(&path, example_theme_toml()) {
            Ok(()) => {
                self.notify_info(
                    t!("theme_ext.example_ok", path = path.display().to_string()).to_string(),
                    cx,
                );
                self.rescan_themes(window, cx);
            }
            Err(err) => self.notify_error(
                t!("theme_ext.example_failed", message = err.to_string()).to_string(),
                cx,
            ),
        }
    }

    pub(super) fn open_themes_dir(&mut self, cx: &mut Context<Self>) {
        self.open_path_in_explorer(Some(themes_dir()), cx);
    }

    /// 标题栏上的主题入口。菜单里的每一项都走 `set_active_theme`，所以切换
    /// 效果、「theme_extension.toml」持久化与「设置 → 外观」完全相同。
    pub(super) fn render_theme_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.theme_menu;
        let trigger = title_icon(
            "title-theme",
            IconName::Palette,
            t!("theme.label").to_string(),
        )
        .selected(open)
        // 用渲染时的状态取反：浮层的 `on_mouse_down_out` 会在同一次点击里
        // 先把菜单关掉，若这里再读运行时状态取反，按钮就关不掉菜单了。
        .on_click(cx.listener(move |this, _, _, cx| {
            this.theme_menu = !open;
            if this.theme_menu {
                this.workspace_menu = false;
                this.notice_panel = false;
            }
            cx.notify();
        }));

        let mut popup = gpui_kit::base::Popup::new("theme-popup", trigger)
            .anchor(Anchor::TopRight)
            .offset(px(6.));
        if open {
            popup = popup.content(self.render_theme_menu(cx));
        }
        titlebar_guard(popup)
    }

    /// 列出当前已载入的全部主题：内置三项，加上按主题文件分组的扩展主题。
    fn render_theme_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = SettingsPalette::new(cx);
        let scroll = self.theme_menu_scroll.clone();
        let active = self.theme_ext.config.active.clone();
        let current = match &active {
            ActiveTheme::System => t!("theme.system").to_string(),
            ActiveTheme::BuiltinLight => t!("theme_ext.builtin_light").to_string(),
            ActiveTheme::BuiltinDark => t!("theme_ext.builtin_dark").to_string(),
            ActiveTheme::Extension(id) => self
                .theme_ext
                .themes
                .iter()
                .find(|theme| &theme.id == id)
                .map(|theme| theme.name.clone())
                .unwrap_or_else(|| id.clone()),
        };

        let mut list = div()
            .id("title-theme-list")
            .v_flex()
            .w_full()
            .gap_0p5()
            // 给常驻滚动条留出右侧空间，避免盖住主题名与选中勾。
            .pr_3()
            .max_h(px(320.))
            .track_scroll(&scroll)
            .overflow_y_scroll();

        list = list.child(Self::theme_menu_group(
            t!("theme_ext.group_builtin").to_string(),
            p,
        ));
        for (index, (label, icon, target)) in [
            (
                t!("theme.system").to_string(),
                IconName::Monitor,
                ActiveTheme::System,
            ),
            (
                t!("theme_ext.builtin_light").to_string(),
                IconName::Sun,
                ActiveTheme::BuiltinLight,
            ),
            (
                t!("theme_ext.builtin_dark").to_string(),
                IconName::Moon,
                ActiveTheme::BuiltinDark,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let selected = active == target;
            list = list.child(Self::theme_menu_row(
                format!("title-theme-builtin-{index}"),
                label,
                String::new(),
                icon,
                target,
                selected,
                p,
                cx,
            ));
        }

        // 与设置页一样按主题文件分组，一个文件里的多个变体归到同一组。
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        let mut group_index: HashMap<String, usize> = HashMap::new();
        for (index, theme) in self.theme_ext.themes.iter().enumerate() {
            let key = theme.collection_id.clone();
            let slot = *group_index.entry(key).or_insert_with(|| {
                groups.push((theme.collection.clone(), Vec::new()));
                groups.len() - 1
            });
            groups[slot].1.push(index);
        }
        if groups.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(p.muted)
                    .child(t!("theme.menu.no_themes").to_string()),
            );
        }
        for (group_no, (label, indexes)) in groups.into_iter().enumerate() {
            list = list.child(Self::theme_menu_group(label, p));
            for (row, index) in indexes.into_iter().enumerate() {
                let theme = &self.theme_ext.themes[index];
                let detail = format!(
                    "{} · {}",
                    if theme.mode == "dark" {
                        t!("theme.dark").to_string()
                    } else {
                        t!("theme.light").to_string()
                    },
                    if theme.author.is_empty() {
                        theme.source.clone()
                    } else {
                        theme.author.clone()
                    }
                );
                let target = ActiveTheme::Extension(theme.id.clone());
                let selected = active == target;
                list = list.child(Self::theme_menu_row(
                    format!("title-theme-ext-{group_no}-{row}"),
                    theme.name.clone(),
                    detail,
                    IconName::Palette,
                    target,
                    selected,
                    p,
                    cx,
                ));
            }
        }

        // 滚动条作为滚动容器的兄弟浮层，不随内容一起滚走；Always 模式下
        // 轨道常驻，thumb 的长度 / 位置直接反映主题总量与浏览进度。
        let scroll_area = div()
            .id("title-theme-scroll")
            .relative()
            .w_full()
            .child(list)
            // 同上：handle 的视口 bounds 才是列表区域，覆盖层自己的 layout
            // bounds 会落在菜单底部之外。
            .child(Scrollbar::vertical(&scroll).mode(ScrollbarMode::Always))
            .into_any_element();

        let header = div()
            .v_flex()
            .w_full()
            .gap_0p5()
            .px_2()
            .pb_1()
            .border_b_1()
            .border_color(p.border)
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("theme.menu.title").to_string()),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(p.muted)
                    .child(t!("theme.menu.current", name = current).to_string()),
            );

        let footer = Button::new("title-theme-open-settings")
            .ghost()
            .compact()
            .icon(IconName::Settings)
            .label(t!("theme.menu.open_settings").to_string())
            .on_click(cx.listener(|this, _, window, cx| {
                this.theme_menu = false;
                cx.notify();
                this.open_settings(window, cx);
                this.set_settings_tab(SettingsTab::Appearance, cx);
            }));

        div()
            .id("title-theme-menu")
            .v_flex()
            .w(px(288.))
            .p_2()
            .gap_1()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.popover)
            .text_color(p.foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.theme_menu {
                    this.theme_menu = false;
                    cx.notify();
                }
            }))
            .child(header)
            .child(scroll_area)
            .child(footer)
            .into_any_element()
    }

    fn theme_menu_group(label: String, p: SettingsPalette) -> AnyElement {
        div()
            .h(px(26.))
            // max_h 的 flex 列会把固定高度的子行一起压缩，文字会叠在一起。
            .flex_shrink_0()
            .w_full()
            .h_flex()
            .items_center()
            .px_2()
            .rounded_md()
            .bg(p.track)
            .text_xs()
            .font_bold()
            .text_color(p.muted)
            .child(label)
            .into_any_element()
    }

    fn theme_menu_row(
        id: String,
        label: String,
        detail: String,
        icon: IconName,
        target: ActiveTheme,
        selected: bool,
        p: SettingsPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .h(px(34.))
            .flex_shrink_0()
            .w_full()
            .h_flex()
            .items_center()
            .gap_2()
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .bg(if selected { p.popover } else { p.transparent })
            .hover(move |style| style.bg(p.hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                // 与设置页里的主题行一样调用 `set_active_theme`；先关菜单并
                // 显式刷新，避免点击当前主题时 `set_active_theme` 提前返回。
                this.theme_menu = false;
                cx.notify();
                this.set_active_theme(target.clone(), window, cx);
            }))
            .child(
                Icon::new(icon)
                    .xsmall()
                    .text_color(if selected { p.accent } else { p.muted }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .when(selected, |text| text.font_bold())
                            .child(label),
                    )
                    .when(!detail.is_empty(), |row| {
                        row.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(p.muted)
                                .child(detail),
                        )
                    }),
            )
            .when(selected, |row| {
                row.child(Icon::new(IconName::Check).xsmall().text_color(p.success))
            })
            .into_any_element()
    }

    /// Cards shown on the appearance settings page. Each one maps to a key in
    /// `theme_extension.toml` (plus a few one-shot developer actions).
    pub(super) fn render_theme_extension_cards(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = SettingsPalette::new(cx);
        let active = &self.theme_ext.config.active;
        let mut rows: Vec<ThemeRow> = Vec::new();
        let builtin_expanded = !self.theme_ext.collapsed.contains(BUILTIN_GROUP);
        rows.push(ThemeRow::Group {
            id: BUILTIN_GROUP.to_string(),
            label: t!("theme_ext.group_builtin").to_string(),
            count: 3,
            expanded: builtin_expanded,
        });
        if builtin_expanded {
            for (label, icon, target) in [
                (
                    t!("theme.system").to_string(),
                    IconName::Monitor,
                    ActiveTheme::System,
                ),
                (
                    t!("theme_ext.builtin_light").to_string(),
                    IconName::Sun,
                    ActiveTheme::BuiltinLight,
                ),
                (
                    t!("theme_ext.builtin_dark").to_string(),
                    IconName::Moon,
                    ActiveTheme::BuiltinDark,
                ),
            ] {
                rows.push(ThemeRow::Option {
                    label,
                    detail: String::new(),
                    icon,
                    selected: *active == target,
                    target,
                });
            }
        }

        // One group per theme file, in scan order. A Zed file with several
        // variants stays one group instead of a flat list of lookalikes.
        let mut groups: Vec<(String, String, Vec<usize>)> = Vec::new();
        let mut group_index: HashMap<String, usize> = HashMap::new();
        for (index, theme) in self.theme_ext.themes.iter().enumerate() {
            let key = theme.collection_id.clone();
            let slot = *group_index.entry(key.clone()).or_insert_with(|| {
                groups.push((key, theme.collection.clone(), Vec::new()));
                groups.len() - 1
            });
            groups[slot].2.push(index);
        }
        for (id, label, indexes) in groups {
            let expanded = !self.theme_ext.collapsed.contains(&id);
            rows.push(ThemeRow::Group {
                id,
                label,
                count: indexes.len(),
                expanded,
            });
            if !expanded {
                continue;
            }
            for index in indexes {
                let theme = &self.theme_ext.themes[index];
                let detail = format!(
                    "{} · {}",
                    if theme.mode == "dark" {
                        t!("theme.dark").to_string()
                    } else {
                        t!("theme.light").to_string()
                    },
                    if theme.author.is_empty() {
                        theme.source.clone()
                    } else {
                        theme.author.clone()
                    }
                );
                let target = ActiveTheme::Extension(theme.id.clone());
                rows.push(ThemeRow::Option {
                    label: theme.name.clone(),
                    detail,
                    icon: IconName::Palette,
                    selected: *active == target,
                    target,
                });
            }
        }

        let mut option_rows: Vec<AnyElement> = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let element =
                match row {
                    ThemeRow::Group {
                        id,
                        label,
                        count,
                        expanded,
                    } => {
                        let group_id = id.clone();
                        let group_count = *count;
                        let chevron = if *expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        };
                        div()
                            .id(format!("settings-theme-group-{index}"))
                            .h(px(THEME_ROW_HEIGHT))
                            .w_full()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(p.track)
                            .hover(move |style| style.bg(p.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_theme_group(group_id.clone(), cx)
                            }))
                            .child(Icon::new(chevron).xsmall().text_color(p.muted))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_bold()
                                    .child(label.clone()),
                            )
                            .child(div().text_xs().text_color(p.muted).child(
                                t!("theme_ext.group_count", count = group_count).to_string(),
                            ))
                            .into_any_element()
                    }
                    ThemeRow::Option {
                        label,
                        detail,
                        icon,
                        target,
                        selected,
                    } => {
                        let target = target.clone();
                        let icon = *icon;
                        let selected = *selected;
                        div()
                            .id(format!("settings-theme-option-{index}"))
                            .h(px(THEME_ROW_HEIGHT))
                            .w_full()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .pl(px(18.))
                            .pr_2()
                            .rounded_md()
                            .cursor_pointer()
                            .bg(if selected { p.popover } else { p.transparent })
                            .hover(move |style| style.bg(p.hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_active_theme(target.clone(), window, cx)
                            }))
                            .child(Icon::new(icon).xsmall().text_color(if selected {
                                p.accent
                            } else {
                                p.muted
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .when(selected, |text| text.font_bold())
                                    .child(label.clone()),
                            )
                            .when(!detail.is_empty(), |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(p.muted)
                                        .truncate()
                                        .child(detail.clone()),
                                )
                            })
                            .when(selected, |row| {
                                row.child(Icon::new(IconName::Check).xsmall().text_color(p.success))
                            })
                            .into_any_element()
                    }
                };
            option_rows.push(element);
        }
        let scroll_area = theme_options_scroll_area(option_rows, &self.theme_list_scroll);

        let should_collapse = self.theme_ext.collapsed.is_empty();
        let group_actions = div()
            .h_flex()
            .w_full()
            .justify_end()
            .child(
                Button::new("settings-theme-toggle-groups")
                    .ghost()
                    .compact()
                    .label(if should_collapse {
                        t!("theme_ext.collapse_all").to_string()
                    } else {
                        t!("theme_ext.expand_all").to_string()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_all_theme_groups(should_collapse, cx)
                    })),
            )
            .into_any_element();

        let mut status = div()
            .v_flex()
            .w_full()
            .gap_1()
            .child(scroll_area)
            .child(group_actions);
        if self.theme_ext.themes.is_empty() {
            status = status.child(
                div().text_xs().text_color(p.muted).child(
                    t!(
                        "theme_ext.no_themes",
                        path = themes_dir().display().to_string()
                    )
                    .to_string(),
                ),
            );
        }
        for failure in self.theme_ext.failures.iter().take(3) {
            status = status.child(
                div()
                    .h_flex()
                    .items_start()
                    .gap_1()
                    .text_xs()
                    .text_color(p.danger)
                    .child(Icon::new(IconName::TriangleAlert).xsmall())
                    .child(format!("{}：{}", failure.name, failure.error)),
            );
        }

        let active_card = settings_card(
            t!("theme_ext.active").to_string(),
            Some(t!("theme_ext.active_hint").to_string()),
            status.into_any_element(),
            p,
        );

        let policy_active = |policy: ThemeLoadPolicy| self.theme_ext.config.load_policy == policy;
        let policy_track = settings_segment_track(
            vec![
                settings_segment(
                    "settings-theme-policy-auto",
                    t!("theme_ext.policy.auto").to_string(),
                    None,
                    policy_active(ThemeLoadPolicy::Auto),
                    cx.listener(|this, _, window, cx| {
                        this.set_theme_load_policy(ThemeLoadPolicy::Auto, window, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "settings-theme-policy-startup",
                    t!("theme_ext.policy.startup").to_string(),
                    None,
                    policy_active(ThemeLoadPolicy::Startup),
                    cx.listener(|this, _, window, cx| {
                        this.set_theme_load_policy(ThemeLoadPolicy::Startup, window, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "settings-theme-policy-disabled",
                    t!("theme_ext.policy.disabled").to_string(),
                    None,
                    policy_active(ThemeLoadPolicy::Disabled),
                    cx.listener(|this, _, window, cx| {
                        this.set_theme_load_policy(ThemeLoadPolicy::Disabled, window, cx)
                    }),
                    p,
                ),
            ],
            p,
        );
        let policy_card = settings_card(
            t!("theme_ext.load_policy").to_string(),
            Some(t!("theme_ext.load_policy_hint").to_string()),
            policy_track,
            p,
        );

        let fallback_active =
            |policy: ThemeFallbackPolicy| self.theme_ext.config.fallback == policy;
        let fallback_track = settings_segment_track(
            vec![
                settings_segment(
                    "settings-theme-fallback-builtin",
                    t!("theme_ext.fallback.builtin").to_string(),
                    None,
                    fallback_active(ThemeFallbackPolicy::Builtin),
                    cx.listener(|this, _, _, cx| {
                        this.set_theme_fallback(ThemeFallbackPolicy::Builtin, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "settings-theme-fallback-keep",
                    t!("theme_ext.fallback.keep_current").to_string(),
                    None,
                    fallback_active(ThemeFallbackPolicy::KeepCurrent),
                    cx.listener(|this, _, _, cx| {
                        this.set_theme_fallback(ThemeFallbackPolicy::KeepCurrent, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "settings-theme-fallback-system",
                    t!("theme_ext.fallback.system").to_string(),
                    None,
                    fallback_active(ThemeFallbackPolicy::System),
                    cx.listener(|this, _, _, cx| {
                        this.set_theme_fallback(ThemeFallbackPolicy::System, cx)
                    }),
                    p,
                ),
            ],
            p,
        );
        let fallback_card = settings_card(
            t!("theme_ext.fallback_policy").to_string(),
            Some(t!("theme_ext.fallback_policy_hint").to_string()),
            fallback_track,
            p,
        );

        let actions = div()
            .h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                Button::new("settings-theme-import-zed")
                    .ghost()
                    .compact()
                    .icon(IconName::FolderInput)
                    .label(t!("theme_ext.import_zed").to_string())
                    .on_click(cx.listener(|this, _, window, cx| this.import_zed_theme(window, cx))),
            )
            .child(
                Button::new("settings-theme-open-dir")
                    .ghost()
                    .compact()
                    .icon(IconName::FolderOpen)
                    .label(t!("theme_ext.open_dir").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.open_themes_dir(cx))),
            )
            .child(
                Button::new("settings-theme-rescan")
                    .ghost()
                    .compact()
                    .icon(IconName::RefreshCw)
                    .label(t!("theme_ext.rescan").to_string())
                    .on_click(cx.listener(|this, _, window, cx| this.rescan_themes(window, cx))),
            )
            .child(
                Button::new("settings-theme-example")
                    .ghost()
                    .compact()
                    .icon(IconName::FilePlus)
                    .label(t!("theme_ext.example").to_string())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.create_example_theme(window, cx)),
                    ),
            )
            .into_any_element();
        let actions_card = settings_card(
            t!("theme_ext.tools").to_string(),
            Some(
                t!(
                    "theme_ext.tools_hint",
                    path = themes_dir().display().to_string()
                )
                .to_string(),
            ),
            actions,
            p,
        );

        vec![active_card, policy_card, fallback_card, actions_card]
    }
}

pub(super) fn preference_for_active(
    active: &ActiveTheme,
    themes: &[ExtensionTheme],
) -> ThemePreference {
    match active {
        ActiveTheme::System => ThemePreference::System,
        ActiveTheme::BuiltinLight => ThemePreference::Light,
        ActiveTheme::BuiltinDark => ThemePreference::Dark,
        ActiveTheme::Extension(id) => {
            let dark = themes
                .iter()
                .find(|theme| &theme.id == id)
                .is_some_and(|theme| theme.mode == "dark");
            if dark {
                ThemePreference::Dark
            } else {
                ThemePreference::Light
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // 不用 `use super::*`：`gpui_kit::*` 会重导出 `gpui::test`，glob 导入会让
    // 标准 `#[test]` 属性被同名宏遮蔽，展开时无限递归。
    use super::{THEME_ROW_HEIGHT, theme_options_scroll_area};
    use gpui::{ScrollDelta, ScrollWheelEvent};
    use gpui_kit::base::StyledExt as _;
    use gpui_kit::{
        Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle,
        StatefulInteractiveElement as _, Styled as _, Window, div, point, px,
    };

    /// 「设置 → 外观」的缩略布局：外层页面可滚动，内层主题列表主题数远超视口。
    struct NestedSettings {
        page_scroll: ScrollHandle,
        theme_scroll: ScrollHandle,
    }

    impl Render for NestedSettings {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let rows = (0..20)
                .map(|index| {
                    div()
                        .id(format!("test-theme-row-{index}"))
                        .h(px(THEME_ROW_HEIGHT))
                        .w_full()
                        .child(format!("主题 {index}"))
                        .into_any_element()
                })
                .collect::<Vec<_>>();
            div()
                .id("test-settings-page")
                .w(px(480.))
                .h(px(360.))
                .v_flex()
                .track_scroll(&self.page_scroll)
                .overflow_y_scroll()
                .child(
                    div()
                        .v_flex()
                        .w_full()
                        .gap_2()
                        .flex_none()
                        .h_auto()
                        .child(div().h(px(40.)).child("页面标题"))
                        .child(theme_options_scroll_area(rows, &self.theme_scroll))
                        .child(div().h(px(400.)).child("后续卡片")),
                )
        }
    }

    /// 回归：滚轮悬停主题列表时先翻主题，外观页保持不动；主题到底后才冒泡。
    #[gpui::test]
    fn wheel_over_theme_list_scrolls_themes_before_the_page(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| gpui_kit::init(cx));
        let theme_scroll = ScrollHandle::new();
        let page_scroll = ScrollHandle::new();
        let inner = theme_scroll.clone();
        let outer = page_scroll.clone();
        let (_, cx) = cx.add_window_view(|_, _| NestedSettings {
            page_scroll,
            theme_scroll,
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(inner.offset().y, px(0.));
        assert_eq!(outer.offset().y, px(0.));

        // 位置落在主题列表内（页面标题 40px 之下、列表高 238px）。
        let wheel = || ScrollWheelEvent {
            position: point(px(60.), px(120.)),
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        };
        cx.simulate_event(wheel());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            inner.offset().y < px(0.),
            "滚轮应滚动主题列表，实际 offset.y = {:?}",
            inner.offset().y
        );
        assert_eq!(outer.offset().y, px(0.), "主题列表还能滚动时不应翻动外观页");

        // 一直滚到列表底部，之后的滚轮才交给外观页（overscroll chaining）。
        for _ in 0..40 {
            cx.simulate_event(wheel());
            cx.update(|window, cx| {
                let _ = window.draw(cx);
            });
        }
        assert!(
            outer.offset().y < px(0.),
            "主题列表到底后滚轮应冒泡给外观页"
        );
    }

    /// 回归：标题栏主题菜单用 `max_h` 限高时，固定行高的子项不能被 flex 压缩，
    /// 否则两行文字会叠在一起，列表也因没有滚动余量而无法翻页。
    #[gpui::test]
    fn max_h_theme_menu_rows_stay_scrollable(cx: &mut gpui_kit::TestAppContext) {
        struct MenuList {
            scroll: ScrollHandle,
        }

        impl Render for MenuList {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let mut list = div()
                    .id("test-theme-menu-list")
                    .v_flex()
                    .w(px(200.))
                    .gap_0p5()
                    .pr_3()
                    .max_h(px(320.))
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll();
                for index in 0..20 {
                    list = list.child(
                        div()
                            .h(px(34.))
                            .flex_shrink_0()
                            .w_full()
                            .child(format!("主题 {index}"))
                            .into_any_element(),
                    );
                }
                div().relative().w_full().child(list)
            }
        }

        cx.update(|cx| gpui_kit::init(cx));
        let scroll = ScrollHandle::new();
        let handle = scroll.clone();
        let (_, cx) = cx.add_window_view(|_, _| MenuList { scroll });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(60.), px(100.)),
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert!(
            handle.offset().y < px(0.),
            "max_h 下列表须保留滚动余量，实际 offset.y = {:?}",
            handle.offset().y
        );
    }
}
