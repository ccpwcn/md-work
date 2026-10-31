use super::util::*;
use super::*;

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
        let mut bar = div()
            .h(px(36.))
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background);

        let shell = cx.entity();
        let visible = self.visible_tabs(cx);
        let rename_label = t!("tree.rename").to_string();
        let close_label = t!("tabs.close").to_string();
        let close_others_label = t!("tabs.close_others").to_string();
        let close_left_label = t!("tabs.close_left").to_string();
        let close_right_label = t!("tabs.close_right").to_string();
        let close_all_label = t!("tabs.close_all").to_string();

        for (index, doc) in self.docs.iter().enumerate() {
            let doc = doc.read(cx);
            if doc.notebook_id.is_some() && doc.notebook_id != self.active_notebook {
                continue;
            }
            let active = self.active_doc == Some(index);
            let title = if doc.notebook_id.is_none() {
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
            let position = visible.iter().position(|&seen| seen == index).unwrap_or(0);
            let left_tabs = visible[..position].to_vec();
            let right_tabs = visible[position + 1..].to_vec();
            let other_tabs: Vec<usize> = visible
                .iter()
                .copied()
                .filter(|&seen| seen != index)
                .collect();
            let rename_target = doc.notebook_id.map(|id| (id, doc.rel_path.clone()));
            let can_rename = rename_target.is_some();
            let shell = shell.clone();
            let visible = visible.clone();
            let rename_label = rename_label.clone();
            let close_label = close_label.clone();
            let close_others_label = close_others_label.clone();
            let close_left_label = close_left_label.clone();
            let close_right_label = close_right_label.clone();
            let close_all_label = close_all_label.clone();

            bar = bar.child(
                div()
                    .id(format!("tab-{index}"))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if active {
                        cx.theme().list_active
                    } else {
                        cx.theme().transparent
                    })
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.activate_doc(index, cx)))
                    .context_menu(move |menu, _, _| {
                        let rename_shell = shell.clone();
                        let close_shell = shell.clone();
                        let others_shell = shell.clone();
                        let left_shell = shell.clone();
                        let right_shell = shell.clone();
                        let all_shell = shell.clone();
                        let rename_target = rename_target.clone();
                        let other_tabs = other_tabs.clone();
                        let left_tabs = left_tabs.clone();
                        let right_tabs = right_tabs.clone();
                        let all_tabs = visible.clone();
                        menu.item(
                            PopupMenuItem::new(rename_label.clone())
                                .disabled(!can_rename)
                                .on_click(move |_, window, cx| {
                                    let Some((notebook_id, rel_path)) = rename_target.clone()
                                    else {
                                        return;
                                    };
                                    rename_shell.update(cx, |this, cx| {
                                        this.activate_doc(index, cx);
                                        this.begin_rename(notebook_id, &rel_path, window, cx);
                                    });
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new(close_label.clone()).on_click(move |_, _, cx| {
                                close_shell.update(cx, |this, cx| this.close_doc(index, cx));
                            }),
                        )
                        .item(
                            PopupMenuItem::new(close_others_label.clone())
                                .disabled(other_tabs.is_empty())
                                .on_click(move |_, _, cx| {
                                    others_shell
                                        .update(cx, |this, cx| this.close_docs(&other_tabs, cx));
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
                                    right_shell
                                        .update(cx, |this, cx| this.close_docs(&right_tabs, cx));
                                }),
                        )
                        .separator()
                        .item(
                            PopupMenuItem::new(close_all_label.clone()).on_click(
                                move |_, _, cx| {
                                    all_shell.update(cx, |this, cx| this.close_docs(&all_tabs, cx));
                                },
                            ),
                        )
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
                        Button::new(format!("close-{index}"))
                            .ghost()
                            .compact()
                            .label("×")
                            .on_click(cx.listener(move |this, _, _, cx| this.close_doc(index, cx))),
                    ),
            );
        }

        if let Some(index) = self.active_doc {
            let toggle_label = if self.preview_mode {
                t!("preview.edit").to_string()
            } else {
                t!("preview.show").to_string()
            };
            let (bookmark_target, is_external) = {
                let doc = self.docs[index].read(cx);
                (
                    doc.notebook_id.map(|id| (id, doc.rel_path.clone())),
                    doc.notebook_id.is_none(),
                )
            };
            bar = bar.child(div().flex_1());
            if is_external {
                bar = bar.child(
                    Button::new("btn-adopt-external")
                        .ghost()
                        .compact()
                        .label(t!("external.save_into").to_string())
                        .tooltip(t!("external.save_into_hint").to_string())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save_external_into_notebook(window, cx)
                        })),
                );
            }
            if let Some((_, rel_path)) = &bookmark_target {
                let starred = self.bookmarks.note(rel_path).is_some();
                bar = bar.child(
                    Button::new("btn-bookmark-note")
                        .ghost()
                        .compact()
                        .label(if starred { "★" } else { "☆" })
                        .tooltip(t!("bookmark.toggle_note").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_note_bookmark(cx))),
                );
            }
            bar = bar.child(
                Button::new("btn-toggle-preview")
                    .ghost()
                    .compact()
                    .label(toggle_label)
                    .tooltip("Ctrl+T".to_string())
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_preview(window, cx))),
            );
            if self.preview_mode {
                let copy_label = if self.copied {
                    t!("preview.copied").to_string()
                } else {
                    t!("preview.copy").to_string()
                };
                let wrap_label = if self.config.code_wrap {
                    t!("preview.wrap").to_string()
                } else {
                    t!("preview.nowrap").to_string()
                };
                bar = bar
                    .child(
                        Button::new("btn-toggle-wrap")
                            .ghost()
                            .compact()
                            .label(wrap_label)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_code_wrap(cx))),
                    )
                    .child(
                        Button::new("btn-copy-preview")
                            .ghost()
                            .compact()
                            .label(copy_label)
                            .on_click(cx.listener(|this, _, _, cx| this.copy_preview(cx))),
                    );
            }
        }

        bar.into_any_element()
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

        if self.preview_mode {
            self.ensure_preview(cx);
            if let Some(cache) = &self.preview_cache {
                let base = doc.read(cx).path.parent().map(|path| path.to_path_buf());
                return preview::render(
                    &cache.ir,
                    base.as_deref(),
                    &self.math,
                    &self.image_paths,
                    self.config.code_wrap,
                    &self.config.note_font,
                    self.config.note_font_size,
                    cx,
                );
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

    pub(super) fn render_status(&self, cx: &mut Context<Self>) -> AnyElement {
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

        if let Some(doc) = self.active_doc_entity() {
            let doc = doc.read(cx);
            let text = doc.text(cx);
            let stats = text_stats(&text);
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
                .child(if self.preview_mode {
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
