//! Markdown → preview intermediate representation.
//!
//! The GUI and the exporters both consume this tree instead of re-parsing
//! Markdown, so headings, tables and code blocks stay consistent everywhere.

use comrak::nodes::{AstNode, ListType, NodeList, NodeTable, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};

use crate::html_subset::{
    self, ElementAttrs, ImgTag, Length, LiftedHtml, StyleSheet, TextBoxTag, lift_html_fragment,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewIr {
    pub blocks: Vec<Block>,
    /// 文档内 `<style>` 汇总；预览/导出应用规则，不渲染该块本身。
    pub styles: StyleSheet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        content: Vec<Inline>,
        line: usize,
    },
    Paragraph {
        content: Vec<Inline>,
        line: usize,
    },
    CodeBlock {
        language: Option<String>,
        text: String,
        line: usize,
    },
    List {
        ordered: bool,
        start: usize,
        tight: bool,
        items: Vec<ListItem>,
        line: usize,
    },
    Quote {
        blocks: Vec<Block>,
        line: usize,
    },
    Table {
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
        aligns: Vec<Align>,
        line: usize,
    },
    Image {
        src: String,
        title: Option<String>,
        alt: Vec<Inline>,
        width: Option<Length>,
        height: Option<Length>,
        attrs: ElementAttrs,
        line: usize,
    },
    /// 受控 `<div>`：内容为纯文本。
    Div {
        text: String,
        attrs: ElementAttrs,
        line: usize,
    },
    Rule {
        line: usize,
    },
    /// 源码里比「块与块之间那一个分隔空行」多出来的空行。
    ///
    /// 一个空行只表示下一块开始，预览和导出用各自的段间距，不再另画一行。
    /// 从第二个空行起每一行记在这里：`lines == 1` 表示连续两个空行。
    /// 复制纯文本时仍丢掉这些空行，避免预览复制出多余空行。
    Blank {
        lines: u32,
        line: usize,
    },
    Math {
        tex: String,
        display: bool,
        line: usize,
    },
    FootnoteDefinition {
        label: String,
        blocks: Vec<Block>,
        line: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// `Some(true)` for a checked task, `Some(false)` for an unchecked one.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Code(String),
    Emphasis(Vec<Inline>),
    Strong(Vec<Inline>),
    Strike(Vec<Inline>),
    Link {
        url: String,
        title: Option<String>,
        children: Vec<Inline>,
    },
    Image {
        src: String,
        alt: Vec<Inline>,
        title: Option<String>,
        width: Option<Length>,
        height: Option<Length>,
        attrs: ElementAttrs,
    },
    /// 受控 `<span>`：内容为纯文本。
    Span {
        text: String,
        attrs: ElementAttrs,
    },
    SoftBreak,
    HardBreak,
    FootnoteRef(String),
    Math {
        tex: String,
        display: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    pub level: u8,
    pub title: String,
    /// One-based source line of the heading.
    pub line: usize,
}

impl PreviewIr {
    pub fn from_markdown(markdown: &str) -> Self {
        let arena = Arena::new();
        let root = parse_document(&arena, markdown, &options());
        let lines: Vec<&str> = markdown.lines().collect();
        let mut blocks = Vec::new();
        let mut styles = StyleSheet::default();
        collect_blocks(root, &lines, &mut blocks, &mut styles);
        let blocks = expand_softbroken_images(blocks);
        Self { blocks, styles }
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn outline(&self) -> Vec<OutlineItem> {
        let mut items = Vec::new();
        collect_outline(&self.blocks, &mut items);
        items
    }

    /// Plain-text projection used for copy-to-clipboard: one newline between
    /// blocks and no blank lines between wrapped lines.
    pub fn to_plain_text(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for block in &self.blocks {
            let text = block_plain_text(block);
            if !text.trim().is_empty() {
                parts.push(text);
            }
        }
        parts.join("\n")
    }
}

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.extension.math_dollars = true;
    options.extension.superscript = true;
    options
}

fn collect_blocks<'a>(
    node: &'a AstNode<'a>,
    lines: &[&str],
    out: &mut Vec<Block>,
    styles: &mut StyleSheet,
) {
    let mut produced: Vec<(usize, usize, Vec<Block>)> = Vec::new();
    for child in node.children() {
        let start = child.data().sourcepos.start.line;
        let end = child.data().sourcepos.end.line;
        let blocks = blocks_from_node(child, lines, styles);
        if !blocks.is_empty() {
            produced.push((start, end, blocks));
        }
    }
    for index in 0..produced.len() {
        if index > 0 {
            let prev_end = produced[index - 1].1;
            let next_start = produced[index].0;
            if let Some(blank) = blank_block_between(lines, prev_end, next_start) {
                out.push(blank);
            }
        }
        out.append(&mut produced[index].2);
    }
}

/// 两个可见块之间，源码若全是空行且至少两行，多出来的行数做成 [`Block::Blank`]。
fn blank_block_between(
    lines: &[&str],
    prev_end_line: usize,
    next_start_line: usize,
) -> Option<Block> {
    let extra = extra_blank_lines(lines, prev_end_line, next_start_line);
    (extra > 0).then_some(Block::Blank {
        lines: extra,
        line: prev_end_line.saturating_add(1),
    })
}

fn extra_blank_lines(lines: &[&str], prev_end_line: usize, next_start_line: usize) -> u32 {
    if prev_end_line == 0 || next_start_line <= prev_end_line.saturating_add(1) {
        return 0;
    }
    let start_idx = prev_end_line;
    let end_idx = next_start_line - 1;
    if end_idx > lines.len() || start_idx >= end_idx {
        return 0;
    }
    let between = &lines[start_idx..end_idx];
    if between.is_empty() || !between.iter().copied().all(is_structural_blank_line) {
        return 0;
    }
    u32::try_from(between.len())
        .unwrap_or(u32::MAX)
        .saturating_sub(1)
}

/// 真正的空行，或只剩块引用标记的行（`>`、`> >`）。
fn is_structural_blank_line(line: &str) -> bool {
    let mut rest = line.trim();
    if rest.is_empty() {
        return true;
    }
    while let Some(after) = rest.strip_prefix('>') {
        rest = after.trim_start();
        if rest.is_empty() {
            return true;
        }
    }
    false
}

fn collect_outline(blocks: &[Block], out: &mut Vec<OutlineItem>) {
    for block in blocks {
        match block {
            Block::Heading {
                level,
                content,
                line,
            } => out.push(OutlineItem {
                level: *level,
                title: inline_plain_text(content),
                line: *line,
            }),
            Block::Quote { blocks, .. } | Block::FootnoteDefinition { blocks, .. } => {
                collect_outline(blocks, out)
            }
            Block::List { items, .. } => {
                for item in items {
                    collect_outline(&item.blocks, out);
                }
            }
            _ => {}
        }
    }
}

fn blocks_from_node<'a>(
    node: &'a AstNode<'a>,
    lines: &[&str],
    styles: &mut StyleSheet,
) -> Vec<Block> {
    let line = node.data().sourcepos.start.line;
    match &node.data().value {
        NodeValue::Heading(heading) => vec![Block::Heading {
            level: heading.level,
            content: inlines(node),
            line,
        }],
        NodeValue::Paragraph => {
            let content = inlines(node);
            if let Some(images) = image_only_blocks(&content, line) {
                return images;
            }
            if let Some(math) = standalone_math(&content, line) {
                return vec![math];
            }
            vec![Block::Paragraph { content, line }]
        }
        NodeValue::CodeBlock(code) => {
            let language = code
                .info
                .split_whitespace()
                .next()
                .filter(|info| !info.is_empty())
                .map(str::to_string);
            vec![Block::CodeBlock {
                language,
                text: code.literal.clone(),
                line,
            }]
        }
        NodeValue::List(list) => vec![block_list(node, list, line, lines, styles)],
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
            let mut blocks = Vec::new();
            collect_blocks(node, lines, &mut blocks, styles);
            vec![Block::Quote { blocks, line }]
        }
        NodeValue::ThematicBreak => vec![Block::Rule { line }],
        NodeValue::Table(table) => vec![block_table(node, table, line)],
        NodeValue::Math(math) => vec![Block::Math {
            tex: math.literal.clone(),
            display: math.display_math,
            line,
        }],
        NodeValue::FootnoteDefinition(footnote) => {
            let mut blocks = Vec::new();
            collect_blocks(node, lines, &mut blocks, styles);
            vec![Block::FootnoteDefinition {
                label: footnote.name.clone(),
                blocks,
                line,
            }]
        }
        NodeValue::HtmlBlock(html) => match lift_html_fragment(&html.literal) {
            LiftedHtml::Style(sheet) => {
                styles.extend(sheet);
                Vec::new()
            }
            LiftedHtml::Img(img) => vec![block_from_img(img, line)],
            LiftedHtml::Div(box_) => vec![block_from_div(box_, line)],
            LiftedHtml::Span(box_) => vec![Block::Paragraph {
                content: vec![inline_from_span(box_)],
                line,
            }],
            LiftedHtml::Ignored => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn block_from_img(img: ImgTag, line: usize) -> Block {
    let alt = if img.alt.is_empty() {
        Vec::new()
    } else {
        vec![Inline::Text(img.alt)]
    };
    Block::Image {
        src: img.src,
        title: img.title,
        alt,
        width: img.width,
        height: img.height,
        attrs: img.attrs,
        line,
    }
}

fn block_from_div(box_: TextBoxTag, line: usize) -> Block {
    Block::Div {
        text: box_.text,
        attrs: box_.attrs,
        line,
    }
}

fn inline_from_span(box_: TextBoxTag) -> Inline {
    Inline::Span {
        text: box_.text,
        attrs: box_.attrs,
    }
}

fn inline_from_img_tag(img: ImgTag) -> Inline {
    let alt = if img.alt.is_empty() {
        Vec::new()
    } else {
        vec![Inline::Text(img.alt)]
    };
    Inline::Image {
        src: img.src,
        alt,
        title: img.title,
        width: img.width,
        height: img.height,
        attrs: img.attrs,
    }
}

/// 软换行分隔的图片行提升为独立 `Block::Image`，避免预览把多图塞进同一 flex 段落。
fn expand_softbroken_images(blocks: Vec<Block>) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            Block::Paragraph { content, line }
                if content.iter().any(|i| matches!(i, Inline::Image { .. })) =>
            {
                out.extend(split_paragraph_images(content, line));
            }
            other => out.push(other),
        }
    }
    out
}

