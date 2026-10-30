use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use walkdir::WalkDir;

use crate::atomic::write_atomic;
use crate::doc::{self, DecodedDoc, EncodingHint, Eol, TextEncoding};
use crate::paths::{ASSETS_DIR, META_DIR, NOTEBOOK_FILE};
use crate::{Error, NOTEBOOK_FORMAT, Result};

pub const TRASH_DIR: &str = "trash";
pub const BOOKMARKS_FILE: &str = "bookmarks.json";
pub const ENCODING_FILE: &str = "encoding.json";
pub const ENCODING_FORMAT: &str = "md-work/encoding@1";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct EncodingFile {
    format: String,
    files: BTreeMap<String, EncodingHint>,
}

impl EncodingFile {
    fn load(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self {
                format: ENCODING_FORMAT.to_string(),
                files: BTreeMap::new(),
            };
        };
        let mut file: Self = serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), error = %err, "编码记录损坏，重新建立");
            Self::default()
        });
        if file.format.is_empty() {
            file.format = ENCODING_FORMAT.to_string();
        }
        file
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|source| crate::Error::Json {
            path: path.to_path_buf(),
            source,
        })?;
        write_atomic(path, &bytes)?;
        Ok(())
    }
}

fn encoding_key(rel_path: &str) -> String {
    rel_path.replace('\\', "/").to_lowercase()
}

fn encoding_file_path(root: &Path) -> PathBuf {
    root.join(META_DIR).join(ENCODING_FILE)
}

pub fn load_encoding_map(root: &Path) -> BTreeMap<String, EncodingHint> {
    EncodingFile::load(&encoding_file_path(root)).files
}

pub fn load_encoding_hint(root: &Path, rel_path: &str) -> Option<EncodingHint> {
    load_encoding_map(root)
        .get(&encoding_key(rel_path))
        .copied()
}

/// Stores the encoding/EOL used to save a note. UTF-8 + LF + no BOM removes
/// the entry, keeping the sidecar limited to notes that need remembering.
pub fn store_encoding_hint(root: &Path, rel_path: &str, hint: EncodingHint) -> Result<()> {
    let path = encoding_file_path(root);
    let mut file = EncodingFile::load(&path);
    let key = encoding_key(rel_path);
    if hint.is_default() {
        if file.files.remove(&key).is_none() {
            return Ok(());
        }
    } else {
        file.files.insert(key, hint);
    }
    file.save(&path)
}

pub fn remove_encoding_hint(root: &Path, rel_path: &str) -> Result<()> {
    let path = encoding_file_path(root);
    let mut file = EncodingFile::load(&path);
    if file.files.remove(&encoding_key(rel_path)).is_none() {
        return Ok(());
    }
    file.save(&path)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    pub format: String,
    pub id: Uuid,
    pub name: String,
    pub created: u64,
}

