//! 墨笺受控 HTML 子集：标签 / 属性 / CSS 白名单解析。
//!
//! 规范见仓库 `doc/墨笺-html-子集.md`。

use std::collections::BTreeMap;

/// 长度：像素或相对内容区宽度的百分比（整数百分比 0–10000 表示 0.00%–100.00%，
/// 常见写法 `50%` 存为 5000）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    Px(u32),
    /// 百分之一的百分点：`50%` → 5000，`12.5%` → 1250。
    PercentCents(u32),
}

impl Length {
    pub fn as_percent_fraction(self) -> Option<f32> {
        match self {
            Self::PercentCents(cents) => Some(cents as f32 / 10_000.0),
            Self::Px(_) => None,
        }
    }

    pub fn as_px(self) -> Option<u32> {
        match self {
            Self::Px(px) => Some(px),
            Self::PercentCents(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleDecls {
    pub font_size: Option<Length>,
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub color: Option<RgbColor>,
    pub background_color: Option<RgbColor>,
}

impl StyleDecls {
    pub fn is_empty(&self) -> bool {
        self.font_size.is_none()
            && self.width.is_none()
            && self.height.is_none()
            && self.color.is_none()
            && self.background_color.is_none()
    }

    /// 后写覆盖先写。
    pub fn merge_from(&mut self, other: &Self) {
        if other.font_size.is_some() {
            self.font_size = other.font_size;
        }
        if other.width.is_some() {
            self.width = other.width;
        }
        if other.height.is_some() {
            self.height = other.height;
        }
        if other.color.is_some() {
            self.color = other.color;
        }
        if other.background_color.is_some() {
            self.background_color = other.background_color;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Class(String),
    Id(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleRule {
    pub selector: Selector,
    pub decls: StyleDecls,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleSheet {
    pub rules: Vec<StyleRule>,
}

impl StyleSheet {
    pub fn extend(&mut self, other: StyleSheet) {
        self.rules.extend(other.rules);
    }

    /// 按规范优先级解析元素最终样式（不含 img 的 width/height 属性）。
    pub fn resolve(&self, id: Option<&str>, classes: &[String], inline: &StyleDecls) -> StyleDecls {
        let mut out = StyleDecls::default();
        for class in classes {
            for rule in &self.rules {
                if let Selector::Class(name) = &rule.selector
                    && name == class
                {
                    out.merge_from(&rule.decls);
                }
            }
        }
        if let Some(id) = id {
            for rule in &self.rules {
                if let Selector::Id(name) = &rule.selector
                    && name == id
                {
                    out.merge_from(&rule.decls);
                }
            }
        }
        out.merge_from(inline);
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElementAttrs {
    pub id: Option<String>,
    pub name: Option<String>,
    pub class: Vec<String>,
    pub title: Option<String>,
    pub style: StyleDecls,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImgTag {
    pub src: String,
    pub alt: String,
    pub title: Option<String>,
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub attrs: ElementAttrs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextBoxTag {
    pub attrs: ElementAttrs,
    /// 纯文本内容（已剥掉嵌套标签记号，保留字面字符）。
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiftedHtml {
    Style(StyleSheet),
    Img(ImgTag),
    Div(TextBoxTag),
    Span(TextBoxTag),
    /// 未知或无法解析：调用方应丢弃。
    Ignored,
}

/// 尝试把一段原始 HTML（块或行内）提升为白名单节点。
pub fn lift_html_fragment(raw: &str) -> LiftedHtml {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return LiftedHtml::Ignored;
    }
    if let Some(body) = extract_style_body(trimmed) {
        return LiftedHtml::Style(parse_style_sheet(body));
    }
    if let Some(img) = parse_void_or_self_img(trimmed) {
        return LiftedHtml::Img(img);
    }
    if let Some((attrs, text)) = parse_paired_box(trimmed, "div") {
        return LiftedHtml::Div(TextBoxTag { attrs, text });
    }
    if let Some((attrs, text)) = parse_paired_box(trimmed, "span") {
        return LiftedHtml::Span(TextBoxTag { attrs, text });
    }
    LiftedHtml::Ignored
}

/// 解析开标签（不含内容），例如 `<span id="a" class="x">` 或 `<img ...>` / `<img ... />`。
pub fn parse_open_tag(raw: &str) -> Option<(String, BTreeMap<String, String>, bool)> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('<') || trimmed.starts_with("</") {
        return None;
    }
    let inner = trimmed.trim_start_matches('<').trim_end();
    let self_closing = inner.ends_with('/');
    let inner = inner.trim_end_matches('/').trim_end_matches('>').trim();
    let mut chars = inner.chars().peekable();
    let mut name = String::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_whitespace() {
            break;
        }
        name.push(ch);
        chars.next();
    }
    if name.is_empty() {
        return None;
    }
    let name = name.to_ascii_lowercase();
    let self_closing = self_closing || name == "img";
    let rest: String = chars.collect();
    let attrs = parse_attributes(&rest);
    Some((name, attrs, self_closing))
}

pub fn is_close_tag(raw: &str, expected: &str) -> bool {
    let trimmed = raw.trim();
    let expected = expected.to_ascii_lowercase();
    let lower = trimmed.to_ascii_lowercase();
    lower == format!("</{expected}>") || lower == format!("</{expected} >")
}

pub fn plain_text_from_htmlish(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '<' {
            for next in chars.by_ref() {
                if next == '>' {
                    break;
                }
            }
            continue;
        }
        if ch == '&' {
            let mut entity = String::new();
            while let Some(&next) = chars.peek() {
                if next == ';' || next.is_ascii_whitespace() || next == '&' || next == '<' {
                    break;
                }
                entity.push(next);
                chars.next();
            }
            if chars.peek() == Some(&';') {
                chars.next();
                out.push_str(&decode_entity(&entity));
            } else {
                out.push('&');
                out.push_str(&entity);
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn decode_entity(name: &str) -> String {
    match name {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" | "#39" => "'".into(),
        "nbsp" => " ".into(),
        other if other.starts_with('#') => {
            let digits = other.trim_start_matches('#');
            let code = if let Some(hex) = digits
                .strip_prefix('x')
                .or_else(|| digits.strip_prefix('X'))
            {
                u32::from_str_radix(hex, 16).ok()
            } else {
                digits.parse().ok()
            };
            code.and_then(char::from_u32)
                .map(|ch| ch.to_string())
                .unwrap_or_default()
        }
        _ => format!("&{name};"),
    }
}

fn extract_style_body(raw: &str) -> Option<&str> {
    let lower = raw.to_ascii_lowercase();
    let start = lower.find("<style")?;
    let after_start = &raw[start + 6..];
    let after_start_lower = after_start.to_ascii_lowercase();
    let gt = after_start_lower.find('>')?;
    // reject <style ...> with nonsense by requiring the open tag ends cleanly
    let open_attrs = &after_start[..gt];
    if open_attrs.contains('<') {
        return None;
    }
    let body_start = start + 6 + gt + 1;
    let rest_lower = raw[body_start..].to_ascii_lowercase();
    let end_rel = rest_lower.find("</style>")?;
    Some(&raw[body_start..body_start + end_rel])
}

pub fn parse_style_sheet(body: &str) -> StyleSheet {
    let mut rules = Vec::new();
    let mut rest = body;
    while let Some((selector_raw, decls_raw, next)) = next_css_rule(rest) {
        rest = next;
        let Some(selector) = parse_selector(selector_raw.trim()) else {
            continue;
        };
        let decls = parse_style_decls(decls_raw);
        if decls.is_empty() {
            continue;
        }
        rules.push(StyleRule { selector, decls });
    }
    StyleSheet { rules }
}

fn next_css_rule(input: &str) -> Option<(&str, &str, &str)> {
    let start = input.find(|ch: char| !ch.is_whitespace())?;
    let input = &input[start..];
    // skip @rules entirely
    if input.starts_with('@') {
        if let Some(end) = input.find('}') {
            return next_css_rule(&input[end + 1..]);
        }
        return None;
    }
    let open = input.find('{')?;
    let close_rel = input[open + 1..].find('}')?;
    let selector = &input[..open];
    let decls = &input[open + 1..open + 1 + close_rel];
    let rest = &input[open + 2 + close_rel..];
    Some((selector, decls, rest))
}

fn parse_selector(raw: &str) -> Option<Selector> {
    let raw = raw.trim();
    // 不支持逗号组合、后代等
    if raw.contains(|ch: char| ch.is_whitespace() || ch == ',' || ch == '>' || ch == '+') {
        return None;
    }
    if let Some(name) = raw.strip_prefix('.')
        && is_ident(name)
    {
        return Some(Selector::Class(name.to_string()));
    }
    if let Some(name) = raw.strip_prefix('#')
        && is_ident(name)
    {
        return Some(Selector::Id(name.to_string()));
    }
    None
}

fn is_ident(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

pub fn parse_style_decls(raw: &str) -> StyleDecls {
    let mut decls = StyleDecls::default();
    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((key, value)) = part.split_once(':') else {
            continue;
        };
        apply_decl(&mut decls, key.trim(), value.trim());
    }
    decls
}

fn apply_decl(decls: &mut StyleDecls, key: &str, value: &str) {
    let key = normalize_prop_name(key);
    match key.as_str() {
        "fontsize" => {
            if let Some(len) = parse_length(value) {
                decls.font_size = Some(len);
            }
        }
        "width" => {
            if let Some(len) = parse_length(value) {
                decls.width = Some(len);
            }
        }
        "height" => {
            if let Some(len) = parse_length(value) {
                decls.height = Some(len);
            }
        }
        "color" => {
            if let Some(color) = parse_color(value) {
                decls.color = Some(color);
            }
        }
        "backgroundcolor" => {
            if let Some(color) = parse_color(value) {
                decls.background_color = Some(color);
            }
        }
        _ => {}
    }
}

fn normalize_prop_name(key: &str) -> String {
    let lower = key.trim().to_ascii_lowercase().replace('-', "");
    // 兼容 backgroud*
    match lower.as_str() {
        "backgroudcolor" | "backgroud" => "backgroundcolor".into(),
        other => other.into(),
    }
}

pub fn parse_length(raw: &str) -> Option<Length> {
    let raw = raw.trim();
    if raw.is_empty() || raw.to_ascii_lowercase().contains("url") {
        return None;
    }
    if let Some(num) = raw.strip_suffix('%') {
        let cents = parse_percent_cents(num.trim())?;
        return Some(Length::PercentCents(cents));
    }
    let num = raw
        .strip_suffix("px")
        .or_else(|| raw.strip_suffix("PX"))
        .unwrap_or(raw)
        .trim();
    let px: f64 = num.parse().ok()?;
    if !px.is_finite() || px < 0.0 {
        return None;
    }
    Some(Length::Px(px.round().clamp(0.0, u32::MAX as f64) as u32))
}

fn parse_percent_cents(raw: &str) -> Option<u32> {
    let value: f64 = raw.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some((value * 100.0).round().clamp(0.0, 1_000_000.0) as u32)
}

pub fn parse_color(raw: &str) -> Option<RgbColor> {
    let raw = raw.trim();
    let hex = raw.strip_prefix('#')?;
    match hex.len() {
        3 => {
            let mut chars = hex.chars();
            let r = chars.next()?;
            let g = chars.next()?;
            let b = chars.next()?;
            Some(RgbColor {
                r: expand_nibble(r)?,
                g: expand_nibble(g)?,
                b: expand_nibble(b)?,
            })
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(RgbColor { r, g, b })
        }
        _ => None,
    }
}

fn expand_nibble(ch: char) -> Option<u8> {
    let v = ch.to_digit(16)?;
    Some((v * 17) as u8)
}

fn parse_void_or_self_img(raw: &str) -> Option<ImgTag> {
    let (name, attrs, _) = parse_open_tag(raw)?;
    if name != "img" {
        return None;
    }
    // 允许 `<img ...>` 单独一段（无强制 />）
    if raw.contains("</") {
        return None;
    }
    Some(img_from_attrs(attrs))
}

fn img_from_attrs(attrs: BTreeMap<String, String>) -> ImgTag {
    let mut element = element_attrs_from_map(&attrs);
    let width = attrs.get("width").and_then(|v| parse_length(v));
    let height = attrs.get("height").and_then(|v| parse_length(v));
    // img 属性尺寸不进 style；resolve 时属性层再盖
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

fn parse_paired_box(raw: &str, tag: &str) -> Option<(ElementAttrs, String)> {
    let lower = raw.to_ascii_lowercase();
    let open_pat = format!("<{tag}");
    if !lower.trim_start().starts_with(&open_pat) {
        return None;
    }
    let start = lower.find(&open_pat)?;
    let after = &raw[start + open_pat.len()..];
    let gt = after.find('>')?;
    let open_full = &raw[start..start + open_pat.len() + gt + 1];
    let (name, attrs, self_closing) = parse_open_tag(open_full)?;
    if name != tag {
        return None;
    }
    if self_closing {
        return Some((element_attrs_from_map(&attrs), String::new()));
    }
    let body_start = start + open_pat.len() + gt + 1;
    let close_pat = format!("</{tag}>");
    let rest_lower = raw[body_start..].to_ascii_lowercase();
    let end_rel = rest_lower.rfind(&close_pat)?;
    let body = &raw[body_start..body_start + end_rel];
    Some((
        element_attrs_from_map(&attrs),
        plain_text_from_htmlish(body),
    ))
}

fn element_attrs_from_map(attrs: &BTreeMap<String, String>) -> ElementAttrs {
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
        .map(|value| parse_style_decls(value))
        .unwrap_or_default();
    ElementAttrs {
        id: attrs.get("id").cloned().filter(|s| !s.is_empty()),
        name: attrs.get("name").cloned().filter(|s| !s.is_empty()),
        class,
        title: attrs.get("title").cloned().filter(|s| !s.is_empty()),
        style,
    }
}

fn parse_attributes(input: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let key_start = i;
        while i < bytes.len()
            && bytes[i] != b'='
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'>'
            && bytes[i] != b'/'
        {
            i += 1;
        }
        if key_start == i {
            i += 1;
            continue;
        }
        let key = input[key_start..i].to_ascii_lowercase();
        if key.starts_with("on") {
            // 跳过事件处理器及其值
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'=' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                    let quote = bytes[i];
                    i += 1;
                    while i < bytes.len() && bytes[i] != quote {
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                } else {
                    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                }
            }
            continue;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let quote = bytes[i];
                i += 1;
                let value_start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                let value = input[value_start..i].to_string();
                if i < bytes.len() {
                    i += 1;
                }
                value
            } else {
                let value_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                input[value_start..i].to_string()
            }
        } else {
            String::new()
        };
        if is_allowed_attr(&key) {
            map.insert(key, value);
        }
    }
    map
}

fn is_allowed_attr(name: &str) -> bool {
    matches!(
        name,
        "src" | "alt" | "title" | "width" | "height" | "id" | "name" | "style" | "class"
    )
}

/// 合并 img 最终尺寸：样式表结果 + 属性覆盖。
pub fn resolve_img_size(
    sheet: &StyleSheet,
    img: &ImgTag,
) -> (StyleDecls, Option<Length>, Option<Length>) {
    let mut style = sheet.resolve(img.attrs.id.as_deref(), &img.attrs.class, &img.attrs.style);
    let width = img.width.or(style.width);
    let height = img.height.or(style.height);
    style.width = width;
    style.height = height;
    (style, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_img_dimensions() {
        let lifted =
            lift_html_fragment(r#"<img src="assets/a.jpg" alt="a" width="300px" height="50%" />"#);
        let LiftedHtml::Img(img) = lifted else {
            panic!("expected img");
        };
        assert_eq!(img.src, "assets/a.jpg");
        assert_eq!(img.width, Some(Length::Px(300)));
        assert_eq!(img.height, Some(Length::PercentCents(5000)));
    }

    #[test]
    fn parses_style_sheet_and_resolve() {
        let sheet = parse_style_sheet(
            r#"
            .narrow { width: 40%; }
            #warn { color: #c00; background-color: #fee; font-size: 14px; }
            div { color: #000; }
            "#,
        );
        assert_eq!(sheet.rules.len(), 2);
        let style = sheet.resolve(Some("warn"), &["narrow".into()], &StyleDecls::default());
        assert_eq!(style.width, Some(Length::PercentCents(4000)));
        assert_eq!(
            style.color,
            Some(RgbColor {
                r: 0xcc,
                g: 0,
                b: 0
            })
        );
        assert_eq!(style.font_size, Some(Length::Px(14)));
    }

    #[test]
    fn div_content_is_plain_text() {
        let lifted = lift_html_fragment(r#"<div id="x">**不是粗体**<b>也不是</b></div>"#);
        let LiftedHtml::Div(box_) = lifted else {
            panic!("expected div");
        };
        assert_eq!(box_.text, "**不是粗体**也不是");
        assert_eq!(box_.attrs.id.as_deref(), Some("x"));
    }

    #[test]
    fn ignores_event_handlers_and_unknown_tags() {
        let lifted = lift_html_fragment(r#"<img src="a.jpg" onerror="alert(1)" width="10">"#);
        let LiftedHtml::Img(img) = lifted else {
            panic!("expected img");
        };
        assert_eq!(img.width, Some(Length::Px(10)));
        assert!(matches!(
            lift_html_fragment("<script>x</script>"),
            LiftedHtml::Ignored
        ));
    }

    #[test]
    fn img_attr_width_overrides_stylesheet() {
        let sheet = parse_style_sheet(".w { width: 10%; }");
        let LiftedHtml::Img(img) =
            lift_html_fragment(r#"<img class="w" src="a.jpg" width="200px" />"#)
        else {
            panic!("img");
        };
        let (_, width, _) = resolve_img_size(&sheet, &img);
        assert_eq!(width, Some(Length::Px(200)));
    }

    #[test]
    fn accepts_background_typo_alias() {
        let decls = parse_style_decls("backgroud-color: #abc");
        assert_eq!(
            decls.background_color,
            Some(RgbColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc
            })
        );
    }
}
