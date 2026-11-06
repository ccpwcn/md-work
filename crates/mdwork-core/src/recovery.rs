//! Crash recovery snapshots for dirty editor buffers.
//!
//! Every dirty buffer is copied into the app data `recovery/` directory a few
//! seconds after the last edit, so a crash or a forced kill loses at most that
//! interval. Snapshots are deleted as soon as the note is saved, and the GUI
//! offers whatever is left at startup.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::atomic::write_atomic;
use crate::{Error, Result};

pub const SNAPSHOT_FORMAT: &str = "md-work/recovery@1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub format: String,
    /// Absolute path of the note the snapshot belongs to.
    pub note_path: PathBuf,
    /// Path relative to the notebook root, for display.
    pub rel_path: String,
    #[serde(default)]
    pub notebook_root: Option<PathBuf>,
    #[serde(default)]
    pub notebook_name: Option<String>,
    /// Unix seconds when the snapshot was written.
    pub saved_at: u64,
    /// sha256 of the on-disk file the buffer was based on, when known.
    #[serde(default)]
    pub base_sha256: Option<String>,
    pub text: String,
}

impl Snapshot {
    pub fn display_name(&self) -> String {
        if self.rel_path.trim().is_empty() {
            self.note_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.note_path.display().to_string())
        } else {
            self.rel_path.clone()
        }
    }

    pub fn age_seconds(&self, now: u64) -> u64 {
        now.saturating_sub(self.saved_at)
    }
}

fn snapshot_path(dir: &Path, note_path: &Path) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(note_path.to_string_lossy().to_lowercase().as_bytes());
    let digest = hasher.finalize();
    let key: String = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    dir.join(format!("{key}.json"))
}

/// Writes (or replaces) the snapshot for `note_path`.
pub fn save(
    dir: &Path,
    note_path: &Path,
    rel_path: &str,
    notebook_root: Option<&Path>,
    notebook_name: Option<&str>,
    base_sha256: Option<&str>,
    text: &str,
) -> Result<()> {
    fs::create_dir_all(dir)?;
    let snapshot = Snapshot {
        format: SNAPSHOT_FORMAT.to_string(),
        note_path: note_path.to_path_buf(),
        rel_path: rel_path.to_string(),
        notebook_root: notebook_root.map(Path::to_path_buf),
        notebook_name: notebook_name.map(str::to_string),
        saved_at: now_unix(),
        base_sha256: base_sha256.map(str::to_string),
        text: text.to_string(),
    };
    let bytes = serde_json::to_vec(&snapshot).map_err(|source| Error::Json {
        path: snapshot_path(dir, note_path),
        source,
    })?;
    write_atomic(&snapshot_path(dir, note_path), &bytes)?;
    Ok(())
}

/// Lists snapshots, newest first. Unreadable or foreign files are skipped.
pub fn list(dir: &Path) -> Vec<Snapshot> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut snapshots: Vec<Snapshot> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let bytes = fs::read(entry.path()).ok()?;
            let snapshot: Snapshot = serde_json::from_slice(&bytes).ok()?;
            (snapshot.format == SNAPSHOT_FORMAT).then_some(snapshot)
        })
        .collect();
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.saved_at));
    snapshots
}

pub fn remove(dir: &Path, note_path: &Path) -> Result<()> {
    match fs::remove_file(snapshot_path(dir, note_path)) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub fn clear(dir: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if entry.path().extension().is_some_and(|ext| ext == "json") {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mdwork-recovery-{name}-{}-{}",
            std::process::id(),
            now_unix()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_list_remove_round_trip() {
        let dir = temp_dir("roundtrip");
        let note = PathBuf::from("E:/notes/alpha.md");
        save(
            &dir,
            &note,
            "alpha.md",
            Some(Path::new("E:/notes")),
            None,
            Some("abc"),
            "hello",
        )
        .unwrap();
        let listed = list(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].text, "hello");
        assert_eq!(listed[0].base_sha256.as_deref(), Some("abc"));
        assert_eq!(listed[0].display_name(), "alpha.md");

        save(
            &dir,
            &note,
            "alpha.md",
            Some(Path::new("E:/notes")),
            None,
            Some("abc"),
            "hello again",
        )
        .unwrap();
        let listed = list(&dir);
        assert_eq!(listed.len(), 1, "snapshots must replace, not duplicate");
        assert_eq!(listed[0].text, "hello again");

        remove(&dir, &note).unwrap();
        assert!(list(&dir).is_empty());
        remove(&dir, &note).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_and_foreign_files_are_skipped() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("broken.json"), b"{not json").unwrap();
        fs::write(
            dir.join("other.json"),
            br#"{"format":"something-else","note_path":"x","rel_path":"x","saved_at":1,"text":""}"#,
        )
        .unwrap();
        assert!(list(&dir).is_empty());
        clear(&dir).unwrap();
        assert!(list(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_directory_lists_empty() {
        let dir = temp_dir("missing");
        assert!(list(&dir).is_empty());
        clear(&dir).unwrap();
    }
}
