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
use crate::toml_io;
use crate::{Error, NOTEBOOK_FORMAT, Result};

pub const TRASH_DIR: &str = "trash";
pub const TRASH_META_FILE: &str = "meta.toml";
pub const BOOKMARKS_FILE: &str = "bookmarks.toml";
pub const BOOKMARKS_FILE_LEGACY: &str = "bookmarks.json";
pub const ENCODING_FILE: &str = "encoding.toml";
pub const ENCODING_FILE_LEGACY: &str = "encoding.json";
pub const ENCODING_FORMAT: &str = "md-work/encoding@1";
pub const MARKER_HEADER: &str =
    "# 墨笺 md-work · 笔记本标识（请勿修改，否则可能导致笔记本无法识别）\n";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct EncodingFile {
    format: String,
    files: BTreeMap<String, EncodingHint>,
}

impl EncodingFile {
    fn fresh() -> Self {
        Self {
            format: ENCODING_FORMAT.to_string(),
            files: BTreeMap::new(),
        }
    }

    fn load(path: &Path) -> Self {
        match toml_io::load_with_legacy::<Self>(path) {
            Ok(Some(mut file)) => {
                if file.format.is_empty() {
                    file.format = ENCODING_FORMAT.to_string();
                }
                file
            }
            Ok(None) => Self::fresh(),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "编码记录损坏，重新建立");
                Self::fresh()
            }
        }
    }

    fn save(&self, path: &Path) -> Result<()> {
        toml_io::save(path, self)
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

pub fn save_encoding_map(root: &Path, files: BTreeMap<String, EncodingHint>) -> Result<()> {
    let file = EncodingFile {
        format: ENCODING_FORMAT.to_string(),
        files,
    };
    file.save(&encoding_file_path(root))
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
    #[serde(default)]
    pub created: u64,
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
        if marker_path.exists() || toml_io::legacy_path(&marker_path).exists() {
            return Err(Error::Exists(root.display().to_string()));
        }

        let name = sanitize_display_name(name);
        fs::create_dir_all(root.join(META_DIR).join(TRASH_DIR))?;
        fs::create_dir_all(root.join(ASSETS_DIR))?;

        let marker = Marker::new(name);
        write_marker(&marker_path, &marker)?;
        let notebook = Self {
            root: root.to_path_buf(),
            marker,
        };
        EncodingFile::fresh().save(&notebook.encoding_path())?;
        crate::bookmarks::Bookmarks::default().save(&notebook)?;
        tracing::info!(
            notebook = %notebook.marker.name,
            id = %notebook.marker.id,
            root = %root.display(),
            "创建笔记本"
        );

        Ok(notebook)
    }

    pub fn open(root: &Path) -> Result<Self> {
        let marker_path = root.join(META_DIR).join(NOTEBOOK_FILE);
        let marker: Marker = toml_io::load_with_legacy(&marker_path)?
            .ok_or_else(|| Error::NotNotebook(root.to_path_buf()))?;
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

    /// Renames the notebook: folder on disk first, then the marker display name.
    ///
    /// Disk rename happens before any software-side update so a locked/busy
    /// folder fails cleanly without needing to roll back the in-app name.
    /// If the marker write fails after a successful folder rename, the folder
    /// is renamed back to keep both sides consistent.
    pub fn rename_display_name(&mut self, name: &str) -> Result<()> {
        let display = sanitize_display_name(name);
        let previous_root = self.root.clone();
        let folder_renamed = self.rename_root_folder(&display)?;

        self.marker.name = display;
        if let Err(err) = self.persist_marker() {
            if folder_renamed {
                if let Err(rollback) = fs::rename(&self.root, &previous_root) {
                    tracing::error!(
                        from = %self.root.display(),
                        to = %previous_root.display(),
                        error = %rollback,
                        "笔记本目录已改名，但写入标识失败且无法回滚目录"
                    );
                } else {
                    self.root = previous_root;
                }
            }
            return Err(err);
        }
        tracing::info!(
            notebook = %self.marker.name,
            id = %self.marker.id,
            root = %self.root.display(),
            "重命名笔记本"
        );
        Ok(())
    }

    /// Renames `self.root` so its basename matches `display` (with a unique
    /// suffix on conflict). Returns whether the directory was moved.
    fn rename_root_folder(&mut self, display: &str) -> Result<bool> {
        let Some(parent) = self.root.parent() else {
            return Ok(false);
        };
        let current_name = self
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = sanitize_notebook_folder_name(display);
        if current_name == base {
            return Ok(false);
        }

        let target_name = unique_sibling_folder_name(parent, &base, &self.root);
        let target = parent.join(&target_name);
        if paths_equal_ignore_case(&self.root, &target) {
            if current_name == target_name {
                return Ok(false);
            }
            // Case-only rename on case-insensitive volumes needs a temp hop.
            let temp_name = unique_sibling_folder_name(
                parent,
                &format!("{target_name}.rename-tmp"),
                &self.root,
            );
            let temp = parent.join(&temp_name);
            fs::rename(&self.root, &temp)?;
            if let Err(err) = fs::rename(&temp, &target) {
                let _ = fs::rename(&temp, &self.root);
                return Err(err.into());
            }
        } else {
            if target.exists() {
                return Err(Error::Exists(target.display().to_string()));
            }
            fs::rename(&self.root, &target)?;
        }
        self.root = target;
        Ok(true)
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

    pub fn load_tree_order(&self) -> crate::order::TreeOrder {
        crate::order::TreeOrder::load(&self.root)
    }

    /// Records `child_name` among the children of `parent` (`""` is the root).
    /// `after_name` is a sibling directory name; `None` appends.
    pub fn place_child(
        &self,
        parent: &str,
        child_name: &str,
        after_name: Option<&str>,
    ) -> Result<()> {
        crate::order::place_child(&self.root, parent, child_name, after_name)
    }

    pub fn save_tree_order(&self, order: &crate::order::TreeOrder) -> Result<()> {
        order.save(&self.root)
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
                Error::Io(
                    err.into_io_error()
                        .unwrap_or_else(|| std::io::Error::other("walk error")),
                )
            })?;

            if item.file_type().is_dir() {
                if !include_folders {
                    continue;
                }
                let rel_path = self.rel_path(item.path())?;
                let metadata = item.metadata().ok();
                entries.push(Entry {
                    name: item.file_name().to_string_lossy().into_owned(),
                    rel_path,
                    kind: EntryKind::Folder,
                    size: 0,
                    created: metadata.as_ref().map(created_unix).unwrap_or_default(),
                    modified: metadata.as_ref().map(modified_unix).unwrap_or_default(),
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
                Error::Io(
                    err.into_io_error()
                        .unwrap_or_else(|| std::io::Error::other("metadata error")),
                )
            })?;
            entries.push(Entry {
                name: file_name.trim_end_matches(".md").to_string(),
                rel_path,
                kind: EntryKind::Note,
                size: metadata.len(),
                created: created_unix(&metadata),
                modified: modified_unix(&metadata),
            });
        }

        entries.sort_by_key(|entry| entry.rel_path.to_lowercase());
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
        let source_rel = self.rel_path(&source)?;
        let target_rel = if is_dir {
            self.normalize_rel(to)?
        } else {
            self.normalize_note_rel(to)?
        };
        let target = self.to_path(&target_rel)?;

        if target.exists() {
            return Err(Error::Exists(target_rel));
        }
        if is_dir && target_rel.starts_with(&format!("{source_rel}/")) {
            return Err(Error::InvalidPath(target_rel));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let hint = if is_dir {
            None
        } else {
            self.encoding_hint(&self.normalize_rel(from)?)
        };
        fs::rename(&source, &target)?;
        if let Some(hint) = hint {
            let _ = self.remember_encoding(&target_rel, hint);
            let _ = self.forget_encoding(from);
        } else if is_dir {
            let _ = self.rekey_encodings_under(from, &target_rel);
        }
        if let Err(err) =
            crate::order::sync_after_rename(&self.root, &source_rel, &target_rel, is_dir)
        {
            tracing::warn!(error = %err, from = %source_rel, to = %target_rel, "更新同级顺序失败");
        }
        tracing::info!(
            notebook = %self.display_name(),
            from = %from,
            to = %target_rel,
            "重命名条目"
        );
        Ok(target_rel)
    }

    /// Same-notebook copy into `to_parent` (`None` = notebook root).
    /// When `unique` is true, conflicts get a ` 副本` suffix; otherwise `Exists`.
    pub fn copy_entry(&self, from: &str, to_parent: Option<&str>, unique: bool) -> Result<String> {
        self.copy_entry_to(self, from, to_parent, unique)
    }

    /// Copy a note or folder tree into `target` under `to_parent`.
    /// Skips `.mdwork/` and `assets/`. Clones encoding hints for copied notes.
    pub fn copy_entry_to(
        &self,
        target: &Notebook,
        from: &str,
        to_parent: Option<&str>,
        unique: bool,
    ) -> Result<String> {
        let source_rel = self.normalize_rel(from)?;
        let source = self.to_path(&source_rel)?;
        if !source.exists() {
            return Err(Error::NotFound(source_rel));
        }
        let is_dir = source.is_dir();
        let basename = source_rel
            .rsplit('/')
            .next()
            .unwrap_or(source_rel.as_str())
            .to_string();
        let desired = match to_parent {
            Some(parent) => {
                let parent = target.normalize_rel(parent)?;
                format!("{parent}/{basename}")
            }
            None => basename,
        };
        let dest_rel = if unique {
            target.unique_copy_rel(&desired, is_dir)?
        } else if is_dir {
            let rel = target.normalize_rel(&desired)?;
            if target.to_path(&rel)?.exists() {
                return Err(Error::Exists(rel));
            }
            rel
        } else {
            let rel = target.normalize_note_rel(&desired)?;
            if target.to_path(&rel)?.exists() {
                return Err(Error::Exists(rel));
            }
            rel
        };

        if is_dir {
            let same_root = self.root == target.root;
            if same_root
                && (dest_rel == source_rel || dest_rel.starts_with(&format!("{source_rel}/")))
            {
                return Err(Error::InvalidPath(dest_rel));
            }
            let dest = target.to_path(&dest_rel)?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            copy_dir_recursive(&source, &dest)?;
            self.clone_encodings_under(target, &source_rel, &dest_rel)?;
        } else {
            let dest = target.to_path(&dest_rel)?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &dest)?;
            if let Some(hint) = self.encoding_hint(&source_rel) {
                target.remember_encoding(&dest_rel, hint)?;
            }
        }

        tracing::info!(
            from_notebook = %self.display_name(),
            to_notebook = %target.display_name(),
            from = %source_rel,
            to = %dest_rel,
            "复制条目"
        );
        Ok(dest_rel)
    }

    /// Relative path that does not yet exist: prefer `desired`, else `stem 副本.ext`.
    pub fn unique_copy_rel(&self, desired: &str, is_dir: bool) -> Result<String> {
        let desired = if is_dir {
            self.normalize_rel(desired)?
        } else {
            self.normalize_note_rel(desired)?
        };
        if !self.to_path(&desired)?.exists() {
            return Ok(desired);
        }
        let (stem, ext) = split_extension(&desired);
        for n in 1..1000 {
            let suffix = if n == 1 {
                " 副本".to_string()
            } else {
                format!(" 副本 {n}")
            };
            let candidate = format!("{stem}{suffix}{ext}");
            let candidate = if is_dir {
                self.normalize_rel(&candidate)?
            } else {
                self.normalize_note_rel(&candidate)?
            };
            if !self.to_path(&candidate)?.exists() {
                return Ok(candidate);
            }
        }
        Err(Error::Exists(desired))
    }

    fn clone_encodings_under(&self, target: &Notebook, from: &str, to: &str) -> Result<()> {
        let from_prefix = format!("{from}/");
        let from_key = encoding_key(from);
        let map = load_encoding_map(&self.root);
        for (key, hint) in map {
            let new_rel = if key == from_key {
                Some(to.to_string())
            } else {
                key_strip_prefix(&key, &encoding_key(&from_prefix)).map(|rest| join_rel(to, rest))
            };
            if let Some(new_rel) = new_rel {
                target.remember_encoding(&new_rel, hint)?;
            }
        }
        Ok(())
    }

    fn rekey_encodings_under(&self, from: &str, to: &str) -> Result<()> {
        let from_prefix = format!("{from}/");
        let from_key = encoding_key(from);
        let mut map = load_encoding_map(&self.root);
        let mut updates = Vec::new();
        map.retain(|key, hint| {
            if *key == from_key {
                updates.push((to.to_string(), *hint));
                false
            } else if let Some(rest) = key_strip_prefix(key, &encoding_key(&from_prefix)) {
                updates.push((join_rel(to, rest), *hint));
                false
            } else {
                true
            }
        });
        for (rel, hint) in updates {
            let key = encoding_key(&rel);
            if hint.is_default() {
                map.remove(&key);
            } else {
                map.insert(key, hint);
            }
        }
        save_encoding_map(&self.root, map)
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
        toml_io::save(&item_dir.join(TRASH_META_FILE), &item)?;
        if let Err(err) =
            crate::order::forget_child(&self.root, &item.rel_path, kind == EntryKind::Folder)
        {
            tracing::warn!(error = %err, path = %item.rel_path, "从同级顺序移除失败");
        }
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
            if !entry.path().join("payload").exists() {
                continue;
            }
            let Ok(Some(item)) =
                toml_io::load_with_legacy::<TrashItem>(&entry.path().join(TRASH_META_FILE))
            else {
                continue;
            };
            items.push(item);
        }
        items.sort_by_key(|item| std::cmp::Reverse(item.deleted_at));
        Ok(items)
    }

    pub fn restore_trash_item(&self, id: &Uuid) -> Result<String> {
        let item_dir = self.trash_dir().join(id.to_string());
        let meta_path = item_dir.join(TRASH_META_FILE);
        let item: TrashItem = toml_io::load_with_legacy(&meta_path)
            .map_err(|_| Error::NotFound(meta_path.display().to_string()))?
            .ok_or_else(|| Error::NotFound(meta_path.display().to_string()))?;

        let target_rel = self.unique_restore_rel(&item.rel_path)?;
        let target = self.to_path(&target_rel)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(item_dir.join("payload"), &target)?;
        fs::remove_dir_all(&item_dir)?;
        if let Err(err) = crate::order::place_child(
            &self.root,
            crate::order::parent_rel(&target_rel),
            crate::order::file_name(&target_rel),
            None,
        ) {
            tracing::warn!(error = %err, path = %target_rel, "恢复后的同级顺序写入失败");
        }
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
    toml_io::save_with_header(path, MARKER_HEADER, marker)
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
        // 笔记名中的点只是普通字符（如「v1.2 计划」「第1.章」）。只有显式以
        // .md / .markdown 结尾时才算带扩展名，其余一律补 .md。此前用
        // Path::extension() 判断，会把中间的点误当成扩展名并拒绝改名，导致
        // 新建笔记输入带点的名称后回车没有任何效果。
        if !lower.ends_with(".md") && !lower.ends_with(".markdown") {
            rel.push_str(".md");
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

fn join_rel(parent: &str, rest: &str) -> String {
    if rest.is_empty() {
        parent.to_string()
    } else {
        format!("{parent}/{rest}")
    }
}

fn key_strip_prefix<'a>(key: &'a str, prefix: &str) -> Option<&'a str> {
    key.strip_prefix(prefix)
}

