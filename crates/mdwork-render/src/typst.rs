//! Markdown AST → Typst source for PDF/PNG export.
//!
//! The renderer keeps a mapping from generated Typst lines back to Markdown
//! lines so compile diagnostics can point at the note.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::html_subset::{ElementAttrs, Length, RgbColor, StyleDecls, StyleSheet};
use crate::preview::{Align, Block, Inline, PreviewIr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaperSize {
    A4,
    A5,
    Letter,
    Custom,
}

impl PaperSize {
    pub fn width_mm(self, options: &ExportOptions) -> f32 {
        match self {
            PaperSize::A4 => 210.0,
            PaperSize::A5 => 148.0,
            PaperSize::Letter => 215.9,
            PaperSize::Custom => options.custom_width_mm.clamp(50.0, 2000.0),
        }
    }

    pub fn height_mm(self, options: &ExportOptions) -> f32 {
        match self {
            PaperSize::A4 => 297.0,
            PaperSize::A5 => 210.0,
            PaperSize::Letter => 279.4,
            PaperSize::Custom => options.custom_height_mm.clamp(50.0, 2000.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PaperSize::A4 => "A4",
            PaperSize::A5 => "A5",
            PaperSize::Letter => "Letter",
            PaperSize::Custom => "Custom",
        }
    }

    pub fn next(self) -> Self {
        match self {
            PaperSize::A4 => PaperSize::A5,
            PaperSize::A5 => PaperSize::Letter,
            PaperSize::Letter => PaperSize::Custom,
            PaperSize::Custom => PaperSize::A4,
        }
    }
}

/// How wide an exported table is.
///
/// Markdown does not specify table width. [`TableWidth::Content`] keeps each
/// column as narrow as its text. [`TableWidth::Full`] stretches the table
/// across the text width, with columns sharing that width equally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TableWidth {
    /// Size each column to its contents.
    #[default]
    Content,
    /// Stretch the table to the full text width.
    Full,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ExportOptions {
    pub paper: PaperSize,
    pub custom_width_mm: f32,
    pub custom_height_mm: f32,
    pub margin_top_mm: f32,
    pub margin_bottom_mm: f32,
    pub margin_left_mm: f32,
    pub margin_right_mm: f32,
    pub cover: bool,
    pub toc: bool,
    pub toc_depth: u8,
    pub page_numbers: bool,
    pub page_number_format: String,
    pub header: String,
    pub footer: String,
    pub font: String,
    pub font_size_pt: f32,
    pub line_height: f32,
    pub code_font: String,
    pub code_size_pt: f32,
    pub code_wrap: bool,
    pub code_highlight: bool,
    pub image_max_width_pct: u8,
    pub justify: bool,
    pub table_width: TableWidth,
    pub png_dpi: u32,
    pub png_page_files: bool,
    pub author: String,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            paper: PaperSize::A4,
            custom_width_mm: 210.0,
            custom_height_mm: 297.0,
            margin_top_mm: 20.0,
            margin_bottom_mm: 20.0,
            margin_left_mm: 20.0,
            margin_right_mm: 20.0,
            cover: false,
            toc: false,
            toc_depth: 3,
            page_numbers: true,
            page_number_format: "1 / 1".to_string(),
            header: String::new(),
            footer: String::new(),
            font: String::new(),
            font_size_pt: 11.0,
            line_height: 1.65,
            code_font: String::new(),
            code_size_pt: 9.0,
            code_wrap: true,
            code_highlight: true,
            image_max_width_pct: 100,
            justify: true,
            table_width: TableWidth::Content,
            png_dpi: 144,
            png_page_files: false,
            author: String::new(),
        }
    }
}

impl ExportOptions {
    pub fn content_width_pt(&self) -> f32 {
        let mm = self.paper.width_mm(self)
            - self.margin_left_mm.clamp(0.0, 80.0)
            - self.margin_right_mm.clamp(0.0, 80.0);
        (mm.max(20.0)) * 72.0 / 25.4
    }

    pub fn content_height_pt(&self) -> f32 {
        let mm = self.paper.height_mm(self)
            - self.margin_top_mm.clamp(0.0, 80.0)
            - self.margin_bottom_mm.clamp(0.0, 80.0);
        (mm.max(20.0)) * 72.0 / 25.4
    }

    pub fn page_width_pt(&self) -> f32 {
        self.paper.width_mm(self) * 72.0 / 25.4
    }

    pub fn page_height_pt(&self) -> f32 {
        self.paper.height_mm(self) * 72.0 / 25.4
    }
}

#[derive(Debug, Clone, Default)]
pub struct DocMeta {
    pub title: String,
    pub notebook: String,
    pub note_rel: String,
    pub date: String,
}

