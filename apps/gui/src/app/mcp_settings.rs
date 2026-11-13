use super::util::*;
use super::*;
use crate::official_docs::OfficialDocKind;
use gpui_kit::component::switch::Switch;

/// 令牌写入线程的返回值：提示消息、新令牌明文、新建 id、最新令牌列表。
type TokenWriteOutcome = Result<(String, Option<String>, Option<Uuid>, Vec<TokenRecord>), String>;

/// 权限行下拉按钮的展示值：联动模式下父节点展示子项聚合状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermDisplay {
    Level(PermLevel),
    Mixed,
    AllHidden,
}

/// 权限树的输入：一个笔记本根节点 + 其扁平条目表。
struct PermSource<'a> {
    id: Uuid,
    label: String,
    entries: &'a [Entry],
}

/// 由扁平条目重建的真实树节点。父节点索引一定小于子节点，因此可以逆序
/// 一次扫描算出整棵树的子项聚合，避免每行都重新遍历全部笔记（O(n²)）。
struct PermNode {
    key: NodeKey,
    label: String,
    depth: usize,
    is_folder: bool,
    children: Vec<usize>,
}

/// 子树「存储等级」的聚合累加器。
#[derive(Clone, Copy)]
struct PermAgg {
    has: bool,
    first: PermLevel,
    uniform: bool,
}

impl PermAgg {
    fn new() -> Self {
        Self {
            has: false,
            first: PermLevel::Hidden,
            uniform: true,
        }
    }

    fn push(&mut self, level: PermLevel) {
        if !self.has {
            self.has = true;
            self.first = level;
        } else if level != self.first {
            self.uniform = false;
        }
    }

    fn merge(&mut self, other: PermAgg) {
        if !other.has {
            return;
        }
        self.push(other.first);
        if !other.uniform {
            self.uniform = false;
        }
    }

    fn aggregate(&self) -> Option<ChildAggregate> {
        if !self.has {
            return None;
        }
        Some(if !self.uniform {
            ChildAggregate::Mixed
        } else if self.first == PermLevel::Hidden {
            ChildAggregate::AllHidden
        } else {
            ChildAggregate::Uniform(self.first)
        })
    }
}

/// 权限页虚拟列表行。只生成展开后可见的节点。
struct PermRow {
    key: NodeKey,
    label: String,
    depth: usize,
    is_folder: bool,
    has_children: bool,
    expanded: bool,
    display: PermDisplay,
    /// 自身存储等级高于有效等级，说明被某个上级节点限制。
    capped: bool,
}

