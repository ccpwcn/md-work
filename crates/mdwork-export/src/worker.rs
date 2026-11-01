//! Runs exports in a `md-work-cli` subprocess and exposes JSON progress.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::exporter::{ExportEvent, ExportRequest};

pub struct ExportJob {
    events: Arc<Mutex<Vec<ExportEvent>>>,
    child: Arc<Mutex<Option<Child>>>,
    finished: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    /// Holds `request.json` and `stderr.log` (outside the output volume).
    control_dir: PathBuf,
    /// Staging directory next to the output so the final rename stays atomic.
    payload_dir: PathBuf,
}

impl ExportJob {
    pub fn start(cli: &Path, request: &ExportRequest) -> Result<Self> {
        let control_dir = request.temp_dir.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("md-work-export-{}", unique_suffix()))
        });
        fs::create_dir_all(&control_dir)
            .with_context(|| format!("无法创建临时目录 {}", control_dir.display()))?;
        let output_parent = request
            .output
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let payload_dir = output_parent.join(format!(".mdwork-export-{}", unique_suffix()));
        // The exporter must stage next to the output, and the job owns that
        // directory so a killed child cannot leave it behind.
        let mut request = request.clone();
        request.temp_dir = Some(payload_dir.clone());
        let request_path = control_dir.join("request.json");
        fs::write(&request_path, serde_json::to_vec_pretty(&request)?)?;

        let mut command = Command::new(cli);
        command
            .arg("export")
            .arg("--request")
            .arg(&request_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(File::create(control_dir.join("stderr.log"))?));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = fs::remove_dir_all(&control_dir);
                return Err(err).with_context(|| format!("无法启动 {}", cli.display()));
            }
        };
        let stdout = child.stdout.take();

        let events = Arc::new(Mutex::new(Vec::new()));
        let child_slot = Arc::new(Mutex::new(Some(child)));
        let finished = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));

        let thread_events = events.clone();
        let thread_child = child_slot.clone();
        let thread_finished = finished.clone();
        let thread_cancelled = cancelled.clone();
        let thread_control = control_dir.clone();
        let thread_payload = payload_dir.clone();
        std::thread::spawn(move || {
            let mut saw_done = false;
            let mut saw_error = false;
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(event) = serde_json::from_str::<ExportEvent>(trimmed) {
                        match &event {
                            ExportEvent::Done { .. } => saw_done = true,
                            ExportEvent::Error { .. } => saw_error = true,
                            _ => {}
                        }
                        if let Ok(mut queue) = thread_events.lock() {
                            queue.push(event);
                        }
                    }
                }
            }
            let status = {
                let mut slot = thread_child.lock().expect("export child lock");
                match slot.take() {
                    Some(mut child) => child.wait().ok(),
                    None => None,
                }
            };
            if !thread_cancelled.load(Ordering::SeqCst) {
                let stderr =
                    fs::read_to_string(thread_control.join("stderr.log")).unwrap_or_default();
                // Prefer the JSON Error event from the child. Only scrape stderr when
                // the process died without reporting; never surface raw backtrace frames
                // like "22: RtlUserThreadStart" to the UI.
                if !saw_done && !saw_error {
                    let message = friendly_export_error(&stderr, status);
                    if let Ok(mut queue) = thread_events.lock() {
                        queue.push(ExportEvent::Error { message });
                    }
                } else if !stderr.trim().is_empty() {
                    eprintln!("md-work: export stderr: {stderr}");
                }
            }
            let _ = fs::remove_dir_all(&thread_payload);
            let _ = fs::remove_dir_all(&thread_control);
            thread_finished.store(true, Ordering::SeqCst);
        });

        Ok(Self {
            events,
            child: child_slot,
            finished,
            cancelled,
            control_dir,
            payload_dir,
        })
    }

    pub fn drain(&self) -> Vec<ExportEvent> {
        let mut queue = self.events.lock().expect("export events lock");
        std::mem::take(&mut *queue)
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn temp_dir(&self) -> &Path {
        &self.payload_dir
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let mut slot = self.child.lock().expect("export child lock");
        if let Some(mut child) = slot.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.payload_dir);
        let _ = fs::remove_dir_all(&self.control_dir);
    }
}

fn unique_suffix() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    )
}