fn split_paragraph_images(content: Vec<Inline>, line: usize) -> Vec<Block> {
    let mut out = Vec::new();
    let mut line_inlines: Vec<Inline> = Vec::new();
    let flush = |line_inlines: &mut Vec<Inline>, out: &mut Vec<Block>, line: usize| {
        if line_inlines.is_empty() {
            return;
        }
        let only_ws = line_inlines
            .iter()
            .all(|inline| matches!(inline, Inline::Text(text) if text.trim().is_empty()));
        if only_ws {
            line_inlines.clear();
            return;
        }
        if let [
            Inline::Image {
                src,
                alt,
                title,
                width,
                height,
                attrs,
            },
        ] = line_inlines.as_slice()
        {
            out.push(Block::Image {
                src: src.clone(),
                title: title.clone(),
                alt: alt.clone(),
                width: *width,
                height: *height,
                attrs: attrs.clone(),
                line,
            });
        } else {
            out.push(Block::Paragraph {
                content: std::mem::take(line_inlines),
                line,
            });
            return;
        }
        line_inlines.clear();
    };
    for inline in content {
        match inline {
            Inline::SoftBreak | Inline::HardBreak => flush(&mut line_inlines, &mut out, line),
            other => line_inlines.push(other),
        }
    }
    flush(&mut line_inlines, &mut out, line);
    out
}

