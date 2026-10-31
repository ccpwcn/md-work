use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn save_config(&self, _cx: &mut Context<Self>) {
        let config = self.config.clone();
        disk::enqueue(move || {
            if let Err(err) = config.save() {
                tracing::warn!(error = %err, "保存设置失败");
            }
        });
    }

    pub(super) fn save_registry(&self, _cx: &mut Context<Self>) {
        let registry = self.workspace_registry.clone();
        disk::enqueue(move || {
            if let Err(err) = registry.save() {
                tracing::warn!(error = %err, "保存工作区列表失败");
            }
        });
    }

    /// Probes workspace folders off the UI thread so the switcher can render
    /// its "missing" markers without touching the filesystem.
    pub(super) fn queue_workspace_presence(&mut self, cx: &mut Context<Self>) {
        let roots: Vec<PathBuf> = self
            .workspace_registry
            .sorted()
            .iter()
            .map(|entry| entry.root())
            .collect();
        cx.spawn(async move |this, cx| {
            let missing = cx
                .background_executor()
                .spawn(async move {
                    roots
                        .into_iter()
                        .filter(|root| !root.is_dir())
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.missing_workspaces = missing.into_iter().collect();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn start_initialization(
        &mut self,
        root: PathBuf,
        root_exists: bool,
        cx: &mut Context<Self>,
    ) {
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let task_root = root.clone();
            let initial =
                cx.background_executor()
                    .spawn(async move {
                        let mut failures = Vec::new();
                        let workspace = match Workspace::open(&task_root) {
                            Ok(workspace) => {
                                if !root_exists {
                                    if let Err(err) = seed_workspace(&workspace) {
                                        failures.push((
                                            NoticeKind::Error,
                                            format!("初始化工作区失败：{err}"),
                                        ));
                                    }
                                }
                                Some(workspace)
                            }
                            Err(err) => {
                                failures.push((
                                    NoticeKind::Error,
                                    format!("无法打开工作区 {}：{err}", task_root.display()),
                                ));
                                None
                            }
                        };
                        let mut entries = Vec::new();
                        let mut issues = Vec::new();
                        if let Some(workspace) = &workspace {
                            match workspace.refresh() {
                                Ok(scan) => {
                                    issues = scan
                                        .issues
                                        .iter()
                                        .map(|issue| {
                                            tracing::warn!(
                                                path = %issue.path.display(),
                                                kind = ?issue.kind,
                                                message = %issue.message,
                                                "扫描工作区发现问题"
                                            );
                                            format!("{}：{}", issue.path.display(), issue.message)
                                        })
                                        .collect();
                                    entries = scan
                                        .notebooks
                                        .into_iter()
                                        .map(|notebook| {
                                            let tree = notebook.list_tree().unwrap_or_default();
                                            let rows = build_rows(&tree, &HashSet::new());
                                            NotebookEntry {
                                                notebook,
                                                tree,
                                                expanded: HashSet::new(),
                                                rows,
                                                root_expanded: false,
                                            }
                                        })
                                        .collect();
                                }
                                Err(err) => failures
                                    .push((NoticeKind::Error, format!("扫描工作区失败：{err}"))),
                            }
                        }
                        let snapshots =
                            mdwork_core::recovery::list(&mdwork_core::paths::recovery_dir());
                        Initialized {
                            root: task_root,
                            workspace,
                            failures,
                            entries,
                            issues,
                            snapshots,
                        }
                    })
                    .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.finish_initialization(initial, window, cx)
                });
            });
        })
        .detach();
    }

    pub(super) fn finish_initialization(
        &mut self,
        initial: Initialized,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.initializing = false;
        for (kind, text) in initial.failures {
            self.notices.push(NoticeItem {
                kind,
                text,
                read: false,
                created: Instant::now(),
            });
        }
        if let Some(workspace) = initial.workspace {
            self.workspace = Some(workspace);
            self.workspace_registry.touch(&initial.root);
            self.save_registry(cx);
        }
        self.notebooks = initial.entries;
        self.scan_issues = initial.issues;
        if self.active_notebook.is_none() {
            self.active_notebook = self.notebooks.first().map(|entry| entry.notebook.marker.id);
        }
        self.ensure_active_root_expanded();
        self.rebuild_sidebar_rows();
        if !initial.snapshots.is_empty() {
            self.recovery = Some(RecoveryDialog {
                entries: initial.snapshots,
            });
        }
        if self.startup_first_run {
            self.pending_welcome = self.notebooks.first().and_then(|entry| {
                let id = entry.notebook.marker.id;
                entry
                    .tree
                    .iter()
                    .find(|item| item.kind == EntryKind::Note)
                    .map(|item| (id, item.rel_path.to_string()))
            });
        }
        self.start_watcher();
        self.queue_workspace_presence(cx);
        self.restore_session(window, cx);
        cx.notify();
    }

    /// Opens CLI paths and the first-run welcome note once the session has
    /// been restored.
    pub(super) fn finish_startup_opens(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.pending_startup_path.take() {
            self.open_path(&path, window, cx);
        }
        let mut deferred = std::mem::take(&mut self.deferred_paths);
        deferred.extend(std::mem::take(&mut self.pending_extra_paths));
        for path in deferred {
            self.open_path(&path, window, cx);
        }
        if self.docs.is_empty() {
            if let Some((id, rel_path)) = self.pending_welcome.take() {
                self.open_note_by_id(id, &rel_path, window, cx);
            }
        }
        self.pending_welcome = None;
        self.startup_first_run = false;
        self.reveal_active(cx);
    }

    pub(super) fn start_watcher(&mut self) {
        self._watcher = None;
        self.changed = None;
        let Some(workspace) = &self.workspace else {
            return;
        };
        match mdwork_core::watch::WorkspaceWatcher::start(workspace.root()) {
            Ok((watcher, rx)) => {
                self._watcher = Some(watcher);
                self.changed = Some(Arc::new(Mutex::new(rx)));
            }
            Err(err) => tracing::warn!(error = %err, "文件监视不可用"),
        }
    }

    pub(super) fn spawn_poller(&self, cx: &mut Context<Self>) {
        let pending = self.pending.clone();
        cx.spawn(async move |this, cx| {
            let mut tick: u32 = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                tick = tick.wrapping_add(1);
                let _ = this.update(cx, |this, cx| {
                    let sig = this.active_doc_entity().map(|doc| {
                        let range = doc.read(cx).editor.read(cx).selected_range();
                        (range.start, range.end)
                    });
                    if sig != this.selection_sig {
                        this.selection_sig = sig;
                        cx.notify();
                    }
                });
                if tick % 8 == 0 {
                    let _ = this.update(cx, |this, cx| {
                        let live = this.mcp.running();
                        if live != this.mcp_live {
                            this.mcp_live = live;
                            cx.notify();
                        }
                    });
                }
                let paths: Vec<String> = {
                    let mut guard = pending.lock().expect("pending paths lock");
                    std::mem::take(&mut *guard)
                };
                let disk: Vec<PathBuf> = this
                    .read_with(cx, |this, _| {
                        let Some(rx) = &this.changed else {
                            return Vec::new();
                        };
                        let rx = rx.lock().expect("watcher receiver lock");
                        let mut paths = Vec::new();
                        while let Ok(batch) = rx.try_recv() {
                            paths.extend(batch);
                        }
                        paths
                    })
                    .unwrap_or_default();
                if paths.is_empty() && disk.is_empty() {
                    continue;
                }
                let Ok(handle) = this.read_with(cx, |this, _| this.window_handle) else {
                    continue;
                };
                let weak = this.clone();
                let _ = handle.update(cx, |_, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if this.initializing {
                            this.deferred_paths.extend(paths);
                        } else {
                            for path in &paths {
                                this.open_path(path, window, cx);
                            }
                        }
                        if !disk.is_empty() {
                            this.handle_external_changes(disk, window, cx);
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    pub(super) fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        self.reload_generation += 1;
        let generation = self.reload_generation;
        let previous: Vec<(Uuid, HashSet<String>, bool)> = self
            .notebooks
            .iter()
            .map(|entry| {
                (
                    entry.notebook.marker.id,
                    entry.expanded.clone(),
                    entry.root_expanded,
                )
            })
            .collect();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match workspace.refresh() {
                        Ok(scan) => {
                            let issues = scan
                                .issues
                                .iter()
                                .map(|issue| {
                                    tracing::warn!(
                                        path = %issue.path.display(),
                                        kind = ?issue.kind,
                                        message = %issue.message,
                                        "扫描工作区发现问题"
                                    );
                                    format!("{}：{}", issue.path.display(), issue.message)
                                })
                                .collect::<Vec<_>>();
                            let entries = scan
                                .notebooks
                                .into_iter()
                                .map(|notebook| {
                                    let (expanded, root_expanded) = previous
                                        .iter()
                                        .find(|(id, _, _)| *id == notebook.marker.id)
                                        .map(|(_, expanded, root)| (expanded.clone(), *root))
                                        .unwrap_or_default();
                                    let tree = notebook.list_tree().unwrap_or_default();
                                    let rows = build_rows(&tree, &expanded);
                                    NotebookEntry {
                                        notebook,
                                        tree,
                                        expanded,
                                        rows,
                                        root_expanded,
                                    }
                                })
                                .collect::<Vec<_>>();
                            Ok((entries, issues))
                        }
                        Err(err) => Err(err.to_string()),
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.reload_generation != generation {
                    return;
                }
                match result {
                    Ok((entries, issues)) => {
                        this.scan_issues = issues;
                        this.notebooks = entries;
                        if !this
                            .notebooks
                            .iter()
                            .any(|entry| Some(entry.notebook.marker.id) == this.active_notebook)
                        {
                            this.active_notebook =
                                this.notebooks.first().map(|entry| entry.notebook.marker.id);
                        }
                        this.ensure_active_root_expanded();
                        this.rebuild_sidebar_rows();
                    }
                    Err(err) => this.notify_error(format!("扫描工作区失败：{err}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn reload_entries(&mut self, cx: &mut Context<Self>) {
        self.entries_generation += 1;
        let generation = self.entries_generation;
        let notebooks: Vec<(Notebook, HashSet<String>)> = self
            .notebooks
            .iter()
            .map(|entry| (entry.notebook.clone(), entry.expanded.clone()))
            .collect();
        cx.spawn(async move |this, cx| {
            let refreshed = cx
                .background_executor()
                .spawn(async move {
                    notebooks
                        .into_iter()
                        .map(|(notebook, expanded)| {
                            let tree = notebook.list_tree().unwrap_or_default();
                            let rows = build_rows(&tree, &expanded);
                            (notebook.marker.id, tree, rows)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.entries_generation != generation {
                    return;
                }
                for (id, tree, rows) in refreshed {
                    if let Some(entry) = this
                        .notebooks
                        .iter_mut()
                        .find(|entry| entry.notebook.marker.id == id)
                    {
                        entry.tree = tree;
                        entry.rows = rows;
                    }
                }
                this.rebuild_sidebar_rows();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn handle_external_changes(
        &mut self,
        paths: Vec<PathBuf>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| !is_atomic_temp(path))
            .collect();
        if paths.is_empty() {
            return;
        }
        let open_paths: Vec<PathBuf> = self
            .docs
            .iter()
            .map(|doc| doc.read(cx).path.clone())
            .collect();
        let needs_scan = paths
            .iter()
            .any(|path| !open_paths.iter().any(|open| paths_equal(open, path)));

        let mut jobs: Vec<(EntityId, PathBuf, String)> = Vec::new();
        for doc in &self.docs {
            let (path, base_hash) = {
                let doc = doc.read(cx);
                (doc.path.clone(), doc.base_hash.clone())
            };
            if paths.iter().any(|changed| paths_equal(changed, &path)) {
                jobs.push((doc.entity_id(), path, base_hash));
            }
        }
        if !jobs.is_empty() {
            let handle = self.window_handle;
            cx.spawn(async move |this, cx| {
                let reads = cx
                    .background_executor()
                    .spawn(async move {
                        jobs.into_iter()
                            .map(|(id, path, base_hash)| {
                                let outcome = match std::fs::read(&path) {
                                    Ok(bytes) => {
                                        let hash = mdwork_core::doc::sha256_hex(&bytes);
                                        if hash == base_hash {
                                            DiskRead::Skip
                                        } else {
                                            DiskRead::Changed(
                                                mdwork_core::doc::decode(&bytes),
                                                hash,
                                            )
                                        }
                                    }
                                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                                        DiskRead::Missing
                                    }
                                    Err(_) => DiskRead::Skip,
                                };
                                (id, outcome)
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                let _ = handle.update(cx, |_, window, cx| {
                    let _ = this.update(cx, |this, cx| {
                        for (id, outcome) in reads {
                            let Some(doc) =
                                this.docs.iter().find(|doc| doc.entity_id() == id).cloned()
                            else {
                                continue;
                            };
                            match outcome {
                                DiskRead::Missing => {
                                    doc.update(cx, |doc, cx| {
                                        doc.mark_conflict(DiskConflict::Removed, cx)
                                    });
                                }
                                DiskRead::Skip => {}
                                DiskRead::Changed(decoded, hash) => {
                                    let (base_hash, dirty) = {
                                        let doc = doc.read(cx);
                                        (doc.base_hash.clone(), doc.dirty)
                                    };
                                    if hash == base_hash {
                                        continue;
                                    }
                                    doc.update(cx, |doc, cx| {
                                        if dirty {
                                            doc.mark_conflict(DiskConflict::Modified, cx);
                                        } else {
                                            doc.apply_disk(decoded, hash, window, cx);
                                        }
                                    });
                                }
                            }
                        }
                        cx.notify();
                    });
                });
            })
            .detach();
        } else {
            // The read result above is applied in a task; nothing to do here.
        }

        if needs_scan {
            let root = self
                .workspace
                .as_ref()
                .map(|workspace| workspace.root().to_path_buf());
            let notebook_touched = root.is_some_and(|root| {
                paths.iter().any(|path| {
                    path.parent() == Some(root.as_path())
                        || path
                            .file_name()
                            .is_some_and(|name| name == "notebook.toml" || name == "notebook.json")
                })
            });
            if notebook_touched {
                self.reload(cx);
            } else {
                self.reload_entries(cx);
            }
        }
        cx.notify();
    }

    pub(super) fn reload_doc(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get(index).cloned() else {
            return;
        };
        let path = doc.read(cx).path.clone();
        let error_path = path.clone();
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    std::fs::read(&path).ok().map(|bytes| {
                        let hash = mdwork_core::doc::sha256_hex(&bytes);
                        (mdwork_core::doc::decode(&bytes), hash)
                    })
                })
                .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    match read {
                        Some((decoded, hash)) => {
                            doc.update(cx, |doc, cx| doc.apply_disk(decoded, hash, window, cx));
                        }
                        None => {
                            this.notify_error(format!("无法读取 {}", error_path.display()), cx);
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn keep_doc(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(doc) = self.docs.get(index) {
            doc.update(cx, |doc, cx| doc.keep_mine(cx));
        }
        cx.notify();
    }

    pub(super) fn recreate_doc(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(doc) = self.docs.get(index) {
            doc.update(cx, |doc, cx| doc.save_now(cx));
        }
        cx.notify();
    }

    pub(super) fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Cursors must come from the session on disk: opening tabs rewrites it.
        let saved_cursors = self.session.clone();
        let session = self.session.clone();
        let mut requests = Vec::new();
        for saved in &session.notebooks {
            let Some(notebook) = self
                .notebooks
                .iter()
                .find(|entry| entry.notebook.marker.id == saved.id)
                .map(|entry| entry.notebook.clone())
            else {
                continue;
            };
            for rel_path in &saved.open_notes {
                requests.push(OpenRequest::Notebook {
                    notebook: notebook.clone(),
                    rel_path: rel_path.clone(),
                });
            }
        }
        for path in &session.external_notes {
            requests.push(OpenRequest::External {
                path: PathBuf::from(path),
            });
        }
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    requests
                        .into_iter()
                        .filter_map(|request| {
                            let result = request.read();
                            result
                                .ok()
                                .map(|(path, decoded, hash)| (request, path, decoded, hash))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.apply_restored_session(session, saved_cursors, loaded, window, cx);
                });
            });
        })
        .detach();
    }

    pub(super) fn apply_restored_session(
        &mut self,
        session: Session,
        saved_cursors: Session,
        loaded: Vec<(OpenRequest, PathBuf, mdwork_core::doc::DecodedDoc, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (request, path, decoded, hash) in loaded {
            self.insert_doc(request, path, decoded, hash, window, cx);
        }
        if let Some(id) = session.active_notebook {
            if self
                .notebooks
                .iter()
                .any(|entry| entry.notebook.marker.id == id)
            {
                self.active_notebook = Some(id);
            }
        }
        self.ensure_active_root_expanded();
        self.rebuild_sidebar_rows();
        if let Some(id) = self.active_notebook {
            if let Some(saved) = session.notebook(&id) {
                if let Some(active) = &saved.active_note {
                    self.activate_note(&id, active, cx);
                }
            }
        }
        if let Some(path) = &session.active_external {
            let path = PathBuf::from(path);
            if let Some(index) = self.docs.iter().position(|doc| {
                let doc = doc.read(cx);
                doc.notebook_id.is_none() && doc.path == path
            }) {
                self.active_doc = Some(index);
            }
        } else if let Some(id) = session.active_notebook {
            // Opening external notes above makes the last one active; put the
            // focus back on the notebook note that was active at shutdown.
            if let Some(saved) = session.notebook(&id) {
                if let Some(active) = &saved.active_note {
                    self.activate_note(&id, active, cx);
                }
            }
        }
        self.restore_cursor(&saved_cursors, window, cx);
        self.persist_session(cx);
        self.finish_startup_opens(window, cx);
        cx.notify();
    }

    pub(super) fn restore_cursor(
        &mut self,
        saved: &Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.active_doc else {
            return;
        };
        let Some(doc) = self.docs.get(index).cloned() else {
            return;
        };
        let cursor = {
            let doc = doc.read(cx);
            match doc.notebook_id {
                Some(id) => saved
                    .notebook(&id)
                    .and_then(|entry| entry.cursors.get(&doc.rel_path).copied()),
                None => saved
                    .external_cursors
                    .get(&doc.path.display().to_string())
                    .copied(),
            }
        };
        if let Some(pos) = cursor {
            doc.update(cx, |doc, cx| {
                doc.set_cursor(pos.line, pos.column, window, cx);
            });
        }
    }

    pub(super) fn persist_session(&mut self, cx: &App) {
        let mut session = Session::default();
        session.active_notebook = self.active_notebook;

        for entry in &self.notebooks {
            let id = entry.notebook.marker.id;
            let mut saved = NotebookSession {
                id,
                ..Default::default()
            };
            for doc in &self.docs {
                let doc = doc.read(cx);
                if doc.notebook_id == Some(id) {
                    saved.open_notes.push(doc.rel_path.clone());
                    let (line, column) = doc.cursor(cx);
                    saved
                        .cursors
                        .insert(doc.rel_path.clone(), CursorPos { line, column });
                }
            }
            if let Some(index) = self.active_doc {
                if let Some(doc) = self.docs.get(index) {
                    let doc = doc.read(cx);
                    if doc.notebook_id == Some(id) {
                        saved.active_note = Some(doc.rel_path.clone());
                    }
                }
            }
            session.notebooks.push(saved);
        }

        for doc in &self.docs {
            let doc = doc.read(cx);
            if doc.notebook_id.is_none() {
                let path = doc.path.display().to_string();
                let (line, column) = doc.cursor(cx);
                session
                    .external_cursors
                    .insert(path.clone(), CursorPos { line, column });
                session.external_notes.push(path);
            }
        }
        if let Some(index) = self.active_doc {
            if let Some(doc) = self.docs.get(index) {
                let doc = doc.read(cx);
                if doc.notebook_id.is_none() {
                    session.active_external = Some(doc.path.display().to_string());
                }
            }
        }

        let path = self.session_file();
        let session_for_disk = session.clone();
        disk::enqueue(move || {
            if let Err(err) = session_for_disk.save_to(&path) {
                tracing::warn!(error = %err, "保存会话失败");
            }
        });
        self.session = session;
    }

    pub(super) fn session_file(&self) -> PathBuf {
        match &self.workspace {
            Some(workspace) => mdwork_core::paths::workspace_session_path(workspace.root()),
            None => mdwork_core::paths::session_path(),
        }
    }
}
