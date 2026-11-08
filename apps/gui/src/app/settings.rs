use super::util::*;
use super::*;
use gpui_kit::component::switch::Switch;

impl AppShell {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        self.open_image_settings(window, cx);
        self.open_mcp_settings(window, cx);
        let ui_font = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(self.config.ui_font.clone())
                .placeholder("Segoe UI / Microsoft YaHei UI")
        });
        let note_font = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(self.config.note_font.clone())
                .placeholder("Consolas / 霞鹜文楷")
        });
        let ui_size = cx.new(|cx| {
            InputState::new(window, cx).default_value(format!("{:.0}", self.config.ui_font_size))
        });
        let note_size = cx.new(|cx| {
            InputState::new(window, cx).default_value(format!("{:.0}", self.config.note_font_size))
        });
        let workspace_alias = self
            .current_workspace_root()
            .and_then(|root| {
                self.workspace_registry
                    .get(&root)
                    .map(|entry| entry.name.clone())
            })
            .unwrap_or_default();
        let workspace_name = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(workspace_alias)
                .placeholder(t!("workspace.name_placeholder").to_string())
        });
        self.settings = Some(SettingsDialog {
            tab: SettingsTab::Workspace,
            workspace_name,
            ui_font,
            note_font,
            ui_size,
            note_size,
        });
        cx.notify();
    }

    /// Copies the font fields into the config, applies them and persists.
    pub(super) fn apply_typography(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings.as_ref() else {
            return;
        };
        let ui_font = dialog.ui_font.read(cx).value().trim().to_string();
        let note_font = dialog.note_font.read(cx).value().trim().to_string();
        let ui_size = parse_font_size(&dialog.ui_size.read(cx).value(), self.config.ui_font_size);
        let note_size = parse_font_size(
            &dialog.note_size.read(cx).value(),
            self.config.note_font_size,
        );
        if self.config.ui_font == ui_font
            && self.config.note_font == note_font
            && self.config.ui_font_size == ui_size
            && self.config.note_font_size == note_size
        {
            return;
        }
        self.config.ui_font = ui_font;
        self.config.note_font = note_font;
        self.config.ui_font_size = ui_size;
        self.config.note_font_size = note_size;
        theme::apply(&self.config, window, cx);
        self.save_config(cx);
        cx.refresh_windows();
    }

    pub(super) fn set_autosave(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.config.autosave == enabled {
            return;
        }
        self.config.autosave = enabled;
        self.save_config(cx);
        for doc in &self.docs {
            doc.update(cx, |doc, cx| doc.set_autosave(enabled, cx));
        }
        cx.notify();
    }

    pub(super) fn register_file_assoc(&mut self, cx: &mut Context<Self>) {
        let receiver = disk::submit(crate::file_assoc::ensure_registered);
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.notify_success(t!("settings.windows_register_ok").to_string(), cx);
                        tracing::info!("已注册「用墨笺打开」右键菜单");
                    }
                    Err(err) => {
                        this.notify_error(
                            format!("{}: {err}", t!("settings.windows_register_failed")),
                            cx,
                        );
                        tracing::warn!(error = %err, "注册文件关联失败");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_windows_default_apps(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = std::process::Command::new("explorer.exe")
            .arg("ms-settings:defaultapps")
            .spawn()
        {
            self.notify_error(
                format!("{}: {err}", t!("settings.windows_default_apps_failed")),
                cx,
            );
        }
        cx.notify();
    }

    pub(super) fn open_path_in_explorer(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(path) = path else {
            return;
        };
        let receiver = disk::submit(move || -> std::io::Result<()> {
            if !path.exists() {
                let _ = std::fs::create_dir_all(&path);
            }
            std::process::Command::new("explorer.exe")
                .arg(&path)
                .spawn()
                .map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                if let Err(err) = result {
                    this.notify_error(
                        format!("{}: {err}", t!("settings.windows_open_folder_failed")),
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_path_with_default_app(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let receiver = disk::submit(move || open_path_with_default_app_sync(&path));
        cx.spawn(async move |this, cx| {
            let result = match receiver.await {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                if let Err(err) = result {
                    this.notify_error(format!("{}: {err}", t!("export.open_after_failed")), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_typography(window, cx);
        self.apply_workspace_name(cx);
        self.settings = None;
        self.image_settings = None;
        self.mcp_settings = None;
        cx.notify();
    }

    pub(super) fn set_settings_tab(&mut self, tab: SettingsTab, cx: &mut Context<Self>) {
        if let Some(dialog) = self.settings.as_mut() {
            if dialog.tab == tab {
                return;
            }
            dialog.tab = tab;
        }
        cx.notify();
    }

    pub(super) fn set_theme_preference(
        &mut self,
        preference: ThemePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.config.theme == preference {
            return;
        }
        let previous = self.config.theme;
        self.config.theme = preference;
        theme::apply(&self.config, window, cx);
        self.save_config(cx);
        tracing::info!(from = ?previous, to = ?preference, "切换主题");
        cx.notify();
    }

    pub(super) fn set_language(&mut self, language: &str, cx: &mut Context<Self>) {
        if self.config.language == language {
            return;
        }
        let previous = self.config.language.clone();
        self.config.language = language.to_string();
        rust_i18n::set_locale(&self.config.language);
        self.save_config(cx);
        tracing::info!(from = %previous, to = %self.config.language, "切换语言");
        cx.refresh_windows();
    }

    /// 侧边导航项：选中项带强调条、浮起白块和箭头。
    pub(super) fn settings_nav(
        &self,
        tab: SettingsTab,
        icon: IconName,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .settings
            .as_ref()
            .is_some_and(|dialog| dialog.tab == tab);
        let p = SettingsPalette::new(cx);
        div()
            .id(match tab {
                SettingsTab::Workspace => "settings-nav-workspace",
                SettingsTab::Appearance => "settings-nav-appearance",
                SettingsTab::Panels => "settings-nav-panels",
                SettingsTab::Images => "settings-nav-images",
                SettingsTab::Mcp => "settings-nav-mcp",
                SettingsTab::Windows => "settings-nav-windows",
            })
            .h_flex()
            .h(px(34.))
            .items_center()
            .gap_2()
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .text_color(if active { p.foreground } else { p.muted })
            .bg(if active { p.popover } else { p.transparent })
            .hover(move |style| style.bg(p.hover))
            .on_click(cx.listener(move |this, _, _, cx| this.set_settings_tab(tab, cx)))
            .child(
                div()
                    .w(px(3.))
                    .h(px(16.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(if active { p.accent } else { p.transparent }),
            )
            .child(
                Icon::new(icon)
                    .small()
                    .text_color(if active { p.foreground } else { p.muted }),
            )
            .child(div().flex_1().min_w_0().child(t!(key).to_string()))
            .when(active, |item| {
                item.child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(p.muted),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_settings_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = SettingsPalette::new(cx);
        let theme_active = |preference: ThemePreference| self.config.theme == preference;
        let theme_track = settings_segment_track(
            vec![
                settings_segment(
                    "settings-theme-system",
                    t!("theme.system").to_string(),
                    Some(IconName::Monitor),
                    theme_active(ThemePreference::System),
                    cx.listener(|this, _, window, cx| {
                        this.set_theme_preference(ThemePreference::System, window, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "settings-theme-light",
                    t!("theme.light").to_string(),
                    Some(IconName::Sun),
                    theme_active(ThemePreference::Light),
                    cx.listener(|this, _, window, cx| {
                        this.set_theme_preference(ThemePreference::Light, window, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "settings-theme-dark",
                    t!("theme.dark").to_string(),
                    Some(IconName::Moon),
                    theme_active(ThemePreference::Dark),
                    cx.listener(|this, _, window, cx| {
                        this.set_theme_preference(ThemePreference::Dark, window, cx)
                    }),
                    p,
                ),
            ],
            p,
        );
        let theme_card = settings_card(
            t!("settings.theme").to_string(),
            Some(t!("settings.theme_hint").to_string()),
            theme_track,
            p,
        );

        let language_track = settings_segment_track(
            vec![
                settings_segment(
                    "settings-language-zh",
                    "中文".to_string(),
                    None,
                    self.config.language == "zh-CN",
                    cx.listener(|this, _, _, cx| this.set_language("zh-CN", cx)),
                    p,
                ),
                settings_segment(
                    "settings-language-en",
                    "English".to_string(),
                    None,
                    self.config.language == "en",
                    cx.listener(|this, _, _, cx| this.set_language("en", cx)),
                    p,
                ),
            ],
            p,
        );
        let language_card = settings_card(
            t!("settings.language").to_string(),
            None,
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(language_track)
                .child(Icon::new(IconName::Languages).small().text_color(p.muted))
                .into_any_element(),
            p,
        );

        let Some(dialog) = self.settings.as_ref() else {
            return div().into_any_element();
        };
        let font_body =
            div()
                .v_flex()
                .gap_2()
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div().flex_1().min_w_0().child(mcp_field(
                                t!("settings.ui_font").to_string(),
                                Input::new(&dialog.ui_font)
                                    .w_full()
                                    .small()
                                    .into_any_element(),
                                p.muted,
                            )),
                        )
                        .child(
                            div().w(px(76.)).child(mcp_field(
                                t!("settings.size").to_string(),
                                Input::new(&dialog.ui_size)
                                    .w_full()
                                    .small()
                                    .into_any_element(),
                                p.muted,
                            )),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            div().flex_1().min_w_0().child(mcp_field(
                                t!("settings.note_font").to_string(),
                                Input::new(&dialog.note_font)
                                    .w_full()
                                    .small()
                                    .into_any_element(),
                                p.muted,
                            )),
                        )
                        .child(
                            div().w(px(76.)).child(mcp_field(
                                t!("settings.size").to_string(),
                                Input::new(&dialog.note_size)
                                    .w_full()
                                    .small()
                                    .into_any_element(),
                                p.muted,
                            )),
                        ),
                )
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("settings-font-apply")
                                .primary()
                                .compact()
                                .icon(IconName::Type)
                                .label(t!("settings.apply_fonts").to_string())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.apply_typography(window, cx)
                                })),
                        )
                        .child(settings_hint(
                            t!("settings.fonts_empty_hint").to_string(),
                            p,
                        )),
                )
                .into_any_element();
        let font_card = settings_card(
            t!("settings.typography").to_string(),
            Some(t!("settings.typography_hint").to_string()),
            font_body,
            p,
        );

        let autosave_card = settings_card(
            t!("settings.autosave").to_string(),
            Some(t!("settings.autosave_hint").to_string()),
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    Switch::new("settings-autosave")
                        .checked(self.config.autosave)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.set_autosave(*checked, cx)
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if self.config.autosave {
                            p.success
                        } else {
                            p.muted
                        })
                        .child(if self.config.autosave {
                            t!("settings.autosave_on").to_string()
                        } else {
                            t!("settings.autosave_off").to_string()
                        }),
                )
                .into_any_element(),
            p,
        );

        settings_body(
            "settings-appearance-body",
            vec![theme_card, language_card, font_card, autosave_card],
        )
    }

    pub(super) fn render_settings_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = SettingsPalette::new(cx);
        let Some(dialog) = self.settings.as_ref() else {
            return div().into_any_element();
        };
        let root = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().display().to_string())
            .unwrap_or_default();
        let folder_name = self
            .workspace
            .as_ref()
            .and_then(|workspace| {
                workspace
                    .root()
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let registered_workspaces = self.workspace_registry.workspaces.len();

        let name_card = settings_card(
            t!("settings.workspace_name").to_string(),
            Some(t!("settings.workspace_name_hint").to_string()),
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&dialog.workspace_name).w_full().small()),
                )
                .child(
                    Button::new("settings-workspace-name-save")
                        .primary()
                        .compact()
                        .icon(IconName::Save)
                        .label(t!("common.save").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.apply_workspace_name(cx))),
                )
                .into_any_element(),
            p,
        );

        let location_summary = if folder_name.is_empty() {
            t!("workspace.none").to_string()
        } else {
            format!(
                "{} · {}",
                folder_name,
                t!("settings.workspace_count", count = registered_workspaces)
            )
        };
        let location_card = settings_card(
            t!("settings.workspace_location").to_string(),
            Some(location_summary),
            div()
                .v_flex()
                .gap_2()
                .child(settings_path_box(root, p))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("settings-workspace-open")
                                .compact()
                                .icon(IconName::FolderOpen)
                                .label(t!("settings.windows_open_workspace").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_path_in_explorer(this.current_workspace_root(), cx)
                                })),
                        )
                        .child(
                            Button::new("settings-workspace-switch")
                                .ghost()
                                .compact()
                                .icon(IconName::FolderCog)
                                .label(t!("workspace.pick").to_string())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_workspace_folder(window, cx)
                                })),
                        )
                        .into_any_element(),
                )
                .into_any_element(),
            p,
        );

        let about_card = settings_card(
            t!("settings.workspace_about").to_string(),
            None,
            div()
                .h_flex()
                .items_start()
                .gap_2()
                .child(Icon::new(IconName::Info).small().text_color(p.muted))
                .child(settings_hint(
                    t!("settings.workspace_about_text").to_string(),
                    p,
                ))
                .into_any_element(),
            p,
        );

        settings_body(
            "settings-workspace-body",
            vec![name_card, location_card, about_card],
        )
    }

    pub(super) fn render_settings_windows(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = SettingsPalette::new(cx);
        let workspace = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().display().to_string())
            .unwrap_or_default();
        let registered = crate::file_assoc::is_registered();
        let status_color = if registered { p.success } else { p.muted };
        let status_label = if registered {
            t!("settings.windows_assoc_on").to_string()
        } else {
            t!("settings.windows_assoc_off").to_string()
        };

        let assoc_card =
            settings_card(
                t!("settings.windows_assoc").to_string(),
                None,
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(settings_status_badge(status_label, status_color))
                            .child(
                                Icon::new(if registered {
                                    IconName::ShieldCheck
                                } else {
                                    IconName::Info
                                })
                                .small()
                                .text_color(status_color),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("settings-register-assoc")
                                    .primary()
                                    .compact()
                                    .icon(IconName::Link)
                                    .label(t!("settings.windows_register").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.register_file_assoc(cx)),
                                    ),
                            )
                            .child(
                                Button::new("settings-default-apps")
                                    .ghost()
                                    .compact()
                                    .icon(IconName::ExternalLink)
                                    .label(t!("settings.windows_default_apps").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_windows_default_apps(cx)
                                    })),
                            ),
                    )
                    .into_any_element(),
                p,
            );

        let paths_card = settings_card(
            t!("settings.windows_workspace").to_string(),
            Some(t!("settings.windows_paths_hint").to_string()),
            div()
                .v_flex()
                .gap_2()
                .child(settings_path_box(workspace, p))
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("settings-open-workspace")
                                .compact()
                                .icon(IconName::FolderOpen)
                                .label(t!("settings.windows_open_workspace").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_path_in_explorer(
                                        this.workspace
                                            .as_ref()
                                            .map(|workspace| workspace.root().to_path_buf()),
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new("settings-open-logs")
                                .ghost()
                                .compact()
                                .icon(IconName::ScrollText)
                                .label(t!("settings.windows_open_logs").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_path_in_explorer(
                                        Some(mdwork_core::paths::logs_dir()),
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new("settings-open-crashes")
                                .ghost()
                                .compact()
                                .icon(IconName::Bug)
                                .label(t!("settings.windows_open_crashes").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_path_in_explorer(
                                        Some(mdwork_core::paths::crashes_dir()),
                                        cx,
                                    )
                                })),
                        ),
                )
                .into_any_element(),
            p,
        );

        settings_body("settings-windows-body", vec![assoc_card, paths_card])
    }

    pub(super) fn render_settings_panels(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = SettingsPalette::new(cx);

        let arrangement_track = settings_segment_track(
            vec![
                settings_segment(
                    "settings-arrangement-horizontal",
                    t!("settings.arrangement.horizontal").to_string(),
                    Some(IconName::Rows3),
                    self.config.panel_arrangement == PanelArrangement::Horizontal,
                    cx.listener(move |this, _, _, cx| {
                        this.set_panel_arrangement(PanelArrangement::Horizontal, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "settings-arrangement-vertical",
                    t!("settings.arrangement.vertical").to_string(),
                    Some(IconName::Columns3),
                    self.config.panel_arrangement == PanelArrangement::Vertical,
                    cx.listener(move |this, _, _, cx| {
                        this.set_panel_arrangement(PanelArrangement::Vertical, cx)
                    }),
                    p,
                ),
            ],
            p,
        );
        let arrangement_card = settings_card(
            t!("settings.panel_arrangement").to_string(),
            Some(t!("settings.panel_arrangement_hint").to_string()),
            arrangement_track,
            p,
        );

        let width_track = settings_segment_track(
            vec![
                settings_segment(
                    "settings-width-narrow",
                    t!("settings.width.narrow").to_string(),
                    None,
                    (self.config.panel_width - 240.0).abs() < 1.0,
                    cx.listener(|this, _, _, cx| this.set_panel_width(240.0, cx)),
                    p,
                ),
                settings_segment(
                    "settings-width-standard",
                    t!("settings.width.standard").to_string(),
                    None,
                    (self.config.panel_width - 300.0).abs() < 1.0,
                    cx.listener(|this, _, _, cx| this.set_panel_width(300.0, cx)),
                    p,
                ),
                settings_segment(
                    "settings-width-wide",
                    t!("settings.width.wide").to_string(),
                    None,
                    (self.config.panel_width - 380.0).abs() < 1.0,
                    cx.listener(|this, _, _, cx| this.set_panel_width(380.0, cx)),
                    p,
                ),
            ],
            p,
        );
        let width_card = settings_card(
            t!("settings.panel_width").to_string(),
            Some(t!("settings.panel_width_hint").to_string()),
            width_track,
            p,
        );

        // 每个面板一行：图标 + 名称 + 左/右 分段控件。
        let mut dock_list = div().v_flex().gap_1();
        for panel in SidePanel::ALL {
            let current = self.config.panel_dock_for(panel.kind());
            let label = match panel {
                SidePanel::Notes => t!("panel.notes"),
                SidePanel::Outline => t!("panel.outline"),
                SidePanel::Search => t!("panel.search"),
                SidePanel::Bookmarks => t!("panel.bookmarks"),
                SidePanel::Trash => t!("panel.trash"),
            };
            let (left_id, right_id) = match panel {
                SidePanel::Notes => ("settings-dock-notes-left", "settings-dock-notes-right"),
                SidePanel::Outline => ("settings-dock-outline-left", "settings-dock-outline-right"),
                SidePanel::Search => ("settings-dock-search-left", "settings-dock-search-right"),
                SidePanel::Bookmarks => (
                    "settings-dock-bookmarks-left",
                    "settings-dock-bookmarks-right",
                ),
                SidePanel::Trash => ("settings-dock-trash-left", "settings-dock-trash-right"),
            };
            let dock_track = settings_segment_track(
                vec![
                    settings_segment(
                        left_id,
                        t!("settings.dock.left_short").to_string(),
                        Some(IconName::PanelLeft),
                        current == PanelDock::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.set_panel_kind_dock(panel, PanelDock::Left, cx)
                        }),
                        p,
                    ),
                    settings_segment(
                        right_id,
                        t!("settings.dock.right_short").to_string(),
                        Some(IconName::PanelRight),
                        current == PanelDock::Right,
                        cx.listener(move |this, _, _, cx| {
                            this.set_panel_kind_dock(panel, PanelDock::Right, cx)
                        }),
                        p,
                    ),
                ],
                p,
            );
            dock_list = dock_list.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(p.border)
                    .child(
                        Icon::new(Self::panel_icon(panel))
                            .small()
                            .text_color(p.muted),
                    )
                    .child(div().flex_1().min_w_0().text_xs().child(label.to_string()))
                    .child(dock_track),
            );
        }
        let dock_body = div()
            .v_flex()
            .gap_2()
            .child(dock_list)
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(settings_hint(t!("settings.panel_dock_all").to_string(), p))
                    .child(
                        Button::new("settings-dock-all-left")
                            .ghost()
                            .compact()
                            .icon(IconName::PanelLeft)
                            .label(t!("settings.dock.left").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_all_panel_docks(PanelDock::Left, cx)
                            })),
                    )
                    .child(
                        Button::new("settings-dock-all-right")
                            .ghost()
                            .compact()
                            .icon(IconName::PanelRight)
                            .label(t!("settings.dock.right").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_all_panel_docks(PanelDock::Right, cx)
                            })),
                    ),
            )
            .into_any_element();
        let dock_card = settings_card(
            t!("settings.panel_dock").to_string(),
            Some(t!("settings.panel_dock_hint").to_string()),
            dock_body,
            p,
        );

        settings_body(
            "settings-panels-body",
            vec![dock_card, arrangement_card, width_card],
        )
    }

    pub(super) fn render_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.settings.as_ref() else {
            return div().into_any_element();
        };
        let tab = dialog.tab;
        let border = cx.theme().border;
        let popover = cx.theme().popover;
        let foreground = cx.theme().popover_foreground;
        let overlay = cx.theme().overlay;
        let muted = cx.theme().muted_foreground;
        let sidebar = cx.theme().sidebar;
        let sidebar_border = cx.theme().sidebar_border;
        let p = SettingsPalette::new(cx);

        let content = match tab {
            SettingsTab::Workspace => self.render_settings_workspace(cx),
            SettingsTab::Appearance => self.render_settings_appearance(cx),
            SettingsTab::Panels => self.render_settings_panels(cx),
            SettingsTab::Images => self.render_image_settings_page(cx),
            SettingsTab::Mcp => self.render_mcp_settings_page(cx),
            SettingsTab::Windows => self.render_settings_windows(cx),
        };
        let (title, hint) = match tab {
            SettingsTab::Workspace => (
                t!("settings.tab.workspace"),
                t!("settings.tab.workspace_hint"),
            ),
            SettingsTab::Appearance => (
                t!("settings.tab.appearance"),
                t!("settings.tab.appearance_hint"),
            ),
            SettingsTab::Panels => (t!("settings.tab.panels"), t!("settings.tab.panels_hint")),
            SettingsTab::Images => (t!("settings.tab.images"), t!("settings.tab.images_hint")),
            SettingsTab::Mcp => (t!("settings.tab.mcp"), t!("settings.tab.mcp_hint")),
            SettingsTab::Windows => (t!("settings.tab.windows"), t!("settings.tab.windows_hint")),
        };

        div()
            .id("settings-overlay")
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
                    .id("settings-dialog")
                    .v_flex()
                    .w(px(880.))
                    .h(px(600.))
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(popover)
                    .text_color(foreground)
                    .shadow_lg()
                    .overflow_hidden()
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .px_5()
                            .py_3()
                            .border_b_1()
                            .border_color(sidebar_border)
                            .child(Icon::new(IconName::Settings2).small().text_color(muted))
                            .child(
                                div()
                                    .v_flex()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_bold()
                                            .child(t!("settings.title").to_string()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t!("settings.dialog_hint").to_string()),
                                    ),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("settings-close")
                                    .ghost()
                                    .compact()
                                    .icon(IconName::X)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.close_settings(window, cx)
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .v_flex()
                                    .w(px(184.))
                                    .h_full()
                                    .flex_shrink_0()
                                    .gap_0p5()
                                    .px_2()
                                    .py_3()
                                    .bg(sidebar)
                                    .border_r_1()
                                    .border_color(sidebar_border)
                                    .child(settings_group_label(
                                        t!("settings.nav.general").to_string(),
                                        p,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Workspace,
                                        IconName::FolderOpen,
                                        "settings.tab.workspace",
                                        cx,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Appearance,
                                        IconName::Palette,
                                        "settings.tab.appearance",
                                        cx,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Panels,
                                        IconName::LayoutPanelLeft,
                                        "settings.tab.panels",
                                        cx,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Windows,
                                        IconName::Monitor,
                                        "settings.tab.windows",
                                        cx,
                                    ))
                                    .child(settings_group_label(
                                        t!("settings.nav.integration").to_string(),
                                        p,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Images,
                                        IconName::Image,
                                        "settings.tab.images",
                                        cx,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::Mcp,
                                        IconName::ServerCog,
                                        "settings.tab.mcp",
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .mt_auto()
                                            .h_flex()
                                            .items_center()
                                            .gap_1()
                                            .px_2()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(Icon::new(IconName::Info).xsmall())
                                            .child(
                                                t!(
                                                    "settings.version",
                                                    version = env!("CARGO_PKG_VERSION")
                                                )
                                                .to_string(),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .id("settings-content")
                                    .v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .overflow_hidden()
                                    .px_5()
                                    .py_4()
                                    .gap_3()
                                    .child(
                                        div()
                                            .v_flex()
                                            .gap_0p5()
                                            .child(
                                                div()
                                                    .text_base()
                                                    .font_bold()
                                                    .child(title.to_string()),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(hint.to_string()),
                                            ),
                                    )
                                    .child(content),
                            ),
                    ),
            )
            .into_any_element()
    }
}
