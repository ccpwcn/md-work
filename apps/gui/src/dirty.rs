//! Publishes notes with unsaved editor changes so the MCP server can refuse
//! to clobber them (`DOC_DIRTY`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mdwork_core::dirty::{self, DirtyDoc, DirtyState};
use mdwork_core::paths::dirty_path;
use uuid::Uuid;

#[derive(Clone)]
pub struct DirtyRegistry {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    pid: u32,
    notes: Vec<DirtyDoc>,
    /// Last snapshot that was actually written to disk, shared with the save
    /// worker so a failed write never advances it.
    saved: Arc<Mutex<Vec<DirtyDoc>>>,
}

impl DirtyRegistry {
    pub fn new() -> Self {
        let inner = Inner {
            path: dirty_path(),
            pid: std::process::id(),
            notes: Vec::new(),
            saved: Arc::new(Mutex::new(Vec::new())),
        };
        let registry = Self {
            inner: Arc::new(Mutex::new(inner)),
        };
        registry.flush();
        registry
    }

    pub fn mark(&self, notebook_id: Uuid, rel_path: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        if inner
            .notes
            .iter()
            .any(|note| note.notebook_id == notebook_id && note.rel_path == rel_path)
        {
            return;
        }
        inner.notes.push(DirtyDoc {
            notebook_id,
            rel_path: rel_path.to_string(),
        });
        inner.flush();
    }

    pub fn clear(&self, notebook_id: Uuid, rel_path: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        let before = inner.notes.len();
        inner
            .notes
            .retain(|note| !(note.notebook_id == notebook_id && note.rel_path == rel_path));
        if inner.notes.len() != before {
            inner.flush();
        }
    }

    fn flush(&self) {
        self.inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .flush();
    }
}

impl Inner {
    fn flush(&mut self) {
        {
            let saved = self.saved.lock().unwrap_or_else(|err| err.into_inner());
            if self.notes == *saved {
                return;
            }
        }
        let state = DirtyState::with_notes(self.pid, self.notes.clone());
        let path = self.path.clone();
        let notes = self.notes.clone();
        let saved = self.saved.clone();
        crate::disk::enqueue(move || match state.save_to(&path) {
            Ok(()) => {
                *saved.lock().unwrap_or_else(|err| err.into_inner()) = notes;
            }
            Err(err) => {
                // 保留旧的 saved 快照：下一次变更会重试，MCP 在这期间
                // 仍按“可能未保存”处理，避免覆盖编辑器缓冲。
                tracing::warn!(path = %path.display(), error = %err, "写入未保存标记失败，稍后重试");
            }
        });
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let path = self.path.clone();
        crate::disk::enqueue(move || dirty::remove_file(&path));
    }
}
