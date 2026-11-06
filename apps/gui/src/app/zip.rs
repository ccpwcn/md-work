use super::util::*;
use super::*;

/// 重新载入任务：标签 id、路径及磁盘读取结果。
type ReloadedDoc = (
    EntityId,
    PathBuf,
    std::io::Result<(mdwork_core::doc::DecodedDoc, String)>,
);

impl AppShell {
    pub(super) fn open_zip_export(
        &mut self,
        notebook_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zip.is_some() {
            return;
        }
        let Some(entry) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            self.notify_warning(t!("zip.no_notebook").to_string(), cx);
            return;
        };
        let notebook_root = entry.notebook.root.clone();
        let notebook_name = entry.notebook.display_name().to_string();
        let stem = notebook_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "notebook".to_string());
        let handle = window.window_handle();
        let job_root = notebook_root.clone();
        cx.spawn(async move |this, cx| {
            let output = cx
                .background_executor()
                .spawn(async move {
                    let prefs = NotebookPrefs::load(&notebook_id);
                    let dir = prefs
                        .last_zip_dir
                        .as_deref()
                        .map(PathBuf::from)
                        .filter(|path| path.is_dir())
                        .or_else(|| job_root.parent().map(Path::to_path_buf))
                        .unwrap_or_else(|| job_root.clone());
                    unique_zip_path(&dir, &stem)
                })
                .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.open_zip_export_ready(notebook_id, notebook_name, output, window, cx);
                });
            });
        })
        .detach();
    }

    pub(super) fn open_zip_export_ready(
        &mut self,
        notebook_id: Uuid,
        notebook_name: String,
        output: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zip.is_some() {
            return;
        }
        let path_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(output.display().to_string())
                .clean_on_escape()
        });
        let rename_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(notebook_name.clone())
                .placeholder(t!("zip.rename_placeholder").to_string())
                .clean_on_escape()
        });
        subscribe_zip_path(cx, &path_input);
        subscribe_zip_path(cx, &rename_input);

        self.zip = Some(ZipDialog {
            mode: ZipDialogMode::Export,
            notebook_id: Some(notebook_id),
            notebook_name,
            workspace_root: self
                .workspace
                .as_ref()
                .map(|workspace| workspace.root().to_path_buf())
                .unwrap_or_default(),
            path_input,
            rename_input,
            import_mode: ImportMode::Merge,
            policy: ConflictPolicy::KeepBoth,
            choices: HashMap::new(),
            plan: None,
            plan_generation: 0,
            plan_loading: false,
            plan_error: None,
            confirm_overwrite: false,
            busy: false,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn open_zip_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.zip.is_some() {
            return;
        }
        let Some(root) = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().to_path_buf())
        else {
            self.notify_warning(t!("zip.no_workspace").to_string(), cx);
            return;
        };
        let notebook_id = self.active_notebook;
        let notebook_name = self
            .active_notebook
            .and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
                    .map(|entry| entry.notebook.display_name().to_string())
            })
            .unwrap_or_default();
        let dir_job = notebook_id;
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let dir = cx
                .background_executor()
                .spawn(async move {
                    dir_job
                        .map(|id| NotebookPrefs::load(&id))
                        .and_then(|prefs| prefs.last_import_dir)
                        .map(PathBuf::from)
                        .filter(|path| path.is_dir())
                })
                .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.open_zip_import_ready(root, notebook_id, notebook_name, dir, window, cx);
                });
            });
        })
        .detach();
    }

    pub(super) fn open_zip_import_ready(
        &mut self,
        root: PathBuf,
        notebook_id: Option<Uuid>,
        notebook_name: String,
        dir: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zip.is_some() {
            return;
        }
        let path_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .placeholder(t!("zip.path_placeholder").to_string())
                .clean_on_escape();
            if let Some(dir) = &dir {
                input = input.default_value(dir.display().to_string());
            }
            input
        });
        let rename_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("zip.rename_placeholder").to_string())
                .clean_on_escape()
        });
        subscribe_zip_path(cx, &path_input);
        subscribe_zip_path(cx, &rename_input);

        self.zip = Some(ZipDialog {
            mode: ZipDialogMode::Import,
            notebook_id,
            notebook_name,
            workspace_root: root,
            path_input,
            rename_input,
            import_mode: ImportMode::Merge,
            policy: ConflictPolicy::KeepBoth,
            choices: HashMap::new(),
            plan: None,
            plan_generation: 0,
            plan_loading: false,
            plan_error: None,
            confirm_overwrite: false,
            busy: false,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn close_zip_dialog(&mut self, cx: &mut Context<Self>) {
        self.zip = None;
        cx.notify();
    }

    pub(super) fn zip_rename_to(&self, cx: &Context<Self>) -> Option<String> {
        let dialog = self.zip.as_ref()?;
        if dialog.import_mode != ImportMode::Rename {
            return None;
        }
        let value = dialog.rename_input.read(cx).value().trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    }

    pub(super) fn schedule_zip_plan(&mut self, cx: &mut Context<Self>) {
        let rename_to = self.zip_rename_to(cx);
        let Some(dialog) = self.zip.as_mut() else {
            return;
        };
        if dialog.mode != ZipDialogMode::Import || dialog.busy {
            return;
        }
        dialog.plan_generation = dialog.plan_generation.wrapping_add(1);
        let generation = dialog.plan_generation;
        let raw = dialog.path_input.read(cx).value().trim().to_string();
        if raw.is_empty() {
            dialog.plan = None;
            dialog.plan_error = None;
            dialog.plan_loading = false;
            cx.notify();
            return;
        }
        let path = PathBuf::from(&raw);
        let mode = dialog.import_mode;
        let root = dialog.workspace_root.clone();
        dialog.plan_loading = true;
        dialog.plan_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(180))
                .await;
            let result = cx
                .background_executor()
                .spawn(async move {
                    if !path.is_file() {
                        return None;
                    }
                    Some(archive::plan_import(
                        &root,
                        &path,
                        mode,
                        rename_to.as_deref(),
                    ))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(dialog) = this.zip.as_mut() else {
                    return;
                };
                if dialog.plan_generation != generation {
                    return;
                }
                dialog.plan_loading = false;
                let Some(result) = result else {
                    dialog.plan = None;
                    dialog.plan_error = None;
                    cx.notify();
                    return;
                };
                match result {
                    Ok(plan) => {
                        dialog
                            .choices
                            .retain(|path, _| plan.conflicts.iter().any(|item| &item.path == path));
                        dialog.plan = Some(plan);
                        dialog.plan_error = None;
                    }
                    Err(err) => {
                        dialog.plan = None;
                        dialog.plan_error = Some(err.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn set_zip_import_mode(&mut self, mode: ImportMode, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_mut() else {
            return;
        };
        if dialog.import_mode == mode || dialog.busy {
            return;
        }
        dialog.import_mode = mode;
        dialog.confirm_overwrite = false;
        dialog.error = None;
        cx.notify();
        self.schedule_zip_plan(cx);
    }

    pub(super) fn set_zip_policy(&mut self, policy: ConflictPolicy, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_mut() else {
            return;
        };
        dialog.policy = policy;
        dialog.choices.clear();
        cx.notify();
    }

    pub(super) fn set_zip_choice(
        &mut self,
        path: &str,
        policy: ConflictPolicy,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.zip.as_mut() else {
            return;
        };
        if policy == dialog.policy {
            dialog.choices.remove(path);
        } else {
            dialog.choices.insert(path.to_string(), policy);
        }
        cx.notify();
    }

    pub(super) fn pick_zip_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let mode = dialog.mode;
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("zip.pick_prompt").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = picked else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(dialog) = this.zip.as_mut() else {
                    return;
                };
                let display = path.display().to_string();
                dialog
                    .path_input
                    .update(cx, |state, cx| state.set_value(display, window, cx));
                match mode {
                    ZipDialogMode::Import => this.schedule_zip_plan(cx),
                    ZipDialogMode::Export => {
                        let path = ensure_zip_extension(path);
                        let display = path.display().to_string();
                        if let Some(dialog) = this.zip.as_mut() {
                            dialog
                                .path_input
                                .update(cx, |state, cx| state.set_value(display, window, cx));
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn pick_zip_export_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let current = PathBuf::from(dialog.path_input.read(cx).value().to_string());
        let dir = current
            .parent()
            .map(Path::to_path_buf)
            .filter(|path| path.is_dir())
            .unwrap_or_else(|| dialog.workspace_root.clone());
        let name = current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{}.zip", dialog.notebook_name));
        let receiver = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(path))) => path,
                _ => return,
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if this.zip.is_some() {
                    let path = ensure_zip_extension(path);
                    if let Some(dialog) = this.zip.as_mut() {
                        dialog.path_input.update(cx, |state, cx| {
                            state.set_value(path.display().to_string(), window, cx)
                        });
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn begin_zip_export(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_ref() else {
            return;
        };
        if dialog.busy {
            return;
        }
        let Some(notebook_id) = dialog.notebook_id else {
            return;
        };
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let output = ensure_zip_extension(PathBuf::from(
            dialog.path_input.read(cx).value().trim().to_string(),
        ));
        if output.file_name().is_none() {
            if let Some(dialog) = self.zip.as_mut() {
                dialog.error = Some(t!("zip.invalid_path").to_string());
            }
            cx.notify();
            return;
        }
        if let Some(dialog) = self.zip.as_mut() {
            dialog.busy = true;
            dialog.error = None;
        }
        cx.notify();
        self.spawn_zip_export(notebook, output, notebook_id, cx);
    }

    pub(super) fn spawn_zip_export(
        &mut self,
        notebook: Notebook,
        output: PathBuf,
        notebook_id: Uuid,
        cx: &mut Context<Self>,
    ) {
        let target = output.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { archive::export_zip(&notebook, &target) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let message = match result {
                    Ok(summary) => {
                        this.zip = None;
                        let dir = output.parent().map(|parent| parent.display().to_string());
                        disk::enqueue(move || {
                            let mut prefs = NotebookPrefs::load(&notebook_id);
                            prefs.last_zip_dir = dir;
                            if let Err(err) = prefs.save(&notebook_id) {
                                tracing::warn!(error = %err, "保存笔记本偏好失败");
                            }
                        });
                        t!(
                            "zip.export_done",
                            path = output.display().to_string(),
                            files = summary.files
                        )
                        .to_string()
                    }
                    Err(err) => {
                        if let Some(dialog) = this.zip.as_mut() {
                            dialog.busy = false;
                            dialog.error = Some(err.to_string());
                        }
                        return cx.notify();
                    }
                };
                this.notify_success(message, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn begin_zip_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.zip.as_ref() else {
            return;
        };
        if dialog.busy || dialog.mode != ZipDialogMode::Import {
            return;
        }
        let zip_path = PathBuf::from(dialog.path_input.read(cx).value().trim().to_string());
        if zip_path.file_name().is_none() {
            if let Some(dialog) = self.zip.as_mut() {
                dialog.error = Some(t!("zip.invalid_path").to_string());
            }
            cx.notify();
            return;
        }
        if dialog.import_mode == ImportMode::Overwrite && !dialog.confirm_overwrite {
            if let Some(dialog) = self.zip.as_mut() {
                dialog.confirm_overwrite = true;
            }
            cx.notify();
            return;
        }
        let options = ImportOptions {
            mode: dialog.import_mode,
            policy: dialog.policy,
            choices: dialog.choices.clone(),
            rename_to: self.zip_rename_to(cx),
        };
        let root = dialog.workspace_root.clone();
        let notebook_id = dialog.notebook_id;

        for doc in &self.docs {
            doc.update(cx, |doc, cx| {
                if doc.dirty {
                    doc.save_now(cx);
                }
            });
        }
        let unsaved = self
            .docs
            .iter()
            .find(|doc| doc.read(cx).dirty)
            .map(|doc| doc.read(cx).path.display().to_string());
        if let Some(path) = unsaved {
            if let Some(dialog) = self.zip.as_mut() {
                dialog.error = Some(t!("zip.save_failed", path = path).to_string());
            }
            cx.notify();
            return;
        }

        if let Some(dialog) = self.zip.as_mut() {
            dialog.busy = true;
            dialog.error = None;
        }
        cx.notify();
        self.spawn_zip_import(root, zip_path, options, notebook_id, window, cx);
    }

    pub(super) fn spawn_zip_import(
        &mut self,
        root: PathBuf,
        zip_path: PathBuf,
        options: ImportOptions,
        notebook_id: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = zip_path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { archive::import_zip(&root, &target, &options) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(outcome) => {
                        let root = outcome.notebook.root.clone();
                        let name = outcome.notebook.display_name().to_string();
                        let new_id = outcome.notebook.marker.id;
                        this.zip = None;
                        if let Some(id) = notebook_id {
                            let dir = zip_path.parent().map(|parent| parent.display().to_string());
                            disk::enqueue(move || {
                                let mut prefs = NotebookPrefs::load(&id);
                                prefs.last_import_dir = dir;
                                if let Err(err) = prefs.save(&id) {
                                    tracing::warn!(error = %err, "保存笔记本偏好失败");
                                }
                            });
                        }
                        this.reload(cx);
                        this.reload_docs_under(&root, window, cx);
                        this.active_notebook = Some(new_id);
                        this.persist_session(cx);
                        let message = t!(
                            "zip.import_done",
                            name = name,
                            imported = outcome.imported,
                            same = outcome.skipped_identical,
                            both = outcome.kept_both
                        )
                        .to_string();
                        this.notify_success(message, cx);
                    }
                    Err(err) => {
                        if let Some(dialog) = this.zip.as_mut() {
                            dialog.busy = false;
                            dialog.error = Some(err.to_string());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Refresh open documents that live in a notebook folder that was just
    /// rewritten, and drop tabs whose files no longer exist.
    pub(super) fn reload_docs_under(
        &mut self,
        root: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets: Vec<(EntityId, PathBuf)> = self
            .docs
            .iter()
            .filter_map(|entity| {
                let doc = entity.read(cx);
                doc.path
                    .starts_with(root)
                    .then(|| (entity.entity_id(), doc.path.clone()))
            })
            .collect();
        if targets.is_empty() {
            return;
        }
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let reads = cx
                .background_executor()
                .spawn(async move {
                    targets
                        .into_iter()
                        .map(|(id, path)| {
                            let result = std::fs::read(&path).map(|bytes| {
                                let hash = mdwork_core::doc::sha256_hex(&bytes);
                                (mdwork_core::doc::decode(&bytes), hash)
                            });
                            (id, path, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| this.apply_reloaded_docs(reads, window, cx));
            });
        })
        .detach();
    }

    pub(super) fn apply_reloaded_docs(
        &mut self,
        reads: Vec<ReloadedDoc>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (id, _path, result) in reads {
            let Some(index) = self.docs.iter().position(|doc| doc.entity_id() == id) else {
                continue;
            };
            let doc = self.docs[index].clone();
            match result {
                Ok((decoded, hash)) => {
                    doc.update(cx, |doc, cx| doc.apply_disk(decoded, hash, window, cx));
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    self.drop_doc_at(index);
                }
                Err(err) => {
                    doc.update(cx, |doc, cx| {
                        doc.error = Some(err.to_string());
                        cx.notify();
                    });
                }
            }
        }
        cx.notify();
    }

    pub(super) fn render_zip_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.zip.as_ref() else {
            return div().into_any_element();
        };
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let overlay = cx.theme().overlay;
        let danger = cx.theme().danger;
        let label = |text: String| {
            div()
                .w(px(64.))
                .flex_shrink_0()
                .text_xs()
                .text_color(muted)
                .child(text)
        };

        let title = match dialog.mode {
            ZipDialogMode::Export => t!("zip.export_title").to_string(),
            ZipDialogMode::Import => t!("zip.import_title").to_string(),
        };

        let import_mode_button = |mode: ImportMode, key: &'static str, cx: &mut Context<Self>| {
            let active = dialog.import_mode == mode;
            Button::new(format!("zip-mode-{key}"))
                .ghost()
                .compact()
                .label(t!(key).to_string())
                .when(active, |button| button.primary())
                .disabled(dialog.busy)
                .on_click(cx.listener(move |this, _, _, cx| this.set_zip_import_mode(mode, cx)))
        };

        let policy_button = |policy: ConflictPolicy,
                             key: &'static str,
                             path: Option<String>,
                             cx: &mut Context<Self>| {
            let active = match &path {
                Some(path) => dialog.choices.get(path).copied().unwrap_or(dialog.policy) == policy,
                None => dialog.policy == policy,
            };
            let id = format!(
                "zip-policy-{key}-{policy:?}-{}",
                path.as_deref().unwrap_or("")
            );
            Button::new(id)
                .ghost()
                .compact()
                .label(t!(key).to_string())
                .when(active, |button| button.primary())
                .disabled(dialog.busy)
                .on_click(cx.listener(move |this, _, _, cx| match &path {
                    Some(path) => this.set_zip_choice(path, policy, cx),
                    None => this.set_zip_policy(policy, cx),
                }))
        };

        let mut body: Vec<AnyElement> = Vec::new();

        match dialog.mode {
            ZipDialogMode::Export => {
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("zip.notebook").to_string()))
                        .child(div().text_sm().child(dialog.notebook_name.clone()))
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("zip.file").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.path_input).w_full().small()),
                        )
                        .child(
                            Button::new("zip-export-browse")
                                .ghost()
                                .compact()
                                .label(t!("zip.browse").to_string())
                                .disabled(dialog.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_zip_export_path(window, cx)
                                })),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("zip.export_hint").to_string())
                        .into_any_element(),
                );
            }
            ZipDialogMode::Import => {
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("zip.file").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.path_input).w_full().small()),
                        )
                        .child(
                            Button::new("zip-import-browse")
                                .ghost()
                                .compact()
                                .label(t!("zip.browse").to_string())
                                .disabled(dialog.busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_zip_path(window, cx)
                                })),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("zip.mode").to_string()))
                        .child(import_mode_button(ImportMode::Merge, "zip.mode.merge", cx))
                        .child(import_mode_button(
                            ImportMode::Overwrite,
                            "zip.mode.overwrite",
                            cx,
                        ))
                        .child(import_mode_button(
                            ImportMode::Rename,
                            "zip.mode.rename",
                            cx,
                        ))
                        .into_any_element(),
                );
                if dialog.import_mode == ImportMode::Rename {
                    body.push(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(label(t!("zip.rename_to").to_string()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&dialog.rename_input).w_full().small()),
                            )
                            .into_any_element(),
                    );
                }

                let mut preview: Vec<AnyElement> = Vec::new();
                if dialog.plan_loading {
                    preview.push(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("zip.checking").to_string())
                            .into_any_element(),
                    );
                } else if let Some(error) = &dialog.plan_error {
                    preview.push(
                        div()
                            .text_xs()
                            .text_color(danger)
                            .child(t!("zip.preview_failed", message = error.clone()).to_string())
                            .into_any_element(),
                    );
                } else if let Some(plan) = &dialog.plan {
                    let folder_state = if plan.target_exists {
                        t!("zip.target_exists").to_string()
                    } else {
                        t!("zip.target_new").to_string()
                    };
                    preview.push(
                        div()
                            .h_flex()
                            .gap_2()
                            .text_xs()
                            .child(div().text_color(muted).child(t!("zip.target").to_string()))
                            .child(div().child(plan.folder_name.clone()))
                            .child(div().text_color(muted).child(folder_state))
                            .into_any_element(),
                    );
                    preview.push(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(
                                t!(
                                    "zip.summary",
                                    new = plan.new_files,
                                    same = plan.identical_files,
                                    conflicts = plan.conflicts.len(),
                                    size = format_bytes(plan.total_bytes)
                                )
                                .to_string(),
                            )
                            .into_any_element(),
                    );
                    if dialog.import_mode == ImportMode::Overwrite && plan.target_exists {
                        preview.push(
                            div()
                                .text_xs()
                                .text_color(danger)
                                .child(t!("zip.overwrite_warning").to_string())
                                .into_any_element(),
                        );
                    }
                    if dialog.import_mode == ImportMode::Merge && !plan.conflicts.is_empty() {
                        preview.push(
                            div()
                                .h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(t!("zip.default_policy").to_string()),
                                )
                                .child(policy_button(
                                    ConflictPolicy::KeepExisting,
                                    "zip.policy.keep_existing",
                                    None,
                                    cx,
                                ))
                                .child(policy_button(
                                    ConflictPolicy::UseImported,
                                    "zip.policy.use_imported",
                                    None,
                                    cx,
                                ))
                                .child(policy_button(
                                    ConflictPolicy::KeepBoth,
                                    "zip.policy.keep_both",
                                    None,
                                    cx,
                                ))
                                .into_any_element(),
                        );
                        let mut rows: Vec<AnyElement> = Vec::new();
                        for conflict in &plan.conflicts {
                            let Conflict { path, .. } = conflict;
                            rows.push(
                                div()
                                    .h_flex()
                                    .items_center()
                                    .gap_1()
                                    .py_1()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .child(path.clone()),
                                    )
                                    .child(policy_button(
                                        ConflictPolicy::KeepExisting,
                                        "zip.policy.short_keep",
                                        Some(path.clone()),
                                        cx,
                                    ))
                                    .child(policy_button(
                                        ConflictPolicy::UseImported,
                                        "zip.policy.short_use",
                                        Some(path.clone()),
                                        cx,
                                    ))
                                    .child(policy_button(
                                        ConflictPolicy::KeepBoth,
                                        "zip.policy.short_both",
                                        Some(path.clone()),
                                        cx,
                                    ))
                                    .into_any_element(),
                            );
                        }
                        preview.push(
                            div()
                                .v_flex()
                                .max_h(px(150.))
                                .id("zip-conflicts")
                                .overflow_y_scroll()
                                .gap_1()
                                .children(rows)
                                .into_any_element(),
                        );
                    }
                }
                body.push(
                    div()
                        .v_flex()
                        .gap_1()
                        .w_full()
                        .p_2()
                        .rounded_md()
                        .bg(cx.theme().list_hover)
                        .children(preview)
                        .into_any_element(),
                );
            }
        }

        let status = if let Some(error) = &dialog.error {
            Some((error.clone(), danger))
        } else if dialog.busy {
            Some((
                match dialog.mode {
                    ZipDialogMode::Export => t!("zip.exporting").to_string(),
                    ZipDialogMode::Import => t!("zip.importing").to_string(),
                },
                muted,
            ))
        } else {
            None
        };

        let can_start = !dialog.busy
            && match dialog.mode {
                ZipDialogMode::Export => !dialog.path_input.read(cx).value().trim().is_empty(),
                ZipDialogMode::Import => dialog.plan.is_some(),
            };
        let dialog_mode = dialog.mode;
        let start_label = match dialog_mode {
            ZipDialogMode::Export => t!("zip.export_start").to_string(),
            ZipDialogMode::Import => match dialog.import_mode {
                ImportMode::Merge => t!("zip.import_merge").to_string(),
                ImportMode::Overwrite if dialog.confirm_overwrite => {
                    t!("zip.confirm_overwrite").to_string()
                }
                ImportMode::Overwrite => t!("zip.import_overwrite").to_string(),
                ImportMode::Rename => t!("zip.import_rename").to_string(),
            },
        };

        let mut footer = div().h_flex().items_center().gap_2().min_h(px(24.)).child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .when_some(status, |element, (text, color)| {
                    element.text_color(color).child(text)
                }),
        );
        if dialog.busy {
            footer = footer.child(
                Button::new("zip-cancel")
                    .ghost()
                    .compact()
                    .label(t!("export.cancel").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.close_zip_dialog(cx))),
            );
        } else {
            let start_mode = dialog_mode;
            footer = footer
                .child(
                    Button::new("zip-dismiss")
                        .ghost()
                        .compact()
                        .label(t!("export.close").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.close_zip_dialog(cx))),
                )
                .child(
                    Button::new("zip-start")
                        .primary()
                        .compact()
                        .label(start_label)
                        .disabled(!can_start)
                        .on_click(cx.listener(move |this, _, window, cx| match start_mode {
                            ZipDialogMode::Export => this.begin_zip_export(cx),
                            ZipDialogMode::Import => this.begin_zip_import(window, cx),
                        })),
                );
        }

        div()
            .id("zip-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .child(
                div()
                    .id("zip-dialog")
                    .v_flex()
                    .w(px(660.))
                    .gap_3()
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().text_base().font_bold().child(title))
                            .child(
                                Button::new("zip-close")
                                    .ghost()
                                    .compact()
                                    .label("×")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.close_zip_dialog(cx)),
                                    ),
                            ),
                    )
                    .children(body)
                    .child(footer),
            )
            .into_any_element()
    }
}
