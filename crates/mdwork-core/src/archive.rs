//! Whole-notebook zip archives: `manifest.json`, export, validation, import.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use walkdir::WalkDir;
use zip::read::ZipArchive;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::bookmarks::{BOOKMARKS_HEADER, Bookmarks};
use crate::notebook::{
    BOOKMARKS_FILE, BOOKMARKS_FILE_LEGACY, ENCODING_FILE, ENCODING_FILE_LEGACY, EntryKind,
    MARKER_HEADER, Notebook, TRASH_META_FILE, TrashItem, sanitize_notebook_folder_name,
};
use crate::order::TREE_ORDER_FILE;
use crate::paths::{ASSETS_DIR, META_DIR, NOTEBOOK_FILE, NOTEBOOK_FILE_LEGACY};
use crate::toml_io;
use crate::{Error, NOTEBOOK_FORMAT, Result};

pub const ARCHIVE_FORMAT: &str = "md-work/archive@1";
pub const MANIFEST_FILE: &str = "manifest.json";
pub const MAX_ARCHIVE_FILES: usize = 50_000;
pub const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;
const STAGING_DIR: &str = ".import-staging";
const WORKSPACE_TRASH_DIR: &str = "trash";

const META_FILES: [(&str, &str); 4] = [
    (NOTEBOOK_FILE, NOTEBOOK_FILE_LEGACY),
    (BOOKMARKS_FILE, BOOKMARKS_FILE_LEGACY),
    (ENCODING_FILE, ENCODING_FILE_LEGACY),
    (TREE_ORDER_FILE, TREE_ORDER_FILE),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub app_version: String,
    pub notebook_id: Uuid,
    pub notebook_name: String,
    pub created: u64,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone)]
pub struct ArchiveSummary {
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    Merge,
    Overwrite,
    Rename,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    KeepExisting,
    UseImported,
    KeepBoth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub path: String,
    pub existing_sha256: String,
    pub incoming_sha256: String,
    pub existing_size: u64,
    pub incoming_size: u64,
}

#[derive(Debug, Clone)]
pub struct ImportPlan {
    pub mode: ImportMode,
    pub notebook_name: String,
    pub notebook_id: Uuid,
    pub folder_name: String,
    pub target_root: PathBuf,
    pub target_exists: bool,
    pub target_is_notebook: bool,
    pub new_files: usize,
    pub identical_files: usize,
    pub conflicts: Vec<Conflict>,
    pub total_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub mode: ImportMode,
    pub policy: ConflictPolicy,
    /// Per-path overrides for conflicting files, keyed by `/`-separated path.
    pub choices: HashMap<String, ConflictPolicy>,
    pub rename_to: Option<String>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            mode: ImportMode::Merge,
            policy: ConflictPolicy::KeepBoth,
            choices: HashMap::new(),
            rename_to: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportOutcome {
    pub notebook: Notebook,
    pub imported: usize,
    pub skipped_identical: usize,
    pub kept_existing: usize,
    pub kept_both: usize,
    pub replaced: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct Inspection {
    manifest: Manifest,
    marker: crate::notebook::Marker,
    entries: BTreeMap<String, ManifestFile>,
}

/// Exports notes, `assets/` and the three metadata files into a zip whose
/// first-class payload is listed in `manifest.json` (path, sha256, size).
pub fn export_zip(notebook: &Notebook, output: &Path) -> Result<ArchiveSummary> {
    let entries = collect_export_entries(notebook)?;
    if entries.len() > MAX_ARCHIVE_FILES {
        return Err(Error::ArchiveLimit(format!(
            "文件数 {} 超过上限 {MAX_ARCHIVE_FILES}",
            entries.len()
        )));
    }
    let total: u64 = entries.iter().map(|(_, _, size)| *size).sum();
    if total > MAX_ARCHIVE_BYTES {
        return Err(Error::ArchiveLimit(format!(
            "解压体积 {total} 字节超过上限 {MAX_ARCHIVE_BYTES}"
        )));
    }

    let parent = output
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&parent)?;
    let file_name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "notebook.zip".to_string());
    let tmp = parent.join(format!(".{file_name}.{}.partial", Uuid::new_v4()));

