use gpui_kit::{App, Bounds, DisplayId, Pixels, Window, WindowBounds, point, px, size};
use mdwork_core::config::{WindowMode, WindowPlacement};

/// Reads the restored rectangle and maximized state. Returns `None` while the
/// platform is reporting a transient frame (fullscreen, or a size we refuse
/// to persist).
pub(super) fn capture(window: &Window) -> Option<WindowPlacement> {
    let reported = window.window_bounds();
    let state = match reported {
        WindowBounds::Windowed(_) => WindowMode::Windowed,
        WindowBounds::Maximized(_) => WindowMode::Maximized,
        WindowBounds::Fullscreen(_) => return None,
    };
    let bounds = reported.get_bounds();
    WindowPlacement::new(
        state,
        bounds.origin.x.as_f32(),
        bounds.origin.y.as_f32(),
        bounds.size.width.as_f32(),
        bounds.size.height.as_f32(),
    )
}

/// Applies a saved placement. The display is the one that contains the
/// rectangle's center, so a window last seen on a secondary monitor is not
/// forced onto the primary display.
pub(crate) fn restore_window(
    options: &mut gpui_kit::WindowOptions,
    placement: WindowPlacement,
    cx: &App,
) {
    let bounds = Bounds {
        origin: point(px(placement.x), px(placement.y)),
        size: size(px(placement.width), px(placement.height)),
    };
    options.display_id = display_containing(bounds, cx);
    options.window_bounds = Some(match placement.state {
        WindowMode::Windowed => WindowBounds::Windowed(bounds),
        WindowMode::Maximized => WindowBounds::Maximized(bounds),
    });
}

fn display_containing(bounds: Bounds<Pixels>, cx: &App) -> Option<DisplayId> {
    let center = bounds.center();
    cx.displays()
        .into_iter()
        .find_map(|display| display.bounds().contains(&center).then(|| display.id()))
}
