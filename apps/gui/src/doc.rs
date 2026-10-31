use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::base::input::Position;
use gpui_kit::component::input::{EditorState, InputEvent};
use gpui_kit::*;
use mdwork_core::atomic::write_atomic;
use mdwork_core::doc::{self, DecodedDoc, Eol, TextEncoding};
use mdwork_core::notebook;
use mdwork_core::{Notebook, Result};
use uuid::Uuid;

use crate::dirty::DirtyRegistry;

const AUTOSAVE_DELAY: Duration = Duration::from_millis(800);
const SNAPSHOT_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub enum DocOrigin {
    Notebook { id: Uuid, name: String },
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskConflict {
    /// The file changed on disk while the buffer had unsaved edits.
    Modified,
    /// The file disappeared from disk.
    Removed,
}

/// Outcome of an asynchronous save, so the shell can close tabs safely and
/// surface failures without the editor blocking on the disk.
#[derive(Debug, Clone)]
pub enum DocEvent {
    Saved { clean: bool },
    SaveFailed { error: String },
}

pub struct Doc {
    pub notebook_id: Option<Uuid>,
    pub notebook_name: String,
    pub notebook_root: Option<PathBuf>,
    pub rel_path: String,
    pub path: PathBuf,
    pub encoding: TextEncoding,
    pub bom: bool,
    pub eol: Eol,
    pub editor: Entity<EditorState>,
    pub dirty: bool,
    pub error: Option<String>,
    pub conflict: Option<DiskConflict>,
    pub base_hash: String,
    pub autosave: bool,
    pub registry: DirtyRegistry,
    revision: u64,
    save_scheduled: bool,
    snapshot_pending: bool,
    saving: bool,
    save_again: bool,
    save_hold: bool,
    /// Highest revision known to be on disk, shared with the writer thread so
    /// recovery snapshots queued behind a save do not resurrect stale text.
    saved_revision: Arc<AtomicU64>,
}

impl Doc {
    pub fn new(
        origin: DocOrigin,
        notebook_root: Option<PathBuf>,
        rel_path: String,
        path: PathBuf,
        decoded: DecodedDoc,
        base_hash: String,
        autosave: bool,
        registry: DirtyRegistry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .line_number(true)
                .soft_wrap(true)
                .searchable(true)
                .replaceable(true)
                .default_value(decoded.text.clone())
        });

        cx.subscribe(&editor, |this, _, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            this.dirty = true;
            this.revision += 1;
            this.sync_dirty_marker();
            cx.notify();
            this.schedule_autosave(cx);
            this.schedule_snapshot(cx);
        })
        .detach();

        let (notebook_id, notebook_name) = match origin {
            DocOrigin::Notebook { id, name } => (Some(id), name),
            DocOrigin::External => (None, String::new()),
        };

        Self {
            notebook_id,
            notebook_name,
            notebook_root,
            rel_path,
            path,
            encoding: decoded.encoding,
            bom: decoded.bom,
            eol: decoded.eol,
            editor,
            dirty: false,
            error: None,
            conflict: None,
            base_hash,
            autosave,
            registry,
            revision: 0,
            save_scheduled: false,
            snapshot_pending: false,
            saving: false,
            save_again: false,
            save_hold: false,
            saved_revision: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Mirrors the dirty flag into the marker file the MCP server reads.
    fn sync_dirty_marker(&self) {
        let Some(notebook_id) = self.notebook_id else {
            return;
        };
        if self.dirty {
            self.registry.mark(notebook_id, &self.rel_path);
        } else {
            self.registry.clear(notebook_id, &self.rel_path);
        }
    }

    fn persist_encoding(&self) {
        let Some(root) = self.notebook_root.clone() else {
            return;
        };
        let hint = doc::EncodingHint {
            encoding: self.encoding,
            bom: self.bom,
            eol: self.eol,
        };
        let rel_path = self.rel_path.clone();
        crate::disk::enqueue(move || {
            if let Err(err) = notebook::store_encoding_hint(&root, &rel_path, hint) {
                tracing::warn!(
                    path = %rel_path,
                    error = %err,
                    "写入编码记录失败"
                );
            }
        });
    }

    /// Bumps whenever the buffer text changes, so derived state such as the
    /// preview IR can be cached without hashing the whole note every frame.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn text(&self, cx: &App) -> SharedString {
        self.editor.read(cx).value()
    }

    pub fn cursor(&self, cx: &App) -> (u32, u32) {
        let position = self.editor.read(cx).cursor_position();
        (position.line, position.character)
    }

    pub fn set_cursor(
        &mut self,
        line: u32,
        column: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.set_cursor_position(Position::new(line, column), window, cx)
        });
    }

    /// Inserts text at the caret (replacing a selection) and marks the doc
    /// dirty through the editor's change event.
    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.replace(text.to_string(), window, cx);
        });
    }

    pub fn title(&self) -> String {
        Path::new(&self.rel_path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.rel_path.clone())
    }

    /// Points the buffer at a renamed/moved file without touching its contents.
    /// The caller holds saves while the rename runs, then releases them.
    pub fn rebase(&mut self, rel_path: String, path: PathBuf) {
        let previous = std::mem::replace(&mut self.rel_path, rel_path);
        self.path = path;
        self.conflict = None;
        if self.dirty {
            if let Some(notebook_id) = self.notebook_id {
                self.registry.clear(notebook_id, &previous);
                self.registry.mark(notebook_id, &self.rel_path);
            }
        }
        if let Some(root) = self.notebook_root.clone() {
            let rel_path = previous.clone();
            crate::disk::enqueue(move || {
                let _ = notebook::remove_encoding_hint(&root, &rel_path);
            });
        }
        self.persist_encoding();
    }

    /// Prevents autosaves while the file is being renamed on disk.
    pub fn hold_saves(&mut self) {
        self.save_hold = true;
    }

    /// Re-enables saving and flushes anything typed while the hold was active.
    pub fn release_saves(&mut self, cx: &mut Context<Self>) {
        self.save_hold = false;
        if self.save_again {
            self.save_again = false;
            self.request_save(cx);
        }
    }

    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        self.request_save(cx);
        cx.notify();
    }

    fn request_save(&mut self, cx: &mut Context<Self>) {
        if self.save_hold {
            self.save_again = true;
            return;
        }
        if self.saving {
            self.save_again = true;
            return;
        }
        let text = self.editor.read(cx).value().to_string();
        let revision = self.revision;
        let path = self.path.clone();
        let encoding = self.encoding;
        let bom = self.bom;
        let eol = self.eol;
        let root = self.notebook_root.clone();
        let rel_path = self.rel_path.clone();
        let name = self.notebook_name.clone();
        let autosave = self.autosave;
        let saved_revision = self.saved_revision.clone();
        self.saving = true;
        let receiver = crate::disk::submit(move || {
            let result = (|| -> Result<(usize, String)> {
                let bytes = doc::encode_text(&text, encoding, bom, eol)?;
                write_atomic(&path, &bytes)?;
                let hash = doc::sha256_hex(&bytes);
                if let Some(root) = &root {
                    let hint = doc::EncodingHint { encoding, bom, eol };
                    if let Err(err) = notebook::store_encoding_hint(root, &rel_path, hint) {
                        tracing::warn!(
                            path = %rel_path,
                            error = %err,
                            "写入编码记录失败"
                        );
                    }
                }
                let _ = mdwork_core::recovery::remove(&mdwork_core::paths::recovery_dir(), &path);
                saved_revision.fetch_max(revision, Ordering::Release);
                tracing::info!(
                    notebook = %name,
                    path = %rel_path,
                    bytes = bytes.len(),
                    encoding = encoding.label(),
                    autosave,
                    "保存笔记"
                );
                Ok((bytes.len(), hash))
            })();
            (revision, result)
        });
        cx.spawn(async move |this, cx| {
            if let Ok((saved, result)) = receiver.await {
                let _ = this.update(cx, |doc, cx| doc.finish_save(saved, result, cx));
            }
        })
        .detach();
    }

    fn finish_save(
        &mut self,
        revision: u64,
        result: Result<(usize, String)>,
        cx: &mut Context<Self>,
    ) {
        self.saving = false;
        match result {
            Ok((_bytes, hash)) => {
                self.base_hash = hash;
                self.error = None;
                self.conflict = None;
                let clean = self.revision == revision;
                if clean {
                    self.dirty = false;
                    self.sync_dirty_marker();
                }
                cx.emit(DocEvent::Saved { clean });
            }
            Err(err) => {
                let error = err.to_string();
                tracing::error!(
                    notebook = %self.notebook_name,
                    path = %self.rel_path,
                    error = %error,
                    "保存笔记失败"
                );
                self.error = Some(error.clone());
                cx.emit(DocEvent::SaveFailed { error });
            }
        }
        if self.save_again {
            self.save_again = false;
            self.request_save(cx);
        }
        cx.notify();
    }

    /// Replaces the buffer with the file contents. Undo history is dropped and
    /// no change event is emitted, so the doc stays clean.
    pub fn apply_disk(
        &mut self,
        decoded: DecodedDoc,
        hash: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.encoding = decoded.encoding;
        self.bom = decoded.bom;
        self.eol = decoded.eol;
        let text = decoded.text;
        self.editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
        self.base_hash = hash;
        self.dirty = false;
        self.revision += 1;
        self.saved_revision
            .fetch_max(self.revision, Ordering::Release);
        self.error = None;
        self.conflict = None;
        self.sync_dirty_marker();
        self.persist_encoding();
        let path = self.path.clone();
        crate::disk::enqueue(move || {
            let _ = mdwork_core::recovery::remove(&mdwork_core::paths::recovery_dir(), &path);
        });
        cx.notify();
    }

    pub fn mark_conflict(&mut self, conflict: DiskConflict, cx: &mut Context<Self>) {
        if self.conflict != Some(conflict) {
            self.conflict = Some(conflict);
            cx.notify();
        }
    }

    pub fn keep_mine(&mut self, cx: &mut Context<Self>) {
        self.conflict = None;
        cx.notify();
    }

    /// Re-encodes the buffer with a different encoding/EOL and remembers the
    /// choice; the write itself happens on the disk thread.
    pub fn save_as_encoding(
        &mut self,
        encoding: TextEncoding,
        bom: bool,
        eol: Eol,
        cx: &mut Context<Self>,
    ) {
        self.encoding = encoding;
        self.bom = bom;
        self.eol = eol;
        tracing::info!(
            path = %self.rel_path,
            encoding = encoding.label(),
            eol = eol.label(),
            bom,
            "另存为其他编码"
        );
        self.request_save(cx);
        cx.notify();
    }

    /// Swaps the whole buffer for `text`, e.g. when restoring a crash snapshot.
    /// The doc is left dirty so the user reviews and saves it explicitly.
    pub fn replace_all(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
        self.dirty = true;
        self.revision += 1;
        self.error = None;
        self.conflict = None;
        self.sync_dirty_marker();
        cx.notify();
    }

    pub fn set_autosave(&mut self, autosave: bool, cx: &mut Context<Self>) {
        if self.autosave == autosave {
            return;
        }
        self.autosave = autosave;
        if autosave && self.dirty {
            self.schedule_autosave(cx);
        }
        cx.notify();
    }

    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        if !self.autosave || self.save_scheduled {
            return;
        }
        self.save_scheduled = true;
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            let _ = weak.update(cx, |doc, cx| {
                doc.save_scheduled = false;
                if doc.dirty {
                    doc.save_now(cx);
                }
            });
        })
        .detach();
    }

    /// Writes a recovery snapshot after a quiet interval so a crash cannot
    /// lose more than [`SNAPSHOT_DELAY`] of typing.
    fn schedule_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.snapshot_pending {
            return;
        }
        self.snapshot_pending = true;
        let weak = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            cx.background_executor().timer(SNAPSHOT_DELAY).await;
            let _ = weak.update(cx, |doc, cx| {
                doc.snapshot_pending = false;
                if !doc.dirty {
                    return;
                }
                let text = doc.editor.read(cx).value().to_string();
                let revision = doc.revision;
                let path = doc.path.clone();
                let rel_path = doc.rel_path.clone();
                let root = doc.notebook_root.clone();
                let name = (!doc.notebook_name.is_empty()).then(|| doc.notebook_name.clone());
                let base_hash = doc.base_hash.clone();
                let saved_revision = doc.saved_revision.clone();
                crate::disk::enqueue(move || {
                    if saved_revision.load(Ordering::Acquire) >= revision {
                        return;
                    }
                    if let Err(err) = mdwork_core::recovery::save(
                        &mdwork_core::paths::recovery_dir(),
                        &path,
                        &rel_path,
                        root.as_deref(),
                        name.as_deref(),
                        Some(&base_hash),
                        &text,
                    ) {
                        tracing::warn!(
                            path = %rel_path,
                            error = %err,
                            "写入崩溃恢复快照失败"
                        );
                    }
                });
            });
        })
        .detach();
    }
}