    let result = write_archive(&tmp, &entries, notebook);
    if let Err(err) = result {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if output.exists() {
        fs::remove_file(output)?;
    }
    fs::rename(&tmp, output)?;
    tracing::info!(
        notebook = %notebook.display_name(),
        output = %output.display(),
        files = entries.len(),
        bytes = total,
        "导出笔记本压缩包"
    );
    Ok(ArchiveSummary {
        files: entries.len(),
        bytes: total,
    })
}

/// Validates an archive and computes the import plan without extracting it.
pub fn plan_import(
    workspace_root: &Path,
    zip_path: &Path,
    mode: ImportMode,
    rename_to: Option<&str>,
) -> Result<ImportPlan> {
    let inspection = inspect(zip_path)?;
    plan_with(workspace_root, &inspection, mode, rename_to)
}

/// Imports a notebook archive into the workspace. Staging lives in
/// `<workspace>/.import-staging/<uuid>/` and is always removed afterwards.
pub fn import_zip(
    workspace_root: &Path,
    zip_path: &Path,
    options: &ImportOptions,
) -> Result<ImportOutcome> {
    let inspection = inspect(zip_path)?;
    let plan = plan_with(
        workspace_root,
        &inspection,
        options.mode,
        options.rename_to.as_deref(),
    )?;

    let staging = workspace_root
        .join(STAGING_DIR)
        .join(Uuid::new_v4().to_string());
    let staged_notebook = staging.join("notebook");
    fs::create_dir_all(&staged_notebook)?;

    let outcome = (|| {
        extract_archive(zip_path, &inspection, &staged_notebook)?;
        normalize_staged_meta(&staged_notebook)?;
        apply_import(
            workspace_root,
            &plan,
            &inspection,
            &staged_notebook,
            options,
        )
    })();

    let _ = fs::remove_dir_all(&staging);
    match &outcome {
        Ok(result) => tracing::info!(
            zip = %zip_path.display(),
            mode = ?options.mode,
            notebook = %result.notebook.display_name(),
            imported = result.imported,
            skipped = result.skipped_identical,
            kept_both = result.kept_both,
            "导入笔记本压缩包"
        ),
        Err(err) => tracing::error!(
            zip = %zip_path.display(),
            mode = ?options.mode,
            error = %err,
            "导入笔记本压缩包失败"
        ),
    }
    outcome
}

fn write_archive(
    tmp: &Path,
    entries: &[(String, PathBuf, u64)],
    notebook: &Notebook,
) -> Result<()> {
    let file = File::create(tmp)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let mut files = Vec::with_capacity(entries.len());
    for (rel, abs, size) in entries {
        let mut source = File::open(abs).map_err(|err| Error::Io(io_with_path(err, abs)))?;
        zip.start_file(rel.as_str(), options).map_err(zip_error)?;
        let mut reader = HashReader {
            inner: &mut source,
            hasher: Sha256::new(),
            bytes: 0,
        };
        std::io::copy(&mut reader, &mut zip)?;
        if reader.bytes != *size {
            return Err(Error::InvalidArchive(format!("{} 在导出期间发生变化", rel)));
        }
        files.push(ManifestFile {
            path: rel.clone(),
            sha256: hex_digest(reader.hasher.finalize()),
            size: reader.bytes,
        });
    }

    let manifest = Manifest {
        format: ARCHIVE_FORMAT.to_string(),
        app_version: crate::version().to_string(),
        notebook_id: notebook.marker.id,
        notebook_name: notebook.display_name().to_string(),
        created: now_unix(),
        files,
    };
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| Error::Json {
        path: tmp.to_path_buf(),
        source,
    })?;
    zip.start_file(MANIFEST_FILE, options).map_err(zip_error)?;
    zip.write_all(&bytes)?;

    let file = zip.finish().map_err(zip_error)?;
    file.sync_all()?;
    Ok(())
}

fn collect_export_entries(notebook: &Notebook) -> Result<Vec<(String, PathBuf, u64)>> {
    let mut entries = Vec::new();
    for entry in notebook.list_entries()? {
        let abs = notebook.to_path(&entry.rel_path)?;
        entries.push((entry.rel_path, abs, entry.size));
    }

    let assets = notebook.assets_dir();
    if assets.is_dir() {
        for item in WalkDir::new(&assets).follow_links(false).min_depth(1) {
            let item = item.map_err(walk_error)?;
            if !item.file_type().is_file() {
                continue;
            }
            let rel = item
                .path()
                .strip_prefix(&assets)
                .map_err(|_| Error::Escape(item.path().display().to_string()))?;
            let rel = format!("{ASSETS_DIR}/{}", rel.to_string_lossy().replace('\\', "/"));
            let metadata = item.metadata().map_err(walk_error)?;
            entries.push((rel, item.path().to_path_buf(), metadata.len()));
        }
    }

    let meta_dir = notebook.meta_dir();
    for (name, legacy) in META_FILES {
        let candidate = if meta_dir.join(name).is_file() {
            name
        } else if meta_dir.join(legacy).is_file() {
            legacy
        } else {
            continue;
        };
        let abs = meta_dir.join(candidate);
        let metadata = fs::metadata(&abs)?;
        entries.push((format!("{META_DIR}/{candidate}"), abs, metadata.len()));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

fn inspect(zip_path: &Path) -> Result<Inspection> {
    let file = File::open(zip_path).map_err(|err| Error::Io(io_with_path(err, zip_path)))?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(zip_error)?;

    let mut names = Vec::new();
    let mut manifest = None;
    let mut total: u64 = 0;

    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(zip_error)?;
        let raw = entry.name().map_err(zip_error)?.into_owned();
        if raw == MANIFEST_FILE {
            if manifest.is_some() {
                return Err(Error::InvalidArchive("manifest.json 重复".to_string()));
            }
            if entry.size() > MAX_MANIFEST_BYTES {
                return Err(Error::ArchiveLimit("manifest.json 过大".to_string()));
            }
            let mut text = String::new();
            entry.take(MAX_MANIFEST_BYTES).read_to_string(&mut text)?;
            let parsed: Manifest = serde_json::from_str(&text)
                .map_err(|err| Error::InvalidArchive(format!("manifest.json 解析失败：{err}")))?;
            manifest = Some(parsed);
            continue;
        }
        if entry.encrypted() {
            return Err(Error::InvalidArchive("不支持加密条目".to_string()));
        }
        if entry.is_symlink() {
            return Err(Error::InvalidArchive(format!("不支持符号链接：{raw}")));
        }
        let path = validate_entry_path(&raw)?;
        if entry.is_dir() {
            continue;
        }
        if names.len() >= MAX_ARCHIVE_FILES {
            return Err(Error::ArchiveLimit(format!(
                "文件数超过上限 {MAX_ARCHIVE_FILES}"
            )));
        }
        total = total.saturating_add(entry.size());
        if total > MAX_ARCHIVE_BYTES {
            return Err(Error::ArchiveLimit(format!(
                "解压体积超过上限 {MAX_ARCHIVE_BYTES} 字节"
            )));
        }
        names.push(path);
    }

    let manifest =
        manifest.ok_or_else(|| Error::InvalidArchive("缺少 manifest.json".to_string()))?;
    if manifest.format != ARCHIVE_FORMAT {
        return Err(Error::UnsupportedFormat {
            path: zip_path.to_path_buf(),
            found: manifest.format,
        });
    }

    let mut listed = BTreeMap::new();
    for file in &manifest.files {
        let path = validate_entry_path(&file.path)?;
        if path != file.path {
            return Err(Error::InvalidArchive(format!(
                "manifest 路径未规范化：{}",
                file.path
            )));
        }
        if file.sha256.len() != 64 || !file.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::InvalidArchive(format!("sha256 无效：{}", file.path)));
        }
        if listed.insert(path.clone(), file.clone()).is_some() {
            return Err(Error::InvalidArchive(format!("manifest 重复列出：{path}")));
        }
    }

    let actual: HashSet<&String> = names.iter().collect();
    for name in &names {
        if !listed.contains_key(name) {
            return Err(Error::InvalidArchive(format!(
                "压缩包包含清单外的文件：{name}"
            )));
        }
    }
    for path in listed.keys() {
        if !actual.contains(path) {
            return Err(Error::InvalidArchive(format!("清单中的文件缺失：{path}")));
        }
    }

    let marker = read_marker(&mut archive)?;

    Ok(Inspection {
        manifest,
        marker,
        entries: listed,
    })
}

