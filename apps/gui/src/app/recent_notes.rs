//! Ctrl+E「最近笔记」面板：左栏笔记本，右栏笔记/文件夹。

use std::collections::HashMap;

use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn open_recent_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recent_notes.is_some() {
            return;
        }
        self.focus_panel(panel::RECENT_NOTES, window, cx);
        // 不启用 clean_on_escape：它会吞掉 Escape，使浮层的关闭快捷键失效。
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("recent.placeholder").to_string())
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.refresh_recent_notes(cx),
            InputEvent::PressEnter { .. } => {
                let handle = this.window_handle;
                let weak = cx.entity().downgrade();
                // 推迟到当前输入事件结束之后：回车会关闭面板并销毁输入框，
                // 在输入框自己的事件回调里重入窗口更新会崩溃。
                cx.spawn(async move |_, cx| {
                    let _ = handle.update(cx, |_, window, cx| {
                        let _ =
                            weak.update(cx, |this, cx| this.run_recent_notes_selected(window, cx));
                    });
                })
                .detach();
            }
            _ => {}
        })
        .detach();
        self.recent_notes = Some(RecentNotes {
            input,
            sort: RecentNotesSort::Viewed,
            rows: Vec::new(),
            notebooks: Vec::new(),
            notebook_index: 0,
            selected: 0,
        });
        self.refresh_recent_notes(cx);
    }

    pub(super) fn close_recent_notes(&mut self, cx: &mut Context<Self>) {
        if self.recent_notes.take().is_some() {
            cx.notify();
        }
    }

    /// 按当前排序与输入过滤重建列表；排序变更时也要走这里。
    pub(super) fn refresh_recent_notes(&mut self, cx: &mut Context<Self>) {
        let (query, sort) = match &self.recent_notes {
            Some(panel) => (
                panel.input.read(cx).value().trim().to_lowercase(),
                panel.sort,
            ),
            None => return,
        };
        // 浏览顺序用记录里的下标（0 最新）而不是秒级时间戳：同一秒内打开
        // 多篇笔记时时间戳相同，会退化成按名字排序，刚打开的反而落到下面。
        let viewed: HashMap<(Uuid, &str), u64> = self
            .session
            .recent_notes
            .iter()
            .enumerate()
            .map(|(index, record)| {
                (
                    (record.notebook, record.path.as_str()),
                    u64::MAX - index as u64,
                )
            })
            .collect();
        let mut rows = Vec::new();
        for entry in &self.notebooks {
            let id = entry.notebook.marker.id;
            for row in &entry.rows {
                if !query.is_empty() && fuzzy_score(&row.name.to_lowercase(), &query).is_none() {
                    continue;
                }
                let stamp = match sort {
                    RecentNotesSort::Viewed => viewed
                        .get(&(id, row.rel_path.as_str()))
                        .copied()
                        .unwrap_or(0),
                    RecentNotesSort::Modified => row.modified,
                    RecentNotesSort::Created => row.created,
                };
                rows.push(RecentNoteRow {
                    notebook: id,
                    path: row.rel_path.clone(),
                    label: row.name.clone(),
                    is_folder: row.kind == EntryKind::Folder,
                    stamp,
                });
            }
        }
        rows.sort_by(|a, b| b.stamp.cmp(&a.stamp).then_with(|| a.label.cmp(&b.label)));
        let mut notebooks = Vec::new();
        for row in &rows {
            if !notebooks.contains(&row.notebook) {
                notebooks.push(row.notebook);
            }
        }
        if let Some(panel) = self.recent_notes.as_mut() {
            panel.rows = rows;
            panel.notebooks = notebooks;
            panel.notebook_index = panel
                .notebook_index
                .min(panel.notebooks.len().saturating_sub(1));
            panel.selected = 0;
        }
        cx.notify();
    }

    pub(super) fn set_recent_notes_sort(&mut self, sort: RecentNotesSort, cx: &mut Context<Self>) {
        if let Some(panel) = self.recent_notes.as_mut() {
            if panel.sort == sort {
                return;
            }
            panel.sort = sort;
            panel.notebook_index = 0;
        }
        self.refresh_recent_notes(cx);
    }

    pub(super) fn select_recent_notebook(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(panel) = self.recent_notes.as_mut()
            && index < panel.notebooks.len()
        {
            panel.notebook_index = index;
            panel.selected = 0;
            cx.notify();
        }
    }

    pub(super) fn select_recent_note(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(panel) = self.recent_notes.as_mut() {
            panel.selected = index;
            cx.notify();
        }
    }

    /// 上下移动选中笔记，左右切换笔记本。
    pub(super) fn recent_notes_step(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(panel) = self.recent_notes.as_ref() else {
            return;
        };
        let id = panel.notebooks.get(panel.notebook_index).copied();
        let len = panel
            .rows
            .iter()
            .filter(|row| Some(row.notebook) == id)
            .count();
        match key {
            "up" if len > 0 => {
                let next = (panel.selected as i64 - 1).rem_euclid(len as i64) as usize;
                self.select_recent_note(next, cx);
            }
            "down" if len > 0 => {
                let next = (panel.selected as i64 + 1).rem_euclid(len as i64) as usize;
                self.select_recent_note(next, cx);
            }
            "left" if panel.notebook_index > 0 => {
                self.select_recent_notebook(panel.notebook_index - 1, cx);
            }
            "right" if panel.notebook_index + 1 < panel.notebooks.len() => {
                self.select_recent_notebook(panel.notebook_index + 1, cx);
            }
            _ => {}
        }
    }

    pub(super) fn run_recent_notes_selected(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = {
            let Some(panel) = self.recent_notes.as_ref() else {
                return;
            };
            let id = panel.notebooks.get(panel.notebook_index).copied();
            panel
                .rows
                .iter()
                .filter(|row| Some(row.notebook) == id)
                .nth(panel.selected)
                .map(|row| (row.notebook, row.path.clone(), row.is_folder))
        };
        let Some((notebook, path, is_folder)) = target else {
            return;
        };
        self.close_recent_notes(cx);
        if is_folder {
            self.expand_to(notebook, &path);
            self.select_tree_entry(notebook, &path, EntryKind::Folder, cx);
        } else {
            self.open_note_by_id(notebook, &path, window, cx);
        }
    }

    pub(super) fn render_recent_notes(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(panel) = self.recent_notes.as_ref() else {
            return div().into_any_element();
        };
        let input = panel.input.clone();
        let sort = panel.sort;
        let notebook_index = panel.notebook_index;
        let selected = panel.selected;
        let active_id = panel.notebooks.get(notebook_index).copied();
        let notebooks: Vec<(usize, Uuid, String, usize)> = panel
            .notebooks
            .iter()
            .enumerate()
            .map(|(index, id)| {
                let count = panel.rows.iter().filter(|row| row.notebook == *id).count();
                (index, *id, self.notebook_name(*id), count)
            })
            .collect();
        let rows: Vec<(usize, String, bool)> = panel
            .rows
            .iter()
            .filter(|row| Some(row.notebook) == active_id)
            .enumerate()
            .map(|(index, row)| (index, row.label.clone(), row.is_folder))
            .collect();

        let muted = cx.theme().muted_foreground;
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let active = cx.theme().list_active;
        let hover = cx.theme().list_hover;
        let shell = cx.entity();

        let mut left = div().v_flex().w(px(210.)).flex_shrink_0().gap_0p5();
        for (index, _id, name, count) in notebooks {
            let click_shell = shell.clone();
            left = left.child(
                div()
                    .id(format!("recent-notebook-{index}"))
                    .h(px(28.))
                    .w_full()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if index == notebook_index {
                        active
                    } else {
                        cx.theme().transparent
                    })
                    .hover(move |style| style.bg(hover))
                    .on_click(move |_, _, cx| {
                        click_shell.update(cx, |this, cx| this.select_recent_notebook(index, cx));
                    })
                    .child(Icon::new(IconName::BookOpen).xsmall().text_color(muted))
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(name))
                    .child(div().text_xs().text_color(muted).child(count.to_string())),
            );
        }

        let mut right = div().v_flex().flex_1().min_w_0().gap_0p5();
        for (index, label, is_folder) in rows {
            let click_shell = shell.clone();
            right = right.child(
                div()
                    .id(format!("recent-note-{index}"))
                    .h(px(28.))
                    .w_full()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if index == selected {
                        active
                    } else {
                        cx.theme().transparent
                    })
                    .hover(move |style| style.bg(hover))
                    .on_click(move |_, window, cx| {
                        let click_shell = click_shell.clone();
                        click_shell.update(cx, |this, cx| {
                            this.select_recent_note(index, cx);
                            this.run_recent_notes_selected(window, cx);
                        });
                    })
                    .child(
                        Icon::new(if is_folder {
                            IconName::Folder
                        } else {
                            IconName::FileText
                        })
                        .xsmall()
                        .text_color(muted),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(label)),
            );
        }
        if panel.rows.is_empty() {
            right = right.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("recent.empty").to_string()),
            );
        }

        let sort_shell = shell.clone();
        let sort_button = Button::new("recent-sort")
            .ghost()
            .compact()
            .label(match sort {
                RecentNotesSort::Viewed => t!("recent.viewed").to_string(),
                RecentNotesSort::Modified => t!("recent.modified").to_string(),
                RecentNotesSort::Created => t!("recent.created").to_string(),
            })
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let options = [
                    (RecentNotesSort::Viewed, t!("recent.viewed").to_string()),
                    (RecentNotesSort::Modified, t!("recent.modified").to_string()),
                    (RecentNotesSort::Created, t!("recent.created").to_string()),
                ];
                let mut menu = menu;
                for (option, label) in options {
                    let sort_shell = sort_shell.clone();
                    menu = menu.item(PopupMenuItem::new(label).checked(option == sort).on_click(
                        move |_, _, cx| {
                            let sort_shell = sort_shell.clone();
                            sort_shell
                                .update(cx, |this, cx| this.set_recent_notes_sort(option, cx));
                        },
                    ));
                }
                menu
            });

        let key_shell = shell.clone();
        div()
            .id("recent-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_center()
            .items_start()
            .pt(px(84.))
            .bg(overlay)
            .occlude()
            .on_mouse_down(MouseButton::Left, {
                let shell = shell.clone();
                move |_, _, cx| {
                    shell.update(cx, |this, cx| this.close_recent_notes(cx));
                }
            })
            .on_key_down(move |event: &KeyDownEvent, _, cx| {
                let key_shell = key_shell.clone();
                key_shell.update(cx, |this, cx| match event.keystroke.key.as_str() {
                    "escape" => this.close_recent_notes(cx),
                    "up" | "down" | "left" | "right" => {
                        this.recent_notes_step(&event.keystroke.key, cx)
                    }
                    _ => {}
                });
            })
            .child(
                div()
                    .id("recent-dialog")
                    .v_flex()
                    .gap_2()
                    .w(px(760.))
                    .h(px(460.))
                    .p_3()
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
                            .child(Input::new(&input).flex_1().min_w_0())
                            .child(sort_button),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .h_flex()
                            .items_start()
                            .gap_2()
                            .overflow_hidden()
                            .child(left)
                            .child(div().w(px(1.)).h_full().bg(border).flex_shrink_0())
                            .child(right),
                    ),
            )
            .focus_trap(panel::RECENT_NOTES, &self.panel_focus(panel::RECENT_NOTES))
            .into_any_element()
    }
}
