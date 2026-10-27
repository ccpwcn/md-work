//! Renders a [`PreviewIr`] with native GPUI elements.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdwork_render::preview::{Align, Block, Inline, ListItem, PreviewIr};

const BODY_SIZE: f32 = 16.0;
const CODE_FAMILY: &str = "Consolas";
/// Point size used when rasterising equations for the preview.
pub const MATH_SIZE_PT: f32 = 12.0;

struct InlineStyle {
    foreground: Hsla,
    muted: Hsla,
    link: Hsla,
    code_background: Hsla,
    code_foreground: Hsla,
    size: f32,
}

struct RenderCtx<'a> {
    base: Option<&'a Path>,
    style: InlineStyle,
    math: &'a HashMap<u64, Option<PathBuf>>,
    wrap_code: bool,
}

pub fn render(
    ir: &PreviewIr,
    base: Option<&Path>,
    math: &HashMap<u64, Option<PathBuf>>,
    wrap_code: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let context = RenderCtx {
        base,
        style: InlineStyle {
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            link: theme.link,
            code_background: theme.list_hover,
            code_foreground: theme.foreground,
            size: BODY_SIZE,
        },
        math,
        wrap_code,
    };

    let mut column = div().v_flex().w_full().gap_3().px(px(28.)).py(px(20.));
    if ir.blocks.is_empty() {
        column = column.child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("—"),
        );
    }
    for block in &ir.blocks {
        column = column.child(render_block(block, &context, cx));
    }

    div()
        .id("preview-scroll")
        .size_full()
        .overflow_y_scroll()
        .child(column)
        .into_any_element()
}

fn render_block(block: &Block, context: &RenderCtx<'_>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let style = &context.style;
    match block {
        Block::Heading { level, content, .. } => {
            let size = match level {
                1 => 30.,
                2 => 24.,
                3 => 20.,
                4 => 18.,
                _ => 16.,
            };
            let heading_style = InlineStyle {
                size,
                foreground: theme.foreground,
                ..clone_style(style)
            };
            let text = styled_inlines(content, &heading_style, FontWeight::BOLD);
            div()
                .w_full()
                .pt(px(8.))
                .child(
                    StyledText::new(text.text)
                        .with_default_highlights(&text.style, text.highlights),
                )
                .into_any_element()
        }
        Block::Paragraph { content, .. } => render_paragraph(content, context),
        Block::CodeBlock { language, text, .. } => {
            let mut block = div()
                .w_full()
                .rounded_md()
                .bg(theme.list_hover)
                .p_3()
                .v_flex()
                .gap_1();
            if let Some(language) = language.as_deref().filter(|lang| !lang.is_empty()) {
                block = block.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(language.to_string()),
                );
            }
            let mut code = div()
                .id(code_block_id(text, context.wrap_code))
                .w_full()
                .font_family(CODE_FAMILY)
                .text_sm()
                .text_color(theme.foreground)
                .child(text.trim_end_matches('\n').to_string());
            let body: AnyElement = if context.wrap_code {
                code.into_any_element()
            } else {
                code = code.whitespace_nowrap();
                div()
                    .id(code_block_id(text, false))
                    .w_full()
                    .overflow_x_scrollbar()
                    .child(code)
                    .into_any_element()
            };
            block.child(body).into_any_element()
        }
        Block::List {
            ordered,
            start,
            items,
            ..
        } => render_list(*ordered, *start, items, context, cx),
        Block::Quote { blocks, .. } => {
            let mut inner = div().v_flex().w_full().gap_2();
            for block in blocks {
                inner = inner.child(render_block(block, context, cx));
            }
            div()
                .w_full()
                .pl_3()
                .border_l_2()
                .border_color(theme.border)
                .child(inner)
                .into_any_element()
        }
        Block::Table {
            head,
            rows,
            aligns,
            ..
        } => render_table(head, rows, aligns, style, cx),
        Block::Image { src, alt, .. } => {
            let label = if alt.is_empty() {
                src.clone()
            } else {
                format!("{} · {src}", plain_inlines(alt))
            };
            let placeholder = div()
                .w_full()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .px_3()
                .py_4()
                .v_flex()
                .gap_1()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(label),
                );
            match resolve_image(src, context.base) {
                Some(path) => div()
                    .w_full()
                    .v_flex()
                    .items_center()
                    .child(img(path).max_w_full().rounded_md())
                    .into_any_element(),
                None => placeholder.into_any_element(),
            }
        }
        Block::Rule { .. } => div()
            .w_full()
            .h(px(1.))
            .bg(theme.border)
            .my(px(6.))
            .into_any_element(),
        Block::Math { tex, display, .. } => {
            let key = math_key(tex, *display, &fill_hex(style.foreground), MATH_SIZE_PT);
            match context.math.get(&key).and_then(|path| path.as_ref()) {
                Some(path) => div()
                    .w_full()
                    .v_flex()
                    .items_center()
                    .child(img(path.clone()).max_w_full())
                    .into_any_element(),
                None => math_placeholder(tex, *display, theme, true),
            }
        }
        Block::FootnoteDefinition { label, blocks, .. } => {
            let mut inner = div().v_flex().gap_1();
            for block in blocks {
                inner = inner.child(render_block(block, context, cx));
            }
            div()
                .w_full()
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("[^{label}]")),
                )
                .child(inner)
                .into_any_element()
        }
        Block::Html { raw, .. } => div()
            .w_full()
            .font_family(CODE_FAMILY)
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(raw.trim_end().to_string())
            .into_any_element(),
    }
}

