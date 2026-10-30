//! Markdown → preview intermediate representation.
//!
//! The GUI and the exporters both consume this tree instead of re-parsing
//! Markdown, so headings, tables and code blocks stay consistent everywhere.

use comrak::nodes::{AstNode, ListType, NodeList, NodeTable, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewIr {
    pub blocks: Vec<Block>,
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
        line: usize,
    },
    Rule {
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
    Html {
        raw: String,
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
        let mut blocks = Vec::new();
        collect_blocks(root, &mut blocks);
        Self { blocks }
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

fn collect_blocks<'a>(node: &'a AstNode<'a>, out: &mut Vec<Block>) {
    for child in node.children() {
        if let Some(block) = block_from_node(child) {
            out.push(block);
        }
    }
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

fn block_from_node<'a>(node: &'a AstNode<'a>) -> Option<Block> {
    let line = node.data().sourcepos.start.line;
    match &node.data().value {
        NodeValue::Heading(heading) => Some(Block::Heading {
            level: heading.level,
            content: inlines(node),
            line,
        }),
        NodeValue::Paragraph => {
            let content = inlines(node);
            if let Some(image) = standalone_image(&content, line) {
                return Some(image);
            }
            if let Some(math) = standalone_math(&content, line) {
                return Some(math);
            }
            Some(Block::Paragraph { content, line })
        }
        NodeValue::CodeBlock(code) => {
            let language = code
                .info
                .split_whitespace()
                .next()
                .filter(|info| !info.is_empty())
                .map(str::to_string);
            Some(Block::CodeBlock {
                language,
                text: code.literal.clone(),
                line,
            })
        }
        NodeValue::List(list) => Some(block_list(node, list, line)),
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
            let mut blocks = Vec::new();
            collect_blocks(node, &mut blocks);
            Some(Block::Quote { blocks, line })
        }
        NodeValue::ThematicBreak => Some(Block::Rule { line }),
        NodeValue::Table(table) => Some(block_table(node, table, line)),
        NodeValue::Math(math) => Some(Block::Math {
            tex: math.literal.clone(),
            display: math.display_math,
            line,
        }),
        NodeValue::FootnoteDefinition(footnote) => {
            let mut blocks = Vec::new();
            collect_blocks(node, &mut blocks);
            Some(Block::FootnoteDefinition {
                label: footnote.name.clone(),
                blocks,
                line,
            })
        }
        NodeValue::HtmlBlock(html) => Some(Block::Html {
            raw: html.literal.clone(),
            line,
        }),
        _ => None,
    }
}

fn standalone_image(content: &[Inline], line: usize) -> Option<Block> {
    let mut image = None;
    for inline in content {
        match inline {
            Inline::Image { src, alt, title } if image.is_none() => {
                image = Some((src.clone(), alt.clone(), title.clone()));
            }
            Inline::Text(text) if text.trim().is_empty() => {}
            Inline::SoftBreak | Inline::HardBreak => {}
            _ => return None,
        }
    }
    image.map(|(src, alt, title)| Block::Image {
        src,
        title,
        alt,
        line,
    })
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

fn block_list<'a>(node: &'a AstNode<'a>, list: &NodeList, line: usize) -> Block {
    let mut items = Vec::new();
    for item in node.children() {
        // With the tasklist extension, checked items sit directly under the
        // list as `TaskItem` while regular items are wrapped in `Item`.
        let task = match &item.data().value {
            NodeValue::TaskItem(marker) => Some(marker.symbol.is_some()),
            NodeValue::Item(_) => None,
            _ => continue,
        };
        items.push(ListItem {
            task,
            blocks: item_blocks(item),
        });
    }
    Block::List {
        ordered: matches!(list.list_type, ListType::Ordered),
        start: list.start,
        tight: list.tight,
        items,
        line,
    }
}

fn item_blocks<'a>(item: &'a AstNode<'a>) -> Vec<Block> {
    let item_line = item.data().sourcepos.start.line;
    let mut blocks = Vec::new();
    let mut pending: Vec<Inline> = Vec::new();
    for child in item.children() {
        if let Some(block) = block_from_node(child) {
            if !pending.is_empty() {
                blocks.push(Block::Paragraph {
                    content: std::mem::take(&mut pending),
                    line: item_line,
                });
            }
            blocks.push(block);
        } else if let Some(inline) = inline_from_node(child) {
            pending.push(inline);
        }
    }
    if !pending.is_empty() {
        blocks.push(Block::Paragraph {
            content: pending,
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
    node.children().filter_map(inline_from_node).collect()
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
        NodeValue::HtmlInline(html) => Some(Inline::Text(html.clone())),
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
        Block::Rule { .. } => "---".to_string(),
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
        Block::Html { raw, .. } => raw.trim_end().to_string(),
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
}