impl Marker {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format: NOTEBOOK_FORMAT.to_string(),
            id: Uuid::new_v4(),
            name: name.into(),
            created: now_unix(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Notebook {
    pub root: PathBuf,
    pub marker: Marker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Note,
    Folder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub rel_path: String,
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashItem {
    pub id: Uuid,
    pub rel_path: String,
    pub kind: EntryKind,
    pub deleted_at: u64,
}

impl Notebook {
    pub fn create(root: &Path, name: &str) -> Result<Self> {
        let marker_path = root.join(META_DIR).join(NOTEBOOK_FILE);
        if marker_path.exists() {
            return Err(Error::Exists(root.display().to_string()));
        }

        let name = sanitize_display_name(name);
        fs::create_dir_all(root.join(META_DIR).join(TRASH_DIR))?;
        fs::create_dir_all(root.join(ASSETS_DIR))?;

        let marker = Marker::new(name);
        write_marker(&marker_path, &marker)?;
        write_atomic(&root.join(META_DIR).join(BOOKMARKS_FILE), b"{}\n")?;
        write_atomic(&root.join(META_DIR).join(ENCODING_FILE), b"{}\n")?;
        tracing::info!(
            notebook = %marker.name,
            id = %marker.id,
            root = %root.display(),
            "创建笔记本"
        );

        Ok(Self {
            root: root.to_path_buf(),
            marker,
        })
    }

    pub fn open(root: &Path) -> Result<Self> {
        let marker_path = root.join(META_DIR).join(NOTEBOOK_FILE);
        let bytes = fs::read(&marker_path).map_err(|_| Error::NotNotebook(root.to_path_buf()))?;
        let marker: Marker = serde_json::from_slice(&bytes).map_err(|source| Error::Json {
            path: marker_path.clone(),
            source,
        })?;
        if marker.format != NOTEBOOK_FORMAT {
            return Err(Error::UnsupportedFormat {
                path: marker_path,
                found: marker.format,
            });
        }
        tracing::debug!(
            notebook = %marker.name,
            id = %marker.id,
            root = %root.display(),
            "打开笔记本"
        );
        Ok(Self {
            root: root.to_path_buf(),
            marker,
        })
    }

    pub fn meta_dir(&self) -> PathBuf {
        self.root.join(META_DIR)
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join(ASSETS_DIR)
    }

    pub fn trash_dir(&self) -> PathBuf {
        self.meta_dir().join(TRASH_DIR)
    }

    pub fn bookmarks_path(&self) -> PathBuf {
        self.meta_dir().join(BOOKMARKS_FILE)
    }

    pub fn encoding_path(&self) -> PathBuf {
        self.meta_dir().join(ENCODING_FILE)
    }

    pub fn encoding_hint(&self, rel_path: &str) -> Option<EncodingHint> {
        load_encoding_hint(&self.root, rel_path)
    }

    pub fn remember_encoding(&self, rel_path: &str, hint: EncodingHint) -> Result<()> {
        store_encoding_hint(&self.root, rel_path, hint)
    }

    pub fn forget_encoding(&self, rel_path: &str) -> Result<()> {
        remove_encoding_hint(&self.root, rel_path)
    }

    pub fn display_name(&self) -> &str {
        &self.marker.name
    }

    pub fn rename_display_name(&mut self, name: &str) -> Result<()> {
        self.marker.name = sanitize_display_name(name);
        self.persist_marker()
    }

    pub fn persist_marker(&self) -> Result<()> {
        write_marker(&self.meta_dir().join(NOTEBOOK_FILE), &self.marker)
    }

    pub fn contains_path(&self, path: &Path) -> bool {
        let root = self
            .root
            .to_string_lossy()
            .replace('\\', "/")
            .to_lowercase();
        let candidate = path.to_string_lossy().replace('\\', "/").to_lowercase();
        candidate == root || candidate.starts_with(&format!("{root}/"))
    }

    pub fn list_entries(&self) -> Result<Vec<Entry>> {
        Ok(self
            .walk(false)?
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::Note)
            .collect())
    }

    /// Notes and folders, sorted by path. Folder rows carry no size or mtime.
    pub fn list_tree(&self) -> Result<Vec<Entry>> {
        self.walk(true)
    }

    fn walk(&self, include_folders: bool) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        let root = self.root.clone();

        for item in WalkDir::new(&self.root)
            .min_depth(1)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !is_reserved(&root, entry.path()))
        {
            let item = item.map_err(|err| {
                Error::Io(err.into_io_error().unwrap_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::Other, "walk error")
                }))
            })?;

            if item.file_type().is_dir() {
                if !include_folders {
                    continue;
                }
                let rel_path = self.rel_path(item.path())?;
                entries.push(Entry {
                    name: item.file_name().to_string_lossy().into_owned(),
                    rel_path,
                    kind: EntryKind::Folder,
                    size: 0,
                    modified: 0,
                });
                continue;
            }
            if !item.file_type().is_file() {
                continue;
            }
            let file_name = item.file_name().to_string_lossy();
            if !is_markdown(&file_name) {
                continue;
            }

            let rel_path = self.rel_path(item.path())?;
            let metadata = item.metadata().map_err(|err| {
                Error::Io(err.into_io_error().unwrap_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::Other, "metadata error")
                }))
            })?;
            entries.push(Entry {
                name: file_name.trim_end_matches(".md").to_string(),
                rel_path,
                kind: EntryKind::Note,
                size: metadata.len(),
                modified: modified_unix(&metadata),
            });
        }

        entries.sort_by(|a, b| a.rel_path.to_lowercase().cmp(&b.rel_path.to_lowercase()));
        Ok(entries)
    }

    pub fn read_note(&self, rel_path: &str) -> Result<DecodedDoc> {
        let path = self.note_path(rel_path)?;
        let bytes = fs::read(&path).map_err(|_| Error::NotFound(rel_path.to_string()))?;
        Ok(doc::decode(&bytes))
    }

    pub fn save_note(
        &self,
        rel_path: &str,
        text: &str,
        encoding: TextEncoding,
        bom: bool,
        eol: Eol,
    ) -> Result<()> {
        let path = self.note_path(rel_path)?;
        let bytes = doc::encode_text(text, encoding, bom, eol)?;
        write_atomic(&path, &bytes)?;
        tracing::info!(
            notebook = %self.display_name(),
            path = %rel_path,
            bytes = bytes.len(),
            encoding = encoding.label(),
            "保存笔记"
        );
        Ok(())
    }

    pub fn create_note(&self, rel_path: &str, text: &str) -> Result<String> {
        let rel = self.normalize_note_rel(rel_path)?;
        let path = self.to_path(&rel)?;
        if path.exists() {
            return Err(Error::Exists(rel));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_atomic(&path, text.as_bytes())?;
        tracing::info!(
            notebook = %self.display_name(),
            path = %rel,
            bytes = text.len(),
            "新建笔记"
        );
        Ok(rel)
    }

    pub fn create_folder(&self, rel_path: &str) -> Result<String> {
        let rel = self.normalize_rel(rel_path)?;
        let path = self.to_path(&rel)?;
        if path.exists() {
            return Err(Error::Exists(rel));
        }
        fs::create_dir_all(&path)?;
        Ok(rel)
    }

    pub fn rename_entry(&self, from: &str, to: &str) -> Result<String> {
        let source = self.to_path(&self.normalize_rel(from)?)?;
        if !source.exists() {
            return Err(Error::NotFound(from.to_string()));
        }
        let is_dir = source.is_dir();
        let target_rel = if is_dir {
            self.normalize_rel(to)?
        } else {
            self.normalize_note_rel(to)?
        };
        let target = self.to_path(&target_rel)?;

        if target.exists() {
            return Err(Error::Exists(target_rel));
        }
        if is_dir {
            let source_rel = self.rel_path(&source)?;
            if target_rel.starts_with(&format!("{source_rel}/")) {
                return Err(Error::InvalidPath(target_rel));
            }
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&source, &target)?;
        tracing::info!(
            notebook = %self.display_name(),
            from = %from,
            to = %target_rel,
            "重命名条目"
        );
        Ok(target_rel)
    }

    pub fn delete_entry(&self, rel_path: &str) -> Result<TrashItem> {
        let rel = self.normalize_rel(rel_path)?;
        let source = self.to_path(&rel)?;
        if !source.exists() {
            return Err(Error::NotFound(rel));
        }
        let kind = if source.is_dir() {
            EntryKind::Folder
        } else {
            EntryKind::Note
        };

        let item = TrashItem {
            id: Uuid::new_v4(),
            rel_path: rel,
            kind,
            deleted_at: now_unix(),
        };
        let item_dir = self.trash_dir().join(item.id.to_string());
        fs::create_dir_all(&item_dir)?;
        fs::rename(&source, item_dir.join("payload"))?;
        let meta = serde_json::to_vec_pretty(&item).map_err(|source| Error::Json {
            path: item_dir.join("meta.json"),
            source,
        })?;
        write_atomic(&item_dir.join("meta.json"), &meta)?;
        tracing::info!(
            notebook = %self.display_name(),
            path = %item.rel_path,
            kind = ?kind,
            trash_id = %item.id,
            "删除条目到回收站"
        );
        Ok(item)
    }

    /// Lists trashed entries, newest first.
    pub fn list_trash(&self) -> Result<Vec<TrashItem>> {
        let dir = self.trash_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };
        let mut items = Vec::new();
        for entry in entries.flatten() {
            let meta_path = entry.path().join("meta.json");
            if !entry.path().join("payload").exists() {
                continue;
            }
            let Ok(bytes) = fs::read(&meta_path) else {
                continue;
            };
            if let Ok(item) = serde_json::from_slice::<TrashItem>(&bytes) {
                items.push(item);
            }
        }
        items.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
        Ok(items)
    }

    pub fn restore_trash_item(&self, id: &Uuid) -> Result<String> {
        let item_dir = self.trash_dir().join(id.to_string());
        let meta_path = item_dir.join("meta.json");
        let bytes =
            fs::read(&meta_path).map_err(|_| Error::NotFound(meta_path.display().to_string()))?;
        let item: TrashItem = serde_json::from_slice(&bytes).map_err(|source| Error::Json {
            path: meta_path,
            source,
        })?;

        let target_rel = self.unique_restore_rel(&item.rel_path)?;
        let target = self.to_path(&target_rel)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(item_dir.join("payload"), &target)?;
        fs::remove_dir_all(&item_dir)?;
        tracing::info!(
            notebook = %self.display_name(),
            path = %target_rel,
            original = %item.rel_path,
            trash_id = %item.id,
            "从回收站恢复条目"
        );
        Ok(target_rel)
    }

    fn unique_restore_rel(&self, rel: &str) -> Result<String> {
        if !self.to_path(rel)?.exists() {
            return Ok(rel.to_string());
        }
        let (stem, ext) = split_extension(rel);
        for n in 1..1000 {
            let suffix = if n == 1 {
                " (restored)".to_string()
            } else {
                format!(" (restored {n})")
            };
            let candidate = format!("{stem}{suffix}{ext}");
            if !self.to_path(&candidate)?.exists() {
                return Ok(candidate);
            }
        }
        Err(Error::Exists(rel.to_string()))
    }

    pub fn normalize_rel(&self, raw: &str) -> Result<String> {
        normalize_relative(raw, false)
    }

    pub fn normalize_note_rel(&self, raw: &str) -> Result<String> {
        normalize_relative(raw, true)
    }

    pub fn note_path(&self, rel_path: &str) -> Result<PathBuf> {
        let rel = self.normalize_note_rel(rel_path)?;
        self.to_path(&rel)
    }

    pub fn to_path(&self, normalized_rel: &str) -> Result<PathBuf> {
        let mut path = self.root.clone();
        for segment in normalized_rel.split('/') {
            path.push(segment);
        }
        Ok(path)
    }

    fn rel_path(&self, path: &Path) -> Result<String> {
        let rel = path
            .strip_prefix(&self.root)
            .map_err(|_| Error::Escape(path.display().to_string()))?;
        let mut parts = Vec::new();
        for component in rel.components() {
            match component {
                Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
                _ => return Err(Error::Escape(path.display().to_string())),
            }
        }
        Ok(parts.join("/"))
    }
}