fn read_marker(archive: &mut ZipArchive<BufReader<File>>) -> Result<crate::notebook::Marker> {
    let toml_path = format!("{META_DIR}/{NOTEBOOK_FILE}");
    let legacy_path = format!("{META_DIR}/{NOTEBOOK_FILE_LEGACY}");
    let (path, text) = match read_zip_text(archive, &toml_path)? {
        Some(text) => (toml_path, text),
        None => match read_zip_text(archive, &legacy_path)? {
            Some(text) => (legacy_path, text),
            None => return Err(Error::InvalidArchive(format!("缺少 {toml_path}"))),
        },
    };
    let marker: crate::notebook::Marker = if path.ends_with(".toml") {
        toml::from_str(&text)
            .map_err(|err| Error::InvalidArchive(format!("{path} 解析失败：{err}")))?
    } else {
        serde_json::from_str(&text)
            .map_err(|err| Error::InvalidArchive(format!("{path} 解析失败：{err}")))?
    };
    if marker.format != NOTEBOOK_FORMAT {
        return Err(Error::UnsupportedFormat {
            path: PathBuf::from(&path),
            found: marker.format,
        });
    }
    Ok(marker)
}

fn read_zip_text(archive: &mut ZipArchive<BufReader<File>>, path: &str) -> Result<Option<String>> {
    let mut entry = match archive.by_name(path) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(err) => return Err(zip_error(err)),
    };
    if entry.size() > 1024 * 1024 {
        return Err(Error::ArchiveLimit(format!("{path} 过大")));
    }
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    Ok(Some(text))
}

fn extract_archive(zip_path: &Path, inspection: &Inspection, dest: &Path) -> Result<()> {
    let file = File::open(zip_path).map_err(|err| Error::Io(io_with_path(err, zip_path)))?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(zip_error)?;
    let mut total: u64 = 0;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(zip_error)?;
        let raw = entry.name().map_err(zip_error)?.into_owned();
        if raw == MANIFEST_FILE || entry.is_dir() {
            continue;
        }
        let path = validate_entry_path(&raw)?;
        let Some(expected) = inspection.entries.get(&path) else {
            return Err(Error::InvalidArchive(format!("清单缺少：{path}")));
        };
        let target = join_rel(dest, &path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&target).map_err(|err| Error::Io(io_with_path(err, &target)))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = entry.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total = total.saturating_add(read as u64);
            if total > MAX_ARCHIVE_BYTES {
                return Err(Error::ArchiveLimit(format!(
                    "解压体积超过上限 {MAX_ARCHIVE_BYTES} 字节"
                )));
            }
            hasher.update(&buffer[..read]);
            out.write_all(&buffer[..read])?;
        }
        out.sync_all()?;
        let sha = hex_digest(hasher.finalize());
        if sha != expected.sha256 {
            return Err(Error::InvalidArchive(format!("校验失败：{path}")));
        }
    }
    Ok(())
}

/// Rewrites legacy JSON metadata inside the staging folder as TOML, so imports
/// always land as TOML regardless of the archive's vintage.
fn normalize_staged_meta(staged: &Path) -> Result<()> {
    let meta = staged.join(META_DIR);

    let marker_path = meta.join(NOTEBOOK_FILE);
    let marker_legacy = meta.join(NOTEBOOK_FILE_LEGACY);
    if !marker_path.exists() && marker_legacy.is_file() {
        let bytes = fs::read(&marker_legacy)?;
        let marker: crate::notebook::Marker =
            serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                path: marker_legacy.clone(),
                source,
            })?;
        toml_io::save_with_header(&marker_path, MARKER_HEADER, &marker)?;
        let _ = fs::remove_file(&marker_legacy);
    }

    let bookmarks_path = meta.join(BOOKMARKS_FILE);
    let bookmarks_legacy = meta.join(BOOKMARKS_FILE_LEGACY);
    if !bookmarks_path.exists() && bookmarks_legacy.is_file() {
        let bytes = fs::read(&bookmarks_legacy)?;
        let bookmarks: Bookmarks =
            serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                path: bookmarks_legacy.clone(),
                source,
            })?;
        toml_io::save_with_header(&bookmarks_path, BOOKMARKS_HEADER, &bookmarks)?;
        let _ = fs::remove_file(&bookmarks_legacy);
    }

    let encoding_path = meta.join(ENCODING_FILE);
    let encoding_legacy = meta.join(ENCODING_FILE_LEGACY);
    if !encoding_path.exists() && encoding_legacy.is_file() {
        let bytes = fs::read(&encoding_legacy)?;
        let legacy: LegacyEncoding =
            serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                path: encoding_legacy.clone(),
                source,
            })?;
        crate::notebook::save_encoding_map(staged, legacy.files)?;
        let _ = fs::remove_file(&encoding_legacy);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct LegacyEncoding {
    #[serde(default)]
    files: BTreeMap<String, crate::EncodingHint>,
}

