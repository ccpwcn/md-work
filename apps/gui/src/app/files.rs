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
        let order = entry.order.clone();
        entry.rows = build_rows(&entry.tree, &expanded, &order);
        self.rebuild_sidebar_rows();
        cx.notify();
    }

    /// Expand a folder without collapsing it when already open (used when selecting).
    pub(super) fn ensure_folder_expanded(
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
        if !entry.expanded.insert(rel_path.to_string()) {
            return;
        }
        let expanded = entry.expanded.clone();
        let order = entry.order.clone();
        entry.rows = build_rows(&entry.tree, &expanded, &order);
        self.rebuild_sidebar_rows();
        cx.notify();
    }

    pub(super) fn reveal_active(&mut self, cx: &mut Context<Self>) {
        // Tab strip first: works for notebook notes and external tabs alike.
        // A deferred retry covers session restore / bulk open, when the strip
        // has not been measured yet on the first call.
        self.ensure_active_tab_visible(cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            let _ = this.update(cx, |this, _cx| {
                this.ensure_active_tab_visible(_cx);
            });
        })
        .detach();

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
        let affected: Vec<EntityId> = self
            .docs
            .iter()
            .filter(|doc| doc.read(cx).notebook_id == Some(notebook_id))
            .map(|doc| doc.entity_id())
            .collect();
        for doc in &self.docs {
            if affected.contains(&doc.entity_id()) {
                doc.update(cx, |doc, _cx| doc.hold_saves());
            }
        }
        let receiver = disk::submit(move || -> mdwork_core::Result<Notebook> {
            let mut nb = notebook;
            // Disk folder first, then marker — see Notebook::rename_display_name.
            nb.rename_display_name(&value)?;
            Ok(nb)
        });
        let workspace = self.workspace.clone();
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
                    Ok(Ok(updated)) => {
                        if let Some(entry) = this
                            .notebooks
                            .iter_mut()
                            .find(|e| e.notebook.marker.id == notebook_id)
                        {
                            entry.notebook = updated.clone();
                        }
                        this.retarget_docs_after_notebook_rename(&updated, cx);
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
                    Ok(Err(err)) => this.notify_error(format!("重命名笔记本失败：{err}"), cx),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Keeps open buffers pointed at the notebook after its root folder moved.
    pub(super) fn retarget_docs_after_notebook_rename(
        &mut self,
        notebook: &Notebook,
        cx: &mut Context<Self>,
    ) {
        let notebook_id = notebook.marker.id;
        let name = notebook.display_name().to_string();
        let root = notebook.root.clone();
        for doc in &self.docs {
            let rel_path = {
                let doc = doc.read(cx);
                if doc.notebook_id != Some(notebook_id) {
                    continue;
                }
                doc.rel_path.clone()
            };
            let Ok(path) = notebook.to_path(&rel_path) else {
                continue;
            };
            doc.update(cx, |doc, cx| {
                doc.notebook_name = name.clone();
                doc.notebook_root = Some(root.clone());
                doc.path = path;
                cx.notify();
            });
        }
    }

    pub(super) fn new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(notebook_id) = self.active_notebook else {
            self.notify_warning(t!("editor.no_notebook").to_string(), cx);
            return;
        };
        let (parent, place) = self
            .tree_selection
            .as_ref()
            .filter(|selection| selection.notebook_id() == notebook_id)
            .map(TreeSelection::create_target)
            .unwrap_or((None, SiblingPlace::End));
        self.new_folder_in(notebook_id, parent, place, window, cx);
    }

    pub(super) fn new_folder_in(
        &mut self,
        notebook_id: Uuid,
        parent: Option<String>,
        place: SiblingPlace,
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
        let anchor = match &place {
            SiblingPlace::End => None,
            SiblingPlace::After(rel) => Some(rel.clone()),
        };
        let placed_after = anchor.clone();
        self.begin_create_hold();
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
                    let created = target.create_folder(&rel)?;
                    place_created(&target, &created, anchor.as_deref());
                    return Ok(created);
                }
                name = format!("新文件夹 {counter}");
                counter += 1;
            }
        });
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => {
                    let _ = this.update(cx, |this, cx| this.end_create_hold(cx));
                    return;
                }
            };
            let applied = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(rel_path) => {
                        this.present_created_entry(
                            notebook_id,
                            &rel_path,
                            EntryKind::Folder,
                            placed_after.as_deref(),
                            window,
                            cx,
                        );
                        this.persist_session(cx);
                    }
                    Err(err) => this.notify_error(format!("新建文件夹失败：{err}"), cx),
                }
                this.end_create_hold(cx);
                cx.notify();
            });
            if applied.is_err() {
                let _ = this.update(cx, |this, cx| this.end_create_hold(cx));
            }
        })
        .detach();
    }

    pub(super) fn begin_move(
        &mut self,
        notebook_id: Uuid,
        from: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            return;
        };
        let Some(kind) = entry
            .tree
            .iter()
            .find(|item| item.rel_path == from)
            .map(|item| item.kind)
        else {
            return;
        };
        let mut root_expanded = HashSet::new();
        root_expanded.insert(notebook_id);
        let mut expanded = HashMap::new();
        expanded.insert(notebook_id, entry.expanded.clone());
        self.move_picker = Some(MovePickerDialog {
            source_notebook_id: notebook_id,
            source_rel: from.to_string(),
            source_kind: kind,
            selected: None,
            root_expanded,
            expanded,
            error: None,
        });
        self.prompt = None;
        cx.notify();
    }

    pub(super) fn close_move_picker(&mut self, cx: &mut Context<Self>) {
        if self.move_picker.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn toggle_move_picker_root(&mut self, notebook_id: Uuid, cx: &mut Context<Self>) {
        let Some(picker) = self.move_picker.as_mut() else {
            return;
        };
        if !picker.root_expanded.remove(&notebook_id) {
            picker.root_expanded.insert(notebook_id);
            picker.expanded.entry(notebook_id).or_default();
        }
        cx.notify();
    }

    pub(super) fn toggle_move_picker_folder(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.move_picker.as_mut() else {
            return;
        };
        let set = picker.expanded.entry(notebook_id).or_default();
        if !set.remove(rel_path) {
            set.insert(rel_path.to_string());
        }
        cx.notify();
    }

    pub(super) fn select_move_destination(
        &mut self,
        notebook_id: Uuid,
        folder_rel: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.move_picker.as_ref() else {
            return;
        };
        if !self.move_destination_allowed(
            picker.source_notebook_id,
            &picker.source_rel,
            picker.source_kind,
            notebook_id,
            folder_rel.as_deref(),
        ) {
            return;
        }
        let Some(picker) = self.move_picker.as_mut() else {
            return;
        };
        picker.selected = Some((notebook_id, folder_rel));
        picker.error = None;
        cx.notify();
    }

    pub(super) fn move_destination_allowed(
        &self,
        source_notebook_id: Uuid,
        source_rel: &str,
        source_kind: EntryKind,
        dest_notebook_id: Uuid,
        dest_folder: Option<&str>,
    ) -> bool {
        let current_parent = source_rel.rfind('/').map(|i| &source_rel[..i]);
        if dest_notebook_id == source_notebook_id {
            match (dest_folder, current_parent) {
                (None, None) => return false,
                (Some(folder), Some(parent)) if folder == parent => return false,
                (None, Some(_)) | (Some(_), None) => {}
                (Some(_), Some(_)) => {}
            }
            if source_kind == EntryKind::Folder {
                if dest_folder.is_some_and(|folder| {
                    folder == source_rel || folder.starts_with(&format!("{source_rel}/"))
                }) {
                    return false;
                }
            }
        }
        true
    }

    pub(super) fn confirm_move_picker(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.move_picker.as_ref() else {
            return;
        };
        let Some((dest_notebook_id, dest_folder)) = picker.selected.clone() else {
            return;
        };
        if !self.move_destination_allowed(
            picker.source_notebook_id,
            &picker.source_rel,
            picker.source_kind,
            dest_notebook_id,
            dest_folder.as_deref(),
        ) {
            return;
        }
        let source_notebook_id = picker.source_notebook_id;
        let source_rel = picker.source_rel.clone();
        let basename = source_rel
            .rsplit('/')
            .next()
            .unwrap_or(source_rel.as_str())
            .to_string();
        let dest_rel = match &dest_folder {
            Some(parent) => format!("{parent}/{basename}"),
            None => basename,
        };
        self.move_picker = None;
        if dest_notebook_id == source_notebook_id {
            self.begin_move_entry(source_notebook_id, source_rel, dest_rel, cx);
        } else {
            self.begin_cross_notebook_move(
                source_notebook_id,
                source_rel,
                dest_notebook_id,
                dest_folder,
                false,
                cx,
            );
        }
        cx.notify();
    }

    /// Copy (unique names) or move (exact name, then trash source) across notebooks.
    pub(super) fn begin_cross_notebook_move(
        &mut self,
        source_notebook_id: Uuid,
        source_rel: String,
        dest_notebook_id: Uuid,
        dest_folder: Option<String>,
        unique: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == source_notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let Some(target) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == dest_notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let target_name = target.display_name().to_string();
        let target_root = target.root.clone();
        let affected: Vec<EntityId> = self
            .docs
            .iter()
            .filter(|doc| {
                let doc = doc.read(cx);
                doc.notebook_id == Some(source_notebook_id)
                    && path_under(&doc.rel_path, &source_rel)
            })
            .map(|doc| doc.entity_id())
            .collect();
        for doc in &self.docs {
            if affected.contains(&doc.entity_id()) {
                doc.update(cx, |doc, _cx| doc.hold_saves());
            }
        }
        let delete_source = !unique;
        let source_rel_for_disk = source_rel.clone();
        let receiver = disk::submit(move || -> mdwork_core::Result<String> {
            let copied = source.copy_entry_to(
                &target,
                &source_rel_for_disk,
                dest_folder.as_deref(),
                unique,
            )?;
            place_created(&target, &copied, None);
            if delete_source {
                let _ = source.delete_entry(&source_rel_for_disk);
            }
            Ok(copied)
        });
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
                        if delete_source {
                            this.update_docs_after_cross_move(
                                source_notebook_id,
                                &source_rel,
                                dest_notebook_id,
                                &target_name,
                                target_root.clone(),
                                &new_rel,
                                cx,
                            );
                            if this.bookmarks_notebook == Some(source_notebook_id) {
                                this.bookmarks
                                    .entries
                                    .retain(|entry| !path_under(&entry.rel_path, &source_rel));
                                this.save_bookmarks(cx);
                            }
                            if this.trash_notebook == Some(source_notebook_id) {
                                this.trash_notebook = None;
                            }
                        }
                        this.reload_entries(cx);
                        this.persist_session(cx);
                        this.expand_to(dest_notebook_id, &new_rel);
                        this.reveal_active(cx);
                    }
                    Ok(Err(err)) => {
                        this.notify_error(
                            t!("tree.move_failed", error = err.to_string()).to_string(),
                            cx,
                        );
                    }
                    Err(_) => {
                        this.notify_error(
                            t!("tree.move_failed", error = "写入任务已停止").to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn update_docs_after_cross_move(
        &mut self,
        source_notebook_id: Uuid,
        from: &str,
        dest_notebook_id: Uuid,
        dest_name: &str,
        dest_root: PathBuf,
        to: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == dest_notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let from_prefix = format!("{from}/");
        for doc in &self.docs {
            let rel_path = {
                let doc = doc.read(cx);
                if doc.notebook_id != Some(source_notebook_id) {
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
            let notebook_meta = (dest_notebook_id, dest_name.to_string(), dest_root.clone());
            doc.update(cx, |doc, cx| {
                doc.rebase_with_notebook(Some(notebook_meta), new_rel, path);
                cx.notify();
            });
        }
    }

    pub(super) fn copy_tree_entry(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        cx: &mut Context<Self>,
    ) {
        self.tree_clipboard = Some(TreeClipboard {
            notebook_id,
            rel_path: rel_path.to_string(),
        });
        self.notify_success(t!("tree.copied").to_string(), cx);
        cx.notify();
    }

    pub(super) fn duplicate_tree_entry(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        cx: &mut Context<Self>,
    ) {
        let parent = rel_path.rfind('/').map(|i| rel_path[..i].to_string());
        self.begin_paste_entry(
            notebook_id,
            rel_path.to_string(),
            notebook_id,
            parent,
            true,
            cx,
        );
    }

    pub(super) fn paste_tree_clipboard(
        &mut self,
        dest_notebook_id: Uuid,
        dest_folder: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(clip) = self.tree_clipboard.clone() else {
            return;
        };
        self.begin_paste_entry(
            clip.notebook_id,
            clip.rel_path,
            dest_notebook_id,
            dest_folder,
            true,
            cx,
        );
    }

    pub(super) fn begin_paste_entry(
        &mut self,
        source_notebook_id: Uuid,
        source_rel: String,
        dest_notebook_id: Uuid,
        dest_folder: Option<String>,
        unique: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == source_notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let Some(target) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == dest_notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let anchor = if unique {
            let source_parent = parent_rel(&source_rel);
            let dest_parent = dest_folder.as_deref().unwrap_or("");
            if source_notebook_id == dest_notebook_id && source_parent == dest_parent {
                Some(source_rel.clone())
            } else {
                None
            }
        } else {
            None
        };
        let receiver = disk::submit(move || -> mdwork_core::Result<String> {
            let copied =
                source.copy_entry_to(&target, &source_rel, dest_folder.as_deref(), unique)?;
            place_created(&target, &copied, anchor.as_deref());
            Ok(copied)
        });
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(new_rel)) => {
                        this.reload_entries(cx);
                        this.expand_to(dest_notebook_id, &new_rel);
                        this.notify_success(
                            t!("tree.pasted", path = new_rel.as_str()).to_string(),
                            cx,
                        );
                    }
                    Ok(Err(err)) => {
                        this.notify_error(
                            t!("tree.copy_failed", error = err.to_string()).to_string(),
                            cx,
                        );
                    }
                    Err(_) => {
                        this.notify_error(
                            t!("tree.copy_failed", error = "写入任务已停止").to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Inserts a just-created note or folder into the open tree and starts
    /// in-place rename on that row.
    ///
    /// The following disk scan is asynchronous, so the new row has to be
    /// staged first. Otherwise rename looks for a row that is not loaded yet
    /// and returns without showing the field.
    pub(super) fn present_created_entry(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        kind: EntryKind,
        after_rel: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stage_created_entry(notebook_id, rel_path, kind, after_rel);
        self.select_tree_entry(notebook_id, rel_path, kind, cx);
        self.scroll_sidebar_entry(notebook_id, rel_path);
        self.begin_rename(notebook_id, rel_path, window, cx);
        self.reload_entries(cx);
    }

    pub(super) fn stage_created_entry(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        kind: EntryKind,
        after_rel: Option<&str>,
    ) {
        let parent = parent_rel(rel_path).to_string();
        let child_name = file_name(rel_path).to_string();
        {
            let Some(entry) = self
                .notebooks
                .iter_mut()
                .find(|entry| entry.notebook.marker.id == notebook_id)
            else {
                return;
            };
            if !entry
                .tree
                .iter()
                .any(|item| item.rel_path.eq_ignore_ascii_case(rel_path))
            {
                entry.tree.push(Entry {
                    rel_path: rel_path.to_string(),
                    name: entry_display_name(rel_path, kind),
                    kind,
                    size: 0,
                    created: 0,
                    modified: 0,
                });
            }
            let mut names = sibling_file_names(&entry.tree, &parent, &entry.order);
            insert_sibling_name(&mut names, child_name, after_rel.map(file_name));
            entry.order.set_names(parent, names);
            entry.root_expanded = true;
            let mut ancestor = rel_path;
            while let Some(index) = ancestor.rfind('/') {
                ancestor = &ancestor[..index];
                entry.expanded.insert(ancestor.to_string());
            }
            let expanded = entry.expanded.clone();
            let order = entry.order.clone();
            entry.rows = build_rows(&entry.tree, &expanded, &order);
        }
        self.rebuild_sidebar_rows();
    }

    pub(super) fn scroll_sidebar_entry(&mut self, notebook_id: Uuid, rel_path: &str) {
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
    }

    pub(super) fn begin_rename(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
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
            .unwrap_or_else(|| {
                let file = file_name(rel_path);
                if file.to_ascii_lowercase().ends_with(".md") {
                    entry_display_name(rel_path, EntryKind::Note)
                } else {
                    file.to_string()
                }
            });
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
        // Drop a scan that is already in flight. Applying it would rebuild the
        // row under the caret and the field's blur handler would commit.
        self.defer_entries_reload();
        cx.notify();
    }

    /// Ignores tree scans until the current create or rename finishes.
    ///
    /// Bumping the generation also drops a scan that has already started, so
    /// it cannot paint a new file at the end of the level before `place_created`
    /// has recorded where it belongs.
    pub(super) fn defer_entries_reload(&mut self) {
        self.entries_generation += 1;
        self.entries_reload_deferred = true;
    }

    pub(super) fn begin_create_hold(&mut self) {
        self.create_hold = self.create_hold.saturating_add(1);
        self.defer_entries_reload();
    }

    pub(super) fn end_create_hold(&mut self, cx: &mut Context<Self>) {
        self.create_hold = self.create_hold.saturating_sub(1);
        if self.create_hold == 0 && self.renaming.is_none() {
            self.flush_deferred_entries_reload(cx);
        }
    }

    pub(super) fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            self.flush_deferred_entries_reload(cx);
            cx.notify();
        }
    }

    fn flush_deferred_entries_reload(&mut self, cx: &mut Context<Self>) {
        if self.entries_reload_deferred {
            self.reload_entries(cx);
        }
    }

    pub(super) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let value = renaming.input.read(cx).value().trim().to_string();
        if value.is_empty() {
            self.flush_deferred_entries_reload(cx);
            cx.notify();
            return;
        }
        let target = match parent_rel(&renaming.rel_path) {
            "" => value,
            parent => format!("{parent}/{value}"),
        };
        // The field shows the note title without `.md`. Accepting that title
        // must keep the file, not try to rename it onto itself.
        if self.rename_is_noop(renaming.notebook_id, &renaming.rel_path, &target) {
            self.flush_deferred_entries_reload(cx);
            cx.notify();
            return;
        }
        self.begin_move_entry(renaming.notebook_id, renaming.rel_path.clone(), target, cx);
        cx.notify();
    }

    /// True when `to` names the same notebook entry as `from`.
    fn rename_is_noop(&self, notebook_id: Uuid, from: &str, to: &str) -> bool {
        if to.eq_ignore_ascii_case(from) {
            return true;
        }
        let Some(entry) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            return false;
        };
        let is_folder = entry
            .tree
            .iter()
            .any(|item| item.rel_path.eq_ignore_ascii_case(from) && item.kind == EntryKind::Folder);
        let Ok(source) = entry.notebook.normalize_rel(from) else {
            return false;
        };
        let normalized = if is_folder {
            entry.notebook.normalize_rel(to)
        } else {
            entry.notebook.normalize_note_rel(to)
        };
        normalized.is_ok_and(|target| target.eq_ignore_ascii_case(&source))
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
            self.flush_deferred_entries_reload(cx);
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
                        this.flush_deferred_entries_reload(cx);
                        this.notify_error(
                            t!("tree.rename_failed", error = err.to_string()).to_string(),
                            cx,
                        );
                    }
                    Err(_) => {
                        this.flush_deferred_entries_reload(cx);
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
