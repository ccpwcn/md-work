use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::notebook::{sanitize_notebook_folder_name, Notebook};
use crate::paths::{
    default_workspace_root, registry_path, workspace_meta_dir, META_DIR, NOTEBOOK_FILE,
};
use crate::{Error, Result, WORKSPACE_FORMAT};

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
        Ok(workspace)
    }

    pub fn open_or_create_default() -> Result<Self> {
        Self::open(&default_workspace_root()?)
    }

    pub fn root(&self) -> &Path {
        &self.root
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
        Notebook::create(&self.root.join(&candidate), display_name)
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
                    let marker = path.join(META_DIR).join(NOTEBOOK_FILE);
                    if !marker.is_file() {
                        continue;
                    }
                    match Notebook::open(&path) {
                        Ok(mut notebook) => {
                            if let Some(issue) =
                                deduplicate(&mut notebook, &mut ids, &mut names)
                            {
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

        result
            .notebooks
            .sort_by(|a, b| a.display_name().to_lowercase().cmp(&b.display_name().to_lowercase()));
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
        Ok(notebook)
    }

    pub fn save_registry(&self, notebooks: &[Notebook]) -> Result<()> {
        fs::create_dir_all(workspace_meta_dir(&self.root))?;
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
        let bytes = serde_json::to_vec_pretty(&registry).map_err(|source| Error::Json {
            path: registry_path(&self.root),
            source,
        })?;
        crate::atomic::write_atomic(&registry_path(&self.root), &bytes)?;
        Ok(())
    }

    fn load_registry(&self) -> Option<Registry> {
        let bytes = fs::read(registry_path(&self.root)).ok()?;
        let registry: Registry = serde_json::from_slice(&bytes).ok()?;
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

pub fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
        == b.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
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
        assert!(scan
            .issues
            .iter()
            .any(|issue| issue.kind == ScanIssueKind::DuplicateIdFixed));
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
    fn external_registration_round_trips() {
        let (root, workspace) = temp_workspace();
        let outside = std::env::temp_dir().join(format!("mdwork-ext-{}", Uuid::new_v4()));
        let notebook = Notebook::create(&outside, "外部").unwrap();
        workspace.register_external(&notebook.root).unwrap();
        let scan = workspace.scan();
        assert_eq!(scan.notebooks.len(), 1);
        assert!(workspace
            .notebook_for_path(&outside.join("a.md"))
            .is_some());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