fn plan_with(
    workspace_root: &Path,
    inspection: &Inspection,
    mode: ImportMode,
    rename_to: Option<&str>,
) -> Result<ImportPlan> {
    let display = if inspection.marker.name.trim().is_empty() {
        inspection.manifest.notebook_name.clone()
    } else {
        inspection.marker.name.clone()
    };
    let base = sanitize_notebook_folder_name(rename_to.unwrap_or(&display));
    let (folder_name, target_root) = match mode {
        ImportMode::Merge | ImportMode::Overwrite => (base.clone(), workspace_root.join(&base)),
        ImportMode::Rename => {
            let unique = unique_folder_name(workspace_root, &base);
            (unique.clone(), workspace_root.join(&unique))
        }
    };

    let target_exists = target_root.exists();
    let target_is_notebook = crate::workspace::has_notebook_marker(&target_root);
    if mode == ImportMode::Merge && target_exists && !target_is_notebook {
        return Err(Error::Exists(format!(
            "{} 已存在且不是墨笺笔记本",
            target_root.display()
        )));
    }

    let mut conflicts = Vec::new();
    let mut new_files = 0;
    let mut identical = 0;
    let total_bytes = inspection
        .entries
        .values()
        .map(|file| file.size)
        .sum::<u64>();

    if mode == ImportMode::Merge && target_is_notebook {
        for (path, incoming) in &inspection.entries {
            if is_meta_path(path) {
                continue;
            }
            let existing = join_rel(&target_root, path);
            if !existing.is_file() {
                new_files += 1;
                continue;
            }
            let (sha, size) = sha256_file(&existing)?;
            if sha == incoming.sha256 {
                identical += 1;
            } else {
                conflicts.push(Conflict {
                    path: path.clone(),
                    existing_sha256: sha,
                    incoming_sha256: incoming.sha256.clone(),
                    existing_size: size,
                    incoming_size: incoming.size,
                });
            }
        }
    } else {
        new_files = inspection
            .entries
            .keys()
            .filter(|path| !is_meta_path(path))
            .count();
    }

    Ok(ImportPlan {
        mode,
        notebook_name: display,
        notebook_id: inspection.marker.id,
        folder_name,
        target_root,
        target_exists,
        target_is_notebook,
        new_files,
        identical_files: identical,
        conflicts,
        total_bytes,
    })
}

fn apply_import(
    workspace_root: &Path,
    plan: &ImportPlan,
    inspection: &Inspection,
    staged_notebook: &Path,
    options: &ImportOptions,
) -> Result<ImportOutcome> {
    match plan.mode {
        ImportMode::Merge => apply_merge(plan, staged_notebook, options),
        ImportMode::Overwrite => {
            let replaced = if plan.target_exists {
                Some(move_to_workspace_trash(
                    workspace_root,
                    &plan.target_root,
                    &plan.folder_name,
                )?)
            } else {
                None
            };
            if plan.target_root.exists() {
                return Err(Error::Exists(plan.target_root.display().to_string()));
            }
            fs::create_dir_all(workspace_root)?;
            fs::rename(staged_notebook, &plan.target_root)?;
            let notebook = Notebook::open(&plan.target_root)?;
            Ok(ImportOutcome {
                notebook,
                imported: plan.new_files,
                skipped_identical: 0,
                kept_existing: 0,
                kept_both: 0,
                replaced,
            })
        }
        ImportMode::Rename => {
            let mut marker = inspection.marker.clone();
            marker.id = Uuid::new_v4();
            marker.name = plan.folder_name.clone();
            let marker_path = staged_notebook.join(META_DIR).join(NOTEBOOK_FILE);
            toml_io::save_with_header(&marker_path, MARKER_HEADER, &marker)?;
            if plan.target_root.exists() {
                return Err(Error::Exists(plan.target_root.display().to_string()));
            }
            fs::rename(staged_notebook, &plan.target_root)?;
            let notebook = Notebook::open(&plan.target_root)?;
            Ok(ImportOutcome {
                notebook,
                imported: plan.new_files,
                skipped_identical: 0,
                kept_existing: 0,
                kept_both: 0,
                replaced: None,
            })
        }
    }
}

