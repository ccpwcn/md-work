//! AppShell integration for the pluggable theme system.
//!
//! Keeps `theme_extension.toml`, the scanned extension themes, the directory
//! watcher and the resulting notification messages together, and renders the
//! matching controls in 「设置 → 外观」.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mdwork_core::config::ThemePreference;
use mdwork_core::theme_ext::{
    ActiveTheme, ExtensionTheme, ThemeExtensionConfig, ThemeFailure, ThemeFallbackPolicy,
    ThemeLoadPolicy, ThemeWatcher, content_signature, example_theme_toml, import_zed_theme,
    main_config_path, scan_themes, themes_dir,
};
use rust_i18n::t;

use super::util::*;
use super::*;

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

        for theme in &themes {
            let signature = content_signature(&theme.path);
            let changed = self.signatures.get(&theme.path) != Some(&signature);
            if initial || changed {
                self.push(
                    NoticeKind::Success,
                    t!("theme_ext.found_ok", name = theme.name.clone()).to_string(),
                );
            }
            signatures.insert(theme.path.clone(), signature);
            names.insert(theme.path.clone(), theme.name.clone());
            if self.config.active == ActiveTheme::Extension(theme.id.clone()) {
                self.last_good = Some(theme.clone());
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
                Ok(files) => {
                    this.notify_success(
                        t!("theme_ext.import_ok", count = files.len()).to_string(),
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

    /// Cards shown on the appearance settings page. Each one maps to a key in
    /// `theme_extension.toml` (plus a few one-shot developer actions).
    pub(super) fn render_theme_extension_cards(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = SettingsPalette::new(cx);
        let active = &self.theme_ext.config.active;
        let options: Vec<(String, String, Option<IconName>, ActiveTheme, bool)> = {
            let mut list = vec![
                (
                    t!("theme.system").to_string(),
                    String::new(),
                    Some(IconName::Monitor),
                    ActiveTheme::System,
                    *active == ActiveTheme::System,
                ),
                (
                    t!("theme_ext.builtin_light").to_string(),
                    String::new(),
                    Some(IconName::Sun),
                    ActiveTheme::BuiltinLight,
                    *active == ActiveTheme::BuiltinLight,
                ),
                (
                    t!("theme_ext.builtin_dark").to_string(),
                    String::new(),
                    Some(IconName::Moon),
                    ActiveTheme::BuiltinDark,
                    *active == ActiveTheme::BuiltinDark,
                ),
            ];
            for theme in &self.theme_ext.themes {
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
                list.push((
                    theme.name.clone(),
                    detail,
                    Some(IconName::Palette),
                    target.clone(),
                    *active == target,
                ));
            }
            list
        };

        let mut list = div()
            .id("settings-theme-options")
            .v_flex()
            .w_full()
            .gap_0p5()
            .max_h(px(210.))
            .overflow_y_scroll();
        for (index, (label, detail, icon, target, is_active)) in options.into_iter().enumerate() {
            let target_for_click = target.clone();
            list = list.child(
                div()
                    .id(format!("settings-theme-option-{index}"))
                    .h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if is_active { p.popover } else { p.transparent })
                    .hover(move |style| style.bg(p.hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_active_theme(target_for_click.clone(), window, cx);
                    }))
                    .when_some(icon, |row, icon| {
                        row.child(Icon::new(icon).xsmall().text_color(if is_active {
                            p.accent
                        } else {
                            p.muted
                        }))
                    })
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_xs()
                                    .when(is_active, |text| text.font_bold())
                                    .child(label),
                            )
                            .when(!detail.is_empty(), |column| {
                                column.child(div().text_xs().text_color(p.muted).child(detail))
                            }),
                    )
                    .when(is_active, |row| {
                        row.child(Icon::new(IconName::Check).xsmall().text_color(p.success))
                    }),
            );
        }

        let mut status = div().v_flex().w_full().gap_1().child(list);
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
