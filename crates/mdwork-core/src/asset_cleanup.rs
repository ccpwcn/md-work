//! 扫描并清理笔记本 `assets/` 目录中未被任何笔记引用的图片。
//!
//! 判定刻意保守：只要某个图片文件名在工作区里任意 Markdown 文件中出现过
//! （包括 `../assets/x.png`、HTML `<img src=...>`、URL 编码等写法），就认为
//! 它仍被引用。宁可少删，也不要误删用户正在使用的图片。
//!
//! 所有磁盘访问都由调用方放到后台线程执行，避免阻塞界面。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// 会被清理的图片扩展名（小写，不含点）。
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "ico", "tif", "tiff",
];

/// 单篇笔记最多读取 8 MiB；异常大的文件跳过并计数，不把它当成「没有引用」。
const MAX_NOTE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnusedAssetReport {
    /// 待清理文件的绝对路径，已排序。
    pub files: Vec<PathBuf>,
    pub bytes: u64,
    /// 扫描到的图片总数。
    pub asset_count: usize,
    pub note_count: usize,
    /// 无法读取的笔记数（编码特殊、体积过大等）。
    pub unreadable_notes: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupOutcome {
    pub deleted: usize,
    pub bytes: u64,
    pub failed: usize,
}

/// 扫描工作区，返回所有「未被引用」的图片。
///
/// `notebook_roots` 决定哪些 `assets/` 目录参与清理；引用则在整个工作区
/// 的 Markdown 文件里查找，跨笔记本引用同样算作「已使用」。
pub fn scan(workspace_root: &Path, notebook_roots: &[PathBuf]) -> UnusedAssetReport {
    let mut report = UnusedAssetReport::default();
    let mut references = HashSet::new();
    // 特殊字符（空格、括号、中文等）可能让路径切分失效，留一份合并文本
    // 兜底；只对这类文件名做全文查找，普通文件名仍走 O(1) 的集合查询。
    let mut note_text = String::new();

    for entry in WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() || !is_note_file(entry.path()) {
            continue;
        }
        report.note_count += 1;
        let bytes = match std::fs::read(entry.path()) {
            Ok(bytes) if bytes.len() as u64 <= MAX_NOTE_BYTES => bytes,
            _ => {
                report.unreadable_notes += 1;
                continue;
            }
        };
        // 笔记可能是 GB18030 等编码；文件名一律是 ASCII/UTF-8，按字节
        // 有损解码后再小写，不会影响路径片段的匹配。
        let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
        collect_references(&text, &mut references);
        note_text.push_str(&text);
        note_text.push('\n');
    }

    for root in notebook_roots {
        let assets = crate::image::assets_dir(root);
        if !assets.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&assets)
            .follow_links(false)
            .min_depth(1)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !is_image_file(name) {
                continue;
            }
            report.asset_count += 1;
            let lower = name.to_ascii_lowercase();
            let referenced = references.contains(&lower)
                || (!is_simple_name(&lower)
                    && (note_text.contains(&lower)
                        || note_text.contains(&encode_url_name(&lower))));
            if referenced {
                continue;
            }
            report.bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            report.files.push(path.to_path_buf());
        }
    }
    report.files.sort();
    report
}

/// 重新扫描后删除未引用图片。重新扫描而不是复用上次的列表，避免用户
/// 在「扫描 → 确认」之间新引用的图片被误删。
pub fn delete_unused(workspace_root: &Path, notebook_roots: &[PathBuf]) -> CleanupOutcome {
    let report = scan(workspace_root, notebook_roots);
    let mut outcome = CleanupOutcome::default();
    for path in &report.files {
        let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        match std::fs::remove_file(path) {
            Ok(()) => {
                outcome.deleted += 1;
                outcome.bytes += size;
            }
            Err(_) => outcome.failed += 1,
        }
    }
    outcome
}

fn is_note_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

fn is_image_file(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    IMAGE_EXTENSIONS
        .iter()
        .any(|candidate| ext.eq_ignore_ascii_case(candidate))
}

/// 只含这些字符的文件名一定能被路径切分完整识别，不需要全文兜底。
fn is_simple_name(name: &str) -> bool {
    name.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// 与 `image::encode_url_path` 一致；入参已小写，十六进制也用小写。
fn encode_url_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '%' => out.push_str("%25"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3f"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            _ => out.push(ch),
        }
    }
    out
}

/// 文件名中可以出现的字符；其余字符视为路径/语法分隔符。
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'-' | b'_' | b'.' | b'%' | b'~' | b'+')
        || byte >= 0x80
}

