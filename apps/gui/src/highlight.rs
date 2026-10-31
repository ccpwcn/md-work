//! Syntect-based syntax highlighting for preview code blocks.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::{HighlightStyle, rgba};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, Theme};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use two_face::theme::EmbeddedThemeName;

pub type CodeSpans = Arc<Vec<(Range<usize>, HighlightStyle)>>;

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

fn theme(dark: bool) -> &'static Theme {
    static LIGHT: OnceLock<Theme> = OnceLock::new();
    static DARK: OnceLock<Theme> = OnceLock::new();
    if dark {
        DARK.get_or_init(|| {
            two_face::theme::extra()
                .get(EmbeddedThemeName::Base16OceanDark)
                .clone()
        })
    } else {
        LIGHT.get_or_init(|| {
            two_face::theme::extra()
                .get(EmbeddedThemeName::InspiredGithub)
                .clone()
        })
    }
}

fn cache() -> &'static Mutex<HashMap<u64, CodeSpans>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, CodeSpans>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Styles for `code`, cached by language/text/theme so scrolling a code-heavy
/// preview does not re-run the parser every frame.
pub fn code_spans(language: Option<&str>, code: &str, dark: bool) -> CodeSpans {
    let Some(language) = language.map(str::trim).filter(|lang| !lang.is_empty()) else {
        return Arc::new(Vec::new());
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    language.hash(&mut hasher);
    code.hash(&mut hasher);
    dark.hash(&mut hasher);
    let key = hasher.finish();
    if let Ok(cache) = cache().lock()
        && let Some(spans) = cache.get(&key)
    {
        return spans.clone();
    }
    let spans = Arc::new(highlight(language, code, dark));
    if let Ok(mut cache) = cache().lock() {
        if cache.len() >= 96 {
            cache.clear();
        }
        cache.insert(key, spans.clone());
    }
    spans
}

fn highlight(language: &str, code: &str, dark: bool) -> Vec<(Range<usize>, HighlightStyle)> {
    let syntaxes = syntaxes();
    let Some(syntax) = syntaxes
        .find_syntax_by_token(language)
        .or_else(|| syntaxes.find_syntax_by_extension(language))
        .or_else(|| syntaxes.find_syntax_by_name(language))
    else {
        return Vec::new();
    };
    let mut highlighter = HighlightLines::new(syntax, theme(dark));
    let mut spans = Vec::new();
    let mut offset = 0usize;
    for line in LinesWithEndings::from(code) {
        let Ok(items) = highlighter.highlight_line(line, syntaxes) else {
            return Vec::new();
        };
        for (style, text) in items {
            let len = text.len();
            if len == 0 {
                continue;
            }
            let end = offset + len;
            // Syntect slices must reassemble the source; if they diverge, fall
            // back to unstyled text rather than feed GPUI invalid runs.
            if end > code.len() || !code.is_char_boundary(offset) || !code.is_char_boundary(end) {
                return Vec::new();
            }
            spans.push((
                offset..end,
                HighlightStyle {
                    color: Some(color_to_hsla(style.foreground)),
                    ..Default::default()
                },
            ));
            offset = end;
        }
    }
    if offset != code.len() {
        return Vec::new();
    }
    spans
}

fn color_to_hsla(color: Color) -> gpui_kit::Hsla {
    rgba(u32::from_be_bytes([color.r, color.g, color.b, color.a])).into()
}

#[cfg(test)]
mod tests {
    use super::highlight;

    #[::core::prelude::v1::test]
    fn rust_highlights_cover_source_exactly() {
        let code = "fn main() {\n    println!(\"你好\");\n}\n";
        let spans = highlight("rust", code, true);
        assert!(!spans.is_empty());
        let mut offset = 0usize;
        for (range, _) in &spans {
            assert_eq!(range.start, offset);
            assert!(code.is_char_boundary(range.start));
            assert!(code.is_char_boundary(range.end));
            offset = range.end;
        }
        assert_eq!(offset, code.len());
    }

    #[::core::prelude::v1::test]
    fn unknown_language_yields_no_spans() {
        assert!(highlight("not-a-real-lang-xyz", "x = 1", false).is_empty());
    }
}
