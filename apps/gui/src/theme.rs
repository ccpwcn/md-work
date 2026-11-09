use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use mdwork_core::{AppConfig, ThemePreference};

pub fn apply(config: &AppConfig, window: &mut Window, cx: &mut App) {
    let mode = match config.theme {
        ThemePreference::Light => ThemeMode::Light,
        ThemePreference::Dark => ThemeMode::Dark,
        ThemePreference::System => match window.appearance() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
            _ => ThemeMode::Light,
        },
    };
    Theme::change(mode, Some(window), cx);

    let ui_font = config.ui_font.trim().to_string();
    let ui_size = config.ui_font_size.clamp(10.0, 32.0);
    let note_font = config.note_font.trim().to_string();
    let note_size = config.note_font_size.clamp(10.0, 32.0);
    Theme::update(cx, move |theme| {
        if !ui_font.is_empty() {
            theme.font_family = SharedString::from(ui_font);
        }
        theme.font_size = px(ui_size);
        if !note_font.is_empty() {
            theme.mono_font_family = SharedString::from(note_font);
        }
        theme.mono_font_size = px(note_size);
    });
}

#[cfg(test)]
mod tests {
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
