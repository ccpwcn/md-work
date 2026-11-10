use super::util::*;
use super::*;
use crate::official_docs::OfficialDocKind;

/// 批量打开任务：打开请求及其对应的磁盘读取结果。
type OpenedPair = (
    OpenRequest,
    mdwork_core::Result<(PathBuf, mdwork_core::doc::DecodedDoc, String)>,
);

impl AppShell {
    /// 查找已打开或仍在保存的文档，使标签关闭后才到达的
    /// 磁盘操作仍能释放该文档占用的保存锁。
    pub(super) fn doc_by_id(&self, id: EntityId) -> Option<Entity<Doc>> {
        self.docs
            .iter()
            .chain(self.closing_docs.values())
            .find(|doc| doc.entity_id() == id)
            .cloned()
    }

    /// 最后一次保存尚未完成的标签仍占用此路径，此时重新打开
    /// 会读到保存前的磁盘内容。
    fn is_closing(&self, path: &Path, cx: &App) -> bool {
        self.closing_docs
            .values()
            .any(|doc| doc.read(cx).path == path)
    }

    pub(super) fn open_path(&mut self, raw: &str, window: &mut Window, cx: &mut Context<Self>) {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.initializing {
            self.deferred_paths.push(trimmed.to_string());
            return;
        }
        let path = PathBuf::from(trimmed);
        let is_markdown = path
            .extension()
            .map(|ext| {
                let ext = ext.to_string_lossy().to_lowercase();
                ext == "md" || ext == "markdown"
            })
            .unwrap_or(false);
        if !is_markdown {
            self.notify_warning(format!("不是 Markdown 文件：{}", path.display()), cx);
            return;
        }
        // 从资源管理器 / 单实例转发打开安装目录里的官方说明文档时，
        // 也必须走只读预览通道，不能当成普通外部文档编辑。
        if let Some(kind) = crate::official_docs::kind_for_path(&path) {
            self.open_official_doc(kind, window, cx);
            return;
        }

        let notebook = self
            .notebooks
            .iter()
            .filter(|entry| entry.notebook.contains_path(&path))
            .max_by_key(|entry| entry.notebook.root.as_os_str().len())
            .map(|entry| entry.notebook.clone());
        if let Some(notebook) = notebook
            && let Ok(rel) = path.strip_prefix(&notebook.root)
        {
            let rel = rel.to_string_lossy().replace('\\', "/");
            self.active_notebook = Some(notebook.marker.id);
            self.open_note(&notebook, &rel, window, cx);
            return;
        }
        self.open_external(&path, window, cx);
    }

    /// 笔记树里某个条目的绝对磁盘路径；笔记本不存在时返回 `None`。
    pub(super) fn tree_entry_path(&self, notebook_id: Uuid, rel_path: &str) -> Option<PathBuf> {
        self.notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .and_then(|entry| entry.notebook.note_path(rel_path).ok())
    }

    /// 把笔记的绝对路径写入系统剪贴板（区别于笔记树内部的应用内剪贴板）。
    pub(super) fn copy_note_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(path) = path else {
            self.notify_error(t!("note.path_unavailable").to_string(), cx);
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
        self.notify_success(t!("note.path_copied").to_string(), cx);
    }

