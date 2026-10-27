//! Publishes notes with unsaved editor changes so the MCP server can refuse
//! to clobber them (`DOC_DIRTY`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use mdwork_core::dirty::{self, DirtyDoc, DirtyState};
use mdwork_core::paths::app_data_dir;
use uuid::Uuid;

#[derive(Clone)]
pub struct DirtyRegistry {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    path: PathBuf,
    pid: u32,
    notes: Vec<DirtyDoc>,
    saved: Vec<DirtyDoc>,
}

impl DirtyRegistry {
    pub fn new() -> Self {
        let inner = Inner {
            path: app_data_dir().join("dirty.json"),
            pid: std::process::id(),
            notes: Vec::new(),
            saved: Vec::new(),
        };
        let registry = Self {
            inner: Arc::new(Mutex::new(inner)),
        };
        registry.flush();
        registry
    }

    pub fn mark(&self, notebook_id: Uuid, rel_path: &str) {
        let mut inner = self.inner.lock().unwrap();
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
        let mut inner = self.inner.lock().unwrap();
        let before = inner.notes.len();
        inner
            .notes
            .retain(|note| !(note.notebook_id == notebook_id && note.rel_path == rel_path));
        if inner.notes.len() != before {
            inner.flush();
        }
    }

    fn flush(&self) {
        self.inner.lock().unwrap().flush();
    }
}

impl Inner {
    fn flush(&mut self) {
        if self.notes == self.saved {
            return;
        }
        let state = DirtyState::with_notes(self.pid, self.notes.clone());
        if state.save_to(&self.path).is_ok() {
            self.saved = self.notes.clone();
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        dirty::remove_file(&self.path);
    }
}
