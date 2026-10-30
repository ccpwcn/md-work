//! File logging and crash reporting for md-work.
//!
//! Writes daily-rotated UTF-8 logs into the app data directory and forwards
//! panics to the log before the default panic hook prints them. Console output
//! is opt-in because the GUI has no attached console and the MCP stdio
//! transport must keep stdout clean.
//!
//! Filtering follows `MDWORK_LOG` (tracing `EnvFilter` syntax, e.g.
//! `MDWORK_LOG=debug` or `MDWORK_LOG=mdwork_core=trace`). Without it the
//! level defaults to `info`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::LocalTime;
use tracing_subscriber::prelude::*;

/// File prefix; the daily file is named `md-work.log.<YYYY-MM-DD>`.
pub const FILE_PREFIX: &str = "md-work";
pub const DEFAULT_RETENTION: usize = 14;
pub const ENV_FILTER: &str = "MDWORK_LOG";

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone)]
pub struct Options {
    pub dir: PathBuf,
    /// Mirror records to stderr. The GUI leaves this off.
    pub console: bool,
    /// How many daily files to keep; older files are deleted at startup.
    pub retention: usize,
}

#[derive(Debug, Clone)]
pub struct LogInfo {
    pub dir: PathBuf,
    pub filter: String,
}

/// Installs the global subscriber and the panic hook. Safe to call twice; the
/// second call returns the already configured location.
pub fn init(options: Options) -> io::Result<LogInfo> {
    fs::create_dir_all(&options.dir)?;
    let _ = LOG_DIR.set(options.dir.clone());

    let filter_text = filter_text();
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return Ok(LogInfo {
            dir: options.dir,
            filter: filter_text,
        });
    }

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(FILE_PREFIX)
        .filename_suffix("log")
        .max_log_files(options.retention.max(1))
        .build(&options.dir)
        .map_err(io::Error::other)?;

    let filter = EnvFilter::try_new(&filter_text)
        .inspect_err(|err| eprintln!("md-work: ignoring {ENV_FILTER}: {err}"))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(appender)
        .with_ansi(false)
        .with_timer(LocalTime::rfc_3339())
        .with_thread_names(true);

    let console_layer = options.console.then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(io::stderr)
            .with_timer(LocalTime::rfc_3339())
            .with_thread_names(true)
    });

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .try_init()
        .map_err(io::Error::other)?;

    install_panic_hook();

    Ok(LogInfo {
        dir: options.dir,
        filter: filter_text,
    })
}

/// Logs the panic with its location, thread and a backtrace, then runs the
/// previous hook so the usual stderr message still appears.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "<unknown>".to_string());
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");
        tracing::error!(
            target: "mdwork::crash",
            location = %location,
            thread = %thread_name,
            "程序崩溃: {message}\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        previous(info);
    }));
}

/// Directory chosen by [`init`], if it ran.
pub fn log_dir() -> Option<&'static Path> {
    LOG_DIR.get().map(PathBuf::as_path)
}

/// `MDWORK_LOG` when set, otherwise `info`.
pub fn filter_text() -> String {
    match std::env::var(ENV_FILTER) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => "info".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_records_to_daily_file() {
        let dir = std::env::temp_dir().join(format!("mdwork-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let info = init(Options {
            dir: dir.clone(),
            console: false,
            retention: 3,
        })
        .expect("logging initializes");
        assert_eq!(info.dir, dir);

        tracing::info!(probe = "smoke", "hello from the log test");

        let panicked = std::panic::catch_unwind(|| panic!("boom from the log test"));
        assert!(panicked.is_err());

        let file = fs::read_dir(&dir)
            .expect("log dir exists")
            .flatten()
            .map(|entry| entry.path())
            .max()
            .expect("a daily log file exists");
        let text = fs::read_to_string(&file).expect("log file is readable");
        assert!(text.contains("hello from the log test"), "{text}");
        assert!(text.contains("probe=\"smoke\""), "{text}");
        assert!(text.contains("程序崩溃: boom from the log test"), "{text}");
        assert!(text.contains("lib.rs"), "{text}");

        let _ = fs::remove_dir_all(&dir);
    }
}
