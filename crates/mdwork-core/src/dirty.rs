//! Unsaved-buffer marker shared with the MCP server.
//!
//! The GUI lists notes with dirty editors in `%LOCALAPPDATA%/md-work/dirty.toml`;
//! external writers refuse to clobber those notes while the owning process
//! (recorded by pid) is still running.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Result;
use crate::paths::dirty_path;
use crate::permissions::{normalize_acl_path, now_unix};
use crate::toml_io;

pub const DIRTY_FORMAT: &str = "md-work/dirty@1";
pub const DIRTY_HEADER: &str = "# 墨笺 md-work · 未保存缓冲区标记（自动生成，可安全删除）\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyDoc {
    pub notebook_id: Uuid,
    pub rel_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DirtyState {
    pub format: String,
    /// Process that owns the dirty buffers; 0 when unknown.
    pub pid: u32,
    pub updated: u64,
    pub notes: Vec<DirtyDoc>,
}

impl Default for DirtyState {
    fn default() -> Self {
        Self {
            format: DIRTY_FORMAT.to_string(),
            pid: 0,
            updated: 0,
            notes: Vec::new(),
        }
    }
}

impl DirtyState {
    pub fn path() -> PathBuf {
        dirty_path()
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path()).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        Ok(toml_io::load_with_legacy::<Self>(path)?.unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        toml_io::save_with_header(path, DIRTY_HEADER, self)
    }

    pub fn with_notes(pid: u32, notes: Vec<DirtyDoc>) -> Self {
        Self {
            format: DIRTY_FORMAT.to_string(),
            pid,
            updated: now_unix(),
            notes,
        }
    }

    pub fn is_dirty(&self, notebook_id: &Uuid, rel_path: &str) -> bool {
        let path = normalize_acl_path(rel_path);
        self.notes
            .iter()
            .any(|doc| doc.notebook_id == *notebook_id && normalize_acl_path(&doc.rel_path) == path)
    }
}

/// Deletes the marker file; used when the owning process exits cleanly.
pub fn remove_file(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_matching() {
        let path = std::env::temp_dir().join(format!("mdwork-dirty-{}.toml", Uuid::new_v4()));
        let notebook = Uuid::new_v4();
        let state = DirtyState::with_notes(
            42,
            vec![DirtyDoc {
                notebook_id: notebook,
                rel_path: "工作/草稿.md".to_string(),
            }],
        );
        state.save_to(&path).unwrap();
        let loaded = DirtyState::load_from(&path).unwrap();
        assert_eq!(loaded, state);
        assert!(loaded.is_dirty(&notebook, "工作/草稿.md"));
        assert!(loaded.is_dirty(&notebook, r"工作\草稿.md"));
        assert!(loaded.is_dirty(&notebook, "工作/草稿.MD"));
        assert!(!loaded.is_dirty(&Uuid::new_v4(), "工作/草稿.md"));
        assert!(!loaded.is_dirty(&notebook, "工作/其他.md"));
        remove_file(&path);
        assert!(!path.exists());
        assert_eq!(DirtyState::load_from(&path).unwrap(), DirtyState::default());
    }
}
