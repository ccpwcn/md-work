#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextStats {
    pub lines: usize,
    pub words: usize,
    pub chars: usize,
}

/// Counts lines, characters and words; CJK characters count as one word each,
/// Latin runs count as one word.
pub fn text_stats(text: &str) -> TextStats {
    let mut chars = 0;
    let mut words = 0;
    let mut in_word = false;

    for ch in text.chars() {
        if ch == '\r' {
            continue;
        }
        chars += 1;
        if ch.is_whitespace() {
            in_word = false;
            continue;
        }
        if is_cjk(ch) {
            words += 1;
            in_word = false;
            continue;
        }
        if ch.is_alphanumeric() {
            if !in_word {
                words += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }

    let lines = if chars == 0 {
        1
    } else {
        text.chars().filter(|ch| *ch == '\n').count() + 1
    };

    TextStats {
        lines,
        words,
        chars,
    }
}

pub fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x2E80..=0x2FFF
        | 0x3000..=0x303F
        | 0x3040..=0x30FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0xFF00..=0xFFEF
        | 0x20000..=0x2FA1F
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_mixed_text() {
        let stats = text_stats("你好 world\nsecond line");
        assert_eq!(stats.lines, 2);
        // 你 + 好 + world + second + line
        assert_eq!(stats.words, 5);
    }

    #[test]
    fn empty_text_has_one_line() {
        let stats = text_stats("");
        assert_eq!(
            stats,
            TextStats {
                lines: 1,
                words: 0,
                chars: 0
            }
        );
    }

    #[test]
    fn trailing_newline_counts_extra_line() {
        assert_eq!(text_stats("a\n").lines, 2);
    }
}
