use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::notebook::{Notebook, sanitize_notebook_folder_name};
use crate::paths::{
    META_DIR, NOTEBOOK_FILE, NOTEBOOK_FILE_LEGACY, default_workspace_root, registry_path,
    workspace_meta_dir,
};
use crate::toml_io;
use crate::{Result, WORKSPACE_FORMAT};

pub const REGISTRY_HEADER: &str = "# 墨笺 md-work · 工作区缓存（自动生成，可安全删除）\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderKind {
    Missing,
    Empty,
    Workspace,
    Notebook,
    Foreign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanIssueKind {
    NotNotebook,
    InvalidMarker,
    DuplicateIdFixed,
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct ScanIssue {
    pub path: PathBuf,
    pub kind: ScanIssueKind,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub notebooks: Vec<Notebook>,
    pub issues: Vec<ScanIssue>,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryEntry {
    id: Uuid,
    name: String,
    path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Registry {
    format: String,
    notebooks: Vec<RegistryEntry>,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn default_root() -> Result<PathBuf> {
        default_workspace_root()
    }

    pub fn open(path: &Path) -> Result<Self> {
        let workspace = Self::new(path);
        fs::create_dir_all(workspace_meta_dir(&workspace.root))?;
        tracing::info!(root = %workspace.root.display(), "打开工作区");
        Ok(workspace)
    }

    pub fn open_or_create_default() -> Result<Self> {
        Self::open(&default_workspace_root()?)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read-only classification used before opening or creating a workspace.
    pub fn inspect(path: &Path) -> FolderKind {
        if !path.is_dir() {
            return FolderKind::Missing;
        }
        if has_notebook_marker(path) {
            return FolderKind::Notebook;
        }
        if path.join(META_DIR).is_dir() {
            return FolderKind::Workspace;
        }
        match fs::read_dir(path) {
            Ok(mut entries) => match entries.next() {
                Some(_) => FolderKind::Foreign,
                None => FolderKind::Empty,
            },
            Err(_) => FolderKind::Foreign,
        }
    }

    pub fn meta_dir(&self) -> PathBuf {
        workspace_meta_dir(&self.root)
    }

    pub fn create_notebook(&self, display_name: &str) -> Result<Notebook> {
        let base = sanitize_notebook_folder_name(display_name);
        let mut candidate = base.clone();
        let mut counter = 2;
        while self.root.join(&candidate).exists() {
            candidate = format!("{base} ({counter})");
            counter += 1;
        }
        fs::create_dir_all(&self.root)?;
        let notebook = Notebook::create(&self.root.join(&candidate), display_name)?;
        tracing::info!(
            workspace = %self.root.display(),
            notebook = %notebook.display_name(),
            id = %notebook.marker.id,
            "新建笔记本"
        );
        Ok(notebook)
    }

    /// Scans registry cache, child notebook folders and external registrations.
    pub fn scan(&self) -> ScanResult {
        let mut result = ScanResult::default();
        let mut ids: HashSet<Uuid> = HashSet::new();
        let mut names: HashSet<String> = HashSet::new();

        match fs::read_dir(&self.root) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    if !has_notebook_marker(&path) {
                        if looks_like_notes(&path) {
                            result.issues.push(ScanIssue {
                                path,
                                kind: ScanIssueKind::NotNotebook,
                                message: "缺少 .mdwork/notebook.toml：请确认这是墨笺笔记本，或在墨笺中重新创建".to_string(),
                            });
                        }
                        continue;
                    }
                    match Notebook::open(&path) {
                        Ok(mut notebook) => {
                            if let Some(issue) = deduplicate(&mut notebook, &mut ids, &mut names) {
                                result.issues.push(issue);
                            }
                            result.notebooks.push(notebook);
                        }
                        Err(err) => result.issues.push(ScanIssue {
                            path,
                            kind: ScanIssueKind::InvalidMarker,
                            message: err.to_string(),
                        }),
                    }
                }
            }
            Err(err) => result.issues.push(ScanIssue {
                path: self.root.clone(),
                kind: ScanIssueKind::Unavailable,
                message: err.to_string(),
            }),
        }

        if let Some(registry) = self.load_registry() {
            for entry in registry.notebooks {
                let path = PathBuf::from(&entry.path);
                if same_path(&path, &self.root) || is_inside(&path, &self.root) {
                    continue;
                }
                match Notebook::open(&path) {
                    Ok(mut notebook) => {
                        if let Some(issue) = deduplicate(&mut notebook, &mut ids, &mut names) {
                            result.issues.push(issue);
                        }
                        result.notebooks.push(notebook);
                    }
                    Err(err) => result.issues.push(ScanIssue {
                        path,
                        kind: ScanIssueKind::Unavailable,
                        message: format!("{} ({err})", entry.path),
                    }),
                }
            }
        }

        result.notebooks.sort_by(|a, b| {
            a.display_name()
                .to_lowercase()
                .cmp(&b.display_name().to_lowercase())
        });
        tracing::debug!(
            root = %self.root.display(),
            notebooks = result.notebooks.len(),
            issues = result.issues.len(),
            "扫描工作区"
        );
        result
    }

    /// Scans and rewrites the rebuildable registry cache.
    pub fn refresh(&self) -> Result<ScanResult> {
        let result = self.scan();
        self.save_registry(&result.notebooks)?;
        Ok(result)
    }

    pub fn register_external(&self, path: &Path) -> Result<Notebook> {
        let notebook = Notebook::open(path)?;
        let mut notebooks: Vec<Notebook> = self
            .scan()
            .notebooks
            .into_iter()
            .filter(|existing| !same_path(&existing.root, &notebook.root))
            .collect();
        notebooks.push(notebook.clone());
        self.save_registry(&notebooks)?;
        tracing::info!(
            notebook = %notebook.display_name(),
            root = %notebook.root.display(),
            "注册外部笔记本"
        );
        Ok(notebook)
    }

    pub fn save_registry(&self, notebooks: &[Notebook]) -> Result<()> {
        let registry = Registry {
            format: WORKSPACE_FORMAT.to_string(),
            notebooks: notebooks
                .iter()
                .map(|notebook| RegistryEntry {
                    id: notebook.marker.id,
                    name: notebook.display_name().to_string(),
                    path: notebook.root.display().to_string(),
                })
                .collect(),
        };
        toml_io::save_with_header(&registry_path(&self.root), REGISTRY_HEADER, &registry)
    }

    fn load_registry(&self) -> Option<Registry> {
        let registry: Registry = toml_io::load_with_legacy(&registry_path(&self.root))
            .ok()
            .flatten()?;
        if registry.format == WORKSPACE_FORMAT {
            Some(registry)
        } else {
            None
        }
    }

    pub fn find_by_id(&self, id: &Uuid) -> Option<Notebook> {
        self.scan()
            .notebooks
            .into_iter()
            .find(|notebook| notebook.marker.id == *id)
    }

    pub fn notebook_for_path(&self, path: &Path) -> Option<Notebook> {
        self.scan()
            .notebooks
            .into_iter()
            .filter(|notebook| notebook.contains_path(path))
            .max_by_key(|notebook| notebook.root.as_os_str().len())
    }
}

fn deduplicate(
    notebook: &mut Notebook,
    ids: &mut HashSet<Uuid>,
    names: &mut HashSet<String>,
) -> Option<ScanIssue> {
    let mut issue = None;
    if !ids.insert(notebook.marker.id) {
        notebook.marker.id = Uuid::new_v4();
        ids.insert(notebook.marker.id);
        let base = notebook.display_name().to_string();
        let unique = unique_name(&base, names);
        notebook.marker.name = unique.clone();
        let _ = notebook.persist_marker();
        issue = Some(ScanIssue {
            path: notebook.root.clone(),
            kind: ScanIssueKind::DuplicateIdFixed,
            message: format!("notebook id collision; reassigned as {unique}"),
        });
    } else {
        names.insert(notebook.display_name().to_lowercase());
    }
    issue
}

fn unique_name(base: &str, names: &mut HashSet<String>) -> String {
    if names.insert(base.to_lowercase()) {
        return base.to_string();
    }
    for n in 2..1000 {
        let candidate = format!("{base} ({n})");
        if names.insert(candidate.to_lowercase()) {
            return candidate;
        }
    }
    let candidate = format!("{base} ({})", Uuid::new_v4());
    names.insert(candidate.to_lowercase());
    candidate
}

pub fn has_notebook_marker(path: &Path) -> bool {
    path.join(META_DIR).join(NOTEBOOK_FILE).is_file()
        || path.join(META_DIR).join(NOTEBOOK_FILE_LEGACY).is_file()
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
        == b.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
}

fn is_inside(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
    let root = root
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase();
    path.starts_with(&format!("{root}/"))
}

/// Heuristic for reporting a folder that was copied into the workspace but has
/// no notebook marker: it is probably a notebook, so tell the user why it was
/// skipped instead of silently ignoring it.
fn looks_like_notes(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            return false;
        }
        entry
            .file_type()
            .map(|kind| {
                kind.is_dir()
                    || name.to_lowercase().ends_with(".md")
                    || name.to_lowercase().ends_with(".markdown")
            })
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace() -> (PathBuf, Workspace) {
        let root = std::env::temp_dir().join(format!("mdwork-ws-{}", Uuid::new_v4()));
        let workspace = Workspace::open(&root).unwrap();
        (root, workspace)
    }

    #[test]
    fn creates_and_scans_notebooks() {
        let (root, workspace) = temp_workspace();
        workspace.create_notebook("工作").unwrap();
        workspace.create_notebook("生活").unwrap();
        let scan = workspace.refresh().unwrap();
        assert_eq!(scan.notebooks.len(), 2);
        assert!(scan.issues.is_empty());
        assert!(registry_path(&root).is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folder_copied_in_is_discovered() {
        let (root, workspace) = temp_workspace();
        let source = workspace.create_notebook("原始").unwrap();
        let copy = root.join("原始 - 副本");
        fs::create_dir_all(&copy).unwrap();
        for entry in walkdir::WalkDir::new(&source.root).min_depth(1) {
            let entry = entry.unwrap();
            let rel = entry.path().strip_prefix(&source.root).unwrap();
            let target = copy.join(rel);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&target).unwrap();
            } else {
                fs::copy(entry.path(), &target).unwrap();
            }
        }

        let scan = workspace.scan();
        assert_eq!(scan.notebooks.len(), 2);
        assert!(
            scan.issues
                .iter()
                .any(|issue| issue.kind == ScanIssueKind::DuplicateIdFixed)
        );
        let ids: HashSet<Uuid> = scan.notebooks.iter().map(|n| n.marker.id).collect();
        assert_eq!(ids.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_marker_is_reported() {
        let (root, workspace) = temp_workspace();
        let broken = root.join("坏的");
        fs::create_dir_all(broken.join(META_DIR)).unwrap();
        fs::write(broken.join(META_DIR).join(NOTEBOOK_FILE), b"{ not json").unwrap();
        let scan = workspace.scan();
        assert!(scan.notebooks.is_empty());
        assert_eq!(scan.issues.len(), 1);
        assert_eq!(scan.issues[0].kind, ScanIssueKind::InvalidMarker);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folder_without_marker_is_reported() {
        let (root, workspace) = temp_workspace();
        let plain = root.join("散落的笔记");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join("a.md"), "# a\n").unwrap();
        let scan = workspace.scan();
        assert!(scan.notebooks.is_empty());
        assert_eq!(scan.issues.len(), 1);
        assert_eq!(scan.issues[0].kind, ScanIssueKind::NotNotebook);

        let empty = root.join("空目录");
        fs::create_dir_all(&empty).unwrap();
        assert!(
            workspace.scan().issues.len() == 1,
            "quiet folders stay quiet"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_registration_round_trips() {
        let (root, workspace) = temp_workspace();
        let outside = std::env::temp_dir().join(format!("mdwork-ext-{}", Uuid::new_v4()));
        let notebook = Notebook::create(&outside, "外部").unwrap();
        workspace.register_external(&notebook.root).unwrap();
        let scan = workspace.scan();
        assert_eq!(scan.notebooks.len(), 1);
        assert!(workspace.notebook_for_path(&outside.join("a.md")).is_some());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn inspect_classifies_folders_read_only() {
        let root = std::env::temp_dir().join(format!("mdwork-inspect-{}", Uuid::new_v4()));
        let empty = root.join("empty");
        let foreign = root.join("foreign");
        let workspace_root = root.join("workspace");
        let notebook_root = root.join("notebook");
        fs::create_dir_all(&empty).unwrap();
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("notes.md"), "# x").unwrap();
        Workspace::open(&workspace_root).unwrap();
        Notebook::create(&notebook_root, "本子").unwrap();

        assert_eq!(
            Workspace::inspect(&root.join("missing")),
            FolderKind::Missing
        );
        assert_eq!(Workspace::inspect(&empty), FolderKind::Empty);
        assert_eq!(Workspace::inspect(&foreign), FolderKind::Foreign);
        assert_eq!(Workspace::inspect(&workspace_root), FolderKind::Workspace);
        assert_eq!(Workspace::inspect(&notebook_root), FolderKind::Notebook);

        assert!(
            fs::read_dir(&empty).unwrap().next().is_none(),
            "inspect must not write"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_json_marker_is_migrated_on_open() {
        let root = std::env::temp_dir().join(format!("mdwork-legacy-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join(META_DIR)).unwrap();
        let marker = crate::notebook::Marker::new("旧本");
        fs::write(
            root.join(META_DIR).join(NOTEBOOK_FILE_LEGACY),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();

        assert_eq!(Workspace::inspect(&root), FolderKind::Notebook);
        let notebook = Notebook::open(&root).unwrap();
        assert_eq!(notebook.marker.id, marker.id);
        assert!(root.join(META_DIR).join(NOTEBOOK_FILE).is_file());
        assert!(!root.join(META_DIR).join(NOTEBOOK_FILE_LEGACY).exists());
        fs::remove_dir_all(root).unwrap();
    }
}
