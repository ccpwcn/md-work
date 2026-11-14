//! 侧边栏笔记树渲染，以及行尾悬浮显示的 `…` / `+` 操作按钮。

use std::rc::Rc;

use super::util::*;
use super::*;

/// 树行右侧为悬浮的 `…` / `+` 按钮预留的宽度，长名称在进入该区域前被截断。
const TRAILING_SLOT: f32 = 56.0;

/// 构造树行右键菜单的回调。
type BuildTreeMenu =
    Rc<dyn Fn(gpui_kit::component::menu::PopupMenu, &App) -> gpui_kit::component::menu::PopupMenu>;

impl AppShell {
    pub(super) fn render_sidebar_tree(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = Arc::new(self.sidebar_rows.clone());
        let active_notebook = self.active_notebook;
        let active_rel = self.active_rel_path(cx);
        let tree_selection = self.tree_selection.clone();
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
        let tree_bounds = self.tree_bounds.clone();
        if let Ok(mut bounds) = self.tree_bounds.lock() {
            bounds.clear();
        }
        let drop_edge = self.tree_drop_edge();
        let dragged = self
            .tree_drag
            .as_ref()
            .filter(|drag| drag.started)
            .map(|drag| drag.source.clone());
        let cut_source = self
            .tree_clipboard
            .as_ref()
            .filter(|clip| clip.action == ClipboardAction::Cut)
            .map(|clip| (clip.notebook_id, clip.rel_path.clone()));
        let drop_color = cx.theme().primary;
        let bookmark_marker = self.config.bookmark_marker;
        let bookmark_marks = self.bookmark_marks.clone();

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
                            let is_selected = matches!(
                                tree_selection.as_ref(),
                                Some(TreeSelection::NotebookRoot(id)) if *id == notebook_id
                            );
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
                            let name_shell = shell.clone();
                            let full_name_shell = shell.clone();
                            let copy_name_label = t!("note.copy_name").to_string();
                            let copy_full_name_label = t!("note.copy_full_name").to_string();
                            let export_label = t!("zip.export_label").to_string();
                            let import_label = t!("zip.import_label").to_string();
                            let rename_label = t!("sidebar.rename_notebook").to_string();
                            let new_note_label = t!("sidebar.new_note").to_string();
                            let new_folder_label = t!("sidebar.new_folder").to_string();
                            let paste_label = t!("tree.paste").to_string();
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
                                let paste_label = paste_label.clone();
                                let rename_label = rename_label.clone();
                                let copy_name_label = copy_name_label.clone();
                                let copy_full_name_label = copy_full_name_label.clone();
                                let export_label = export_label.clone();
                                let import_label = import_label.clone();
                                let name_shell = name_shell.clone();
                                let full_name_shell = full_name_shell.clone();
                                move |menu: gpui_kit::component::menu::PopupMenu, cx: &App| {
                                    let export_shell = export_shell.clone();
                                    let import_shell = import_shell.clone();
                                    let rename_shell = rename_shell.clone();
                                    let name_shell = name_shell.clone();
                                    let full_name_shell = full_name_shell.clone();
                                    let create_shell = create_shell.clone();
                                    let note_shell = create_shell.clone();
                                    let can_paste = create_shell.read(cx).tree_clipboard.is_some();
                                    let menu = menu
                                        .item(PopupMenuItem::new(new_note_label.clone()).on_click(
                                            move |_, window, cx| {
                                                note_shell.update(cx, |this, cx| {
                                                    this.new_note_in(
                                                        notebook_id,
                                                        None,
                                                        SiblingPlace::End,
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            },
                                        ))
                                        .item(
                                            PopupMenuItem::new(new_folder_label.clone()).on_click(
                                                move |_, window, cx| {
                                                    create_shell.update(cx, |this, cx| {
                                                        this.new_folder_in(
                                                            notebook_id,
                                                            None,
                                                            SiblingPlace::End,
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                },
                                            ),
                                        );
                                    let menu = if can_paste {
                                        let paste_shell = export_shell.clone();
                                        menu.item(PopupMenuItem::new(paste_label.clone()).on_click(
                                            move |_, _, cx| {
                                                paste_shell.update(cx, |this, cx| {
                                                    this.paste_tree_clipboard(notebook_id, None, cx)
                                                });
                                            },
                                        ))
                                    } else {
                                        menu
                                    };
                                    menu.separator()
                                        .item(PopupMenuItem::new(rename_label.clone()).on_click(
                                            move |_, window, cx| {
                                                rename_shell.update(cx, |this, cx| {
                                                    let name = this
                                                        .notebooks
                                                        .iter()
                                                        .find(|e| {
                                                            e.notebook.marker.id == notebook_id
                                                        })
                                                        .map(|e| {
                                                            e.notebook.display_name().to_string()
                                                        })
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
                                        .item(PopupMenuItem::new(copy_name_label.clone()).on_click(
                                            move |_, _, cx| {
                                                name_shell.update(cx, |this, cx| {
                                                    let name = this.notebook_name(notebook_id);
                                                    this.copy_tree_name(name, cx);
                                                });
                                            },
                                        ))
                                        .item(PopupMenuItem::new(copy_full_name_label.clone()).on_click(
                                            move |_, _, cx| {
                                                full_name_shell.update(cx, |this, cx| {
                                                    // 笔记本位于层级顶端，全名称与名称相同。
                                                    let name = this.notebook_name(notebook_id);
                                                    this.copy_tree_name(name, cx);
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
                                        .item(PopupMenuItem::new(import_label.clone()).on_click(
                                            move |_, window, cx| {
                                                import_shell.update(cx, |this, cx| {
                                                    this.open_zip_import(window, cx)
                                                });
                                            },
                                        ))
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
                                .bg(if is_selected || is_active {
                                    active_bg
                                } else {
                                    transparent_black()
                                })
                                .hover(move |style| style.bg(hover_bg))
                                .when(
                                    matches!(
                                        &dragged,
                                        Some(tree_drag::DragSource::Notebook(id)) if *id == notebook_id
                                    ),
                                    |row| row.opacity(0.55).cursor(CursorStyle::ClosedHand),
                                )
                                .on_prepaint({
                                    let bounds = tree_bounds.clone();
                                    move |painted, _, _| {
                                        if let Ok(mut list) = bounds.lock() {
                                            list.push((index, painted));
                                        }
                                    }
                                })
                                .on_mouse_down(MouseButton::Left, {
                                    let drag_shell = activate_shell.clone();
                                    move |event: &MouseDownEvent, _, cx| {
                                        let y = f32::from(event.position.y);
                                        drag_shell.update(cx, |this, cx| {
                                            this.begin_tree_drag(
                                                tree_drag::DragSource::Notebook(notebook_id),
                                                y,
                                                cx,
                                            );
                                        });
                                    }
                                })
                                .on_mouse_up(MouseButton::Left, {
                                    let drag_shell = activate_shell.clone();
                                    move |_, _, cx| {
                                        drag_shell.update(cx, |this, cx| {
                                            this.cancel_tree_drag_if_idle(cx);
                                        });
                                    }
                                })
                                .on_click(move |_, _, cx| {
                                    activate_shell.update(cx, |this, cx| {
                                        if this.take_tree_click() {
                                            return;
                                        }
                                        // 箭头已经切换过这一行，
                                        // 同一次手势随后的行点击
                                        // 不能再把它翻回去。
                                        if this.take_tree_toggle(notebook_id, None) {
                                            return;
                                        }
                                        if this.active_notebook == Some(notebook_id) {
                                            this.select_tree_root(notebook_id, cx);
                                            this.toggle_notebook_root(notebook_id, cx);
                                        } else {
                                            this.activate_notebook(notebook_id, cx);
                                        }
                                    });
                                })
                                .context_menu({
                                    let build_notebook_menu = build_notebook_menu.clone();
                                    move |menu, _, cx| (build_notebook_menu)(menu, cx)
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
                                                this.mark_tree_toggle(notebook_id, None);
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
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .truncate()
                                        // 为悬浮的 `…` / `+` 按钮预留位置，
                                        // 长名称不会覆盖到按钮上。
                                        .pr(px(TRAILING_SLOT))
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
                                    more_label,
                                    child_note_label,
                                    menu_shell,
                                    plus_shell,
                                    notebook_id,
                                    PlusCreate::Child(None),
                                    build_notebook_menu,
                                    if is_selected || is_active {
                                        active_bg
                                    } else {
                                        transparent_black()
                                    },
                                    hover_bg,
                                ))
                                .when(drop_edge == Some(tree_drag::DropEdge::Before(index)), |row| {
                                    row.child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left_0()
                                            .right_0()
                                            .h(px(2.))
                                            .bg(drop_color),
                                    )
                                })
                                .when(drop_edge == Some(tree_drag::DropEdge::After(index)), |row| {
                                    row.child(
                                        div()
                                            .absolute()
                                            .bottom_0()
                                            .left_0()
                                            .right_0()
                                            .h(px(2.))
                                            .bg(drop_color),
                                    )
                                })
                                .into_any_element()
                        }
                        SidebarRow::Entry { notebook_id, row } => {
                            let notebook_id = *notebook_id;
                            let active = active_notebook == Some(notebook_id)
                                && active_rel.as_deref() == Some(row.rel_path.as_str());
                            let selected = tree_selection.as_ref().is_some_and(|selection| {
                                selection.matches_entry(notebook_id, &row.rel_path)
                            });
                            let flashing = flash_rel.as_ref().is_some_and(|(id, path)| {
                                *id == notebook_id && path == &row.rel_path
                            });
                            let background = if active || selected {
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
                            let color = if is_folder || active || selected {
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
                            // 标记只对应「整篇笔记被收藏」，与工具栏的星形语义一致；
                            // 标题级书签不在这里体现。
                            let bookmarked = bookmark_marker != BookmarkMarker::None
                                && bookmark_marks
                                    .get(&notebook_id)
                                    .is_some_and(|paths| paths.contains(&row.rel_path));

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
                                    .child(
                                        div()
                                            .w(px(12.))
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(chevron),
                                    )
                                    .child(
                                        div().flex_shrink_0().child(
                                            Icon::new(type_icon).xsmall().text_color(muted),
                                        ),
                                    )
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
                            let copy_label = t!("tree.copy").to_string();
                            let cut_label = t!("tree.cut").to_string();
                            let duplicate_label = t!("tree.duplicate").to_string();
                            let paste_label = t!("tree.paste").to_string();
                            let new_note_label = t!("tree.new_note").to_string();
                            let new_subfolder_label = t!("tree.new_subfolder").to_string();
                            let more_label = t!("tree.more").to_string();
                            let child_note_label = t!("tree.new_child_note").to_string();
                            let sibling_note_label = t!("tree.new_sibling_note").to_string();
                            let sibling_folder_label = t!("tree.new_sibling_folder").to_string();
                            let note_copy_name_label = t!("note.copy_name").to_string();
                            let note_copy_full_name_label = t!("note.copy_full_name").to_string();
                            let note_copy_path_label = t!("note.copy_path").to_string();
                            let note_open_folder_label = t!("note.open_folder").to_string();
                            let note_path_shell = shell.clone();
                            let tooltip_rel = rel_path.clone();
                            let tooltip_folder = is_folder;
                            let tooltip_count = row.note_count;
                            let tooltip_created = row.created;
                            let tooltip_modified = row.modified;
                            let tooltip_bookmarked =
                                bookmarked.then(|| t!("tree.bookmarked").to_string());
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
                                let copy_label = copy_label.clone();
                                let cut_label = cut_label.clone();
                                let duplicate_label = duplicate_label.clone();
                                let paste_label = paste_label.clone();
                                let new_note_label = new_note_label.clone();
                                let new_subfolder_label = new_subfolder_label.clone();
                                let child_note_label = child_note_label.clone();
                                let sibling_note_label = sibling_note_label.clone();
                                let sibling_folder_label = sibling_folder_label.clone();
                                let note_copy_name_label = note_copy_name_label.clone();
                                let note_copy_full_name_label = note_copy_full_name_label.clone();
                                let note_copy_path_label = note_copy_path_label.clone();
                                let note_open_folder_label = note_open_folder_label.clone();
                                let note_path_shell = note_path_shell.clone();
                                move |menu: gpui_kit::component::menu::PopupMenu, cx: &App| {
                                    let rename_shell = rename_shell.clone();
                                    let delete_shell = delete_shell.clone();
                                    let move_shell = move_shell.clone();
                                    let create_shell = create_shell.clone();
                                    let rename_rel = rename_rel.clone();
                                    let delete_rel = delete_rel.clone();
                                    let move_rel = move_rel.clone();
                                    let create_rel = create_rel.clone();
                                    let child_note_label = child_note_label.clone();
                                    let sibling_note_label = sibling_note_label.clone();
                                    let sibling_folder_label = sibling_folder_label.clone();
                                    // 粘贴既可作用于文件夹（放入其中），
                                    // 也可作用于笔记（放入其所在文件夹）。
                                    let can_paste = move_shell.read(cx).tree_clipboard.is_some();
                                    let menu = if is_folder {
                                        let note_shell = create_shell.clone();
                                        let note_rel = create_rel.clone();
                                        let folder_shell = create_shell.clone();
                                        let folder_rel = create_rel.clone();
                                        let sibling_shell = create_shell.clone();
                                        let sibling_rel = create_rel.clone();
                                        let sibling_folder_shell = create_shell.clone();
                                        let sibling_folder_rel = create_rel.clone();
                                        menu.item(
                                            PopupMenuItem::new(child_note_label).on_click(
                                                move |_, window, cx| {
                                                    note_shell.update(cx, |this, cx| {
                                                        this.new_note_in(
                                                            notebook_id,
                                                            Some(note_rel.clone()),
                                                            SiblingPlace::End,
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
                                                    folder_shell.update(cx, |this, cx| {
                                                        this.new_folder_in(
                                                            notebook_id,
                                                            Some(folder_rel.clone()),
                                                            SiblingPlace::End,
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new(sibling_note_label).on_click(
                                                move |_, window, cx| {
                                                    sibling_shell.update(cx, |this, cx| {
                                                        let parent = parent_rel(&sibling_rel);
                                                        let parent = if parent.is_empty() {
                                                            None
                                                        } else {
                                                            Some(parent.to_string())
                                                        };
                                                        this.new_note_in(
                                                            notebook_id,
                                                            parent,
                                                            SiblingPlace::After(sibling_rel.clone()),
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                },
                                            ),
                                        )
                                        .item(
                                            PopupMenuItem::new(sibling_folder_label).on_click(
                                                move |_, window, cx| {
                                                    sibling_folder_shell.update(cx, |this, cx| {
                                                        let parent = parent_rel(&sibling_folder_rel);
                                                        let parent = if parent.is_empty() {
                                                            None
                                                        } else {
                                                            Some(parent.to_string())
                                                        };
                                                        this.new_folder_in(
                                                            notebook_id,
                                                            parent,
                                                            SiblingPlace::After(
                                                                sibling_folder_rel.clone(),
                                                            ),
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                },
                                            ),
                                        )
                                        .separator()
                                    } else {
                                        let note_shell = create_shell.clone();
                                        let note_rel = create_rel.clone();
                                        let folder_shell = create_shell.clone();
                                        let folder_rel = create_rel.clone();
                                        let parent = parent_rel(&create_rel);
                                        let parent = if parent.is_empty() {
                                            None
                                        } else {
                                            Some(parent.to_string())
                                        };
                                        let note_parent = parent.clone();
                                        let folder_parent = parent;
                                        menu.item(
                                            PopupMenuItem::new(new_note_label.clone()).on_click(
                                                move |_, window, cx| {
                                                    note_shell.update(cx, |this, cx| {
                                                        this.new_note_in(
                                                            notebook_id,
                                                            note_parent.clone(),
                                                            SiblingPlace::After(note_rel.clone()),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                },
                                            ),
                                        )
                                        .item(
                                            PopupMenuItem::new(t!("sidebar.new_folder").to_string())
                                                .on_click(move |_, window, cx| {
                                                    folder_shell.update(cx, |this, cx| {
                                                        this.new_folder_in(
                                                            notebook_id,
                                                            folder_parent.clone(),
                                                            SiblingPlace::After(folder_rel.clone()),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }),
                                        )
                                        .separator()
                                    };
                                    let copy_shell = move_shell.clone();
                                    let copy_rel = move_rel.clone();
                                    let cut_shell = move_shell.clone();
                                    let cut_rel = move_rel.clone();
                                    let dup_shell = move_shell.clone();
                                    let dup_rel = move_rel.clone();
                                    let menu = menu
                                        .item(PopupMenuItem::new(copy_label.clone()).on_click(
                                            move |_, _, cx| {
                                                copy_shell.update(cx, |this, cx| {
                                                    this.copy_tree_entry(notebook_id, &copy_rel, cx)
                                                });
                                            },
                                        ))
                                        .item(PopupMenuItem::new(cut_label.clone()).on_click(
                                            move |_, _, cx| {
                                                cut_shell.update(cx, |this, cx| {
                                                    this.cut_tree_entry(notebook_id, &cut_rel, cx)
                                                });
                                            },
                                        ))
                                        .item(
                                            PopupMenuItem::new(duplicate_label.clone()).on_click(
                                                move |_, _, cx| {
                                                    dup_shell.update(cx, |this, cx| {
                                                        this.duplicate_tree_entry(
                                                            notebook_id,
                                                            &dup_rel,
                                                            cx,
                                                        )
                                                    });
                                                },
                                            ),
                                        );
                                    let menu = if can_paste {
                                        let paste_shell = move_shell.clone();
                                        let paste_rel = create_rel.clone();
                                        let dest_folder = if is_folder {
                                            Some(paste_rel.clone())
                                        } else {
                                            let parent = parent_rel(&paste_rel);
                                            (!parent.is_empty()).then(|| parent.to_string())
                                        };
                                        menu.item(PopupMenuItem::new(paste_label.clone()).on_click(
                                            move |_, _, cx| {
                                                paste_shell.update(cx, |this, cx| {
                                                    this.paste_tree_clipboard(
                                                        notebook_id,
                                                        dest_folder.clone(),
                                                        cx,
                                                    )
                                                });
                                            },
                                        ))
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
                                    .item(PopupMenuItem::new(note_copy_name_label.clone()).on_click(
                                        {
                                            let name_shell = note_path_shell.clone();
                                            let name_rel = create_rel.clone();
                                            let name_folder = is_folder;
                                            move |_, _, cx| {
                                                name_shell.update(cx, |this, cx| {
                                                    let name = tree_entry_name(&name_rel, name_folder);
                                                    this.copy_tree_name(name, cx);
                                                });
                                            }
                                        },
                                    ))
                                    .item(PopupMenuItem::new(note_copy_full_name_label.clone()).on_click(
                                        {
                                            let full_shell = note_path_shell.clone();
                                            let full_rel = create_rel.clone();
                                            let full_folder = is_folder;
                                            move |_, _, cx| {
                                                full_shell.update(cx, |this, cx| {
                                                    let full = this.tree_full_entry_name(
                                                        notebook_id,
                                                        &full_rel,
                                                        full_folder,
                                                    );
                                                    this.copy_tree_name(full, cx);
                                                });
                                            }
                                        },
                                    ))
                                    .item(PopupMenuItem::new(note_copy_path_label.clone()).on_click(
                                        {
                                            let path_shell = note_path_shell.clone();
                                            let path_rel = create_rel.clone();
                                            move |_, _, cx| {
                                                path_shell.update(cx, |this, cx| {
                                                    let path = this.tree_entry_path(notebook_id, &path_rel);
                                                    this.copy_note_path(path, cx);
                                                });
                                            }
                                        },
                                    ))
                                    .item(PopupMenuItem::new(note_open_folder_label.clone()).on_click(
                                        {
                                            let path_shell = note_path_shell.clone();
                                            let path_rel = create_rel.clone();
                                            move |_, _, cx| {
                                                path_shell.update(cx, |this, cx| {
                                                    let path = this.tree_entry_path(notebook_id, &path_rel);
                                                    this.reveal_note_path(path, cx);
                                                });
                                            }
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
                                    let bookmarked_tip = tooltip_bookmarked.clone();
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
                                        if let Some(tip) = bookmarked_tip.clone() {
                                            column = column.child(
                                                div().text_xs().text_color(muted).child(tip),
                                            );
                                        }
                                        column
                                    })
                                    .build(window, cx)
                                })
                                .when(
                                    matches!(
                                        &dragged,
                                        Some(tree_drag::DragSource::Entry { notebook_id: id, rel_path: path })
                                            if *id == notebook_id && path == &rel_path
                                    ),
                                    |row| row.opacity(0.55).cursor(CursorStyle::ClosedHand),
                                )
                                .when(
                                    cut_source.as_ref().is_some_and(|(id, path)| {
                                        *id == notebook_id && path == &rel_path
                                    }),
                                    |row| row.opacity(0.55),
                                )
                                .on_prepaint({
                                    let bounds = tree_bounds.clone();
                                    move |painted, _, _| {
                                        if let Ok(mut list) = bounds.lock() {
                                            list.push((index, painted));
                                        }
                                    }
                                })
                                .on_mouse_down(MouseButton::Left, {
                                    let drag_shell = shell.clone();
                                    let drag_rel = rel_path.clone();
                                    move |event: &MouseDownEvent, _, cx| {
                                        let y = f32::from(event.position.y);
                                        drag_shell.update(cx, |this, cx| {
                                            this.begin_tree_drag(
                                                tree_drag::DragSource::Entry {
                                                    notebook_id,
                                                    rel_path: drag_rel.clone(),
                                                },
                                                y,
                                                cx,
                                            );
                                        });
                                    }
                                })
                                .on_mouse_up(MouseButton::Left, {
                                    let drag_shell = shell.clone();
                                    move |_, _, cx| {
                                        drag_shell.update(cx, |this, cx| {
                                            this.cancel_tree_drag_if_idle(cx);
                                        });
                                    }
                                })
                                .on_click({
                                    let click_shell = shell.clone();
                                    let click_rel = rel_path.clone();
                                    move |_, window, cx| {
                                        click_shell.update(cx, |this, cx| {
                                            if this.take_tree_click() {
                                                return;
                                            }
                                            // 箭头在鼠标按下时已切换该文件夹，
                                            // 随后的行点击不能再把它展开回去；
                                            // 笔记行没有箭头，
                                            // 因此不会消耗这条
                                            // 防重复切换记录。
                                            if is_folder
                                                && this.take_tree_toggle(
                                                    notebook_id,
                                                    Some(click_rel.as_str()),
                                                )
                                            {
                                                this.select_tree_entry(
                                                    notebook_id,
                                                    &click_rel,
                                                    EntryKind::Folder,
                                                    cx,
                                                );
                                                return;
                                            }
                                            if is_folder {
                                                this.select_tree_entry(
                                                    notebook_id,
                                                    &click_rel,
                                                    EntryKind::Folder,
                                                    cx,
                                                );
                                                this.ensure_folder_expanded(
                                                    notebook_id,
                                                    &click_rel,
                                                    cx,
                                                );
                                            } else {
                                                this.open_note_by_id(
                                                    notebook_id,
                                                    &click_rel,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        });
                                    }
                                })
                                .context_menu({
                                    let build_entry_menu = build_entry_menu.clone();
                                    move |menu, _, cx| (build_entry_menu)(menu, cx)
                                })
                                .child({
                                    let toggle_shell = shell.clone();
                                    let toggle_rel = rel_path.clone();
                                    div()
                                        .id(format!("tree-chevron-{index}"))
                                        .w(px(12.))
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(muted)
                                        .when(is_folder, |el| {
                                            el.cursor_pointer().on_mouse_down(
                                                MouseButton::Left,
                                                move |_, _, cx| {
                                                    cx.stop_propagation();
                                                    toggle_shell.update(cx, |this, cx| {
                                                        this.mark_tree_toggle(
                                                            notebook_id,
                                                            Some(&toggle_rel),
                                                        );
                                                        this.toggle_folder(
                                                            notebook_id,
                                                            &toggle_rel,
                                                            cx,
                                                        );
                                                    });
                                                },
                                            )
                                        })
                                        .child(chevron)
                                })
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .child(Icon::new(type_icon).xsmall().text_color(muted)),
                                )
                                .when(bookmark_marker != BookmarkMarker::None, |row| {
                                    // 固定槽位让所有行的名称左边缘保持对齐，
                                    // 未收藏的行只是留白；标记沿用图标的 muted 色，
                                    // 不占用背景 / 字重 / 文字颜色这些导航状态通道。
                                    row.child(
                                        div()
                                            .flex_shrink_0()
                                            .w(px(12.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(bookmarked, |slot| match bookmark_marker {
                                                BookmarkMarker::Dot => slot.child(
                                                    div().size(px(5.)).rounded_full().bg(muted),
                                                ),
                                                BookmarkMarker::Bookmark => slot.child(
                                                    Icon::new(IconName::Bookmark)
                                                        .xsmall()
                                                        .text_color(muted),
                                                ),
                                                BookmarkMarker::None => slot,
                                            }),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .text_color(color)
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .truncate()
                                        // 为悬浮的 `…` / `+` 按钮预留位置，
                                        // 长名称不会覆盖到按钮上。
                                        .pr(px(TRAILING_SLOT))
                                        .child(label),
                                )
                                .child(sidebar_row_trailing(
                                    index,
                                    &group,
                                    menu_open,
                                    more_label,
                                    if is_folder {
                                        child_note_label
                                    } else {
                                        new_note_label
                                    },
                                    menu_shell,
                                    plus_shell,
                                    notebook_id,
                                    if is_folder {
                                        PlusCreate::Child(Some(create_rel.clone()))
                                    } else {
                                        PlusCreate::After(create_rel.clone())
                                    },
                                    build_entry_menu,
                                    background,
                                    hover_bg,
                                ))
                                .when(drop_edge == Some(tree_drag::DropEdge::Before(index)), |row| {
                                    row.child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .left_0()
                                            .right_0()
                                            .h(px(2.))
                                            .bg(drop_color),
                                    )
                                })
                                .when(drop_edge == Some(tree_drag::DropEdge::After(index)), |row| {
                                    row.child(
                                        div()
                                            .absolute()
                                            .bottom_0()
                                            .left_0()
                                            .right_0()
                                            .h(px(2.))
                                            .bg(drop_color),
                                    )
                                })
                                .when(drop_edge == Some(tree_drag::DropEdge::Inside(index)), |row| {
                                    row.child(
                                        div()
                                            .absolute()
                                            .top_0()
                                            .bottom_0()
                                            .left_0()
                                            .right_0()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(drop_color),
                                    )
                                })
                                .into_any_element()
                        }
                    }
                })
                .collect::<Vec<_>>()
        })
        .panel_scrollbar(&self.tree_scroll)
    }
}

/// 悬浮时显示在树行末尾的 `…` / `+` 操作按钮。
#[expect(
    clippy::too_many_arguments,
    reason = "树行操作需要行号、菜单回调与两套颜色，均是渲染时的一次性快照"
)]
fn sidebar_row_trailing(
    index: usize,
    group: &str,
    menu_open: bool,
    more_label: String,
    plus_label: String,
    menu_shell: Entity<AppShell>,
    plus_shell: Entity<AppShell>,
    notebook_id: Uuid,
    plus: PlusCreate,
    build_menu: BuildTreeMenu,
    row_bg: Hsla,
    hover_bg: Hsla,
) -> AnyElement {
    let more = Button::new(format!("tree-more-{index}"))
        .ghost()
        .compact()
        .icon(IconName::Ellipsis)
        .tooltip(more_label)
        .dropdown_menu_with_anchor(Anchor::TopRight, {
            let build_menu = build_menu.clone();
            move |menu, _, cx| (build_menu)(menu, cx)
        })
        .on_open_change(move |open, _, cx| {
            menu_shell.update(cx, |this, cx| {
                this.sidebar_row_menu = if *open { Some(index) } else { None };
                cx.notify();
            });
        });

    // 绝对定位让空闲时的笔记数量贴右显示，悬浮操作
    // 覆盖同一尾部区域，不会把数量挤向左侧。
    // 不透明背景避免被截断的长名称从按钮后面透出来，
    // 名称也预留了 `TRAILING_SLOT`，因此不会延伸到按钮处。
    let mut actions = div()
        .id(format!("tree-actions-{index}"))
        .absolute()
        .right_0()
        .h_full()
        .h_flex()
        .items_center()
        .gap_0p5()
        .bg(row_bg)
        .group_hover(group.to_string(), move |style| {
            let style = style.bg(hover_bg);
            if menu_open { style } else { style.visible() }
        })
        .when(!menu_open, |el| el.invisible())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(more);

    actions = actions.child(
        Button::new(format!("tree-add-{index}"))
            .ghost()
            .compact()
            .icon(IconName::Plus)
            .tooltip(plus_label)
            .on_click(move |_, window, cx| {
                let plus = plus.clone();
                plus_shell.update(cx, |this, cx| match plus {
                    PlusCreate::Child(parent) => {
                        this.new_note_in(notebook_id, parent, SiblingPlace::End, window, cx);
                    }
                    PlusCreate::After(rel) => {
                        let parent = parent_rel(&rel);
                        let parent = if parent.is_empty() {
                            None
                        } else {
                            Some(parent.to_string())
                        };
                        this.new_note_in(notebook_id, parent, SiblingPlace::After(rel), window, cx);
                    }
                });
            }),
    );

    actions.into_any_element()
}