fn write_marker(path: &Path, marker: &Marker) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(marker).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })?;
    write_atomic(path, &bytes)?;
    Ok(())
}

fn normalize_relative(raw: &str, is_note: bool) -> Result<String> {
    let trimmed = raw.trim().replace('\\', "/");
    if trimmed.starts_with('/') {
        return Err(Error::InvalidPath(raw.to_string()));
    }
    let trimmed = trimmed.trim_matches('/');
    if trimmed.is_empty() {
        return Err(Error::InvalidPath(raw.to_string()));
    }

    let mut segments = Vec::new();
    for (index, segment) in trimmed.split('/').enumerate() {
        if segment.is_empty() || segment.trim() != segment {
            return Err(Error::InvalidPath(raw.to_string()));
        }
        if segment == "." || segment == ".." {
            return Err(Error::InvalidPath(raw.to_string()));
        }
        if segment.starts_with('.') {
            return Err(Error::InvalidPath(raw.to_string()));
        }
        if segment.contains(':') || segment.chars().any(|c| c.is_control()) {
            return Err(Error::InvalidPath(raw.to_string()));
        }
        if segment.ends_with('.') || segment.ends_with(' ') {
            return Err(Error::InvalidPath(raw.to_string()));
        }
        if index == 0 && is_reserved_name(segment) {
            return Err(Error::ReservedName(segment.to_string()));
        }
        if is_windows_device_name(segment) {
            return Err(Error::ReservedName(segment.to_string()));
        }
        segments.push(segment.to_string());
    }

    if is_reserved_name(&segments[0]) {
        return Err(Error::ReservedName(segments[0].clone()));
    }

    let mut rel = segments.join("/");
    if rel.len() > 900 {
        return Err(Error::InvalidPath(raw.to_string()));
    }

    if is_note {
        let lower = rel.to_lowercase();
        if lower.ends_with(".md") || lower.ends_with(".markdown") {
            // keep the caller's spelling
        } else if Path::new(&rel).extension().is_none() {
            rel.push_str(".md");
        } else {
            return Err(Error::InvalidPath(format!(
                "{raw}: notes must end with .md or .markdown"
            )));
        }
    }
    Ok(rel)
}