/// 把各笔记本的扁平条目还原成树；`roots` 是笔记本根节点索引。
fn build_perm_nodes(sources: &[PermSource<'_>]) -> (Vec<PermNode>, Vec<usize>) {
    let mut nodes = Vec::new();
    let mut roots = Vec::new();
    for source in sources {
        let root = nodes.len();
        nodes.push(PermNode {
            key: NodeKey::notebook(source.id),
            label: source.label.clone(),
            depth: 0,
            is_folder: true,
            children: Vec::new(),
        });
        roots.push(root);
        // rel_path -> 节点索引；空串代表笔记本根。
        let mut index: HashMap<&str, usize> = HashMap::new();
        index.insert("", root);
        for entry in source.entries {
            let parent = index
                .get(parent_rel(&entry.rel_path))
                .copied()
                .unwrap_or(root);
            let node = nodes.len();
            index.insert(entry.rel_path.as_str(), node);
            nodes.push(PermNode {
                key: NodeKey::entry(source.id, &entry.rel_path),
                label: entry.name.clone(),
                depth: entry.rel_path.split('/').count(),
                is_folder: entry.kind == EntryKind::Folder,
                children: Vec::new(),
            });
            nodes[parent].children.push(node);
        }
    }
    (nodes, roots)
}

/// 后序一次扫描算出每个节点的「全部后代」聚合状态。
fn perm_aggregates(nodes: &[PermNode], tree: &PermissionTree) -> Vec<PermAgg> {
    let mut aggs = vec![PermAgg::new(); nodes.len()];
    for index in (0..nodes.len()).rev() {
        let mut agg = PermAgg::new();
        for &child in &nodes[index].children {
            agg.push(tree.level(&nodes[child].key));
            agg.merge(aggs[child]);
        }
        aggs[index] = agg;
    }
    aggs
}

/// 按展开状态生成可见行；未展开的文件夹其子树完全不进虚拟列表。
fn perm_rows(
    nodes: &[PermNode],
    roots: &[usize],
    aggs: &[PermAgg],
    tree: &PermissionTree,
    expanded: &HashSet<String>,
) -> Vec<PermRow> {
    let mut rows = Vec::new();
    let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
    while let Some(index) = stack.pop() {
        let node = &nodes[index];
        let aggregate = aggs[index].aggregate();
        let is_open = expanded.contains(&node.key.to_string());
        rows.push(PermRow {
            key: node.key.clone(),
            label: node.label.clone(),
            depth: node.depth,
            is_folder: node.is_folder,
            has_children: !node.children.is_empty(),
            expanded: is_open,
            display: perm_display(tree, &node.key, aggregate),
            capped: perm_capped(tree, &node.key, aggregate),
        });
        if is_open {
            for &child in node.children.iter().rev() {
                stack.push(child);
            }
        }
    }
    rows
}

/// 笔记本 entries 中 key 的全部后代（不含自身）；其他笔记本的 key 结果为空。
fn perm_descendants(notebook_id: Uuid, entries: &[Entry], key: &NodeKey) -> Vec<NodeKey> {
    entries
        .iter()
        .map(|entry| NodeKey::entry(notebook_id, &entry.rel_path))
        .filter(|candidate| key.is_ancestor_of(candidate))
        .collect()
}

/// 权限行的展示值：联动且有后代时看子项聚合，否则看自身有效等级。
///
/// 聚合等级同样要受祖先链限制：父节点被限制为只读时，即使子项存的是
/// 「删除」，也不能误导用户以为 Agent 真的能删。
fn perm_display(
    tree: &PermissionTree,
    key: &NodeKey,
    aggregate: Option<ChildAggregate>,
) -> PermDisplay {
    let effective = tree.effective(key);
    if tree.mode == GrantMode::Linked
        && let Some(aggregate) = aggregate
    {
        return match aggregate {
            ChildAggregate::AllHidden => PermDisplay::AllHidden,
            ChildAggregate::Uniform(level) => PermDisplay::Level(level.min(effective)),
            ChildAggregate::Mixed => PermDisplay::Mixed,
        };
    }
    PermDisplay::Level(effective)
}

/// 节点自身设置高于实际生效等级，需要在行内提示「受上级限制」。
fn perm_capped(tree: &PermissionTree, key: &NodeKey, aggregate: Option<ChildAggregate>) -> bool {
    let raw = if tree.mode == GrantMode::Linked {
        match aggregate {
            Some(ChildAggregate::Uniform(level)) => level,
            _ => tree.level(key),
        }
    } else {
        tree.level(key)
    };
    raw > tree.effective(key)
}

fn perm_level_label(level: PermLevel) -> String {
    let key = match level {
        PermLevel::Hidden => "mcp.perm.level.hidden",
        PermLevel::Read => "mcp.perm.level.read",
        PermLevel::Modify => "mcp.perm.level.modify",
        PermLevel::Delete => "mcp.perm.level.delete",
    };
    t!(key).to_string()
}

fn perm_display_label(display: PermDisplay) -> String {
    match display {
        PermDisplay::Level(level) => perm_level_label(level),
        PermDisplay::Mixed => t!("mcp.perm.mixed").to_string(),
        PermDisplay::AllHidden => t!("mcp.perm.all_hidden").to_string(),
    }
}

fn perm_level_icon(level: PermLevel) -> IconName {
    match level {
        PermLevel::Hidden => IconName::EyeOff,
        PermLevel::Read => IconName::Eye,
        PermLevel::Modify => IconName::Pencil,
        PermLevel::Delete => IconName::Trash,
    }
}

/// 带标题的设置分区卡片，让四个 Tab 的内容都有明确的视觉层次。
fn mcp_card(
    title: String,
    subtitle: Option<String>,
    body: AnyElement,
    muted: gpui_kit::Hsla,
    border: gpui_kit::Hsla,
) -> AnyElement {
    div()
        .v_flex()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .child(
            div()
                .v_flex()
                .gap_0p5()
                .child(div().text_sm().font_bold().child(title))
                .when_some(subtitle, |header, text| {
                    header.child(div().text_xs().text_color(muted).child(text))
                }),
        )
        .child(body)
        .into_any_element()
}

/// 分段控件的单个选项：选中态是「浮起的白块」，非选中态是浅色文字。
fn mcp_segment<F>(
    id: &'static str,
    label: String,
    active: bool,
    on_click: F,
    muted: gpui_kit::Hsla,
    foreground: gpui_kit::Hsla,
    popover: gpui_kit::Hsla,
    hover: gpui_kit::Hsla,
    transparent: gpui_kit::Hsla,
) -> AnyElement
where
    F: Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
{
    div()
        .id(id)
        .px_3()
        .py_1p5()
        .rounded_md()
        .text_xs()
        .cursor_pointer()
        .bg(if active { popover } else { transparent })
        .text_color(if active { foreground } else { muted })
        .when(active, |item| item.font_bold())
        .hover(move |style| style.bg(hover))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

/// 分段控件容器。
fn mcp_segment_track(
    items: Vec<AnyElement>,
    track: gpui_kit::Hsla,
    border: gpui_kit::Hsla,
) -> AnyElement {
    div()
        .h_flex()
        .items_center()
        .gap_0p5()
        .p_0p5()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .bg(track)
        .children(items)
        .into_any_element()
}

impl AppShell {
    fn mcp_tab_item(&self, tab: McpTab, key: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let active = self
            .mcp_settings
            .as_ref()
            .is_some_and(|dialog| dialog.tab == tab);
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let popover = cx.theme().popover;
        let hover = cx.theme().list_hover;
        let transparent = cx.theme().transparent;
        let id = match tab {
            McpTab::Server => "mcp-tab-server",
            McpTab::Tokens => "mcp-tab-tokens",
            McpTab::Permissions => "mcp-tab-permissions",
            McpTab::Audit => "mcp-tab-audit",
        };
        mcp_segment(
            id,
            t!(key).to_string(),
            active,
            cx.listener(move |this, _, _, cx| this.set_mcp_tab(tab, cx)),
            muted,
            foreground,
            popover,
            hover,
            transparent,
        )
    }

    fn mcp_snippet_item(
        &self,
        kind: SnippetKind,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .mcp_settings
            .as_ref()
            .is_some_and(|dialog| dialog.snippet == kind);
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let popover = cx.theme().popover;
        let hover = cx.theme().list_hover;
        let transparent = cx.theme().transparent;
        let id = match kind {
            SnippetKind::Http => "mcp-snippet-http",
            SnippetKind::Stdio => "mcp-snippet-stdio",
        };
        mcp_segment(
            id,
            t!(key).to_string(),
            active,
            cx.listener(move |this, _, _, cx| this.set_mcp_snippet(kind, cx)),
            muted,
            foreground,
            popover,
            hover,
            transparent,
        )
    }

    fn mcp_mode_item(
        &self,
        value: GrantMode,
        key: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .mcp_settings
            .as_ref()
            .is_some_and(|dialog| dialog.perms.mode == value);
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let popover = cx.theme().popover;
        let hover = cx.theme().list_hover;
        let transparent = cx.theme().transparent;
        let id = match value {
            GrantMode::Single => "mcp-perm-mode-single",
            GrantMode::Linked => "mcp-perm-mode-linked",
        };
        mcp_segment(
            id,
            t!(key).to_string(),
            active,
            cx.listener(move |this, _, _, cx| this.set_mcp_perm_mode(value, cx)),
            muted,
            foreground,
            popover,
            hover,
            transparent,
        )
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
        let border = cx.theme().border;
        let track = cx.theme().muted;

        let body = match tab {
            McpTab::Server => self.render_mcp_server_tab(cx),
            McpTab::Tokens => self.render_mcp_tokens_tab(cx),
            McpTab::Permissions => self.render_mcp_permissions_tab(cx),
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

        let tabs = mcp_segment_track(
            vec![
                self.mcp_tab_item(McpTab::Server, "mcp.tab.server", cx),
                self.mcp_tab_item(McpTab::Tokens, "mcp.tab.tokens", cx),
                self.mcp_tab_item(McpTab::Permissions, "mcp.tab.permissions", cx),
                self.mcp_tab_item(McpTab::Audit, "mcp.tab.audit", cx),
            ],
            track,
            border,
        );
        // 点击后先关掉设置面板，再以只读预览打开随包发布的 mcp.md。
        let usage_doc = Button::new("mcp-usage-doc")
            .ghost()
            .compact()
            .icon(IconName::BookOpen)
            .label(t!("mcp.usage_doc").to_string())
            .tooltip(t!("mcp.usage_doc_hint").to_string())
            .on_click(cx.listener(|this, _, window, cx| {
                this.close_settings(window, cx);
                this.open_official_doc(OfficialDocKind::Mcp, window, cx);
            }));
        let tabs_row = div()
            .h_flex()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w_0().child(tabs))
            .child(usage_doc)
            .into_any_element();

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap_3()
            .child(tabs_row)
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
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;
        let success = cx.theme().success;
        let track = cx.theme().muted;
        let snippet_kind = dialog.snippet;

        let status_color = if running { success } else { muted };
        let status_label = if running {
            t!("mcp.status.running_short").to_string()
        } else {
            t!("mcp.status.stopped_short").to_string()
        };
        let url_text = t!("mcp.running", port = self.mcp.port.max(1)).to_string();

        let status_row = div()
            .h_flex()
            .items_center()
            .gap_2()
            .child(Switch::new("mcp-enabled").checked(dialog.enabled).on_click(
                cx.listener(|this, checked: &bool, _, cx| this.toggle_mcp_enabled(*checked, cx)),
            ))
            .child(div().flex_1().text_sm().child(t!("mcp.enable").to_string()))
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_full()
                    .bg(status_color.opacity(0.12))
                    .child(div().size(px(6.)).rounded_full().bg(status_color))
                    .child(div().text_xs().text_color(status_color).child(status_label)),
            )
            .into_any_element();

        let port_row = div()
            .h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(52.))
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
                    .label(t!("mcp.apply_port").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.apply_mcp_server(cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_xs()
                    .text_color(if running { foreground } else { muted })
                    .child(url_text),
            )
            .into_any_element();

        let token_row = div()
            .h_flex()
            .items_center()
            .gap_2()
            .child(
                Switch::new("mcp-require-token")
                    .checked(dialog.require_token)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.toggle_mcp_require_token(*checked, cx)
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .child(t!("mcp.require_token").to_string()),
            )
            .into_any_element();

        let mut server_rows: Vec<AnyElement> = vec![status_row, port_row, token_row];
        server_rows.push(
            div()
                .text_xs()
                .text_color(muted)
                .child(t!("mcp.enable_hint").to_string())
                .into_any_element(),
        );
        server_rows.push(
            div()
                .text_xs()
                .text_color(muted)
                .child(t!("mcp.port_hint").to_string())
                .into_any_element(),
        );
        server_rows.push(
            div()
                .text_xs()
                .text_color(muted)
                .child(t!("mcp.require_token_hint").to_string())
                .into_any_element(),
        );
        if !running && let Some(error) = mcp_error {
            server_rows.push(
                div()
                    .text_xs()
                    .text_color(danger)
                    .child(error)
                    .into_any_element(),
            );
        }
        let server_card = mcp_card(
            t!("mcp.section.server").to_string(),
            None,
            div()
                .v_flex()
                .gap_2()
                .children(server_rows)
                .into_any_element(),
            muted,
            border,
        );

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
        let url = format!("http://127.0.0.1:{port}/mcp");
        let require_token = dialog.require_token;
        let snippet_hint = match (snippet_kind, require_token) {
            (SnippetKind::Http, true) => t!("mcp.snippet_hint_http", url = url.clone()).to_string(),
            (SnippetKind::Http, false) => {
                t!("mcp.snippet_hint_http_no_token", url = url.clone()).to_string()
            }
            (SnippetKind::Stdio, true) => t!("mcp.snippet_hint_stdio").to_string(),
            (SnippetKind::Stdio, false) => t!("mcp.snippet_hint_stdio_no_token").to_string(),
        };
        let snippet = match (snippet_kind, require_token) {
            (SnippetKind::Http, true) => serde_json::json!({
                "mcpServers": {
                    "md-work": {
                        "type": "http",
                        "url": url.clone(),
                        "headers": { "Authorization": format!("Bearer {sample_token}") }
                    }
                }
            }),
            (SnippetKind::Http, false) => serde_json::json!({
                "mcpServers": {
                    "md-work": {
                        "type": "http",
                        "url": url.clone()
                    }
                }
            }),
            (SnippetKind::Stdio, true) => serde_json::json!({
                "mcpServers": {
                    "md-work": {
                        "command": cli.clone(),
                        "args": ["mcp", "serve", "--stdio"],
                        "env": { "MDWORK_TOKEN": sample_token }
                    }
                }
            }),
            (SnippetKind::Stdio, false) => serde_json::json!({
                "mcpServers": {
                    "md-work": {
                        "command": cli.clone(),
                        "args": ["mcp", "serve", "--stdio"]
                    }
                }
            }),
        };
        let snippet = serde_json::to_string_pretty(&snippet).unwrap_or_default();
        let snippet_copy = snippet.clone();
        let mut code_lines = div().v_flex();
        for line in snippet.lines() {
            code_lines = code_lines.child(
                div()
                    .font_family("Consolas")
                    .text_xs()
                    .child(line.to_string()),
            );
        }
        let client_body = div()
            .v_flex()
            .gap_2()
            .child(mcp_segment_track(
                vec![
                    self.mcp_snippet_item(SnippetKind::Http, "mcp.snippet.http", cx),
                    self.mcp_snippet_item(SnippetKind::Stdio, "mcp.snippet.stdio", cx),
                ],
                track,
                border,
            ))
            .child(
                div()
                    .id("mcp-snippet-code")
                    .max_h(px(150.))
                    .overflow_y_scroll()
                    .p_2()
                    .rounded_md()
                    .bg(track.opacity(0.5))
                    .child(code_lines),
            )
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("mcp-copy-snippet")
                            .ghost()
                            .compact()
                            .icon(IconName::Copy)
                            .label(t!("mcp.copy").to_string())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.copy_mcp_text(snippet_copy.clone(), cx)
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(muted)
                            .child(snippet_hint),
                    ),
            )
            .into_any_element();
        let client_card = mcp_card(
            t!("mcp.section.clients").to_string(),
            None,
            client_body,
            muted,
            border,
        );

        div()
            .id("mcp-server-body")
            .v_flex()
            .gap_3()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(server_card)
            .child(client_card)
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
        let border = cx.theme().border;
        let accent = cx.theme().accent;
        let danger = cx.theme().danger;
        let success = cx.theme().success;
        let mut rows: Vec<AnyElement> = Vec::new();
        if dialog.tokens.is_empty() {
            rows.push(empty_hint(t!("mcp.no_tokens").to_string(), muted));
        }
        for record in &dialog.tokens {
            let id = record.id;
            let active = dialog.selected == Some(id);
            let (status, status_color) = if record.revoked {
                (t!("mcp.status.revoked").to_string(), danger)
            } else if !record.is_active(perms::now_unix()) {
                (t!("mcp.status.expired").to_string(), danger)
            } else {
                (t!("mcp.status.active").to_string(), success)
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
                    .gap_1()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(if active { accent } else { border })
                    .bg(if active { active_bg } else { transparent })
                    .cursor_pointer()
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
                            .gap_1p5()
                            .child(div().size(px(6.)).rounded_full().bg(status_color))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .child(record.name.clone()),
                            )
                            .child(div().text_xs().text_color(status_color).child(status)),
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
            .w(px(244.))
            .min_h_0()
            .flex_shrink_0()
            .v_flex()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .child(
                div()
                    .text_sm()
                    .font_bold()
                    .child(t!("mcp.token.list_title").to_string()),
            )
            .child(
                Button::new("mcp-new-token")
                    .primary()
                    .compact()
                    .w_full()
                    .icon(IconName::Plus)
                    .label(t!("mcp.create").to_string())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.begin_new_mcp_token(window, cx)),
                    ),
            )
            .child(
                div()
                    .id("mcp-token-list-rows")
                    .v_flex()
                    .gap_1()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .into_any_element()
    }

    pub(super) fn render_mcp_token_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;
        let warning = cx.theme().warning;
        let editing = dialog.creating || dialog.selected.is_some();
        let mut editor: Vec<AnyElement> = Vec::new();
        editor.push(
            div()
                .text_sm()
                .font_bold()
                .child(t!("mcp.token.editor_title").to_string())
                .into_any_element(),
        );
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
            let admin = dialog.scopes.contains(&Scope::Admin);
            let mut scope_row = div().h_flex().flex_wrap().gap_2();
            for scope in Scope::ALL {
                let checked = if scope == Scope::Admin {
                    admin
                } else {
                    !admin && dialog.scopes.contains(&scope)
                };
                scope_row = scope_row.child(
                    div().w(px(132.)).child(
                        Checkbox::new(format!("mcp-scope-{}", scope.as_str()))
                            .label(scope_label(scope))
                            .checked(checked)
                            .disabled(admin && scope != Scope::Admin)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.toggle_mcp_scope(scope, *checked, cx)
                            })),
                    ),
                );
            }
            editor.push(mcp_field(
                t!("mcp.scopes").to_string(),
                scope_row.into_any_element(),
                muted,
            ));
            if admin {
                editor.push(
                    div()
                        .text_xs()
                        .text_color(warning)
                        .child(t!("mcp.scope.admin_hint").to_string())
                        .into_any_element(),
                );
            }

            let known: HashSet<Uuid> = self
                .notebooks
                .iter()
                .map(|entry| entry.notebook.marker.id)
                .collect();
            let mut notebook_rows = div().h_flex().flex_wrap().gap_2();
            for entry in &self.notebooks {
                let id = entry.notebook.marker.id;
                let checked = dialog.notebooks.contains(&id);
                notebook_rows = notebook_rows.child(
                    div().w(px(160.)).child(
                        Checkbox::new(format!("mcp-notebook-{id}"))
                            .label(entry.notebook.display_name().to_string())
                            .checked(checked)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.toggle_mcp_notebook(id, *checked, cx)
                            })),
                    ),
                );
            }
            for id in dialog.notebooks.iter().filter(|id| !known.contains(id)) {
                let id = *id;
                notebook_rows = notebook_rows.child(
                    div().w(px(160.)).child(
                        Checkbox::new(format!("mcp-notebook-{id}"))
                            .label(t!("mcp.unknown_notebook", id = id.to_string()).to_string())
                            .checked(true)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                this.toggle_mcp_notebook(id, *checked, cx)
                            })),
                    ),
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
                        .border_color(danger)
                        .bg(danger.opacity(0.06))
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
                                        .icon(IconName::Copy)
                                        .label(t!("mcp.copy").to_string())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.copy_mcp_text(copy_value.clone(), cx)
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .font_family("Consolas")
                                .text_xs()
                                .w_full()
                                .overflow_hidden()
                                .child(secret),
                        )
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
            let mut actions = div().h_flex().items_center().gap_2();
            actions = actions.child(
                Button::new("mcp-save-token")
                    .primary()
                    .compact()
                    .label(t!("mcp.save").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.save_mcp_token(cx))),
            );
            actions = actions.child(div().flex_1());
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
                        .text_color(danger)
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
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .overflow_y_scroll()
            .children(editor)
            .into_any_element()
    }

    pub(super) fn render_mcp_permissions_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let danger = cx.theme().danger;
        let border = cx.theme().border;
        let hover = cx.theme().list_hover;
        let track = cx.theme().muted;
        let info = cx.theme().info;
        let success = cx.theme().success;
        let warning = cx.theme().warning;
        let tampered = dialog.perms_tampered;

        let sources: Vec<PermSource<'_>> = self
            .notebooks
            .iter()
            .map(|entry| PermSource {
                id: entry.notebook.marker.id,
                label: entry.notebook.display_name().to_string(),
                entries: &entry.tree,
            })
            .collect();
        let (nodes, roots) = build_perm_nodes(&sources);
        let aggs = perm_aggregates(&nodes, &dialog.perms);
        let rows = perm_rows(&nodes, &roots, &aggs, &dialog.perms, &dialog.perms_expanded);
        let visible_count = rows.len();

        let toolbar = div()
            .h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("mcp.perm.mode").to_string()),
            )
            .child(mcp_segment_track(
                vec![
                    self.mcp_mode_item(GrantMode::Single, "mcp.perm.mode.single", cx),
                    self.mcp_mode_item(GrantMode::Linked, "mcp.perm.mode.linked", cx),
                ],
                track,
                border,
            ))
            .child(div().flex_1())
            .child(
                Button::new("mcp-perm-collapse-all")
                    .ghost()
                    .compact()
                    .label(t!("mcp.perm.collapse_all").to_string())
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_mcp_perm_expanded_all(false, cx)),
                    ),
            )
            .child(
                Button::new("mcp-perm-expand-all")
                    .ghost()
                    .compact()
                    .label(t!("mcp.perm.expand_all").to_string())
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_mcp_perm_expanded_all(true, cx)),
                    ),
            )
            .into_any_element();

        let header = div()
            .v_flex()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .child(toolbar)
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("mcp.perm.mode_hint").to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t!("mcp.perm.node_count", count = visible_count).to_string()),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("mcp.perm.tree_hint").to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t!("mcp.perm.hint").to_string()),
            )
            .when(tampered, |header| {
                header.child(
                    div()
                        .text_xs()
                        .text_color(danger)
                        .child(t!("mcp.perm.tampered").to_string()),
                )
            })
            .into_any_element();

        let body: AnyElement = if rows.is_empty() {
            empty_hint(t!("mcp.perm.empty").to_string(), muted)
        } else {
            let shell = cx.entity();
            let count = rows.len();
            let rows = Arc::new(rows);
            uniform_list("mcp-perm-tree", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let row = &rows[index];
                        let shell = shell.clone();
                        let key = row.key.clone();
                        let toggle_key = row.key.to_string();
                        let depth = row.depth as f32;
                        let icon = if row.is_folder {
                            IconName::Folder
                        } else {
                            IconName::FileText
                        };
                        let color = if row.is_folder { foreground } else { muted };
                        let (pill_text, pill_color) = match row.display {
                            PermDisplay::Level(PermLevel::Hidden) => {
                                (perm_display_label(row.display), muted)
                            }
                            PermDisplay::Level(PermLevel::Read) => {
                                (perm_display_label(row.display), info)
                            }
                            PermDisplay::Level(PermLevel::Modify) => {
                                (perm_display_label(row.display), success)
                            }
                            PermDisplay::Level(PermLevel::Delete) => {
                                (perm_display_label(row.display), danger)
                            }
                            PermDisplay::Mixed => (perm_display_label(row.display), warning),
                            PermDisplay::AllHidden => (perm_display_label(row.display), muted),
                        };
                        let chevron = if row.has_children {
                            if row.expanded { "▾" } else { "▸" }
                        } else {
                            ""
                        };
                        let toggle_shell = shell.clone();
                        let current_level = match row.display {
                            PermDisplay::Level(level) => Some(level),
                            _ => None,
                        };
                        div()
                            .id(format!("mcp-perm-row-{index}"))
                            .h(px(29.))
                            .w_full()
                            .h_flex()
                            .items_center()
                            .gap_1()
                            .pl(px(6.0 + depth * 14.0))
                            .pr_2()
                            .rounded_sm()
                            .hover(move |style| style.bg(hover))
                            .child(
                                div()
                                    .id(format!("mcp-perm-toggle-{index}"))
                                    .w(px(14.))
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .when(row.has_children, |toggle| toggle.cursor_pointer())
                                    .child(chevron)
                                    .on_click(move |_, _, cx| {
                                        if !chevron.is_empty() {
                                            let key = toggle_key.clone();
                                            let _ = toggle_shell.update(cx, |this, cx| {
                                                this.toggle_mcp_perm_expand(key, cx);
                                            });
                                        }
                                    }),
                            )
                            .child(Icon::new(icon).xsmall().text_color(muted))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(color)
                                    .child(row.label.clone()),
                            )
                            .when(row.capped, |element| {
                                element.child(
                                    div()
                                        .text_xs()
                                        .text_color(warning)
                                        .child(t!("mcp.perm.capped").to_string()),
                                )
                            })
                            .child(div().size(px(6.)).rounded_full().bg(pill_color))
                            .child(
                                Button::new(format!("mcp-perm-{index}"))
                                    .ghost()
                                    .compact()
                                    .label(pill_text)
                                    .dropdown_menu(move |menu, _, _| {
                                        let mut menu = menu;
                                        for level in [
                                            PermLevel::Hidden,
                                            PermLevel::Read,
                                            PermLevel::Modify,
                                            PermLevel::Delete,
                                        ] {
                                            let shell = shell.clone();
                                            let key = key.clone();
                                            menu = menu.item(
                                                PopupMenuItem::new(perm_level_label(level))
                                                    .icon(Icon::new(perm_level_icon(level)))
                                                    .checked(current_level == Some(level))
                                                    .on_click(move |_, _, cx| {
                                                        let _ = shell.update(cx, |this, cx| {
                                                            this.set_mcp_perm_level(
                                                                key.clone(),
                                                                level,
                                                                cx,
                                                            )
                                                        });
                                                    }),
                                            );
                                        }
                                        menu
                                    }),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .panel_scrollbar(&dialog.perms_scroll)
        };

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap_2()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub(super) fn set_mcp_snippet(&mut self, kind: SnippetKind, cx: &mut Context<Self>) {
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.snippet = kind;
        }
        cx.notify();
    }

    /// 开关即时启停：以前勾了开关还要点「应用」才生效，切换标签页就丢改动。
    pub(super) fn toggle_mcp_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.config.mcp_http = enabled;
        let port = self.config.mcp_port.max(1);
        let require_token = self.config.mcp_require_token;
        let workspace = self.current_workspace_root();
        self.save_config(cx);
        self.mcp.stop();
        self.mcp_live = false;
        let mut message = None;
        let mut error = None;
        if enabled {
            if self.mcp.start(port, workspace.as_deref(), require_token) {
                self.mcp_live = true;
                message = Some(t!("mcp.started", port = port).to_string());
            } else {
                error = self.mcp.error.clone();
            }
        } else {
            message = Some(t!("mcp.stopped").to_string());
        }
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.enabled = enabled;
            dialog.message = message;
            dialog.error = error;
        }
        cx.notify();
    }

    /// 令牌策略切换即时生效：重启 HTTP 子进程，已连接的客户端会断开。
    /// stdio 由客户端自行拉起 CLI，其策略来自配置文件或环境变量。
    pub(super) fn toggle_mcp_require_token(&mut self, required: bool, cx: &mut Context<Self>) {
        self.config.mcp_require_token = required;
        self.save_config(cx);
        let mut message = None;
        let mut error = None;
        if self.config.mcp_http {
            let port = self.config.mcp_port.max(1);
            let workspace = self.current_workspace_root();
            self.mcp.stop();
            self.mcp_live = false;
            if self.mcp.start(port, workspace.as_deref(), required) {
                self.mcp_live = true;
                message = Some(t!("mcp.started", port = port).to_string());
            } else {
                error = self.mcp.error.clone();
            }
        }
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.require_token = required;
            dialog.message = message;
            dialog.error = error;
        }
        cx.notify();
    }

    pub(super) fn set_mcp_perm_mode(&mut self, mode: GrantMode, cx: &mut Context<Self>) {
        let Some(dialog) = self.mcp_settings.as_mut() else {
            return;
        };
        if dialog.perms.mode == mode {
            return;
        }
        dialog.perms.mode = mode;
        self.save_mcp_perms(cx);
    }

    pub(super) fn toggle_mcp_perm_expand(&mut self, key: String, cx: &mut Context<Self>) {
        if let Some(dialog) = self.mcp_settings.as_mut() {
            if !dialog.perms_expanded.remove(&key) {
                dialog.perms_expanded.insert(key);
            }
        }
        cx.notify();
    }

    pub(super) fn set_mcp_perm_expanded_all(&mut self, expand: bool, cx: &mut Context<Self>) {
        let keys: HashSet<String> = {
            let sources: Vec<PermSource<'_>> = self
                .notebooks
                .iter()
                .map(|entry| PermSource {
                    id: entry.notebook.marker.id,
                    label: entry.notebook.display_name().to_string(),
                    entries: &entry.tree,
                })
                .collect();
            let (nodes, _roots) = build_perm_nodes(&sources);
            if expand {
                nodes
                    .iter()
                    .filter(|node| !node.children.is_empty())
                    .map(|node| node.key.to_string())
                    .collect()
            } else {
                // 连笔记本根也收起：笔记特别多时一键回到只剩几个笔记本的视图。
                HashSet::new()
            }
        };
        if let Some(dialog) = self.mcp_settings.as_mut() {
            dialog.perms_expanded = keys;
        }
        cx.notify();
    }

    pub(super) fn set_mcp_perm_level(
        &mut self,
        key: NodeKey,
        level: PermLevel,
        cx: &mut Context<Self>,
    ) {
        let descendants = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == key.notebook)
            .map(|entry| perm_descendants(key.notebook, &entry.tree, &key))
            .unwrap_or_default();
        let Some(dialog) = self.mcp_settings.as_mut() else {
            return;
        };
        dialog.perms.set_level(&key, level, &descendants);
        self.save_mcp_perms(cx);
    }

    /// 权限变更在写线程落盘并重建签名，避免每次点击都在 UI 线程做
    /// DPAPI 解密 + HMAC + 整棵树的 TOML 序列化（笔记多时明显卡顿）。
    fn save_mcp_perms(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.mcp_settings.as_mut() else {
            return;
        };
        let tree = dialog.perms.clone();
        dialog.error = None;
        let receiver = disk::submit(move || {
            PermissionStore::new(PermissionStore::default_path())
                .save(&tree)
                .map_err(|err| err.to_string())
        });
        cx.spawn(async move |this, cx| {
            let outcome = match receiver.await {
                Ok(outcome) => outcome,
                Err(_) => return,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.mcp_settings.as_mut() {
                    match outcome {
                        Ok(()) => {
                            dialog.perms_tampered = false;
                            dialog.message = Some(t!("mcp.perm.saved").to_string());
                            dialog.error = None;
                        }
                        Err(message) => {
                            dialog.error =
                                Some(t!("mcp.perm.save_error", message = message).to_string());
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

    pub(super) fn render_mcp_audit_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.mcp_settings.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let danger = cx.theme().danger;
        let success = cx.theme().success;
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_bold()
                        .child(t!("mcp.audit.title").to_string()),
                )
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
                        .icon(IconName::RefreshCw)
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
            let result_color = if entry.result == "ok" {
                success
            } else {
                danger
            };
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
                    .py_1p5()
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
                                    .truncate()
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
                            .text_color(muted)
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
            .child(div().flex_1().min_h_0().v_flex().gap_1().children(rows))
            .into_any_element()
    }

    pub(super) fn open_mcp_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let port = self.config.mcp_port.to_string();
        let enabled = self.config.mcp_http;
        let require_token = self.config.mcp_require_token;
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
        let loaded = PermissionStore::new(PermissionStore::default_path()).load();
        // 默认只展开笔记本根，深层文件夹收起，避免笔记多时一屏刷不完。
        let perms_expanded: HashSet<String> = self
            .notebooks
            .iter()
            .map(|entry| NodeKey::notebook(entry.notebook.marker.id).to_string())
            .collect();
        self.mcp_settings = Some(McpSettingsDialog {
            tab: McpTab::Server,
            enabled,
            require_token,
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
            perms: loaded.tree,
            perms_tampered: loaded.tampered,
            perms_expanded,
            perms_scroll: UniformListScrollHandle::new(),
            snippet: SnippetKind::Http,
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
            // 进入权限页时重新验签加载，及时发现外部篡改。
            if tab == McpTab::Permissions {
                let loaded = PermissionStore::new(PermissionStore::default_path()).load();
                dialog.perms = loaded.tree;
                dialog.perms_tampered = loaded.tampered;
            }
        }
        cx.notify();
    }

    pub(super) fn set_mcp_tokens(dialog: &mut McpSettingsDialog, tokens: Vec<TokenRecord>) {
        dialog.tokens = tokens;
        if let Some(id) = dialog.selected
            && !dialog.tokens.iter().any(|record| record.id == id)
        {
            dialog.selected = None;
            dialog.creating = false;
            dialog.secret = None;
        }
        // 不在这里清空 error：后台刷新会覆盖上一次失败操作的提示。
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

        let receiver = disk::submit(move || -> TokenWriteOutcome {
            let store = token_store();
            let mut secret = None;
            let mut created_id = None;
            let message = if creating {
                let (record, wire) = store
                    .create(&name, scopes, notebooks, allow, deny)
                    .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
                created_id = Some(record.id);
                secret = Some(wire);
                t!("mcp.created").to_string()
            } else if let Some(id) = selected {
                let list = store
                    .list()
                    .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
                let mut record = list
                    .into_iter()
                    .find(|record| record.id == id)
                    .ok_or_else(|| t!("mcp.err.select").to_string())?;
                record.name = name;
                record.scopes = scopes;
                record.notebooks = notebooks;
                record.note_allow = allow;
                record.note_deny = deny;
                store
                    .update(record)
                    .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
                t!("mcp.saved").to_string()
            } else {
                return Err(t!("mcp.err.select").to_string());
            };
            let list = store
                .list()
                .map_err(|err| t!("mcp.token_error", message = err.to_string()).to_string())?;
            Ok((message, secret, created_id, list))
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
            // Admin 已包含全部权限：不再保存一组互相冗余的 scope。
            if scope == Scope::Admin {
                dialog.scopes = if enabled {
                    vec![Scope::Admin]
                } else {
                    Vec::new()
                };
            } else if enabled {
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
        let Some((raw_port, enabled, require_token)) = self.mcp_settings.as_ref().map(|dialog| {
            (
                dialog.port_input.read(cx).value().trim().to_string(),
                dialog.enabled,
                dialog.require_token,
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
        self.config.mcp_require_token = require_token;
        self.save_config(cx);
        let workspace = self.current_workspace_root();
        let mut message = None;
        self.mcp.stop();
        self.mcp_live = false;
        if enabled {
            if self.mcp.start(port, workspace.as_deref(), require_token) {
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

#[cfg(test)]
mod tests {
    // 不能用 `use super::*`：GPUI 的通配导入会遮蔽内置的 `#[test]`。
    use super::{
        PermDisplay, PermSource, build_perm_nodes, perm_aggregates, perm_capped, perm_descendants,
        perm_display, perm_rows,
    };
    use mdwork_core::mcp_permissions::{
        ChildAggregate, GrantMode, NodeKey, PermLevel, PermissionTree,
    };
    use mdwork_core::notebook::{Entry, EntryKind};
    use std::collections::HashSet;
    use uuid::Uuid;

    fn id() -> Uuid {
        Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    fn entry(rel_path: &str, kind: EntryKind) -> Entry {
        Entry {
            rel_path: rel_path.to_string(),
            name: rel_path.rsplit('/').next().unwrap().to_string(),
            kind,
            size: 0,
            created: 0,
            modified: 0,
        }
    }

    fn entries() -> Vec<Entry> {
        vec![
            entry("工作", EntryKind::Folder),
            entry("工作/设计.md", EntryKind::Note),
            entry("工作/草稿", EntryKind::Folder),
            entry("工作/草稿/暂存.md", EntryKind::Note),
            entry("根.md", EntryKind::Note),
        ]
    }

    fn sources(items: &[Entry]) -> Vec<PermSource<'_>> {
        vec![PermSource {
            id: id(),
            label: "demo".to_string(),
            entries: items,
        }]
    }

    fn all_nodes() -> (Vec<super::PermNode>, Vec<usize>) {
        build_perm_nodes(&sources(&entries()))
    }

    #[test]
    fn descendants_of_notebook_root_cover_all_entries() {
        let found = perm_descendants(id(), &entries(), &NodeKey::notebook(id()));
        assert_eq!(found.len(), 5);
        assert!(found.contains(&NodeKey::entry(id(), "工作/草稿/暂存.md")));
    }

    #[test]
    fn descendants_of_folder_exclude_siblings_and_self() {
        let found = perm_descendants(id(), &entries(), &NodeKey::entry(id(), "工作"));
        let paths: Vec<_> = found.iter().filter_map(|key| key.path.as_deref()).collect();
        assert_eq!(
            paths,
            vec!["工作/设计.md", "工作/草稿", "工作/草稿/暂存.md"]
        );
    }

    #[test]
    fn descendants_of_note_or_other_notebook_are_empty() {
        assert!(perm_descendants(id(), &entries(), &NodeKey::entry(id(), "根.md")).is_empty());
        assert!(perm_descendants(id(), &entries(), &NodeKey::notebook(Uuid::new_v4())).is_empty());
    }

    #[test]
    fn nodes_rebuild_parent_child_links() {
        let (nodes, roots) = all_nodes();
        assert_eq!(roots.len(), 1);
        assert_eq!(nodes.len(), 6);
        let root = &nodes[roots[0]];
        assert_eq!(root.children.len(), 2);
        let folder = &nodes[root.children[0]];
        assert_eq!(folder.label, "工作");
        assert_eq!(folder.children.len(), 2);
        assert_eq!(nodes[folder.children[1]].depth, 2);
        assert!(nodes[folder.children[1]].is_folder);
    }

    #[test]
    fn only_expanded_nodes_become_visible_rows() {
        let items = entries();
        let sources = sources(&items);
        let (nodes, roots) = build_perm_nodes(&sources);
        let tree = PermissionTree {
            mode: GrantMode::Single,
            ..PermissionTree::default()
        };
        let aggs = perm_aggregates(&nodes, &tree);

        let rows = perm_rows(&nodes, &roots, &aggs, &tree, &HashSet::new());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "demo");

        let mut root_open = HashSet::new();
        root_open.insert(NodeKey::notebook(id()).to_string());
        let rows = perm_rows(&nodes, &roots, &aggs, &tree, &root_open);
        let labels: Vec<_> = rows.iter().map(|row| row.label.as_str()).collect();
        assert_eq!(labels, vec!["demo", "工作", "根.md"]);
        assert!(rows[1].has_children);
        assert!(!rows[2].has_children);

        let mut all = root_open.clone();
        for node in &nodes {
            if !node.children.is_empty() {
                all.insert(node.key.to_string());
            }
        }
        let rows = perm_rows(&nodes, &roots, &aggs, &tree, &all);
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[4].label, "暂存.md");
    }

    #[test]
    fn aggregates_cover_all_descendants_in_one_pass() {
        let items = entries();
        let sources = sources(&items);
        let (nodes, _roots) = build_perm_nodes(&sources);
        let mut tree = PermissionTree::default();
        let folder = NodeKey::entry(id(), "工作");
        let descendants = perm_descendants(id(), &items, &folder);
        tree.set_level(&folder, PermLevel::Read, &descendants);
        let folder_index = nodes.iter().position(|node| node.key == folder).unwrap();

        let aggs = perm_aggregates(&nodes, &tree);
        assert_eq!(
            aggs[folder_index].aggregate(),
            Some(ChildAggregate::Uniform(PermLevel::Read))
        );

        tree.set_level(
            &NodeKey::entry(id(), "工作/草稿/暂存.md"),
            PermLevel::Delete,
            &[],
        );
        let aggs = perm_aggregates(&nodes, &tree);
        assert_eq!(aggs[folder_index].aggregate(), Some(ChildAggregate::Mixed));
    }

    #[test]
    fn display_shows_effective_level_when_capped_by_ancestor() {
        let mut tree = PermissionTree {
            mode: GrantMode::Single,
            ..PermissionTree::default()
        };
        tree.set_level(&NodeKey::notebook(id()), PermLevel::Read, &[]);
        tree.set_level(&NodeKey::entry(id(), "工作"), PermLevel::Read, &[]);
        let note = NodeKey::entry(id(), "工作/设计.md");
        tree.set_level(&note, PermLevel::Delete, &[]);
        // 自身存的是「删除」，但上级只读，Agent 实际只能读。
        assert_eq!(
            perm_display(&tree, &note, None),
            PermDisplay::Level(PermLevel::Read)
        );
        assert!(perm_capped(&tree, &note, None));
    }

    #[test]
    fn missing_ancestor_grant_keeps_descendant_hidden() {
        let mut tree = PermissionTree {
            mode: GrantMode::Single,
            ..PermissionTree::default()
        };
        let note = NodeKey::entry(id(), "工作/设计.md");
        tree.set_level(&note, PermLevel::Delete, &[]);
        assert_eq!(
            perm_display(&tree, &note, None),
            PermDisplay::Level(PermLevel::Hidden)
        );
        assert!(perm_capped(&tree, &note, None));
    }

    #[test]
    fn display_in_linked_mode_aggregates_children() {
        let items = entries();
        let sources = sources(&items);
        let (nodes, _roots) = build_perm_nodes(&sources);
        let mut tree = PermissionTree::default();
        // 笔记本根先授权，否则整条祖先链都是隐藏，聚合等级会被正确压到隐藏。
        tree.set_level(&NodeKey::notebook(id()), PermLevel::Read, &[]);
        let folder = NodeKey::entry(id(), "工作");
        let folder_index = nodes.iter().position(|node| node.key == folder).unwrap();
        let descendants = perm_descendants(id(), &items, &folder);

        let aggs = perm_aggregates(&nodes, &tree);
        assert_eq!(
            perm_display(&tree, &folder, aggs[folder_index].aggregate()),
            PermDisplay::AllHidden
        );

        tree.set_level(&folder, PermLevel::Read, &descendants);
        let aggs = perm_aggregates(&nodes, &tree);
        assert_eq!(
            perm_display(&tree, &folder, aggs[folder_index].aggregate()),
            PermDisplay::Level(PermLevel::Read)
        );

        tree.set_level(
            &NodeKey::entry(id(), "工作/设计.md"),
            PermLevel::Delete,
            &[],
        );
        let aggs = perm_aggregates(&nodes, &tree);
        assert_eq!(
            perm_display(&tree, &folder, aggs[folder_index].aggregate()),
            PermDisplay::Mixed
        );
    }

    #[test]
    fn linked_aggregate_never_overstates_ancestor_limit() {
        let mut tree = PermissionTree::default();
        let folder = NodeKey::entry(id(), "工作");
        tree.set_level(&NodeKey::notebook(id()), PermLevel::Read, &[]);
        tree.set_level(&folder, PermLevel::Delete, &[]);
        assert_eq!(
            perm_display(
                &tree,
                &folder,
                Some(ChildAggregate::Uniform(PermLevel::Delete))
            ),
            PermDisplay::Level(PermLevel::Read)
        );
        assert!(perm_capped(
            &tree,
            &folder,
            Some(ChildAggregate::Uniform(PermLevel::Delete))
        ));
    }

    #[test]
    fn display_of_leaf_or_childless_folder_shows_own_level() {
        let mut tree = PermissionTree::default();
        tree.set_level(&NodeKey::notebook(id()), PermLevel::Modify, &[]);
        let note = NodeKey::entry(id(), "根.md");
        tree.set_level(&note, PermLevel::Modify, &[]);
        assert_eq!(
            perm_display(&tree, &note, None),
            PermDisplay::Level(PermLevel::Modify)
        );
        let empty_folder = NodeKey::entry(id(), "空夹");
        assert_eq!(
            perm_display(&tree, &empty_folder, None),
            PermDisplay::Level(PermLevel::Hidden)
        );
    }
}