#[derive(Debug)]
pub struct Asset {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct TypstDoc {
    pub source: String,
    /// `(typst_line, markdown_line)` pairs, both one-based.
    pub line_map: Vec<(usize, usize)>,
    pub assets: Vec<Asset>,
    pub warnings: Vec<String>,
}

impl TypstDoc {
    /// Maps a one-based Typst line back to a one-based Markdown line.
    pub fn markdown_line(&self, typst_line: usize) -> Option<usize> {
        let mut mapped = None;
        for (typst, markdown) in &self.line_map {
            if *typst > typst_line {
                break;
            }
            mapped = Some((*typst, *markdown));
        }
        mapped.map(|(typst, markdown)| markdown + typst_line.saturating_sub(typst))
    }
}

const TEXT_FONT_FALLBACKS: &[&str] = &[
    "Segoe UI",
    "Microsoft YaHei",
    "Noto Sans SC",
    "Noto Sans CJK SC",
    "SimSun",
];

const CODE_FONT_FALLBACKS: &[&str] = &[
    "Cascadia Mono",
    "Consolas",
    "JetBrains Mono",
    "Courier New",
    "DejaVu Sans Mono",
];

/// Renders the Markdown preview IR into Typst source and collects local images.
pub fn convert(
    ir: &PreviewIr,
    options: &ExportOptions,
    meta: &DocMeta,
    notebook_root: &Path,
) -> TypstDoc {
    convert_with(ir, options, meta, notebook_root, false)
}

/// Like [`convert`], with `continuous_page` forcing `height: auto` for long raster exports.
pub fn convert_with(
    ir: &PreviewIr,
    options: &ExportOptions,
    meta: &DocMeta,
    notebook_root: &Path,
    continuous_page: bool,
) -> TypstDoc {
    let mut emitter = Emitter::new(options, meta, notebook_root, &ir.styles, continuous_page);
    emitter.preamble(ir);
    emitter.body(ir);
    emitter.finish()
}

struct Emitter<'a> {
    options: &'a ExportOptions,
    meta: &'a DocMeta,
    root: &'a Path,
    styles: &'a StyleSheet,
    continuous_page: bool,
    out: String,
    line: usize,
    line_map: Vec<(usize, usize)>,
    assets: Vec<Asset>,
    warnings: Vec<String>,
    footnotes: HashMap<String, Vec<Block>>,
    used_footnotes: BTreeSet<String>,
    next_image: usize,
}

impl<'a> Emitter<'a> {
    fn new(
        options: &'a ExportOptions,
        meta: &'a DocMeta,
        root: &'a Path,
        styles: &'a StyleSheet,
        continuous_page: bool,
    ) -> Self {
        Self {
            options,
            meta,
            root,
            styles,
            continuous_page,
            out: String::new(),
            line: 0,
            line_map: Vec::new(),
            assets: Vec::new(),
            warnings: Vec::new(),
            footnotes: HashMap::new(),
            used_footnotes: BTreeSet::new(),
            next_image: 0,
        }
    }

    fn finish(self) -> TypstDoc {
        TypstDoc {
            source: self.out,
            line_map: self.line_map,
            assets: self.assets,
            warnings: self.warnings,
        }
    }

    fn push(&mut self, text: &str) {
        self.line += text.bytes().filter(|byte| *byte == b'\n').count();
        self.out.push_str(text);
    }

    fn line(&mut self, text: &str) {
        self.push(text);
        self.push("\n");
    }

    fn next_line(&self) -> usize {
        self.line + 1
    }