fn friendly_export_error(stderr: &str, status: Option<std::process::ExitStatus>) -> String {
    if let Some(message) = panic_message_from_stderr(stderr) {
        return format!("导出失败：{message}");
    }
    if let Some(message) = anyhow_message_from_stderr(stderr) {
        return message;
    }
    if stderr_looks_like_backtrace(stderr) {
        return "导出进程异常退出。请重试；若持续失败，可先改用 PNG。".to_string();
    }
    let fallback = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && !is_backtrace_frame(line));
    match fallback {
        Some(line) => line.to_string(),
        None => match status {
            Some(status) if !status.success() => {
                format!("导出失败（进程退出码 {}）", status_code(status))
            }
            Some(_) | None => "导出进程意外结束".to_string(),
        },
    }
}

fn panic_message_from_stderr(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let Some(rest) = trimmed
            .strip_prefix("thread '")
            .and_then(|rest| rest.split_once("' panicked at "))
            .map(|(_, rest)| rest)
        else {
            continue;
        };
        // Newer rustc:
        //   thread 'main' panicked at src\foo.rs:12:5:
        //   actual message
        // Older rustc:
        //   thread 'main' panicked at 'message', src\foo.rs:12:5
        let message = if looks_like_rust_location(rest.trim_end_matches(':').trim()) {
            lines
                .get(index + 1)
                .map(|next| next.trim())
                .filter(|next| !next.is_empty() && !is_backtrace_frame(next))
                .unwrap_or("")
        } else if let Some((message, location)) = rest.rsplit_once(", ") {
            if looks_like_rust_location(location) {
                message
            } else {
                rest
            }
        } else {
            rest
        };
        let message = message.trim().trim_matches('"');
        if !message.is_empty() && !looks_like_rust_location(message) {
            return Some(message.to_string());
        }
    }
    None
}

fn anyhow_message_from_stderr(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        let trimmed = line.trim();
        if let Some(message) = trimmed.strip_prefix("Error:") {
            let message = message.trim();
            if !message.is_empty() {
                return Some(message.to_string());
            }
        }
    }
    None
}

fn stderr_looks_like_backtrace(stderr: &str) -> bool {
    stderr.lines().any(is_backtrace_frame)
        || stderr.contains("stack backtrace")
        || stderr.contains("Stack backtrace")
}

fn is_backtrace_frame(line: &str) -> bool {
    let trimmed = line.trim();
    // "  22: RtlUserThreadStart" / "at /rustc/..." / "stack backtrace:"
    if trimmed.eq_ignore_ascii_case("stack backtrace:") {
        return true;
    }
    let mut parts = trimmed.split_whitespace();
    let Some(index) = parts.next() else {
        return false;
    };
    let Some(rest) = index.strip_suffix(':') else {
        return false;
    };
    rest.chars().all(|ch| ch.is_ascii_digit()) && parts.next().is_some()
}

fn looks_like_rust_location(text: &str) -> bool {
    text.contains(".rs:") || text.contains('\\') || text.contains('/')
}

fn status_code(status: std::process::ExitStatus) -> String {
    status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| status.to_string())
}

impl Drop for ExportJob {
    fn drop(&mut self) {
        if self
            .child
            .lock()
            .map(|slot| slot.is_some())
            .unwrap_or(false)
        {
            self.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrapes_panic_message_instead_of_backtrace_tail() {
        let stderr = r#"
thread 'main' panicked at src\png.rs:12:5:
WebP boom
stack backtrace:
  21: BaseThreadInitThunk
  22: RtlUserThreadStart
"#;
        let message = friendly_export_error(stderr, None);
        assert!(
            message.contains("WebP boom"),
            "unexpected message: {message}"
        );
        assert!(!message.contains("RtlUserThreadStart"));
    }

    #[test]
    fn prefers_anyhow_error_line() {
        let stderr = "Error: unknown variant `webp`, expected `pdf` or `png`\n";
        assert_eq!(
            friendly_export_error(stderr, None),
            "unknown variant `webp`, expected `pdf` or `png`"
        );
    }

    #[test]
    fn backtrace_only_stderr_gets_friendly_fallback() {
        let stderr = "stack backtrace:\n  22: RtlUserThreadStart\n";
        let message = friendly_export_error(stderr, None);
        assert!(message.contains("导出进程异常退出"));
        assert!(!message.contains("RtlUserThreadStart"));
    }
}
