use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::base::input::Position;
use gpui_kit::component::input::{EditorState, InputEvent};
use gpui_kit::*;
use mdwork_core::atomic::write_atomic;
use mdwork_core::doc::{self, DecodedDoc, Eol, TextEncoding};
use mdwork_core::{Notebook, Result};
use uuid::Uuid;

use crate::dirty::DirtyRegistry;

const AUTOSAVE_DELAY: Duration = Duration::from_millis(800);

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

pub struct Doc {
    pub notebook_id: Option<Uuid>,
    pub notebook_name: String,
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
}

impl Doc {
    pub fn new(
        origin: DocOrigin,
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
        })
        .detach();

        let (notebook_id, notebook_name) = match origin {
            DocOrigin::Notebook { id, name } => (Some(id), name),
            DocOrigin::External => (None, String::new()),
        };

        Self {
            notebook_id,
            notebook_name,
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
    }

    pub fn save(&mut self, cx: &mut Context<Self>) -> Result<()> {
        let text = self.editor.read(cx).value().to_string();
        let bytes = doc::encode_text(&text, self.encoding, self.bom, self.eol)?;
        write_atomic(&self.path, &bytes)?;
        self.base_hash = doc::sha256_hex(&bytes);
        self.dirty = false;
        self.error = None;
        self.conflict = None;
        self.sync_dirty_marker();
        cx.notify();
        Ok(())
    }

    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.save(cx) {
            self.error = Some(err.to_string());
            cx.notify();
        }
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
        self.error = None;
        self.conflict = None;
        self.sync_dirty_marker();
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
}

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
    let path = notebook.note_path(rel_path)?;
    let bytes = std::fs::read(&path)?;
    let hash = doc::sha256_hex(&bytes);
    Ok((path, doc::decode(&bytes), hash))
}

pub fn read_external_doc(path: &Path) -> Result<(DecodedDoc, String)> {
    let bytes = std::fs::read(path)?;
    let hash = doc::sha256_hex(&bytes);
    Ok((doc::decode(&bytes), hash))
}