fn apply_merge(
    plan: &ImportPlan,
    staged_notebook: &Path,
    options: &ImportOptions,
) -> Result<ImportOutcome> {
    if !plan.target_is_notebook {
        if plan.target_root.exists() {
            return Err(Error::Exists(plan.target_root.display().to_string()));
        }
        if let Some(parent) = plan.target_root.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(staged_notebook, &plan.target_root)?;
        let notebook = Notebook::open(&plan.target_root)?;
        return Ok(ImportOutcome {
            notebook,
            imported: plan.new_files,
            skipped_identical: 0,
            kept_existing: 0,
            kept_both: 0,
            replaced: None,
        });
    }

    let target_root = &plan.target_root;
    let mut imported = 0;
    let mut skipped_identical = 0;
    let mut kept_existing = 0;
    let mut kept_both = 0;

    for item in WalkDir::new(staged_notebook)
        .follow_links(false)
        .min_depth(1)
    {
        let item = item.map_err(walk_error)?;
        if !item.file_type().is_file() {
            continue;
        }
        let rel = item
            .path()
            .strip_prefix(staged_notebook)
            .map_err(|_| Error::Escape(item.path().display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if is_meta_path(&rel) {
            continue;
        }
        let target = join_rel(target_root, &rel);
        if !target.exists() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(item.path(), &target)?;
            imported += 1;
            continue;
        }

        let (staged_sha, _) = sha256_file(item.path())?;
        let (target_sha, _) = sha256_file(&target)?;
        if staged_sha == target_sha {
            skipped_identical += 1;
            let _ = fs::remove_file(item.path());
            continue;
        }

        let policy = options.choices.get(&rel).copied().unwrap_or(options.policy);
        match policy {
            ConflictPolicy::KeepExisting => {
                kept_existing += 1;
                let _ = fs::remove_file(item.path());
            }
            ConflictPolicy::UseImported => {
                fs::remove_file(&target)?;
                fs::rename(item.path(), &target)?;
                imported += 1;
            }
            ConflictPolicy::KeepBoth => {
                let unique = unique_file_name(&target);
                fs::rename(item.path(), &unique)?;
                kept_both += 1;
            }
        }
    }

    merge_bookmarks(target_root, staged_notebook)?;
    merge_encoding(target_root, staged_notebook)?;

    let notebook = Notebook::open(target_root)?;
    Ok(ImportOutcome {
        notebook,
        imported,
        skipped_identical,
        kept_existing,
        kept_both,
        replaced: None,
    })
}

fn merge_bookmarks(target_root: &Path, staged_notebook: &Path) -> Result<()> {
    let staged = staged_notebook.join(META_DIR).join(BOOKMARKS_FILE);
    let Ok(Some(incoming)) = toml_io::load::<Bookmarks>(&staged) else {
        return Ok(());
    };
    if incoming.entries.is_empty() {
        return Ok(());
    }
    let notebook = Notebook::open(target_root)?;
    let mut existing = Bookmarks::load(&notebook)?;
    let before = existing.entries.len();
    for entry in incoming.entries {
        let duplicate = existing.entries.iter().any(|current| {
            current.rel_path == entry.rel_path
                && current.heading == entry.heading
                && current.line == entry.line
        });
        if !duplicate {
            existing.entries.push(entry);
        }
    }
    if existing.entries.len() != before {
        existing.save(&notebook)?;
    }
    Ok(())
}

fn merge_encoding(target_root: &Path, staged_notebook: &Path) -> Result<()> {
    let incoming = crate::notebook::load_encoding_map(staged_notebook);
    if incoming.is_empty() {
        return Ok(());
    }
    let mut merged = crate::notebook::load_encoding_map(target_root);
    let mut changed = false;
    for (key, value) in incoming {
        if let std::collections::btree_map::Entry::Vacant(slot) = merged.entry(key) {
            slot.insert(value);
            changed = true;
        }
    }
    if changed {
        crate::notebook::save_encoding_map(target_root, merged)?;
    }
    Ok(())
}

fn move_to_workspace_trash(
    workspace_root: &Path,
    source: &Path,
    rel_name: &str,
) -> Result<PathBuf> {
    let id = Uuid::new_v4();
    let item_dir = workspace_root
        .join(META_DIR)
        .join(WORKSPACE_TRASH_DIR)
        .join(id.to_string());
    fs::create_dir_all(&item_dir)?;
    let payload = item_dir.join("payload");
    fs::rename(source, &payload)?;
    let item = TrashItem {
        id,
        rel_path: rel_name.to_string(),
        kind: EntryKind::Folder,
        deleted_at: now_unix(),
    };
    toml_io::save(&item_dir.join(TRASH_META_FILE), &item)?;
    Ok(payload)
}

/// Validates one archive entry path and returns it normalized with `/`.
fn validate_entry_path(raw: &str) -> Result<String> {
    if raw.is_empty() {
        return Err(Error::InvalidArchive("条目路径为空".to_string()));
    }
    if raw.contains('\\') {
        return Err(Error::InvalidArchive(format!("条目路径包含反斜杠：{raw}")));
    }
    if raw.starts_with('/') {
        return Err(Error::InvalidArchive(format!("条目路径为绝对路径：{raw}")));
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err(Error::InvalidArchive(format!("条目路径含控制字符：{raw}")));
    }

    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(Error::InvalidArchive("条目路径为空".to_string()));
    }
    let in_assets = trimmed
        .split('/')
        .next()
        .is_some_and(|first| first.eq_ignore_ascii_case(ASSETS_DIR));
    let mut segments = Vec::new();
    for (index, segment) in trimmed.split('/').enumerate() {
        if segment.is_empty() {
            return Err(Error::InvalidArchive(format!("条目路径含空段：{raw}")));
        }
        if segment == "." || segment == ".." {
            return Err(Error::InvalidArchive(format!("条目路径含 ..：{raw}")));
        }
        if segment.ends_with('.') || segment.ends_with(' ') {
            return Err(Error::InvalidArchive(format!(
                "条目路径段以点或空格结尾：{raw}"
            )));
        }
        if segment.contains(':') {
            return Err(Error::InvalidArchive(format!("条目路径含冒号：{raw}")));
        }
        if is_windows_device_name(segment) {
            return Err(Error::InvalidArchive(format!("条目路径含设备名：{raw}")));
        }
        if index == 0 {
            let lower = segment.to_ascii_lowercase();
            if lower == META_DIR {
                if !is_allowed_meta_file(trimmed) {
                    return Err(Error::InvalidArchive(format!("不允许的元数据文件：{raw}")));
                }
            } else if lower == ASSETS_DIR {
                // assets/ may contain arbitrary file names, including dotfiles.
            } else if segment.starts_with('.') {
                return Err(Error::InvalidArchive(format!("条目路径以点开头：{raw}")));
            }
        } else if segment.starts_with('.') && !in_assets {
            return Err(Error::InvalidArchive(format!("条目路径以点开头：{raw}")));
        }
        segments.push(segment);
    }
    let normalized = segments.join("/");
    if normalized.len() > 900 {
        return Err(Error::InvalidArchive(format!("条目路径过长：{raw}")));
    }
    if normalized == MANIFEST_FILE {
        return Err(Error::InvalidArchive(
            "manifest.json 不能作为普通文件".to_string(),
        ));
    }
    Ok(normalized)
}

fn is_allowed_meta_file(path: &str) -> bool {
    META_FILES.iter().any(|(name, legacy)| {
        path.eq_ignore_ascii_case(&format!("{META_DIR}/{name}"))
            || path.eq_ignore_ascii_case(&format!("{META_DIR}/{legacy}"))
    })
}

