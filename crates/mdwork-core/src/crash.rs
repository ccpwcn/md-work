//! Local crash records and optional upload to the Gitee issue tracker.
//!
//! When the process panics we write a small JSON file under
//! `%LOCALAPPDATA%/md-work/crashes/`. On the next start the app may upload
//! pending records to Gitee as issues when a token is configured; note bodies
//! are never included. Without a token nothing is requested, and once Gitee
//! rejects a token as invalid the refusal is remembered in
//! `upload-disabled.json` (token fingerprint only) so later starts make no
//! request at all until the token changes.

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::config::CrashReportConfig;

const KEEP: usize = 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const REPORTED_SUFFIX: &str = ".reported";
const DISABLED_FILE: &str = "upload-disabled.json";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportSummary {
    pub reported: usize,
    pub failed: usize,
    /// Why no report was attempted; empty when at least one upload ran.
    pub skipped: String,
}

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

/// Uploads every pending crash record as a Gitee issue.
///
/// Never panics and never blocks the caller for long: the GUI runs it on a
/// background thread after startup. Without a token or network it only logs.
pub fn report_pending(config: &CrashReportConfig) -> ReportSummary {
    report_pending_in(&crate::paths::crashes_dir(), config)
}

/// Like [`report_pending`] but reads records from `dir`. Exposed for tests.
pub fn report_pending_in(dir: &Path, config: &CrashReportConfig) -> ReportSummary {
    let mut summary = ReportSummary::default();
    if !config.enabled {
        summary.skipped = "崩溃上报已在配置中关闭".to_string();
        return summary;
    }

    let pending = pending_records(dir);
    if pending.is_empty() {
        summary.skipped = "没有待上报的崩溃日志".to_string();
        return summary;
    }

    let Some(token) = config.resolved_token() else {
        summary.skipped =
            "未配置 Gitee token：请在 runtime.toml 的 [crash_report] token 或环境变量 MDWORK_GITEE_TOKEN 中设置"
                .to_string();
        tracing::info!(pending = pending.len(), "{}", summary.skipped);
        return summary;
    };

    let fingerprint = crate::doc::sha256_hex(token.as_bytes());
    if let Some(marker) = load_disabled(dir) {
        if marker.token_fingerprint == fingerprint {
            summary.skipped = format!(
                "Gitee token 已确认失效，停止上报：{}（删除 {} 或更换 token 后可重试）",
                marker.reason,
                disabled_path(dir).display()
            );
            tracing::info!(pending = pending.len(), "{}", summary.skipped);
            return summary;
        }
        let _ = std::fs::remove_file(disabled_path(dir));
    }

    let repo = config.repo.trim().trim_matches('/').to_string();
    if !repo.contains('/') || repo.split('/').any(|part| part.trim().is_empty()) {
        summary.skipped = format!("崩溃上报仓库无效：{}", config.repo);
        tracing::warn!("{}", summary.skipped);
        return summary;
    }

    if !network_available() {
        summary.skipped = "网络不可用，崩溃日志将在下次启动时重试".to_string();
        tracing::info!(pending = pending.len(), "{}", summary.skipped);
        return summary;
    }

    let url = format!(
        "{}/repos/{repo}/issues",
        config.api_base.trim().trim_end_matches('/')
    );
    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();

    for path in pending {
        match upload_one(&agent, &url, &token, &path) {
            Ok(issue_url) => {
                mark_reported(&path);
                summary.reported += 1;
                tracing::info!(
                    crash = %path.display(),
                    issue = %issue_url,
                    "已上报崩溃日志"
                );
            }
            Err(UploadError::Auth(message)) => {
                let reason = format!("Gitee token 无效或无权限：{message}");
                save_disabled(dir, &fingerprint, &reason);
                summary.skipped = format!("{reason}；后续启动不再重试，更换 token 后自动恢复");
                tracing::warn!("{}", summary.skipped);
                break;
            }
            Err(UploadError::Other(err)) => {
                summary.failed += 1;
                tracing::warn!(crash = %path.display(), error = %err, "上报崩溃日志失败");
            }
        }
    }
    summary
}

