//! Local crash records.
//!
//! When the process panics we write a small JSON file under
//! `%LOCALAPPDATA%/md-work/crashes/` so a user can attach it to a bug report.
//! Nothing is uploaded anywhere; note bodies are never included.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

const KEEP: usize = 20;

pub fn install_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = panic_message(info);
        let location = info
            .location()
            .map(|location| format!("{}:{}:{}", location.file(), location.line(), location.column()));
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        if let Err(err) = record(
            &crate::paths::crashes_dir(),
            &message,
            location.as_deref(),
            &thread,
            &backtrace,
        ) {
            eprintln!("md-work: failed to write crash record: {err}");
        }
        previous(info);
    }));
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Writes one crash record and prunes older ones. Exposed for tests.
pub fn record(
    dir: &Path,
    message: &str,
    location: Option<&str>,
    thread: &str,
    backtrace: &str,
) -> crate::Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let millis = now.as_millis();

    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("crash-{millis}-{:09}.json", now.subsec_nanos()));
    let record = json!({
        "format": "md-work/crash@1",
        "timestamp_ms": millis,
        "version": crate::version(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "thread": thread,
        "message": message,
        "location": location,
        "backtrace": backtrace,
    });
    let mut file = std::fs::File::create(&path)?;
    let bytes = serde_json::to_vec_pretty(&record).map_err(|source| crate::Error::Json {
        path: path.clone(),
        source,
    })?;
    file.write_all(&bytes)?;
    file.flush()?;
    prune(dir);
    Ok(path)
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(u128, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let stem = name.strip_prefix("crash-")?.strip_suffix(".json")?;
            let (millis, nanos) = stem.split_once('-')?;
            Some((millis.parse::<u128>().ok()? * 1_000_000_000 + nanos.parse::<u128>().ok()?, path))
        })
        .collect();
    if files.len() <= KEEP {
        return;
    }
    files.sort_by_key(|(stamp, _)| *stamp);
    let excess = files.len() - KEEP;
    for (_, path) in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("mdwork-crash-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn writes_and_prunes_records() {
        let dir = scratch();
        let path = record(&dir, "boom", Some("src/main.rs:1:2"), "main", "stack").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"message\": \"boom\""));
        assert!(text.contains("md-work/crash@1"));

        for _ in 0..KEEP + 5 {
            record(&dir, "again", None, "worker", "").unwrap();
        }
        let count = std::fs::read_dir(&dir).unwrap().count();
        assert!(count <= KEEP, "expected pruning, found {count} files");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
