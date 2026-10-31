//! Renders a [`PreviewIr`] with native GPUI elements.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdwork_render::preview::{Align, Block, Inline, ListItem, PreviewIr};
use rust_i18n::t;

const DEFAULT_BODY_SIZE: f32 = 16.0;
const CODE_FAMILY: &str = "Consolas";
/// Point size used when rasterising equations for the preview.
pub const MATH_SIZE_PT: f32 = 12.0;

struct InlineStyle {
    foreground: Hsla,
    muted: Hsla,
    link: Hsla,
    code_background: Hsla,
    code_foreground: Hsla,
    family: SharedString,
    size: f32,
}

struct RenderCtx<'a> {
    base: Option<&'a Path>,
    style: InlineStyle,
    math: &'a HashMap<u64, Option<PathBuf>>,
    images: &'a HashMap<u64, Option<PathBuf>>,
    wrap_code: bool,
    body_size: f32,
}

pub fn render(
    ir: &PreviewIr,
    base: Option<&Path>,
    math: &HashMap<u64, Option<PathBuf>>,
    images: &HashMap<u64, Option<PathBuf>>,
    wrap_code: bool,
    note_font: &str,
    note_size: f32,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let body_size = if note_size > 0.0 {
        note_size.clamp(10.0, 32.0)
    } else {
        DEFAULT_BODY_SIZE
    };
    let family = if note_font.trim().is_empty() {
        theme.font_family.clone()
    } else {
        SharedString::from(note_font.trim().to_string())
    };
    let context = RenderCtx {
        base,
        style: InlineStyle {
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            link: theme.link,
            code_background: theme.list_hover,
            code_foreground: theme.foreground,
            family: family.clone(),
            size: body_size,
        },
        math,
        images,
        wrap_code,
        body_size,
    };

    let mut column = div()
        .v_flex()
        .w_full()
        .gap_3()
        .px(px(28.))
        .py(px(20.))
        .font_family(family);
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
            let size = context.body_size
                * match level {
                    1 => 1.875,
                    2 => 1.5,
                    3 => 1.25,
                    4 => 1.125,
                    _ => 1.0,
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
            let code_text = text.trim_end_matches('\n').to_string();
            let mut block = div()
                .w_full()
                .rounded_md()
                .bg(theme.list_hover)
                .p_3()
                .v_flex()
                .gap_1();

            let mut header = div().h_flex().w_full().items_center().gap_2();
            if let Some(language) = language.as_deref().filter(|lang| !lang.is_empty()) {
                header = header.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(language.to_string()),
                );
            }
            header = header.child(div().flex_1());
            if !code_text.is_empty() {
                let copy = code_text.clone();
                header = header.child(
                    Button::new(code_copy_id(&code_text))
                        .ghost()
                        .compact()
                        .label(t!("preview.copy_code").to_string())
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        }),
                );
            }
            block = block.child(header);

            let mut text_style = TextStyle::default();
            text_style.color = theme.foreground;
            text_style.font_family = SharedString::from(CODE_FAMILY);
            text_style.font_size = px((context.body_size - 2.0).max(10.0)).into();
            let spans =
                crate::highlight::code_spans(language.as_deref(), &code_text, theme.is_dark());
            let spans = flatten_highlights(&code_text, spans.as_ref().clone());
            let styled =
                StyledText::new(code_text.clone()).with_default_highlights(&text_style, spans);
            let body: AnyElement = if context.wrap_code {
                div()
                    .id(code_block_id(&code_text, true))
                    .w_full()
                    .child(styled)
                    .into_any_element()
            } else {
                div()
                    .id(code_block_id(&code_text, false))
                    .w_full()
                    .overflow_x_scrollbar()
                    .whitespace_nowrap()
                    .child(styled)
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
            head, rows, aligns, ..
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
            match context
                .images
                .get(&image_key(src, context.base))
                .and_then(|path| path.clone())
            {
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

fn math_placeholder(
    tex: &str,
    display: bool,
    theme: &gpui_kit::component::Theme,
    align: bool,
) -> AnyElement {
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
        block = div().w_full().v_flex().items_center().child(block);
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
            Inline::Math {
                tex,
                display: false,
            } => tex,
            _ => {
                index += 1;
                continue;
            }
        };
        if start < index {
            row = row.child(styled_text(&inlines[start..index], &context.style));
        }
        let key = math_key(
            tex,
            false,
            &fill_hex(context.style.foreground),
            MATH_SIZE_PT,
        );
        row = row.child(
            match context.math.get(&key).and_then(|path| path.as_ref()) {
                Some(path) => img(path.clone())
                    .h(px(context.body_size * 1.25))
                    .max_w_full()
                    .into_any_element(),
                None => styled_text(&inlines[index..index + 1], &context.style),
            },
        );
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
        let mut cell_div = div().flex_1().min_w_0().px_2().py_1().text_sm().child(
            StyledText::new(text.text).with_default_highlights(&text.style, text.highlights),
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

fn code_copy_id(text: &str) -> ElementId {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    ElementId::Name(format!("copy-{:016x}", hasher.finish()).into())
}

fn styled_inlines(inlines: &[Inline], style: &InlineStyle, weight: FontWeight) -> StyledContent {
    let mut text = String::new();
    let mut highlights = Vec::new();
    append_inlines(inlines, style, 0, &mut text, &mut highlights);

    let mut text_style = TextStyle::default();
    text_style.color = style.foreground;
    text_style.font_family = style.family.clone();
    text_style.font_size = px(style.size).into();
    text_style.font_weight = weight;
    // Nested inlines (bold+code, link+emphasis, …) emit overlapping ranges;
    // GPUI's StyledText::with_default_highlights requires a non-overlapping
    // partition of the text or it panics with "invalid text run".
    let highlights = flatten_highlights(&text, highlights);
    StyledContent {
        text,
        style: text_style,
        highlights,
    }
}

/// Merge overlapping / unsorted highlight ranges into a sorted, non-overlapping
/// list. Later styles win on conflicts via [`HighlightStyle::highlight`].
/// Ranges that are empty, past `text`, or not on UTF-8 char boundaries are dropped.
fn flatten_highlights(
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    if highlights.is_empty() || text.is_empty() {
        return Vec::new();
    }

    let text_len = text.len();
    let mut bounds = Vec::with_capacity(highlights.len() * 2);
    let mut layers = Vec::with_capacity(highlights.len());
    for (range, style) in highlights {
        let start = range.start.min(text_len);
        let end = range.end.min(text_len);
        if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        bounds.push(start);
        bounds.push(end);
        layers.push((start..end, style));
    }
    if layers.is_empty() {
        return Vec::new();
    }
    bounds.sort_unstable();
    bounds.dedup();

    let mut out = Vec::new();
    for window in bounds.windows(2) {
        let start = window[0];
        let end = window[1];
        if start >= end {
            continue;
        }
        let mut merged: Option<HighlightStyle> = None;
        for (range, style) in &layers {
            if range.start <= start && range.end >= end {
                merged = Some(match merged {
                    Some(prev) => prev.highlight(*style),
                    None => *style,
                });
            }
        }
        if let Some(style) = merged {
            out.push((start..end, style));
        }
    }
    out
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
            Inline::Emphasis(children) | Inline::Strong(children) | Inline::Strike(children) => {
                out.push_str(&plain_inlines(children))
            }
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
        family: style.family.clone(),
        size: style.size,
    }
}

/// Resolves a markdown image source to a path without touching the disk; the
/// caller checks existence on the background executor.
pub fn resolve_image_path(src: &str, base: Option<&Path>) -> Option<PathBuf> {
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
    if path.is_absolute() {
        Some(path)
    } else {
        base.map(|base| base.join(path))
    }
}

/// Cache key that distinguishes the same relative `src` in different folders.
pub fn image_key(src: &str, base: Option<&Path>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    base.map(|path| path.to_string_lossy().to_lowercase())
        .hash(&mut hasher);
    src.hash(&mut hasher);
    hasher.finish()
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

/// Visits every image source in the document so paths can be resolved off the
/// render thread.
pub fn collect_images(ir: &PreviewIr, visit: &mut impl FnMut(&str)) {
    fn walk_block(block: &Block, visit: &mut impl FnMut(&str)) {
        match block {
            Block::Image { src, .. } => visit(src),
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
            _ => {}
        }
    }

    for block in &ir.blocks {
        walk_block(block, visit);
    }
}

#[cfg(test)]
mod tests {
    // Avoid `use super::*`: `gpui_kit::*` re-exports `gpui::test`, which would
    // shadow the std `#[test]` attribute and recurse during expansion.
    use super::{
        FontStyle, FontWeight, HighlightStyle, Inline, InlineStyle, SharedString,
        flatten_highlights, styled_inlines,
    };
    use std::ops::Range;

    fn sample_style() -> InlineStyle {
        InlineStyle {
            foreground: gpui_kit::rgb(0x111111).into(),
            muted: gpui_kit::rgb(0x666666).into(),
            link: gpui_kit::rgb(0x0066cc).into(),
            code_background: gpui_kit::rgb(0xeeeeee).into(),
            code_foreground: gpui_kit::rgb(0x111111).into(),
            family: SharedString::from("sans"),
            size: 16.0,
        }
    }

    /// Mirrors GPUI's `StyledText::with_runs` invariant: run lengths must be
    /// char-boundary slices that never overlap.
    fn assert_valid_highlight_partition(text: &str, highlights: &[(Range<usize>, HighlightStyle)]) {
        let mut ix = 0usize;
        for (range, _) in highlights {
            assert!(
                range.start >= ix,
                "highlights must be sorted and non-overlapping: {range:?} after {ix}"
            );
            assert!(text.is_char_boundary(range.start));
            assert!(text.is_char_boundary(range.end));
            assert!(range.end <= text.len());
            assert!(range.start < range.end);
            ix = range.end;
        }
        let mut cursor = 0usize;
        for (range, _) in highlights {
            let _ = &text[cursor..range.start];
            let _ = &text[range.start..range.end];
            cursor = range.end;
        }
        let _ = &text[cursor..];
    }

    #[::core::prelude::v1::test]
    fn nested_strong_and_code_highlights_are_partitioned() {
        let inlines = [Inline::Strong(vec![
            Inline::Text("bold ".into()),
            Inline::Code("code".into()),
            Inline::Text(" more".into()),
        ])];
        let styled = styled_inlines(&inlines, &sample_style(), FontWeight::NORMAL);
        assert!(styled.text.contains("code"));
        assert_valid_highlight_partition(&styled.text, &styled.highlights);
        for (i, (a, _)) in styled.highlights.iter().enumerate() {
            for (b, _) in styled.highlights.iter().skip(i + 1) {
                assert!(a.end <= b.start || b.end <= a.start);
            }
        }
    }

    #[::core::prelude::v1::test]
    fn nested_link_emphasis_highlights_are_partitioned() {
        let inlines = [Inline::Link {
            url: "https://example.com".into(),
            title: None,
            children: vec![Inline::Emphasis(vec![Inline::Text("click".into())])],
        }];
        let styled = styled_inlines(&inlines, &sample_style(), FontWeight::NORMAL);
        assert_eq!(styled.text, "click");
        assert_valid_highlight_partition(&styled.text, &styled.highlights);
    }

    #[::core::prelude::v1::test]
    fn flatten_merges_overlap_and_drops_bad_ranges() {
        let text = "你好世界";
        let bad = vec![
            (
                0..text.len(),
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
            ),
            (
                0..text.chars().next().unwrap().len_utf8(),
                HighlightStyle {
                    color: Some(gpui_kit::rgb(0xff0000).into()),
                    ..Default::default()
                },
            ),
            (
                1..text.len(),
                HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
            ),
        ];
        let flat = flatten_highlights(text, bad);
        assert_valid_highlight_partition(text, &flat);
        assert!(!flat.is_empty());
    }
}
