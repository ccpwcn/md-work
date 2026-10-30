use anyhow::{Result, anyhow};
use mdwork_render::typst::TypstDoc;
use typst::WorldExt;
use typst::compile;
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;

use crate::world::ExportWorld;

/// Compiles a standalone Typst snippet (used by the math preview).
pub fn compile_document(source: &str) -> Result<PagedDocument> {
    let world = ExportWorld::new(source);
    compile::<PagedDocument>(&world)
        .output
        .map_err(|errors| anyhow!("Typst compile failed: {errors:?}"))
}

/// Compiles an exported Markdown document and maps diagnostics back to
/// Markdown line numbers where possible.
pub fn compile_typst(doc: &TypstDoc) -> Result<PagedDocument> {
    let world = ExportWorld::from_doc(doc);
    compile::<PagedDocument>(&world)
        .output
        .map_err(|errors| anyhow!("{}", format_diagnostics(&world, doc, &errors)))
}

fn format_diagnostics(
    world: &ExportWorld,
    doc: &TypstDoc,
    errors: &[typst::diag::SourceDiagnostic],
) -> String {
    let source = world.source_text();
    let mut message = String::new();
    for (index, error) in errors.iter().enumerate() {
        if index > 0 {
            message.push('\n');
        }
        let typst_line = world.range(error.span).map(|range| {
            1 + source
                .as_bytes()
                .iter()
                .take(range.start)
                .filter(|byte| **byte == b'\n')
                .count()
        });
        match typst_line {
            Some(line) => {
                let markdown = doc.markdown_line(line);
                match markdown {
                    Some(markdown) => {
                        message.push_str(&format!(
                            "Typst 第 {line} 行（Markdown 第 {markdown} 行）：{}",
                            error.message
                        ));
                    }
                    None => message.push_str(&format!("Typst 第 {line} 行：{}", error.message)),
                }
            }
            None => message.push_str(&error.message),
        }
        for hint in &error.hints {
            message.push_str(&format!("\n提示：{}", hint.v));
        }
    }
    if message.is_empty() {
        "Typst compile failed".to_string()
    } else {
        message
    }
}

pub fn export_pdf(doc: &TypstDoc) -> Result<Vec<u8>> {
    let document = compile_typst(doc)?;
    export_pdf_pages(&document)
}

/// Writes an already compiled document to PDF bytes, so callers that need the
/// page count do not have to compile the source twice.
pub fn export_pdf_pages(document: &PagedDocument) -> Result<Vec<u8>> {
    typst_pdf::pdf(document, &PdfOptions::default())
        .map_err(|errors| anyhow!("PDF export failed: {errors:?}"))
}

pub fn export_pdf_source(source: &str) -> Result<Vec<u8>> {
    let document = compile_document(source)?;
    typst_pdf::pdf(&document, &PdfOptions::default())
        .map_err(|errors| anyhow!("PDF export failed: {errors:?}"))
}
