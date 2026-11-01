use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn current_workspace_root(&self) -> Option<PathBuf> {
        self.workspace
            .as_ref()
            .map(|workspace| workspace.root().to_path_buf())
    }

    pub(super) fn workspace_label(&self) -> String {
        self.current_workspace_root()
            .and_then(|root| self.workspace_registry.display_name(&root))
            .unwrap_or_else(|| t!("workspace.none").to_string())
    }

    pub(super) fn switch_workspace(
        &mut self,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(current) = self.current_workspace_root() {
            if mdwork_core::workspace::same_path(&current, &root) {
                self.workspace_menu = false;
                cx.notify();
                return;
            }
        }
        for doc in &self.docs {
            if doc.read(cx).dirty {
                doc.update(cx, |doc, cx| doc.save_now(cx));
            }
        }
        self.persist_session(cx);

        self.switch_generation += 1;
        let generation = self.switch_generation;
        self.reload_generation += 1;
        self.entries_generation += 1;
        self.trash_generation += 1;

        self.workspace = None;
        self._watcher = None;
        self.changed = None;
        self.config.set_workspace_root(&root);
        self.save_config(cx);
        self.workspace_registry.touch(&root);
        self.save_registry(cx);

        self.docs.clear();
        self.pending_close.clear();
        self.pending_delete = None;
        self.pending_jump = None;
        self.pending_recovery = None;
        self.opening.clear();
        self.deferred_paths.clear();
        self.pending_extra_paths.clear();
        self.pending_welcome = None;
        self.active_doc = None;
        self.notebooks.clear();
        self.sidebar_rows.clear();
        self.tree_selection = None;
        self.active_notebook = None;
        self.tree_scroll = UniformListScrollHandle::new();
        self.tree_drag = None;
        self.tree_click_suppressed = false;
        self.tab_scroll = ScrollHandle::new();
        self.tab_drag = None;
        self.delete_confirm = None;
        self.flash_rel = None;
        self.preview_cache = None;
        self.outline.clear();
        self.outline_cache = None;
        self.math.clear();
        self.math_pending.clear();
        self.image_paths.clear();
        self.images_pending.clear();
        self.bookmarks = Bookmarks::default();
        self.bookmarks_notebook = None;
        self.bookmarks_loading = None;
        self.trash.clear();
        self.trash_notebook = None;
        self.search_results.clear();
        self.search_status = SearchStatus::Idle;
        self.scan_issues.clear();
        self.renaming = None;
        self.entries_reload_deferred = false;
        self.create_hold = 0;
        self.export = None;
        self.zip = None;
        self.selection_sig = None;
        self.session = Session::default();
        self.initializing = true;
        self.startup_first_run = false;
        self.workspace_menu = false;
        self.search_input
            .update(cx, |state, cx| state.set_value("", window, cx));

        let handle = window.window_handle();
        let task_root = root.clone();
        cx.spawn(async move |this, cx| {
            let switched =
                cx.background_executor()
                    .spawn(async move {
                        let existed = task_root.is_dir();
                        let mut failures = Vec::new();
                        let workspace = match Workspace::open(&task_root) {
                            Ok(workspace) => {
                                if !existed {
                                    if let Err(err) = seed_workspace(&workspace) {
                                        failures.push((
                                            NoticeKind::Warning,
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
                                            load_notebook_entry(notebook, HashSet::new(), false)
                                        })
                                        .collect();
                                }
                                Err(err) => failures
                                    .push((NoticeKind::Error, format!("扫描工作区失败：{err}"))),
                            }
                        }
                        let session = load_workspace_session(&task_root, false);
                        Switched {
                            root: task_root,
                            workspace,
                            failures,
                            entries,
                            issues,
                            session,
                        }
                    })
                    .await;
            let _ = handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.finish_workspace_switch(generation, switched, window, cx)
                });
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn finish_workspace_switch(
        &mut self,
        generation: u64,
        switched: Switched,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.switch_generation != generation {
            return;
        }
        self.initializing = false;
        for (kind, text) in switched.failures {
            self.notices.push(NoticeItem {
                kind,
                text,
                read: false,
                created: Instant::now(),
            });
        }
        self.session = switched.session;
        if switched.workspace.is_some() {
            self.workspace_registry.touch(&switched.root);
            self.save_registry(cx);
        }
        self.workspace = switched.workspace;
        self.notebooks = switched.entries;
        self.scan_issues = switched.issues;
        self.active_notebook = self.notebooks.first().map(|entry| entry.notebook.marker.id);
        self.ensure_active_root_expanded();
        self.rebuild_sidebar_rows();
        self.start_watcher();
        self.queue_workspace_presence(cx);

        if self.config.mcp_http {
            self.mcp_live = self.mcp.start(self.config.mcp_port);
        }

        let label = self
            .workspace_registry
            .display_name(&switched.root)
            .unwrap_or_else(|| switched.root.display().to_string());
        if let Some(settings) = self.settings.as_mut() {
            settings
                .workspace_name
                .update(cx, |state, cx| state.set_value(label.clone(), window, cx));
        }
        self.restore_session(window, cx);
        self.notify_success(t!("workspace.switched", name = label).to_string(), cx);
        cx.notify();
    }

    pub(super) fn pick_workspace_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("workspace.pick_prompt").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = picked else {
                return;
            };
            // Classify the folder off the UI thread; `inspect` only stats.
            let inspect_path = path.clone();
            let kind = cx
                .background_executor()
                .spawn(async move { Workspace::inspect(&inspect_path) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.resolve_workspace_choice(path, kind, window, cx);
            });
        })
        .detach();
    }

    /// Opens a chosen folder directly when it is empty or already a workspace;
    /// otherwise asks for confirmation first (default "no").
    pub(super) fn resolve_workspace_choice(
        &mut self,
        path: PathBuf,
        kind: FolderKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            FolderKind::Foreign | FolderKind::Notebook => {
                self.workspace_confirm = Some(WorkspaceConfirm { path, kind });
                cx.notify();
            }
            _ => self.switch_workspace(path, window, cx),
        }
    }

    pub(super) fn render_workspace_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let label = self.workspace_label();
        let open = self.workspace_menu;
        let hover = cx.theme().list_hover;
        let active = cx.theme().list_active;
        let muted = cx.theme().muted_foreground;

        let trigger = div()
            .id("workspace-switcher")
            .h(px(22.))
            .max_w(px(260.))
            .px_2()
            .h_flex()
            .items_center()
            .gap_1()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .when(open, |element| element.bg(active))
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(|this, _, _, cx| {
                this.workspace_menu = !this.workspace_menu;
                if this.workspace_menu {
                    this.notice_panel = false;
                }
                cx.notify();
            }))
            .child(div().min_w_0().truncate().child(label))
            .child(Icon::new(IconName::ChevronDown).xsmall().text_color(muted));

        let mut popup = gpui_kit::base::Popup::new("workspace-popup", trigger)
            .anchor(Anchor::TopLeft)
            .offset(px(6.));
        if open {
            popup = popup.content(self.render_workspace_menu(cx));
        }
        titlebar_guard(popup)
    }

    pub(super) fn render_workspace_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let active_bg = cx.theme().list_active;
        let hover = cx.theme().list_hover;
        let warning = cx.theme().warning;
        let current = self.current_workspace_root();
        let entries = self.workspace_registry.sorted();

        let mut list = div().v_flex().w_full().gap_0p5();
        for (index, entry) in entries.iter().enumerate() {
            let root = entry.root();
            let is_active = current
                .as_ref()
                .is_some_and(|current| mdwork_core::workspace::same_path(current, &root));
            let missing = self.missing_workspaces.contains(&root);
            let name = entry.display_name();
            let path = entry.path.clone();
            let switch_root = root.clone();
            let switch_shell = cx.entity();

            let content = div()
                .id(format!("ws-row-{index}"))
                .flex_1()
                .min_w_0()
                .h_flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_md()
                .when(is_active, |element| element.bg(active_bg))
                .when(!missing, |element| {
                    element.cursor_pointer().hover(move |style| style.bg(hover))
                })
                .on_click(move |_, window, cx| {
                    if missing {
                        return;
                    }
                    switch_shell.update(cx, |this, cx| {
                        this.switch_workspace(switch_root.clone(), window, cx)
                    });
                })
                .child(
                    Icon::new(if is_active {
                        IconName::Check
                    } else {
                        IconName::BookOpen
                    })
                    .xsmall()
                    .text_color(if is_active { foreground } else { muted }),
                )
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .when(is_active, |element| element.font_medium())
                                .truncate()
                                .child(name),
                        )
                        .child(div().text_xs().text_color(muted).truncate().child(path))
                        .when(missing, |element| {
                            element.child(
                                div()
                                    .text_xs()
                                    .text_color(warning)
                                    .child(t!("workspace.missing").to_string()),
                            )
                        }),
                );

            let mut row = div()
                .h_flex()
                .items_center()
                .w_full()
                .gap_1()
                .child(content);
            if is_active {
                row = row.child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("workspace.current").to_string()),
                );
            } else {
                let rename_shell = cx.entity();
                row = row.child(
                    Button::new(format!("ws-rename-{index}"))
                        .ghost()
                        .compact()
                        .icon(IconName::Pencil)
                        .tooltip(t!("workspace.rename").to_string())
                        .on_click(move |_, window, cx| {
                            rename_shell.update(cx, |this, cx| {
                                this.workspace_menu = false;
                                this.open_settings(window, cx);
                                if let Some(settings) = this.settings.as_mut() {
                                    settings.tab = SettingsTab::Workspace;
                                }
                                cx.notify();
                            });
                        }),
                );
                let forget_shell = cx.entity();
                let forget_root = root.clone();
                let forget_name = entry.display_name();
                row = row.child(
                    Button::new(format!("ws-forget-{index}"))
                        .ghost()
                        .compact()
                        .icon(IconName::X)
                        .tooltip(t!("workspace.forget").to_string())
                        .on_click(move |_, _, cx| {
                            forget_shell.update(cx, |this, cx| {
                                if this.workspace_registry.forget(&forget_root) {
                                    this.save_registry(cx);
                                    this.notify_info(
                                        t!("workspace.forgotten", name = forget_name.clone())
                                            .to_string(),
                                        cx,
                                    );
                                }
                                cx.notify();
                            });
                        }),
                );
            }
            list = list.child(row);
        }
        if entries.is_empty() {
            list = list.child(empty_hint(t!("workspace.empty_list").to_string(), muted));
        }

        div()
            .id("workspace-menu")
            .v_flex()
            .w(px(340.))
            .p_2()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(popover)
            .text_color(foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.workspace_menu = false;
                cx.notify();
            }))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_bold()
                            .child(t!("workspace.title").to_string()),
                    )
                    .child(
                        Button::new("workspace-open-folder")
                            .ghost()
                            .compact()
                            .icon(IconName::FolderOpen)
                            .tooltip(t!("workspace.pick").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.pick_workspace_folder(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("workspace-list")
                    .v_flex()
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                Button::new("workspace-pick")
                    .ghost()
                    .compact()
                    .w_full()
                    .icon(IconName::FolderPlus)
                    .label(t!("workspace.pick").to_string())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.pick_workspace_folder(window, cx);
                    })),
            )
            .into_any_element()
    }
}
