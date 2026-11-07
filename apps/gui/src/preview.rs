//! Renders a [`PreviewIr`] with native GPUI elements.

use std::cell::Cell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use mdwork_render::html_subset::{
    ElementAttrs, Length, RgbColor, StyleDecls, StyleSheet, resolve_img_size,
};
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
    /// 本次渲染共享的阅读顺序，用作每个文本片段的稳定
    /// 元素 id 与文档顺序；纯文本单元测试中为 `None`。
    order: Option<Rc<Cell<u64>>>,
    /// 本次渲染共享的选区帧数据；纯文本单元测试中为 `None`。
    selection: Option<crate::preview_selection::SharedPreviewSelection>,
}

/// Resolved preview image: on-disk path plus natural pixel size (for layout).
#[derive(Clone, Debug)]
pub struct PreviewImage {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

struct RenderCtx<'a> {
    base: Option<&'a Path>,
    style: InlineStyle,
    border: Hsla,
    styles: &'a StyleSheet,
    math: &'a HashMap<u64, Option<PathBuf>>,
    images: &'a HashMap<u64, Option<PreviewImage>>,
    wrap_code: bool,
    body_size: f32,
}

#[expect(
    clippy::too_many_arguments,
    reason = "预览渲染需要显式传入主题、图片、字体等彼此独立的参数"
)]
pub fn render(
    ir: &PreviewIr,
    base: Option<&Path>,
    math: &HashMap<u64, Option<PathBuf>>,
    images: &HashMap<u64, Option<PreviewImage>>,
    wrap_code: bool,
    note_font: &str,
    note_size: f32,
    frame: crate::preview_selection::SharedPreviewSelection,
    cx: &App,
) -> AnyElement {
    // 全量重建正文前清空上一帧的选区片段：内容层缓存失效（滚动、窗口
    // 缩放、图片/公式加载）会重新 prepaint 并向共享帧追加 runs。若不清空，
    // 覆盖层会把同一选区按累积的重复片段绘制多次——半透明高亮越叠越深，
    // 旧片段还带着旧滚动偏移的 bounds，投影出错位残影，且 runs 无限增长
    // 让 update_runs 每帧越来越慢。每次全量 re-prepaint 前必经一次 render()，
    // 在此清空即可保证 runs 恒为「当前这一份」。
    frame.borrow_mut().clear();
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
            order: Some(Rc::new(Cell::new(0))),
            selection: Some(frame.clone()),
        },
        border: theme.border,
        styles: &ir.styles,
        math,
        images,
        wrap_code,
        body_size,
    };

    let mut column = div()
        .v_flex()
        .w_full()
        .gap_1()
        .px(px(28.))
        .py(px(16.))
        .font_family(family)
        .text_size(px(body_size))
        .line_height(relative(1.35));
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
        .overflow_y_scrollbar()
        .child(column)
        .into_any_element()
}

/// 预览正文内容层：把 [`PreviewIr`] 渲染成原生 GPUI 元素树。它被上层用
/// `Entity::cached` 包裹，文档内容不变时整段跳过重绘与重新排版；选区高亮
/// 由独立的 [`SelectionOverlay`](crate::preview_selection::SelectionOverlay)
/// 负责，本层不绘制选区。
pub struct MarkdownPreview {
    ir: PreviewIr,
    base: Option<PathBuf>,
    math: HashMap<u64, Option<PathBuf>>,
    images: HashMap<u64, Option<PreviewImage>>,
    wrap_code: bool,
    note_font: String,
    note_size: f32,
    frame: crate::preview_selection::SharedPreviewSelection,
}

impl MarkdownPreview {
    pub fn new(
        ir: PreviewIr,
        base: Option<PathBuf>,
        math: HashMap<u64, Option<PathBuf>>,
        images: HashMap<u64, Option<PreviewImage>>,
        wrap_code: bool,
        note_font: String,
        note_size: f32,
        frame: crate::preview_selection::SharedPreviewSelection,
    ) -> Self {
        Self {
            ir,
            base,
            math,
            images,
            wrap_code,
            note_font,
            note_size,
            frame,
        }
    }

