// Debug keeps a console for logs/panics; release is a GUI-only process.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod dirty;
mod disk;
mod doc;
mod file_assoc;
mod highlight;
mod install_hint;
mod mcp_server;
mod preview;
mod print;
mod single_instance;
mod theme;

use std::sync::{Arc, Mutex};

use gpui_kit::*;

rust_i18n::i18n!("locales", fallback = "en");

gpui_kit::actions!(
    mdwork,
    [
        SaveDoc,
        TogglePreview,
        OpenPalette,
        NewNote,
        ToggleBold,
        ToggleItalic,
        InsertLink,
        PastePlain
    ]
);

fn main() {
    let logging = mdwork_log::init(mdwork_log::Options {
        dir: mdwork_core::paths::logs_dir(),
        console: std::env::var_os("MDWORK_LOG_CONSOLE").is_some(),
        retention: mdwork_log::DEFAULT_RETENTION,
    });
    match &logging {
        Ok(info) => tracing::info!(
            version = mdwork_core::version(),
            pid = std::process::id(),
            log_dir = %info.dir.display(),
            filter = %info.filter,
            args = ?std::env::args().skip(1).collect::<Vec<_>>(),
            "程序启动"
        ),
        Err(err) => eprintln!("md-work: 日志不可用：{err}"),
    }
    mdwork_core::crash::install_hook();
    #[cfg(debug_assertions)]
    if std::env::var_os("MDWORK_TEST_PANIC").is_some() {
        panic!("panic requested by MDWORK_TEST_PANIC");
    }

    let initial_path = std::env::args().skip(1).find(|arg| !arg.starts_with('-'));

    if let Some(path) = &initial_path {
        if single_instance::try_forward(path) {
            tracing::info!(path = %path, "已转发到正在运行的实例，本进程退出");
            return;
        }
    }

    if let Err(err) = file_assoc::ensure_registered() {
        tracing::warn!(error = %err, "文件关联注册失败");
    }

    // Read settings, the workspace list and the saved session before the event
    // loop starts; the workspace itself is scanned off the UI thread.
    let startup = app::Startup::load();
    startup.report_crashes();

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
                KeyBinding::new("ctrl-shift-p", OpenPalette, None),
                KeyBinding::new("ctrl-n", NewNote, None),
                KeyBinding::new("ctrl-b", ToggleBold, None),
                KeyBinding::new("ctrl-i", ToggleItalic, None),
                KeyBinding::new("ctrl-k", InsertLink, None),
                KeyBinding::new("ctrl-shift-v", PastePlain, None),
            ]);

            let mut startup = Some(startup);
            gpui_kit::open_window(options, cx, move |window, cx| {
                let pending = pending.clone();
                let startup = startup.take().unwrap_or_else(app::Startup::load);
                let shell = cx.new(|cx| {
                    app::AppShell::new(window, cx, pending, initial_path.clone(), startup)
                });
                cx.new(|cx| gpui_kit::component::Root::new(shell, window, cx))
            })
            .expect("failed to open md-work window");
        });

    disk::flush(std::time::Duration::from_secs(3));
    tracing::info!("程序退出");
}
