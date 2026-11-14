use std::rc::Rc;

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::*;
use mdwork_core::theme_ext::ExtensionTheme;
use mdwork_core::{AppConfig, ThemePreference};

/// Applies the current appearance to GPUI.
///
/// The two built-in themes always stay installed as the light/dark defaults;
/// an extension theme replaces only the slot matching its own mode. That way
/// switching back to a built-in (or to "follow system") restores the original
/// colours and syntax highlighting instead of leaving a custom theme behind.
pub fn apply(
    config: &AppConfig,
    extension: Option<&ExtensionTheme>,
    window: Option<&mut Window>,
    cx: &mut App,
) {
    let appearance = window
        .as_ref()
        .map(|window| window.appearance())
        .unwrap_or_else(|| cx.window_appearance());
    let preferred_mode = match config.theme {
        ThemePreference::Light => ThemeMode::Light,
        ThemePreference::Dark => ThemeMode::Dark,
        ThemePreference::System => ThemeMode::from(appearance),
    };

    let custom: Option<Rc<ThemeConfig>> = extension.and_then(|ext| {
        serde_json::from_value::<ThemeConfig>(ext.config.clone())
            .map(Rc::new)
            .map_err(|err| {
                tracing::error!(
                    theme = %ext.name,
                    error = %err,
                    "扩展主题与当前组件库主题格式不兼容"
                );
                err
            })
            .ok()
    });
    let mode = custom
        .as_ref()
        .map(|theme| theme.mode)
        .unwrap_or(preferred_mode);

    let (default_light, default_dark) = {
        let registry = ThemeRegistry::global(cx);
        (
            registry.default_light_theme().clone(),
            registry.default_dark_theme().clone(),
        )
    };
    let (light, dark) = match &custom {
        Some(theme) if theme.mode.is_dark() => (default_light, Rc::clone(theme)),
        Some(theme) => (Rc::clone(theme), default_dark),
        None => (default_light, default_dark),
    };

    // Install the configs first, then `change` reloads the active mode from the
    // installed slot. `Theme::change` is the documented reload path and also
    // projects the palette onto the Base layer and refreshes every window.
    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
    });
    Theme::change(mode, window, cx);

    let missing_highlight = custom
        .as_ref()
        .is_some_and(|theme| theme.highlight.is_none());
    if missing_highlight {
        let syntax = if mode.is_dark() {
            HighlightTheme::default_dark()
        } else {
            HighlightTheme::default_light()
        };
        Theme::update(cx, |theme| theme.highlight_theme = syntax);
    }

    let theme_ui_font = extension
        .and_then(|ext| ext.typography.ui_font.clone())
        .unwrap_or_default();
    let theme_note_font = extension
        .and_then(|ext| ext.typography.note_font.clone())
        .unwrap_or_default();
    let ui_font = config.ui_font.trim().to_string();
    let ui_size = config.ui_font_size.clamp(10.0, 32.0);
    let note_font = config.note_font.trim().to_string();
    let note_size = config.note_font_size.clamp(10.0, 32.0);
    Theme::update(cx, move |theme| {
        if !ui_font.is_empty() {
            theme.font_family = SharedString::from(ui_font);
        } else if !theme_ui_font.is_empty() {
            theme.font_family = SharedString::from(theme_ui_font);
        }
        theme.font_size = px(ui_size);
        if !note_font.is_empty() {
            theme.mono_font_family = SharedString::from(note_font);
        } else if !theme_note_font.is_empty() {
            theme.mono_font_family = SharedString::from(theme_note_font);
        }
        theme.mono_font_size = px(note_size);
    });
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui_kit::component::{Theme, ThemeConfig, ThemeMode};
    use mdwork_core::theme_ext::{parse_theme_file, parse_theme_toml, zed_json_to_theme_document};
    use mdwork_core::{AppConfig, ThemePreference};

    /// 扩展主题生成的 JSON 必须能被组件库的公开主题结构直接反序列化，
    /// 否则主题文件即使解析成功也无法真正应用到界面上。
    #[test]
    fn extension_theme_json_matches_component_theme_schema() {
        let native = r##"
[theme]
name = "Native Dark"
mode = "dark"
[colors]
background = "#101010"
foreground = "#eeeeee"
"accent.background" = "#202020"
[highlight]
"editor.background" = "#101010"
[highlight.syntax]
keyword = "#ff00ff"
"##;
        let theme = parse_theme_toml(Path::new("native.toml"), native).unwrap();
        let config: ThemeConfig = serde_json::from_value(theme.config).unwrap();
        assert_eq!(config.mode, ThemeMode::Dark);
        assert_eq!(config.colors.background.as_deref(), Some("#101010"));
        assert!(config.highlight.is_some());

        let zed = r##"{
            "name": "Zed Dark",
            "themes": [{
                "name": "Zed Dark",
                "appearance": "dark",
                "style": {
                    "editor.background": "#1e1e2e",
                    "editor.foreground": "#cdd6f4",
                    "border": "#313244",
                    "error": "#ff0000",
                    "syntax": {
                        "keyword": "#cba6f7",
                        "comment": { "color": "#6c7086", "font_style": "italic" }
                    }
                }
            }]
        }"##;
        let doc = zed_json_to_theme_document(zed, "zed").unwrap();
        let converted = parse_theme_file(Path::new("zed.toml"), &doc.text)
            .unwrap()
            .remove(0);
        let config: ThemeConfig = serde_json::from_value(converted.config).unwrap();
        assert_eq!(config.mode, ThemeMode::Dark);
        assert_eq!(config.colors.background.as_deref(), Some("#1e1e2e"));
        assert!(config.highlight.is_some());
    }

    /// 扩展主题真正落到全局 Theme 上，切回内置主题后能完整恢复。
    #[gpui::test]
    fn extension_theme_applies_and_builtin_is_restored(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| gpui_kit::init(cx));
        let mut config = AppConfig::default();
        config.theme = ThemePreference::Light;
        let text = r##"