/// CommonMark keeps adjacent image lines in one paragraph (soft breaks). Promote
/// an image-only paragraph into one block per image so preview/export treat them
/// like standalone figures.
fn image_only_blocks(content: &[Inline], line: usize) -> Option<Vec<Block>> {
    let mut images = Vec::new();
    for inline in content {
        match inline {
            Inline::Image {
                src,
                alt,
                title,
                width,
                height,
                attrs,
            } => {
                images.push(Block::Image {
                    src: src.clone(),
                    title: title.clone(),
                    alt: alt.clone(),
                    width: *width,
                    height: *height,
                    attrs: attrs.clone(),
                    line,
                });
            }
            Inline::Text(text) if text.trim().is_empty() => {}
            Inline::SoftBreak | Inline::HardBreak => {}
            _ => return None,
        }
    }
    if images.is_empty() {
        None
    } else {
        Some(images)
    }
}

fn standalone_math(content: &[Inline], line: usize) -> Option<Block> {
    match content {
        [Inline::Math { tex, display: true }] => Some(Block::Math {
            tex: tex.clone(),
            display: true,
            line,
        }),
        _ => None,
    }
}

fn block_list<'a>(
    node: &'a AstNode<'a>,
    list: &NodeList,
    line: usize,
    lines: &[&str],
    styles: &mut StyleSheet,
) -> Block {
    let mut items = Vec::new();
    let mut prev_end: Option<usize> = None;
    for item in node.children() {
        let task = match &item.data().value {
            NodeValue::TaskItem(marker) => Some(marker.symbol.is_some()),
            NodeValue::Item(_) => None,
            _ => continue,
        };
        let start_line = item.data().sourcepos.start.line;
        let end_line = item.data().sourcepos.end.line;
        let mut blocks = item_blocks(item, lines, styles);
        if let Some(prev_end) = prev_end
            && let Some(blank) = blank_block_between(lines, prev_end, start_line)
        {
            blocks.insert(0, blank);
        }
        prev_end = Some(end_line);
        items.push(ListItem { task, blocks });
    }
    Block::List {
        ordered: matches!(list.list_type, ListType::Ordered),
        start: list.start,
        tight: list.tight,
        items,
        line,
    }
}

