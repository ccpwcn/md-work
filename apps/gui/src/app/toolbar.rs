use super::util::*;
use super::*;
use crate::print::{self, PrintResult};
use gpui_kit::component::input::{Redo, Undo};

impl AppShell {
    pub(super) fn render_doc_toolbar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let index = self.active_doc?;
        let doc = self.docs.get(index)?.clone();
        let is_external = doc.read(cx).notebook_id.is_none();
        let editing = !self.preview_mode;
        let border = cx.theme().border;
        let shell = cx.entity();
        let mode_tip = if self.preview_mode {
            t!("preview.edit").to_string()
        } else {
            t!("preview.show").to_string()
        };

        let mut left = div()
            .id("doc-toolbar-left")
            .h_flex()
            .items_center()
            .gap_0p5()
            .min_w_0()
            .overflow_x_scroll();

        left = left.child(
            Button::new("tb-toggle-preview")
                .ghost()
                .compact()
                .icon(if self.preview_mode {
                    IconName::SquarePen
                } else {
                    IconName::Eye
                })
                .tooltip(format!("{mode_tip} (Ctrl+T)"))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_preview(window, cx))),
        );
        if self.preview_mode {
            let wrap_label = if self.config.code_wrap {
                t!("preview.wrap").to_string()
            } else {
                t!("preview.nowrap").to_string()
            };
            let copy_label = if self.copied {
                t!("preview.copied").to_string()
            } else {
                t!("preview.copy").to_string()
            };
            left = left
                .child(
                    Button::new("tb-toggle-wrap")
                        .ghost()
                        .compact()
                        .label(wrap_label)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_code_wrap(cx))),
                )
                .child(
                    Button::new("tb-copy-preview")
                        .ghost()
                        .compact()
                        .label(copy_label)
                        .on_click(cx.listener(|this, _, _, cx| this.copy_preview(cx))),
                );
        }

        left = left
            .child(toolbar_sep(border))
            .child(
                Button::new("tb-insert-image")
                    .ghost()
                    .compact()
                    .icon(IconName::Image)
                    .tooltip(t!("toolbar.image").to_string())
                    .disabled(!editing)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.pick_and_insert_image(window, cx)),
                    ),
            )
            .child(toolbar_sep(border))
            .child(
                Button::new("tb-undo")
                    .ghost()
                    .compact()
                    .icon(IconName::Undo2)
                    .tooltip(t!("toolbar.undo").to_string())
                    .disabled(!editing)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dispatch_editor_action(Box::new(Undo), window, cx)
                    })),
            )
            .child(
                Button::new("tb-redo")
                    .ghost()
                    .compact()
                    .icon(IconName::Redo2)
                    .tooltip(t!("toolbar.redo").to_string())
                    .disabled(!editing)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dispatch_editor_action(Box::new(Redo), window, cx)
                    })),
            )
            .child(toolbar_sep(border));

        if editing {
            left = left
                .child(
                    Button::new("tb-heading")
                        .ghost()
                        .compact()
                        .icon(IconName::Heading)
                        .tooltip(t!("toolbar.heading").to_string())
                        .dropdown_menu({
                            let shell = shell.clone();
                            move |menu, _, _| {
                                let mut menu = menu;
                                for level in 1u8..=6 {
                                    let shell = shell.clone();
                                    let label = t!("toolbar.heading_n", n = level).to_string();
                                    menu = menu.item(PopupMenuItem::new(label).on_click(
                                        move |_, window, cx| {
                                            shell.update(cx, |this, cx| {
                                                this.apply_heading(level, window, cx);
                                            });
                                        },
                                    ));
                                }
                                let shell = shell.clone();
                                menu.item(
                                    PopupMenuItem::new(t!("toolbar.heading_clear").to_string())
                                        .on_click(move |_, window, cx| {
                                            shell.update(cx, |this, cx| {
                                                this.clear_heading(window, cx);
                                            });
                                        }),
                                )
                            }
                        }),
                )
                .child(fmt_btn(
                    "tb-bold",
                    IconName::Bold,
                    t!("toolbar.bold"),
                    cx,
                    |this, window, cx| this.wrap_selection("**", "**", window, cx),
                ))
                .child(fmt_btn(
                    "tb-italic",
                    IconName::Italic,
                    t!("toolbar.italic"),
                    cx,
                    |this, window, cx| this.wrap_selection("*", "*", window, cx),
                ))
                .child(fmt_btn(
                    "tb-strike",
                    IconName::Strikethrough,
                    t!("toolbar.strike"),
                    cx,
                    |this, window, cx| this.wrap_selection("~~", "~~", window, cx),
                ))
                .child(fmt_btn(
                    "tb-ul",
                    IconName::List,
                    t!("toolbar.ul"),
                    cx,
                    |this, window, cx| this.toggle_line_prefix("- ", window, cx),
                ))
                .child(fmt_btn(
                    "tb-ol",
                    IconName::ListOrdered,
                    t!("toolbar.ol"),
                    cx,
                    |this, window, cx| this.toggle_line_prefix("1. ", window, cx),
                ))
                .child(fmt_btn(
                    "tb-todo",
                    IconName::ListTodo,
                    t!("toolbar.todo"),
                    cx,
                    |this, window, cx| this.toggle_line_prefix("- [ ] ", window, cx),
                ))
                .child(fmt_btn(
                    "tb-code",
                    IconName::CodeXml,
                    t!("toolbar.code"),
                    cx,
                    |this, window, cx| this.wrap_selection("`", "`", window, cx),
                ))
                .child(fmt_btn(
                    "tb-code-block",
                    IconName::FileCode,
                    t!("toolbar.code_block"),
                    cx,
                    |this, window, cx| this.wrap_selection("```\n", "\n```", window, cx),
                ))
                .child(fmt_btn(
                    "tb-math",
                    IconName::Sigma,
                    t!("toolbar.math"),
                    cx,
                    |this, window, cx| this.wrap_selection("$", "$", window, cx),
                ))
                .child(fmt_btn(
                    "tb-math-block",
                    IconName::SquareSigma,
                    t!("toolbar.math_block"),
                    cx,
                    |this, window, cx| this.wrap_selection("$$\n", "\n$$", window, cx),
                ))
                .child(fmt_btn(
                    "tb-quote",
                    IconName::TextQuote,
                    t!("toolbar.quote"),
                    cx,
                    |this, window, cx| this.toggle_line_prefix("> ", window, cx),
                ))
                .child(fmt_btn(
                    "tb-link",
                    IconName::Link,
                    t!("toolbar.link"),
                    cx,
                    |this, window, cx| this.wrap_selection("[", "]()", window, cx),
                ))
                .child(fmt_btn(
                    "tb-table",
                    IconName::Table,
                    t!("toolbar.table"),
                    cx,
                    |this, window, cx| {
                        this.insert_snippet(
                            "| 列1 | 列2 | 列3 |\n| --- | --- | --- |\n|  |  |  |\n|  |  |  |\n",
                            window,
                            cx,
                        )
                    },
                ));
        }

        let starred = (!is_external).then(|| {
            let rel_path = doc.read(cx).rel_path.clone();
            self.bookmarks.note(&rel_path).is_some()
        });

        let mut right = div().h_flex().items_center().gap_0p5().flex_none();
        if let Some(starred) = starred {
            right = right.child(
                Button::new("tb-bookmark")
                    .ghost()
                    .compact()
                    .label(if starred { "★" } else { "☆" })
                    .tooltip(t!("bookmark.toggle_note").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_note_bookmark(cx))),
            );
        }
        right = right
            .child(
                Button::new("tb-search")
                    .ghost()
                    .compact()
                    .icon(IconName::Search)
                    .tooltip(t!("toolbar.find").to_string())
                    .disabled(!editing)
                    .on_click(cx.listener(|this, _, window, cx| this.open_doc_search(window, cx))),
            )
            .child(
                Button::new("tb-more")
                    .ghost()
                    .compact()
                    .icon(IconName::Ellipsis)
                    .tooltip(t!("toolbar.more").to_string())
                    .dropdown_menu({
                        let shell = shell.clone();
                        move |menu, _, _| {
                            let export_shell = shell.clone();
                            let print_shell = shell.clone();
                            let delete_shell = shell.clone();
                            let adopt_shell = shell.clone();
                            let mut menu = menu
                                .item(
                                    PopupMenuItem::new(t!("toolbar.export").to_string()).on_click(
                                        move |_, window, cx| {
                                            export_shell.update(cx, |this, cx| {
                                                this.open_export_dialog(window, cx);
                                            });
                                        },
                                    ),
                                )
                                .item(
                                    PopupMenuItem::new(t!("toolbar.print").to_string()).on_click(
                                        move |_, _, cx| {
                                            print_shell.update(cx, |this, cx| {
                                                this.print_active_doc(cx);
                                            });
                                        },
                                    ),
                                )
                                .item(
                                    PopupMenuItem::new(t!("toolbar.delete").to_string()).on_click(
                                        move |_, _, cx| {
                                            delete_shell.update(cx, |this, cx| {
                                                this.request_delete_active(cx);
                                            });
                                        },
                                    ),
                                );
                            if is_external {
                                menu = menu.separator().item(
                                    PopupMenuItem::new(t!("external.save_into").to_string())
                                        .on_click(move |_, window, cx| {
                                            adopt_shell.update(cx, |this, cx| {
                                                this.save_external_into_notebook(window, cx);
                                            });
                                        }),
                                );
                            }
                            menu
                        }
                    }),
            );

        Some(
            div()
                .h(px(34.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(border)
                .bg(cx.theme().background)
                .child(left.flex_1().min_w_0())
                .child(right)
                .into_any_element(),
        )
    }

    pub(super) fn dispatch_editor_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview_mode {
            return;
        }
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        let handle = doc.read(cx).editor.read(cx).focus_handle(cx).clone();
        window.focus(&handle, cx);
        window.dispatch_action(action, cx);
        cx.notify();
    }

    pub(super) fn open_doc_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview_mode {
            return;
        }
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        doc.update(cx, |doc, cx| {
            let handle = doc.editor.read(cx).focus_handle(cx).clone();
            window.focus(&handle, cx);
            doc.editor.update(cx, |editor, cx| {
                editor.open_search(false, cx);
            });
        });
        cx.notify();
    }

    pub(super) fn pick_and_insert_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview_mode {
            return;
        }
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        let (notebook_root, note_path) = {
            let d = doc.read(cx);
            let root = d.notebook_id.and_then(|id| {
                self.notebooks
                    .iter()
                    .find(|entry| entry.notebook.marker.id == id)
                    .map(|entry| entry.notebook.root.clone())
            });
            (root, d.path.clone())
        };
        let Some(root) = notebook_root else {
            self.notify_warning(t!("toolbar.image_needs_notebook").to_string(), cx);
            return;
        };
        let host = self.config.image_host.clone();
        let shell = cx.entity().downgrade();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t!("toolbar.image_pick").to_string().into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let images: Vec<PastedImage> = paths
                .into_iter()
                .filter(|path| ImageKind::from_extension(path).is_some())
                .map(PastedImage::File)
                .collect();
            if images.is_empty() {
                let _ = this.update(cx, |this, cx| {
                    this.notify_warning(t!("toolbar.image_none").to_string(), cx);
                });
                return;
            }
            let _ = this.update_in(cx, |_this, window, cx| {
                spawn_image_insert(shell, doc, root, note_path, host, images, window, cx);
            });
        })
        .detach();
    }

    pub(super) fn insert_snippet(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview_mode {
            return;
        }
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        doc.update(cx, |doc, cx| doc.insert_text(text, window, cx));
        cx.notify();
    }

    pub(super) fn apply_heading(&mut self, level: u8, window: &mut Window, cx: &mut Context<Self>) {
        let hashes = "#".repeat(level.clamp(1, 6) as usize);
        self.transform_current_line(
            |line| {
                let body = strip_heading_prefix(line);
                format!("{hashes} {body}")
            },
            window,
            cx,
        );
    }

    pub(super) fn clear_heading(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transform_current_line(|line| strip_heading_prefix(line).to_string(), window, cx);
    }

    pub(super) fn toggle_line_prefix(
        &mut self,
        prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefix = prefix.to_string();
        self.transform_current_line(
            move |line| {
                if let Some(rest) = line.strip_prefix(&prefix) {
                    rest.to_string()
                } else {
                    format!("{prefix}{}", line.trim_start())
                }
            },
            window,
            cx,
        );
    }

    pub(super) fn transform_current_line(
        &mut self,
        transform: impl FnOnce(&str) -> String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview_mode {
            return;
        }
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        let (text, line, column) = {
            let d = doc.read(cx);
            let (line, column) = d.cursor(cx);
            (d.text(cx).to_string(), line, column)
        };
        let lines: Vec<&str> = text.split('\n').collect();
        let idx = (line as usize).min(lines.len().saturating_sub(1));
        let mut new_lines: Vec<String> = lines.iter().map(|s| (*s).to_string()).collect();
        if new_lines.is_empty() {
            new_lines.push(String::new());
        }
        new_lines[idx] = transform(&new_lines[idx]);
        let new_text = new_lines.join("\n");
        let new_col = column.min(new_lines[idx].chars().count() as u32);
        doc.update(cx, |doc, cx| {
            doc.editor.update(cx, |editor, cx| {
                editor.replace_all(new_text, window, cx);
            });
            doc.set_cursor(line, new_col, window, cx);
        });
        cx.notify();
    }

    pub(super) fn print_active_doc(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            self.notify_warning(t!("export.no_doc").to_string(), cx);
            return;
        };
        let (title, full, selection) = {
            let d = doc.read(cx);
            let selection = d.editor.read(cx).selected_value().to_string();
            (d.title(), d.text(cx).to_string(), selection)
        };
        let allow_selection = !selection.is_empty();
        let body = if allow_selection { selection } else { full };
        let result = print::print_text(&title, &body, allow_selection);
        match result {
            PrintResult::Cancelled => {}
            PrintResult::Printed => self.notify_success(t!("toolbar.print_done").to_string(), cx),
            PrintResult::Failed(err) => self.notify_error(err, cx),
        }
    }

    pub(super) fn request_delete_active(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.active_doc else {
            return;
        };
        let Some(doc) = self.docs.get(index) else {
            return;
        };
        let (notebook_id, rel_path, title) = {
            let d = doc.read(cx);
            (d.notebook_id, d.rel_path.clone(), d.title())
        };
        let Some(notebook_id) = notebook_id else {
            self.delete_confirm = Some(DeleteConfirm {
                kind: DeleteConfirmKind::CloseExternal { index },
                title,
            });
            cx.notify();
            return;
        };
        self.delete_confirm = Some(DeleteConfirm {
            kind: DeleteConfirmKind::Trash {
                notebook_id,
                rel_path,
            },
            title,
        });
        cx.notify();
    }

    pub(super) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.delete_confirm.take() else {
            return;
        };
        match confirm.kind {
            DeleteConfirmKind::Trash {
                notebook_id,
                rel_path,
            } => self.delete_entry(notebook_id, rel_path, cx),
            DeleteConfirmKind::CloseExternal { index } => self.close_doc(index, cx),
        }
        cx.notify();
    }

    pub(super) fn cancel_delete_confirm(&mut self, cx: &mut Context<Self>) {
        if self.delete_confirm.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn render_delete_confirm(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(confirm) = self.delete_confirm.as_ref() else {
            return div().into_any_element();
        };
        let title = confirm.title.clone();
        let message = match &confirm.kind {
            DeleteConfirmKind::Trash { .. } => t!("toolbar.delete_confirm").to_string(),
            DeleteConfirmKind::CloseExternal { .. } => {
                t!("toolbar.delete_external_confirm").to_string()
            }
        };
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;

        div()
            .id("delete-confirm-overlay")
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
                    shell.update(cx, |this, cx| this.cancel_delete_confirm(cx));
                }
            })
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .w(px(420.))
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
                                    .child(t!("toolbar.delete").to_string()),
                            ),
                    )
                    .child(div().text_sm().child(message))
                    .child(div().text_xs().text_color(muted).truncate().child(title))
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("delete-confirm-no")
                                    .ghost()
                                    .compact()
                                    .label(t!("toolbar.delete_cancel").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.cancel_delete_confirm(cx)
                                        }),
                                    ),
                            )
                            .child(
                                Button::new("delete-confirm-yes")
                                    .primary()
                                    .compact()
                                    .label(t!("toolbar.delete_yes").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_delete(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

fn toolbar_sep(border: Hsla) -> AnyElement {
    div()
        .w(px(1.))
        .h(px(16.))
        .mx_1()
        .bg(border)
        .flex_none()
        .into_any_element()
}

fn fmt_btn(
    id: &'static str,
    icon: IconName,
    tip: impl ToString,
    cx: &mut Context<AppShell>,
    on_click: impl Fn(&mut AppShell, &mut Window, &mut Context<AppShell>) + 'static + Clone,
) -> Button {
    let tip = tip.to_string();
    Button::new(id)
        .ghost()
        .compact()
        .icon(icon)
        .tooltip(tip)
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
}

fn strip_heading_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    let mut i = 0;
    for ch in trimmed.chars() {
        if ch == '#' && i < 6 {
            i += 1;
        } else {
            break;
        }
    }
    if i == 0 {
        return line.trim_start();
    }
    let rest = &trimmed[i..];
    rest.strip_prefix(' ').unwrap_or(rest)
}
