//! Ctrl+E「最近笔记」面板：单栏跨笔记本列表，按右上角排序依据倒序。

use std::collections::HashMap;

use super::util::*;
use super::*;

/// 第一帧最多绘制多少行；超出的行靠关键字过滤定位，避免 Debug 构建卡顿。
const RECENT_VISIBLE_LIMIT: usize = 80;

impl AppShell {
    pub(super) fn open_recent_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recent_notes.is_some() {
            return;
        }
        // 面板的 focus_trap 会接管键盘焦点；先把原焦点记下来，取消时还回去。
        // 没有显式焦点时退回主面板，避免关闭后焦点悬空导致快捷键失灵。
        let previous_focus = window
            .focused(cx)
            .or_else(|| Some(self.panel_focus(panel::MAIN)));
        // 只把焦点交给面板里的输入框；先 focus_panel 再 focus 输入框会在同一
        // 次按键里改两次焦点、排两帧，Debug 构建下就是那一下迟滞感。
        // 焦点陷阱在浮层上，输入焦点在其内部，Tab 仍被限制在面板里。
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
            previous_focus,
            sort: RecentNotesSort::Viewed,
            rows: Vec::new(),
            selected: 0,
        });
        self.refresh_recent_notes(cx);
    }

    /// 只是移除面板状态，不碰焦点。选中笔记打开时用：焦点由打开笔记的流程接管。
    fn take_recent_notes(&mut self, cx: &mut Context<Self>) -> Option<RecentNotes> {
        let panel = self.recent_notes.take();
        if panel.is_some() {
            cx.notify();
        }
        panel
    }

    /// 取消面板（ESC / 点遮罩 / 再次 Ctrl+E）：焦点回到打开面板前的位置。
    pub(super) fn close_recent_notes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = self.take_recent_notes(cx)
            && let Some(handle) = panel.previous_focus
        {
            window.focus(&handle, cx);
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
        // 以 viewed_at 为主、记录顺序为辅：跨会话的时间戳不会失真，
        // 同一秒内打开多篇时仍按「后打开的在前」排列。
        let viewed: HashMap<(Uuid, &str), (u64, u64)> = self
            .session
            .recent_notes
            .iter()
            .enumerate()
            .map(|(index, record)| {
                (
                    (record.notebook, record.path.as_str()),
                    (record.viewed_at, u64::MAX - index as u64),
                )
            })
            .collect();
        let mut rows = Vec::new();
        for entry in &self.notebooks {
            let id = entry.notebook.marker.id;
            let notebook_name = entry.notebook.display_name();
            for row in &entry.rows {
                // 单栏展示全名：笔记本/文件夹/笔记名称（笔记去掉 .md）。
                let mut path = row.rel_path.replace('\\', "/");
                if row.kind == EntryKind::Note {
                    path = path.strip_suffix(".md").unwrap_or(&path).to_string();
                }
                let label = format!("{notebook_name}/{path}");
                if !query.is_empty() && fuzzy_score(&label.to_lowercase(), &query).is_none() {
                    continue;
                }
                let stamp = match sort {
                    // 没有浏览记录的笔记不参与「最近浏览」，否则会退化成
                    // 按全名字母序，看起来像被当成最近访问过。
                    RecentNotesSort::Viewed => {
                        let Some(stamp) = viewed.get(&(id, row.rel_path.as_str())).copied() else {
                            continue;
                        };
                        stamp
                    }
                    RecentNotesSort::Modified => (row.modified, 0),
                    RecentNotesSort::Created => (row.created, 0),
                };
                rows.push(RecentNoteRow {
                    notebook: id,
                    path: row.rel_path.clone(),
                    label,
                    is_folder: row.kind == EntryKind::Folder,
                    stamp,
                });
            }
        }
        // 跨笔记本按右上角的排序依据统一倒序；同分时按全名排，保持结果稳定。
        rows.sort_by(|a, b| b.stamp.cmp(&a.stamp).then_with(|| a.label.cmp(&b.label)));
        if let Some(panel) = self.recent_notes.as_mut() {
            panel.rows = rows;
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
        }
        self.refresh_recent_notes(cx);
    }

    pub(super) fn select_recent_note(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(panel) = self.recent_notes.as_mut() {
            panel.selected = index;
            cx.notify();
        }
    }

    /// 上下在单栏列表内移动选中项。
    pub(super) fn recent_notes_step(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(panel) = self.recent_notes.as_ref() else {
            return;
        };
        let len = panel.rows.len();
        match key {
            "up" if len > 0 => {
                let next = (panel.selected as i64 - 1).rem_euclid(len as i64) as usize;
                self.select_recent_note(next, cx);
            }
            "down" if len > 0 => {
                let next = (panel.selected as i64 + 1).rem_euclid(len as i64) as usize;
                self.select_recent_note(next, cx);
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
            panel
                .rows
                .get(panel.selected)
                .map(|row| (row.notebook, row.path.clone(), row.is_folder))
        };
        let Some((notebook, path, is_folder)) = target else {
            return;
        };
        self.take_recent_notes(cx);
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
        let selected = panel.selected;
        // 单栏列表只画前面若干行，更多内容靠关键字过滤定位，
        // 避免 Debug 构建首帧排版卡顿。
        let rows: Vec<(usize, String, bool)> = panel
            .rows
            .iter()
            .take(RECENT_VISIBLE_LIMIT)
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

        let mut list = div().v_flex().flex_1().min_w_0().gap_0p5();
        for (index, label, is_folder) in rows {
            let click_shell = shell.clone();
            list = list.child(
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
            let empty_hint = if sort == RecentNotesSort::Viewed {
                t!("recent.no_history").to_string()
            } else {
                t!("recent.empty").to_string()
            };
            list = list.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(muted)
                    .child(empty_hint),
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
                move |_, window, cx| {
                    shell.update(cx, |this, cx| this.close_recent_notes(window, cx));
                }
            })
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let key_shell = key_shell.clone();
                key_shell.update(cx, |this, cx| match event.keystroke.key.as_str() {
                    "escape" => this.close_recent_notes(window, cx),
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
                            .overflow_hidden()
                            .child(list),
                    ),
            )
            .focus_trap(panel::RECENT_NOTES, &self.panel_focus(panel::RECENT_NOTES))
            .into_any_element()
    }
}
