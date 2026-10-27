//! Append-only MCP audit log: one JSON object per tool call, monthly files,
//! 90-day retention. Note bodies and secrets are never recorded.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context as _, Result};
use mdwork_core::paths::app_data_dir;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::now_unix;

pub const RETENTION_DAYS: u64 = 90;
const SECS_PER_DAY: u64 = 86_400;
const PRUNE_INTERVAL_SECS: u64 = 3_600;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditEntry {
    /// Unix seconds, UTC.
    pub time: u64,
    pub request_id: String,
    pub token_id: Option<Uuid>,
    pub token_name: String,
    pub tool: String,
    pub notebook: Option<Uuid>,
    pub notebook_name: Option<String>,
    /// Notebook-relative note path; never file contents.
    pub path: Option<String>,
    /// Search text for `search_notes`; never file contents.
    pub query: Option<String>,
    /// Payload size in bytes when the call read or wrote note content.
    pub bytes: Option<u64>,
    /// `ok` or `error:<CODE>`.
    pub result: String,
    pub duration_ms: u64,
    pub client: Option<String>,
}

impl Default for AuditEntry {
    fn default() -> Self {
        Self {
            time: 0,
            request_id: String::new(),
            token_id: None,
            token_name: String::new(),
            tool: String::new(),
            notebook: None,
            notebook_name: None,
            path: None,
            query: None,
            bytes: None,
            result: "ok".to_string(),
            duration_ms: 0,
            client: None,
        }
    }
}

impl AuditEntry {
    pub fn new(tool: impl Into<String>, token_name: impl Into<String>) -> Self {
        Self {
            time: now_unix(),
            tool: tool.into(),
            token_name: token_name.into(),
            ..Self::default()
        }
    }

    pub fn error(mut self, code: &str) -> Self {
        self.result = format!("error:{code}");
        self
    }
}

pub struct AuditLog {
    dir: PathBuf,
    last_prune: Mutex<u64>,
}

impl AuditLog {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            last_prune: Mutex::new(0),
        }
    }

    pub fn default_dir() -> PathBuf {
        app_data_dir().join("mcp").join("audit")
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn append(&self, entry: &AuditEntry) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("创建审计目录 {}", self.dir.display()))?;
        let path = self.dir.join(format!("{}.jsonl", month_key(entry.time)));
        let mut line = serde_json::to_string(entry)?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("打开审计日志 {}", path.display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("写入审计日志 {}", path.display()))?;
        self.prune_due();
        Ok(())
    }

    /// Newest entries first, across month files.
    pub fn recent(&self, limit: usize) -> Result<Vec<AuditEntry>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut files = self.files()?;
        files.reverse();
        let mut entries = Vec::new();
        for path in files {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            for line in text.lines().rev() {
                if let Ok(entry) = serde_json::from_str::<AuditEntry>(line) {
                    entries.push(entry);
                    if entries.len() >= limit {
                        return Ok(entries);
                    }
                }
            }
        }
        Ok(entries)
    }

    /// Deletes whole month files older than the retention window.
    pub fn prune(&self) -> Result<usize> {
        let cutoff = month_key(now_unix().saturating_sub(RETENTION_DAYS * SECS_PER_DAY));
        let Some(cutoff) = parse_month(&cutoff) else {
            return Ok(0);
        };
        let mut removed = 0;
        for path in self.files()? {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(month) = parse_month(name) else {
                continue;
            };
            if month < cutoff && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn prune_due(&self) {
        let now = now_unix();
        {
            let mut last = self.last_prune.lock().unwrap();
            if now.saturating_sub(*last) < PRUNE_INTERVAL_SECS {
                return;
            }
            *last = now;
        }
        let _ = self.prune();
    }

    fn files(&self) -> Result<Vec<PathBuf>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".jsonl"))
            })
            .collect();
        files.sort();
        Ok(files)
    }
}

/// `YYYY-MM` for a Unix timestamp.
pub fn month_key(time: u64) -> String {
    let (year, month, _) = civil_from_days((time / SECS_PER_DAY) as i64);
    format!("{year:04}-{month:02}")
}

fn parse_month(name: &str) -> Option<(i64, u32)> {
    let stem = name.strip_suffix(".jsonl").unwrap_or(name);
    let (year, month) = stem.split_once('-')?;
    let year = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    (1..=12).contains(&month).then_some((year, month))
}

/// Days since 1970-01-01 to civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log() -> (PathBuf, AuditLog) {
        let dir = std::env::temp_dir().join(format!("mdwork-audit-{}", Uuid::new_v4()));
        let log = AuditLog::new(&dir);
        (dir, log)
    }

    #[test]
    fn month_keys_are_calendar_months() {
        assert_eq!(month_key(0), "1970-01");
        // 2026-10-27T00:00:00Z
        assert_eq!(month_key(1_793_059_200), "2026-10");
        assert_eq!(month_key(1_709_251_200), "2024-03"); // 2024-03-01
        assert_eq!(month_key(1_704_067_200), "2024-01"); // 2024-01-01
    }

    #[test]
    fn append_writes_json_lines_without_bodies() {
        let (dir, log) = temp_log();
        let mut entry = AuditEntry::new("read_note", "代理");
        entry.request_id = "req-1".to_string();
        entry.path = Some("笔记/a.md".to_string());
        entry.bytes = Some(42);
        log.append(&entry).unwrap();
        log.append(&entry.error("DOC_DIRTY")).unwrap();

        let path = dir.join(format!("{}.jsonl", month_key(now_unix())));
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let parsed: AuditEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.tool, "read_note");
        assert_eq!(parsed.path.as_deref(), Some("笔记/a.md"));
        assert_eq!(parsed.bytes, Some(42));
        assert_eq!(parsed.result, "ok");
        let parsed: AuditEntry = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(parsed.result, "error:DOC_DIRTY");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn entries_rotate_by_month_and_list_newest_first() {
        let (dir, log) = temp_log();
        let now = now_unix();
        let older = now - 40 * SECS_PER_DAY;

        let mut first = AuditEntry::new("old", "t");
        first.time = older;
        log.append(&first).unwrap();
        let mut second = AuditEntry::new("new", "t");
        second.time = now;
        log.append(&second).unwrap();
        if month_key(older) == month_key(now) {
            // The two timestamps can land in the same month; force a second file.
            let mut far = AuditEntry::new("older", "t");
            far.time = now - 45 * SECS_PER_DAY;
            log.append(&far).unwrap();
        }

        let files = log.files().unwrap();
        assert!(files.len() >= 2, "{files:?}");
        let recent = log.recent(2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].tool, "new");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prune_drops_month_files_past_retention() {
        let (dir, log) = temp_log();
        fs::create_dir_all(&dir).unwrap();
        let old = dir.join("1999-12.jsonl");
        fs::write(&old, "{\"tool\":\"x\"}\n").unwrap();
        let current = dir.join(format!("{}.jsonl", month_key(now_unix())));
        fs::write(&current, "{\"tool\":\"y\"}\n").unwrap();
        fs::write(dir.join("notes.txt"), "keep").unwrap();

        assert_eq!(log.prune().unwrap(), 1);
        assert!(!old.exists());
        assert!(current.exists());
        assert!(dir.join("notes.txt").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_directory_reads_empty() {
        let (dir, log) = temp_log();
        assert!(log.recent(10).unwrap().is_empty());
        assert_eq!(log.prune().unwrap(), 0);
        let _ = fs::remove_dir_all(dir);
    }
}