fn math_placeholder(tex: &str, display: bool, theme: &gpui_kit::component::Theme, align: bool) -> AnyElement {
    let mut block = div()
        .rounded_md()
        .bg(theme.list_hover)
        .px_3()
        .py_2()
        .font_family(CODE_FAMILY)
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(if display {
            tex.trim().to_string()
        } else {
            format!("${tex}$")
        });
    if align {
        block = div()
            .w_full()
            .v_flex()
            .items_center()
            .child(block);
    }
    block.into_any_element()
}

fn render_paragraph(inlines: &[Inline], context: &RenderCtx<'_>) -> AnyElement {
    let has_inline_math = inlines
        .iter()
        .any(|inline| matches!(inline, Inline::Math { display: false, .. }));
    if !has_inline_math {
        return styled_text(inlines, &context.style);
    }

    // Inline equations are SVG images, so the paragraph is split into a
    // wrapping row of text runs and equation images.
    let mut row = div().w_full().flex().flex_wrap().items_center();
    let mut start = 0;
    let mut index = 0;
    while index < inlines.len() {
        let tex = match &inlines[index] {
            Inline::Math { tex, display: false } => tex,
            _ => {
                index += 1;
                continue;
            }
        };
        if start < index {
            row = row.child(styled_text(&inlines[start..index], &context.style));
        }
        let key = math_key(tex, false, &fill_hex(context.style.foreground), MATH_SIZE_PT);
        row = row.child(match context.math.get(&key).and_then(|path| path.as_ref()) {
            Some(path) => img(path.clone())
                .h(px(BODY_SIZE * 1.25))
                .max_w_full()
                .into_any_element(),
            None => styled_text(&inlines[index..index + 1], &context.style),
        });
        start = index + 1;
        index += 1;
    }
    if start < inlines.len() {
        row = row.child(styled_text(&inlines[start..], &context.style));
    }
    row.into_any_element()
}

fn styled_text(inlines: &[Inline], style: &InlineStyle) -> AnyElement {
    let text = styled_inlines(inlines, style, FontWeight::NORMAL);
    StyledText::new(text.text)
        .with_default_highlights(&text.style, text.highlights)
        .into_any_element()
}

fn render_list(
    ordered: bool,
    start: usize,
    items: &[ListItem],
    context: &RenderCtx<'_>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let mut column = div().v_flex().w_full().gap_1();
    for (index, item) in items.iter().enumerate() {
        let marker = match item.task {
            Some(true) => "☑".to_string(),
            Some(false) => "☐".to_string(),
            None if ordered => format!("{}.", start + index),
            None => "•".to_string(),
        };
        let mut content = div().v_flex().flex_1().min_w_0().gap_1();
        for block in &item.blocks {
            if let Block::List {
                ordered,
                start,
                items,
                ..
            } = block
            {
                content = content.child(render_list(*ordered, *start, items, context, cx));
            } else {
                content = content.child(render_block(block, context, cx));
            }
        }
        column = column.child(
            div()
                .w_full()
                .flex()
                .gap_2()
                .child(
                    div()
                        .w(px(20.))
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(marker),
                )
                .child(content),
        );
    }
    column.into_any_element()
}