/// Recursively copies a folder tree. Nested `.mdwork` / `assets` segments are skipped.
fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case(META_DIR) || name_str.eq_ignore_ascii_case(ASSETS_DIR) {
            continue;
        }
        let from = entry.path();
        let to = dest.join(&name);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
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

fn paths_equal_ignore_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
        == b.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
}

/// Picks a free sibling folder name under `parent`, treating `current` as ours
/// so a case-only rename can reuse the same basename.
fn unique_sibling_folder_name(parent: &Path, base: &str, current: &Path) -> String {
    let candidate = parent.join(base);
    if !candidate.exists() || paths_equal_ignore_case(&candidate, current) {
        return base.to_string();
    }
    for n in 2..10_000 {
        let name = format!("{base} ({n})");
        let path = parent.join(&name);
        if !path.exists() || paths_equal_ignore_case(&path, current) {
            return name;
        }
    }
    format!("{base} ({})", Uuid::new_v4())
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

fn created_unix(metadata: &fs::Metadata) -> u64 {
    metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_else(|| modified_unix(metadata))
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
    fn rename_display_name_renames_folder_first() {
        let parent = std::env::temp_dir().join(format!("mdwork-rename-{}", Uuid::new_v4()));
        fs::create_dir_all(&parent).unwrap();
        let mut notebook = Notebook::create(&parent.join("旧名称"), "旧名称").unwrap();
        notebook.create_note("笔记.md", "内容\n").unwrap();

        notebook.rename_display_name("新名称").unwrap();

        assert_eq!(notebook.display_name(), "新名称");
        assert_eq!(
            notebook
                .root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
            Some("新名称".to_string())
        );
        assert!(!parent.join("旧名称").exists());
        assert!(
            parent
                .join("新名称")
                .join(META_DIR)
                .join(NOTEBOOK_FILE)
                .is_file()
        );
        assert!(parent.join("新名称").join("笔记.md").is_file());
        let reopened = Notebook::open(&notebook.root).unwrap();
        assert_eq!(reopened.display_name(), "新名称");
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn rename_display_name_avoids_sibling_collision() {
        let parent = std::env::temp_dir().join(format!("mdwork-rename-coll-{}", Uuid::new_v4()));
        fs::create_dir_all(&parent).unwrap();
        let _taken = Notebook::create(&parent.join("目标"), "目标").unwrap();
        let mut notebook = Notebook::create(&parent.join("来源"), "来源").unwrap();

        notebook.rename_display_name("目标").unwrap();

        assert_eq!(notebook.display_name(), "目标");
        assert_eq!(
            notebook
                .root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
            Some("目标 (2)".to_string())
        );
        assert!(parent.join("目标").is_dir());
        assert!(parent.join("目标 (2)").is_dir());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn tree_entries_carry_timestamps_for_folders_and_notes() {
        let (root, notebook) = temp_notebook();
        notebook.create_folder("工作").unwrap();
        notebook.create_note("工作/第一篇", "# 你好").unwrap();

        let tree = notebook.list_tree().unwrap();
        let folder = tree
            .iter()
            .find(|entry| entry.kind == EntryKind::Folder)
            .unwrap();
        assert!(folder.created > 0, "folder creation time");
        assert!(folder.modified > 0, "folder modification time");
        let note = tree
            .iter()
            .find(|entry| entry.kind == EntryKind::Note)
            .unwrap();
        assert!(note.created > 0, "note creation time");
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

    /// 回归：新建笔记的名称里带点（不是扩展名分隔符）时，回车重命名必须生效。
    #[test]
    fn note_names_with_inner_dots_are_normalized_to_md() {
        let (root, notebook) = temp_notebook();
        assert_eq!(
            notebook.normalize_note_rel("v1.2 计划").unwrap(),
            "v1.2 计划.md"
        );
        assert_eq!(
            notebook.normalize_note_rel("第1.章").unwrap(),
            "第1.章.md"
        );
        assert_eq!(
            notebook.normalize_note_rel("报告.final.MD").unwrap(),
            "报告.final.MD"
        );

        let created = notebook.create_note("v1.2 计划", "内容").unwrap();
        assert_eq!(created, "v1.2 计划.md");
        assert!(root.join(&created).is_file());

        let renamed = notebook
            .rename_entry(&created, "第1.章")
            .unwrap();
        assert_eq!(renamed, "第1.章.md");
        assert!(root.join(&renamed).is_file());
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

    #[test]
    fn copy_note_and_folder_with_unique_suffix() {
        let (root, notebook) = temp_notebook();
        notebook.create_note("工作/设计.md", "# 设计").unwrap();
        let copied = notebook
            .copy_entry("工作/设计.md", Some("工作"), true)
            .unwrap();
        assert_eq!(copied, "工作/设计 副本.md");
        assert_eq!(
            fs::read_to_string(notebook.note_path(&copied).unwrap()).unwrap(),
            "# 设计"
        );

        notebook.create_folder("归档").unwrap();
        notebook.create_note("归档/a.md", "a").unwrap();
        notebook.create_note("归档/子/b.md", "b").unwrap();
        let folder_copy = notebook.copy_entry("归档", None, true).unwrap();
        assert_eq!(folder_copy, "归档 副本");
        assert!(notebook.note_path("归档 副本/a.md").unwrap().exists());
        assert!(notebook.note_path("归档 副本/子/b.md").unwrap().exists());
        // Original intact.
        assert!(notebook.note_path("归档/a.md").unwrap().exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_rejects_folder_into_itself() {
        let (root, notebook) = temp_notebook();
        notebook.create_folder("a/b").unwrap();
        let err = notebook.copy_entry("a", Some("a/b"), false).unwrap_err();
        assert!(matches!(err, Error::InvalidPath(_)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_to_other_notebook_clones_encoding() {
        let (root_a, source) = temp_notebook();
        let root_b = std::env::temp_dir().join(format!("mdwork-test-{}", Uuid::new_v4()));
        let target = Notebook::create(&root_b, "目标").unwrap();
        source.create_note("笔记.md", "内容").unwrap();
        let hint = EncodingHint {
            encoding: TextEncoding::Gb18030,
            bom: false,
            eol: Eol::Crlf,
        };
        source.remember_encoding("笔记.md", hint).unwrap();
        let copied = source
            .copy_entry_to(&target, "笔记.md", None, false)
            .unwrap();
        assert_eq!(copied, "笔记.md");
        assert_eq!(target.encoding_hint("笔记.md"), Some(hint));
        assert!(source.note_path("笔记.md").unwrap().exists());
        fs::remove_dir_all(root_a).unwrap();
        fs::remove_dir_all(root_b).unwrap();
    }

    #[test]
    fn copy_without_unique_errors_on_conflict() {
        let (root, notebook) = temp_notebook();
        notebook.create_note("a.md", "1").unwrap();
        notebook.create_folder("箱").unwrap();
        notebook.create_note("箱/a.md", "2").unwrap();
        let err = notebook.copy_entry("a.md", Some("箱"), false).unwrap_err();
        assert!(matches!(err, Error::Exists(_)));
        fs::remove_dir_all(root).unwrap();
    }
}