enum UploadError {
    Auth(String),
    Other(String),
}

fn upload_one(
    agent: &ureq::Agent,
    url: &str,
    token: &str,
    path: &Path,
) -> std::result::Result<String, UploadError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|err| UploadError::Other(format!("读取崩溃日志失败：{err}")))?;
    let record: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
    let version = record
        .get("version")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let os = record
        .get("os")
        .and_then(|value| value.as_str())
        .unwrap_or(std::env::consts::OS);
    let arch = record
        .get("arch")
        .and_then(|value| value.as_str())
        .unwrap_or(std::env::consts::ARCH);
    let message = record
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or("未知错误");
    let location = record
        .get("location")
        .and_then(|value| value.as_str())
        .unwrap_or("未知位置");
    let thread = record
        .get("thread")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let timestamp = record
        .get("timestamp_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or_default();

    let title = format!(
        "[崩溃] {} ({version}, {os}/{arch})",
        truncate(message, 80)
    );
    let body = format!(
        "### 自动上报的崩溃日志\n\n\
         - 版本：`{version}`\n\
         - 系统：`{os}/{arch}`\n\
         - 时间戳（毫秒）：`{timestamp}`\n\
         - 线程：`{thread}`\n\
         - 位置：`{location}`\n\n\
         ### 崩溃详情\n\n```json\n{raw}\n```\n\n\
         > 本 Issue 由墨笺在启动时自动创建；日志仅包含崩溃堆栈，不包含笔记内容。\n"
    );

    match agent
        .post(url)
        .send_form(&[
            ("access_token", token),
            ("title", &title),
            ("body", &body),
            ("labels", "bug,crash"),
        ])
    {
        Ok(response) => {
            let text = read_response(response);
            let issue = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|value| {
                    value
                        .get("html_url")
                        .or_else(|| value.get("url"))
                        .and_then(|url| url.as_str())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| text.chars().take(200).collect());
            Ok(issue)
        }
        Err(ureq::Error::Status(code, response)) => {
            let text = read_response(response);
            if code == 401 || code == 403 {
                Err(UploadError::Auth(format!("HTTP {code}")))
            } else {
                Err(UploadError::Other(format!(
                    "HTTP {code}: {}",
                    truncate(&text, 200)
                )))
            }
        }
        Err(err) => Err(UploadError::Other(err.to_string())),
    }
}

fn read_response(response: ureq::Response) -> String {
    let mut bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut response.into_reader(), &mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn truncate(text: &str, limit: usize) -> String {
    let cleaned = text.replace(['\r', '\n'], " ");
    if cleaned.chars().count() <= limit {
        cleaned
    } else {
        let mut out: String = cleaned.chars().take(limit).collect();
        out.push('…');
        out
    }
}

fn network_available() -> bool {
    let Ok(mut addresses) = "gitee.com:443".to_socket_addrs() else {
        return false;
    };
    addresses.any(|address| TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).is_ok())
}

/// Remembers that Gitee rejected a token so later starts make no request.
/// Stores only a fingerprint of the token, never the token itself.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DisabledMarker {
    token_fingerprint: String,
    reason: String,
    timestamp_ms: u64,
}

fn disabled_path(dir: &Path) -> PathBuf {
    dir.join(DISABLED_FILE)
}

