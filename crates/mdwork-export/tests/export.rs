use std::fs;
use std::path::{Path, PathBuf};

use mdwork_export::exporter::{ExportEvent, ExportFormat, ExportRequest, run};
use mdwork_export::{ExportOptions, TableWidth};

fn workspace(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mdwork-export-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("assets")).unwrap();
    dir
}

const NOTE: &str = r#"# 导出测试

这是一段中文正文，包含 **加粗**、*斜体*、`行内代码` 与 [链接](https://example.com)。

## 列表

- 第一项
- 第二项
  - 嵌套

- [x] 已完成
- [ ] 未完成

## 代码

```rust
fn main() {
    println!("你好，墨笺");
}
```

## 公式与表格

$$ a^2 + b^2 = c^2 $$

| 名称 | 数值 |
|:--|--:|
| 一 | 1 |

![示意图](assets/demo.svg)

> 引用文本

---

脚注[^1]。

[^1]: 脚注内容
"#;

#[test]
fn exports_pdf_and_png() {
    let dir = workspace("pdf-png");
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="60"><rect width="120" height="60" fill="#dbeafe"/><text x="10" y="35" font-size="16">图</text></svg>"##;
    fs::write(dir.join("assets/demo.svg"), svg).unwrap();

    let request = ExportRequest {
        notebook_root: dir.clone(),
        note_rel: "测试.md".to_string(),
        notebook_name: "测试笔记本".to_string(),
        title: "导出测试".to_string(),
        format: ExportFormat::Pdf,
        output: dir.join("out.pdf"),
        options: ExportOptions {
            cover: true,
            toc: true,
            header: "{title} — {page}".to_string(),
            footer: "第 {page} 页".to_string(),
            ..Default::default()
        },
        markdown: Some(NOTE.to_string()),
        temp_dir: Some(dir.join("tmp-pdf")),
    };
    let mut events = Vec::new();
    let outcome = run(&request, &mut |event| events.push(event)).expect("pdf export");
    assert_eq!(outcome.paths, vec![dir.join("out.pdf")]);
    let bytes = fs::read(&outcome.paths[0]).unwrap();
    assert!(bytes.starts_with(b"%PDF"));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ExportEvent::Done { .. }))
    );
    assert!(!dir.join("tmp-pdf").exists());

    let request = ExportRequest {
        format: ExportFormat::Png,
        output: dir.join("out.png"),
        options: ExportOptions {
            png_dpi: 72,
            ..Default::default()
        },
        temp_dir: Some(dir.join("tmp-png")),
        ..request
    };
    let outcome = run(&request, &mut |_| {}).expect("png export");
    assert_eq!(outcome.paths, vec![dir.join("out.png")]);
    let bytes = fs::read(&outcome.paths[0]).unwrap();
    assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    assert!(!dir.join("tmp-png").exists());
}

#[test]
fn exports_page_files_and_split_names() {
    let dir = workspace("page-files");
    let request = ExportRequest {
        notebook_root: dir.clone(),
        note_rel: "多页.md".to_string(),
        notebook_name: String::new(),
        title: String::new(),
        format: ExportFormat::Png,
        output: dir.join("多页.png"),
        options: ExportOptions {
            png_dpi: 72,
            png_page_files: true,
            ..Default::default()
        },
        markdown: Some(format!(
            "{}\n{}",
            "# 第一页\n\n正文。\n", "# 第二页\n\n正文。\n"
        )),
        temp_dir: None,
    };
    let outcome = run(&request, &mut |_| {}).expect("png export");
    assert!(!outcome.paths.is_empty());
    for path in &outcome.paths {
        assert!(path.exists(), "{}", path.display());
        assert!(path.file_name().unwrap().to_string_lossy().contains('-'));
    }
    cleanup(&dir);
}

#[test]
fn long_png_is_taller_than_one_a4_page() {
    let dir = workspace("long-png");
    let mut markdown = String::from("# 长图\n\n");
    for index in 0..40 {
        markdown.push_str(&format!(
            "## 节 {index}\n\n这是第 {index} 段正文，用来撑开连续长图。重复句子保证内容足够长。\n\n"
        ));
    }
    let request = ExportRequest {
        notebook_root: dir.clone(),
        note_rel: "长图.md".to_string(),
        notebook_name: String::new(),
        title: "长图".to_string(),
        format: ExportFormat::Png,
        output: dir.join("长图.png"),
        options: ExportOptions {
            png_dpi: 72,
            png_page_files: false,
            page_numbers: false,
            ..Default::default()
        },
        markdown: Some(markdown),
        temp_dir: Some(dir.join("tmp")),
    };
    let outcome = run(&request, &mut |_| {}).expect("long png");
    assert_eq!(outcome.paths.len(), 1);
    let bytes = fs::read(&outcome.paths[0]).unwrap();
    let image = image::load_from_memory(&bytes).unwrap();
    // A4 @ 72dpi is ~842px tall; a continuous long image must exceed that.
    assert!(
        image.height() > 900,
        "expected a tall continuous image, got {}x{}",
        image.width(),
        image.height()
    );
    cleanup(&dir);
}

