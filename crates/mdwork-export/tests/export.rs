use std::fs;
use std::path::{Path, PathBuf};

use mdwork_export::exporter::{ExportFormat, ExportEvent, ExportRequest, run};
use mdwork_export::ExportOptions;

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mdwork-export-test-{name}-{}",
        std::process::id()
    ));
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
    assert!(events.iter().any(|event| matches!(event, ExportEvent::Done { .. })));
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
        markdown: Some(format!("{}\n{}", "# 第一页\n\n正文。\n", "# 第二页\n\n正文。\n")),
        temp_dir: None,
    };
    let outcome = run(&request, &mut |_| {}).expect("png export");
    assert!(outcome.paths.len() >= 1);
    for path in &outcome.paths {
        assert!(path.exists(), "{}", path.display());
        assert!(path.file_name().unwrap().to_string_lossy().contains('-'));
    }
    cleanup(&dir);
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}