    fn preamble(&mut self, ir: &PreviewIr) {
        self.footnotes = HashMap::new();
        collect_footnotes(ir, &mut self.footnotes);

        let options = self.options;
        let title = if self.meta.title.trim().is_empty() {
            "未命名笔记"
        } else {
            self.meta.title.trim()
        };

        let mut setup = String::new();
        setup.push_str(&format!("#set document(title: {})\n", quote_str(title)));
        if !options.author.trim().is_empty() {
            setup.push_str(&format!(
                "#set document(author: {})\n",
                quote_str(options.author.trim())
            ));
        }

        let paper = if self.continuous_page {
            // Long PNG/WebP: one continuous page at paper width (no A4 pagination).
            format!("width: {}mm, height: auto", options.paper.width_mm(options))
        } else {
            match options.paper {
                PaperSize::A4 => "paper: \"a4\"".to_string(),
                PaperSize::A5 => "paper: \"a5\"".to_string(),
                PaperSize::Letter => "paper: \"us-letter\"".to_string(),
                PaperSize::Custom => format!(
                    "width: {}mm, height: {}mm",
                    options.paper.width_mm(options),
                    options.paper.height_mm(options)
                ),
            }
        };
        let numbering = if options.page_numbers {
            let format = if options.page_number_format.trim().is_empty() {
                "1"
            } else {
                options.page_number_format.trim()
            };
            format!("numbering: {}", quote_str(format))
        } else {
            "numbering: none".to_string()
        };
        let header = template_setup(&options.header, self.meta, Align::Right);
        let footer = template_setup(&options.footer, self.meta, Align::Center);
        let header_arg = header
            .map(|content| format!("header: {content},"))
            .unwrap_or_default();
        let footer_arg = footer
            .map(|content| format!("footer: {content},"))
            .unwrap_or_default();

        setup.push_str(&format!(
            "#set page(\n  {paper},\n  margin: (top: {}mm, bottom: {}mm, left: {}mm, right: {}mm),\n  {numbering},\n  {header_arg}\n  {footer_arg}\n)\n",
            options.margin_top_mm.clamp(0.0, 80.0),
            options.margin_bottom_mm.clamp(0.0, 80.0),
            options.margin_left_mm.clamp(0.0, 80.0),
            options.margin_right_mm.clamp(0.0, 80.0),
        ));

        setup.push_str(&format!(
            "#set text(font: {}, size: {}pt, lang: \"zh\", region: \"cn\")\n",
            font_list(&options.font, TEXT_FONT_FALLBACKS),
            options.font_size_pt.clamp(6.0, 30.0),
        ));
        let leading = (options.line_height.clamp(1.0, 3.0) - 1.0) * options.font_size_pt;
        setup.push_str(&format!(
            "#set par(justify: {}, leading: {}pt)\n",
            if options.justify { "true" } else { "false" },
            leading.max(0.0)
        ));
        setup.push_str("#set heading(numbering: none)\n");
        setup.push_str("#show heading: set block(above: 1.35em, below: 0.65em)\n");
        setup.push_str("#show heading.where(level: 1): set text(size: 20pt, weight: \"bold\")\n");
        setup.push_str("#show heading.where(level: 2): set text(size: 16pt, weight: \"bold\")\n");
        setup.push_str("#show heading.where(level: 3): set text(size: 14pt, weight: \"bold\")\n");
        setup.push_str("#show link: set text(fill: rgb(\"#1f5fbf\"))\n");
        setup.push_str("#set raw(tab-size: 4)\n");
        setup.push_str(&format!(
            "#set raw(theme: {})\n",
            if options.code_highlight {
                "auto"
            } else {
                "none"
            }
        ));
        setup.push_str(&format!(
            "#show raw: set text(font: {})\n",
            font_list(&options.code_font, CODE_FONT_FALLBACKS)
        ));
        setup.push_str(&format!(
            "#show raw.where(block: true): it => block(fill: luma(246), inset: 8pt, radius: 4pt, width: 100%, above: 0.7em, below: 0.7em, text(size: {}pt, it))\n",
            options.code_size_pt.clamp(5.0, 20.0)
        ));
        setup.push_str("#show raw.where(block: false): it => box(fill: luma(240), inset: (x: 3pt, y: 0pt), outset: (y: 2pt), radius: 2pt, it)\n");
        setup.push_str("#set list(indent: 1.2em, body-indent: 0.5em)\n");
        setup.push_str("#set enum(indent: 1.2em, body-indent: 0.5em)\n");
        setup.push_str("#set table(inset: 6pt, stroke: 0.5pt + luma(190))\n");
        setup.push_str("#set figure(gap: 0.8em)\n");
        setup.push('\n');

        self.push(&setup);

        if self.options.cover {
            let notebook = if self.meta.notebook.trim().is_empty() {
                String::new()
            } else {
                self.meta.notebook.trim().to_string()
            };
            let date = self.meta.date.trim().to_string();
            let mut cover = String::from("#page(header: none, footer: none, numbering: none)[\n");
            cover.push_str("  #v(18%)\n");
            cover.push_str(&format!(
                "  #align(center)[#text(size: 26pt, weight: \"bold\")[{}]]\n",
                escape_text(title)
            ));
            if !notebook.is_empty() {
                cover.push_str("  #v(1.2em)\n");
                cover.push_str(&format!(
                    "  #align(center)[#text(size: 13pt, fill: luma(90))[{}]]\n",
                    escape_text(&notebook)
                ));
            }
            if !date.is_empty() {
                cover.push_str("  #v(0.6em)\n");
                cover.push_str(&format!(
                    "  #align(center)[#text(size: 11pt, fill: luma(130))[{}]]\n",
                    escape_text(&date)
                ));
            }
            cover.push_str("]\n#pagebreak()\n");
            self.line(&cover);
        }

        if self.options.toc {
            let depth = self.options.toc_depth.clamp(1, 6);
            self.line(&format!("#outline(title: [目录], depth: {depth})"));
            self.line("#pagebreak()");
        }
    }

    fn body(&mut self, ir: &PreviewIr) {
        let mut extra_before = 0u32;
        for block in &ir.blocks {
            if matches!(block, Block::FootnoteDefinition { .. }) {
                continue;
            }
            if let Block::Blank { lines, .. } = block {
                extra_before += lines;
                continue;
            }
            let markup = self.block(block);
            if markup.trim().is_empty() {
                continue;
            }
            if extra_before > 0 {
                // 跟在上一段的段落分隔之后，避免再被空行拆成独立段落把间距加倍。
                self.line(&self.blank_space(extra_before));
                extra_before = 0;
            }
            let line = block_line(block);
            self.line_map.push((self.next_line(), line));
            self.line(&markup);
            self.line("");
        }
        if extra_before > 0 {
            self.line(&self.blank_space(extra_before));
        }

        let unused: Vec<String> = self
            .footnotes
            .keys()
            .filter(|label| !self.used_footnotes.contains(*label))
            .cloned()
            .collect();
        if unused.is_empty() {
            return;
        }
        let mut markup = String::new();
        for label in unused {
            let content = self.footnote_content(&label);
            markup.push_str(&format!("#footnote[{}]\u{200b}", content));
        }
        if !markup.trim().is_empty() {
            self.line(&markup);
        }
    }

    fn block(&mut self, block: &Block) -> String {
        match block {
            Block::Heading { level, content, .. } => {
                let level = (*level).clamp(1, 6) as usize;
                format!("{} {}", "=".repeat(level), self.inlines(content))
            }
            Block::Paragraph { content, .. } => self.inlines(content),
            Block::CodeBlock { language, text, .. } => self.code_block(language.as_deref(), text),
            Block::List {
                ordered,
                start,
                items,
                ..
            } => self.list(*ordered, *start, items),
            Block::Quote { blocks, .. } => quote_block(&self.blocks_markup(blocks)),
            Block::Table {
                head, rows, aligns, ..
            } => self.table(head, rows, aligns),
            Block::Image {
                src,
                title,
                alt,
                width,
                height,
                attrs,
                ..
            } => self.block_image(src, title.as_deref(), alt, *width, *height, attrs),
            Block::Div { text, attrs, .. } => self.styled_text_box(text, attrs, true),
            Block::Rule { .. } => "#line(length: 100%, stroke: 0.5pt + luma(185))".to_string(),
            // 与预览同一约定：`lines` 是分隔空行之外的额外行数，不折叠。
            Block::Blank { lines, .. } => self.blank_space(*lines),
            Block::Math { tex, .. } => self.math(tex, true),
            Block::FootnoteDefinition { .. } => String::new(),
        }
    }

