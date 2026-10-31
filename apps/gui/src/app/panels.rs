use std::rc::Rc;

use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn set_panel(&mut self, panel: SidePanel, cx: &mut Context<Self>) {
        match self.config.panel_dock_for(panel.kind()) {
            PanelDock::Left => self.left_panel = panel,
            PanelDock::Right => self.right_panel = panel,
        }
        cx.notify();
    }

    /// Panels currently docked to `dock`, in canonical order.
    pub(super) fn panels_in_dock(&self, dock: PanelDock) -> Vec<SidePanel> {
        SidePanel::ALL
            .into_iter()
            .filter(|panel| self.config.panel_dock_for(panel.kind()) == dock)
            .collect()
    }

    /// The panel to display in `dock`: the last one the user selected there if
    /// it is still docked there, otherwise the first available one.
    pub(super) fn active_panel(&self, dock: PanelDock) -> Option<SidePanel> {
        let stored = match dock {
            PanelDock::Left => self.left_panel,
            PanelDock::Right => self.right_panel,
        };
        let panels = self.panels_in_dock(dock);
        if panels.contains(&stored) {
            Some(stored)
        } else {
            panels.first().copied()
        }
    }

    pub(super) fn set_panel_kind_dock(
        &mut self,
        panel: SidePanel,
        dock: PanelDock,
        cx: &mut Context<Self>,
    ) {
        if !self.config.set_panel_dock(panel.kind(), dock) {
            return;
        }
        match dock {
            PanelDock::Left => self.left_panel = panel,
            PanelDock::Right => self.right_panel = panel,
        }
        self.save_config(cx);
        cx.notify();
    }

    /// Docks every panel to the same side (quick uniform switch).
    pub(super) fn set_all_panel_docks(&mut self, dock: PanelDock, cx: &mut Context<Self>) {
        let mut changed = false;
        for panel in SidePanel::ALL {
            changed |= self.config.set_panel_dock(panel.kind(), dock);
        }
        if !changed {
            return;
        }
        self.save_config(cx);
        cx.notify();
    }

    pub(super) fn set_panel_arrangement(
        &mut self,
        arrangement: PanelArrangement,
        cx: &mut Context<Self>,
    ) {
        if self.config.panel_arrangement == arrangement {
            return;
        }
        self.config.panel_arrangement = arrangement;
        self.save_config(cx);
        cx.notify();
    }

    /// Applies a uniform width preset to both docks, clearing per-dock overrides.
    pub(super) fn set_panel_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
        if (self.config.panel_width - width).abs() < 0.5
            && self.config.left_width == 0.0
            && self.config.right_width == 0.0
        {
            return;
        }
        self.config.panel_width = width;
        self.config.left_width = 0.0;
        self.config.right_width = 0.0;
        self.save_config(cx);
        cx.notify();
    }

    pub(super) fn begin_panel_resize(
        &mut self,
        dock: PanelDock,
        mouse_x: f32,
        cx: &mut Context<Self>,
    ) {
        let width = self.config.width_for_dock(dock);
        self.panel_resize = Some((dock, mouse_x, width));
        cx.notify();
    }

    pub(super) fn update_panel_resize(&mut self, mouse_x: f32, cx: &mut Context<Self>) {
        let Some((dock, origin, start_width)) = self.panel_resize else {
            return;
        };
        let delta = if dock == PanelDock::Right {
            origin - mouse_x
        } else {
            mouse_x - origin
        };
        let width = (start_width + delta).clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
        if (self.config.width_for_dock(dock) - width).abs() < 0.5 {
            return;
        }
        self.config.set_width_for_dock(dock, width);
        cx.notify();
    }

    pub(super) fn end_panel_resize(&mut self, cx: &mut Context<Self>) {
        let Some((dock, _, _)) = self.panel_resize.take() else {
            return;
        };
        let width = self.config.width_for_dock(dock).round();
        self.config.set_width_for_dock(dock, width);
        self.save_config(cx);
        cx.notify();
    }

    /// Double-click on a divider resets that dock's width to the default.
    pub(super) fn reset_panel_width(&mut self, dock: PanelDock, cx: &mut Context<Self>) {
        self.config.set_width_for_dock(dock, 300.0);
        self.save_config(cx);
        cx.notify();
    }

    /// Renders one sidebar (left or right). Returns `None` when no panel is
    /// docked to that side, so the divider and sidebar are both omitted.
    pub(super) fn render_dock(
        &mut self,
        dock: PanelDock,
        show_workspace_path: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let active = self.active_panel(dock)?;
        let vertical = self.config.panel_arrangement == PanelArrangement::Vertical;
        let border = cx.theme().border;
        let sidebar_border = cx.theme().sidebar_border;
        let width = self.config.width_for_dock(dock);

        let body = match active {
            SidePanel::Notes => self.render_notes_panel(cx),
            SidePanel::Outline => self.render_outline_panel(cx),
            SidePanel::Search => self.render_search_panel(cx),
            SidePanel::Bookmarks => self.render_bookmarks_panel(cx),
            SidePanel::Trash => self.render_trash_panel(cx),
        };

        let mut content = div().v_flex().flex_1().h_full().min_w_0().min_h_0();
        if !vertical {
            content = content.child(self.panel_tab_row(dock, cx));
        }
        content = content.child(body);
        if show_workspace_path {
            let workspace_path = self
                .workspace
                .as_ref()
                .map(|workspace| workspace.root().display().to_string())
                .unwrap_or_default();
            content = content.child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(sidebar_border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(workspace_path),
            );
        }

        let mut root = div()
            .w(px(width))
            .h_full()
            .flex_none()
            .bg(cx.theme().sidebar)
            .border_color(border);
        root = if dock == PanelDock::Right {
            root.border_l_1()
        } else {
            root.border_r_1()
        };

        if vertical {
            let rail = self.panel_rail(dock, cx);
            let mut row = div().flex().flex_row().items_start().size_full();
            row = if dock == PanelDock::Right {
                row.child(content).child(rail)
            } else {
                row.child(rail).child(content)
            };
            Some(root.child(row).into_any_element())
        } else {
            Some(root.v_flex().child(content).into_any_element())
        }
    }

    pub(super) fn render_panel_handle(
        &mut self,
        dock: PanelDock,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dragging = self
            .panel_resize
            .is_some_and(|(active, _, _)| active == dock);
        let idle = cx.theme().background;
        let hover = cx.theme().border;
        let active = cx.theme().primary;
        div()
            .id(match dock {
                PanelDock::Left => "panel-resize-handle-left",
                PanelDock::Right => "panel-resize-handle-right",
            })
            .w(px(4.))
            .h_full()
            .flex_none()
            .cursor(CursorStyle::ResizeLeftRight)
            .bg(if dragging { active } else { idle })
            .hover(move |style| style.bg(hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    if event.click_count >= 2 {
                        this.reset_panel_width(dock, cx);
                    } else {
                        this.begin_panel_resize(dock, f32::from(event.position.x), cx);
                    }
                }),
            )
            .into_any_element()
    }

    /// Window mouse listeners may only be registered from a paint callback, so
    /// the active drag owns a zero-sized canvas instead of registering them in
    /// `render` (which runs during layout and panics in debug builds).
    pub(super) fn render_panel_resize_listeners(&self, cx: &mut Context<Self>) -> AnyElement {
        let shell = cx.entity().downgrade();
        canvas(
            |_bounds, _window, _cx| {},
            move |_bounds, _state, window, _cx| {
                let mover = shell.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _phase, _window, cx| {
                    let x = f32::from(event.position.x);
                    if let Some(shell) = mover.upgrade() {
                        let _ = shell.update(cx, |this, cx| this.update_panel_resize(x, cx));
                    }
                });
                window.on_mouse_event(move |_event: &MouseUpEvent, _phase, _window, cx| {
                    if let Some(shell) = shell.upgrade() {
                        let _ = shell.update(cx, |this, cx| this.end_panel_resize(cx));
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .w(px(0.))
        .h(px(0.))
        .into_any_element()
    }

    pub(super) fn panel_icon(panel: SidePanel) -> IconName {
        match panel {
            SidePanel::Notes => IconName::NotebookPen,
            SidePanel::Outline => IconName::ListTree,
            SidePanel::Search => IconName::Search,
            SidePanel::Bookmarks => IconName::Bookmark,
            SidePanel::Trash => IconName::Trash,
        }
    }

    pub(super) fn panel_tab_row(&self, dock: PanelDock, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .h(px(34.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().sidebar_border);
        for panel in self.panels_in_dock(dock) {
            let (label, id) = match panel {
                SidePanel::Notes => (t!("panel.notes").to_string(), "panel-notes"),
                SidePanel::Outline => (t!("panel.outline").to_string(), "panel-outline"),
                SidePanel::Search => (t!("panel.search").to_string(), "panel-search"),
                SidePanel::Bookmarks => (t!("panel.bookmarks").to_string(), "panel-bookmarks"),
                SidePanel::Trash => (t!("panel.trash").to_string(), "panel-trash"),
            };
            row = row.child(self.panel_tab(panel, label, id, cx));
        }
        row.into_any_element()
    }

    pub(super) fn panel_rail(&self, dock: PanelDock, cx: &mut Context<Self>) -> AnyElement {
        let mut rail = div()
            .v_flex()
            .w(px(40.))
            .h_full()
            .flex_shrink_0()
            .items_center()
            .py_2()
            .gap_1()
            .border_color(cx.theme().sidebar_border);
        rail = if dock == PanelDock::Right {
            rail.border_l_1()
        } else {
            rail.border_r_1()
        };
        let active_panel = self.active_panel(dock);
        for panel in self.panels_in_dock(dock) {
            let active = active_panel == Some(panel);
            let label = match panel {
                SidePanel::Notes => t!("panel.notes"),
                SidePanel::Outline => t!("panel.outline"),
                SidePanel::Search => t!("panel.search"),
                SidePanel::Bookmarks => t!("panel.bookmarks"),
                SidePanel::Trash => t!("panel.trash"),
            };
            rail = rail.child(
                Button::new(match panel {
                    SidePanel::Notes => "rail-notes",
                    SidePanel::Outline => "rail-outline",
                    SidePanel::Search => "rail-search",
                    SidePanel::Bookmarks => "rail-bookmarks",
                    SidePanel::Trash => "rail-trash",
                })
                .ghost()
                .compact()
                .icon(Self::panel_icon(panel))
                .tooltip(label.to_string())
                .when(active, |button| button.primary())
                .on_click(cx.listener(move |this, _, _, cx| this.set_panel(panel, cx))),
            );
        }
        rail.into_any_element()
    }

    pub(super) fn panel_tab(
        &self,
        panel: SidePanel,
        label: String,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.active_panel(self.config.panel_dock_for(panel.kind())) == Some(panel);
        let hover = cx.theme().list_hover;
        let color = if active {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        };
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .h(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(color)
            .bg(if active {
                cx.theme().list_active
            } else {
                cx.theme().transparent
            })
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| this.set_panel(panel, cx)))
            .child(
                Icon::new(Self::panel_icon(panel))
                    .xsmall()
                    .text_color(color),
            )
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    }

    pub(super) fn panel_header(&self, title: String, cx: &mut Context<Self>) -> AnyElement {
        let notebook = self
            .active_notebook
            .and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
            })
            .map(|entry| entry.notebook.display_name().to_string())
            .unwrap_or_default();
        div()
            .px_3()
            .pt_2()
            .pb_1()
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_xs().font_bold().child(title))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(notebook),
            )
            .into_any_element()
    }

    pub(super) fn render_notes_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex_1().min_h_0().v_flex().gap_1().px_2().pb_2();

        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .pt_1()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("sidebar.notebooks").to_string()),
                )
                .child(
                    Button::new("btn-new-notebook")
                        .ghost()
                        .compact()
                        .icon(IconName::Plus)
                        .tooltip(t!("sidebar.new_notebook").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.new_notebook(cx))),
                )
                .child(
                    Button::new("btn-rescan")
                        .ghost()
                        .compact()
                        .icon(IconName::RefreshCw)
                        .tooltip(t!("sidebar.refresh").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reload(cx);
                            this.persist_session(cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("btn-sidebar-open-notebook")
                        .ghost()
                        .compact()
                        .icon(IconName::Ellipsis)
                        .tooltip(t!("sidebar.open_notebook").to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_external_notebook_prompt(window, cx)
                        })),
                ),
        );

        if !self.scan_issues.is_empty() {
            let issues = self.scan_issues.clone();
            body = body.child(
                div()
                    .v_flex()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .bg(cx.theme().list_hover)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(cx.theme().danger)
                                    .child(
                                        t!("sidebar.scan_issues", count = issues.len()).to_string(),
                                    ),
                            )
                            .child(
                                Button::new("btn-dismiss-scan-issues")
                                    .ghost()
                                    .compact()
                                    .label("×")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.scan_issues.clear();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .children(issues.into_iter().take(4).map(|issue| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(issue)
                    })),
            );
        }

        if self.notebooks.is_empty() {
            body = body.child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("sidebar.empty").to_string()),
            );
        } else {
            body = body.child(self.render_sidebar_tree(cx));
        }

        body.into_any_element()
    }

    pub(super) fn render_outline_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.refresh_outline(cx);
        let items = Arc::new(self.outline.clone());
        let count = items.len();
        let has_doc = self.active_doc.is_some();
        let target = self.active_doc_entity().and_then(|doc| {
            let doc = doc.read(cx);
            doc.notebook_id.map(|id| (id, doc.rel_path.clone()))
        });
        let bookmarks = self.bookmarks.clone();
        let shell = cx.entity();
        let hover = cx.theme().list_hover;
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;

        let body: AnyElement = if !has_doc {
            empty_hint(t!("outline.no_doc").to_string(), muted)
        } else if items.is_empty() {
            empty_hint(t!("outline.empty").to_string(), muted)
        } else {
            uniform_list("outline-list", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let item = &items[index];
                        let level = item.level.clamp(1, 6);
                        let line = item.line;
                        let title = item.title.clone();
                        let (notebook_id, rel_path) = match &target {
                            Some((id, rel)) => (Some(*id), Some(rel.clone())),
                            None => (None, None),
                        };
                        let starred = rel_path
                            .as_deref()
                            .map(|rel| bookmarks.has_heading(rel, &title))
                            .unwrap_or(false);
                        let row_shell = shell.clone();
                        let star_shell = shell.clone();
                        let jump_rel = rel_path.clone();
                        let star_rel = rel_path.clone();
                        let star_title = title.clone();

                        div()
                            .id(format!("outline-row-{index}"))
                            .h(px(26.))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_1()
                            .pl(px(8.0 + (level - 1) as f32 * 12.0))
                            .pr_1()
                            .rounded_md()
                            .hover(move |style| style.bg(hover))
                            .child(
                                div()
                                    .id(format!("outline-jump-{index}"))
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        if let (Some(id), Some(rel)) =
                                            (notebook_id, jump_rel.clone())
                                        {
                                            row_shell.update(cx, |this, cx| {
                                                this.jump_to(id, &rel, line, window, cx)
                                            });
                                        }
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .text_color(foreground)
                                            .child(title),
                                    ),
                            )
                            .children(star_rel.map(|rel| {
                                Button::new(format!("outline-star-{index}"))
                                    .ghost()
                                    .compact()
                                    .label(if starred { "★" } else { "☆" })
                                    .tooltip(t!("outline.bookmark").to_string())
                                    .on_click(move |_, _, cx| {
                                        star_shell.update(cx, |this, cx| {
                                            this.toggle_heading_bookmark(
                                                &rel,
                                                &star_title,
                                                line,
                                                cx,
                                            )
                                        });
                                    })
                            }))
                    })
                    .collect::<Vec<_>>()
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .child(self.panel_header(t!("panel.outline").to_string(), cx))
            .child(body)
            .into_any_element()
    }

    pub(super) fn render_search_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let all = self.search_all;
        let results = Arc::new(self.search_results.clone());
        let count = results.len();
        let has_query = !self.search_input.read(cx).value().is_empty();
        let status = match &self.search_status {
            SearchStatus::Running => t!("search.running").to_string(),
            SearchStatus::Failed(err) => err.clone(),
            SearchStatus::Idle => {
                if !has_query {
                    t!("search.hint").to_string()
                } else if count == 0 {
                    t!("search.empty").to_string()
                } else {
                    t!("search.count", count = count).to_string()
                }
            }
        };
        let failed = matches!(self.search_status, SearchStatus::Failed(_));
        let status_color = if failed {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        let muted = cx.theme().muted_foreground;
        let running = matches!(self.search_status, SearchStatus::Running);
        let foreground = cx.theme().foreground;
        let hover = cx.theme().list_hover;

        let current_button = div()
            .id("scope-current")
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(if all { muted } else { foreground })
            .bg(if all {
                cx.theme().transparent
            } else {
                cx.theme().list_active
            })
            .on_click(cx.listener(|this, _, _, cx| this.set_search_scope(false, cx)))
            .child(t!("search.current").to_string());
        let all_button = div()
            .id("scope-all")
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(if all { foreground } else { muted })
            .bg(if all {
                cx.theme().list_active
            } else {
                cx.theme().transparent
            })
            .on_click(cx.listener(|this, _, _, cx| this.set_search_scope(true, cx)))
            .child(t!("search.all").to_string());

        let toggle = |id: &'static str,
                      key: &'static str,
                      active: bool,
                      option: SearchOption,
                      cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_xs()
                .text_color(if active { foreground } else { muted })
                .bg(if active {
                    cx.theme().list_active
                } else {
                    cx.theme().transparent
                })
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_search_option(option, cx)))
                .child(t!(key).to_string())
        };

        let mut toolbar = div()
            .px_2()
            .py_2()
            .v_flex()
            .gap_1()
            .child(Input::new(&self.search_input).cleanable(true).w_full())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(current_button)
                    .child(all_button)
                    .child(toggle(
                        "search-opt-case",
                        "search.opt.case",
                        self.search_case,
                        SearchOption::Case,
                        cx,
                    ))
                    .child(toggle(
                        "search-opt-regex",
                        "search.opt.regex",
                        self.search_regex,
                        SearchOption::Regex,
                        cx,
                    ))
                    .child(toggle(
                        "search-opt-filename",
                        "search.opt.filename",
                        self.search_filename,
                        SearchOption::Filename,
                        cx,
                    ))
                    .child(div().flex_1())
                    .children(running.then(|| {
                        Button::new("btn-search-cancel")
                            .ghost()
                            .compact()
                            .label("×")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.search_cancel.store(true, Ordering::Relaxed);
                                this.search_status = SearchStatus::Idle;
                                cx.notify();
                            }))
                    })),
            )
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .text_color(status_color)
                    .truncate()
                    .child(status),
            );

        let shell = cx.entity();

        let body: AnyElement = if results.is_empty() {
            div().flex_1().into_any_element()
        } else {
            uniform_list("search-results", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let hit = &results[index];
                        let shell = shell.clone();
                        let rel_path = hit.rel_path.clone();
                        let notebook_id = hit.notebook_id;
                        let line = hit.line;
                        let notebook_name = hit.notebook_name.clone();
                        let file_name = Path::new(&hit.rel_path)
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or_else(|| hit.rel_path.clone());

                        div()
                            .id(format!("search-row-{index}"))
                            .h(px(44.))
                            .w_full()
                            .px_2()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .on_click(move |_, window, cx| {
                                shell.update(cx, |this, cx| {
                                    this.jump_to(notebook_id, &rel_path, line, window, cx)
                                });
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().text_sm().truncate().child(file_name))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(format!(":{}", hit.line)),
                                    )
                                    .children(all.then(|| {
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .truncate()
                                            .child(notebook_name)
                                    })),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(hit.text.clone()),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        toolbar = toolbar.child(body);
        toolbar.flex_1().min_h_0().into_any_element()
    }

    pub(super) fn render_bookmarks_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let notebook_id = self.active_notebook;
        let entries = Arc::new(self.bookmarks.entries.clone());
        let count = entries.len();
        let shell = cx.entity();
        let hover = cx.theme().list_hover;
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        let body: AnyElement = if entries.is_empty() {
            empty_hint(t!("bookmark.empty").to_string(), muted)
        } else {
            uniform_list("bookmark-list", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let entry = &entries[index];
                        let rel_path = entry.rel_path.clone();
                        let heading = entry.heading.clone();
                        let subtitle = match &entry.heading {
                            Some(heading) => heading.clone(),
                            None => entry.rel_path.clone(),
                        };
                        let line = entry.line;
                        let row_shell = shell.clone();
                        let remove_shell = shell.clone();
                        let remove_rel = rel_path.clone();
                        let remove_heading = heading.clone();
                        let title = match (&entry.heading, &entry.title) {
                            (Some(_), note_title) => note_title.clone(),
                            (None, note_title) => note_title.clone(),
                        };

                        div()
                            .id(format!("bookmark-row-{index}"))
                            .h(px(46.))
                            .w_full()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded_md()
                            .hover(move |style| style.bg(hover))
                            .child(
                                div()
                                    .id(format!("bookmark-jump-{index}"))
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .on_click(move |_, window, cx| {
                                        if let Some(id) = notebook_id {
                                            row_shell.update(cx, |this, cx| {
                                                this.jump_to(id, &rel_path, line, window, cx)
                                            });
                                        }
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .v_flex()
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_sm()
                                                    .text_color(foreground)
                                                    .child(title),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(subtitle),
                                            ),
                                    ),
                            )
                            .child(
                                Button::new(format!("bookmark-remove-{index}"))
                                    .ghost()
                                    .compact()
                                    .label("×")
                                    .tooltip(t!("bookmark.remove").to_string())
                                    .on_click(move |_, _, cx| {
                                        remove_shell.update(cx, |this, cx| {
                                            this.remove_bookmark(
                                                &remove_rel,
                                                remove_heading.as_deref(),
                                                cx,
                                            )
                                        });
                                    }),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .child(self.panel_header(t!("panel.bookmarks").to_string(), cx))
            .child(body)
            .into_any_element()
    }

    pub(super) fn render_trash_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_trash(cx);
        let items = Arc::new(self.trash.clone());
        let count = items.len();
        let shell = cx.entity();
        let hover = cx.theme().list_hover;
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        let body: AnyElement = if items.is_empty() {
            empty_hint(t!("trash.empty").to_string(), muted)
        } else {
            uniform_list("trash-list", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let item = &items[index];
                        let id = item.id;
                        let name = if item.kind == EntryKind::Folder {
                            format!("{}/", item.rel_path)
                        } else {
                            item.rel_path.clone()
                        };
                        let age = age_label(item.deleted_at);
                        let restore_shell = shell.clone();

                        div()
                            .id(format!("trash-row-{index}"))
                            .h(px(46.))
                            .w_full()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded_md()
                            .hover(move |style| style.bg(hover))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .v_flex()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_sm()
                                            .text_color(foreground)
                                            .child(name),
                                    )
                                    .child(div().text_xs().text_color(muted).child(age)),
                            )
                            .child(
                                Button::new(format!("trash-restore-{index}"))
                                    .ghost()
                                    .compact()
                                    .label("↩")
                                    .tooltip(t!("trash.restore").to_string())
                                    .on_click(move |_, _, cx| {
                                        restore_shell
                                            .update(cx, |this, cx| this.restore_trash(id, cx));
                                    }),
                            )
                    })
                    .collect::<Vec<_>>()
            })
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .v_flex()
            .child(self.panel_header(t!("panel.trash").to_string(), cx))
            .child(body)
            .into_any_element()
    }

    pub(super) fn render_conflict(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let index = self.active_doc?;
        let (conflict, title) = {
            let doc = self.docs.get(index)?.read(cx);
            (doc.conflict?, doc.title())
        };
        let message = match conflict {
            DiskConflict::Modified => t!("conflict.modified", title = title).to_string(),
            DiskConflict::Removed => t!("conflict.removed", title = title).to_string(),
        };
        let primary = match conflict {
            DiskConflict::Modified => (t!("conflict.reload").to_string(), "btn-conflict-reload"),
            DiskConflict::Removed => (t!("conflict.recreate").to_string(), "btn-conflict-recreate"),
        };
        let secondary = match conflict {
            DiskConflict::Modified => (t!("conflict.keep").to_string(), "btn-conflict-keep"),
            DiskConflict::Removed => (t!("conflict.close").to_string(), "btn-conflict-close"),
        };

        Some(
            div()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().list_active)
                .text_sm()
                .child(div().flex_1().min_w_0().child(message))
                .child(
                    Button::new(primary.1)
                        .primary()
                        .compact()
                        .label(primary.0)
                        .on_click(cx.listener(move |this, _, window, cx| match conflict {
                            DiskConflict::Modified => this.reload_doc(index, window, cx),
                            DiskConflict::Removed => this.recreate_doc(index, cx),
                        })),
                )
                .child(
                    Button::new(secondary.1)
                        .ghost()
                        .compact()
                        .label(secondary.0)
                        .on_click(cx.listener(move |this, _, _, cx| match conflict {
                            DiskConflict::Modified => this.keep_doc(index, cx),
                            DiskConflict::Removed => this.close_doc(index, cx),
                        })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_sidebar_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = Arc::new(self.sidebar_rows.clone());
        let active_notebook = self.active_notebook;
        let active_rel = self.active_rel_path(cx);
        let flash_rel = self.flash_rel.clone();
        let renaming = self.renaming.clone();
        let renaming_notebook = self.renaming_notebook.clone();
        let sidebar_row_menu = self.sidebar_row_menu;
        let shell = cx.entity();
        let active_bg = cx.theme().list_active;
        let flash_bg = cx.theme().list_hover;
        let hover_bg = cx.theme().list_hover;
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let count = rows.len();
        let notes_label = t!("sidebar.notes").to_string();

        uniform_list("sidebar-tree", count, move |range, _window, _cx| {
            range
                .map(|index| {
                    let shell = shell.clone();
                    let group = format!("sb-row-{index}");
                    let menu_open = sidebar_row_menu == Some(index);
                    match &rows[index] {
                        SidebarRow::Notebook {
                            id,
                            name,
                            note_count,
                            expanded,
                        } => {
                            let notebook_id = *id;
                            let is_active = active_notebook == Some(notebook_id);
                            let count_label = format!("{note_count} {notes_label}");
                            let chevron = if *expanded { "▾" } else { "▸" };

                            if let Some((_, input)) = renaming_notebook
                                .as_ref()
                                .filter(|(id, _)| *id == notebook_id)
                            {
                                let input = input.clone();
                                let cancel_shell = shell.clone();
                                let escape_shell = shell.clone();
                                return div()
                                    .id(format!("notebook-rename-{index}"))
                                    .h(px(28.))
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .px_2()
                                    .rounded_md()
                                    .bg(active_bg)
                                    .on_key_down(move |event: &KeyDownEvent, _, cx| {
                                        if event.keystroke.key == "escape" {
                                            escape_shell.update(cx, |this, cx| {
                                                this.cancel_notebook_rename(cx)
                                            });
                                        }
                                    })
                                    .child(Input::new(&input).small().w_full())
                                    .child(
                                        Button::new(format!("notebook-rename-cancel-{index}"))
                                            .ghost()
                                            .compact()
                                            .label("×")
                                            .on_click(move |_, _, cx| {
                                                cancel_shell.update(cx, |this, cx| {
                                                    this.cancel_notebook_rename(cx)
                                                });
                                            }),
                                    )
                                    .into_any_element();
                            }

                            let export_shell = shell.clone();
                            let import_shell = shell.clone();
                            let create_shell = shell.clone();
                            let rename_shell = shell.clone();
                            let export_label = t!("zip.export_label").to_string();
                            let import_label = t!("zip.import_label").to_string();
                            let rename_label = t!("sidebar.rename_notebook").to_string();
                            let new_note_label = t!("sidebar.new_note").to_string();
                            let new_folder_label = t!("sidebar.new_folder").to_string();
                            let more_label = t!("tree.more").to_string();
                            let child_note_label = t!("tree.new_child_note").to_string();
                            let toggle_shell = shell.clone();
                            let activate_shell = shell.clone();
                            let plus_shell = shell.clone();
                            let menu_shell = shell;

                            let build_notebook_menu = Rc::new({
                                let export_shell = export_shell.clone();
                                let import_shell = import_shell.clone();
                                let rename_shell = rename_shell.clone();
                                let create_shell = create_shell.clone();
                                let new_note_label = new_note_label.clone();
                                let new_folder_label = new_folder_label.clone();
                                let rename_label = rename_label.clone();
                                let export_label = export_label.clone();
                                let import_label = import_label.clone();
                                move |menu: gpui_kit::component::menu::PopupMenu| {
                                    let export_shell = export_shell.clone();
                                    let import_shell = import_shell.clone();
                                    let rename_shell = rename_shell.clone();
                                    let create_shell = create_shell.clone();
                                    let note_shell = create_shell.clone();
                                    menu.item(PopupMenuItem::new(new_note_label.clone()).on_click(
                                        move |_, window, cx| {
                                            note_shell.update(cx, |this, cx| {
                                                this.new_note_in(notebook_id, None, window, cx)
                                            });
                                        },
                                    ))
                                    .item(PopupMenuItem::new(new_folder_label.clone()).on_click(
                                        move |_, window, cx| {
                                            create_shell.update(cx, |this, cx| {
                                                this.new_folder_in(notebook_id, None, window, cx)
                                            });
                                        },
                                    ))
                                    .separator()
                                    .item(PopupMenuItem::new(rename_label.clone()).on_click(
                                        move |_, window, cx| {
                                            rename_shell.update(cx, |this, cx| {
                                                let name = this
                                                    .notebooks
                                                    .iter()
                                                    .find(|e| e.notebook.marker.id == notebook_id)
                                                    .map(|e| e.notebook.display_name().to_string())
                                                    .unwrap_or_default();
                                                this.begin_notebook_rename(
                                                    notebook_id,
                                                    &name,
                                                    window,
                                                    cx,
                                                );
                                            });
                                        },
                                    ))
                                    .item(PopupMenuItem::new(export_label.clone()).on_click(
                                        move |_, window, cx| {
                                            export_shell.update(cx, |this, cx| {
                                                this.open_zip_export(notebook_id, window, cx)
                                            });
                                        },
                                    ))
                                    .item(
                                        PopupMenuItem::new(import_label.clone()).on_click(
                                            move |_, window, cx| {
                                                import_shell.update(cx, |this, cx| {
                                                    this.open_zip_import(window, cx)
                                                });
                                            },
                                        ),
                                    )
                                }
                            });

                            div()
                                .id(format!("notebook-row-{index}"))
                                .group(group.clone())
                                .relative()
                                .h(px(28.))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(if is_active {
                                    active_bg
                                } else {
                                    transparent_black()
                                })
                                .hover(move |style| style.bg(hover_bg))
                                .on_click(move |_, _, cx| {
                                    activate_shell.update(cx, |this, cx| {
                                        if this.active_notebook == Some(notebook_id) {
                                            this.toggle_notebook_root(notebook_id, cx);
                                        } else {
                                            this.activate_notebook(notebook_id, cx);
                                        }
                                    });
                                })
                                .context_menu({
                                    let build_notebook_menu = build_notebook_menu.clone();
                                    move |menu, _, _| (build_notebook_menu)(menu)
                                })
                                .child(
                                    div()
                                        .id(format!("notebook-chevron-{index}"))
                                        .w(px(12.))
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(muted)
                                        .cursor_pointer()
                                        .child(chevron)
                                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                            cx.stop_propagation();
                                            toggle_shell.update(cx, |this, cx| {
                                                this.toggle_notebook_root(notebook_id, cx);
                                            });
                                        }),
                                )
                                .child(div().flex_shrink_0().child(
                                    Icon::new(IconName::BookOpen).xsmall().text_color(muted),
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .font_bold()
                                        .truncate()
                                        .child(name.clone()),
                                )
                                .when(!menu_open, |row| {
                                    row.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(muted)
                                            .group_hover(group.clone(), |style| style.invisible())
                                            .child(count_label),
                                    )
                                })
                                .child(sidebar_row_trailing(
                                    index,
                                    &group,
                                    menu_open,
                                    true,
                                    more_label,
                                    child_note_label,
                                    menu_shell,
                                    plus_shell,
                                    notebook_id,
                                    None,
                                    build_notebook_menu,
                                ))
                                .into_any_element()
                        }
                        SidebarRow::Entry { notebook_id, row } => {
                            let notebook_id = *notebook_id;
                            let active = active_notebook == Some(notebook_id)
                                && active_rel.as_deref() == Some(row.rel_path.as_str());
                            let flashing = flash_rel.as_ref().is_some_and(|(id, path)| {
                                *id == notebook_id && path == &row.rel_path
                            });
                            let background = if active {
                                active_bg
                            } else if flashing {
                                flash_bg
                            } else {
                                transparent_black()
                            };
                            let is_folder = row.kind == EntryKind::Folder;
                            let chevron = if is_folder {
                                if row.expanded { "▾" } else { "▸" }
                            } else {
                                ""
                            };
                            let color = if is_folder || active {
                                foreground
                            } else {
                                muted
                            };
                            let rel_path = row.rel_path.clone();
                            let label = row.name.clone();
                            let depth = row.depth as f32;
                            let type_icon = if is_folder {
                                IconName::Folder
                            } else {
                                IconName::FileText
                            };

                            if let Some(renaming) = renaming.as_ref().filter(|renaming| {
                                renaming.notebook_id == notebook_id && renaming.rel_path == rel_path
                            }) {
                                let input = renaming.input.clone();
                                let cancel_shell = shell.clone();
                                let escape_shell = shell.clone();
                                return div()
                                    .id(format!("tree-rename-{index}"))
                                    .h(px(28.))
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .pl(px(10.0 + depth * 14.0))
                                    .pr_1()
                                    .rounded_md()
                                    .bg(active_bg)
                                    .on_key_down(move |event: &KeyDownEvent, _, cx| {
                                        if event.keystroke.key == "escape" {
                                            escape_shell
                                                .update(cx, |this, cx| this.cancel_rename(cx));
                                        }
                                    })
                                    .child(Input::new(&input).small().w_full())
                                    .child(
                                        Button::new(format!("tree-rename-cancel-{index}"))
                                            .ghost()
                                            .compact()
                                            .label("×")
                                            .on_click(move |_, _, cx| {
                                                cancel_shell
                                                    .update(cx, |this, cx| this.cancel_rename(cx));
                                            }),
                                    )
                                    .into_any_element();
                            }

                            let rename_shell = shell.clone();
                            let delete_shell = shell.clone();
                            let move_shell = shell.clone();
                            let create_shell = shell.clone();
                            let plus_shell = shell.clone();
                            let menu_shell = shell.clone();
                            let rename_rel = rel_path.clone();
                            let delete_rel = rel_path.clone();
                            let move_rel = rel_path.clone();
                            let create_rel = rel_path.clone();
                            let rename_label = t!("tree.rename").to_string();
                            let delete_label = t!("tree.delete").to_string();
                            let move_label = t!("tree.move").to_string();
                            let new_note_label = t!("tree.new_note").to_string();
                            let new_subfolder_label = t!("tree.new_subfolder").to_string();
                            let more_label = t!("tree.more").to_string();
                            let child_note_label = t!("tree.new_child_note").to_string();
                            let tooltip_rel = rel_path.clone();
                            let tooltip_folder = is_folder;
                            let tooltip_count = row.note_count;
                            let tooltip_created = row.created;
                            let tooltip_modified = row.modified;

                            let build_entry_menu = Rc::new({
                                let rename_shell = rename_shell.clone();
                                let delete_shell = delete_shell.clone();
                                let move_shell = move_shell.clone();
                                let create_shell = create_shell.clone();
                                let rename_rel = rename_rel.clone();
                                let delete_rel = delete_rel.clone();
                                let move_rel = move_rel.clone();
                                let create_rel = create_rel.clone();
                                let rename_label = rename_label.clone();
                                let delete_label = delete_label.clone();
                                let move_label = move_label.clone();
                                let new_note_label = new_note_label.clone();
                                let new_subfolder_label = new_subfolder_label.clone();
                                move |menu: gpui_kit::component::menu::PopupMenu| {
                                    let rename_shell = rename_shell.clone();
                                    let delete_shell = delete_shell.clone();
                                    let move_shell = move_shell.clone();
                                    let create_shell = create_shell.clone();
                                    let rename_rel = rename_rel.clone();
                                    let delete_rel = delete_rel.clone();
                                    let move_rel = move_rel.clone();
                                    let create_rel = create_rel.clone();
                                    let menu = if is_folder {
                                        let note_shell = create_shell.clone();
                                        let note_rel = create_rel.clone();
                                        menu.item(
                                            PopupMenuItem::new(new_note_label.clone()).on_click(
                                                move |_, window, cx| {
                                                    note_shell.update(cx, |this, cx| {
                                                        this.new_note_in(
                                                            notebook_id,
                                                            Some(note_rel.clone()),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                },
                                            ),
                                        )
                                        .item(
                                            PopupMenuItem::new(new_subfolder_label.clone())
                                                .on_click(move |_, window, cx| {
                                                    create_shell.update(cx, |this, cx| {
                                                        this.new_folder_in(
                                                            notebook_id,
                                                            Some(create_rel.clone()),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }),
                                        )
                                        .separator()
                                    } else {
                                        menu
                                    };
                                    menu.item(PopupMenuItem::new(rename_label.clone()).on_click(
                                        move |_, window, cx| {
                                            rename_shell.update(cx, |this, cx| {
                                                this.begin_rename(
                                                    notebook_id,
                                                    &rename_rel,
                                                    window,
                                                    cx,
                                                )
                                            });
                                        },
                                    ))
                                    .item(PopupMenuItem::new(move_label.clone()).on_click(
                                        move |_, window, cx| {
                                            move_shell.update(cx, |this, cx| {
                                                this.begin_move(notebook_id, &move_rel, window, cx)
                                            });
                                        },
                                    ))
                                    .separator()
                                    .item(
                                        PopupMenuItem::new(delete_label.clone()).on_click(
                                            move |_, _, cx| {
                                                delete_shell.update(cx, |this, cx| {
                                                    this.delete_entry(
                                                        notebook_id,
                                                        delete_rel.clone(),
                                                        cx,
                                                    )
                                                });
                                            },
                                        ),
                                    )
                                }
                            });

                            div()
                                .id(format!("tree-row-{index}"))
                                .group(group.clone())
                                .relative()
                                .h(px(26.))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap_1()
                                .pl(px(10.0 + depth * 14.0))
                                .pr_2()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(background)
                                .hover(move |style| style.bg(hover_bg))
                                .tooltip(move |window, cx| {
                                    let path = tooltip_rel.clone();
                                    let notes = tooltip_folder.then(|| {
                                        t!("tree.contains_notes", count = tooltip_count).to_string()
                                    });
                                    let created = (tooltip_created > 0).then(|| {
                                        t!(
                                            "tree.created_at",
                                            time = local_datetime_label(tooltip_created)
                                        )
                                        .to_string()
                                    });
                                    let updated = (tooltip_modified > 0).then(|| {
                                        t!(
                                            "tree.updated_at",
                                            time = local_datetime_label(tooltip_modified)
                                        )
                                        .to_string()
                                    });
                                    Tooltip::element(move |_, cx| {
                                        let muted = cx.theme().muted_foreground;
                                        let mut column = div()
                                            .flex()
                                            .flex_col()
                                            .gap_0p5()
                                            .child(div().child(path.clone()));
                                        if let Some(notes) = notes.clone() {
                                            column = column.child(
                                                div().text_xs().text_color(muted).child(notes),
                                            );
                                        }
                                        if let Some(created) = created.clone() {
                                            column = column.child(
                                                div().text_xs().text_color(muted).child(created),
                                            );
                                        }
                                        if let Some(updated) = updated.clone() {
                                            column = column.child(
                                                div().text_xs().text_color(muted).child(updated),
                                            );
                                        }
                                        column
                                    })
                                    .build(window, cx)
                                })
                                .on_click(move |_, window, cx| {
                                    shell.update(cx, |this, cx| {
                                        if is_folder {
                                            this.toggle_folder(notebook_id, &rel_path, cx);
                                        } else {
                                            this.open_note_by_id(
                                                notebook_id,
                                                &rel_path,
                                                window,
                                                cx,
                                            );
                                        }
                                    });
                                })
                                .context_menu({
                                    let build_entry_menu = build_entry_menu.clone();
                                    move |menu, _, _| (build_entry_menu)(menu)
                                })
                                .child(
                                    div()
                                        .w(px(12.))
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(chevron),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .child(Icon::new(type_icon).xsmall().text_color(muted)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .text_color(color)
                                        .truncate()
                                        .child(label),
                                )
                                .child(sidebar_row_trailing(
                                    index,
                                    &group,
                                    menu_open,
                                    is_folder,
                                    more_label,
                                    child_note_label,
                                    menu_shell,
                                    plus_shell,
                                    notebook_id,
                                    is_folder.then(|| create_rel.clone()),
                                    build_entry_menu,
                                ))
                                .into_any_element()
                        }
                    }
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.tree_scroll)
        .flex_1()
        .min_h_0()
        .into_any_element()
    }
}

/// Hover-revealed trailing `…` / `+` controls for a Notes-panel tree row.
fn sidebar_row_trailing(
    index: usize,
    group: &str,
    menu_open: bool,
    can_add_child: bool,
    more_label: String,
    child_note_label: String,
    menu_shell: Entity<AppShell>,
    plus_shell: Entity<AppShell>,
    notebook_id: Uuid,
    parent: Option<String>,
    build_menu: Rc<
        dyn Fn(gpui_kit::component::menu::PopupMenu) -> gpui_kit::component::menu::PopupMenu,
    >,
) -> AnyElement {
    let more = Button::new(format!("tree-more-{index}"))
        .ghost()
        .compact()
        .icon(IconName::Ellipsis)
        .tooltip(more_label)
        .dropdown_menu_with_anchor(Anchor::TopRight, {
            let build_menu = build_menu.clone();
            move |menu, _, _| (build_menu)(menu)
        })
        .on_open_change(move |open, _, cx| {
            menu_shell.update(cx, |this, cx| {
                this.sidebar_row_menu = if *open { Some(index) } else { None };
                cx.notify();
            });
        });

    // Absolute so the idle note-count can sit flush-right; hover actions
    // overlay the same trailing slot instead of pushing the count left.
    let mut actions = div()
        .id(format!("tree-actions-{index}"))
        .absolute()
        .right_0()
        .h_full()
        .h_flex()
        .items_center()
        .gap_0p5()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(!menu_open, |el| {
            el.invisible()
                .group_hover(group.to_string(), |style| style.visible())
        })
        .child(more);

    if can_add_child {
        let parent = parent.clone();
        actions = actions.child(
            Button::new(format!("tree-add-{index}"))
                .ghost()
                .compact()
                .icon(IconName::Plus)
                .tooltip(child_note_label)
                .on_click(move |_, window, cx| {
                    plus_shell.update(cx, |this, cx| {
                        this.new_note_in(notebook_id, parent.clone(), window, cx);
                    });
                }),
        );
    }

    actions.into_any_element()
}