/// 从一段（已小写的）笔记文本里收集被引用的图片文件名。
///
/// 以 `.png` 等扩展名为锚点向前扫描出一个路径片段，再取最后一段文件名
/// 并做百分号解码。这样 `![](assets/x.png)`、`![x](../assets/my%20pic.png)`、
/// `<img src="assets/x.png">`、引用式链接都能被识别，且是线性扫描。
fn collect_references(text: &str, out: &mut HashSet<String>) {
    let bytes = text.as_bytes();
    for ext in IMAGE_EXTENSIONS {
        let needle = format!(".{ext}");
        let mut cursor = 0;
        while let Some(offset) = text[cursor..].find(&needle) {
            let dot = cursor + offset;
            let end = dot + needle.len();
            let mut start = dot;
            while start > 0 && is_name_byte(bytes[start - 1]) {
                start -= 1;
            }
            // 多字节 UTF-8 字符的后续字节同样 >= 0x80，回退可能停在字符中间。
            while start > 0 && !text.is_char_boundary(start) {
                start -= 1;
            }
            let token = &text[start..end];
            let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
            out.insert(percent_decode(name));
            cursor = end;
        }
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hi = hex_value(bytes[index + 1]);
            let lo = hex_value(bytes[index + 2]);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push(hi << 4 | lo);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_workspace(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("mdwork-assets-{label}-{stamp}"));
        std::fs::create_dir_all(root.join("notebook/assets")).unwrap();
        root
    }

    #[test]
    fn references_cover_relative_html_and_encoded_paths() {
        let mut found = HashSet::new();
        let text = r#"
            ![cover](assets/cover.png)
            ![photo](../assets/my%20photo.JPG)
            <img src="assets/diagram.svg" width="100">
            [ref]: ./assets/ref.webp
        "#
        .to_ascii_lowercase();
        collect_references(&text, &mut found);
        assert!(found.contains("cover.png"));
        assert!(found.contains("my photo.jpg"));
        assert!(found.contains("diagram.svg"));
        assert!(found.contains("ref.webp"));
        assert!(!found.contains("missing.png"));
    }

    #[test]
    fn scan_only_reports_unreferenced_images() {
        let root = temp_workspace("scan");
        let notebook = root.join("notebook");
        let assets = notebook.join("assets");
        std::fs::write(assets.join("used.png"), b"used").unwrap();
        std::fs::write(assets.join("orphan.png"), b"orphan").unwrap();
        // 带空格与括号的文件名走 URL 编码引用，也必须判定为「已使用」。
        std::fs::write(assets.join("my photo(1).png"), b"special").unwrap();
        std::fs::write(assets.join("中文 图.png"), b"unicode").unwrap();
        std::fs::write(assets.join("notes.txt"), b"not an image").unwrap();
        std::fs::write(
            notebook.join("note.md"),
            "# Note\n\n![used](assets/used.png)\n\
             ![s](assets/my%20photo%281%29.png)\n\
             ![c](assets/中文%20图.png)\n",
        )
        .unwrap();

        let report = scan(&root, std::slice::from_ref(&notebook));
        assert_eq!(report.asset_count, 4);
        assert_eq!(report.note_count, 1);
        assert_eq!(report.files, vec![assets.join("orphan.png")]);
        assert_eq!(report.bytes, 6);

        let outcome = delete_unused(&root, std::slice::from_ref(&notebook));
        assert_eq!(outcome.deleted, 1);
        assert_eq!(outcome.failed, 0);
        assert!(assets.join("used.png").is_file());
        assert!(assets.join("my photo(1).png").is_file());
        assert!(assets.join("中文 图.png").is_file());
        assert!(!assets.join("orphan.png").exists());

        // 清理后重新扫描应为空，且不会触碰非图片文件。
        assert!(scan(&root, &[notebook]).files.is_empty());
        assert!(assets.join("notes.txt").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn references_in_other_notebooks_keep_assets() {
        let root = temp_workspace("cross");
        let first = root.join("first");
        let second = root.join("second");
        std::fs::create_dir_all(first.join("assets")).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("assets/shared.png"), b"x").unwrap();
        std::fs::write(second.join("note.md"), "![x](../first/assets/shared.png)").unwrap();

        let report = scan(&root, &[first.clone(), second.clone()]);
        assert!(report.files.is_empty(), "跨笔记本引用必须算作已使用");
        std::fs::remove_dir_all(root).unwrap();
    }
}