    /// 在文件管理器中定位笔记（Windows 会选中该文件）。
    pub(super) fn reveal_note_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(path) = path else {
            self.notify_error(t!("note.path_unavailable").to_string(), cx);
            return;
        };
        let receiver = disk::submit(move || reveal_path_in_explorer_sync(&path));
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                if let Err(err) = result {
                    this.notify_error(
                        t!("note.open_folder_failed", message = err.to_string()).to_string(),
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn apply_pending_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line)) = self.pending_jump.take() else {
            return;
        };
        if let Some(doc) = self
            .docs
            .iter()
            .find(|doc| doc.read(cx).path == path)
            .cloned()
        {
            doc.update(cx, |doc, cx| {
                doc.set_cursor(line.saturating_sub(1), 0, window, cx)
            });
        }
    }

    pub(super) fn open_note(
        &mut self,
        notebook: &Notebook,
        rel_path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Ok(path) = notebook.note_path(rel_path)
            && self.is_closing(&path, cx)
        {
            return;
        }
        self.select_tree_entry(notebook.marker.id, rel_path, EntryKind::Note, cx);
        if let Ok(path) = notebook.note_path(rel_path)
            && let Some(index) = self.docs.iter().position(|doc| doc.read(cx).path == path)
        {
            self.active_doc = Some(index);
            self.active_notebook = Some(notebook.marker.id);
            self.persist_session(cx);
            self.reveal_active(cx);
            self.apply_pending_jump(window, cx);
            self.request_editor_focus(cx);
            cx.notify();
            return;
        }
        // 异步读盘结束后再聚焦编辑器（此时实体还没创建）。
        self.focus_on_open_path = notebook.note_path(rel_path).ok();
        let request = OpenRequest::Notebook {
            notebook: notebook.clone(),
            rel_path: rel_path.to_string(),
        };
        self.open_requests(vec![request], window, cx);
    }

    pub(super) fn open_note_by_id(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        window: &mut Window,
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
        self.open_note(&notebook, rel_path, window, cx);
    }

    pub(super) fn open_external(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_closing(path, cx) {
            return;
        }
        if let Some(index) = self.docs.iter().position(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id.is_none() && doc.path == path
        }) {
            self.active_doc = Some(index);
            self.persist_session(cx);
            self.reveal_active(cx);
            cx.notify();
            return;
        }
        let request = OpenRequest::External {
            path: path.to_path_buf(),
        };
        self.open_requests(vec![request], window, cx);
    }

    pub(super) fn open_requests(
        &mut self,
        mut requests: Vec<OpenRequest>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        requests.retain(|request| {
            let Some(path) = request.target_path() else {
                return false;
            };
            if self.opening.contains(&path) {
                return false;
            }
            !self
                .docs
                .iter()
                .chain(self.closing_docs.values())
                .any(|doc| doc.read(cx).path == path)
        });
        if requests.is_empty() {
            return;
        }
        for request in &requests {
            if let Some(path) = request.target_path() {
                self.opening.insert(path);
            }
        }
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let mut pairs = Vec::new();
            for request in requests {
                let pair = cx
                    .background_executor()
                    .spawn(async move {
                        let result = request.read();
                        (request, result)
                    })
                    .await;
                pairs.push(pair);
            }
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.apply_opened(pairs, window, cx));
            });
        })
        .detach();
    }

    pub(super) fn apply_opened(
        &mut self,
        pairs: Vec<OpenedPair>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for (request, result) in pairs {
            if let Some(path) = request.target_path() {
                self.opening.remove(&path);
            }
            match result {
                Ok((path, decoded, hash)) => {
                    if self
                        .insert_doc(request, path, decoded, hash, window, cx)
                        .is_some()
                    {
                        changed = true;
                    }
                }
                Err(err) => {
                    self.notify_error(format!("无法打开 {}：{err}", request.label()), cx);
                }
            }
        }
        if let Some((path, _)) = &self.pending_jump
            && self.docs.iter().any(|doc| doc.read(cx).path == *path)
        {
            self.apply_pending_jump(window, cx);
            changed = true;
        }
        if let Some(snapshot) = self.pending_recovery.take()
            && let Some(doc) = self
                .docs
                .iter()
                .find(|doc| doc.read(cx).path == snapshot.note_path)
                .cloned()
        {
            let text = snapshot.text.clone();
            doc.update(cx, |doc, cx| doc.replace_all(text, window, cx));
            let note_path = snapshot.note_path.clone();
            disk::enqueue(move || {
                let _ =
                    mdwork_core::recovery::remove(&mdwork_core::paths::recovery_dir(), &note_path);
            });
            if let Some(dialog) = self.recovery.as_mut() {
                dialog
                    .entries
                    .retain(|entry| entry.note_path != snapshot.note_path);
                if dialog.entries.is_empty() {
                    self.recovery = None;
                }
            }
            self.notify_success(t!("recovery.restored").to_string(), cx);
            changed = true;
        }
        // `opening` 为空说明本轮所有打开任务都已落地；此时再把官方说明
        // 文档设为活动标签，不会被后到达的欢迎笔记 / 命令行路径挤掉。
        if self.opening.is_empty()
            && let Some(kind) = self.pending_official_focus.take()
            && let Some(index) = self
                .docs
                .iter()
                .position(|doc| doc.read(cx).official == Some(kind))
        {
            self.active_doc = Some(index);
            changed = true;
        }
        // 从笔记树点开的笔记此时才真正落地，交给下一帧聚焦编辑区。
        if self.focus_on_open_path.as_ref().is_some_and(|path| {
            self.active_doc_entity()
                .is_some_and(|doc| doc.read(cx).path == *path)
        }) {
            self.focus_on_open_path = None;
            self.request_editor_focus(cx);
        }
        if changed {
            self.reveal_active(cx);
            self.persist_session(cx);
        }
        cx.notify();
    }

    /// Creates a tab for an already-read file and subscribes it to shell
    /// events. Returns `None` when the file is already open.
    pub(super) fn insert_doc(
        &mut self,
        request: OpenRequest,
        path: PathBuf,
        decoded: mdwork_core::doc::DecodedDoc,
        hash: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        if let Some(index) = self.docs.iter().position(|doc| doc.read(cx).path == path) {
            self.active_doc = Some(index);
            return Some(index);
        }
        let official = match &request {
            OpenRequest::Official { kind, .. } => Some(*kind),
            _ => None,
        };
        let (origin, root, rel_path) = match request {
            OpenRequest::Notebook { notebook, rel_path } => (
                DocOrigin::Notebook {
                    id: notebook.marker.id,
                    name: notebook.display_name().to_string(),
                },
                Some(notebook.root.clone()),
                rel_path,
            ),
            OpenRequest::External { path } => {
                let file_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                (DocOrigin::External, None, file_name)
            }
            OpenRequest::Official { kind, .. } => (DocOrigin::External, None, kind.title()),
        };
        let autosave = self.config.autosave;
        let registry = self.dirty.clone();
        let doc = cx.new(|cx| {
            Doc::new(
                origin, root, rel_path, path, decoded, hash, autosave, registry, official, window,
                cx,
            )
        });
        cx.observe(&doc, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&doc, |this, doc, event: &DocEvent, cx| {
            this.handle_doc_event(doc.entity_id(), event.clone(), cx);
        })
        .detach();
        self.docs.push(doc);
        self.active_doc = Some(self.docs.len() - 1);
        Some(self.docs.len() - 1)
    }

    /// Copies an externally opened buffer into the active notebook as a new
    /// note. The original file on disk is left untouched.
    pub(super) fn save_external_into_notebook(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.active_doc else {
            return;
        };
        let Some(doc) = self.docs.get(index).cloned() else {
            return;
        };
        if doc.read(cx).official.is_some() {
            self.warn_official_readonly(cx);
            return;
        }
        let (text, encoding, bom, eol, file_name) = {
            let doc = doc.read(cx);
            if doc.notebook_id.is_some() {
                return;
            }
            (
                doc.text(cx).to_string(),
                doc.encoding,
                doc.bom,
                doc.eol,
                doc.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "note.md".to_string()),
            )
        };

        let notebook = self
            .active_notebook
            .and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
            })
            .or_else(|| self.notebooks.first())
            .map(|entry| entry.notebook.clone());
        let Some(notebook) = notebook else {
            self.notify_warning(t!("external.no_notebook").to_string(), cx);
            return;
        };

        let target_notebook = notebook.clone();
        let source_name = file_name.clone();
        let receiver = disk::submit(move || -> mdwork_core::Result<String> {
            let (stem, ext) = match source_name.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
                    (stem.to_string(), Some(ext.to_string()))
                }
                _ => (source_name.clone(), None),
            };
            let mut rel_path = source_name.clone();
            let mut counter = 2;
            while target_notebook
                .note_path(&rel_path)
                .map(|path| path.exists())
                .unwrap_or(true)
            {
                rel_path = match &ext {
                    Some(ext) => format!("{stem} ({counter}).{ext}"),
                    None => format!("{stem} ({counter})"),
                };
                counter += 1;
            }
            let path = target_notebook.note_path(&rel_path)?;
            let bytes = mdwork_core::doc::encode_text(&text, encoding, bom, eol)?;
            mdwork_core::atomic::write_atomic(&path, &bytes)?;
            let hint = mdwork_core::doc::EncodingHint { encoding, bom, eol };
            if let Err(err) =
                mdwork_core::notebook::store_encoding_hint(&target_notebook.root, &rel_path, hint)
            {
                tracing::warn!(
                    path = %rel_path,
                    error = %err,
                    "写入编码记录失败"
                );
            }
            tracing::info!(
                notebook = %target_notebook.display_name(),
                path = %rel_path,
                "外部文件已存入笔记本"
            );
            Ok(rel_path)
        });
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(rel_path) => {
                        if let Some(index) = this
                            .docs
                            .iter()
                            .position(|open| open.entity_id() == doc.entity_id())
                        {
                            this.drop_doc_at(index);
                        }
                        this.active_notebook = Some(notebook.marker.id);
                        this.reload_entries(cx);
                        this.open_note(&notebook, &rel_path, window, cx);
                        this.notify_success(t!("external.saved", path = rel_path).to_string(), cx);
                    }
                    Err(err) => this.notify_error(format!("存入笔记本失败：{err}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn activate_note(
        &mut self,
        notebook_id: &Uuid,
        rel_path: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.docs.iter().position(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id == Some(*notebook_id) && doc.rel_path == rel_path
        }) {
            self.active_doc = Some(index);
            self.select_tree_entry(*notebook_id, rel_path, EntryKind::Note, cx);
            self.reveal_active(cx);
        }
    }

    pub(super) fn activate_doc(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.docs.len() {
            return;
        }
        self.active_doc = Some(index);
        self.sync_tree_selection_to_active_doc(cx);
        if let Some(id) = self.docs[index].read(cx).notebook_id {
            self.active_notebook = Some(id);
        }
        self.persist_session(cx);
        self.reveal_active(cx);
        cx.notify();
    }

    pub(super) fn activate_notebook(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.select_tree_root(id, cx);
        if let Some(entry) = self
            .notebooks
            .iter_mut()
            .find(|entry| entry.notebook.marker.id == id)
            && !entry.root_expanded
        {
            entry.root_expanded = true;
            self.rebuild_sidebar_rows();
        }
        let preferred = self.docs.iter().position(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id == Some(id)
                && self
                    .session
                    .notebook(&id)
                    .and_then(|saved| saved.active_note.as_deref())
                    .map(|active| active == doc.rel_path)
                    .unwrap_or(false)
        });
        if let Some(index) = preferred {
            self.active_doc = Some(index);
        }
        self.persist_session(cx);
        self.reveal_active(cx);
        cx.notify();
    }

    /// 把标签稳定的实体 id 解析为当前下标。监听器必须在事件发生时解析：
    /// 关闭前面的任一标签都会让后续下标整体前移。
    pub(super) fn doc_index(&self, id: EntityId) -> Option<usize> {
        self.docs.iter().position(|doc| doc.entity_id() == id)
    }

    pub(super) fn activate_doc_by_id(&mut self, id: EntityId, cx: &mut Context<Self>) {
        if let Some(index) = self.doc_index(id) {
            self.activate_doc(index, cx);
            self.request_editor_focus(cx);
        }
    }

    /// 请求下一帧把输入焦点交给当前编辑区；render 里执行，保证编辑器已创建。
    pub(super) fn request_editor_focus(&mut self, cx: &mut Context<Self>) {
        self.pending_editor_focus = true;
        cx.notify();
    }

    /// 从笔记树打开笔记 / 切换页签后，让默认编辑态直接可以打字。
    /// 预览态、官方只读文档、笔记树上的命名输入框以及任何模态对话框
    /// 打开时都不抢焦点。
    pub(super) fn focus_active_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        // 新建笔记 / 重命名 / 新建笔记本时，名称输入框优先。抢走焦点不仅
        // 打不了字，输入框的 Blur 还会被当成「确认」直接提交默认名。
        if self.renaming.is_some() || self.renaming_notebook.is_some() {
            return;
        }
        if self.settings.is_some()
            || self.palette.is_some()
            || self.prompt.is_some()
            || self.move_picker.is_some()
            || self.export.is_some()
            || self.zip.is_some()
            || self.recovery.is_some()
            || self.delete_confirm.is_some()
            || self.workspace_confirm.is_some()
            || !self.official_doc_alerts.is_empty()
        {
            return;
        }
        let Some(doc) = self.active_doc_entity() else {
            return;
        };
        let state = doc.read(cx);
        if state.preview_mode || state.official.is_some() {
            return;
        }
        let handle = state.editor.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    pub(super) fn close_doc(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(id) = self.docs.get(index).map(|doc| doc.entity_id()) else {
            return;
        };
        self.close_doc_by_id(id, cx);
    }

    /// 按稳定 id 移除标签。标签立即离开标签栏，
    /// 即使缓冲区仍需保存；由 `closing_docs` 持有实体，
    /// 直到保存事件到达后才释放。
    pub(super) fn close_doc_by_id(&mut self, id: EntityId, cx: &mut Context<Self>) {
        if self.remove_doc_by_id(id, cx) {
            self.after_tabs_changed(cx);
        }
    }

    /// 返回是否真的移除了标签；批量关闭只在末尾同步一次状态。
    pub(super) fn remove_doc_by_id(&mut self, id: EntityId, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.doc_index(id) else {
            return false;
        };
        let doc = self.docs[index].clone();
        let official = doc.read(cx).official;
        if doc.read(cx).dirty && !self.pending_close.contains(&id) {
            self.pending_close.insert(id);
            self.closing_docs.insert(id, doc.clone());
            doc.update(cx, |doc, cx| doc.save_now(cx));
        }
        self.drop_doc_at(index);
        // 用户关掉这份更新说明后，记下版本，下次启动不再自动打开。
        if official == Some(OfficialDocKind::Changelog) {
            self.mark_changelog_seen(cx);
        }
        true
    }

    /// 持久化「本版本更新说明已读」标记。
    pub(super) fn mark_changelog_seen(&mut self, cx: &mut Context<Self>) {
        let version = mdwork_core::version();
        if self.config.shown_changelog_version == version {
            return;
        }
        self.config.shown_changelog_version = version.to_string();
        self.save_config(cx);
    }

    /// 标签移除后重新同步所有由 `docs` 派生的状态：
    /// 活动标签、侧边栏高亮、会话以及标签栏滚动位置。
    pub(super) fn after_tabs_changed(&mut self, cx: &mut Context<Self>) {
        self.tab_drag = None;
        let visible = self.visible_tabs(cx);
        if let Some(active) = self.active_doc
            && !visible.contains(&active)
        {
            self.active_doc = visible.last().copied();
        }
        self.sync_tree_selection_to_active_doc(cx);
        self.persist_session(cx);
        self.ensure_active_tab_visible(cx);
        self.reveal_active(cx);
        cx.notify();
    }

    pub(super) fn drop_doc_at(&mut self, index: usize) {
        if index >= self.docs.len() {
            return;
        }
        self.docs.remove(index);
        self.active_doc = match self.active_doc {
            Some(active) if active == index => {
                if self.docs.is_empty() {
                    None
                } else {
                    Some(index.min(self.docs.len() - 1))
                }
            }
            Some(active) if active > index => Some(active - 1),
            other => other,
        };
    }

    /// Indices of the docs shown in the tab bar, in display order.
    pub(super) fn visible_tabs(&self, cx: &App) -> Vec<usize> {
        self.docs
            .iter()
            .enumerate()
            .filter_map(|(index, doc)| {
                let doc = doc.read(cx);
                if doc.notebook_id.is_some() && doc.notebook_id != self.active_notebook {
                    None
                } else {
                    Some(index)
                }
            })
            .collect()
    }

    /// Reorders a tab among the currently visible tabs.
    ///
    /// `from` is a `docs` index; `insert_at` is a slot in the visible list
    /// before the move (`0..=visible.len()`). Entities are cloned handles only
    /// — never recreated — so editors keep their undo history and view state.
    pub(super) fn reorder_visible_tab(
        &mut self,
        from: usize,
        insert_at: usize,
        cx: &mut Context<Self>,
    ) {
        let visible = self.visible_tabs(cx);
        let Some(from_pos) = visible.iter().position(|&idx| idx == from) else {
            return;
        };
        let mut order = visible.clone();
        let moved = order.remove(from_pos);
        // `insert_at` is relative to the pre-remove list.
        let dest = if insert_at > from_pos {
            insert_at - 1
        } else {
            insert_at
        };
        let dest = dest.min(order.len());
        if dest == from_pos {
            return;
        }
        order.insert(dest, moved);

        let active_id = self
            .active_doc
            .and_then(|i| self.docs.get(i).map(|doc| doc.entity_id()));
        let visible_set: HashSet<usize> = visible.into_iter().collect();
        let mut order_iter = order.into_iter();
        let mut new_docs = Vec::with_capacity(self.docs.len());
        let mut new_active = None;
        for (old_idx, doc) in self.docs.iter().enumerate() {
            let next = if visible_set.contains(&old_idx) {
                let src = order_iter
                    .next()
                    .expect("visible order must cover every visible slot");
                self.docs[src].clone()
            } else {
                doc.clone()
            };
            if active_id == Some(next.entity_id()) {
                new_active = Some(new_docs.len());
            }
            new_docs.push(next);
        }
        self.docs = new_docs;
        self.active_doc = new_active;
        self.persist_session(cx);
        cx.notify();
    }

    /// 批量关闭标签。实体 id 在整个批次中保持有效，
    /// 而同一渲染帧里记录的下标会失效。
    pub(super) fn close_docs(&mut self, ids: &[EntityId], cx: &mut Context<Self>) {
        let mut changed = false;
        for &id in ids {
            changed |= self.remove_doc_by_id(id, cx);
        }
        if changed {
            self.after_tabs_changed(cx);
        }
    }

    /// Expands every ancestor folder of `rel_path` so the entry becomes visible.
    pub(super) fn expand_to(&mut self, notebook_id: Uuid, rel_path: &str) {
        let Some(entry) = self
            .notebooks
            .iter_mut()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            return;
        };
        let mut changed = !entry.root_expanded;
        entry.root_expanded = true;
        let mut ancestor = rel_path;
        while let Some(index) = ancestor.rfind('/') {
            ancestor = &ancestor[..index];
            changed |= entry.expanded.insert(ancestor.to_string());
        }
        if changed {
            let expanded = entry.expanded.clone();
            let order = entry.order.clone();
            entry.rows = build_rows(&entry.tree, &expanded, &order);
            self.rebuild_sidebar_rows();
        }
    }

    /// Handles asynchronous save results: completes tab closes and deletions
    /// that were waiting for the buffer to reach disk.
    pub(super) fn handle_doc_event(
        &mut self,
        id: EntityId,
        event: DocEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            DocEvent::Saved { clean } => {
                if !self.pending_close.contains(&id) {
                    return;
                }
                if clean {
                    self.pending_close.remove(&id);
                    self.closing_docs.remove(&id);
                    self.try_complete_delete(cx);
                } else if let Some(doc) = self.closing_docs.get(&id).cloned() {
                    // Typed while the save was in flight; save the new text.
                    doc.update(cx, |doc, cx| doc.save_now(cx));
                }
                cx.notify();
            }
            DocEvent::SaveFailed { error } => {
                if self.pending_close.remove(&id) {
                    if let Some(doc) = self.closing_docs.remove(&id) {
                        // 把未保存的缓冲区放回标签栏，避免内容丢失。
                        self.docs.push(doc);
                        self.active_doc = Some(self.docs.len() - 1);
                        self.after_tabs_changed(cx);
                    }
                    self.pending_delete = None;
                    self.notify_error(format!("保存失败，未关闭：{error}"), cx);
                } else {
                    self.notify_error(format!("保存失败：{error}"), cx);
                }
                cx.notify();
            }
        }
    }

    pub(super) fn try_complete_delete(&mut self, cx: &mut Context<Self>) {
        let Some((notebook_id, rel_path)) = self.pending_delete.clone() else {
            return;
        };
        let still_open = self
            .docs
            .iter()
            .chain(self.closing_docs.values())
            .any(|doc| {
                let doc = doc.read(cx);
                doc.notebook_id == Some(notebook_id) && path_under(&doc.rel_path, &rel_path)
            });
        if still_open {
            return;
        }
        self.pending_delete = None;
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let target_rel = rel_path.clone();
        let receiver = disk::submit(move || notebook.delete_entry(&target_rel));
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(item) => {
                        if this.trash_notebook == Some(notebook_id) {
                            this.trash_notebook = None;
                        }
                        if this.bookmarks_notebook == Some(notebook_id) {
                            this.bookmarks
                                .entries
                                .retain(|entry| !path_under(&entry.rel_path, &rel_path));
                            this.save_bookmarks(cx);
                        }
                        this.reload_entries(cx);
                        this.persist_session(cx);
                        this.notify_success(
                            format!("{}：{}", t!("trash.moved"), item.rel_path),
                            cx,
                        );
                    }
                    Err(err) => {
                        this.notify_error(
                            t!("tree.delete_failed", error = err.to_string()).to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_notebook else {
            self.notify_warning(t!("editor.no_notebook").to_string(), cx);
            return;
        };
        let (parent, place) = self
            .tree_selection
            .as_ref()
            .filter(|selection| selection.notebook_id() == id)
            .map(TreeSelection::create_target)
            .unwrap_or_else(|| {
                let parent = self.active_rel_path(cx).and_then(|rel| {
                    let parent = parent_rel(&rel);
                    if parent.is_empty() {
                        None
                    } else {
                        Some(parent.to_string())
                    }
                });
                (parent, SiblingPlace::End)
            });
        self.new_note_in(id, parent, place, window, cx);
    }

    pub(super) fn select_tree_root(&mut self, notebook_id: Uuid, cx: &mut Context<Self>) {
        self.active_notebook = Some(notebook_id);
        self.tree_selection = Some(TreeSelection::NotebookRoot(notebook_id));
        cx.notify();
    }

    pub(super) fn select_tree_entry(
        &mut self,
        notebook_id: Uuid,
        rel_path: &str,
        kind: EntryKind,
        cx: &mut Context<Self>,
    ) {
        self.active_notebook = Some(notebook_id);
        self.tree_selection = Some(TreeSelection::Entry {
            notebook_id,
            rel_path: rel_path.to_string(),
            kind,
        });
        cx.notify();
    }

    pub(super) fn sync_tree_selection_to_active_doc(&mut self, cx: &App) {
        let Some((notebook_id, rel_path)) = self.active_doc_entity().and_then(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id
                .map(|notebook_id| (notebook_id, doc.rel_path.clone()))
        }) else {
            return;
        };
        self.tree_selection = Some(TreeSelection::Entry {
            notebook_id,
            rel_path,
            kind: EntryKind::Note,
        });
    }

    pub(super) fn new_note_in(
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
            let mut name = "新笔记.md".to_string();
            let mut counter = 2;
            loop {
                let rel = match &parent {
                    Some(parent) => format!("{parent}/{name}"),
                    None => name.clone(),
                };
                let exists = target
                    .note_path(&rel)
                    .map(|path| path.exists())
                    .unwrap_or(true);
                if !exists {
                    let created = target.create_note(&rel, "")?;
                    place_created(&target, &created, anchor.as_deref());
                    return Ok(created);
                }
                name = format!("新笔记 {counter}.md");
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
                        // Stage the row first so the rename field has a place
                        // to land, then open the note, then focus the field.
                        this.stage_created_entry(
                            notebook_id,
                            &rel_path,
                            EntryKind::Note,
                            placed_after.as_deref(),
                        );
                        this.scroll_sidebar_entry(notebook_id, &rel_path);
                        this.open_note(&notebook, &rel_path, window, cx);
                        this.begin_rename(notebook_id, &rel_path, window, cx);
                        this.reload_entries(cx);
                    }
                    Err(err) => this.notify_error(format!("新建笔记失败：{err}"), cx),
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

    pub(super) fn save_active(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.active_doc else {
            return;
        };
        if let Some(doc) = self.docs.get(index) {
            if doc.read(cx).official.is_some() {
                self.warn_official_readonly(cx);
                return;
            }
            doc.update(cx, |doc, cx| doc.save_now(cx));
        }
        self.persist_session(cx);
        cx.notify();
    }

    pub(super) fn current_encoding(&self, cx: &App) -> (TextEncoding, bool, Eol) {
        self.active_doc_entity()
            .map(|doc| {
                let doc = doc.read(cx);
                (doc.encoding, doc.bom, doc.eol)
            })
            .unwrap_or((TextEncoding::Utf8, false, Eol::Lf))
    }

    pub(super) fn active_doc_entity(&self) -> Option<&Entity<Doc>> {
        self.active_doc.and_then(|index| self.docs.get(index))
    }

    pub(super) fn active_rel_path(&self, cx: &App) -> Option<String> {
        let doc = self.active_doc_entity()?;
        let doc = doc.read(cx);
        doc.notebook_id?;
        Some(doc.rel_path.clone())
    }
}