    /// 异步加载完图片/公式后，由 AppShell 注入最新资产并触发本层重绘。
    pub fn set_assets(
        &mut self,
        math: HashMap<u64, Option<PathBuf>>,
        images: HashMap<u64, Option<PreviewImage>>,
        cx: &mut Context<Self>,
    ) {
        self.math = math;
        self.images = images;
        cx.notify();
    }
}

impl Render for MarkdownPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        render(
            &self.ir,
            self.base.as_deref(),
            &self.math,
            &self.images,
            self.wrap_code,
            &self.note_font,
            self.note_size,
            self.frame.clone(),
            cx,
        )
    }
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
                .pt(px(2.))
                .line_height(relative(1.2))
                .child(selectable_styled_text(text, &heading_style))
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

            let text_style = TextStyle {
                color: theme.foreground,
                font_family: SharedString::from(CODE_FAMILY),
                font_size: px((context.body_size - 2.0).max(10.0)).into(),
                ..Default::default()
            };
            let spans =
                crate::highlight::code_spans(language.as_deref(), &code_text, theme.is_dark());
            let spans = flatten_highlights(&code_text, spans.as_ref().clone());
            let styled = selectable_styled_text(
                StyledContent {
                    text: code_text.clone(),
                    style: text_style,
                    highlights: spans,
                },
                style,
            );
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
        Block::Image {
            src,
            alt,
            width,
            height,
            attrs,
            ..
        } => render_image(src, alt, *width, *height, attrs, true, context),
        Block::Div { text, attrs, .. } => render_text_box(text, attrs, true, context),
        Block::Rule { .. } => div()
            .w_full()
            .h(px(1.))
            .bg(theme.border)
            .my(px(6.))
            .into_any_element(),
        Block::Blank { lines, .. } => {
            let line_h = context.body_size * 1.35;
            div()
                .w_full()
                .h(px(line_h * (*lines as f32)))
                .into_any_element()
        }
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
    let has_image = inlines
        .iter()
        .any(|inline| matches!(inline, Inline::Image { .. }));
    // 含图片时按软换行纵向堆叠，避免横向 flex + items_center 把矮文本
    // 与高图垂直居中，看起来像「中间空出一大截」。
    if has_image {
        return render_paragraph_stack(inlines, context);
    }

    let needs_split = inlines.iter().any(|inline| {
        matches!(
            inline,
            Inline::Math { display: false, .. } | Inline::Span { .. }
        )
    });
    if !needs_split {
        return styled_text(inlines, &context.style);
    }

    render_inline_embeds_row(inlines, context)
}

/// 把段落按 Soft/HardBreak 切成多行，再纵向排列；图片单独占满一行。
fn render_paragraph_stack(inlines: &[Inline], context: &RenderCtx<'_>) -> AnyElement {
    let mut column = div().w_full().v_flex().gap_1();
    let mut line: Vec<Inline> = Vec::new();
    for inline in inlines {
        match inline {
            Inline::SoftBreak | Inline::HardBreak => {
                if !line.is_empty() {
                    column = column.child(render_paragraph_line(&line, context));
                    line.clear();
                }
            }
            other => line.push(other.clone()),
        }
    }
    if !line.is_empty() {
        column = column.child(render_paragraph_line(&line, context));
    }
    column.into_any_element()
}

fn render_paragraph_line(inlines: &[Inline], context: &RenderCtx<'_>) -> AnyElement {
    match inlines {
        [
            Inline::Image {
                src,
                alt,
                width,
                height,
                attrs,
                ..
            },
        ] => render_image(src, alt, *width, *height, attrs, true, context),
        _ if inlines
            .iter()
            .any(|inline| matches!(inline, Inline::Image { .. })) =>
        {
            // 同一软换行行内混有文字与图片：图片仍独占视觉行，纵向排。
            let mut column = div().w_full().v_flex().gap_1();
            let mut text_run: Vec<Inline> = Vec::new();
            for inline in inlines {
                match inline {
                    Inline::Image {
                        src,
                        alt,
                        width,
                        height,
                        attrs,
                        ..
                    } => {
                        if !text_run.is_empty() {
                            column = column.child(render_inline_embeds_row(&text_run, context));
                            text_run.clear();
                        }
                        column = column.child(render_image(
                            src, alt, *width, *height, attrs, true, context,
                        ));
                    }
                    other => text_run.push(other.clone()),
                }
            }
            if !text_run.is_empty() {
                column = column.child(render_inline_embeds_row(&text_run, context));
            }
            column.into_any_element()
        }
        _ => render_inline_embeds_row(inlines, context),
    }
}

