use super::util::*;
use super::*;

impl AppShell {
    /// Rebuild the unified Notes-panel virtual list from per-notebook trees.
    pub(super) fn rebuild_sidebar_rows(&mut self) {
        let mut rows = Vec::new();
        for entry in &self.notebooks {
            let note_count = entry
                .tree
                .iter()
                .filter(|item| item.kind == EntryKind::Note)
                .count();
            rows.push(SidebarRow::Notebook {
                id: entry.notebook.marker.id,
                name: entry.notebook.display_name().to_string(),
                note_count,
                expanded: entry.root_expanded,
            });
            if !entry.root_expanded {
                continue;
            }
            for row in &entry.rows {
                let mut row = row.clone();
                row.depth = row.depth.saturating_add(1);
                rows.push(SidebarRow::Entry {
                    notebook_id: entry.notebook.marker.id,
                    row,
                });
            }
        }
        self.sidebar_rows = rows;
        self.sidebar_row_menu = None;
    }

    /// Expand the active notebook root so its contents are visible by default.
    pub(super) fn ensure_active_root_expanded(&mut self) {
        let Some(active) = self.active_notebook else {
            return;
        };
        if let Some(entry) = self
            .notebooks
            .iter_mut()
            .find(|entry| entry.notebook.marker.id == active)
        {
            entry.root_expanded = true;
        }
    }