    fn blocks_markup(&mut self, blocks: &[Block]) -> String {
        let mut parts = Vec::new();
        let mut extra_before = 0u32;
        for block in blocks {
            if matches!(block, Block::FootnoteDefinition { .. }) {
                continue;
            }
            if let Block::Blank { lines, .. } = block {
                extra_before += lines;
                continue;
            }
            let mut markup = self.block(block);
            if markup.trim().is_empty() {
                continue;
            }
            if extra_before > 0 {
                markup = format!("{}\n{markup}", self.blank_space(extra_before));
                extra_before = 0;
            }
            parts.push(markup);
        }
        if extra_before > 0 {
            parts.push(self.blank_space(extra_before));
        }
        parts.join("\n\n")
    }

    /// 一行额外空行 = 当前正文行距。`weak: false` 让它不会被段落间距吃掉。
    fn blank_space(&self, lines: u32) -> String {
        let em = (lines as f32) * self.options.line_height.clamp(1.0, 3.0);
        let rounded = (em * 100.0).round() / 100.0;
        let text = format!("{rounded:.2}");
        let text = text.trim_end_matches('0').trim_end_matches('.');
        format!("#v({text}em, weak: false)")
    }

    fn list(&mut self, ordered: bool, start: usize, items: &[crate::preview::ListItem]) -> String {
        let all_tasks = !items.is_empty() && items.iter().all(|item| item.task.is_some());
        let mut bodies = Vec::new();
        for item in items {
            let mut body = self.blocks_markup(&item.blocks);
            if let Some(task) = item.task {
                let marker = if task { "☑" } else { "☐" };
                body = if body.is_empty() {
                    marker.to_string()
                } else {
                    format!("{marker} {body}")
                };
            }
            bodies.push(format!("[{body}]"));
        }
        let joined = bodies.join(", ");
        if ordered {
            format!("#enum(start: {}, {joined})", start.max(1))
        } else if all_tasks {
            format!("#list(marker: none, indent: 0em, body-indent: 1.2em, {joined})")
        } else {
            format!("#list({joined})")
        }
    }

    fn table(
        &mut self,
        head: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
        aligns: &[Align],
    ) -> String {
        let columns = head
            .len()
            .max(rows.iter().map(Vec::len).max().unwrap_or(0))
            .max(1);
        let align_items: Vec<&str> = (0..columns)
            .map(
                |index| match aligns.get(index).copied().unwrap_or(Align::None) {
                    Align::Left => "left",
                    Align::Center => "center",
                    Align::Right => "right",
                    Align::None => "auto",
                },
            )
            .collect();
        let align = format!("({})", align_items.join(", "));

        let mut cells: Vec<String> = Vec::new();
        if !head.is_empty() {
            let mut header_cells = Vec::new();
            for index in 0..columns {
                let cell = head
                    .get(index)
                    .map(|cell| self.inlines(cell))
                    .unwrap_or_default();
                header_cells.push(format!("[#strong[{cell}]]"));
            }
            cells.push(format!("table.header({})", header_cells.join(", ")));
        }
        for row in rows {
            for index in 0..columns {
                let cell = row
                    .get(index)
                    .map(|cell| self.inlines(cell))
                    .unwrap_or_default();
                cells.push(format!("[{cell}]"));
            }
        }
        let columns = table_column_spec(columns, self.options.table_width);
        format!(
            "#table(columns: {columns}, align: {align}, {})",
            cells.join(", ")
        )
    }

    fn block_image(
        &mut self,
        src: &str,
        title: Option<&str>,
        alt: &[Inline],
        width: Option<Length>,
        height: Option<Length>,
        attrs: &ElementAttrs,
    ) -> String {
        let resolved = self
            .styles
            .resolve(attrs.id.as_deref(), &attrs.class, &attrs.style);
        let width = width.or(resolved.width);
        let height = height.or(resolved.height);
        match self.image_asset(src) {
            Some((name, _)) => {
                let width_arg = typst_image_width(width, height, self.options);
                let caption = title.map(str::trim).filter(|title| !title.is_empty());
                match caption {
                    Some(caption) => format!(
                        "#figure(image({}, {width_arg}), caption: [{}])",
                        quote_str(&name),
                        escape_text(caption)
                    ),
                    None => format!("#align(center)[#image({}, {width_arg})]", quote_str(&name)),
                }
            }
            None => {
                let label = if !alt.is_empty() {
                    plain_inlines(alt)
                } else {
                    src.to_string()
                };
                format!(
                    "#block(stroke: 0.5pt + luma(170), inset: 8pt, radius: 3pt, width: 100%, align(center)[#text(fill: luma(110))[图片缺失：{}]])",
                    escape_text(&label)
                )
            }
        }
    }

    fn styled_text_box(&self, text: &str, attrs: &ElementAttrs, block: bool) -> String {
        let style = self
            .styles
            .resolve(attrs.id.as_deref(), &attrs.class, &attrs.style);
        wrap_styled_typst(&escape_text(text), &style, block)
    }

