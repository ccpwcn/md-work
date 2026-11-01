use super::util::*;
use super::*;

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
                    this.notify_error(
                        format!("{}: {err}", t!("export.open_after_failed")),
                        cx,
                    );
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

    /// Docks a single panel to `dock` and makes it the active panel there.
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
        let color = if active {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        };
        let hover = cx.theme().list_hover;
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
            .h(px(32.))
            .px_3()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .text_sm()
            .text_color(if active {
                cx.theme().foreground
            } else {
                cx.theme().muted_foreground
            })
            .bg(if active {
                cx.theme().list_active
            } else {
                cx.theme().transparent
            })
            .hover(move |style| style.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| this.set_settings_tab(tab, cx)))
            .child(Icon::new(icon).small().text_color(color))
            .child(t!(key).to_string())
            .into_any_element()
    }

    pub(super) fn render_settings_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let dangerish = cx.theme().border;

        let theme_button =
            |preference: ThemePreference, key: &'static str, cx: &mut Context<Self>| {
                let active = self.config.theme == preference;
                let id = match preference {
                    ThemePreference::System => "settings-theme-system",
                    ThemePreference::Light => "settings-theme-light",
                    ThemePreference::Dark => "settings-theme-dark",
                };
                Button::new(id)
                    .ghost()
                    .compact()
                    .label(t!(key).to_string())
                    .when(active, |button| button.primary())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_theme_preference(preference, window, cx)
                    }))
            };

        let language_button =
            |language: &'static str, label: &'static str, cx: &mut Context<Self>| {
                let active = self.config.language == language;
                Button::new(match language {
                    "zh-CN" => "settings-language-zh",
                    _ => "settings-language-en",
                })
                .ghost()
                .compact()
                .label(label)
                .when(active, |button| button.primary())
                .on_click(cx.listener(move |this, _, _, cx| this.set_language(language, cx)))
            };

        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.theme").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.theme_hint").to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(theme_button(ThemePreference::System, "theme.system", cx))
                    .child(theme_button(ThemePreference::Light, "theme.light", cx))
                    .child(theme_button(ThemePreference::Dark, "theme.dark", cx)),
            )
            .child(div().my_2().h(px(1.)).bg(dangerish))
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.language").to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(language_button("zh-CN", "中文", cx))
                    .child(language_button("en", "English", cx)),
            )
            .child(div().my_2().h(px(1.)).bg(dangerish))
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.typography").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.typography_hint").to_string()),
            )
            .child({
                let (ui_font, note_font, ui_size, note_size) = match self.settings.as_ref() {
                    Some(dialog) => (
                        dialog.ui_font.clone(),
                        dialog.note_font.clone(),
                        dialog.ui_size.clone(),
                        dialog.note_size.clone(),
                    ),
                    None => return div().into_any_element(),
                };
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t!("settings.ui_font").to_string()),
                                    )
                                    .child(Input::new(&ui_font).small()),
                            )
                            .child(
                                div()
                                    .w(px(72.))
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t!("settings.size").to_string()),
                                    )
                                    .child(Input::new(&ui_size).small()),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t!("settings.note_font").to_string()),
                                    )
                                    .child(Input::new(&note_font).small()),
                            )
                            .child(
                                div()
                                    .w(px(72.))
                                    .v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(t!("settings.size").to_string()),
                                    )
                                    .child(Input::new(&note_size).small()),
                            ),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("settings-font-apply")
                                    .primary()
                                    .compact()
                                    .label(t!("settings.apply_fonts").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.apply_typography(window, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t!("settings.fonts_empty_hint").to_string()),
                            ),
                    )
                    .into_any_element()
            })
            .child(div().my_2().h(px(1.)).bg(dangerish))
            .child(
                Checkbox::new("settings-autosave")
                    .label(t!("settings.autosave").to_string())
                    .checked(self.config.autosave)
                    .on_click(
                        cx.listener(|this, checked: &bool, _, cx| this.set_autosave(*checked, cx)),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.autosave_hint").to_string()),
            )
            .into_any_element()
    }

    pub(super) fn render_settings_workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let Some(dialog) = self.settings.as_ref() else {
            return div().into_any_element();
        };
        let input = dialog.workspace_name.clone();
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

        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .child(t!("settings.workspace_name").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("settings.workspace_name_hint").to_string()),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(Input::new(&input).small()))
                            .child(
                                Button::new("settings-workspace-name-save")
                                    .compact()
                                    .label(t!("common.save").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.apply_workspace_name(cx)),
                                    ),
                            ),
                    ),
            )
            .child(div().h(px(1.)).bg(border))
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .child(t!("settings.workspace_location").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(if folder_name.is_empty() {
                                t!("workspace.none").to_string()
                            } else {
                                format!(
                                    "{} · {}",
                                    folder_name,
                                    t!("settings.workspace_count", count = registered_workspaces)
                                )
                            }),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(cx.theme().list_hover)
                            .text_xs()
                            .truncate()
                            .child(root),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("settings-workspace-open")
                                    .compact()
                                    .label(t!("settings.windows_open_workspace").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_path_in_explorer(
                                            this.current_workspace_root(),
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                Button::new("settings-workspace-switch")
                                    .ghost()
                                    .compact()
                                    .label(t!("workspace.pick").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_workspace_folder(window, cx)
                                    })),
                            ),
                    ),
            )
            .child(div().h(px(1.)).bg(border))
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .child(t!("settings.workspace_about").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("settings.workspace_about_text").to_string()),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_settings_windows(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let workspace = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.root().display().to_string())
            .unwrap_or_default();
        let registered = crate::file_assoc::is_registered();

        let mut rows = div().v_flex().gap_3();

        rows =
            rows.child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .child(t!("settings.windows_assoc").to_string()),
                    )
                    .child(div().text_xs().text_color(muted).child(if registered {
                        t!("settings.windows_assoc_on").to_string()
                    } else {
                        t!("settings.windows_assoc_off").to_string()
                    }))
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                Button::new("settings-register-assoc")
                                    .compact()
                                    .label(t!("settings.windows_register").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.register_file_assoc(cx)),
                                    ),
                            )
                            .child(
                                Button::new("settings-default-apps")
                                    .ghost()
                                    .compact()
                                    .label(t!("settings.windows_default_apps").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.open_windows_default_apps(cx)
                                    })),
                            ),
                    ),
            );

        rows = rows.child(div().h(px(1.)).bg(border)).child(
            div()
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_bold()
                        .child(t!("settings.windows_workspace").to_string()),
                )
                .child(div().text_xs().text_color(muted).child(workspace.clone()))
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .child(
                            Button::new("settings-open-workspace")
                                .compact()
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
                                .label(t!("settings.windows_open_crashes").to_string())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.open_path_in_explorer(
                                        Some(mdwork_core::paths::crashes_dir()),
                                        cx,
                                    )
                                })),
                        ),
                ),
        );

        rows.into_any_element()
    }

    pub(super) fn render_settings_panels(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let card = cx.theme().sidebar;

        let arrangement_button = |arrangement: PanelArrangement,
                                  icon: IconName,
                                  key: &'static str,
                                  cx: &mut Context<Self>| {
            let active = self.config.panel_arrangement == arrangement;
            Button::new(match arrangement {
                PanelArrangement::Horizontal => "settings-arrangement-horizontal",
                PanelArrangement::Vertical => "settings-arrangement-vertical",
            })
            .ghost()
            .compact()
            .icon(icon)
            .label(t!(key).to_string())
            .when(active, |button| button.primary())
            .on_click(
                cx.listener(move |this, _, _, cx| this.set_panel_arrangement(arrangement, cx)),
            )
        };

        let width_button =
            |width: f32, id: &'static str, key: &'static str, cx: &mut Context<Self>| {
                let active = (self.config.panel_width - width).abs() < 1.0;
                Button::new(id)
                    .ghost()
                    .compact()
                    .label(t!(key).to_string())
                    .when(active, |button| button.primary())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_panel_width(width, cx)))
            };

        // Per-panel dock rows: each of the five panels can sit left or right.
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
            let side_button =
                |dock: PanelDock,
                 id: &'static str,
                 panel: SidePanel,
                 current: PanelDock,
                 cx: &mut Context<Self>| {
                    Button::new(id)
                        .ghost()
                        .compact()
                        .icon(match dock {
                            PanelDock::Left => IconName::PanelLeft,
                            PanelDock::Right => IconName::PanelRight,
                        })
                        .label(
                            t!(match dock {
                                PanelDock::Left => "settings.dock.left_short",
                                PanelDock::Right => "settings.dock.right_short",
                            })
                            .to_string(),
                        )
                        .when(current == dock, |button| button.primary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_panel_kind_dock(panel, dock, cx)
                        }))
                };
            dock_list = dock_list.child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(card)
                    .child(Icon::new(Self::panel_icon(panel)).small().text_color(muted))
                    .child(div().flex_1().min_w_0().text_sm().child(label.to_string()))
                    .child(side_button(
                        PanelDock::Left,
                        match panel {
                            SidePanel::Notes => "settings-dock-notes-left",
                            SidePanel::Outline => "settings-dock-outline-left",
                            SidePanel::Search => "settings-dock-search-left",
                            SidePanel::Bookmarks => "settings-dock-bookmarks-left",
                            SidePanel::Trash => "settings-dock-trash-left",
                        },
                        panel,
                        current,
                        cx,
                    ))
                    .child(side_button(
                        PanelDock::Right,
                        match panel {
                            SidePanel::Notes => "settings-dock-notes-right",
                            SidePanel::Outline => "settings-dock-outline-right",
                            SidePanel::Search => "settings-dock-search-right",
                            SidePanel::Bookmarks => "settings-dock-bookmarks-right",
                            SidePanel::Trash => "settings-dock-trash-right",
                        },
                        panel,
                        current,
                        cx,
                    )),
            );
        }

        let all_button = |dock: PanelDock, id: &'static str, cx: &mut Context<Self>| {
            Button::new(id)
                .ghost()
                .compact()
                .icon(match dock {
                    PanelDock::Left => IconName::PanelLeft,
                    PanelDock::Right => IconName::PanelRight,
                })
                .label(
                    t!(match dock {
                        PanelDock::Left => "settings.dock.left",
                        PanelDock::Right => "settings.dock.right",
                    })
                    .to_string(),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.set_all_panel_docks(dock, cx)))
        };

        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.panel_dock").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.panel_dock_hint").to_string()),
            )
            .child(dock_list)
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("settings.panel_dock_all").to_string()),
                    )
                    .child(all_button(PanelDock::Left, "settings-dock-all-left", cx))
                    .child(all_button(PanelDock::Right, "settings-dock-all-right", cx)),
            )
            .child(div().my_2().h(px(1.)).bg(border))
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.panel_arrangement").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.panel_arrangement_hint").to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(arrangement_button(
                        PanelArrangement::Horizontal,
                        IconName::Rows3,
                        "settings.arrangement.horizontal",
                        cx,
                    ))
                    .child(arrangement_button(
                        PanelArrangement::Vertical,
                        IconName::Columns3,
                        "settings.arrangement.vertical",
                        cx,
                    )),
            )
            .child(div().my_2().h(px(1.)).bg(border))
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("settings.panel_width").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("settings.panel_width_hint").to_string()),
            )
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(width_button(
                        240.0,
                        "settings-width-narrow",
                        "settings.width.narrow",
                        cx,
                    ))
                    .child(width_button(
                        300.0,
                        "settings-width-standard",
                        "settings.width.standard",
                        cx,
                    ))
                    .child(width_button(
                        380.0,
                        "settings-width-wide",
                        "settings.width.wide",
                        cx,
                    )),
            )
            .into_any_element()
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

        let content = match tab {
            SettingsTab::Workspace => self.render_settings_workspace(cx),
            SettingsTab::Appearance => self.render_settings_appearance(cx),
            SettingsTab::Panels => self.render_settings_panels(cx),
            SettingsTab::Images => self.render_image_settings_page(cx),
            SettingsTab::Mcp => self.render_mcp_settings_page(cx),
            SettingsTab::Windows => self.render_settings_windows(cx),
        };
        let title = match tab {
            SettingsTab::Workspace => t!("settings.tab.workspace"),
            SettingsTab::Appearance => t!("settings.tab.appearance"),
            SettingsTab::Panels => t!("settings.tab.panels"),
            SettingsTab::Images => t!("settings.tab.images"),
            SettingsTab::Mcp => t!("settings.tab.mcp"),
            SettingsTab::Windows => t!("settings.tab.windows"),
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
                            .py_4()
                            .border_b_1()
                            .border_color(sidebar_border)
                            .child(
                                div()
                                    .flex_1()
                                    .text_base()
                                    .font_bold()
                                    .child(t!("settings.title").to_string()),
                            )
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
                                    .w(px(176.))
                                    .h_full()
                                    .flex_shrink_0()
                                    .gap_1()
                                    .px_2()
                                    .py_3()
                                    .bg(sidebar)
                                    .border_r_1()
                                    .border_color(sidebar_border)
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
                                    .child(self.settings_nav(
                                        SettingsTab::Windows,
                                        IconName::Monitor,
                                        "settings.tab.windows",
                                        cx,
                                    ))
                                    .child(
                                        div().mt_auto().px_2().text_xs().text_color(muted).child(
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
                                    .overflow_y_scroll()
                                    .px_5()
                                    .py_4()
                                    .gap_3()
                                    .child(div().text_sm().font_bold().child(title.to_string()))
                                    .child(content),
                            ),
                    ),
            )
            .into_any_element()
    }
}
