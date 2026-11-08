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
        });
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
                vec![mode_card, config_card],
            ))
            .child(footer)
            .into_any_element()
    }
}