fn item_blocks<'a>(item: &'a AstNode<'a>, lines: &[&str], styles: &mut StyleSheet) -> Vec<Block> {
    let item_line = item.data().sourcepos.start.line;
    let mut blocks = Vec::new();
    let mut pending: Vec<Inline> = Vec::new();
    let mut prev_end: Option<usize> = None;
    for child in item.children() {
        let start = child.data().sourcepos.start.line;
        let end = child.data().sourcepos.end.line;
        let child_blocks = blocks_from_node(child, lines, styles);
        if !child_blocks.is_empty() {
            if !pending.is_empty() {
                blocks.push(Block::Paragraph {
                    content: std::mem::take(&mut pending),
                    line: item_line,
                });
            }
            if let Some(prev_end) = prev_end
                && let Some(blank) = blank_block_between(lines, prev_end, start)
            {
                blocks.push(blank);
            }
            blocks.extend(child_blocks);
            prev_end = Some(end);
        } else if let Some(inline) = inline_from_node(child) {
            pending.push(inline);
        }
    }
    if !pending.is_empty() {
        // 列表项里也可能夹杂拆开的 HTML 行内标签
        let coalesced = coalesce_inline_list(pending);
        blocks.push(Block::Paragraph {
            content: coalesced,
            line: item_line,
        });
    }
    blocks
}

fn block_table<'a>(node: &'a AstNode<'a>, table: &NodeTable, line: usize) -> Block {
    let mut head = Vec::new();
    let mut rows = Vec::new();
    for row in node.children() {
        let NodeValue::TableRow(is_header) = row.data().value else {
            continue;
        };
        let cells: Vec<Vec<Inline>> = row.children().map(inlines).collect();
        if is_header {
            head = cells;
        } else {
            rows.push(cells);
        }
    }
    let aligns = table
        .alignments
        .iter()
        .map(|align| match align {
            TableAlignment::Left => Align::Left,
            TableAlignment::Center => Align::Center,
            TableAlignment::Right => Align::Right,
            TableAlignment::None => Align::None,
        })
        .collect();
    Block::Table {
        head,
        rows,
        aligns,
        line,
    }
}

fn inlines<'a>(node: &'a AstNode<'a>) -> Vec<Inline> {
    let children: Vec<_> = node.children().collect();
    coalesce_html_inlines(&children)
}

fn coalesce_html_inlines<'a>(nodes: &[&'a AstNode<'a>]) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < nodes.len() {
        let node = nodes[index];
        match &node.data().value {
            NodeValue::HtmlInline(html) => {
                if let Some((name, attrs_map, self_closing)) = html_subset::parse_open_tag(html) {
                    if name == "img" {
                        let img = img_tag_from_attr_map(attrs_map);
                        out.push(inline_from_img_tag(img));
                        index += 1;
                        continue;
                    }
                    if (name == "span" || name == "div") && !self_closing {
                        let attrs = element_attrs_from_open_map(attrs_map);
                        let mut text = String::new();
                        index += 1;
                        while index < nodes.len() {
                            match &nodes[index].data().value {
                                NodeValue::HtmlInline(close)
                                    if html_subset::is_close_tag(close, &name) =>
                                {
                                    index += 1;
                                    break;
                                }
                                NodeValue::HtmlInline(other) => {
                                    text.push_str(&html_subset::plain_text_from_htmlish(other));
                                    index += 1;
                                }
                                NodeValue::Text(chunk) => {
                                    text.push_str(chunk);
                                    index += 1;
                                }
                                NodeValue::SoftBreak => {
                                    text.push(' ');
                                    index += 1;
                                }
                                NodeValue::LineBreak => {
                                    text.push('\n');
                                    index += 1;
                                }
                                _ => {
                                    if let Some(inline) = inline_from_node(nodes[index]) {
                                        text.push_str(&inline_plain_text(&[inline]));
                                    }
                                    index += 1;
                                }
                            }
                        }
                        out.push(Inline::Span { text, attrs });
                        continue;
                    }
                }
                match lift_html_fragment(html) {
                    LiftedHtml::Img(img) => out.push(inline_from_img_tag(img)),
                    LiftedHtml::Span(box_) | LiftedHtml::Div(box_) => {
                        out.push(inline_from_span(box_));
                    }
                    LiftedHtml::Style(_) | LiftedHtml::Ignored => {}
                }
                index += 1;
            }
            _ => {
                if let Some(inline) = inline_from_node(node) {
                    out.push(inline);
                }
                index += 1;
            }
        }
    }
    out
}

