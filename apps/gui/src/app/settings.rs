use std::sync::LazyLock;

use super::util::*;
use super::*;
use gpui_kit::component::link::Link;
use gpui_kit::component::switch::Switch;

/// 应用图标与 MIT 许可协议正文在编译期嵌入，运行时不依赖安装目录里的文件。
const APP_ICON_SVG: &[u8] = include_bytes!("../../../../assets/icon.svg");
const APP_LICENSE: &str = include_str!("../../../../LICENSE");
/// 彩色图标只构造一次；`ImageFormat::Svg` 让 GPUI 走位图栅格化，保留原始配色。
static APP_ICON: LazyLock<Arc<gpui_kit::Image>> = LazyLock::new(|| {
    Arc::new(gpui_kit::Image::from_bytes(
        gpui_kit::ImageFormat::Svg,
        APP_ICON_SVG.to_vec(),
    ))
});
/// 与 README、发布脚本保持一致的仓库地址。
const REPOSITORY_URL: &str = "https://github.com/ccpwcn/md-work";
const REPOSITORY_LABEL: &str = "github.com/ccpwcn/md-work";
/// build.rs 在编译期注入的仓库最后一次提交哈希；源码包 / 无 git 时为空。
const GIT_COMMIT: &str = env!("MDWORK_GIT_COMMIT");

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
            font_picker: None,
            show_license: false,
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
        let extension = self.theme_ext.active_definition();
        theme::apply(&self.config, extension.as_ref(), Some(window), cx);
        self.save_config(cx);
        cx.refresh_windows();
    }

    /// 系统字体枚举放到后台线程，只做一次并缓存，字体再多也不会卡界面。
    pub(super) fn ensure_installed_fonts(&mut self, cx: &mut Context<Self>) {
        if self.installed_fonts.is_some() || self.fonts_loading {
            return;
        }
        self.fonts_loading = true;
        // `Context` 可能解引用到返回 `&Arc<TextSystem>` 的那一侧；显式克隆
        // 出拥有所有权的 Arc，才能安全移进后台任务。
        let text_system = cx.text_system().clone();
        cx.spawn(async move |this, cx| {
            let fonts = cx
                .background_executor()
                .spawn(async move { text_system.all_font_names() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.installed_fonts = Some(Arc::new(fonts));
                this.fonts_loading = false;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn open_font_picker(
        &mut self,
        target: FontPickerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("settings.font_picker_search").to_string())
                .clean_on_escape()
        });
        cx.subscribe(&query, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        if let Some(dialog) = self.settings.as_mut() {
            dialog.font_picker = Some(FontPicker { target, query });
        }
        self.font_list_scroll = UniformListScrollHandle::new();
        self.ensure_installed_fonts(cx);
        cx.notify();
    }

    pub(super) fn close_font_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.settings.as_mut() {
            dialog.font_picker = None;
        }
        cx.notify();
    }

    /// 选中字体写入对应输入框并立即应用；空字符串表示系统默认字体。
    pub(super) fn choose_font(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.settings.as_ref() else {
            return;
        };
        let Some(picker) = &dialog.font_picker else {
            return;
        };
        let input = match picker.target {
            FontPickerTarget::Ui => dialog.ui_font.clone(),
            FontPickerTarget::Note => dialog.note_font.clone(),
        };
        input.update(cx, |state, cx| state.set_value(name, window, cx));
        if let Some(dialog) = self.settings.as_mut() {
            dialog.font_picker = None;
        }
        self.apply_typography(window, cx);
        cx.notify();
    }

    /// 恢复出厂字体与字号：两个字体名留空（跟随系统），字号 15 / 16。
    pub(super) fn restore_default_typography(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.settings.as_ref() else {
            return;
        };
        let defaults = AppConfig::default();
        let inputs = [
            (dialog.ui_font.clone(), String::new()),
            (dialog.note_font.clone(), String::new()),
            (
                dialog.ui_size.clone(),
                format!("{:.0}", defaults.ui_font_size),
            ),
            (
                dialog.note_size.clone(),
                format!("{:.0}", defaults.note_font_size),
            ),
        ];
        for (input, value) in inputs {
            input.update(cx, |state, cx| state.set_value(value, window, cx));
        }
        if let Some(dialog) = self.settings.as_mut() {
            dialog.font_picker = None;
        }
        self.apply_typography(window, cx);
        cx.notify();
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

    /// 「关于」页里展开 / 收起许可协议全文。
    pub(super) fn toggle_settings_license(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.settings.as_mut() {
            dialog.show_license = !dialog.show_license;
        }
        cx.notify();
    }

    pub(super) fn set_theme_preference(
        &mut self,
        preference: ThemePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The three built-in choices are entries in `theme_extension.toml`'s
        // `active` key, so the mode switch and the extension dropdown always
        // agree on the currently selected theme.
        let active = match preference {
            ThemePreference::Light => mdwork_core::theme_ext::ActiveTheme::BuiltinLight,
            ThemePreference::Dark => mdwork_core::theme_ext::ActiveTheme::BuiltinDark,
            ThemePreference::System => mdwork_core::theme_ext::ActiveTheme::System,
        };
        self.set_active_theme(active, window, cx);
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

    pub(super) fn set_bookmark_marker(&mut self, marker: BookmarkMarker, cx: &mut Context<Self>) {
        if self.config.bookmark_marker == marker {
            return;
        }
        let previous = self.config.bookmark_marker;
        self.config.bookmark_marker = marker;
        self.save_config(cx);
        tracing::info!(from = ?previous, to = ?marker, "切换笔记树书签标记");
        cx.notify();
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
                SettingsTab::About => "settings-nav-about",
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
        let theme_active = |preference: ThemePreference| {
            let active = match preference {
                ThemePreference::Light => mdwork_core::theme_ext::ActiveTheme::BuiltinLight,
                ThemePreference::Dark => mdwork_core::theme_ext::ActiveTheme::BuiltinDark,
                ThemePreference::System => mdwork_core::theme_ext::ActiveTheme::System,
            };
            self.theme_ext.config.active == active
        };
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
        // 已安装字体用虚拟列表渲染：字体再多也只绘制视口内的十几行，
        // 配合输入框筛选，不会出现几百上千个组件的卡顿。
        let font_picker_panel: Option<AnyElement> = dialog.font_picker.as_ref().map(|picker| {
            let query = picker.query.read(cx).value().trim().to_lowercase();
            let fonts = self.installed_fonts.clone().unwrap_or_default();
            let shown: Arc<Vec<String>> = Arc::new(
                fonts
                    .iter()
                    .filter(|name| query.is_empty() || name.to_lowercase().contains(&query))
                    .cloned()
                    .collect(),
            );
            // 第 0 项固定是「系统默认（留空）」。
            let count = shown.len() + 1;
            let shell = cx.entity();
            let hover = p.hover;
            let scroll = self.font_list_scroll.clone();
            let shown_for_list = shown.clone();
            let list = uniform_list("settings-font-list", count, move |range, _, _| {
                range
                    .map(|index| {
                        let label = if index == 0 {
                            t!("settings.font_picker_default").to_string()
                        } else {
                            shown_for_list.get(index - 1).cloned().unwrap_or_default()
                        };
                        let selected = (index != 0).then(|| label.clone());
                        let shell = shell.clone();
                        div()
                            .id(format!("settings-font-item-{index}"))
                            .h(px(26.))
                            .w_full()
                            .px_2()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .on_click(move |_, window, cx| {
                                let name = selected.clone().unwrap_or_default();
                                shell.update(cx, |this, cx| this.choose_font(name, window, cx));
                            })
                            .child(div().text_xs().truncate().child(label))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .h(px(176.))
            .w_full()
            .track_scroll(&scroll)
            .into_any_element();

            let status = if self.fonts_loading {
                t!("settings.font_picker_loading").to_string()
            } else if self
                .installed_fonts
                .as_ref()
                .is_some_and(|fonts| fonts.is_empty())
            {
                t!("settings.font_picker_empty").to_string()
            } else if shown.is_empty() {
                t!("settings.font_picker_no_match").to_string()
            } else {
                t!(
                    "settings.font_picker_count",
                    count = fonts.len(),
                    shown = shown.len()
                )
                .to_string()
            };
            div()
                .v_flex()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(p.border)
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .font_bold()
                                .child(t!("settings.font_picker").to_string()),
                        )
                        .child(
                            Button::new("settings-font-picker-close")
                                .ghost()
                                .compact()
                                .label(t!("settings.font_picker_close").to_string())
                                .on_click(cx.listener(|this, _, _, cx| this.close_font_picker(cx))),
                        ),
                )
                .child(Input::new(&picker.query).w_full().small())
                .child(div().text_xs().text_color(p.muted).child(status))
                .child(list)
                .into_any_element()
        });

        let font_body = div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(mcp_field(
                            t!("settings.ui_font").to_string(),
                            div()
                                .h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(Input::new(&dialog.ui_font).w_full().small()),
                                )
                                .child(
                                    Button::new("settings-ui-font-pick")
                                        .ghost()
                                        .compact()
                                        .icon(IconName::ChevronDown)
                                        .label(t!("settings.font_choose").to_string())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_font_picker(FontPickerTarget::Ui, window, cx)
                                        })),
                                )
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
                            div()
                                .h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(Input::new(&dialog.note_font).w_full().small()),
                                )
                                .child(
                                    Button::new("settings-note-font-pick")
                                        .ghost()
                                        .compact()
                                        .icon(IconName::ChevronDown)
                                        .label(t!("settings.font_choose").to_string())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_font_picker(
                                                FontPickerTarget::Note,
                                                window,
                                                cx,
                                            )
                                        })),
                                )
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
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.apply_typography(window, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new("settings-font-restore")
                            .ghost()
                            .compact()
                            .label(t!("settings.restore_defaults").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.restore_default_typography(window, cx)
                            })),
                    )
                    .child(settings_hint(
                        t!("settings.fonts_empty_hint").to_string(),
                        p,
                    )),
            )
            .children(font_picker_panel)
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

        let marker_active = |marker: BookmarkMarker| self.config.bookmark_marker == marker;
        let bookmark_marker_track = settings_segment_track(
            vec![
                settings_segment(
                    "settings-bookmark-marker-none",
                    t!("settings.bookmark_marker.none").to_string(),
                    None,
                    marker_active(BookmarkMarker::None),
                    cx.listener(|this, _, _, cx| {
                        this.set_bookmark_marker(BookmarkMarker::None, cx)
                    }),
                    p,
                ),
                settings_segment(
                    "settings-bookmark-marker-dot",
                    t!("settings.bookmark_marker.dot").to_string(),
                    None,
                    marker_active(BookmarkMarker::Dot),
                    cx.listener(|this, _, _, cx| this.set_bookmark_marker(BookmarkMarker::Dot, cx)),
                    p,
                ),
                settings_segment(
                    "settings-bookmark-marker-bookmark",
                    t!("settings.bookmark_marker.bookmark").to_string(),
                    Some(IconName::Bookmark),
                    marker_active(BookmarkMarker::Bookmark),
                    cx.listener(|this, _, _, cx| {
                        this.set_bookmark_marker(BookmarkMarker::Bookmark, cx)
                    }),
                    p,
                ),
            ],
            p,
        );
        let bookmark_marker_card = settings_card(
            t!("settings.bookmark_marker").to_string(),
            Some(t!("settings.bookmark_marker_hint").to_string()),
            bookmark_marker_track,
            p,
        );

        let mut cards = vec![theme_card];
        cards.extend(self.render_theme_extension_cards(cx));
        cards.extend([
            language_card,
            font_card,
            autosave_card,
            bookmark_marker_card,
        ]);
        settings_body("settings-appearance-body", cards)
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
                .w_full()
                .items_start()
                .gap_2()
                .child(Icon::new(IconName::Info).small().text_color(p.muted))
                .child(div().v_flex().flex_1().min_w_0().child(settings_hint(
                    t!("settings.workspace_about_text").to_string(),
                    p,
                )))
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

    /// 「关于」页：居中的应用图标，下面依次是版本、许可协议、仓库入口与欢迎文档。
    pub(super) fn render_settings_about(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = SettingsPalette::new(cx);
        let show_license = self
            .settings
            .as_ref()
            .is_some_and(|dialog| dialog.show_license);

        let hero = div()
            .v_flex()
            .w_full()
            .items_center()
            .gap_1()
            .py_3()
            // svg()/Icon 会把图形栅格化成单色 alpha mask，彩色 logo 会糊成色块；
            // img() 走位图通道（ImageFormat::Svg），保留原本的蓝底白字。
            .child(img(APP_ICON.clone()).size(px(64.)))
            .child(
                div()
                    .text_lg()
                    .font_bold()
                    .child(t!("settings.about.app_name").to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(p.muted)
                    .child(t!("settings.version", version = env!("CARGO_PKG_VERSION")).to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(p.muted)
                    .child(t!("settings.about.tagline").to_string()),
            )
            .into_any_element();

        // 哈希右侧跟一个 icon-only 的小按钮，反馈问题时一键复制。
        let commit_value = if GIT_COMMIT.is_empty() {
            t!("settings.about.commit_unknown").to_string()
        } else {
            GIT_COMMIT.to_string()
        };
        let copy_commit = Button::new("settings-about-copy-commit")
            .ghost()
            .compact()
            .icon(IconName::Copy)
            .tooltip(t!("settings.about.commit_copy").to_string())
            .on_click(cx.listener(|this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(GIT_COMMIT.to_string()));
                this.notify_success(t!("settings.about.commit_copied").to_string(), cx);
            }));
        let commit_body = div()
            .h_flex()
            .items_center()
            .gap_1()
            .child(Icon::new(IconName::GitBranch).small().text_color(p.muted))
            .child(div().font_family("Consolas").text_xs().child(commit_value))
            .when(!GIT_COMMIT.is_empty(), |row| row.child(copy_commit))
            .child(div().flex_1())
            .into_any_element();
        let commit_card = settings_card(
            t!("settings.about.commit").to_string(),
            Some(t!("settings.about.commit_hint").to_string()),
            commit_body,
            p,
        );

        // 一行一个 div：GPUI 的普通文本不会把 `\n` 渲染成换行。
        let license_lines: Vec<AnyElement> = APP_LICENSE
            .lines()
            .map(|line| {
                if line.is_empty() {
                    div().h(px(8.)).into_any_element()
                } else {
                    div().text_xs().child(line.to_string()).into_any_element()
                }
            })
            .collect();
        let license_text = div()
            .id("settings-about-license-text")
            .v_flex()
            .w_full()
            .h(px(132.))
            .p_2()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(p.border)
            .bg(p.track.opacity(0.5))
            .font_family("Consolas")
            .text_xs()
            .overflow_y_scroll()
            .children(license_lines)
            .into_any_element();

        let license_body = div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::Scale).small().text_color(p.muted))
                    .child(
                        Button::new("settings-about-license-toggle")
                            .ghost()
                            .compact()
                            .icon(if show_license {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .label(if show_license {
                                t!("settings.about.license_hide").to_string()
                            } else {
                                t!("settings.about.license_show").to_string()
                            })
                            .on_click(
                                cx.listener(|this, _, _, cx| this.toggle_settings_license(cx)),
                            ),
                    ),
            )
            .when(show_license, |body| body.child(license_text))
            .into_any_element();
        let license_card = settings_card(
            t!("settings.about.license").to_string(),
            Some(t!("settings.about.license_summary").to_string()),
            license_body,
            p,
        );

        let github_card = settings_card(
            t!("settings.about.github").to_string(),
            Some(t!("settings.about.github_hint").to_string()),
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(Icon::new(IconName::Github).small().text_color(p.muted))
                .child(
                    div().flex_1().min_w_0().child(
                        Link::new("settings-about-github-link")
                            .href(REPOSITORY_URL)
                            .text_xs()
                            .child(REPOSITORY_LABEL.to_string()),
                    ),
                )
                .child(
                    Button::new("settings-about-github")
                        .outline()
                        .compact()
                        .icon(IconName::ExternalLink)
                        .label(t!("settings.about.github_open").to_string())
                        .on_click(cx.listener(|_, _, _, cx| cx.open_url(REPOSITORY_URL))),
                )
                .into_any_element(),
            p,
        );

        let welcome_row = div()
            .h_flex()
            .w_full()
            .justify_center()
            .py_1()
            .child(
                Button::new("settings-open-welcome")
                    .outline()
                    .icon(IconName::BookOpen)
                    .label(t!("settings.open_welcome").to_string())
                    .tooltip(t!("settings.open_welcome_hint").to_string())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_settings(window, cx);
                        this.open_official_doc(OfficialDocKind::Welcome, window, cx);
                    })),
            )
            .into_any_element();

        settings_body(
            "settings-about-body",
            vec![hero, commit_card, license_card, github_card, welcome_row],
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
            SettingsTab::About => self.render_settings_about(cx),
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
            SettingsTab::About => (t!("settings.tab.about"), t!("settings.tab.about_hint")),
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
                                    .child(settings_group_label(
                                        t!("settings.nav.about").to_string(),
                                        p,
                                    ))
                                    .child(self.settings_nav(
                                        SettingsTab::About,
                                        IconName::Info,
                                        "settings.tab.about",
                                        cx,
                                    )),
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