[theme]
name = "Runtime Dark"
mode = "dark"
[colors]
background = "#102030"
foreground = "#e0e0e0"
"##;
        let extension = parse_theme_toml(Path::new("runtime-dark.toml"), text).unwrap();

        cx.update(|cx| {
            crate::theme::apply(&config, Some(&extension), None, cx);
            let theme = Theme::global(cx);
            assert_eq!(theme.mode, ThemeMode::Dark);
            assert_eq!(theme.theme_name().to_string(), "Runtime Dark");
            assert_eq!(
                theme.background,
                gpui_kit::component::try_parse_color("#102030").unwrap()
            );
        });

        cx.update(|cx| {
            crate::theme::apply(&config, None, None, cx);
            let theme = Theme::global(cx);
            assert_eq!(theme.mode, ThemeMode::Light);
            assert_eq!(theme.theme_name().to_string(), "Default Light");
            assert_ne!(
                theme.background,
                gpui_kit::component::try_parse_color("#102030").unwrap()
            );
        });
    }

    /// 编辑区是纯文本源码视图。内置语法主题不能给源码加斜体，否则输入
    /// `_` / `*` 等强调标记时，tree-sitter 的 emphasis 节点会随输入反复
    /// 出现和消失，整段文字在斜体与非斜体之间来回跳。
    const DEFAULT_THEME: &str =
        include_str!("../../../vendor/gpui-component/src/theme/default-theme.json");

    #[test]
    fn syntax_highlighting_never_italicizes_source_text() {
        let json: serde_json::Value = serde_json::from_str(DEFAULT_THEME).unwrap();
        let themes = json["themes"].as_array().expect("themes array");
        assert!(themes.len() >= 2);
        for theme in themes {
            let syntax = &theme["highlight"]["syntax"];
            for name in ["emphasis", "emphasis.strong", "link_text", "link_uri"] {
                let font_style = syntax[name]["font_style"].as_str();
                assert_ne!(
                    font_style,
                    Some("italic"),
                    "theme {} capture {} must not italicize editor text",
                    theme["name"],
                    name
                );
            }
        }
    }
}
