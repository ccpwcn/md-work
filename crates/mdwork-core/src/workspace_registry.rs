//! List of workspaces known to this user, persisted in app data rather than in
//! any single workspace, so switching works even after the current folder is
//! moved or deleted.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::paths::workspaces_path;
use crate::toml_io;
use crate::workspace::same_path;

pub const WORKSPACES_FORMAT: &str = "md-work/workspaces@1";
pub const REGISTRY_HEADER: &str = "# 墨笺 md-work · 工作区列表（可手动编辑，删除后重新添加即可）\n";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceEntry {
    pub path: String,
    /// Optional user-chosen label; the folder name is used when empty.
    pub name: String,
    pub last_opened: u64,
}

impl WorkspaceEntry {
    pub fn folder_name(&self) -> String {
        Path::new(&self.path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.path.clone())
    }

    pub fn display_name(&self) -> String {
        let alias = self.name.trim();
        if alias.is_empty() {
            self.folder_name()
        } else {
            alias.to_string()
        }
    }

    pub fn root(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    pub fn exists(&self) -> bool {
        Path::new(&self.path).is_dir()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceRegistry {
    pub format: String,
    /// Path of the workspace that was open last.
    pub active: Option<String>,
    pub workspaces: Vec<WorkspaceEntry>,
}

impl Default for WorkspaceRegistry {
    fn default() -> Self {
        Self {
            format: WORKSPACES_FORMAT.to_string(),
            active: None,
            workspaces: Vec::new(),
        }
    }
}

impl WorkspaceRegistry {
    pub fn load() -> Self {
        let path = workspaces_path();
        match Self::load_from(&path) {
            Ok(registry) => registry,
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "工作区列表读取失败，使用默认列表"
                );
                Self::default()
            }
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let Some(mut registry) = toml_io::load_with_legacy::<Self>(path)? else {
            return Ok(Self::default());
        };
        if registry.format != WORKSPACES_FORMAT {
            tracing::warn!(
                path = %path.display(),
                format = %registry.format,
                "工作区列表格式不受支持，忽略旧数据"
            );
            return Ok(Self::default());
        }
        registry.format = WORKSPACES_FORMAT.to_string();
        let mut seen: Vec<String> = Vec::new();
        registry.workspaces.retain(|entry| {
            if entry.path.trim().is_empty() {
                return false;
            }
            if seen
                .iter()
                .any(|path| same_path(Path::new(path), Path::new(&entry.path)))
            {
                return false;
            }
            seen.push(entry.path.clone());
            true
        });
        Ok(registry)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&workspaces_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        toml_io::save_with_header(path, REGISTRY_HEADER, self)
    }

    pub fn active_path(&self) -> Option<PathBuf> {
        let active = self.active.as_deref()?;
        if active.trim().is_empty() {
            return None;
        }
        Some(PathBuf::from(active))
    }

    /// Registers `root` (or refreshes it) and makes it the active workspace.
    pub fn touch(&mut self, root: &Path) -> &WorkspaceEntry {
        let now = now_secs();
        let path = root.display().to_string();
        let index = self
            .workspaces
            .iter()
            .position(|entry| same_path(Path::new(&entry.path), root));
        let index = match index {
            Some(index) => {
                self.workspaces[index].path = path.clone();
                self.workspaces[index].last_opened = now;
                index
            }
            None => {
                self.workspaces.push(WorkspaceEntry {
                    path: path.clone(),
                    name: String::new(),
                    last_opened: now,
                });
                self.workspaces.len() - 1
            }
        };
        self.active = Some(path);
        &self.workspaces[index]
    }

    pub fn set_active(&mut self, root: &Path) {
        self.active = Some(root.display().to_string());
    }

    pub fn get(&self, root: &Path) -> Option<&WorkspaceEntry> {
        self.workspaces
            .iter()
            .find(|entry| same_path(Path::new(&entry.path), root))
    }

    pub fn display_name(&self, root: &Path) -> Option<String> {
        self.get(root).map(WorkspaceEntry::display_name)
    }

    pub fn set_name(&mut self, root: &Path, name: &str) -> bool {
        let Some(entry) = self
            .workspaces
            .iter_mut()
            .find(|entry| same_path(Path::new(&entry.path), root))
        else {
            return false;
        };
        let name = name.trim();
        if entry.name == name {
            return false;
        }
        entry.name = name.to_string();
        true
    }

    /// Drops a workspace from the list without touching its files.
    pub fn forget(&mut self, root: &Path) -> bool {
        let before = self.workspaces.len();
        self.workspaces
            .retain(|entry| !same_path(Path::new(&entry.path), root));
        let removed = self.workspaces.len() != before;
        if removed
            && self
                .active
                .as_deref()
                .is_some_and(|active| same_path(Path::new(active), root))
        {
            self.active = None;
        }
        removed
    }

    /// Entries ordered by most recently opened, skipping duplicates.
    pub fn sorted(&self) -> Vec<&WorkspaceEntry> {
        let mut entries: Vec<&WorkspaceEntry> = self.workspaces.iter().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_opened));
        entries
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("mdwork-workspaces-{}.toml", Uuid::new_v4()))
    }

    #[test]
    fn missing_registry_is_empty() {
        let path = temp_path();
        let registry = WorkspaceRegistry::load_from(&path).unwrap();
        assert!(registry.workspaces.is_empty());
        assert!(registry.active_path().is_none());
    }

    #[test]
    fn touch_registers_and_updates_without_duplicates() {
        let mut registry = WorkspaceRegistry::default();
        registry.touch(Path::new(r"D:\notes"));
        registry.touch(Path::new(r"d:/NOTES\"));
        registry.touch(Path::new(r"E:\work"));
        assert_eq!(registry.workspaces.len(), 2);
        assert_eq!(
            registry.active.as_deref(),
            Some(Path::new(r"E:\work").display().to_string().as_str())
        );

        registry.touch(Path::new(r"D:\notes"));
        assert_eq!(registry.active_path().unwrap(), PathBuf::from(r"D:\notes"));
    }

    #[test]
    fn aliases_do_not_change_the_path() {
        let mut registry = WorkspaceRegistry::default();
        registry.touch(Path::new(r"D:\notes"));
        assert!(registry.set_name(Path::new(r"d:/notes"), "  工作  "));
        let entry = registry.get(Path::new(r"D:\notes")).unwrap();
        assert_eq!(entry.name, "工作");
        assert_eq!(entry.path, r"D:\notes");
        assert_eq!(entry.display_name(), "工作");

        registry.set_name(Path::new(r"D:\notes"), "");
        assert_eq!(
            registry.get(Path::new(r"D:\notes")).unwrap().display_name(),
            "notes"
        );
    }

    #[test]
    fn round_trip_and_forget() {
        let path = temp_path();
        let mut registry = WorkspaceRegistry::default();
        registry.touch(Path::new(r"D:\notes"));
        registry.touch(Path::new(r"E:\work"));
        registry.set_name(Path::new(r"E:\work"), "工作");
        registry.save_to(&path).unwrap();

        let loaded = WorkspaceRegistry::load_from(&path).unwrap();
        assert_eq!(loaded, registry);
        assert_eq!(
            loaded.display_name(Path::new(r"E:\work")).as_deref(),
            Some("工作")
        );

        let mut loaded = loaded;
        assert!(loaded.forget(Path::new(r"E:\work")));
        assert_eq!(loaded.workspaces.len(), 1);
        assert!(
            loaded.active_path().is_none(),
            "forgetting active clears it"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn duplicate_paths_are_collapsed_on_load() {
        let path = temp_path();
        let legacy = path.with_extension("json");
        let json = format!(
            r#"{{"format":"{WORKSPACES_FORMAT}","active":"D:\\a","workspaces":[
                {{"path":"D:\\a","name":"","last_opened":1}},
                {{"path":"d:/A","name":"dup","last_opened":2}},
                {{"path":"","name":"","last_opened":0}}
            ]}}"#
        );
        std::fs::write(&legacy, json).unwrap();
        let registry = WorkspaceRegistry::load_from(&path).unwrap();
        assert_eq!(registry.workspaces.len(), 1);
        assert!(path.is_file(), "legacy JSON is rewritten as TOML");
        assert!(!legacy.exists());
        assert!(
            std::fs::read_to_string(&path).unwrap().contains("D:"),
            "TOML keeps the Windows path"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_file_name(format!(
            "{}.migrated",
            legacy.file_name().unwrap().to_string_lossy()
        )));
    }
}
