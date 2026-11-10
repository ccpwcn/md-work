use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn open_image_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.image_settings.is_some() {
            return;
        }
        let (mode, picgo, http_endpoint, field, url_path, headers) = match &self.config.image_host {
            ImageHost::Local => (
                ImageHostMode::Local,
                image::default_picgo_endpoint(),
                String::new(),
                image::default_field(),
                image::default_url_path(),
                String::new(),
            ),
            ImageHost::PicGo { endpoint } => (
                ImageHostMode::PicGo,
                endpoint.clone(),
                String::new(),
                image::default_field(),
                image::default_url_path(),
                String::new(),
            ),
            ImageHost::Http {
                endpoint,
                field,
                url_path,
                headers,
            } => (
                ImageHostMode::Http,
                image::default_picgo_endpoint(),
                endpoint.clone(),
                field.clone(),
                url_path.clone(),
                headers.clone(),
            ),
        };
        let picgo_endpoint = cx.new(|cx| InputState::new(window, cx).default_value(picgo));
        let http_endpoint = cx.new(|cx| InputState::new(window, cx).default_value(http_endpoint));
        let http_field = cx.new(|cx| InputState::new(window, cx).default_value(field));
        let http_url_path = cx.new(|cx| InputState::new(window, cx).default_value(url_path));
        let http_headers = cx.new(|cx| TextareaState::new(window, cx).default_value(headers));
        self.image_settings = Some(ImageSettingsDialog {
            mode,
            picgo_endpoint,
            http_endpoint,
            http_field,
            http_url_path,
            http_headers,
            error: None,
            message: None,
            cleanup: AssetCleanupState::default(),
        });
        // 打开页面就异步扫描一次；大工作区也不会卡住界面。
        self.start_asset_cleanup_scan(cx);
        cx.notify();
    }

    fn asset_scan_inputs(&self) -> (PathBuf, Vec<PathBuf>) {
        let root = self.current_workspace_root().unwrap_or_default();
        let notebooks = self
            .notebooks
            .iter()
            .map(|entry| entry.notebook.root.clone())
            .collect();
        (root, notebooks)
    }

    /// 后台扫描各笔记本 assets 目录里未被任何笔记引用的图片。
    pub(super) fn start_asset_cleanup_scan(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.image_settings.as_mut() else {
            return;
        };
        dialog.cleanup.phase = AssetCleanupPhase::Scanning;
        dialog.cleanup.error = None;
        self.asset_cleanup_generation = self.asset_cleanup_generation.wrapping_add(1);
        let generation = self.asset_cleanup_generation;
        let (root, notebooks) = self.asset_scan_inputs();
        cx.spawn(async move |this, cx| {
            let report = cx
                .background_executor()
                .spawn(async move { mdwork_core::asset_cleanup::scan(&root, &notebooks) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.asset_cleanup_generation != generation {
                    return;
                }
                let Some(dialog) = this.image_settings.as_mut() else {
                    return;
                };
                dialog.cleanup.phase = AssetCleanupPhase::Ready;
                dialog.cleanup.files = report.files;
                dialog.cleanup.bytes = report.bytes;
                dialog.cleanup.error = None;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// 一键清理：先弹二次确认，真正删除时再重新扫描一次。
    pub(super) fn request_asset_cleanup(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.image_settings.as_mut() else {
            return;
        };
        if dialog.cleanup.files.is_empty()
            || dialog.cleanup.phase != AssetCleanupPhase::Ready
            || dialog.cleanup.confirm
        {
            return;
        }
        dialog.cleanup.confirm = true;
        cx.notify();
    }

    pub(super) fn cancel_asset_cleanup(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.image_settings.as_mut() {
            dialog.cleanup.confirm = false;
        }
        cx.notify();
    }

    pub(super) fn confirm_asset_cleanup(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.image_settings.as_mut() else {
            return;
        };
        dialog.cleanup.confirm = false;
        dialog.cleanup.phase = AssetCleanupPhase::Cleaning;
        self.asset_cleanup_generation = self.asset_cleanup_generation.wrapping_add(1);
        let generation = self.asset_cleanup_generation;
        let (root, notebooks) = self.asset_scan_inputs();
        cx.spawn(async move |this, cx| {
            let (outcome, report) = cx
                .background_executor()
                .spawn(async move {
                    // 确认时重新扫描，避免「扫描 → 确认」之间新引用的图片被删掉。
                    let outcome = mdwork_core::asset_cleanup::delete_unused(&root, &notebooks);
                    let report = mdwork_core::asset_cleanup::scan(&root, &notebooks);
                    (outcome, report)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.asset_cleanup_generation != generation {
                    return;
                }
                let size = format_bytes(outcome.bytes);
                if outcome.failed > 0 {
                    this.notify_error(
                        t!(
                            "image.cleanup_partial",
                            count = outcome.deleted,
                            failed = outcome.failed
                        )
                        .to_string(),
                        cx,
                    );
                } else {
                    let message = t!(
                        "image.cleanup_done",
                        count = outcome.deleted,
                        size = size.clone()
                    )
                    .to_string();
                    this.notify_success(message.clone(), cx);
                    if let Some(dialog) = this.image_settings.as_mut() {
                        dialog.message = Some(message);
                    }
                }
                if let Some(dialog) = this.image_settings.as_mut() {
                    dialog.cleanup.phase = AssetCleanupPhase::Ready;
                    dialog.cleanup.files = report.files;
                    dialog.cleanup.bytes = report.bytes;
                    dialog.cleanup.error = None;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn set_image_host_mode(&mut self, mode: ImageHostMode, cx: &mut Context<Self>) {
        let Some(dialog) = self.image_settings.as_mut() else {
            return;
        };
        if dialog.mode == mode {
            return;
        }
        dialog.mode = mode;
        dialog.error = None;
        dialog.message = None;
        cx.notify();
    }

    pub(super) fn save_image_settings(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.image_settings.as_ref() else {
            return;
        };
        let host = match dialog.mode {
            ImageHostMode::Local => ImageHost::Local,
            ImageHostMode::PicGo => ImageHost::PicGo {
                endpoint: dialog.picgo_endpoint.read(cx).value().trim().to_string(),
            },
            ImageHostMode::Http => ImageHost::Http {
                endpoint: dialog.http_endpoint.read(cx).value().trim().to_string(),
                field: dialog.http_field.read(cx).value().trim().to_string(),
                url_path: dialog.http_url_path.read(cx).value().trim().to_string(),
                headers: dialog.http_headers.read(cx).value().to_string(),
            },
        };
        if let Err(err) = image::validate_host(&host) {
            if let Some(dialog) = self.image_settings.as_mut() {
                dialog.error = Some(err.to_string());
                dialog.message = None;
            }
            cx.notify();
            return;
        }
        self.config.image_host = host;
        self.save_config(cx);
        // 保存后保留对话框：以前置 None 会让页面退回「加载中」，必须关闭
        // 整个设置窗口再打开才能继续编辑。
        if let Some(dialog) = self.image_settings.as_mut() {
            dialog.error = None;
            dialog.message = Some(t!("image.saved").to_string());
        }
        cx.notify();
    }

    pub(super) fn render_image_settings_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.image_settings.as_ref() else {
            return empty_hint(t!("image.loading").to_string(), cx.theme().muted_foreground);
        };
        let p = SettingsPalette::new(cx);

        let mode_track = settings_segment_track(
            vec![
                settings_segment(
                    "image-mode-local",
                    t!("image.mode.local").to_string(),
                    Some(IconName::HardDrive),
                    dialog.mode == ImageHostMode::Local,
                    cx.listener(move |this, _, _, cx| {
                        this.set_image_host_mode(ImageHostMode::Local, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "image-mode-picgo",
                    t!("image.mode.picgo").to_string(),
                    Some(IconName::CloudUpload),
                    dialog.mode == ImageHostMode::PicGo,
                    cx.listener(move |this, _, _, cx| {
                        this.set_image_host_mode(ImageHostMode::PicGo, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "image-mode-http",
                    t!("image.mode.http").to_string(),
                    Some(IconName::Globe),
                    dialog.mode == ImageHostMode::Http,
                    cx.listener(move |this, _, _, cx| {
                        this.set_image_host_mode(ImageHostMode::Http, cx)
                    }),
                    p,
                ),
            ],
            p,
        );
        let mode_card = settings_card(
            t!("image.mode").to_string(),
            Some(t!("image.host_hint").to_string()),
            mode_track,
            p,
        );

        let config_body: AnyElement = match dialog.mode {
            ImageHostMode::Local => div()
                .h_flex()
                .w_full()
                .items_start()
                .gap_2()
                .child(Icon::new(IconName::FolderOpen).small().text_color(p.muted))
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(settings_hint(t!("image.local_hint").to_string(), p)),
                )
                .into_any_element(),
            ImageHostMode::PicGo => div()
                .v_flex()
                .gap_2()
                .child(mcp_field(
                    t!("image.endpoint").to_string(),
                    Input::new(&dialog.picgo_endpoint)
                        .w_full()
                        .small()
                        .into_any_element(),
                    p.muted,
                ))
                .child(settings_hint(t!("image.picgo_hint").to_string(), p))
                .into_any_element(),
            ImageHostMode::Http => div()
                .v_flex()
                .gap_2()
                .child(mcp_field(
                    t!("image.endpoint").to_string(),
                    Input::new(&dialog.http_endpoint)
                        .w_full()
                        .small()
                        .into_any_element(),
                    p.muted,
                ))
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div().flex_1().min_w_0().child(mcp_field(
                                t!("image.field").to_string(),
                                Input::new(&dialog.http_field)
                                    .w_full()
                                    .small()
                                    .into_any_element(),
                                p.muted,
                            )),
                        )
                        .child(
                            div().flex_1().min_w_0().child(mcp_field(
                                t!("image.url_path").to_string(),
                                Input::new(&dialog.http_url_path)
                                    .w_full()
                                    .small()
                                    .into_any_element(),
                                p.muted,
                            )),
                        ),
                )
                .child(mcp_field(
                    t!("image.headers").to_string(),
                    div()
                        .v_flex()
                        .gap_1()
                        .child(Textarea::new(&dialog.http_headers).h(px(64.)).w_full())
                        .child(settings_hint(t!("image.headers_hint").to_string(), p))
                        .into_any_element(),
                    p.muted,
                ))
                .child(settings_hint(t!("image.http_hint").to_string(), p))
                .into_any_element(),
        };
        let config_card = settings_card(t!("image.host_title").to_string(), None, config_body, p);

        let cleanup = &dialog.cleanup;
        let (cleanup_status, cleanup_color) = if let Some(error) = &cleanup.error {
            (
                t!("image.cleanup_error", message = error.clone()).to_string(),
                p.danger,
            )
        } else {
            match cleanup.phase {
                AssetCleanupPhase::Scanning => (t!("image.cleanup_scanning").to_string(), p.muted),
                AssetCleanupPhase::Cleaning => {
                    (t!("image.cleanup_cleaning").to_string(), p.warning)
                }
                AssetCleanupPhase::Ready if cleanup.files.is_empty() => {
                    (t!("image.cleanup_empty").to_string(), p.success)
                }
                AssetCleanupPhase::Ready => (
                    t!(
                        "image.cleanup_ready",
                        count = cleanup.files.len(),
                        size = format_bytes(cleanup.bytes)
                    )
                    .to_string(),
                    p.foreground,
                ),
            }
        };
        let can_clean = cleanup.phase == AssetCleanupPhase::Ready && !cleanup.files.is_empty();
        let cleanup_body = div()
            .v_flex()
            .gap_2()
            .child(settings_hint(t!("image.cleanup_hint").to_string(), p))
            .child(
                div()
                    .text_sm()
                    .text_color(cleanup_color)
                    .child(cleanup_status),
            )
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("image-cleanup-rescan")
                            .ghost()
                            .compact()
                            .icon(IconName::RefreshCw)
                            .label(t!("image.cleanup_rescan").to_string())
                            .disabled(cleanup.phase != AssetCleanupPhase::Ready)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.start_asset_cleanup_scan(cx)),
                            ),
                    )
                    .child(
                        Button::new("image-cleanup-run")
                            .danger()
                            .compact()
                            .icon(IconName::Trash)
                            .label(t!("image.cleanup_button").to_string())
                            .disabled(!can_clean)
                            .on_click(cx.listener(|this, _, _, cx| this.request_asset_cleanup(cx))),
                    ),
            )
            .into_any_element();
        let cleanup_card =
            settings_card(t!("image.cleanup_title").to_string(), None, cleanup_body, p);

        let (notice, notice_color) = if let Some(error) = dialog.error.clone() {
            (error, p.danger)
        } else if let Some(message) = dialog.message.clone() {
            (message, p.success)
        } else {
            (String::new(), p.muted)
        };
        let footer = div()
            .h_flex()
            .items_center()
            .gap_2()
            .min_h(px(24.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(notice_color)
                    .child(notice),
            )
            .child(
                Button::new("image-save")
                    .primary()
                    .compact()
                    .icon(IconName::Save)
                    .label(t!("image.save").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.save_image_settings(cx))),
            );

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .child(settings_body(
                "settings-images-body",
                vec![mode_card, config_card, cleanup_card],
            ))
            .child(footer)
            .into_any_element()
    }

    /// 「被删除清理的资源将无法恢复，是否继续？」二次确认。
    /// 遮罩放在设置面板之上，默认主按钮是「取消」，避免误删。
    pub(super) fn render_image_cleanup_confirm(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.image_settings.as_ref() else {
            return div().into_any_element();
        };
        let count = dialog.cleanup.files.len();
        let size = format_bytes(dialog.cleanup.bytes);
        let overlay = cx.theme().overlay;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;

        div()
            .id("image-cleanup-confirm-overlay")
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
                    shell.update(cx, |this, cx| this.cancel_asset_cleanup(cx));
                }
            })
            .child(
                div()
                    .v_flex()
                    .gap_3()
                    .w(px(440.))
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
                                    .child(t!("image.cleanup_confirm_title").to_string()),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(t!("image.cleanup_confirm").to_string()),
                    )
                    .child(div().text_xs().text_color(muted).child(
                        t!("image.cleanup_confirm_detail", count = count, size = size).to_string(),
                    ))
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("image-cleanup-confirm-no")
                                    .primary()
                                    .compact()
                                    .label(t!("image.cleanup_confirm_no").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_asset_cleanup(cx)),
                                    ),
                            )
                            .child(
                                Button::new("image-cleanup-confirm-yes")
                                    .danger()
                                    .compact()
                                    .label(t!("image.cleanup_confirm_yes").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.confirm_asset_cleanup(cx)
                                        }),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
