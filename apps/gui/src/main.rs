mod app;
mod dirty;
mod doc;
mod file_assoc;
mod mcp_server;
mod preview;
mod single_instance;
mod theme;

use std::sync::{Arc, Mutex};

use gpui_kit::*;

rust_i18n::i18n!("locales", fallback = "en");

gpui_kit::actions!(mdwork, [SaveDoc, TogglePreview]);

fn main() {
    let initial_path = std::env::args().skip(1).find(|arg| !arg.starts_with('-'));

    if let Some(path) = &initial_path {
        if single_instance::try_forward(path) {
            return;
        }
    }

    if let Err(err) = file_assoc::ensure_registered() {
        eprintln!("md-work: file association not registered: {err}");
    }

    let rx = single_instance::spawn_server();
    let pending: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let pending = pending.clone();
        std::thread::spawn(move || {
            while let Ok(path) = rx.recv() {
                pending.lock().expect("pending paths lock").push(path);
            }
        });
    }

    let bounds = Bounds {
        origin: point(px(90.), px(70.)),
        size: size(px(1360.), px(900.)),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(900.), px(600.))),
        app_id: Some("md-work".to_string()),
        ..gpui_kit::component::TitleBar::window_options()
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-s", SaveDoc, None),
            KeyBinding::new("ctrl-t", TogglePreview, None),
        ]);

        gpui_kit::open_window(options, cx, move |window, cx| {
            let pending = pending.clone();
            let shell = cx.new(|cx| {
                app::AppShell::new(window, cx, pending, initial_path.clone())
            });
            cx.new(|cx| gpui_kit::component::Root::new(shell, window, cx))
        })
        .expect("failed to open md-work window");
    });
}