fn is_reserved_name(segment: &str) -> bool {
    segment.eq_ignore_ascii_case(META_DIR) || segment.eq_ignore_ascii_case(ASSETS_DIR)
}

fn is_windows_device_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn is_reserved(root: &Path, path: &Path) -> bool {
    if path == root {
        return false;
    }
    let Ok(rel) = path.strip_prefix(root) else {
        return true;
    };
    let Some(first) = rel.components().next() else {
        return false;
    };
    let Component::Normal(name) = first else {
        return true;
    };
    let name = name.to_string_lossy();
    name.eq_ignore_ascii_case(META_DIR) || name.eq_ignore_ascii_case(ASSETS_DIR)
}

fn is_markdown(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

fn split_extension(rel: &str) -> (String, String) {
    let path = Path::new(rel);
    match path.extension() {
        Some(ext) => {
            let ext = format!(".{}", ext.to_string_lossy());
            let stem = rel[..rel.len() - ext.len()].to_string();
            (stem, ext)
        }
        None => (rel.to_string(), String::new()),
    }
}

fn sanitize_display_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').trim();
    if cleaned.is_empty() {
        "未命名".to_string()
    } else {
        cleaned.chars().take(80).collect()
    }
}

pub fn sanitize_notebook_folder_name(name: &str) -> String {
    sanitize_display_name(name)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn modified_unix(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_notebook() -> (PathBuf, Notebook) {
        let root = std::env::temp_dir().join(format!("mdwork-test-{}", Uuid::new_v4()));
        let notebook = Notebook::create(&root, "测试笔记本").unwrap();
        (root, notebook)
    }

    #[test]
    fn creates_marker_and_lists_notes() {
        let (root, notebook) = temp_notebook();
        let rel = notebook.create_note("工作/第一篇", "# 你好").unwrap();
        assert_eq!(rel, "工作/第一篇.md");
        let entries = notebook.list_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rel_path, "工作/第一篇.md");
        assert!(!root.join(ASSETS_DIR).join("x").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tree_lists_folders_before_notes() {
        let (root, notebook) = temp_notebook();
        notebook.create_note("工作/项目/设计", "# 设计").unwrap();
        notebook.create_note("随笔", "x").unwrap();

        let tree = notebook.list_tree().unwrap();
        let paths: Vec<(&str, EntryKind)> = tree
            .iter()
            .map(|entry| (entry.rel_path.as_str(), entry.kind))
            .collect();
        assert_eq!(
            paths,
            vec![
                ("工作", EntryKind::Folder),
                ("工作/项目", EntryKind::Folder),
                ("工作/项目/设计.md", EntryKind::Note),
                ("随笔.md", EntryKind::Note),
            ]
        );
        assert_eq!(notebook.list_entries().unwrap().len(), 2);
        assert_eq!(tree[0].name, "工作");
        assert_eq!(tree[2].name, "设计");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_paths_outside_notebook() {
        let (root, notebook) = temp_notebook();
        for bad in [
            "../escape.md",
            "/abs.md",
            r"C:\abs.md",
            ".mdwork/x.md",
            "assets/x.md",
        ] {
            assert!(notebook.normalize_note_rel(bad).is_err(), "{bad}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_and_restore_moves_through_trash() {
        let (root, notebook) = temp_notebook();
        notebook.create_note("note.md", "x").unwrap();
        let item = notebook.delete_entry("note.md").unwrap();
        assert!(!notebook.note_path("note.md").unwrap().exists());
        let trash = notebook.list_trash().unwrap();
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].rel_path, "note.md");
        let restored = notebook.restore_trash_item(&item.id).unwrap();
        assert_eq!(restored, "note.md");
        assert!(notebook.note_path("note.md").unwrap().exists());
        assert!(notebook.list_trash().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn encoding_sidecar_round_trip_and_default_cleanup() {
        let (root, notebook) = temp_notebook();
        let hint = EncodingHint {
            encoding: TextEncoding::Gb18030,
            bom: false,
            eol: Eol::Crlf,
        };
        notebook.remember_encoding("工作/笔记.md", hint).unwrap();
        assert_eq!(notebook.encoding_hint("工作\\笔记.md"), Some(hint));
        assert!(notebook.encoding_path().exists());

        notebook
            .remember_encoding("工作/笔记.md", EncodingHint::default())
            .unwrap();
        assert_eq!(notebook.encoding_hint("工作/笔记.md"), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn decode_uses_remembered_encoding_and_eol() {
        let (bytes, _, errors) = encoding_rs::GB18030.encode("第一行\r\n第二行");
        assert!(!errors);
        let hint = EncodingHint {
            encoding: TextEncoding::Gb18030,
            bom: false,
            eol: Eol::Crlf,
        };
        let decoded = doc::decode_with_hint(&bytes, Some(hint));
        assert_eq!(decoded.encoding, TextEncoding::Gb18030);
        assert_eq!(decoded.eol, Eol::Crlf);
        assert_eq!(decoded.text, "第一行\n第二行");
    }
}
