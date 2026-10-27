//! Note and heading bookmarks stored per notebook in `.mdwork/bookmarks.json`.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::atomic::write_atomic;
use crate::notebook::Notebook;
use crate::{Error, Result};

pub const BOOKMARKS_FORMAT: &str = "md-work/bookmarks@1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub rel_path: String,
    /// Display title for note bookmarks, heading text for heading bookmarks.
    pub title: String,
    pub heading: Option<String>,
    /// One-based line of the bookmark target.
    pub line: usize,
    pub created: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bookmarks {
    pub format: String,
    pub entries: Vec<Bookmark>,
}

impl Default for Bookmarks {
    fn default() -> Self {
        Self {
            format: BOOKMARKS_FORMAT.to_string(),
            entries: Vec::new(),
        }
    }
}

impl Bookmarks {
    pub fn load(notebook: &Notebook) -> Result<Self> {
        let path = notebook.bookmarks_path();
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                path,
                source,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(Error::Io(err)),
        }
    }

    pub fn save(&self, notebook: &Notebook) -> Result<()> {
        let path = notebook.bookmarks_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|source| Error::Json {
            path: path.clone(),
            source,
        })?;
        write_atomic(&path, &bytes)?;
        Ok(())
    }

    pub fn note(&self, rel_path: &str) -> Option<&Bookmark> {
        self.entries
            .iter()
            .find(|entry| entry.heading.is_none() && entry.rel_path == rel_path)
    }

    pub fn has_heading(&self, rel_path: &str, heading: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.rel_path == rel_path && entry.heading.as_deref() == Some(heading))
    }

    /// Adds the note bookmark unless present; returns `true` when the bookmark now exists.
    pub fn toggle_note(&mut self, rel_path: &str, title: &str) -> bool {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.heading.is_none() && entry.rel_path == rel_path)
        {
            self.entries.remove(index);
            return false;
        }
        self.entries.push(Bookmark {
            rel_path: rel_path.to_string(),
            title: title.to_string(),
            heading: None,
            line: 1,
            created: now_unix(),
        });
        true
    }

    /// Adds a heading bookmark unless present; returns `true` when it now exists.
    pub fn toggle_heading(
        &mut self,
        rel_path: &str,
        note_title: &str,
        heading: &str,
        line: usize,
    ) -> bool {
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.rel_path == rel_path && entry.heading.as_deref() == Some(heading)
        }) {
            self.entries.remove(index);
            return false;
        }
        self.entries.push(Bookmark {
            rel_path: rel_path.to_string(),
            title: note_title.to_string(),
            heading: Some(heading.to_string()),
            line,
            created: now_unix(),
        });
        true
    }

    pub fn remove(&mut self, rel_path: &str, heading: Option<&str>) {
        self.entries.retain(|entry| {
            !(entry.rel_path == rel_path && entry.heading.as_deref() == heading)
        });
    }

    /// Keeps bookmarks attached when a note or folder is renamed.
    pub fn rekey(&mut self, from: &str, to: &str) {
        let from_prefix = format!("{from}/");
        for entry in &mut self.entries {
            if entry.rel_path == from {
                entry.rel_path = to.to_string();
            } else if let Some(rest) = entry.rel_path.strip_prefix(&from_prefix) {
                entry.rel_path = format!("{to}/{rest}");
            }
        }
    }

    /// Drops entries whose notes no longer exist; returns the removed count.
    pub fn prune(&mut self, notebook: &Notebook) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|entry| notebook.note_path(&entry.rel_path).is_ok_and(|path| path.is_file()));
        before - self.entries.len()
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_notebook() -> Notebook {
        let root = std::env::temp_dir().join(format!("mdwork-bookmarks-{}", Uuid::new_v4()));
        Notebook::create(&root, "测试").unwrap()
    }

    #[test]
    fn toggle_note_and_heading_round_trip() {
        let notebook = temp_notebook();
        notebook.create_note("笔记.md", "# 一\n").unwrap();
        let mut bookmarks = Bookmarks::load(&notebook).unwrap();
        assert!(bookmarks.toggle_note("笔记.md", "笔记"));
        assert!(bookmarks.toggle_heading("笔记.md", "笔记", "一", 1));
        bookmarks.save(&notebook).unwrap();

        let loaded = Bookmarks::load(&notebook).unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert!(loaded.note("笔记.md").is_some());
        assert!(loaded.has_heading("笔记.md", "一"));

        let mut loaded = loaded;
        assert!(!loaded.toggle_note("笔记.md", "笔记"));
        assert_eq!(loaded.entries.len(), 1);
        fs::remove_dir_all(&notebook.root).unwrap();
    }

    #[test]
    fn rename_and_prune_updates_entries() {
        let notebook = temp_notebook();
        notebook.create_note("旧.md", "内容\n").unwrap();
        let mut bookmarks = Bookmarks::default();
        bookmarks.toggle_note("旧.md", "旧");
        bookmarks.toggle_note("丢失.md", "丢失");

        bookmarks.rekey("旧.md", "新.md");
        notebook.rename_entry("旧.md", "新.md").unwrap();
        assert!(bookmarks.note("新.md").is_some());
        assert_eq!(bookmarks.prune(&notebook), 1);
        assert_eq!(bookmarks.entries.len(), 1);
        fs::remove_dir_all(&notebook.root).unwrap();
    }
}