    pub(super) fn toggle_notebook_root(&mut self, notebook_id: Uuid, cx: &mut Context<Self>) {
        let Some(entry) = self
            .notebooks
            .iter_mut()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            return;
        };
        entry.root_expanded = !entry.root_expanded;
        self.rebuild_sidebar_rows();
        cx.notify();
    }

    pub(super) fn toggle_folder(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .notebooks
            .iter_mut()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            return;
        };
        if !entry.expanded.remove(rel_path) {
            entry.expanded.insert(rel_path.to_string());
        }
        let expanded = entry.expanded.clone();
        entry.rows = build_rows(&entry.tree, &expanded);
        self.rebuild_sidebar_rows();
        cx.notify();
    }

    pub(super) fn reveal_active(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.active_doc.and_then(|index| self.docs.get(index)) else {
            return;
        };
        let (notebook_id, rel_path) = {
            let doc = active.read(cx);
            let Some(notebook_id) = doc.notebook_id else {
                return;
            };
            (notebook_id, doc.rel_path.clone())
        };

        self.expand_to(notebook_id, &rel_path);
        let Some(row) = self.sidebar_rows.iter().position(|row| match row {
            SidebarRow::Entry {
                notebook_id: id,
                row,
            } => *id == notebook_id && row.rel_path == rel_path,
            SidebarRow::Notebook { .. } => false,
        }) else {
            return;
        };
        self.tree_scroll.scroll_to_item(row, ScrollStrategy::Center);
        self.flash_rel = Some((notebook_id, rel_path));
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.flash_rel = None;
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn new_notebook(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let mut name = "新笔记本".to_string();
        let mut counter = 2;
        while self
            .notebooks
            .iter()
            .any(|entry| entry.notebook.display_name() == name)
        {
            name = format!("新笔记本 {counter}");
            counter += 1;
        }
        let default_name = name.clone();
        let receiver = disk::submit(move || workspace.create_notebook(&name));
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(notebook) => {
                        let id = notebook.marker.id;
                        this.active_notebook = Some(id);
                        this.reload(cx);
                        this.persist_session(cx);
                        this.begin_notebook_rename(id, &default_name, window, cx);
                    }
                    Err(err) => this.notify_error(format!("新建笔记本失败：{err}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn begin_notebook_rename(
        &mut self,
        notebook_id: Uuid,
        current_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // No clean_on_escape: Escape must cancel the rename, not clear the field.
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(current_name.to_string()));
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_notebook_rename(cx),
            _ => {}
        })
        .detach();
        self.renaming = None;
        self.renaming_notebook = Some((notebook_id, input));
        cx.notify();
    }

    pub(super) fn cancel_notebook_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming_notebook.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn commit_notebook_rename(&mut self, cx: &mut Context<Self>) {
        let Some((notebook_id, input)) = self.renaming_notebook.take() else {
            return;
        };
        let value = input.read(cx).value().trim().to_string();
        if value.is_empty() {
            cx.notify();
            return;
        }
        let Some(entry) = self
            .notebooks
            .iter_mut()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            cx.notify();
            return;
        };
        if entry.notebook.display_name() == value {
            cx.notify();
            return;
        }
        let notebook = entry.notebook.clone();
        let receiver = disk::submit(move || -> mdwork_core::Result<Notebook> {
            let mut nb = notebook;
            nb.rename_display_name(&value)?;
            Ok(nb)
        });
        let workspace = self.workspace.clone();
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(updated) => {
                        if let Some(entry) = this
                            .notebooks
                            .iter_mut()
                            .find(|e| e.notebook.marker.id == notebook_id)
                        {
                            entry.notebook = updated.clone();
                        }
                        this.rebuild_sidebar_rows();
                        if let Some(ws) = &workspace {
                            let notebooks: Vec<Notebook> =
                                this.notebooks.iter().map(|e| e.notebook.clone()).collect();
                            let ws = ws.clone();
                            disk::enqueue(move || {
                                if let Err(err) = ws.save_registry(&notebooks) {
                                    tracing::warn!(error = %err, "保存工作区注册表失败");
                                }
                            });
                        }
                        this.persist_session(cx);
                    }
                    Err(err) => this.notify_error(format!("重命名笔记本失败：{err}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(notebook_id) = self.active_notebook else {
            self.notify_warning(t!("editor.no_notebook").to_string(), cx);
            return;
        };
        self.new_folder_in(notebook_id, None, window, cx);
    }

    pub(super) fn new_folder_in(
        &mut self,
        notebook_id: Uuid,
        parent: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let target = notebook.clone();
        let receiver = disk::submit(move || -> mdwork_core::Result<String> {
            let mut name = "新文件夹".to_string();
            let mut counter = 2;
            loop {
                let rel = match &parent {
                    Some(parent) => format!("{parent}/{name}"),
                    None => name.clone(),
                };
                let exists = target
                    .normalize_rel(&rel)
                    .and_then(|normalized| target.to_path(&normalized))
                    .map(|path| path.exists())
                    .unwrap_or(true);
                if !exists {
                    return target.create_folder(&rel);
                }
                name = format!("新文件夹 {counter}");
                counter += 1;
            }
        });
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(rel_path) => {
                        this.reload_entries(cx);
                        this.persist_session(cx);
                        this.expand_to(notebook_id, &rel_path);
                        this.begin_rename(notebook_id, &rel_path, window, cx);
                    }
                    Err(err) => this.notify_error(format!("新建文件夹失败：{err}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn begin_move(
        &mut self,
        notebook_id: Uuid,
        from: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(from.to_string())
                .clean_on_escape()
        });
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        cx.subscribe(&input, move |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_prompt(cx);
            }
        })
        .detach();
        self.prompt = Some(PromptDialog {
            kind: PromptKind::MoveEntry {
                notebook_id,
                from: from.to_string(),
            },
            title: t!("prompt.move").to_string(),
            hint: t!("prompt.move_hint").to_string(),
            input,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn begin_rename(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .and_then(|entry| {
                entry
                    .rows
                    .iter()
                    .find(|row| row.rel_path == rel_path)
                    .map(|row| row.name.clone())
            })
        else {
            return;
        };
        // No clean_on_escape: Escape must cancel the rename, not clear the field.
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_rename(cx),
            _ => {}
        })
        .detach();
        self.renaming_notebook = None;
        self.renaming = Some(Renaming {
            notebook_id,
            rel_path: rel_path.to_string(),
            input,
        });
        cx.notify();
    }

    pub(super) fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let value = renaming.input.read(cx).value().trim().to_string();
        if value.is_empty() {
            cx.notify();
            return;
        }
        let target = match renaming.rel_path.rsplit_once('/') {
            Some((parent, _)) => format!("{parent}/{value}"),
            None => value,
        };
        if target.eq_ignore_ascii_case(&renaming.rel_path) {
            cx.notify();
            return;
        }
        self.begin_move_entry(renaming.notebook_id, renaming.rel_path.clone(), target, cx);
        cx.notify();
    }

    /// Renames or moves an entry on the writer thread. Open buffers are held
    /// so a mid-rename save cannot recreate the old path.
    pub(super) fn begin_move_entry(
        &mut self,
        notebook_id: Uuid,
        from: String,
        to: String,
        cx: &mut Context<Self>,
    ) {
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let affected: Vec<EntityId> = self
            .docs
            .iter()
            .filter(|doc| {
                let doc = doc.read(cx);
                doc.notebook_id == Some(notebook_id) && path_under(&doc.rel_path, &from)
            })
            .map(|doc| doc.entity_id())
            .collect();
        for doc in &self.docs {
            if affected.contains(&doc.entity_id()) {
                doc.update(cx, |doc, _cx| doc.hold_saves());
            }
        }
        let submitted_from = from.clone();
        let receiver = disk::submit(move || notebook.rename_entry(&submitted_from, &to));
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |this, cx| {
                for id in &affected {
                    if let Some(doc) = this.docs.iter().find(|doc| doc.entity_id() == *id).cloned()
                    {
                        doc.update(cx, |doc, cx| doc.release_saves(cx));
                    }
                }
                match result {
                    Ok(Ok(new_rel)) => {
                        this.update_docs_after_rename(notebook_id, &from, &new_rel, cx);
                        if let Some(entry) = this
                            .notebooks
                            .iter_mut()
                            .find(|entry| entry.notebook.marker.id == notebook_id)
                        {
                            let from_prefix = format!("{from}/");
                            let expanded: HashSet<String> = entry
                                .expanded
                                .drain()
                                .map(|path| {
                                    if path == from {
                                        new_rel.clone()
                                    } else if let Some(rest) = path.strip_prefix(&from_prefix) {
                                        format!("{new_rel}/{rest}")
                                    } else {
                                        path
                                    }
                                })
                                .collect();
                            entry.expanded = expanded;
                        }
                        if this.bookmarks_notebook == Some(notebook_id) {
                            this.bookmarks.rekey(&from, &new_rel);
                            this.save_bookmarks(cx);
                        }
                        this.reload_entries(cx);
                        this.persist_session(cx);
                        this.reveal_active(cx);
                    }
                    Ok(Err(err)) => {
                        this.notify_error(
                            t!("tree.rename_failed", error = err.to_string()).to_string(),
                            cx,
                        );
                    }
                    Err(_) => {
                        this.notify_error(
                            t!("tree.rename_failed", error = "写入任务已停止").to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn update_docs_after_rename(
        &mut self,
        notebook_id: Uuid,
        from: &str,
        to: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let from_prefix = format!("{from}/");
        for doc in &self.docs {
            let rel_path = {
                let doc = doc.read(cx);
                if doc.notebook_id != Some(notebook_id) {
                    continue;
                }
                doc.rel_path.clone()
            };
            let new_rel = if rel_path == from {
                to.to_string()
            } else if let Some(rest) = rel_path.strip_prefix(&from_prefix) {
                format!("{to}/{rest}")
            } else {
                continue;
            };
            let Ok(path) = notebook.to_path(&new_rel) else {
                continue;
            };
            doc.update(cx, |doc, cx| {
                doc.rebase(new_rel, path);
                cx.notify();
            });
        }
    }

    pub(super) fn delete_entry(
        &mut self,
        notebook_id: Uuid,
        rel_path: String,
        cx: &mut Context<Self>,
    ) {
        if self.pending_delete.is_some() {
            self.notify_warning(t!("tree.delete_busy").to_string(), cx);
            return;
        }
        let affected: Vec<Entity<Doc>> = self
            .docs
            .iter()
            .filter(|doc| {
                let doc = doc.read(cx);
                doc.notebook_id == Some(notebook_id) && path_under(&doc.rel_path, &rel_path)
            })
            .cloned()
            .collect();
        self.pending_delete = Some((notebook_id, rel_path));
        for doc in affected {
            if doc.read(cx).dirty {
                self.pending_close.insert(doc.entity_id());
                doc.update(cx, |doc, cx| doc.save_now(cx));
            } else if let Some(index) = self
                .docs
                .iter()
                .position(|open| open.entity_id() == doc.entity_id())
            {
                self.drop_doc_at(index);
            }
        }
        self.persist_session(cx);
        self.try_complete_delete(cx);
        cx.notify();
    }

    pub(super) fn ensure_trash(&mut self, cx: &mut Context<Self>) {
        let target = self.active_notebook;
        if self.trash_notebook == target {
            return;
        }
        self.trash_notebook = target;
        self.trash.clear();
        self.trash_generation += 1;
        let generation = self.trash_generation;
        let Some(target) = target else {
            cx.notify();
            return;
        };
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == target)
            .map(|entry| entry.notebook.clone())
        else {
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let list = cx
                .background_executor()
                .spawn(async move { notebook.list_trash().unwrap_or_default() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.trash_generation != generation || this.trash_notebook != Some(target) {
                    return;
                }
                this.trash = list;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn restore_trash(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let Some(notebook) = self
            .active_notebook
            .and_then(|notebook_id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == notebook_id)
            })
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let receiver = disk::submit(move || notebook.restore_trash_item(&id));
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(rel_path) => {
                        this.trash.retain(|item| item.id != id);
                        this.reload_entries(cx);
                        this.persist_session(cx);
                        this.notify_success(format!("{}：{rel_path}", t!("trash.restored")), cx);
                    }
                    Err(err) => {
                        this.notify_error(
                            t!("trash.failed", error = err.to_string()).to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn jump_to(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.docs.iter().position(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id == Some(notebook_id) && doc.rel_path == rel_path
        });
        if let Some(index) = existing {
            self.active_doc = Some(index);
            self.active_notebook = Some(notebook_id);
            self.persist_session(cx);
            self.reveal_active(cx);
            let doc = self.docs[index].clone();
            doc.update(cx, |doc, cx| {
                doc.set_cursor(line.saturating_sub(1) as u32, 0, window, cx)
            });
            cx.notify();
            return;
        }

        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        if let Ok(path) = notebook.note_path(rel_path) {
            self.pending_jump = Some((path, line.min(u32::MAX as usize) as u32));
        }
        self.active_notebook = Some(notebook_id);
        self.open_note(&notebook, rel_path, window, cx);
    }
}