    fn image_asset(&mut self, src: &str) -> Option<(String, ())> {
        let src = src.trim();
        if src.is_empty()
            || src.starts_with("http://")
            || src.starts_with("https://")
            || src.starts_with("data:")
        {
            return None;
        }
        let decoded = percent_decode(src);
        let raw = PathBuf::from(&decoded);
        let absolute = raw.is_absolute();
        let note_dir = Path::new(&self.meta.note_rel)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        let candidates = if absolute {
            vec![raw]
        } else {
            vec![self.root.join(note_dir).join(&raw), self.root.join(&raw)]
        };
        for candidate in candidates {
            let Ok(path) = fs::canonicalize(&candidate) else {
                continue;
            };
            if !path.is_file() {
                continue;
            }
            if !absolute && !path_under(self.root, &path) {
                continue;
            }
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let extension = match extension.as_str() {
                "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => extension,
                _ => continue,
            };
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            self.next_image += 1;
            let name = format!("mdwork-img/{}.{}", self.next_image, extension);
            self.assets.push(Asset {
                name: name.clone(),
                bytes,
            });
            return Some((name, ()));
        }
        None
    }

    fn code_block(&mut self, language: Option<&str>, text: &str) -> String {
        let mut code = text.replace("\r\n", "\n").replace('\r', "\n");
        code = code.trim_end_matches('\n').to_string();
        if self.options.code_wrap {
            let columns = code_columns(self.options);
            code = wrap_code(&code, columns);
        }
        let lang = language.map(str::trim).filter(|lang| {
            !lang.is_empty()
                && lang
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '_' | '#'))
        });
        match lang {
            Some(lang) => format!(
                "#raw({}, block: true, lang: {})",
                quote_str(&code),
                quote_str(lang)
            ),
            None => format!("#raw({}, block: true)", quote_str(&code)),
        }
    }

    fn math(&mut self, tex: &str, block: bool) -> String {
        match mitex::convert_math(tex.trim(), None) {
            Ok(body) => {
                let body = body.trim();
                if body.is_empty() {
                    return String::new();
                }
                if block {
                    format!("$ {body} $")
                } else {
                    format!("${body}$")
                }
            }
            Err(err) => {
                self.warnings.push(format!("公式转换失败：{err}\n{tex}"));
                format!(
                    "#block(stroke: 0.5pt + luma(200), inset: 6pt, width: 100%, raw({}, block: true))",
                    quote_str(&format!("$$ {tex} $$"))
                )
            }
        }
    }

    fn inlines(&mut self, inlines: &[Inline]) -> String {
        let mut out = String::new();
        for inline in inlines {
            out.push_str(&self.inline(inline));
        }
        out
    }

    fn inline(&mut self, inline: &Inline) -> String {
        match inline {
            Inline::Text(text) => escape_text(text),
            Inline::Code(code) => format!("#raw({})", quote_str(code)),
            Inline::Emphasis(children) => format!("#emph[{}]", self.inlines(children)),
            Inline::Strong(children) => format!("#strong[{}]", self.inlines(children)),
            Inline::Strike(children) => format!("#strike[{}]", self.inlines(children)),
            Inline::Link { url, children, .. } => {
                let text = if children.is_empty() {
                    escape_text(url)
                } else {
                    self.inlines(children)
                };
                format!("#link({})[{}]", quote_str(url), text)
            }
            Inline::Image {
                src,
                alt,
                title,
                width,
                height,
                attrs,
            } => {
                let resolved = self
                    .styles
                    .resolve(attrs.id.as_deref(), &attrs.class, &attrs.style);
                let width = width.or(resolved.width);
                let height = height.or(resolved.height);
                match self.image_asset(src) {
                    Some((name, ())) => {
                        if width.is_some() || height.is_some() {
                            let width_arg = typst_image_width(width, height, self.options);
                            format!("#image({}, {width_arg})", quote_str(&name))
                        } else {
                            format!("#image({}, height: 1.1em)", quote_str(&name))
                        }
                    }
                    None => {
                        let label = if !alt.is_empty() {
                            plain_inlines(alt)
                        } else {
                            src.clone()
                        };
                        let tooltip = title
                            .as_deref()
                            .map(str::trim)
                            .filter(|title| !title.is_empty())
                            .map(|title| format!("（{title}）"))
                            .unwrap_or_default();
                        format!(
                            "#text(fill: luma(120))[图片缺失:{}{}]",
                            escape_text(&label),
                            escape_text(&tooltip)
                        )
                    }
                }
            }
            Inline::Span { text, attrs } => self.styled_text_box(text, attrs, false),
            Inline::SoftBreak => " ".to_string(),
            Inline::HardBreak => " \\\n".to_string(),
            Inline::FootnoteRef(label) => {
                let content = self.footnote_content(label);
                format!("#footnote[{content}]")
            }
            Inline::Math { tex, .. } => {
                let markup = self.math(tex, false);
                if markup.is_empty() {
                    escape_text(tex)
                } else {
                    markup
                }
            }
        }
    }

    fn footnote_content(&mut self, label: &str) -> String {
        self.used_footnotes.insert(label.to_string());
        let Some(blocks) = self.footnotes.get(label).cloned() else {
            return escape_text(label);
        };
        self.blocks_markup(&blocks)
    }
}

fn block_line(block: &Block) -> usize {
    match block {
        Block::Heading { line, .. }
        | Block::Paragraph { line, .. }
        | Block::CodeBlock { line, .. }
        | Block::List { line, .. }
        | Block::Quote { line, .. }
        | Block::Table { line, .. }
        | Block::Image { line, .. }
        | Block::Div { line, .. }
        | Block::Rule { line }
        | Block::Blank { line, .. }
        | Block::Math { line, .. }
        | Block::FootnoteDefinition { line, .. } => *line,
    }
}

