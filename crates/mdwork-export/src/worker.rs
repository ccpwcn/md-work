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
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(event) = serde_json::from_str::<ExportEvent>(trimmed) {
                        if matches!(event, ExportEvent::Done { .. }) {
                            saw_done = true;
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
                if !saw_done {
                    let message = stderr
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| match status {
                            Some(status) => format!("导出失败（{}）", status),
                            None => "导出进程意外结束".to_string(),
                        });
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