fn load_disabled(dir: &Path) -> Option<DisabledMarker> {
    let text = std::fs::read_to_string(disabled_path(dir)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_disabled(dir: &Path, fingerprint: &str, reason: &str) {
    let marker = DisabledMarker {
        token_fingerprint: fingerprint.to_string(),
        reason: reason.to_string(),
        timestamp_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    };
    let Ok(bytes) = serde_json::to_vec_pretty(&marker) else {
        return;
    };
    if let Err(err) = std::fs::write(disabled_path(dir), bytes) {
        tracing::warn!(error = %err, "写入崩溃上报停用标记失败");
    }
}

fn mark_reported(path: &Path) {
    let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else {
        return;
    };
    let target = path.with_file_name(format!("{name}{REPORTED_SUFFIX}"));
    if let Err(err) = std::fs::rename(path, &target) {
        tracing::warn!(path = %path.display(), error = %err, "标记已上报崩溃日志失败");
    }
}

fn pending_records(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(u128, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let rest = name.strip_prefix("crash-")?;
            let stem = rest.strip_suffix(".json")?;
            let (millis, nanos) = stem.split_once('-')?;
            let stamp =
                millis.parse::<u128>().ok()? * 1_000_000_000 + nanos.parse::<u128>().ok()?;
            Some((stamp, path))
        })
        .collect();
    files.sort_by_key(|(stamp, _)| *stamp);
    files.into_iter().map(|(_, path)| path).collect()
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
            let rest = name.strip_prefix("crash-")?;
            let stem = rest
                .strip_suffix(".json")
                .or_else(|| rest.strip_suffix(".json.reported"))?;
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
    use crate::config::CRASH_TOKEN_ENV;

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

    #[test]
    fn reported_records_are_not_pending_but_still_pruned() {
        let dir = scratch();
        let first = record(&dir, "first", None, "main", "").unwrap();
        mark_reported(&first);
        assert!(!first.exists());
        assert!(first.with_file_name(format!(
            "{}{REPORTED_SUFFIX}",
            first.file_name().unwrap().to_string_lossy()
        )).is_file());

        let second = record(&dir, "second", None, "main", "").unwrap();
        assert_eq!(pending_records(&dir), vec![second]);

        for _ in 0..KEEP + 5 {
            record(&dir, "again", None, "worker", "").unwrap();
        }
        let count = std::fs::read_dir(&dir).unwrap().count();
        assert!(count <= KEEP, "reported files must count toward pruning");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn disabled_report_is_skipped_before_touching_disk() {
        let config = CrashReportConfig {
            enabled: false,
            ..Default::default()
        };
        let summary = report_pending(&config);
        assert_eq!(summary.reported, 0);
        assert_eq!(summary.failed, 0);
        assert!(summary.skipped.contains("关闭"));
    }

    fn config_with_token(token: &str, repo: &str) -> CrashReportConfig {
        CrashReportConfig {
            enabled: true,
            repo: repo.to_string(),
            token: token.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn confirmed_invalid_token_stops_further_requests() {
        let dir = scratch();
        let record_path = record(&dir, "boom", None, "main", "").unwrap();
        let config = config_with_token("tok-a", "ccpwcn/md-work");
        let fingerprint = crate::doc::sha256_hex(
            config.resolved_token().unwrap_or_default().as_bytes(),
        );
        save_disabled(&dir, &fingerprint, "Gitee token 无效或无权限：HTTP 401");

        let summary = report_pending_in(&dir, &config);
        assert_eq!((summary.reported, summary.failed), (0, 0));
        assert!(summary.skipped.contains("已确认失效"), "{}", summary.skipped);
        assert!(record_path.is_file(), "pending record must survive");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn changed_token_clears_stale_marker() {
        if std::env::var(CRASH_TOKEN_ENV).is_ok() {
            eprintln!("skipped: {CRASH_TOKEN_ENV} overrides the config token");
            return;
        }
        let dir = scratch();
        let _record = record(&dir, "boom", None, "main", "").unwrap();
        save_disabled(
            &dir,
            &crate::doc::sha256_hex(b"tok-a"),
            "Gitee token 无效或无权限：HTTP 401",
        );

        // An invalid repo name stops the flow before any network probe, which
        // proves the stale marker no longer short-circuits the run.
        let summary = report_pending_in(&dir, &config_with_token("tok-b", "invalid"));
        assert!(summary.skipped.contains("仓库无效"), "{}", summary.skipped);
        assert!(!disabled_path(&dir).exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
