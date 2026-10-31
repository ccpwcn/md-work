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
            }
            cx.notify();
            return;
        }
        self.config.image_host = host;
        self.save_config(cx);
        self.image_settings = None;
        cx.notify();
    }

    pub(super) fn render_image_settings_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.image_settings.as_ref() else {
            return empty_hint(t!("image.loading").to_string(), cx.theme().muted_foreground);
        };
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let label = |text: String| {
            div()
                .w(px(96.))
                .flex_shrink_0()
                .text_xs()
                .text_color(muted)
                .child(text)
        };
        let mode_button = |mode: ImageHostMode, key: &'static str, cx: &mut Context<Self>| {
            let active = dialog.mode == mode;
            Button::new(match mode {
                ImageHostMode::Local => "image-mode-local",
                ImageHostMode::PicGo => "image-mode-picgo",
                ImageHostMode::Http => "image-mode-http",
            })
            .ghost()
            .compact()
            .label(t!(key).to_string())
            .when(active, |button| button.primary())
            .on_click(cx.listener(move |this, _, _, cx| this.set_image_host_mode(mode, cx)))
        };

        let mut body: Vec<AnyElement> = Vec::new();
        body.push(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(label(t!("image.mode").to_string()))
                .child(mode_button(ImageHostMode::Local, "image.mode.local", cx))
                .child(mode_button(ImageHostMode::PicGo, "image.mode.picgo", cx))
                .child(mode_button(ImageHostMode::Http, "image.mode.http", cx))
                .into_any_element(),
        );
        match dialog.mode {
            ImageHostMode::Local => {
                body.push(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("image.local_hint").to_string())
                        .into_any_element(),
                );
            }
            ImageHostMode::PicGo => {
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("image.endpoint").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.picgo_endpoint).w_full().small()),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("image.picgo_hint").to_string())
                        .into_any_element(),
                );
            }
            ImageHostMode::Http => {
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("image.endpoint").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.http_endpoint).w_full().small()),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("image.field").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.http_field).w_full().small()),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(label(t!("image.url_path").to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&dialog.http_url_path).w_full().small()),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .h_flex()
                        .items_start()
                        .gap_2()
                        .child(label(t!("image.headers").to_string()))
                        .child(
                            div()
                                .v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(Textarea::new(&dialog.http_headers).h(px(64.)).w_full())
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(t!("image.headers_hint").to_string()),
                                ),
                        )
                        .into_any_element(),
                );
                body.push(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("image.http_hint").to_string())
                        .into_any_element(),
                );
            }
        }

        let mut footer = div().h_flex().items_center().gap_2().min_h(px(24.)).child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(danger)
                .when_some(dialog.error.clone(), |element, error| element.child(error)),
        );
        footer = footer.child(
            Button::new("image-save")
                .primary()
                .compact()
                .label(t!("image.save").to_string())
                .on_click(cx.listener(|this, _, _, cx| this.save_image_settings(cx))),
        );

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .children(body)
            .child(footer)
            .into_any_element()
    }
}