fn coalesce_inline_list(pending: Vec<Inline>) -> Vec<Inline> {
    // 列表项路径已经拆过节点；此处仅作占位，保持顺序。
    pending
}

fn img_tag_from_attr_map(attrs: std::collections::BTreeMap<String, String>) -> ImgTag {
    let width = attrs
        .get("width")
        .and_then(|v| html_subset::parse_length(v));
    let height = attrs
        .get("height")
        .and_then(|v| html_subset::parse_length(v));
    let mut element = element_attrs_from_open_map(attrs.clone());
    element.style.width = None;
    element.style.height = None;
    ImgTag {
        src: attrs.get("src").cloned().unwrap_or_default(),
        alt: attrs.get("alt").cloned().unwrap_or_default(),
        title: attrs.get("title").cloned().filter(|s| !s.is_empty()),
        width,
        height,
        attrs: element,
    }
}

fn element_attrs_from_open_map(attrs: std::collections::BTreeMap<String, String>) -> ElementAttrs {
    let class = attrs
        .get("class")
        .map(|value| {
            value
                .split_whitespace()
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let style = attrs
        .get("style")
        .map(|value| html_subset::parse_style_decls(value))
        .unwrap_or_default();
    ElementAttrs {
        id: attrs.get("id").cloned().filter(|s| !s.is_empty()),
        name: attrs.get("name").cloned().filter(|s| !s.is_empty()),
        class,
        title: attrs.get("title").cloned().filter(|s| !s.is_empty()),
        style,
    }
}

fn inline_from_node<'a>(node: &'a AstNode<'a>) -> Option<Inline> {
    match &node.data().value {
        NodeValue::Text(text) => Some(Inline::Text(text.to_string())),
        NodeValue::Code(code) => Some(Inline::Code(code.literal.clone())),
        NodeValue::Emph => Some(Inline::Emphasis(inlines(node))),
        NodeValue::Strong => Some(Inline::Strong(inlines(node))),
        NodeValue::Strikethrough => Some(Inline::Strike(inlines(node))),
        NodeValue::Link(link) => Some(Inline::Link {
            url: link.url.clone(),
            title: title(&link.title),
            children: inlines(node),
        }),
        NodeValue::Image(link) => Some(Inline::Image {
            src: link.url.clone(),
            alt: inlines(node),
            title: title(&link.title),
            width: None,
            height: None,
            attrs: ElementAttrs::default(),
        }),
        NodeValue::SoftBreak => Some(Inline::SoftBreak),
        NodeValue::LineBreak => Some(Inline::HardBreak),
        NodeValue::FootnoteReference(reference) => {
            Some(Inline::FootnoteRef(reference.name.clone()))
        }
        NodeValue::Math(math) => Some(Inline::Math {
            tex: math.literal.clone(),
            display: math.display_math,
        }),
        NodeValue::HtmlInline(_) => None,
        NodeValue::Raw(raw) => Some(Inline::Text(raw.clone())),
        _ => None,
    }
}

fn title(title: &str) -> Option<String> {
    (!title.is_empty()).then(|| title.to_string())
}

fn block_plain_text(block: &Block) -> String {
    match block {
        Block::Heading { content, .. } | Block::Paragraph { content, .. } => {
            inline_plain_text(content)
        }
        Block::CodeBlock { language, text, .. } => {
            let fence = "```";
            let language = language.as_deref().unwrap_or("");
            let body = text.trim_end_matches('\n');
            format!("{fence}{language}\n{body}\n{fence}")
        }
        Block::List {
            ordered,
            start,
            items,
            ..
        } => {
            let mut lines = Vec::new();
            for (index, item) in items.iter().enumerate() {
                let marker = if *ordered {
                    format!("{}. ", start + index)
                } else {
                    "- ".to_string()
                };
                let padding = " ".repeat(marker.len());
                let body = item
                    .blocks
                    .iter()
                    .map(block_plain_text)
                    .filter(|text| !text.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                let body = match item.task {
                    Some(true) => format!("[x] {body}"),
                    Some(false) => format!("[ ] {body}"),
                    None => body,
                };
                let mut item_lines = body.split('\n');
                let first = item_lines.next().unwrap_or_default();
                lines.push(format!("{marker}{first}"));
                for line in item_lines {
                    if line.is_empty() {
                        lines.push(padding.clone());
                    } else {
                        lines.push(format!("{padding}{line}"));
                    }
                }
            }
            lines.join("\n")
        }
        Block::Quote { blocks, .. } => blocks
            .iter()
            .map(block_plain_text)
            .filter(|text| !text.trim().is_empty())
            .flat_map(|text| {
                text.split('\n')
                    .map(|line| {
                        if line.is_empty() {
                            ">".to_string()
                        } else {
                            format!("> {line}")
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Table { head, rows, .. } => {
            let mut lines = Vec::new();
            for row in std::iter::once(head).chain(rows.iter()) {
                let cells: Vec<String> = row.iter().map(|cell| inline_plain_text(cell)).collect();
                lines.push(cells.join("\t"));
            }
            lines.join("\n")
        }
        Block::Image { src, alt, .. } => {
            let alt = inline_plain_text(alt);
            if alt.is_empty() { src.clone() } else { alt }
        }
        Block::Div { text, .. } => text.clone(),
        Block::Rule { .. } => "---".to_string(),
        Block::Blank { .. } => String::new(),
        Block::Math { tex, display, .. } => {
            if *display {
                format!("$$\n{}\n$$", tex.trim())
            } else {
                format!("${tex}$")
            }
        }
        Block::FootnoteDefinition { label, blocks, .. } => {
            let body = blocks
                .iter()
                .map(block_plain_text)
                .filter(|text| !text.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            format!("[^{label}]: {body}")
        }
    }
}

fn inline_plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push_str(text),
            Inline::Code(code) => {
                out.push('`');
                out.push_str(code);
                out.push('`');
            }
            Inline::Emphasis(children) | Inline::Strong(children) | Inline::Strike(children) => {
                out.push_str(&inline_plain_text(children))
            }
            Inline::Link { url, children, .. } => {
                if children.is_empty() {
                    out.push_str(url);
                } else {
                    out.push_str(&inline_plain_text(children));
                }
            }
            Inline::Image { src, alt, .. } => {
                let alt = inline_plain_text(alt);
                if alt.is_empty() {
                    out.push_str(src);
                } else {
                    out.push_str(&alt);
                }
            }
            Inline::Span { text, .. } => out.push_str(text),
            Inline::SoftBreak => out.push(' '),
            Inline::HardBreak => out.push('\n'),
            Inline::FootnoteRef(name) => {
                out.push_str("[^");
                out.push_str(name);
                out.push(']');
            }
            Inline::Math { tex, display } => {
                if *display {
                    out.push_str("$$");
                    out.push_str(tex);
                    out.push_str("$$");
                } else {
                    out.push('$');
                    out.push_str(tex);
                    out.push('$');
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_subset::{Length, RgbColor};

    #[test]
    fn parses_headings_paragraphs_and_code() {
        let ir = PreviewIr::from_markdown("# 标题\n\n第一段。\n\n```rust\nfn main() {}\n```\n");
        assert_eq!(ir.blocks.len(), 3);
        assert!(matches!(
            &ir.blocks[0],
            Block::Heading { level: 1, content, .. } if content == &[Inline::Text("标题".into())]
        ));
        assert!(matches!(&ir.blocks[1], Block::Paragraph { content, .. } if content.len() == 1));
        assert!(matches!(
            &ir.blocks[2],
            Block::CodeBlock { language: Some(lang), text, .. }
                if lang == "rust" && text == "fn main() {}\n"
        ));
    }

    #[test]
    fn parses_nested_list_and_tasks() {
        let ir = PreviewIr::from_markdown("- 一\n  - 二\n- [x] 完成\n- [ ] 未完成\n");
        let Block::List { items, ordered, .. } = &ir.blocks[0] else {
            panic!("expected list, got {:?}", ir.blocks[0]);
        };
        assert!(!ordered);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].blocks.len(), 2);
        assert_eq!(items[1].task, Some(true));
        assert_eq!(items[2].task, Some(false));
    }

    #[test]
    fn parses_table_and_alignment() {
        let ir = PreviewIr::from_markdown("| a | b |\n|:--|--:|\n| 1 | 2 |\n");
        let Block::Table {
            head, rows, aligns, ..
        } = &ir.blocks[0]
        else {
            panic!("expected table, got {:?}", ir.blocks[0]);
        };
        assert_eq!(head.len(), 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(aligns, &[Align::Left, Align::Right]);
        assert_eq!(ir.to_plain_text(), "a\tb\n1\t2");
    }

    #[test]
    fn one_blank_line_is_only_a_block_break() {
        let ir = PreviewIr::from_markdown("# 标题\n\n一段文本。\n");
        assert_eq!(ir.blocks.len(), 2);
        assert!(
            !ir.blocks
                .iter()
                .any(|block| matches!(block, Block::Blank { .. }))
        );
    }

    #[test]
    fn extra_blank_lines_stay_between_blocks() {
        let ir = PreviewIr::from_markdown("一段文本。\n\n\n# 表格\n");
        assert!(matches!(ir.blocks[0], Block::Paragraph { .. }));
        assert!(matches!(ir.blocks[1], Block::Blank { lines: 1, .. }));
        assert!(matches!(ir.blocks[2], Block::Heading { level: 1, .. }));

        let ir = PreviewIr::from_markdown("甲\n\n\n\n乙\n");
        assert!(matches!(ir.blocks[1], Block::Blank { lines: 2, .. }));

        let ir = PreviewIr::from_markdown(
            "| 名称 | 单价 |\n| --- | --- |\n| 苹果 | 6元 |\n\n\n# 引用\n",
        );
        assert!(
            matches!(ir.blocks[0], Block::Table { .. }),
            "{:?}",
            ir.blocks[0]
        );
        assert!(matches!(ir.blocks[1], Block::Blank { lines: 1, .. }));
        assert!(matches!(ir.blocks[2], Block::Heading { .. }));
    }

    #[test]
    fn extra_blank_lines_between_list_items_are_kept() {
        let ir = PreviewIr::from_markdown("- 甲\n\n\n- 乙\n");
        match &ir.blocks[0] {
            Block::List { items, .. } => {
                assert_eq!(items.len(), 2, "{:?}", items);
                assert!(
                    matches!(items[1].blocks.first(), Some(Block::Blank { lines: 1, .. })),
                    "{:?}",
                    items[1].blocks
                );
            }
            Block::Blank { .. } => panic!("blank should sit inside the list, got {:?}", ir.blocks),
            other => panic!("expected a list, got {other:?}"),
        }
        let tight = PreviewIr::from_markdown("- 甲\n\n- 乙\n");
        let Block::List { items, .. } = &tight.blocks[0] else {
            panic!("expected a loose list, got {:?}", tight.blocks);
        };
        assert!(
            items.iter().all(|item| item
                .blocks
                .iter()
                .all(|block| !matches!(block, Block::Blank { .. }))),
            "one blank line between items is the loose-list break, {:?}",
            items
        );
    }

    #[test]
    fn extra_blank_lines_inside_quote_are_kept() {
        let ir = PreviewIr::from_markdown("> 甲\n>\n>\n> 乙\n");
        let Block::Quote { blocks, .. } = &ir.blocks[0] else {
            panic!("expected quote, got {:?}", ir.blocks[0]);
        };
        assert!(matches!(blocks[1], Block::Blank { lines: 1, .. }));
    }

    #[test]
    fn code_block_internal_blank_lines_are_content() {
        let ir = PreviewIr::from_markdown("```\na\n\n\nb\n```\n");
        assert_eq!(ir.blocks.len(), 1);
        assert!(matches!(&ir.blocks[0], Block::CodeBlock { text, .. } if text.contains("\n\n\n")));
    }

    #[test]
    fn plain_text_drops_extra_blank_lines() {
        let text = PreviewIr::from_markdown("甲\n\n\n乙\n").to_plain_text();
        assert_eq!(text, "甲\n乙");
    }

    #[test]
    fn plain_text_has_no_blank_lines_between_blocks() {
        let markdown = "# 标题\n\n第一行\n第二行\n\n- 条目\n- 条目二\n";
        let text = PreviewIr::from_markdown(markdown).to_plain_text();
        assert_eq!(text, "标题\n第一行 第二行\n- 条目\n- 条目二");
    }

    #[test]
    fn plain_text_keeps_code_block_lines() {
        let markdown = "```js\nlet a = 1;\nlet b = 2;\n```\n";
        let text = PreviewIr::from_markdown(markdown).to_plain_text();
        assert_eq!(text, "```js\nlet a = 1;\nlet b = 2;\n```");
    }

    #[test]
    fn parses_math_and_footnotes() {
        let markdown = "$$\na^2 + b^2\n$$\n\n引用[^1]\n\n[^1]: 脚注内容\n";
        let ir = PreviewIr::from_markdown(markdown);
        assert!(
            matches!(&ir.blocks[0], Block::Math { tex, display: true, .. } if tex.contains("a^2"))
        );
        assert!(
            matches!(&ir.blocks[1], Block::Paragraph { content, .. } if content.iter().any(|i| matches!(i, Inline::FootnoteRef(name) if name == "1")))
        );
        assert!(matches!(&ir.blocks[2], Block::FootnoteDefinition { label, .. } if label == "1"));
    }

    #[test]
    fn detects_standalone_images() {
        let ir = PreviewIr::from_markdown("![示意图](assets/demo.png)\n");
        assert!(
            matches!(&ir.blocks[0], Block::Image { src, alt, .. } if src == "assets/demo.png" && !alt.is_empty())
        );
    }

    #[test]
    fn splits_adjacent_image_lines_into_blocks() {
        let ir = PreviewIr::from_markdown("![one](../assets/a.jpg)\n![two](../assets/b.jpg)\n");
        assert_eq!(ir.blocks.len(), 2);
        assert!(matches!(
            &ir.blocks[0],
            Block::Image { src, .. } if src == "../assets/a.jpg"
        ));
        assert!(matches!(
            &ir.blocks[1],
            Block::Image { src, .. } if src == "../assets/b.jpg"
        ));
    }

    #[test]
    fn splits_softbroken_images_from_text_paragraph() {
        let ir = PreviewIr::from_markdown(
            "what? one.\n![one](../assets/a.jpg)\n<img src=\"../assets/b.jpg\" width=\"30%\">\n",
        );
        assert!(matches!(
            &ir.blocks[0],
            Block::Paragraph { content, .. }
                if content.iter().any(|item| matches!(item, Inline::Text(_)))
                    && !content.iter().any(|item| matches!(item, Inline::Image { .. }))
        ));
        assert!(matches!(
            &ir.blocks[1],
            Block::Image { src, .. } if src == "../assets/a.jpg"
        ));
        assert!(matches!(
            &ir.blocks[2],
            Block::Image { src, width: Some(Length::PercentCents(3000)), .. }
                if src == "../assets/b.jpg"
        ));
    }

    #[test]
    fn outline_lists_headings_with_lines() {
        let ir = PreviewIr::from_markdown("# 一\n\n正文\n\n## 二 **加粗**\n\n> ### 三\n");
        let outline = ir.outline();
        assert_eq!(outline.len(), 3);
        assert_eq!(
            outline[0],
            OutlineItem {
                level: 1,
                title: "一".into(),
                line: 1
            }
        );
        assert_eq!(
            outline[1],
            OutlineItem {
                level: 2,
                title: "二 加粗".into(),
                line: 5
            }
        );
        assert_eq!(outline[2].title, "三");
        assert_eq!(outline[2].line, 7);
    }

    #[test]
    fn lifts_html_img_div_span_and_style() {
        let markdown = r#"
<style>
.narrow { width: 40%; }
#warn { color: #c00; }
</style>

<img src="assets/a.jpg" alt="a" width="300px" />

<div id="warn" class="narrow">**不是粗体**<b>x</b></div>

hello <span style="color:#06c">蓝</span>
"#;
        let ir = PreviewIr::from_markdown(markdown);
        assert_eq!(ir.styles.rules.len(), 2);
        assert!(ir.blocks.iter().any(|block| matches!(
            block,
            Block::Image { src, width: Some(Length::Px(300)), .. } if src == "assets/a.jpg"
        )));
        assert!(ir.blocks.iter().any(|block| matches!(
            block,
            Block::Div { text, attrs, .. }
                if text == "**不是粗体**x" && attrs.id.as_deref() == Some("warn")
        )));
        let has_span = ir.blocks.iter().any(|block| match block {
            Block::Paragraph { content, .. } => content.iter().any(|inline| {
                matches!(
                    inline,
                    Inline::Span { text, attrs, .. }
                        if text == "蓝" && attrs.style.color == Some(RgbColor { r: 0, g: 0x66, b: 0xcc })
                )
            }),
            _ => false,
        });
        assert!(has_span);
        assert!(!ir.to_plain_text().contains("<style"));
    }
}