impl EventEmitter<DocEvent> for Doc {}

impl Drop for Doc {
    fn drop(&mut self) {
        if self.dirty {
            if let Some(notebook_id) = self.notebook_id {
                self.registry.clear(notebook_id, &self.rel_path);
            }
        }
    }
}

pub fn read_notebook_doc(
    notebook: &Notebook,
    rel_path: &str,
) -> Result<(PathBuf, DecodedDoc, String)> {
    let result = (|| {
        let path = notebook.note_path(rel_path)?;
        let bytes = std::fs::read(&path)?;
        let hash = doc::sha256_hex(&bytes);
        Ok((path, bytes, hash))
    })();
    match result {
        Ok((path, bytes, hash)) => {
            let decoded = doc::decode_with_hint(&bytes, notebook.encoding_hint(rel_path));
            tracing::info!(
                notebook = %notebook.display_name(),
                path = %rel_path,
                bytes = bytes.len(),
                encoding = decoded.encoding.label(),
                "打开笔记"
            );
            Ok((path, decoded, hash))
        }
        Err(err) => {
            tracing::warn!(
                notebook = %notebook.display_name(),
                path = %rel_path,
                error = %err,
                "打开笔记失败"
            );
            Err(err)
        }
    }
}

pub fn read_external_doc(path: &Path) -> Result<(DecodedDoc, String)> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "打开外部文件失败");
            return Err(err.into());
        }
    };
    let decoded = doc::decode(&bytes);
    tracing::info!(
        path = %path.display(),
        bytes = bytes.len(),
        encoding = decoded.encoding.label(),
        "打开外部文件"
    );
    let hash = doc::sha256_hex(&bytes);
    Ok((decoded, hash))
}

/// Description of a note the shell wants to show, resolved on a background
/// thread before the editor entity is created.
#[derive(Clone)]
pub enum OpenRequest {
    Notebook {
        notebook: Notebook,
        rel_path: String,
    },
    External {
        path: PathBuf,
    },
}

impl OpenRequest {
    pub fn target_path(&self) -> Option<PathBuf> {
        match self {
            OpenRequest::Notebook { notebook, rel_path } => notebook.note_path(rel_path).ok(),
            OpenRequest::External { path } => Some(path.clone()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            OpenRequest::Notebook { rel_path, .. } => rel_path.clone(),
            OpenRequest::External { path } => path.display().to_string(),
        }
    }

    pub fn read(&self) -> Result<(PathBuf, DecodedDoc, String)> {
        match self {
            OpenRequest::Notebook { notebook, rel_path } => read_notebook_doc(notebook, rel_path),
            OpenRequest::External { path } => {
                let (decoded, hash) = read_external_doc(path)?;
                Ok((path.clone(), decoded, hash))
            }
        }
    }
}