fn is_meta_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower == META_DIR || lower.starts_with(&format!("{META_DIR}/"))
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

fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for segment in rel.split('/') {
        path.push(segment);
    }
    path
}

fn unique_folder_name(workspace_root: &Path, base: &str) -> String {
    if !workspace_root.join(base).exists() {
        return base.to_string();
    }
    for n in 2..10_000 {
        let candidate = format!("{base} ({n})");
        if !workspace_root.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{base} ({})", Uuid::new_v4())
}

fn unique_file_name(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let extension = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..10_000 {
        let suffix = if n == 1 {
            " (导入)".to_string()
        } else {
            format!(" (导入 {n})")
        };
        let candidate = parent.join(format!("{stem}{suffix}{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem} (导入 {}){extension}", Uuid::new_v4()))
}

fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path).map_err(|err| Error::Io(io_with_path(err, path)))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
    }
    Ok((hex_digest(hasher.finalize()), size))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn zip_error(err: zip::result::ZipError) -> Error {
    match err {
        zip::result::ZipError::Io(source) => Error::Io(source),
        other => Error::InvalidArchive(other.to_string()),
    }
}

fn walk_error(err: walkdir::Error) -> Error {
    Error::Io(
        err.into_io_error()
            .unwrap_or_else(|| std::io::Error::other("walk error")),
    )
}

fn io_with_path(err: std::io::Error, path: &Path) -> std::io::Error {
    std::io::Error::new(err.kind(), format!("{}: {err}", path.display()))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

struct HashReader<'a> {
    inner: &'a mut File,
    hasher: Sha256,
    bytes: u64,
}

