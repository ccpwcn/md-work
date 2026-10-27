use std::path::PathBuf;

use mdwork_export::{export_pdf_source, export_png};

const SOURCE: &str = r#"#set page(paper: "a4", margin: 2cm)
#set text(font: ("Microsoft YaHei", "SimSun", "Noto Sans CJK SC"), lang: "zh", size: 11pt)
#set heading(numbering: "1.")

= 墨笺导出验证

这是一段中文正文，用于验证 Typst 的系统字体发现、CJK 字体嵌入与 PDF 输出。
中英文混排：md-work 是一款纯 Rust 的 Markdown 笔记本。

== 公式

$ x^2 + y^2 = z^2 $

== 代码

```rust
fn main() {
    println!("你好，墨笺");
}
```

== 表格

#table(
  columns: 3,
  [项目], [状态], [说明],
  [PDF], [通过], [Typst 0.15],
  [PNG], [通过], [typst-render],
  [中文], [验证中], [系统字体],
)
"#;

fn main() -> anyhow::Result<()> {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/m0");
    std::fs::create_dir_all(&out_dir)?;

    let pdf = export_pdf_source(SOURCE)?;
    let pdf_path = out_dir.join("m0-export.pdf");
    std::fs::write(&pdf_path, &pdf)?;
    println!("PDF: {} ({} bytes)", pdf_path.display(), pdf.len());

    let png = export_png(SOURCE, 144)?;
    let png_path = out_dir.join("m0-export.png");
    std::fs::write(&png_path, &png)?;
    println!("PNG: {} ({} bytes)", png_path.display(), png.len());

    Ok(())
}