fn render_inline_embeds_row(inlines: &[Inline], context: &RenderCtx<'_>) -> AnyElement {
    let needs_split = inlines.iter().any(|inline| {
        matches!(
            inline,
            Inline::Math { display: false, .. } | Inline::Span { .. } | Inline::Image { .. }
        )
    });
    if !needs_split {
        return styled_text(inlines, &context.style);
    }

    let mut row = div().w_full().flex().flex_wrap().items_baseline().gap_1();
    let mut start = 0;
    let mut index = 0;
    while index < inlines.len() {
        match &inlines[index] {
            Inline::Math {
                tex,
                display: false,
            } => {
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
            }
            Inline::Image {
                src,
                alt,
                width,
                height,
                attrs,
                ..
            } => {
                if start < index {
                    row = row.child(styled_text(&inlines[start..index], &context.style));
                }
                row = row.child(render_image(
                    src, alt, *width, *height, attrs, false, context,
                ));
                start = index + 1;
            }
            Inline::Span { text, attrs } => {
                if start < index {
                    row = row.child(styled_text(&inlines[start..index], &context.style));
                }
                row = row.child(render_text_box(text, attrs, false, context));
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if start < inlines.len() {
        row = row.child(styled_text(&inlines[start..], &context.style));
    }
    row.into_any_element()
}

fn render_image(
    src: &str,
    alt: &[Inline],
    width: Option<Length>,
    height: Option<Length>,
    attrs: &ElementAttrs,
    _block: bool,
    context: &RenderCtx<'_>,
) -> AnyElement {
    let img_tag = mdwork_render::html_subset::ImgTag {
        src: src.to_string(),
        alt: plain_inlines(alt),
        title: None,
        width,
        height,
        attrs: attrs.clone(),
    };
    let (_, width, height) = resolve_img_size(context.styles, &img_tag);
    let label = if alt.is_empty() {
        src.to_string()
    } else {
        format!("{} · {src}", plain_inlines(alt))
    };
    let placeholder = div()
        .rounded_md()
        .border_1()
        .border_color(context.border)
        .px_3()
        .py_2()
        .text_sm()
        .text_color(context.style.muted)
        .child(label);
    match context
        .images
        .get(&image_key(src, context.base))
        .and_then(|image| image.clone())
    {
        Some(resolved) => {
            let natural = (resolved.width > 0 && resolved.height > 0)
                .then_some((resolved.width as f32, resolved.height as f32));
            sized_image(
                img(resolved.path.clone()).rounded_md(),
                width,
                height,
                natural,
            )
        }
        None => sized_box(placeholder, width, height),
    }
}

/// 左对齐。
///
/// GPUI `Img` 在宽/高为 `Auto` 时会写成**原图像素绝对尺寸**；之后即便有
/// `max_w` / `Contain`，高度仍按原图像素占位，看起来就是大片空白。
/// 有自然尺寸时：外层 `div` 负责宽约束 + `aspect_ratio`，内层 `Img` 用
/// `w_full().h_full()` 填满（Relative 不会被改写成原图像素）。
fn sized_image(
    image: Img,
    width: Option<Length>,
    height: Option<Length>,
    natural: Option<(f32, f32)>,
) -> AnyElement {
    let image = image.object_fit(ObjectFit::Contain);
    let ratio = natural.and_then(|(w, h)| (w > 0.0 && h > 0.0).then_some(w / h));

    match (width, height, ratio) {
        (Some(Length::Px(w)), Some(Length::Px(h)), _) => image
            .w(gpui_kit::px(w as f32))
            .h(gpui_kit::px(h as f32))
            .max_w_full()
            .into_any_element(),
        // 绝对宽度：Img 只在绝对宽度时才会按比例推算高度
        (Some(Length::Px(w)), _, _) => image
            .w(gpui_kit::px(w as f32))
            .max_w_full()
            .into_any_element(),
        (Some(Length::PercentCents(cents)), Some(Length::Px(h)), Some(ratio)) => div()
            .w(relative((cents as f32 / 10_000.0).clamp(0.01, 1.0)))
            .h(gpui_kit::px(h as f32))
            .aspect_ratio(ratio)
            .child(image.w_full().h_full())
            .into_any_element(),
        (Some(Length::PercentCents(cents)), _, Some(ratio)) => div()
            .w(relative((cents as f32 / 10_000.0).clamp(0.01, 1.0)))
            .aspect_ratio(ratio)
            .child(image.w_full().h_full())
            .into_any_element(),
        (None, Some(Length::Px(h)), Some(ratio)) => div()
            .h(gpui_kit::px(h as f32))
            .max_w_full()
            .aspect_ratio(ratio)
            .child(image.w_full().h_full())
            .into_any_element(),
        (None, Some(Length::PercentCents(cents)), Some(ratio)) => div()
            .max_h(relative((cents as f32 / 10_000.0).clamp(0.01, 1.0)))
            .max_w_full()
            .aspect_ratio(ratio)
            .child(image.w_full().h_full())
            .into_any_element(),
        // 未指定尺寸：按原图宽，但不超过父级；高度由 aspect_ratio 随 used width 缩放
        (None, None, Some(ratio)) => div()
            .w(gpui_kit::px(natural.map(|(w, _)| w).unwrap_or(1.0)))
            .max_w_full()
            .aspect_ratio(ratio)
            .child(image.w_full().h_full())
            .into_any_element(),
        // 尚无自然尺寸时的降级（加载中 / SVG 等）
        (Some(Length::PercentCents(cents)), Some(Length::Px(h)), None) => image
            .max_w(relative((cents as f32 / 10_000.0).clamp(0.01, 1.0)))
            .h(gpui_kit::px(h as f32))
            .into_any_element(),
        (Some(Length::PercentCents(cents)), _, None) => image
            .max_w(relative((cents as f32 / 10_000.0).clamp(0.01, 1.0)))
            .into_any_element(),
        (None, Some(Length::Px(h)), None) => image
            .h(gpui_kit::px(h as f32))
            .max_w_full()
            .into_any_element(),
        (None, Some(Length::PercentCents(cents)), None) => image
            .max_h(relative((cents as f32 / 10_000.0).clamp(0.01, 1.0)))
            .max_w_full()
            .into_any_element(),
        (None, None, None) => image.max_w_full().into_any_element(),
    }
}

fn sized_box(el: Div, width: Option<Length>, height: Option<Length>) -> AnyElement {
    apply_box_length(el, width, height).into_any_element()
}

fn render_text_box(
    text: &str,
    attrs: &ElementAttrs,
    block: bool,
    context: &RenderCtx<'_>,
) -> AnyElement {
    let style = context
        .styles
        .resolve(attrs.id.as_deref(), &attrs.class, &attrs.style);
    let mut el = div().child(text.to_string());
    if block {
        el = el.w_full();
    }
    el = apply_text_style(el, &style, context);
    el = apply_box_length(el, style.width, style.height);
    el.into_any_element()
}

fn apply_text_style<E: Styled>(mut el: E, style: &StyleDecls, context: &RenderCtx<'_>) -> E {
    if let Some(color) = style.color {
        el = el.text_color(rgb_to_hsla(color));
    } else {
        el = el.text_color(context.style.foreground);
    }
    if let Some(bg) = style.background_color {
        el = el.bg(rgb_to_hsla(bg)).rounded_md().px_1();
    }
    match style.font_size {
        Some(Length::Px(px)) => el = el.text_size(gpui_kit::px(px as f32)),
        Some(Length::PercentCents(cents)) => {
            let size = context.body_size * (cents as f32 / 10_000.0);
            el = el.text_size(gpui_kit::px(size.max(1.0)));
        }
        None => {}
    }
    el
}

fn apply_box_length<E: Styled>(mut el: E, width: Option<Length>, height: Option<Length>) -> E {
    match width {
        Some(Length::Px(px)) => el = el.w(gpui_kit::px(px as f32)).max_w_full(),
        Some(Length::PercentCents(cents)) => {
            el = el
                .w(relative((cents as f32 / 10_000.0).clamp(0.01, 1.0)))
                .max_w_full();
        }
        None => {}
    }
    match height {
        Some(Length::Px(px)) => el = el.h(gpui_kit::px(px as f32)),
        Some(Length::PercentCents(cents)) => {
            el = el.h(relative((cents as f32 / 10_000.0).clamp(0.01, 1.0)));
        }
        None => {}
    }
    el
}

fn rgb_to_hsla(color: RgbColor) -> Hsla {
    gpui_kit::rgb(((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32)).into()
}

fn styled_text(inlines: &[Inline], style: &InlineStyle) -> AnyElement {
    let text = styled_inlines(inlines, style, FontWeight::NORMAL);
    selectable_styled_text(text, style)
}

/// 把富文本片段挂到整篇预览共用的选区帧上，使预览文本
/// 可以被拖选、高亮并 Ctrl+C 复制。`document_order` 保证
/// 复制顺序稳定；片段本身不再注册窗口选区参与者。
fn selectable_styled_text(text: StyledContent, style: &InlineStyle) -> AnyElement {
    let styled =
        StyledText::new(text.text.clone()).with_default_highlights(&text.style, text.highlights);
    let Some((counter, frame)) = style.order.as_ref().zip(style.selection.as_ref()) else {
        return styled.into_any_element();
    };
    let order = counter.get();
    counter.set(order.saturating_add(1));
    crate::preview_selection::SelectableStyledText::new(
        ElementId::Name(format!("preview-text-{order}").into()),
        text.text,
        styled,
        order,
        frame.clone(),
    )
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
        let mut item_blocks = item.blocks.as_slice();
        while let Some((blank @ Block::Blank { .. }, tail)) = item_blocks.split_first() {
            column = column.child(render_block(blank, context, cx));
            item_blocks = tail;
        }
        let marker = match item.task {
            Some(true) => "☑".to_string(),
            Some(false) => "☐".to_string(),
            None if ordered => format!("{}.", start + index),
            None => "•".to_string(),
        };
        let mut content = div().v_flex().flex_1().min_w_0().gap_1();
        for block in item_blocks {
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
            .child(selectable_styled_text(text, style));
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

    let text_style = TextStyle {
        color: style.foreground,
        font_family: style.family.clone(),
        font_size: px(style.size).into(),
        line_height: relative(1.35),
        font_weight: weight,
        ..Default::default()
    };
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
            Inline::Code(code) => out.push_str(code),
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
            Inline::Span { text, .. } => {
                out.push_str(text);
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
            Inline::Span { text, .. } => out.push_str(text),
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
        order: style.order.clone(),
        selection: style.selection.clone(),
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

/// Resolves path + natural pixel size on a background thread.
///
/// Dimension probe failure (e.g. SVG) still yields a usable path with `0×0`,
/// so the image can paint; layout then falls back without aspect-ratio.
pub fn resolve_preview_image(src: &str, base: Option<&Path>) -> Option<PreviewImage> {
    let path = resolve_image_path(src, base).filter(|path| path.is_file())?;
    let (width, height) = imagesize::size(&path)
        .ok()
        .map(|size| (size.width as u32, size.height as u32))
        .unwrap_or((0, 0));
    Some(PreviewImage {
        path,
        width,
        height,
    })
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
/// render thread. Includes both standalone figures and images nested inside
/// paragraphs, headings, and table cells (CommonMark keeps adjacent image
/// lines in one paragraph when text precedes them).
pub fn collect_images(ir: &PreviewIr, visit: &mut impl FnMut(&str)) {
    fn walk_inline(inlines: &[Inline], visit: &mut impl FnMut(&str)) {
        for inline in inlines {
            match inline {
                Inline::Image { src, alt, .. } => {
                    visit(src);
                    walk_inline(alt, visit);
                }
                Inline::Emphasis(children)
                | Inline::Strong(children)
                | Inline::Strike(children)
                | Inline::Link { children, .. } => walk_inline(children, visit),
                _ => {}
            }
        }
    }

    fn walk_block(block: &Block, visit: &mut impl FnMut(&str)) {
        match block {
            Block::Image { src, .. } => visit(src),
            Block::Heading { content, .. } | Block::Paragraph { content, .. } => {
                walk_inline(content, visit);
            }
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
            order: None,
            selection: None,
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