fn typst_image_width(
    width: Option<Length>,
    height: Option<Length>,
    options: &ExportOptions,
) -> String {
    match (width, height) {
        (Some(Length::Px(px)), _) => {
            // 导出按 96dpi 近似：1px ≈ 0.75pt
            format!("width: {}pt", (px as f32) * 0.75)
        }
        (Some(Length::PercentCents(cents)), _) => {
            let pct = (cents as f32 / 100.0).clamp(1.0, 100.0);
            format!("width: {pct}%")
        }
        (None, Some(Length::Px(px))) => format!("height: {}pt", (px as f32) * 0.75),
        (None, Some(Length::PercentCents(cents))) => {
            let pct = (cents as f32 / 100.0).clamp(1.0, 100.0);
            format!("height: {pct}%")
        }
        (None, None) => {
            let width = options.image_max_width_pct.clamp(10, 100);
            format!("width: {width}%")
        }
    }
}

fn wrap_styled_typst(content: &str, style: &StyleDecls, block: bool) -> String {
    let mut inner = content.to_string();
    if let Some(color) = style.color {
        inner = format!("#text(fill: {})[{inner}]", typst_rgb(color));
    }
    if let Some(Length::Px(px)) = style.font_size {
        inner = format!("#text(size: {px}pt)[{inner}]");
    } else if let Some(Length::PercentCents(cents)) = style.font_size {
        let em = (cents as f32 / 10_000.0).clamp(0.1, 10.0);
        inner = format!("#text(size: {em}em)[{inner}]");
    }
    let mut box_args = Vec::new();
    if let Some(width) = style.width {
        box_args.push(typst_length_arg("width", width));
    }
    if let Some(height) = style.height {
        box_args.push(typst_length_arg("height", height));
    }
    if let Some(bg) = style.background_color {
        box_args.push(format!("fill: {}", typst_rgb(bg)));
        box_args.push("inset: 4pt".to_string());
        box_args.push("radius: 2pt".to_string());
    }
    if !box_args.is_empty() {
        let kind = if block { "block" } else { "box" };
        inner = format!("#{kind}({}, [{inner}])", box_args.join(", "));
    } else if block {
        inner = format!("#block[{inner}]");
    }
    inner
}

fn typst_length_arg(name: &str, length: Length) -> String {
    match length {
        Length::Px(px) => format!("{name}: {}pt", (px as f32) * 0.75),
        Length::PercentCents(cents) => {
            let pct = (cents as f32 / 100.0).clamp(0.0, 100.0);
            format!("{name}: {pct}%")
        }
    }
}

fn typst_rgb(color: RgbColor) -> String {
    format!("rgb(\"#{:02x}{:02x}{:02x}\")", color.r, color.g, color.b)
}

fn collect_footnotes(ir: &PreviewIr, out: &mut HashMap<String, Vec<Block>>) {
    for block in &ir.blocks {
        collect_footnote_block(block, out);
    }
}

fn collect_footnotes_recursive(blocks: &[Block], out: &mut HashMap<String, Vec<Block>>) {
    for block in blocks {
        collect_footnote_block(block, out);
    }
}

fn collect_footnote_block(block: &Block, out: &mut HashMap<String, Vec<Block>>) {
    match block {
        Block::FootnoteDefinition { label, blocks, .. } => {
            out.insert(label.clone(), blocks.clone());
        }
        Block::Quote { blocks, .. } => collect_footnotes_recursive(blocks, out),
        Block::List { items, .. } => {
            for item in items {
                collect_footnotes_recursive(&item.blocks, out);
            }
        }
        _ => {}
    }
}

fn template_setup(template: &str, meta: &DocMeta, align: Align) -> Option<String> {
    if template.trim().is_empty() {
        return None;
    }
    let content = template_markup(template, meta);
    let align_fn = match align {
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
        Align::None => "left",
    };
    Some(format!(
        "context {{\n    set text(size: 8.5pt, fill: luma(120))\n    align({align_fn})[{content}]\n  }}"
    ))
}

fn template_markup(template: &str, meta: &DocMeta) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&escape_text(&rest[..start]));
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            out.push_str(&escape_text(&rest[start..]));
            rest = "";
            break;
        };
        let token = &after[..end];
        match token {
            "title" => out.push_str(&escape_text(&meta.title)),
            "notebook" => out.push_str(&escape_text(&meta.notebook)),
            "date" => out.push_str(&escape_text(&meta.date)),
            "page" => out.push_str("#counter(page).display()"),
            other => {
                out.push('{');
                out.push_str(&escape_text(other));
                out.push('}');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(&escape_text(rest));
    out
}

/// Markdown block quote: a left rule across the text width.
///
/// Typst's `#quote(block: true)` only insets both sides, so the rule the
/// preview draws never appears in the PDF.
fn quote_block(body: &str) -> String {
    let frame = "width: 100%, inset: (left: 10pt, y: 1pt), \
        stroke: (left: 2pt + luma(176)), above: 0.65em, below: 0.65em";
    format!("#block({frame})[\n{body}\n]")
}

fn table_column_spec(count: usize, width: TableWidth) -> String {
    let count = count.max(1);
    match width {
        TableWidth::Content => count.to_string(),
        TableWidth::Full => format!("({})", vec!["1fr"; count].join(", ")),
    }
}

fn quote_str(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn font_list(raw: &str, fallbacks: &[&str]) -> String {
    let mut names: Vec<String> = Vec::new();
    for part in raw.split(',') {
        let name = part.trim();
        if !name.is_empty()
            && !names
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            names.push(name.to_string());
        }
    }
    for fallback in fallbacks {
        if !names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(fallback))
        {
            names.push((*fallback).to_string());
        }
    }
    let items: Vec<String> = names.iter().map(|name| quote_str(name)).collect();
    format!("({})", items.join(", "))
}