impl Read for HashReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        if read > 0 {
            self.hasher.update(&buffer[..read]);
            self.bytes += read as u64;
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmarks::Bookmark;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("mdwork-archive-{tag}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_test_zip(
        path: &Path,
        files: &[(String, Vec<u8>, Option<u32>)],
        manifest: Option<&Manifest>,
    ) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let base = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, bytes, mode) in files {
            let options = match mode {
                Some(mode) => base
                    .system(zip::System::Unix)
                    .external_attributes(mode << 16),
                None => base,
            };
            zip.start_file(name.as_str(), options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        if let Some(manifest) = manifest {
            let bytes = serde_json::to_vec(manifest).unwrap();
            zip.start_file(MANIFEST_FILE, base).unwrap();
            zip.write_all(&bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    fn manifest_for(
        files: &[(String, Vec<u8>, Option<u32>)],
        marker: &crate::notebook::Marker,
    ) -> Manifest {
        Manifest {
            format: ARCHIVE_FORMAT.to_string(),
            app_version: "test".to_string(),
            notebook_id: marker.id,
            notebook_name: marker.name.clone(),
            created: 0,
            files: files
                .iter()
                .map(|(path, bytes, _)| ManifestFile {
                    path: path.clone(),
                    sha256: hex_digest(Sha256::digest(bytes)),
                    size: bytes.len() as u64,
                })
                .collect(),
        }
    }

    fn note_file(rel: &str, text: &str) -> (String, Vec<u8>, Option<u32>) {
        (rel.to_string(), text.as_bytes().to_vec(), None)
    }

    fn meta_file(marker: &crate::notebook::Marker) -> (String, Vec<u8>, Option<u32>) {
        (
            format!("{META_DIR}/{NOTEBOOK_FILE}"),
            toml::to_string_pretty(marker).unwrap().into_bytes(),
            None,
        )
    }

    #[test]
    fn exports_and_imports_round_trip() {
        let source_root = temp_dir("export-src");
        let notebook = Notebook::create(&source_root, "我的笔记本").unwrap();
        notebook
            .create_note("工作/第一篇.md", "# 标题\n\n正文")
            .unwrap();
        fs::create_dir_all(notebook.assets_dir()).unwrap();
        fs::write(notebook.assets_dir().join("demo.png"), b"PNG-BYTES").unwrap();
        fs::write(notebook.assets_dir().join(".gitkeep"), b"").unwrap();
        let mut bookmarks = Bookmarks::load(&notebook).unwrap();
        bookmarks.entries.push(Bookmark {
            rel_path: "工作/第一篇.md".to_string(),
            title: "标题".to_string(),
            heading: Some("标题".to_string()),
            line: 1,
            created: 1,
        });
        bookmarks.save(&notebook).unwrap();
        notebook.create_note("垃圾.md", "x").unwrap();
        notebook.delete_entry("垃圾.md").unwrap();

        let zip_path = source_root.join("导出.zip");
        let summary = export_zip(&notebook, &zip_path).unwrap();
        assert_eq!(summary.files, 6);
        assert!(summary.bytes > 0);

        let target_root = temp_dir("import-dst");
        let workspace = target_root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let outcome = import_zip(
            &workspace,
            &zip_path,
            &ImportOptions {
                mode: ImportMode::Merge,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(outcome.notebook.marker.id, notebook.marker.id);
        assert_eq!(
            fs::read_to_string(outcome.notebook.note_path("工作/第一篇.md").unwrap()).unwrap(),
            "# 标题\n\n正文"
        );
        assert!(outcome.notebook.assets_dir().join("demo.png").is_file());
        assert!(outcome.notebook.assets_dir().join(".gitkeep").is_file());
        assert_eq!(Bookmarks::load(&outcome.notebook).unwrap().entries.len(), 1);
        assert!(outcome.notebook.list_trash().unwrap().is_empty());
        assert!(
            !workspace.join(STAGING_DIR).exists()
                || fs::read_dir(workspace.join(STAGING_DIR))
                    .unwrap()
                    .next()
                    .is_none()
        );

        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(target_root);
    }

    #[test]
    fn merge_handles_identical_and_conflicting_files() {
        let source_root = temp_dir("merge-src");
        let source = Notebook::create(&source_root.join("source"), "我的笔记本").unwrap();
        source.create_note("共享.md", "导入内容").unwrap();
        source.create_note("仅导入.md", "新文件").unwrap();
        let zip_path = source_root.join("source.zip");
        export_zip(&source, &zip_path).unwrap();

        let target_root = temp_dir("merge-dst");
        let workspace = target_root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let existing = Notebook::create(&workspace.join("我的笔记本"), "我的笔记本").unwrap();
        existing.create_note("共享.md", "导入内容").unwrap();
        existing.create_note("本地.md", "本地").unwrap();

        let plan = plan_import(&workspace, &zip_path, ImportMode::Merge, None).unwrap();
        assert_eq!(plan.identical_files, 1);
        assert!(plan.conflicts.is_empty());
        assert_eq!(plan.new_files, 1);

        let outcome = import_zip(
            &workspace,
            &zip_path,
            &ImportOptions {
                mode: ImportMode::Merge,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.skipped_identical, 1);
        assert_eq!(outcome.imported, 1);
        assert!(existing.note_path("仅导入.md").unwrap().is_file());

        source
            .save_note(
                "共享.md",
                "更新内容",
                crate::doc::TextEncoding::Utf8,
                false,
                crate::doc::Eol::Lf,
            )
            .unwrap();
        export_zip(&source, &zip_path).unwrap();
        let plan = plan_import(&workspace, &zip_path, ImportMode::Merge, None).unwrap();
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.conflicts[0].path, "共享.md");

        let outcome = import_zip(
            &workspace,
            &zip_path,
            &ImportOptions {
                mode: ImportMode::Merge,
                policy: ConflictPolicy::KeepBoth,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.kept_both, 1);
        assert_eq!(
            fs::read_to_string(existing.note_path("共享.md").unwrap()).unwrap(),
            "导入内容"
        );
        assert_eq!(
            fs::read_to_string(existing.note_path("共享 (导入).md").unwrap()).unwrap(),
            "更新内容"
        );

        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(target_root);
    }

    #[test]
    fn merge_use_imported_replaces_conflict() {
        let source_root = temp_dir("use-src");
        let source = Notebook::create(&source_root.join("source"), "笔记本").unwrap();
        source.create_note("共享.md", "导入版").unwrap();
        let zip_path = source_root.join("source.zip");
        export_zip(&source, &zip_path).unwrap();

        let target_root = temp_dir("use-dst");
        let workspace = target_root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let existing = Notebook::create(&workspace.join("笔记本"), "笔记本").unwrap();
        existing.create_note("共享.md", "本地版").unwrap();

        let mut choices = HashMap::new();
        choices.insert("共享.md".to_string(), ConflictPolicy::UseImported);
        let outcome = import_zip(
            &workspace,
            &zip_path,
            &ImportOptions {
                mode: ImportMode::Merge,
                policy: ConflictPolicy::KeepExisting,
                choices,
                rename_to: None,
            },
        )
        .unwrap();
        assert_eq!(outcome.imported, 1);
        assert_eq!(
            fs::read_to_string(existing.note_path("共享.md").unwrap()).unwrap(),
            "导入版"
        );

        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(target_root);
    }

    #[test]
    fn overwrite_moves_existing_notebook_to_workspace_trash() {
        let source_root = temp_dir("overwrite-src");
        let source = Notebook::create(&source_root.join("source"), "我的笔记本").unwrap();
        source.create_note("新笔记.md", "新").unwrap();
        let zip_path = source_root.join("source.zip");
        export_zip(&source, &zip_path).unwrap();

        let target_root = temp_dir("overwrite-dst");
        let workspace = target_root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let old = Notebook::create(&workspace.join("我的笔记本"), "我的笔记本").unwrap();
        old.create_note("旧笔记.md", "旧").unwrap();

        let outcome = import_zip(
            &workspace,
            &zip_path,
            &ImportOptions {
                mode: ImportMode::Overwrite,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!outcome.notebook.note_path("旧笔记.md").unwrap().exists());
        assert!(outcome.notebook.note_path("新笔记.md").unwrap().exists());
        assert_eq!(outcome.notebook.marker.id, source.marker.id);
        let replaced = outcome.replaced.expect("old notebook should be trashed");
        assert!(replaced.join("旧笔记.md").is_file());
        assert!(replaced.starts_with(workspace.join(META_DIR).join(WORKSPACE_TRASH_DIR)));

        let scan = crate::Workspace::new(&workspace).scan();
        assert_eq!(scan.notebooks.len(), 1);

        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(target_root);
    }

    #[test]
    fn rename_imports_with_new_folder_and_id() {
        let source_root = temp_dir("rename-src");
        let source = Notebook::create(&source_root.join("source"), "我的笔记本").unwrap();
        source.create_note("笔记.md", "内容").unwrap();
        let zip_path = source_root.join("source.zip");
        export_zip(&source, &zip_path).unwrap();

        let target_root = temp_dir("rename-dst");
        let workspace = target_root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let old = Notebook::create(&workspace.join("我的笔记本"), "我的笔记本").unwrap();
        old.create_note("笔记.md", "旧").unwrap();

        let outcome = import_zip(
            &workspace,
            &zip_path,
            &ImportOptions {
                mode: ImportMode::Rename,
                ..Default::default()
            },
        )
        .unwrap();
        assert_ne!(outcome.notebook.marker.id, source.marker.id);
        assert_eq!(
            outcome.notebook.root.file_name().unwrap().to_string_lossy(),
            "我的笔记本 (2)"
        );
        assert!(workspace.join("我的笔记本").join("笔记.md").is_file());
        assert_eq!(
            fs::read_to_string(outcome.notebook.note_path("笔记.md").unwrap()).unwrap(),
            "内容"
        );

        let outcome = import_zip(
            &workspace,
            &zip_path,
            &ImportOptions {
                mode: ImportMode::Rename,
                rename_to: Some("导入副本".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            outcome.notebook.root.file_name().unwrap().to_string_lossy(),
            "导入副本"
        );

        let _ = fs::remove_dir_all(source_root);
        let _ = fs::remove_dir_all(target_root);
    }

    #[test]
    fn imports_legacy_json_metadata_as_toml() {
        let root = temp_dir("legacy");
        let marker = crate::notebook::Marker::new("旧笔记本");
        let files = vec![
            (
                format!("{META_DIR}/{NOTEBOOK_FILE_LEGACY}"),
                serde_json::to_vec_pretty(&marker).unwrap(),
                None,
            ),
            (
                format!("{META_DIR}/{BOOKMARKS_FILE_LEGACY}"),
                r#"{"format":"md-work/bookmarks@1","entries":[{"rel_path":"笔记.md","title":"笔记","heading":null,"line":1,"created":1}]}"#
                    .as_bytes()
                    .to_vec(),
                None,
            ),
            (
                format!("{META_DIR}/{ENCODING_FILE_LEGACY}"),
                br#"{"format":"md-work/encoding@1","files":{}}"#.to_vec(),
                None,
            ),
            note_file("笔记.md", "内容"),
        ];
        let manifest = manifest_for(&files, &marker);
        let zip_path = root.join("legacy.zip");
        write_test_zip(&zip_path, &files, Some(&manifest));

        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let outcome = import_zip(&workspace, &zip_path, &ImportOptions::default()).unwrap();
        let meta = outcome.notebook.meta_dir();
        assert!(meta.join(NOTEBOOK_FILE).is_file());
        assert!(!meta.join(NOTEBOOK_FILE_LEGACY).exists());
        assert!(meta.join(BOOKMARKS_FILE).is_file());
        assert!(!meta.join(BOOKMARKS_FILE_LEGACY).exists());
        assert!(meta.join(ENCODING_FILE).is_file());
        assert!(!meta.join(ENCODING_FILE_LEGACY).exists());
        assert_eq!(Bookmarks::load(&outcome.notebook).unwrap().entries.len(), 1);
        assert_eq!(outcome.notebook.marker.id, marker.id);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_path_traversal_and_reserved_paths() {
        let root = temp_dir("evil");
        for bad in [
            "../evil.md",
            "/evil.md",
            "C:/evil.md",
            "assets\\..\\..\\evil.md",
        ] {
            let marker = crate::notebook::Marker::new("坏本");
            let files = vec![
                meta_file(&marker),
                (bad.to_string(), b"evil".to_vec(), None),
            ];
            let manifest = manifest_for(&files, &marker);
            let zip_path = root.join(format!("bad-{}.zip", Uuid::new_v4()));
            write_test_zip(&zip_path, &files, Some(&manifest));
            let err = inspect(&zip_path).expect_err(bad);
            assert!(matches!(err, Error::InvalidArchive(_)), "{bad}: {err:?}");
        }

        let marker = crate::notebook::Marker::new("坏本");
        let files = vec![
            meta_file(&marker),
            (
                format!("{META_DIR}/trash/item/payload"),
                b"x".to_vec(),
                None,
            ),
        ];
        let manifest = manifest_for(&files, &marker);
        let zip_path = root.join("trash.zip");
        write_test_zip(&zip_path, &files, Some(&manifest));
        assert!(inspect(&zip_path).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_symlink_entries() {
        let root = temp_dir("symlink");
        let marker = crate::notebook::Marker::new("坏本");
        let files = vec![
            meta_file(&marker),
            (
                "assets/link".to_string(),
                b"target".to_vec(),
                Some(0o120777),
            ),
        ];
        let manifest = manifest_for(&files, &marker);
        let zip_path = root.join("symlink.zip");
        write_test_zip(&zip_path, &files, Some(&manifest));
        assert!(matches!(inspect(&zip_path), Err(Error::InvalidArchive(_))));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_manifest_mismatch_and_bad_hash() {
        let root = temp_dir("mismatch");
        let marker = crate::notebook::Marker::new("坏本");

        let files = vec![meta_file(&marker), note_file("a.md", "a")];
        let mut manifest = manifest_for(&files, &marker);
        manifest.files.retain(|file| file.path != "a.md");
        let zip_path = root.join("extra.zip");
        write_test_zip(&zip_path, &files, Some(&manifest));
        assert!(matches!(inspect(&zip_path), Err(Error::InvalidArchive(_))));

        let files = vec![meta_file(&marker), note_file("a.md", "a")];
        let mut manifest = manifest_for(&files, &marker);
        manifest.files[1].sha256 = "0".repeat(64);
        let zip_path = root.join("hash.zip");
        write_test_zip(&zip_path, &files, Some(&manifest));
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let err = import_zip(&workspace, &zip_path, &ImportOptions::default()).unwrap_err();
        assert!(matches!(err, Error::InvalidArchive(_)));
        assert!(
            !workspace.join(STAGING_DIR).exists()
                || fs::read_dir(workspace.join(STAGING_DIR))
                    .unwrap()
                    .next()
                    .is_none()
        );
        assert!(!workspace.join("坏本").exists());

        let files = vec![note_file("a.md", "a")];
        let manifest = manifest_for(&files, &marker);
        let zip_path = root.join("nometa.zip");
        write_test_zip(&zip_path, &files, Some(&manifest));
        assert!(matches!(inspect(&zip_path), Err(Error::InvalidArchive(_))));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_duplicate_manifest_entries() {
        let root = temp_dir("dup");
        let marker = crate::notebook::Marker::new("坏本");
        let files = vec![meta_file(&marker), note_file("a.md", "a")];
        let mut manifest = manifest_for(&files, &marker);
        let duplicate = manifest.files[1].clone();
        manifest.files.push(duplicate);
        let zip_path = root.join("dup.zip");
        write_test_zip(&zip_path, &files, Some(&manifest));
        assert!(matches!(inspect(&zip_path), Err(Error::InvalidArchive(_))));
        let _ = fs::remove_dir_all(root);
    }
}
