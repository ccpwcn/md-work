use chardetng::EncodingDetector;
use encoding_rs::{Encoding, GB18030, GBK, UTF_16BE, UTF_16LE, UTF_8, WINDOWS_1252};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextEncoding {
    #[serde(rename = "utf-8")]
    Utf8,
    #[serde(rename = "utf-16le")]
    Utf16Le,
    #[serde(rename = "utf-16be")]
    Utf16Be,
    #[serde(rename = "gb18030")]
    Gb18030,
    #[serde(rename = "windows-1252")]
    Windows1252,
}

impl TextEncoding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Gb18030 => "GB18030",
            Self::Windows1252 => "Windows-1252",
        }
    }

    fn encoding(self) -> &'static Encoding {
        match self {
            Self::Utf8 => UTF_8,
            Self::Utf16Le => UTF_16LE,
            Self::Utf16Be => UTF_16BE,
            Self::Gb18030 => GB18030,
            Self::Windows1252 => WINDOWS_1252,
        }
    }

    fn from_encoding(encoding: &'static Encoding) -> Self {
        if encoding == UTF_8 {
            Self::Utf8
        } else if encoding == UTF_16LE {
            Self::Utf16Le
        } else if encoding == UTF_16BE {
            Self::Utf16Be
        } else if encoding == GBK || encoding == GB18030 {
            Self::Gb18030
        } else {
            Self::Windows1252
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecodedDoc {
    pub text: String,
    pub encoding: TextEncoding,
    pub bom: bool,
    pub eol: Eol,
}

impl DecodedDoc {
    pub fn encode(&self, text: &str) -> Result<Vec<u8>> {
        encode_text(text, self.encoding, self.bom, self.eol)
    }
}

pub fn decode(bytes: &[u8]) -> DecodedDoc {
    let (text, encoding, bom) = decode_text(bytes);
    let eol = detect_eol(&text);
    DecodedDoc {
        text: normalize_eol(&text),
        encoding,
        bom,
        eol,
    }
}

fn decode_text(bytes: &[u8]) -> (String, TextEncoding, bool) {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return (
            text.into_owned(),
            TextEncoding::from_encoding(encoding),
            true,
        );
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        return (text.to_string(), TextEncoding::Utf8, false);
    }

    let mut detector = EncodingDetector::new();
    detector.feed(bytes, true);
    let guessed = detector.guess(None, true);
    let preferred = TextEncoding::from_encoding(guessed);

    let candidate = if preferred == TextEncoding::Gb18030 {
        GB18030
    } else if preferred == TextEncoding::Utf8 {
        GB18030
    } else {
        WINDOWS_1252
    };

    let (text, _, had_errors) = candidate.decode(bytes);
    if !had_errors {
        (text.into_owned(), TextEncoding::from_encoding(candidate), false)
    } else {
        let (text, _, _) = WINDOWS_1252.decode(bytes);
        (text.into_owned(), TextEncoding::Windows1252, false)
    }
}

pub fn encode_text(
    text: &str,
    encoding: TextEncoding,
    bom: bool,
    eol: Eol,
) -> Result<Vec<u8>> {
    let canonical = normalize_eol(text);
    let with_eol = match eol {
        Eol::Lf => canonical,
        Eol::Crlf => canonical.replace('\n', "\r\n"),
    };

    let (bytes, _, had_errors) = encoding.encoding().encode(&with_eol);
    if had_errors {
        return Err(Error::Unencodable {
            encoding: encoding.label(),
        });
    }

    let mut out = Vec::with_capacity(bytes.len() + 3);
    if bom {
        match encoding {
            TextEncoding::Utf8 => out.extend_from_slice(b"\xEF\xBB\xBF"),
            TextEncoding::Utf16Le => out.extend_from_slice(b"\xFF\xFE"),
            TextEncoding::Utf16Be => out.extend_from_slice(b"\xFE\xFF"),
            _ => {}
        }
    }
    out.extend_from_slice(&bytes);
    Ok(out)
}

pub fn detect_eol(text: &str) -> Eol {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count().saturating_sub(crlf);
    if crlf > lf { Eol::Crlf } else { Eol::Lf }
}

pub fn normalize_eol(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_roundtrip_with_crlf() {
        let original = "第一行\r\nsecond line\r\n";
        let decoded = decode(original.as_bytes());
        assert_eq!(decoded.encoding, TextEncoding::Utf8);
        assert_eq!(decoded.eol, Eol::Crlf);
        assert_eq!(decoded.text, "第一行\nsecond line\n");
        let bytes = decoded.encode(&decoded.text).unwrap();
        assert_eq!(bytes, original.as_bytes());
    }

    #[test]
    fn utf8_bom_is_preserved() {
        let mut original = vec![0xEF, 0xBB, 0xBF];
        original.extend_from_slice("笔记".as_bytes());
        let decoded = decode(&original);
        assert!(decoded.bom);
        assert_eq!(decoded.text, "笔记");
        assert_eq!(decoded.encode(&decoded.text).unwrap(), original);
    }

    #[test]
    fn gb18030_roundtrip() {
        let text = "中文笔记：测试";
        let (bytes, _, had_errors) = GB18030.encode(text);
        assert!(!had_errors);
        let decoded = decode(&bytes);
        assert_eq!(decoded.encoding, TextEncoding::Gb18030);
        assert_eq!(decoded.text, text);
    }

    #[test]
    fn crlf_detection_prefers_dominant_style() {
        assert_eq!(detect_eol("a\nb\nc\r\n"), Eol::Lf);
        assert_eq!(detect_eol("a\r\nb\r\nc\n"), Eol::Crlf);
    }
}
