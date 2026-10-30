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
