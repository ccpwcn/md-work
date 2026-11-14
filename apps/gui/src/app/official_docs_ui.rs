use super::*;
use crate::official_docs::{OfficialDocKind, OfficialDocStatus};

impl AppShell {
    /// 官方只读文档被要求编辑/保存时的统一提示。
    /// 通知中心默认只显示铃铛角标，这里顺手展开，确保用户真能看到原因。
    pub(super) fn warn_official_readonly(&mut self, cx: &mut Context<Self>) {
        self.notify_warning(t!("official_docs.readonly").to_string(), cx);
        self.notice_panel = true;
        self.workspace_menu = false;
        self.theme_menu = false;
    }

    /// 以只读预览模式打开随包发布的官方说明文档。
    ///
    /// 正文来自编译期内置副本；磁盘上的文件只做 SHA-256 校验。打开前再校验
    /// 一次，覆盖「程序运行期间文件被改动」的情况，但同一份文档只提醒一次。
    pub(super) fn open_official_doc(
        &mut self,
        kind: OfficialDocKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.official_doc_warned.contains(&kind)
            && let Some(alert) = crate::official_docs::check_deployed_doc(kind)
        {
            self.official_doc_warned.insert(kind);
            self.official_doc_alerts.push(alert);
        }
        if let Some(index) = self
            .docs
            .iter()
            .position(|doc| doc.read(cx).official == Some(kind))
        {
            self.active_doc = Some(index);
            self.reveal_active(cx);
            cx.notify();
            return;
        }
        let Some(path) = kind.deployed_path() else {
            self.notify_error(t!("official_docs.unavailable").to_string(), cx);
            return;
        };
        // 启动时的欢迎笔记、命令行路径会并发打开；等全部结束后再把这份
        // 官方说明文档置为活动标签，保证用户一进来看到的就是它。
        self.open_requests(
            vec![OpenRequest::Official {
                kind,
                path: path.clone(),
            }],
            window,
            cx,
        );
        if self.opening.contains(&path) {
            self.pending_official_focus = Some(kind);
        }
    }

    pub(super) fn dismiss_official_doc_alert(&mut self, cx: &mut Context<Self>) {
        if !self.official_doc_alerts.is_empty() {
            self.official_doc_alerts.remove(0);
            cx.notify();
        }
    }

    /// 「此文件已被修改，不是官方权威版本」的校验弹窗。
    pub(super) fn render_official_doc_alert(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(alert) = self.official_doc_alerts.first().cloned() else {
            return div().into_any_element();
        };
        let message = match alert.status {
            OfficialDocStatus::Modified { .. } => t!("official_docs.modified").to_string(),
            OfficialDocStatus::Missing(_) | OfficialDocStatus::Intact => {
                t!("official_docs.missing").to_string()
            }
        };
        let path = alert
            .kind
            .deployed_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| alert.kind.file_name().to_string());
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;

        div()
            .id("official-doc-alert-overlay")
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
                    .w(px(480.))
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
                                    .child(t!("official_docs.alert_title").to_string()),
                            ),
                    )
                    .child(div().text_sm().child(message))
                    .child(div().text_xs().text_color(muted).truncate().child(path))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("official_docs.authoritative").to_string()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1())
                            .child(
                                Button::new("official-doc-alert-ok")
                                    .primary()
                                    .compact()
                                    .label(t!("official_docs.ok").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dismiss_official_doc_alert(cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
