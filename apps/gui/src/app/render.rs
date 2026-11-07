use super::previewer::PreviewCopyKind;
use super::util::*;
use super::*;
use gpui_kit::base::TextSelection;

impl AppShell {
    pub(super) fn render_palette(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(palette) = self.palette.as_ref() else {
            return div().into_any_element();
        };
        let input = palette.input.clone();
        let muted = cx.theme().muted_foreground;
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let active = cx.theme().list_active;
        let hover = cx.theme().list_hover;

        let rows: Vec<(usize, String, String, bool)> = palette
            .filtered
            .iter()
            .take(10)
            .enumerate()
            .map(|(row, index)| {
                let item = &palette.items[*index];
                (
                    row,
                    item.label.clone(),
                    item.hint.clone(),
                    row == palette.selected,
                )
            })
            .collect();
        let shell = cx.entity();
        let empty = rows.is_empty();

        let mut list = div().v_flex().gap_0p5();
        for (row, label, hint, selected) in rows {
            let click_shell = shell.clone();
            list = list.child(
                div()
                    .id(format!("palette-row-{row}"))
                    .h(px(32.))
                    .w_full()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if selected {
                        active
                    } else {
                        cx.theme().transparent
                    })
                    .hover(move |style| style.bg(hover))
                    .on_click(move |_, window, cx| {
                        click_shell.update(cx, |this, cx| {
                            if let Some(palette) = this.palette.as_mut() {
                                palette.selected = row;
                            }
                            this.run_palette_selected(window, cx);
                        });
                    })
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(label))
                    .child(div().text_xs().text_color(muted).child(hint)),
            );
        }
        if empty {
            list = list.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("palette.empty").to_string()),
            );
        }

        let key_shell = shell.clone();
        div()
            .id("palette-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(96.))
            .bg(overlay)
            .occlude()
            .on_mouse_down(MouseButton::Left, {
                let shell = shell.clone();
                move |_, _, cx| {
                    shell.update(cx, |this, cx| this.close_palette(cx));
                }
            })
            .on_key_down(move |event: &KeyDownEvent, _window, cx| {
                let key = event.keystroke.key.clone();
                key_shell.update(cx, |this, cx| match key.as_str() {
                    "escape" => this.close_palette(cx),
                    "up" => this.palette_step(-1, cx),
                    "down" => this.palette_step(1, cx),
                    _ => {}
                });
            })
            .child(
                div()
                    .id("palette-dialog")
                    .v_flex()
                    .gap_2()
                    .w(px(640.))
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Input::new(&input).w_full())
                    .child(list),
            )
            .into_any_element()
    }

    pub(super) fn render_recovery(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.recovery.as_ref() else {
            return div().into_any_element();
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let rows: Vec<(usize, String, String)> = dialog
            .entries
            .iter()
            .take(8)
            .enumerate()
            .map(|(index, snapshot)| {
                let title = snapshot.display_name();
                let notebook = snapshot
                    .notebook_name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| t!("recovery.external").to_string());
                let subtitle = format!("{} · {}", notebook, format_age(snapshot.age_seconds(now)));
                (index, title, subtitle)
            })
            .collect();
        let more = dialog.entries.len().saturating_sub(rows.len());
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().list_hover;
        let shell = cx.entity();

        let mut list = div().v_flex().gap_1();
        for (index, title, subtitle) in rows {
            let restore_shell = shell.clone();
            let discard_shell = shell.clone();
            list = list.child(
                div()
                    .id(format!("recovery-row-{index}"))
                    .h_flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .hover(move |style| style.bg(hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .v_flex()
                            .child(div().text_sm().truncate().child(title))
                            .child(div().text_xs().text_color(muted).truncate().child(subtitle)),
                    )
                    .child(
                        Button::new(format!("recovery-restore-{index}"))
                            .primary()
                            .compact()
                            .label(t!("recovery.restore").to_string())
                            .on_click(move |_, window, cx| {
                                restore_shell.update(cx, |this, cx| {
                                    this.restore_recovery(index, window, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new(format!("recovery-discard-{index}"))
                            .ghost()
                            .compact()
                            .label(t!("recovery.discard").to_string())
                            .on_click(move |_, _, cx| {
                                discard_shell
                                    .update(cx, |this, cx| this.discard_recovery(index, cx));
                            }),
                    ),
            );
        }
        if more > 0 {
            list = list.child(
                div()
                    .px_3()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("recovery.more", count = more).to_string()),
            );
        }

        div()
            .id("recovery-overlay")
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
                    .v_flex()
                    .gap_3()
                    .w(px(560.))
                    .p_5()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_base()
                                    .font_bold()
                                    .child(t!("recovery.title").to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t!("recovery.desc").to_string()),
                            ),
                    )
                    .child(list)
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("recovery-discard-all")
                                    .ghost()
                                    .compact()
                                    .label(t!("recovery.discard_all").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.discard_all_recovery(cx)),
                                    ),
                            )
                            .child(
                                Button::new("recovery-close")
                                    .compact()
                                    .label(t!("common.close").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.close_recovery(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_prompt(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prompt) = self.prompt.as_ref() else {
            return div().into_any_element();
        };
        let input = prompt.input.clone();
        let title = prompt.title.clone();
        let hint = prompt.hint.clone();
        let error = prompt.error.clone();
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;

        div()
            .id("prompt-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .on_mouse_down(MouseButton::Left, {
                let shell = cx.entity();
                move |_, _, cx| {
                    shell.update(cx, |this, cx| this.close_prompt(cx));
                }
            })
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .w(px(520.))
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().text_base().font_bold().child(title))
                    .child(div().text_xs().text_color(muted).child(hint))
                    .child(Input::new(&input).w_full())
                    .children(error.map(|error| div().text_xs().text_color(danger).child(error)))
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("prompt-cancel")
                                    .ghost()
                                    .compact()
                                    .label(t!("common.cancel").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| this.close_prompt(cx))),
                            )
                            .child(
                                Button::new("prompt-ok")
                                    .primary()
                                    .compact()
                                    .label(t!("common.ok").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| this.submit_prompt(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_move_picker(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(picker) = self.move_picker.as_ref() else {
            return div().into_any_element();
        };
        let source_notebook_id = picker.source_notebook_id;
        let source_rel = picker.source_rel.clone();
        let source_kind = picker.source_kind;
        let selected = picker.selected.clone();
        let root_expanded = picker.root_expanded.clone();
        let expanded = picker.expanded.clone();
        let error = picker.error.clone();
        let can_confirm = selected.as_ref().is_some_and(|(id, folder)| {
            self.move_destination_allowed(
                source_notebook_id,
                &source_rel,
                source_kind,
                *id,
                folder.as_deref(),
            )
        });

        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let active_bg = cx.theme().list_active;
        let hover_bg = cx.theme().list_hover;

        let mut rows: Vec<AnyElement> = Vec::new();
        for (nb_index, entry) in self.notebooks.iter().enumerate() {
            let notebook_id = entry.notebook.marker.id;
            let name = entry.notebook.display_name().to_string();
            let is_root_open = root_expanded.contains(&notebook_id);
            let is_selected = matches!(
                selected,
                Some((id, None)) if id == notebook_id
            );
            let root_allowed = self.move_destination_allowed(
                source_notebook_id,
                &source_rel,
                source_kind,
                notebook_id,
                None,
            );
            let chevron = if is_root_open { "▾" } else { "▸" };
            let shell = cx.entity();
            let toggle_shell = shell.clone();
            let select_shell = shell.clone();
            rows.push(
                div()
                    .id(format!("move-nb-{nb_index}"))
                    .h(px(28.))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if is_selected {
                        active_bg
                    } else {
                        transparent_black()
                    })
                    .hover(move |style| {
                        if root_allowed {
                            style.bg(hover_bg)
                        } else {
                            style
                        }
                    })
                    .opacity(if root_allowed { 1.0 } else { 0.45 })
                    .on_click(move |_, _, cx| {
                        if root_allowed {
                            select_shell.update(cx, |this, cx| {
                                this.select_move_destination(notebook_id, None, cx);
                            });
                        }
                    })
                    .child(
                        div()
                            .id(format!("move-nb-chevron-{nb_index}"))
                            .w(px(12.))
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(muted)
                            .cursor_pointer()
                            .child(chevron)
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cx.stop_propagation();
                                toggle_shell.update(cx, |this, cx| {
                                    this.toggle_move_picker_root(notebook_id, cx);
                                });
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(Icon::new(IconName::BookOpen).xsmall().text_color(muted)),
                    )
                    .child(div().flex_1().min_w_0().font_bold().truncate().child(name))
                    .into_any_element(),
            );

            if !is_root_open {
                continue;
            }
            let folder_expanded = expanded.get(&notebook_id).cloned().unwrap_or_default();
            let folder_tree: Vec<Entry> = entry
                .tree
                .iter()
                .filter(|item| item.kind == EntryKind::Folder)
                .cloned()
                .collect();
            let folder_rows = build_rows(&folder_tree, &folder_expanded, &entry.order);
            for (row_index, row) in folder_rows.iter().enumerate() {
                let rel_path = row.rel_path.clone();
                let label = row.name.clone();
                let depth = row.depth as f32;
                let is_open = row.expanded;
                let is_selected = matches!(
                    &selected,
                    Some((id, Some(folder))) if *id == notebook_id && folder == &rel_path
                );
                let allowed = self.move_destination_allowed(
                    source_notebook_id,
                    &source_rel,
                    source_kind,
                    notebook_id,
                    Some(rel_path.as_str()),
                );
                let has_children = folder_tree.iter().any(|item| {
                    item.rel_path
                        .rsplit_once('/')
                        .is_some_and(|(parent, _)| parent == rel_path)
                });
                let chevron = if has_children {
                    if is_open { "▾" } else { "▸" }
                } else {
                    ""
                };
                let shell = cx.entity();
                let toggle_shell = shell.clone();
                let select_shell = shell;
                let toggle_rel = rel_path.clone();
                let select_rel = rel_path.clone();
                rows.push(
                    div()
                        .id(format!("move-folder-{nb_index}-{row_index}"))
                        .h(px(26.))
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl(px(10.0 + (depth + 1.0) * 14.0))
                        .pr_2()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(if is_selected {
                            active_bg
                        } else {
                            transparent_black()
                        })
                        .hover(
                            move |style| {
                                if allowed { style.bg(hover_bg) } else { style }
                            },
                        )
                        .opacity(if allowed { 1.0 } else { 0.45 })
                        .on_click(move |_, _, cx| {
                            if allowed {
                                select_shell.update(cx, |this, cx| {
                                    this.select_move_destination(
                                        notebook_id,
                                        Some(select_rel.clone()),
                                        cx,
                                    );
                                });
                            }
                        })
                        .child(
                            div()
                                .id(format!("move-folder-chevron-{nb_index}-{row_index}"))
                                .w(px(12.))
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(muted)
                                .when(has_children, |el| {
                                    el.cursor_pointer().on_mouse_down(
                                        MouseButton::Left,
                                        move |_, _, cx| {
                                            cx.stop_propagation();
                                            toggle_shell.update(cx, |this, cx| {
                                                this.toggle_move_picker_folder(
                                                    notebook_id,
                                                    &toggle_rel,
                                                    cx,
                                                );
                                            });
                                        },
                                    )
                                })
                                .child(chevron),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .child(Icon::new(IconName::Folder).xsmall().text_color(muted)),
                        )
                        .child(div().flex_1().min_w_0().text_sm().truncate().child(label))
                        .into_any_element(),
                );
            }
        }

        div()
            .id("move-picker-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .on_mouse_down(MouseButton::Left, {
                let shell = cx.entity();
                move |_, _, cx| {
                    shell.update(cx, |this, cx| this.close_move_picker(cx));
                }
            })
            .child(
                div()
                    .v_flex()
                    .gap_2()
                    .w(px(480.))
                    .max_h(px(520.))
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .text_base()
                            .font_bold()
                            .child(t!("tree.move_title").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("tree.move_hint", path = source_rel.as_str()).to_string()),
                    )
                    .child(
                        div()
                            .id("move-picker-list")
                            .flex_1()
                            .min_h(px(240.))
                            .max_h(px(360.))
                            .overflow_y_scroll()
                            .border_1()
                            .border_color(border)
                            .rounded_md()
                            .p_1()
                            .children(rows),
                    )
                    .children(error.map(|error| div().text_xs().text_color(danger).child(error)))
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("move-picker-cancel")
                                    .ghost()
                                    .compact()
                                    .label(t!("common.cancel").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.close_move_picker(cx)),
                                    ),
                            )
                            .child(
                                Button::new("move-picker-ok")
                                    .primary()
                                    .compact()
                                    .disabled(!can_confirm)
                                    .label(t!("tree.move_confirm").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_move_picker(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_workspace_confirm(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(confirm) = self.workspace_confirm.as_ref() else {
            return div().into_any_element();
        };
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;

        let message = match confirm.kind {
            FolderKind::Notebook => t!("workspace.confirm_notebook").to_string(),
            _ => t!("workspace.confirm_foreign").to_string(),
        };
        let path = confirm.path.display().to_string();

        div()
            .id("workspace-confirm-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(overlay)
            .occlude()
            .on_mouse_down(MouseButton::Left, {
                let shell = cx.entity();
                move |_, _, cx| {
                    shell.update(cx, |this, cx| this.cancel_workspace_confirm(cx));
                }
            })
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .w(px(520.))
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::TriangleAlert).text_color(warning))
                            .child(
                                div()
                                    .text_base()
                                    .font_bold()
                                    .child(t!("workspace.confirm_title").to_string()),
                            ),
                    )
                    .child(div().text_sm().child(message))
                    .child(div().text_xs().text_color(muted).truncate().child(path))
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("workspace-confirm-yes")
                                    .ghost()
                                    .compact()
                                    .label(t!("workspace.confirm_yes").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.confirm_workspace(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("workspace-confirm-no")
                                    .primary()
                                    .compact()
                                    .label(t!("workspace.confirm_no").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_workspace_confirm(cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let shell = cx.entity();
        let visible = self.visible_tabs(cx);
        let rename_label = t!("tree.rename").to_string();
        let close_label = t!("tabs.close").to_string();
        let close_others_label = t!("tabs.close_others").to_string();
        let close_left_label = t!("tabs.close_left").to_string();
        let close_right_label = t!("tabs.close_right").to_string();
        let close_all_label = t!("tabs.close_all").to_string();
        let adopt_label = t!("external.save_into").to_string();
        let drag_from = self.tab_drag.as_ref().map(|d| d.from);
        let insert_at = self
            .tab_drag
            .as_ref()
            .filter(|d| d.started)
            .map(|d| d.insert_at);
        let primary = cx.theme().primary;
        // 关闭叉号要有自己的悬停/按下反馈：ghost 按钮的悬停色与标签本身的
        // 悬停色几乎相同，叠在一起看不出变化，所以单独用一层危险色提示。
        let close_fg = cx.theme().muted_foreground;
        let close_hover_bg = cx.theme().danger.opacity(0.16);
        let close_hover_fg = cx.theme().danger;
        let close_pressed_bg = cx.theme().danger.opacity(0.28);
        let tab_bounds = self.tab_bounds.clone();

        // Clear stale bounds; each tab re-registers on paint.
        if let Ok(mut bounds) = tab_bounds.lock() {
            bounds.clear();
        }

        let mut tabs_row = div()
            .id("tabs-inner")
            .h_flex()
            .items_center()
            .gap_1()
            .overflow_x_scroll()
            .lock_scroll_axis()
            .track_scroll(&self.tab_scroll);

        // 使用每个标签稳定的实体 id。关闭标签会导致下标整体前移，
        // 因此元素 id 与菜单目标都不能由下标推导。
        let tab_ids: Vec<EntityId> = visible
            .iter()
            .map(|&index| self.docs[index].entity_id())
            .collect();

        for (position, &index) in visible.iter().enumerate() {
            let doc = self.docs[index].read(cx);
            let doc_id = self.docs[index].entity_id();
            let active = self.active_doc == Some(index);
            let is_external = doc.notebook_id.is_none();
            let title = if is_external {
                format!("外部 · {}", doc.rel_path)
            } else {
                doc.title()
            };
            let dirty = doc.dirty;
            let doc_label = if doc.conflict.is_some() {
                format!("⚠ {title}")
            } else if dirty {
                format!("● {title}")
            } else {
                title
            };
            let label_color = if active {
                cx.theme().foreground
            } else {
                cx.theme().muted_foreground
            };
            let hover = cx.theme().list_hover;
            let left_tabs = tab_ids[..position].to_vec();
            let right_tabs = tab_ids[position + 1..].to_vec();
            let other_tabs: Vec<EntityId> = tab_ids
                .iter()
                .copied()
                .filter(|&seen| seen != doc_id)
                .collect();
            let rename_target = doc.notebook_id.map(|id| (id, doc.rel_path.clone()));
            let can_rename = rename_target.is_some();
            let shell = shell.clone();
            let visible_tabs = tab_ids.clone();
            let rename_label = rename_label.clone();
            let close_label = close_label.clone();
            let close_button_label = close_label.clone();
            let close_others_label = close_others_label.clone();
            let close_left_label = close_left_label.clone();
            let close_right_label = close_right_label.clone();
            let close_all_label = close_all_label.clone();
            let adopt_label = adopt_label.clone();
            let dragging = drag_from == Some(index);
            let show_insert_before = insert_at == Some(position);

            if show_insert_before {
                tabs_row = tabs_row.child(
                    div()
                        .w(px(2.))
                        .h(px(22.))
                        .rounded_sm()
                        .bg(primary)
                        .flex_none(),
                );
            }

            let bounds_store = tab_bounds.clone();
            tabs_row = tabs_row.child(
                div()
                    .id(format!("tab-{doc_id:?}"))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor(if dragging {
                        CursorStyle::ClosedHand
                    } else {
                        CursorStyle::PointingHand
                    })
                    .opacity(if dragging { 0.55 } else { 1.0 })
                    .bg(if active {
                        cx.theme().list_active
                    } else {
                        cx.theme().transparent
                    })
                    .hover(move |style| style.bg(hover))
                    .on_prepaint({
                        let bounds_store = bounds_store.clone();
                        move |bounds, _, _| {
                            if let Ok(mut list) = bounds_store.lock() {
                                list.push((index, bounds));
                            }
                        }
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            if let Some(index) = this.doc_index(doc_id) {
                                this.begin_tab_drag(index, f32::from(event.position.x), cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, _, cx| {
                            this.close_doc_by_id(doc_id, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            // Fast click may finish before global drag listeners mount.
                            if this.tab_drag.as_ref().is_some_and(|d| !d.started) {
                                this.tab_drag = None;
                                cx.notify();
                            }
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.tab_drag.as_ref().is_some_and(|d| d.started) {
                            return;
                        }
                        this.tab_drag = None;
                        this.activate_doc_by_id(doc_id, cx);
                    }))
                    .context_menu(move |menu, _, _| {
                        let rename_shell = shell.clone();
                        let close_shell = shell.clone();
                        let others_shell = shell.clone();
                        let left_shell = shell.clone();
                        let right_shell = shell.clone();
                        let all_shell = shell.clone();
                        let adopt_shell = shell.clone();
                        let rename_target = rename_target.clone();
                        let other_tabs = other_tabs.clone();
                        let left_tabs = left_tabs.clone();
                        let right_tabs = right_tabs.clone();
                        let all_tabs = visible_tabs.clone();
                        let mut menu = menu.item(
                            PopupMenuItem::new(rename_label.clone())
                                .disabled(!can_rename)
                                .on_click(move |_, window, cx| {
                                    let Some((notebook_id, rel_path)) = rename_target.clone()
                                    else {
                                        return;
                                    };
                                    rename_shell.update(cx, |this, cx| {
                                        this.activate_doc_by_id(doc_id, cx);
                                        this.begin_rename(notebook_id, &rel_path, window, cx);
                                    });
                                }),
                        );
                        if is_external {
                            menu = menu.item(PopupMenuItem::new(adopt_label.clone()).on_click(
                                move |_, window, cx| {
                                    adopt_shell.update(cx, |this, cx| {
                                        this.activate_doc_by_id(doc_id, cx);
                                        this.save_external_into_notebook(window, cx);
                                    });
                                },
                            ));
                        }
                        menu.separator()
                            .item(PopupMenuItem::new(close_label.clone()).on_click(
                                move |_, _, cx| {
                                    close_shell
                                        .update(cx, |this, cx| this.close_doc_by_id(doc_id, cx));
                                },
                            ))
                            .item(
                                PopupMenuItem::new(close_others_label.clone())
                                    .disabled(other_tabs.is_empty())
                                    .on_click(move |_, _, cx| {
                                        others_shell.update(cx, |this, cx| {
                                            this.close_docs(&other_tabs, cx)
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(close_left_label.clone())
                                    .disabled(left_tabs.is_empty())
                                    .on_click(move |_, _, cx| {
                                        left_shell
                                            .update(cx, |this, cx| this.close_docs(&left_tabs, cx));
                                    }),
                            )
                            .item(
                                PopupMenuItem::new(close_right_label.clone())
                                    .disabled(right_tabs.is_empty())
                                    .on_click(move |_, _, cx| {
                                        right_shell.update(cx, |this, cx| {
                                            this.close_docs(&right_tabs, cx)
                                        });
                                    }),
                            )
                            .separator()
                            .item(PopupMenuItem::new(close_all_label.clone()).on_click(
                                move |_, _, cx| {
                                    all_shell.update(cx, |this, cx| this.close_docs(&all_tabs, cx));
                                },
                            ))
                    })
                    .child(
                        div()
                            .text_sm()
                            .text_color(label_color)
                            .truncate()
                            .max_w(px(180.))
                            .child(doc_label),
                    )
                    .child(
                        // 独立的关闭控件：常态灰色叉号，悬停出现底色且叉号变红，
                        // 按下时底色加深。同时阻止左键冒泡，避免关闭时被当成
                        // 标签拖拽或激活标签。
                        div()
                            .id(format!("close-{doc_id:?}"))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(20.))
                            .flex_shrink_0()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(cx.theme().transparent)
                            .text_color(close_fg)
                            .hover(move |style| style.bg(close_hover_bg).text_color(close_hover_fg))
                            .active(move |style| style.bg(close_pressed_bg))
                            .role(Role::Button)
                            .aria_label(close_button_label)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_doc_by_id(doc_id, cx);
                            }))
                            .child(Icon::new(IconName::X).xsmall()),
                    ),
            );
        }

        if insert_at == Some(visible.len()) {
            tabs_row = tabs_row.child(
                div()
                    .w(px(2.))
                    .h(px(22.))
                    .rounded_sm()
                    .bg(primary)
                    .flex_none(),
            );
        }

        let tab_switch_items: Vec<(EntityId, String)> = visible
            .iter()
            .map(|&index| {
                let doc = self.docs[index].read(cx);
                let is_external = doc.notebook_id.is_none();
                let title = if is_external {
                    format!("外部 · {}", doc.rel_path)
                } else {
                    doc.title()
                };
                let label = if doc.conflict.is_some() {
                    format!("⚠ {title}")
                } else if doc.dirty {
                    format!("● {title}")
                } else {
                    title
                };
                (self.docs[index].entity_id(), label)
            })
            .collect();
        let has_tabs = !tab_switch_items.is_empty();
        let switch_tip = if has_tabs {
            t!("tabs.switch").to_string()
        } else {
            t!("tabs.switch_empty").to_string()
        };
        let switch_shell = shell.clone();

        let actions = div()
            .id("tabs-actions")
            .h_full()
            .flex()
            .items_center()
            .px_1()
            .flex_none()
            .child(
                Button::new("btn-tab-switch")
                    .ghost()
                    .compact()
                    .dropdown_caret(true)
                    .tooltip(switch_tip)
                    .disabled(!has_tabs)
                    .dropdown_menu_with_anchor(Anchor::TopRight, {
                        let items = tab_switch_items;
                        move |menu, _, cx| {
                            let mut menu = menu.scrollable(true);
                            if items.is_empty() {
                                return menu.item(
                                    PopupMenuItem::new(t!("tabs.switch_empty").to_string())
                                        .disabled(true),
                                );
                            }
                            // 菜单展开时再解析活动标签，
                            // 而不是依赖上一次渲染标签栏时的帧状态。
                            let active = switch_shell
                                .read(cx)
                                .active_doc_entity()
                                .map(|doc| doc.entity_id());
                            for (id, label) in items.iter() {
                                let id = *id;
                                let shell = switch_shell.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label.clone())
                                        .checked(Some(id) == active)
                                        .on_click(move |_, _, cx| {
                                            shell.update(cx, |this, cx| {
                                                this.activate_doc_by_id(id, cx);
                                            });
                                        }),
                                );
                            }
                            menu
                        }
                    }),
            );

        div()
            .h(px(36.))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                div()
                    .id("tabs-scroll")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .px_2()
                    .overflow_x_hidden()
                    .child(tabs_row)
                    // Plain mouse wheel → horizontal tab scroll (no Shift needed).
                    .child(ScrollableMask::vertical_wheel(&self.tab_scroll)),
            )
            .child(actions)
            .into_any_element()
    }

    /// Scrolls the tab strip so the foreground note's tab is in view.
    ///
    /// Safe to call before the strip has been laid out: `ScrollHandle` keeps
    /// the request and applies it on the next paint. Callers that create or
    /// restore many tabs at once should also schedule a deferred retry.
    pub(super) fn ensure_active_tab_visible(&self, cx: &App) {
        let Some(active) = self.active_doc else {
            return;
        };
        let Some(pos) = self
            .visible_tabs(cx)
            .iter()
            .position(|&seen| seen == active)
        else {
            return;
        };
        self.tab_scroll.scroll_to_item(pos);
    }

    pub(super) fn begin_tab_drag(&mut self, index: usize, x: f32, cx: &mut Context<Self>) {
        let visible = self.visible_tabs(cx);
        let Some(pos) = visible.iter().position(|&seen| seen == index) else {
            return;
        };
        self.tab_drag = Some(TabDrag {
            from: index,
            pointer_x: x,
            insert_at: pos,
            started: false,
        });
        cx.notify();
    }

    pub(super) fn update_tab_drag(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.tab_drag.as_ref() else {
            return;
        };
        let started = drag.started || (x - drag.pointer_x).abs() > 6.0;
        if !started {
            return;
        }
        let insert_at = self.insert_slot_for_x(x);
        let drag = self.tab_drag.as_mut().expect("tab drag checked above");
        let changed = !drag.started || drag.insert_at != insert_at;
        drag.started = true;
        drag.pointer_x = x;
        drag.insert_at = insert_at;
        if changed {
            cx.notify();
        }
    }

    pub(super) fn end_tab_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.tab_drag.take() else {
            return;
        };
        if drag.started {
            self.reorder_visible_tab(drag.from, drag.insert_at, cx);
        }
        cx.notify();
    }

    fn insert_slot_for_x(&self, x: f32) -> usize {
        let Ok(bounds) = self.tab_bounds.lock() else {
            return 0;
        };
        if bounds.is_empty() {
            return 0;
        }
        for (slot, (_index, bound)) in bounds.iter().enumerate() {
            let mid = f32::from(bound.origin.x) + f32::from(bound.size.width) * 0.5;
            if x < mid {
                return slot;
            }
        }
        bounds.len()
    }

    pub(super) fn render_tab_drag_listeners(&self, cx: &mut Context<Self>) -> AnyElement {
        let shell = cx.entity().downgrade();
        canvas(
            |_bounds, _window, _cx| {},
            move |_bounds, _state, window, _cx| {
                let mover = shell.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _phase, _window, cx| {
                    let x = f32::from(event.position.x);
                    if let Some(shell) = mover.upgrade() {
                        shell.update(cx, |this, cx| this.update_tab_drag(x, cx));
                    }
                });
                window.on_mouse_event(move |_event: &MouseUpEvent, _phase, _window, cx| {
                    if let Some(shell) = shell.upgrade() {
                        shell.update(cx, |this, cx| this.end_tab_drag(cx));
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

    pub(super) fn render_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(doc) = self.active_doc_entity().cloned() else {
            let message = if self.active_notebook.is_some() {
                t!("editor.empty").to_string()
            } else {
                t!("editor.no_notebook").to_string()
            };
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(message)
                .into_any_element();
        };

        if doc.read(cx).preview_mode {
            self.ensure_preview(cx);
            if let (Some(content), Some(overlay)) = (&self.preview_view, &self.selection_overlay) {
                let shell = cx.entity().downgrade();
                let copy_label = t!("preview.context_copy").to_string();
                let markdown_label = t!("preview.context_copy_markdown").to_string();
                // 右键菜单触发器始终挂载：若按条件切换
                // `context_menu` 包装层，元素 id 路径会变化，
                // 所有文本片段已保留的选区句柄都会丢失。
                return div()
                    .id("preview-root")
                    .size_full()
                    // 整个预览都是可选文本，光标与编辑器一致使用
                    // I 形光标。内部按钮（例如代码块的复制按钮）
                    // 仍会以各自的
                    // 手型光标覆盖它。
                    .cursor(CursorStyle::IBeam)
                    .context_menu(move |menu, window, cx| {
                        // 与 Zed 一致：未选中文本时不弹菜单。空的
                        // PopupMenu 会被 ContextMenu 过滤掉，
                        // 此时右键不会显示任何内容。
                        let selected = TextSelection::selected_text(window, cx).trim().to_string();
                        if selected.is_empty() {
                            return menu;
                        }
                        let copy_shell = shell.clone();
                        let copy_selected = selected.clone();
                        let markdown_shell = shell.clone();
                        menu.item(PopupMenuItem::new(copy_label.clone()).on_click(
                            move |_, _, cx| {
                                let _ = copy_shell.update(cx, |this, cx| {
                                    this.copy_preview_text(
                                        &copy_selected,
                                        PreviewCopyKind::Text,
                                        cx,
                                    );
                                });
                            },
                        ))
                        .item(
                            PopupMenuItem::new(markdown_label.clone()).on_click(move |_, _, cx| {
                                let _ = markdown_shell.update(cx, |this, cx| {
                                    this.copy_preview_text(
                                        &selected,
                                        PreviewCopyKind::Markdown,
                                        cx,
                                    );
                                });
                            }),
                        )
                    })
                    // 正文内容层用 `Entity::cached` 包裹：文档不变时整段
                    // 缓存、跳过重绘与重新排版，拖选大文档才不会每帧重排
                    // 上千个文本元素。选区高亮由独立的覆盖层负责，只在选区
                    // 变化时重绘。
                    .child(
                        content
                            .clone()
                            .cached(gpui_kit::StyleRefinement::default().size_full()),
                    )
                    .child(overlay.clone())
                    .into_any_element();
            }
        }

        let editor = doc.read(cx).editor.clone();
        let notebook_root = {
            let doc = doc.read(cx);
            doc.notebook_id.and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
                    .map(|entry| entry.notebook.root.clone())
            })
        };
        let note_path = doc.read(cx).path.clone();
        let host = self.config.image_host.clone();
        let shell = cx.entity().downgrade();

        let paste_root = notebook_root.clone();
        let paste_note = note_path.clone();
        let paste_host = host.clone();
        let paste_shell = shell.clone();
        let paste_doc = doc.clone();

        let drop_root = notebook_root;
        let drop_note = note_path;
        let drop_host = host;
        let drop_shell = shell;
        let drop_doc = doc.clone();

        div()
            .size_full()
            .min_h_0()
            .id("editor-drop")
            .on_drop::<ExternalPaths>(move |paths, window, cx| {
                let Some(root) = drop_root.clone() else {
                    return;
                };
                let images: Vec<PastedImage> = paths
                    .paths()
                    .iter()
                    .filter(|path| ImageKind::from_extension(path).is_some())
                    .map(|path| PastedImage::File(path.clone()))
                    .collect();
                if images.is_empty() {
                    return;
                }
                spawn_image_insert(
                    drop_shell.clone(),
                    drop_doc.clone(),
                    root,
                    drop_note.clone(),
                    drop_host.clone(),
                    images,
                    window,
                    cx,
                );
            })
            .child(
                Editor::new(&editor)
                    .h_full()
                    .w_full()
                    .on_paste(move |item, window, cx| {
                        let Some(root) = paste_root.clone() else {
                            return false;
                        };
                        let images = clipboard_images(item);
                        if images.is_empty() {
                            return false;
                        }
                        spawn_image_insert(
                            paste_shell.clone(),
                            paste_doc.clone(),
                            root,
                            paste_note.clone(),
                            paste_host.clone(),
                            images,
                            window,
                            cx,
                        );
                        true
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_status(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut status = div()
            .h(px(26.))
            .px_3()
            .flex()
            .items_center()
            .gap_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .text_xs()
            .text_color(cx.theme().muted_foreground);

        if let Some(doc) = self.active_doc_entity().cloned() {
            let entity_id = doc.entity_id();
            let revision = doc.read(cx).revision();
            // 全文统计只在文本修订号变化时重算；窗口每次重绘都扫描
            // 整篇大文档会明显放大预览模式的 CPU 占用。
            let stats = match self.status_stats {
                Some((id, rev, cached)) if id == entity_id && rev == revision => cached,
                _ => {
                    let text = doc.read(cx).text(cx);
                    let stats = text_stats(&text);
                    self.status_stats = Some((entity_id, revision, stats));
                    stats
                }
            };
            let doc = doc.read(cx);
            let saved_label = if doc.dirty {
                t!("status.unsaved").to_string()
            } else {
                t!("status.saved").to_string()
            };
            let notebook_label = if doc.notebook_name.is_empty() {
                t!("status.external").to_string()
            } else {
                doc.notebook_name.clone()
            };
            let (line, column) = doc.cursor(cx);
            let cursor_label =
                t!("status.cursor", line = line + 1, column = column + 1).to_string();
            status = status
                .child(notebook_label)
                .child(cursor_label)
                .child(t!("status.lines", count = stats.lines).to_string())
                .child(t!("status.words", count = stats.words).to_string())
                .child(t!("status.chars", count = stats.chars).to_string());
            let selection = doc.editor.read(cx).selected_value();
            if !selection.trim().is_empty() {
                let selected = text_stats(&selection);
                status = status.child(
                    t!(
                        "status.selection",
                        chars = selected.chars,
                        words = selected.words
                    )
                    .to_string(),
                );
            }
            status = status
                .child(doc.encoding.label())
                .child(doc.eol.label())
                .child(saved_label)
                .child(if doc.preview_mode {
                    t!("status.mode.preview").to_string()
                } else {
                    t!("status.mode.edit").to_string()
                });
            if let Some(err) = &doc.error {
                status = status.child(
                    div()
                        .text_color(cx.theme().danger)
                        .child(format!("{}: {err}", t!("status.error"))),
                );
            }
            if doc.autosave {
                status = status.child(t!("status.autosave").to_string());
            }
        } else {
            status = status.child(t!("status.no_doc").to_string());
        }

        let mcp_status: AnyElement = if self.mcp_live {
            div()
                .child(t!("status.mcp.on", port = self.mcp.port).to_string())
                .into_any_element()
        } else if self.config.mcp_http && self.mcp.error.is_some() {
            div()
                .text_color(cx.theme().danger)
                .child(t!("status.mcp.error").to_string())
                .into_any_element()
        } else {
            div()
                .child(t!("status.mcp.off").to_string())
                .into_any_element()
        };
        status = status.child(div().flex_1()).child(mcp_status);

        status.into_any_element()
    }
}