fn render_table(
    head: &[Vec<Inline>],
    rows: &[Vec<Vec<Inline>>],
    aligns: &[Align],
    style: &InlineStyle,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let mut table = div()
        .w_full()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .v_flex();

    if !head.is_empty() {
        table = table.child(table_row(head, aligns, style, true));
    }
    for row in rows {
        table = table.child(table_row(row, aligns, style, false));
    }
    table.into_any_element()
}

fn table_row(
    cells: &[Vec<Inline>],
    aligns: &[Align],
    style: &InlineStyle,
    header: bool,
) -> AnyElement {
    let mut row = div()
        .w_full()
        .flex()
        .border_b_1()
        .when(header, |this| this.bg(style.code_background));
    for (index, cell) in cells.iter().enumerate() {
        let align = aligns.get(index).copied().unwrap_or(Align::None);
        let weight = if header {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        };
        let text = styled_inlines(cell, style, weight);
        let mut cell_div = div()
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1()
            .text_sm()
            .child(
                StyledText::new(text.text)
                    .with_default_highlights(&text.style, text.highlights),
            );
        cell_div = match align {
            Align::Center => cell_div.text_center(),
            Align::Right => cell_div.text_right(),
            Align::Left | Align::None => cell_div,
        };
        row = row.child(cell_div);
    }
    row.into_any_element()
}

struct StyledContent {
    text: String,
    style: TextStyle,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
}

/// Distinct identity per wrap mode so GPUI re-measures the text when toggled.
fn code_block_id(text: &str, wrap: bool) -> ElementId {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    wrap.hash(&mut hasher);
    ElementId::Name(format!("code-{:016x}", hasher.finish()).into())
}

fn styled_inlines(
    inlines: &[Inline],
    style: &InlineStyle,
    weight: FontWeight,
) -> StyledContent {
    let mut text = String::new();
    let mut highlights = Vec::new();
    append_inlines(inlines, style, 0, &mut text, &mut highlights);

    let mut text_style = TextStyle::default();
    text_style.color = style.foreground;
    text_style.font_size = px(style.size).into();
    text_style.font_weight = weight;
    StyledContent {
        text,
        style: text_style,
        highlights,
    }
}

fn append_inlines(
    inlines: &[Inline],
    style: &InlineStyle,
    weight: u8,
    out: &mut String,
    highlights: &mut Vec<(Range<usize>, HighlightStyle)>,
) {
    for inline in inlines {
        let start = out.len();
        match inline {
            Inline::Text(text) => out.push_str(text),
            Inline::Code(code) => {
                out.push('`');
                out.push_str(code);
                out.push('`');
            }
            Inline::Emphasis(children) | Inline::Strong(children) | Inline::Strike(children) => {
                let is_strong = matches!(inline, Inline::Strong(_));
                let is_strike = matches!(inline, Inline::Strike(_));
                let start = out.len();
                append_inlines(children, style, weight, out, highlights);
                let mut highlight = HighlightStyle::default();
                if is_strong || weight >= 2 {
                    highlight.font_weight = Some(FontWeight::BOLD);
                } else if weight == 1 || matches!(inline, Inline::Emphasis(_)) {
                    highlight.font_style = Some(FontStyle::Italic);
                }
                if is_strike {
                    highlight.strikethrough = Some(StrikethroughStyle {
                        thickness: px(1.),
                        color: Some(style.muted),
                    });
                }
                highlights.push((start..out.len(), highlight));
                continue;
            }
            Inline::Link { url, children, .. } => {
                if children.is_empty() {
                    out.push_str(url);
                } else {
                    append_inlines(children, style, weight, out, highlights);
                }
                highlights.push((
                    start..out.len(),
                    HighlightStyle {
                        color: Some(style.link),
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(style.link),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                ));
                continue;
            }
            Inline::Image { src, alt, .. } => {
                let alt = plain_inlines(alt);
                if alt.is_empty() {
                    out.push_str(src);
                } else {
                    out.push_str(&alt);
                }
                highlights.push((
                    start..out.len(),
                    HighlightStyle {
                        color: Some(style.muted),
                        font_style: Some(FontStyle::Italic),
                        ..Default::default()
                    },
                ));
                continue;
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
                highlights.push((
                    start..out.len(),
                    HighlightStyle {
                        color: Some(style.code_foreground),
                        background_color: Some(style.code_background),
                        ..Default::default()
                    },
                ));
                continue;
            }
        }
        if out.len() > start {
            let mut highlight = HighlightStyle::default();
            match inline {
                Inline::Code(_) => {
                    highlight.background_color = Some(style.code_background);
                    highlight.color = Some(style.code_foreground);
                }
                Inline::FootnoteRef(_) => {
                    highlight.color = Some(style.link);
                }
                _ => {}
            }
            if highlight.color.is_some() || highlight.background_color.is_some() {
                highlights.push((start..out.len(), highlight));
            }
        }
    }
}

fn plain_inlines(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) => out.push_str(text),
            Inline::Code(code) => out.push_str(code),
            Inline::Emphasis(children)
            | Inline::Strong(children)
            | Inline::Strike(children) => out.push_str(&plain_inlines(children)),
            Inline::Link { url, children, .. } => {
                if children.is_empty() {
                    out.push_str(url);
                } else {
                    out.push_str(&plain_inlines(children));
                }
            }
            Inline::Image { src, alt, .. } => {
                let alt = plain_inlines(alt);
                out.push_str(if alt.is_empty() { src } else { &alt });
            }
            Inline::SoftBreak => out.push(' '),
            Inline::HardBreak => out.push('\n'),
            Inline::FootnoteRef(name) => out.push_str(&format!("[^{name}]")),
            Inline::Math { tex, .. } => out.push_str(tex),
        }
    }
    out
}

