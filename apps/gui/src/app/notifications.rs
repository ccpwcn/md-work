use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn notify_with(
        &mut self,
        kind: NoticeKind,
        text: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.notices.push(NoticeItem {
            kind,
            text: text.into(),
            read: false,
            created: Instant::now(),
        });
        if self.notices.len() > 100 {
            self.notices.remove(0);
        }
        cx.notify();
    }

    pub(super) fn notify_info(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.notify_with(NoticeKind::Info, text, cx);
    }

    pub(super) fn notify_success(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.notify_with(NoticeKind::Success, text, cx);
    }

    pub(super) fn notify_warning(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.notify_with(NoticeKind::Warning, text, cx);
    }

    pub(super) fn notify_error(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.notify_with(NoticeKind::Error, text, cx);
    }

    pub(super) fn unread_notices(&self) -> usize {
        self.notices.iter().filter(|item| !item.read).count()
    }

    pub(super) fn render_title(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .h_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .pr_2()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .child(t!("app.name").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("app.subtitle").to_string()),
                    ),
            )
            .child(self.render_workspace_switcher(cx))
            .child(div().flex_1())
            .child(titlebar_guard(
                title_icon(
                    "title-new-note",
                    IconName::SquarePen,
                    t!("sidebar.new_note").to_string(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx))),
            ))
            .child(titlebar_guard(
                title_icon(
                    "title-export",
                    IconName::FileOutput,
                    t!("export.title").to_string(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_export_dialog(window, cx))),
            ))
            .child(titlebar_guard(
                title_icon(
                    "title-zip-import",
                    IconName::FolderInput,
                    t!("zip.import_label").to_string(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_zip_import(window, cx))),
            ))
            .child(self.render_theme_switcher(cx))
            .child(titlebar_guard(
                title_icon(
                    "title-settings",
                    IconName::Settings,
                    t!("settings.title").to_string(),
                )
                .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            ))
            .child(self.render_notification_bell(cx))
            .into_any_element()
    }

    pub(super) fn render_notification_bell(&self, cx: &mut Context<Self>) -> AnyElement {
        let unread = self.unread_notices();
        let open = self.notice_panel;
        let hover = cx.theme().list_hover;
        let active = cx.theme().list_active;
        let danger = cx.theme().danger;
        let danger_foreground = cx.theme().danger_foreground;

        let trigger = div()
            .id("title-notifications")
            .relative()
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .when(open, |element| element.bg(active))
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(|this, _, _, cx| {
                this.notice_panel = !this.notice_panel;
                if this.notice_panel {
                    this.workspace_menu = false;
                    this.theme_menu = false;
                }
                cx.notify();
            }))
            .child(Icon::new(IconName::Bell).small())
            .when(unread > 0, |element| {
                element.child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .right(px(1.))
                        .min_w(px(14.))
                        .h(px(14.))
                        .px(px(3.))
                        .rounded_full()
                        .bg(danger)
                        .text_color(danger_foreground)
                        .text_xs()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if unread > 99 {
                            "99+".to_string()
                        } else {
                            unread.to_string()
                        }),
                )
            });

        let mut popup = gpui_kit::base::Popup::new("notifications-popup", trigger)
            .anchor(Anchor::TopRight)
            .offset(px(6.));
        if open {
            popup = popup.content(self.render_notification_panel(cx));
        }
        titlebar_guard(popup)
    }

    pub(super) fn render_notification_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().list_hover;
        let active_bg = cx.theme().list_active;
        let unread = self.unread_notices();

        let rows: Vec<(usize, NoticeKind, String, bool, String)> = self
            .notices
            .iter()
            .enumerate()
            .rev()
            .take(50)
            .map(|(index, item)| {
                (
                    index,
                    item.kind,
                    item.text.clone(),
                    item.read,
                    format_notice_age(item.created),
                )
            })
            .collect();
        let empty = rows.is_empty();

        let mut list = div().v_flex().w_full().gap_0p5().p_2();
        for (index, kind, text, read, age) in rows {
            let color = notice_color(kind, cx);
            let shell = cx.entity();
            list = list.child(
                div()
                    .id(format!("notice-row-{index}"))
                    .h_flex()
                    .items_start()
                    .gap_2()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .when(!read, |element| element.bg(active_bg))
                    .hover(move |style| style.bg(hover))
                    .on_click(move |_, _, cx| {
                        shell.update(cx, |this, cx| {
                            if let Some(item) = this.notices.get_mut(index) {
                                item.read = true;
                            }
                            cx.notify();
                        });
                    })
                    .child(
                        Icon::new(notice_icon(kind))
                            .small()
                            .text_color(color)
                            .mt(px(2.)),
                    )
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .when(!read, |element| element.font_medium())
                                    .child(text),
                            )
                            .child(
                                div()
                                    .h_flex()
                                    .items_center()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(age)
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_1()
                                            .child(div().size(px(5.)).rounded_full().bg(if read {
                                                muted
                                            } else {
                                                color
                                            }))
                                            .child(if read {
                                                t!("notice.read").to_string()
                                            } else {
                                                t!("notice.unread").to_string()
                                            }),
                                    ),
                            ),
                    ),
            );
        }

        div()
            .id("notifications-panel")
            .v_flex()
            .w(px(380.))
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(popover)
            .text_color(foreground)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.notice_panel = false;
                cx.notify();
            }))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_bold()
                            .child(t!("notice.title").to_string()),
                    )
                    .child(
                        Button::new("notice-read-all")
                            .ghost()
                            .compact()
                            .label(t!("notice.mark_all_read").to_string())
                            .disabled(unread == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                for item in &mut this.notices {
                                    item.read = true;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("notice-clear")
                            .ghost()
                            .compact()
                            .icon(IconName::Trash)
                            .tooltip(t!("notice.clear").to_string())
                            .disabled(self.notices.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.notices.clear();
                                cx.notify();
                            })),
                    ),
            )
            .when(empty, |element| {
                element.child(empty_hint(t!("notice.empty").to_string(), muted))
            })
            .when(!empty, |element| {
                element.child(
                    div()
                        .id("notice-list")
                        .max_h(px(420.))
                        .overflow_y_scroll()
                        .child(list),
                )
            })
            .into_any_element()
    }
}