#[test]
fn exports_webp_and_can_overwrite() {
    let dir = workspace("webp");
    let request = ExportRequest {
        notebook_root: dir.clone(),
        note_rel: "图.md".to_string(),
        notebook_name: String::new(),
        title: "图".to_string(),
        format: ExportFormat::Webp,
        output: dir.join("out.webp"),
        options: ExportOptions {
            png_dpi: 72,
            png_page_files: false,
            ..Default::default()
        },
        markdown: Some("# WebP\n\n正文。\n".to_string()),
        temp_dir: Some(dir.join("tmp1")),
    };
    let outcome = run(&request, &mut |_| {}).expect("webp export");
    let bytes = fs::read(&outcome.paths[0]).unwrap();
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WEBP");

    let again = ExportRequest {
        temp_dir: Some(dir.join("tmp2")),
        ..request
    };
    run(&again, &mut |_| {}).expect("webp overwrite");
    cleanup(&dir);
}

#[test]
fn quote_rule_is_on_the_left_and_table_width_is_measurable() {
    let dir = workspace("quote-table");
    let quote = export_image(
        &dir,
        "quote.png",
        "> 来自哪里。\n",
        ExportOptions {
            page_numbers: false,
            png_dpi: 144,
            ..Default::default()
        },
    );
    let bar_x = leftmost_rule_column(&quote).unwrap_or_else(|| {
        panic!(
            "expected a vertical quote rule, image {}x{}",
            quote.width(),
            quote.height()
        )
    });
    assert!(
        bar_x < quote.width() / 4,
        "quote rule should sit on the left, x={bar_x} width={}",
        quote.width()
    );

    let table =
        "| 名称 | emoji | 单价 |\n| --- | --- | --- |\n| 苹果 | 🍎 | 6元 |\n| 梨 | 🍐 | 5元 |\n";
    let content = export_image(
        &dir,
        "table-content.png",
        table,
        ExportOptions {
            page_numbers: false,
            png_dpi: 96,
            table_width: TableWidth::Content,
            ..Default::default()
        },
    );
    let full = export_image(
        &dir,
        "table-full.png",
        table,
        ExportOptions {
            page_numbers: false,
            png_dpi: 96,
            table_width: TableWidth::Full,
            ..Default::default()
        },
    );
    let content_span = ink_span(&content);
    let full_span = ink_span(&full);
    assert!(
        full_span > content_span * 3 / 2,
        "full-width table span {full_span}px should be clearly wider than content span {content_span}px"
    );
    assert!(
        full_span * 10 > u64::from(full.width()) * 7,
        "full-width table should cover most of the text width, span {full_span}px page {}px",
        full.width()
    );
    cleanup(&dir);
}

fn export_image(
    dir: &Path,
    name: &str,
    markdown: &str,
    options: ExportOptions,
) -> image::DynamicImage {
    let request = ExportRequest {
        notebook_root: dir.to_path_buf(),
        note_rel: "样本.md".to_string(),
        notebook_name: String::new(),
        title: "样本".to_string(),
        format: ExportFormat::Png,
        output: dir.join(name),
        options,
        markdown: Some(markdown.to_string()),
        temp_dir: Some(dir.join(format!("tmp-{name}"))),
    };
    let outcome = run(&request, &mut |_| {}).unwrap_or_else(|err| panic!("export {name}: {err}"));
    image::load_from_memory(&fs::read(&outcome.paths[0]).unwrap()).unwrap()
}

fn leftmost_rule_column(image: &image::DynamicImage) -> Option<u32> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    (0..width).find(|&x| {
        let mut run = 0u32;
        (0..height).any(|y| {
            if is_rule_gray(rgba.get_pixel(x, y).0) {
                run += 1;
                run >= 12
            } else {
                run = 0;
                false
            }
        })
    })
}

fn is_rule_gray(pixel: [u8; 4]) -> bool {
    let [r, g, b, a] = pixel;
    if a < 128 {
        return false;
    }
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    (140..=210).contains(&max) && min >= 120 && max - min <= 20
}

fn ink_span(image: &image::DynamicImage) -> u64 {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut widest = 0u64;
    for y in 0..height {
        let mut first = None;
        let mut last = 0u32;
        for x in 0..width {
            let [r, g, b, a] = rgba.get_pixel(x, y).0;
            if a > 20 && (r < 245 || g < 245 || b < 245) {
                if first.is_none() {
                    first = Some(x);
                }
                last = x;
            }
        }
        if let Some(first) = first {
            widest = widest.max(u64::from(last - first + 1));
        }
    }
    widest
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}