fn clone_style(style: &InlineStyle) -> InlineStyle {
    InlineStyle {
        foreground: style.foreground,
        muted: style.muted,
        link: style.link,
        code_background: style.code_background,
        code_foreground: style.code_foreground,
        size: style.size,
    }
}

fn resolve_image(src: &str, base: Option<&Path>) -> Option<PathBuf> {
    let lower = src.to_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("data:")
        || lower.starts_with("//")
    {
        return None;
    }
    let decoded = percent_decode(src);
    let path = PathBuf::from(decoded.replace('/', std::path::MAIN_SEPARATOR_STR));
    let path = if path.is_absolute() {
        path
    } else {
        base?.join(path)
    };
    path.is_file().then_some(path)
}

fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Stable cache key for a rendered equation.
pub fn math_key(tex: &str, display: bool, fill: &str, size_pt: f32) -> u64 {
    let mut hasher = DefaultHasher::new();
    tex.hash(&mut hasher);
    display.hash(&mut hasher);
    fill.hash(&mut hasher);
    size_pt.to_bits().hash(&mut hasher);
    hasher.finish()
}

pub fn fill_hex(color: Hsla) -> String {
    let rgba: Rgba = color.into();
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgba.r),
        channel(rgba.g),
        channel(rgba.b)
    )
}

/// Visits every equation in the document once, in document order.
pub fn collect_math(ir: &PreviewIr, visit: &mut impl FnMut(&str, bool)) {
    fn walk_inline(inlines: &[Inline], visit: &mut impl FnMut(&str, bool)) {
        for inline in inlines {
            match inline {
                Inline::Emphasis(children)
                | Inline::Strong(children)
                | Inline::Strike(children) => walk_inline(children, visit),
                Inline::Link { children, .. } => walk_inline(children, visit),
                Inline::Image { alt, .. } => walk_inline(alt, visit),
                Inline::Math { tex, display } => visit(tex, *display),
                _ => {}
            }
        }
    }

    fn walk_block(block: &Block, visit: &mut impl FnMut(&str, bool)) {
        match block {
            Block::Heading { content, .. } | Block::Paragraph { content, .. } => {
                walk_inline(content, visit)
            }
            Block::Math { tex, display, .. } => visit(tex, *display),
            Block::List { items, .. } => {
                for item in items {
                    for block in &item.blocks {
                        walk_block(block, visit);
                    }
                }
            }
            Block::Quote { blocks, .. } | Block::FootnoteDefinition { blocks, .. } => {
                for block in blocks {
                    walk_block(block, visit);
                }
            }
            Block::Table { head, rows, .. } => {
                for cell in head {
                    walk_inline(cell, visit);
                }
                for row in rows {
                    for cell in row {
                        walk_inline(cell, visit);
                    }
                }
            }
            Block::Image { alt, .. } => walk_inline(alt, visit),
            _ => {}
        }
    }

    for block in &ir.blocks {
        walk_block(block, visit);
    }
}