fn plain_inlines(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) | Inline::Code(text) => out.push_str(text),
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
                if alt.is_empty() {
                    out.push_str(src);
                } else {
                    out.push_str(&plain_inlines(alt));
                }
            }
            Inline::Span { text, .. } => out.push_str(text),
            Inline::SoftBreak => out.push(' '),
            Inline::HardBreak => out.push(' '),
            Inline::FootnoteRef(label) => out.push_str(label),
            Inline::Math { tex, .. } => out.push_str(tex),
        }
    }
    out
}

/// Escapes text for Typst markup mode.
pub fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' | '#' | '$' | '*' | '_' | '`' | '<' | '>' | '@' | '~' | '[' | ']' | '-' | '+'
            | '=' | '/' => {
                out.push('\\');
                out.push(ch);
            }
            '\n' | '\r' | '\t' => out.push(' '),
            ch => out.push(ch),
        }
    }
    out
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn path_under(root: &Path, path: &Path) -> bool {
    let Ok(root) = fs::canonicalize(root) else {
        return false;
    };
    let root = root
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let path = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    path == root || path.starts_with(&format!("{root}\\"))
}

/// Returns the number of monospace columns available for a code block.
pub fn code_columns(options: &ExportOptions) -> usize {
    let width = options.content_width_pt() - 16.0;
    let size = options.code_size_pt.clamp(5.0, 20.0);
    ((width / (size * 0.6)).floor() as usize).clamp(20, 240)
}

/// Soft-wraps code to `max_columns` display columns (CJK counts as two).
pub fn wrap_code(text: &str, max_columns: usize) -> String {
    let mut lines = Vec::new();
    for line in text.split('\n') {
        wrap_line(line, max_columns, &mut lines);
    }
    lines.join("\n")
}

fn wrap_line(line: &str, max_columns: usize, out: &mut Vec<String>) {
    if display_width(line) <= max_columns {
        out.push(line.to_string());
        return;
    }
    let mut current = String::new();
    let mut width = 0usize;
    let mut last_space: Option<(usize, usize)> = None;
    for ch in line.chars() {
        let char_width = char_columns(ch);
        if width + char_width > max_columns && !current.is_empty() {
            match last_space {
                Some((byte_index, _)) if byte_index > 0 => {
                    let rest = current[byte_index..].trim_start().to_string();
                    current.truncate(byte_index);
                    out.push(current.trim_end().to_string());
                    current = rest;
                    width = display_width(&current);
                }
                _ => {
                    out.push(current.trim_end().to_string());
                    current = String::new();
                    width = 0;
                }
            }
            last_space = None;
        }
        if ch == '\t' {
            let spaces = 4 - (width % 4);
            for _ in 0..spaces {
                current.push(' ');
                width += 1;
            }
            continue;
        }
        current.push(ch);
        width += char_width;
        if ch == ' ' {
            last_space = Some((current.len() - 1, width));
        }
    }
    if !current.is_empty() {
        out.push(current.trim_end().to_string());
    }
    if out.is_empty() {
        out.push(String::new());
    }
}

fn display_width(text: &str) -> usize {
    text.chars().map(char_columns).sum()
}

fn char_columns(ch: char) -> usize {
    if ch == '\t' {
        4
    } else if is_wide(ch) {
        2
    } else {
        1
    }
}

fn is_wide(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x115F
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x3FFFD
    )
}

/// ISO `YYYY-MM-DD` date from the system clock (UTC).
pub fn today_iso() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_secs() / 86_400) as i64)
        .unwrap_or(0);
    civil_from_days(days)
}

