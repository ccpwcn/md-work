use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn mcp_tab_button(
        &self,
        tab: McpTab,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> Button {
        let active = self
            .mcp_settings
            .as_ref()
            .is_some_and(|dialog| dialog.tab == tab);
        Button::new(match tab {
            McpTab::Server => "mcp-tab-server",
            McpTab::Tokens => "mcp-tab-tokens",
            McpTab::Audit => "mcp-tab-audit",
        })
        .ghost()
        .compact()
        .label(t!(key).to_string())
        .when(active, |button| button.primary())
        .on_click(cx.listener(move |this, _, _, cx| this.set_mcp_tab(tab, cx)))
    }

    pub(super) fn render_mcp_settings_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return empty_hint(t!("mcp.loading").to_string(), cx.theme().muted_foreground);
        };
        let tab = dialog.tab;
        let has_error = dialog.error.is_some();
        let message = dialog.error.clone().or_else(|| dialog.message.clone());
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;

        let body = match tab {
            McpTab::Server => self.render_mcp_server_tab(cx),
            McpTab::Tokens => self.render_mcp_tokens_tab(cx),
            McpTab::Audit => self.render_mcp_audit_tab(cx),
        };

        let message_color = if has_error { danger } else { muted };
        let footer = div().h_flex().items_center().gap_2().min_h(px(24.)).child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(message_color)
                .when_some(message, |element, text| element.child(text)),
        );

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .child(
                div()
                    .h_flex()
                    .gap_1()
                    .child(self.mcp_tab_button(McpTab::Server, "mcp.tab.server", cx))
                    .child(self.mcp_tab_button(McpTab::Tokens, "mcp.tab.tokens", cx))
                    .child(self.mcp_tab_button(McpTab::Audit, "mcp.tab.audit", cx)),
            )
            .child(body)
            .child(footer)
            .into_any_element()
    }

    pub(super) fn render_mcp_server_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let running = self.mcp.running();
        let mcp_error = self.mcp.error.clone();
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().popover_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;

        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            Checkbox::new("mcp-enabled")
                .label(t!("mcp.enable").to_string())
                .checked(dialog.enabled)
                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                    if let Some(dialog) = this.mcp_settings.as_mut() {
                        dialog.enabled = *checked;
                        dialog.message = None;
                        dialog.error = None;
                    }
                    cx.notify();
                }))
                .into_any_element(),
        );
        rows.push(
            div()
                .text_xs()
                .text_color(muted)
                .child(t!("mcp.enable_hint").to_string())
                .into_any_element(),
        );
        rows.push(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(96.))
                        .text_xs()
                        .text_color(muted)
                        .child(t!("mcp.port").to_string()),
                )
                .child(
                    div()
                        .w(px(110.))
                        .child(Input::new(&dialog.port_input).w_full().small()),
                )
                .child(
                    Button::new("mcp-apply")
                        .primary()
                        .compact()
                        .label(t!("mcp.apply").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.apply_mcp_server(cx))),
                )
                .into_any_element(),
        );
        let status_text = if running {
            t!("mcp.running", port = self.mcp.port).to_string()
        } else {
            t!("mcp.stopped").to_string()
        };
        rows.push(
            div()
                .text_xs()
                .text_color(if running { foreground } else { muted })
                .child(status_text)
                .into_any_element(),
        );
        if !running {
            if let Some(error) = mcp_error {
                rows.push(
                    div()
                        .text_xs()
                        .text_color(danger)
                        .child(error)
                        .into_any_element(),
                );
            }
        }

        let cli = self
            .cli
            .clone()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "md-work-cli.exe".to_string());
        let port = self.config.mcp_port.max(1);
        let sample_token = dialog
            .secret
            .clone()
            .unwrap_or_else(|| t!("mcp.token_placeholder").to_string());
        let snippet = serde_json::json!({
            "mcpServers": {
                "md-work": {
                    "command": cli,
                    "args": ["mcp", "serve", "--stdio"],
                    "env": { "MDWORK_TOKEN": sample_token }
                }
            }
        });
        let snippet = serde_json::to_string_pretty(&snippet).unwrap_or_default();
        rows.push(
            div()
                .text_xs()
                .font_bold()
                .child(t!("mcp.clients").to_string())
                .into_any_element(),
        );
        rows.push(
            div()
                .text_xs()
                .text_color(muted)
                .child(
                    t!(
                        "mcp.snippet_hint",
                        url = format!("http://127.0.0.1:{port}/mcp")
                    )
                    .to_string(),
                )
                .into_any_element(),
        );
        let snippet_copy = snippet.clone();
        rows.push(
            div()
                .v_flex()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .child(div().font_family("Consolas").text_xs().child(snippet))
                .child(
                    Button::new("mcp-copy-snippet")
                        .ghost()
                        .compact()
                        .label(t!("mcp.copy").to_string())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.copy_mcp_text(snippet_copy.clone(), cx)
                        })),
                )
                .into_any_element(),
        );

        div()
            .id("mcp-server-body")
            .v_flex()
            .gap_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }

    pub(super) fn render_mcp_tokens_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let list = self.render_mcp_token_list(cx);
        let editor = self.render_mcp_token_editor(cx);
        div()
            .h_flex()
            .gap_3()
            .flex_1()
            .min_h_0()
            .child(list)
            .child(editor)
            .into_any_element()
    }

    pub(super) fn render_mcp_token_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let hover = cx.theme().list_hover;
        let active_bg = cx.theme().list_active;
        let transparent = cx.theme().transparent;
        let mut rows: Vec<AnyElement> = Vec::new();
        if dialog.tokens.is_empty() {
            rows.push(empty_hint(t!("mcp.no_tokens").to_string(), muted));
        }
        for record in &dialog.tokens {
            let id = record.id;
            let active = dialog.selected == Some(id);
            let status = if record.revoked {
                t!("mcp.status.revoked").to_string()
            } else if !record.is_active(perms::now_unix()) {
                t!("mcp.status.expired").to_string()
            } else {
                t!("mcp.status.active").to_string()
            };
            let scopes_label = record
                .scopes
                .iter()
                .map(|scope| scope_label(*scope))
                .collect::<Vec<_>>()
                .join(" · ");
            rows.push(
                div()
                    .id(format!("mcp-token-{id}"))
                    .v_flex()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if active { active_bg } else { transparent })
                    .hover(move |style| style.bg(hover))
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.select_mcp_token(id, window, cx)
                        }),
                    )
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(record.name.clone()),
                            )
                            .child(div().text_xs().text_color(muted).child(status)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .truncate()
                            .child(scopes_label),
                    )
                    .into_any_element(),
            );
        }

        div()
            .id("mcp-token-list")
            .w(px(230.))
            .min_h_0()
            .v_flex()
            .gap_1()
            .overflow_y_scroll()
            .child(
                Button::new("mcp-new-token")
                    .compact()
                    .label(t!("mcp.create").to_string())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.begin_new_mcp_token(window, cx)),
                    ),
            )
            .children(rows)
            .into_any_element()
    }

    pub(super) fn render_mcp_token_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;
        let editing = dialog.creating || dialog.selected.is_some();
        let mut editor: Vec<AnyElement> = Vec::new();
        if !editing {
            editor.push(empty_hint(t!("mcp.select_or_new").to_string(), muted));
        } else {
            editor.push(mcp_field(
                t!("mcp.name").to_string(),
                Input::new(&dialog.name_input)
                    .w_full()
                    .small()
                    .into_any_element(),
                muted,
            ));
            let mut scope_row = div().h_flex().flex_wrap().gap_3();
            for scope in Scope::ALL {
                let checked = dialog.scopes.contains(&scope);
                scope_row = scope_row.child(
                    Checkbox::new(format!("mcp-scope-{}", scope.as_str()))
                        .label(scope_label(scope))
                        .checked(checked)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.toggle_mcp_scope(scope, *checked, cx)
                        })),
                );
            }
            editor.push(mcp_field(
                t!("mcp.scopes").to_string(),
                scope_row.into_any_element(),
                muted,
            ));

            let known: HashSet<Uuid> = self
                .notebooks
                .iter()
                .map(|entry| entry.notebook.marker.id)
                .collect();
            let mut notebook_rows = div().v_flex().gap_1();
            for entry in &self.notebooks {
                let id = entry.notebook.marker.id;
                let checked = dialog.notebooks.contains(&id);
                notebook_rows = notebook_rows.child(
                    Checkbox::new(format!("mcp-notebook-{id}"))
                        .label(entry.notebook.display_name().to_string())
                        .checked(checked)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.toggle_mcp_notebook(id, *checked, cx)
                        })),
                );
            }
            for id in dialog.notebooks.iter().filter(|id| !known.contains(id)) {
                let id = *id;
                notebook_rows = notebook_rows.child(
                    Checkbox::new(format!("mcp-notebook-{id}"))
                        .label(t!("mcp.unknown_notebook", id = id.to_string()).to_string())
                        .checked(true)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.toggle_mcp_notebook(id, *checked, cx)
                        })),
                );
            }
            editor.push(mcp_field(
                t!("mcp.notebooks").to_string(),
                notebook_rows.into_any_element(),
                muted,
            ));
            editor.push(mcp_field(
                t!("mcp.allow").to_string(),
                Input::new(&dialog.allow_input)
                    .w_full()
                    .small()
                    .into_any_element(),
                muted,
            ));
            editor.push(mcp_field(
                t!("mcp.deny").to_string(),
                Input::new(&dialog.deny_input)
                    .w_full()
                    .small()
                    .into_any_element(),
                muted,
            ));
            editor.push(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("mcp.globs_hint").to_string())
                    .into_any_element(),
            );

            if let Some(secret) = dialog.secret.clone() {
                let copy_value = secret.clone();
                editor.push(
                    div()
                        .v_flex()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .child(
                            div()
                                .h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .font_bold()
                                        .child(t!("mcp.secret_title").to_string()),
                                )
                                .child(
                                    Button::new("mcp-copy-secret")
                                        .ghost()
                                        .compact()
                                        .label(t!("mcp.copy").to_string())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.copy_mcp_text(copy_value.clone(), cx)
                                        })),
                                ),
                        )
                        .child(div().font_family("Consolas").text_xs().child(secret))
                        .child(
                            div()
                                .text_xs()
                                .text_color(danger)
                                .child(t!("mcp.secret_warning").to_string()),
                        )
                        .into_any_element(),
                );
            }

            let has_selected = dialog.selected.is_some();
            let selected_revoked = dialog
                .tokens
                .iter()
                .find(|record| Some(record.id) == dialog.selected)
                .is_some_and(|record| record.revoked);
            let mut actions = div().h_flex().gap_2();
            actions = actions.child(
                Button::new("mcp-save-token")
                    .primary()
                    .compact()
                    .label(t!("mcp.save").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.save_mcp_token(cx))),
            );
            if has_selected {
                let rotate_label = if dialog.pending_rotate {
                    t!("mcp.confirm_rotate").to_string()
                } else {
                    t!("mcp.rotate").to_string()
                };
                actions = actions.child(
                    Button::new("mcp-rotate-token")
                        .ghost()
                        .compact()
                        .disabled(selected_revoked)
                        .label(rotate_label)
                        .on_click(cx.listener(|this, _, _, cx| this.rotate_mcp_token(cx))),
                );
                let revoke_label = if dialog.pending_revoke {
                    t!("mcp.confirm_revoke").to_string()
                } else {
                    t!("mcp.revoke").to_string()
                };
                actions = actions.child(
                    Button::new("mcp-revoke-token")
                        .ghost()
                        .compact()
                        .disabled(selected_revoked)
                        .label(revoke_label)
                        .on_click(cx.listener(|this, _, _, cx| this.revoke_mcp_token(cx))),
                );
                let delete_label = if dialog.pending_delete {
                    t!("mcp.confirm_delete").to_string()
                } else {
                    t!("mcp.delete").to_string()
                };
                actions = actions.child(
                    Button::new("mcp-delete-token")
                        .ghost()
                        .compact()
                        .label(delete_label)
                        .on_click(cx.listener(|this, _, _, cx| this.delete_mcp_token(cx))),
                );
            }
            editor.push(actions.into_any_element());
        }

        div()
            .id("mcp-token-editor")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .v_flex()
            .gap_2()
            .overflow_y_scroll()
            .children(editor)
            .into_any_element()
    }

    pub(super) fn render_mcp_audit_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(muted)
                        .child(t!("mcp.audit_hint").to_string()),
                )
                .child(
                    Button::new("mcp-audit-refresh")
                        .ghost()
                        .compact()
                        .label(t!("mcp.audit_refresh").to_string())
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_mcp_audit(cx))),
                )
                .into_any_element(),
        );
        if let Some(error) = dialog.audit_error.clone() {
            rows.push(
                div()
                    .text_xs()
                    .text_color(danger)
                    .child(error)
                    .into_any_element(),
            );
        }
        if dialog.audit.is_empty() && dialog.audit_error.is_none() {
            rows.push(empty_hint(t!("mcp.audit_empty").to_string(), muted));
        }
        for entry in &dialog.audit {
            let when = age_label(entry.time);
            let result_color = if entry.result == "ok" { muted } else { danger };
            let target = entry.path.clone().or_else(|| {
                entry
                    .query
                    .as_deref()
                    .map(|query| t!("mcp.audit_query", query = query).to_string())
            });
            let location = match (&entry.notebook_name, target) {
                (Some(notebook), Some(target)) => format!("{notebook} · {target}"),
                (Some(notebook), None) => notebook.clone(),
                (None, Some(target)) => target,
                (None, None) => String::new(),
            };
            rows.push(
                div()
                    .v_flex()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(muted).child(when))
                            .child(div().text_xs().font_bold().child(entry.tool.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(entry.token_name.clone()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(result_color)
                                    .child(entry.result.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .child(format!("{location} · {} ms", entry.duration_ms)),
                    )
                    .into_any_element(),
            );
        }
        div()
            .id("mcp-audit-body")
            .v_flex()
            .gap_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }

    pub(super) fn open_mcp_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let port = self.config.mcp_port.to_string();
        let enabled = self.config.mcp_http;
        let port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(port)
                .placeholder("11026")
        });
        let name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("mcp.name_placeholder").to_string())
        });
        let allow_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("mcp.allow_placeholder").to_string())
        });
        let deny_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("mcp.deny_placeholder").to_string())
        });
        self.mcp_settings = Some(McpSettingsDialog {
            tab: McpTab::Server,
            enabled,
            port_input,
            tokens: Vec::new(),
            selected: None,
            creating: false,
            name_input,
            allow_input,
            deny_input,
            scopes: Vec::new(),
            notebooks: HashSet::new(),
            secret: None,
            pending_rotate: false,
            pending_revoke: false,
            pending_delete: false,
            audit: Vec::new(),
            audit_error: None,
            message: None,
            error: None,
        });
        self.refresh_mcp_state(cx);
        cx.notify();
    }

    /// Loads tokens and the audit trail on the background executor and fills
    /// the already-rendered MCP dialog when the read completes.
    pub(super) fn refresh_mcp_state(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (tokens, audit) = cx
                .background_executor()
                .spawn(async move { (token_store().list(), load_audit_entries()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.mcp_settings.as_mut() {
                    match tokens {
                        Ok(list) => Self::set_mcp_tokens(dialog, list),
                        Err(err) => {
                            dialog.error =
                                Some(t!("mcp.token_error", message = err.to_string()).to_string());
                        }
                    }
                    dialog.audit = audit.0;
                    dialog.audit_error = audit.1;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn set_mcp_tab(&mut self, tab: McpTab, cx: &mut Context<Self>) {
        if let Some(dialog) = self.mcp_settings.as_mut() {
            if dialog.tab == tab {
                return;
            }
            dialog.tab = tab;
            dialog.message = None;
            dialog.error = None;
        }
        cx.notify();
    }

    pub(super) fn set_mcp_tokens(dialog: &mut McpSettingsDialog, tokens: Vec<TokenRecord>) {
        dialog.tokens = tokens;
        if let Some(id) = dialog.selected {
            if !dialog.tokens.iter().any(|record| record.id == id) {
                dialog.selected = None;
                dialog.creating = false;
                dialog.secret = None;
            }
        }
        dialog.error = None;
    }

    pub(super) fn select_mcp_token(
        &mut self,
        id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.mcp_settings.as_mut() else {
            return;
        };
        let Some(record) = dialog.tokens.iter().find(|record| record.id == id).cloned() else {
            return;
        };
        dialog.selected = Some(id);
        dialog.creating = false;
        dialog.secret = None;
        dialog.pending_rotate = false;
        dialog.pending_revoke = false;
        dialog.pending_delete = false;
        dialog.message = None;
        dialog.error = None;
        dialog.scopes = record.scopes.clone();
        dialog.notebooks = record.notebooks.iter().copied().collect();
        let allow = record.note_allow.join(", ");
        let deny = record.note_deny.join(", ");
        dialog.name_input.update(cx, |state, cx| {
            state.set_value(record.name.clone(), window, cx)
        });
        dialog
            .allow_input
            .update(cx, |state, cx| state.set_value(allow, window, cx));
        dialog
            .deny_input
            .update(cx, |state, cx| state.set_value(deny, window, cx));
        cx.notify();
    }

    pub(super) fn begin_new_mcp_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active_notebook;
        let Some(dialog) = self.mcp_settings.as_mut() else {
            return;
        };
        dialog.selected = None;
        dialog.creating = true;
        dialog.secret = None;
        dialog.pending_rotate = false;
        dialog.pending_revoke = false;
        dialog.pending_delete = false;
        dialog.message = None;
        dialog.error = None;
        dialog.scopes = vec![Scope::Read, Scope::Search];
        dialog.notebooks = active.into_iter().collect();
        for input in [&dialog.name_input, &dialog.allow_input, &dialog.deny_input] {
            input.update(cx, |state, cx| state.set_value(String::new(), window, cx));
        }
        cx.notify();
    }

    pub(super) fn save_mcp_token(&mut self, cx: &mut Context<Self>) {
        let Some((name, allow, deny, scopes, notebooks, creating, selected)) =
            self.mcp_settings.as_ref().map(|dialog| {
                (
                    dialog.name_input.read(cx).value().trim().to_string(),
                    split_globs(&dialog.allow_input.read(cx).value()),
                    split_globs(&dialog.deny_input.read(cx).value()),
                    Scope::ALL
                        .into_iter()
                        .filter(|scope| dialog.scopes.contains(scope))
                        .collect::<Vec<_>>(),
                    dialog.notebooks.iter().copied().collect::<Vec<_>>(),
                    dialog.creating,
                    dialog.selected,
                )
            })
        else {
            return;
        };

        let validation = if name.is_empty() {
            Some(t!("mcp.err.name").to_string())
        } else if scopes.is_empty() {
            Some(t!("mcp.err.scopes").to_string())
        } else if notebooks.is_empty() {
            Some(t!("mcp.err.notebooks").to_string())
        } else {
            None
        };
        if let Some(message) = validation {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.error = Some(message);
                dialog.message = None;
            }
            cx.notify();
            return;
        }

        let receiver = disk::submit(
            move || -> Result<(String, Option<String>, Option<Uuid>, Vec<TokenRecord>), String> {
                let store = token_store();
                let mut secret = None;
                let mut created_id = None;
                let message = if creating {
                    let (record, wire) = store
                        .create(&name, scopes, notebooks, allow, deny)
                        .map_err(|err| {
                            t!("mcp.token_error", message = err.to_string()).to_string()
                        })?;
                    created_id = Some(record.id);
                    secret = Some(wire);
                    t!("mcp.created").to_string()
                } else if let Some(id) = selected {
                    let list = store.list().map_err(|err| {
                        t!("mcp.token_error", message = err.to_string()).to_string()
                    })?;
                    let mut record = list
                        .into_iter()
                        .find(|record| record.id == id)
                        .ok_or_else(|| t!("mcp.err.select").to_string())?;
                    record.name = name;
                    record.scopes = scopes;
                    record.notebooks = notebooks;
                    record.note_allow = allow;
                    record.note_deny = deny;
                    store.update(record).map_err(|err| {
                        t!("mcp.token_error", message = err.to_string()).to_string()
                    })?;
                    t!("mcp.saved").to_string()
                } else {
                    return Err(t!("mcp.err.select").to_string());
                };
                let list = store
                    .list()
                    .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
                Ok((message, secret, created_id, list))
            },
        );

        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.error = None;
        }
        cx.spawn(async move |this, cx| {
            let outcome = match receiver.await {
                Ok(outcome) => outcome,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.mcp_settings.as_mut() {
                    match outcome {
                        Ok((message, secret, created_id, list)) => {
                            Self::set_mcp_tokens(dialog, list);
                            if let Some(id) = created_id {
                                dialog.selected = Some(id);
                                dialog.creating = false;
                            }
                            if let Some(secret) = secret {
                                dialog.secret = Some(secret);
                            }
                            dialog.pending_rotate = false;
                            dialog.pending_revoke = false;
                            dialog.pending_delete = false;
                            dialog.message = Some(message);
                        }
                        Err(error) => {
                            dialog.error = Some(error);
                            dialog.message = None;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn rotate_mcp_token(&mut self, cx: &mut Context<Self>) {
        let Some((id, armed)) = self
            .mcp_settings
            .as_ref()
            .and_then(|dialog| dialog.selected.map(|id| (id, dialog.pending_rotate)))
        else {
            return;
        };
        if !armed {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.pending_rotate = true;
                dialog.message = None;
                dialog.error = None;
            }
            cx.notify();
            return;
        }
        let receiver = disk::submit(move || -> Result<(String, Vec<TokenRecord>), String> {
            let store = token_store();
            let list = store
                .list()
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
            let mut record = list
                .into_iter()
                .find(|record| record.id == id)
                .ok_or_else(|| t!("mcp.err.select").to_string())?;
            let secret = perms::new_secret();
            record.hash = perms::hash_secret(&secret);
            let wire = perms::format_token(&record.id, &secret);
            store
                .update(record)
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
            let list = store
                .list()
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
            Ok((wire, list))
        });
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.error = None;
        }
        cx.spawn(async move |this, cx| {
            let outcome = match receiver.await {
                Ok(outcome) => outcome,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.mcp_settings.as_mut() {
                    dialog.pending_rotate = false;
                    match outcome {
                        Ok((wire, list)) => {
                            Self::set_mcp_tokens(dialog, list);
                            dialog.secret = Some(wire);
                            dialog.message = Some(t!("mcp.rotated").to_string());
                            dialog.error = None;
                        }
                        Err(error) => {
                            dialog.error = Some(error);
                            dialog.message = None;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn revoke_mcp_token(&mut self, cx: &mut Context<Self>) {
        let Some((id, armed)) = self
            .mcp_settings
            .as_ref()
            .and_then(|dialog| dialog.selected.map(|id| (id, dialog.pending_revoke)))
        else {
            return;
        };
        if !armed {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.pending_revoke = true;
                dialog.message = None;
                dialog.error = None;
            }
            cx.notify();
            return;
        }
        let receiver = disk::submit(move || -> Result<Vec<TokenRecord>, String> {
            let store = token_store();
            store
                .revoke(&id)
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
            store
                .list()
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())
        });
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.error = None;
        }
        cx.spawn(async move |this, cx| {
            let outcome = match receiver.await {
                Ok(outcome) => outcome,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.mcp_settings.as_mut() {
                    dialog.pending_revoke = false;
                    match outcome {
                        Ok(list) => {
                            Self::set_mcp_tokens(dialog, list);
                            dialog.secret = None;
                            dialog.message = Some(t!("mcp.revoked").to_string());
                            dialog.error = None;
                        }
                        Err(error) => {
                            dialog.error = Some(error);
                            dialog.message = None;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn delete_mcp_token(&mut self, cx: &mut Context<Self>) {
        let Some((id, armed)) = self
            .mcp_settings
            .as_ref()
            .and_then(|dialog| dialog.selected.map(|id| (id, dialog.pending_delete)))
        else {
            return;
        };
        if !armed {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.pending_delete = true;
                dialog.message = None;
                dialog.error = None;
            }
            cx.notify();
            return;
        }
        let receiver = disk::submit(move || -> Result<Vec<TokenRecord>, String> {
            let store = token_store();
            store
                .delete(&id)
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
            store
                .list()
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())
        });
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.error = None;
        }
        cx.spawn(async move |this, cx| {
            let outcome = match receiver.await {
                Ok(outcome) => outcome,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.mcp_settings.as_mut() {
                    dialog.pending_delete = false;
                    match outcome {
                        Ok(list) => {
                            dialog.selected = None;
                            dialog.creating = false;
                            dialog.secret = None;
                            Self::set_mcp_tokens(dialog, list);
                            dialog.message = Some(t!("mcp.deleted").to_string());
                            dialog.error = None;
                        }
                        Err(error) => {
                            dialog.error = Some(error);
                            dialog.message = None;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn toggle_mcp_scope(&mut self, scope: Scope, enabled: bool, cx: &mut Context<Self>) {
        if let Some(dialog) = self.mcp_settings.as_mut() {
            if enabled {
                if !dialog.scopes.contains(&scope) {
                    dialog.scopes.push(scope);
                }
            } else {
                dialog.scopes.retain(|item| *item != scope);
            }
            dialog.error = None;
        }
        cx.notify();
    }

    pub(super) fn toggle_mcp_notebook(
        &mut self,
        notebook_id: Uuid,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.mcp_settings.as_mut() {
            if enabled {
                dialog.notebooks.insert(notebook_id);
            } else {
                dialog.notebooks.remove(&notebook_id);
            }
            dialog.error = None;
        }
        cx.notify();
    }

    pub(super) fn apply_mcp_server(&mut self, cx: &mut Context<Self>) {
        let Some((raw_port, enabled)) = self.mcp_settings.as_ref().map(|dialog| {
            (
                dialog.port_input.read(cx).value().trim().to_string(),
                dialog.enabled,
            )
        }) else {
            return;
        };
        let port = raw_port.parse::<u16>().ok().filter(|port| *port > 0);
        let Some(port) = port else {
            if let Some(dialog) = self.mcp_settings.as_mut() {
                dialog.error = Some(t!("mcp.err.port").to_string());
                dialog.message = None;
            }
            cx.notify();
            return;
        };

        self.config.mcp_port = port;
        self.config.mcp_http = enabled;
        self.save_config(cx);
        let mut message = None;
        self.mcp.stop();
        self.mcp_live = false;
        if enabled {
            if self.mcp.start(port) {
                self.mcp_live = true;
                message = Some(t!("mcp.started", port = port).to_string());
            }
        } else {
            message = Some(t!("mcp.stopped").to_string());
        }
        let error = if enabled && !self.mcp_live {
            self.mcp.error.clone()
        } else {
            None
        };
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.error = error;
            dialog.message = message;
        }
        cx.notify();
    }

    pub(super) fn refresh_mcp_audit(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (audit, error) = cx
                .background_executor()
                .spawn(async move { load_audit_entries() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.mcp_settings.as_mut() {
                    dialog.audit = audit;
                    dialog.audit_error = error;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_mcp_text(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.message = Some(t!("mcp.copied").to_string());
            dialog.error = None;
        }
        cx.notify();
    }
}