fn civil_from_days(days_since_epoch: i64) -> String {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> DocMeta {
        DocMeta {
            title: "测试笔记".to_string(),
            notebook: "我的笔记本".to_string(),
            note_rel: "a.md".to_string(),
            date: "2026-10-27".to_string(),
        }
    }

    #[test]
    fn extra_blank_lines_become_vertical_space() {
        let ir = PreviewIr::from_markdown("甲\n\n\n乙\n");
        let doc = convert(&ir, &ExportOptions::default(), &meta(), Path::new("."));
        assert!(
            doc.source.contains("#v(1.65em, weak: false)\n乙"),
            "two source blank lines should keep one visible line on the next block, got {}",
            doc.source
        );
        assert!(
            !doc.source.contains("#v(1.65em, weak: false)\n\n"),
            "the extra line must not become a second paragraph break"
        );

        let ir = PreviewIr::from_markdown("甲\n\n\n\n乙\n");
        let doc = convert(&ir, &ExportOptions::default(), &meta(), Path::new("."));
        assert!(doc.source.contains("#v(3.3em, weak: false)"));

        let ir = PreviewIr::from_markdown("甲\n\n乙\n");
        let doc = convert(&ir, &ExportOptions::default(), &meta(), Path::new("."));
        assert!(
            !doc.source.contains("#v("),
            "a single blank line is only the block break"
        );
    }

    #[test]
    fn renders_headings_and_paragraphs() {
        let ir = PreviewIr::from_markdown("# 标题\n\n正文 **加粗** 和 `代码`。\n");
        let doc = convert(&ir, &ExportOptions::default(), &meta(), Path::new("."));
        assert!(doc.source.contains("= 标题"));
        assert!(doc.source.contains("#strong[加粗]"));
        assert!(doc.source.contains("#raw(\"代码\")"));
        assert!(doc.source.contains("#set page("));
        assert!(!doc.source.contains("height: auto"));
    }

    #[test]
    fn continuous_page_uses_auto_height() {
        let ir = PreviewIr::from_markdown("# 标题\n\n正文");
        let doc = convert_with(
            &ir,
            &ExportOptions::default(),
            &meta(),
            Path::new("."),
            true,
        );
        assert!(
            doc.source.contains("height: auto"),
            "continuous raster export should use auto page height"
        );
        assert!(!doc.line_map.is_empty());
    }

    #[test]
    fn exports_html_subset_styles_without_style_block() {
        let ir = PreviewIr::from_markdown(
            r#"
<style>
#t { color: #c00; }
.w { width: 40%; }
</style>
<div id="t" class="w">纯文本</div>

前文 <span style="color:#06c">蓝</span> 后文
"#,
        );
        let doc = convert(&ir, &ExportOptions::default(), &meta(), Path::new("."));
        assert!(!doc.source.contains("<style"), "{}", doc.source);
        assert!(doc.source.contains("纯文本"), "{}", doc.source);
        assert!(doc.source.contains("蓝"), "{}", doc.source);
        assert!(
            doc.source.contains("rgb(\"#cc0000\")") || doc.source.contains("rgb(\"#0066cc\")"),
            "{}",
            doc.source
        );
        assert!(doc.source.contains("40%"), "{}", doc.source);
    }

    #[test]
    fn escapes_markup_characters() {
        assert_eq!(
            escape_text("#标题 $x$ *a* [b]"),
            "\\#标题 \\$x\\$ \\*a\\* \\[b\\]"
        );
    }

    #[test]
    fn renders_tables_lists_and_tasks() {
        let ir = PreviewIr::from_markdown(
            "| 名称 | 数值 |\n|:--|--:|\n| a | 1 |\n\n- [x] 完成\n- [ ] 未完成\n",
        );
        let doc = convert(&ir, &ExportOptions::default(), &meta(), Path::new("."));
        assert!(
            doc.source
                .contains("#table(columns: 2, align: (left, right)")
        );
        assert!(doc.source.contains("table.header("));
        assert!(doc.source.contains("☑"));
        assert!(doc.source.contains("☐"));
    }

    #[test]
    fn full_width_tables_share_the_text_width() {
        let ir = PreviewIr::from_markdown("| 名称 | 数值 |\n|:--|--:|\n| a | 1 |\n");
        let options = ExportOptions {
            table_width: TableWidth::Full,
            ..Default::default()
        };
        let doc = convert(&ir, &options, &meta(), Path::new("."));
        assert!(
            doc.source
                .contains("#table(columns: (1fr, 1fr), align: (left, right)")
        );
    }

    #[test]
    fn blockquote_draws_a_left_rule() {
        let ir = PreviewIr::from_markdown("# 引用\n\n下面是引用块\n\n> 来自哪里。\n");
        let doc = convert(&ir, &ExportOptions::default(), &meta(), Path::new("."));
        assert!(doc.source.contains("来自哪里"), "{}", doc.source);
        assert!(
            doc.source.contains("stroke: (left: 2pt + luma(176))"),
            "{}",
            doc.source
        );
        assert!(!doc.source.contains("#quote("), "{}", doc.source);
    }

    #[test]
    fn renders_math_and_footnotes() {
        let ir = PreviewIr::from_markdown("公式 $a^2+b^2=c^2$ 与脚注[^1]。\n\n[^1]: 脚注内容\n");
        let doc = convert(&ir, &ExportOptions::default(), &meta(), Path::new("."));
        assert!(doc.source.contains("#footnote["));
        assert!(doc.source.contains("脚注内容"));
        assert!(doc.warnings.is_empty());
        assert!(!doc.source.contains("[^1]:"));
        // Inline math must stay inline: no padding spaces inside `$...$`,
        // which would make Typst treat it as a block equation.
        assert!(doc.source.contains("公式 $a ^(2 )"), "{}", doc.source);
    }

    #[test]
    fn renders_block_math_as_display() {
        let ir = PreviewIr::from_markdown("$$\na^2 + b^2\n$$\n");
        let doc = convert(&ir, &ExportOptions::default(), &meta(), Path::new("."));
        assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
        assert!(
            doc.source.contains("$ a ^(2 ) +  b ^(2 ) $"),
            "{}",
            doc.source
        );
    }

    #[test]
    fn cover_outline_and_templates() {
        let ir = PreviewIr::from_markdown("# 一\n");
        let mut options = ExportOptions {
            cover: true,
            toc: true,
            header: "{title} — {notebook}".to_string(),
            footer: "第 {page} 页".to_string(),
            ..Default::default()
        };
        options.toc_depth = 2;
        let doc = convert(&ir, &options, &meta(), Path::new("."));
        assert!(doc.source.contains("#page(header: none"));
        assert!(doc.source.contains("#outline(title: [目录], depth: 2)"));
        assert!(doc.source.contains("#counter(page).display()"));
        assert!(doc.source.contains("测试笔记"));
    }

    #[test]
    fn wraps_long_code_lines() {
        let wrapped = wrap_code("abcdefghij", 4);
        assert_eq!(wrapped, "abcd\nefgh\nij");
        let wrapped = wrap_code("abc def ghi", 7);
        assert_eq!(wrapped, "abc\ndef ghi");
        let wrapped = wrap_code("中文中文中文", 4);
        assert_eq!(wrapped, "中文\n中文\n中文");
    }

    #[test]
    fn code_columns_respects_margins() {
        let options = ExportOptions::default();
        let columns = code_columns(&options);
        assert!(columns > 40 && columns < 200);
    }

    #[test]
    fn dates_are_iso() {
        let date = today_iso();
        assert_eq!(date.len(), 10);
        assert_eq!(&date[4..5], "-");
        assert_eq!(civil_from_days(0), "1970-01-01");
    }
}
