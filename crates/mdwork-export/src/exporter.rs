//! Export orchestration shared by the CLI and the GUI subprocess.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use mdwork_core::notebook::Notebook;
use mdwork_render::preview::PreviewIr;
use mdwork_render::typst::{self, DocMeta, ExportOptions};
use serde::{Deserialize, Serialize};

use crate::pdf;
use crate::png::{self, Stitcher, max_raster_height};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    Pdf,
    Png,
    Webp,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Pdf => "pdf",
            ExportFormat::Png => "png",
            ExportFormat::Webp => "webp",
        }
    }

    pub fn is_raster(self) -> bool {
        matches!(self, ExportFormat::Png | ExportFormat::Webp)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRequest {
    pub notebook_root: PathBuf,
    pub note_rel: String,
    #[serde(default)]
    pub notebook_name: String,
    #[serde(default)]
    pub title: String,
    pub format: ExportFormat,
    pub output: PathBuf,
    #[serde(default)]
    pub options: ExportOptions,
    /// When set, exports this text instead of reading the note from disk
    /// (used for unsaved GUI buffers).
    #[serde(default)]
    pub markdown: Option<String>,
    /// Optional working directory used for temporary output. The GUI passes a
    /// directory next to the output so it can clean up after canceling.
    #[serde(default)]
    pub temp_dir: Option<PathBuf>,
}

impl ExportRequest {
    /// Validates fields coming from `request.json`, which may be edited by
    /// another process or produced by a mismatched CLI/GUI version.
    pub fn validate(&self) -> Result<()> {
        if self.note_rel.trim().is_empty() {
            bail!("导出请求缺少笔记路径");
        }
        if self.output.file_name().is_none() {
            bail!("导出请求缺少输出文件名");
        }
        let extension = self
            .output
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension != self.format.extension() {
            bail!(
                "输出扩展名 .{} 与导出格式 {} 不一致",
                extension,
                self.format.extension()
            );
        }
        let options = &self.options;
        let finite = |name: &str, value: f32, min: f32, max: f32| -> Result<()> {
            if !value.is_finite() || !(min..=max).contains(&value) {
                bail!("导出参数 {name} 超出范围");
            }
            Ok(())
        };
        finite("纸张宽度", options.custom_width_mm, 10.0, 2_000.0)?;
        finite("纸张高度", options.custom_height_mm, 10.0, 2_000.0)?;
        finite("上边距", options.margin_top_mm, 0.0, 300.0)?;
        finite("下边距", options.margin_bottom_mm, 0.0, 300.0)?;
        finite("左边距", options.margin_left_mm, 0.0, 300.0)?;
        finite("右边距", options.margin_right_mm, 0.0, 300.0)?;
        finite("正文字号", options.font_size_pt, 4.0, 96.0)?;
        finite("行距", options.line_height, 0.5, 4.0)?;
        finite("代码字号", options.code_size_pt, 4.0, 96.0)?;
        if !(1..=100).contains(&options.image_max_width_pct) {
            bail!("导出参数图片宽度百分比超出范围");
        }
        if !(36..=1_200).contains(&options.png_dpi) {
            bail!("导出参数 DPI 超出范围");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ExportEvent {
    Stage { stage: String },
    Page { page: usize, pages: usize },
    Warning { message: String },
    Done { paths: Vec<String>, pages: usize },
    Error { message: String },
}

#[derive(Debug, Clone)]
pub struct ExportOutcome {
    pub paths: Vec<PathBuf>,
    pub pages: usize,
}

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn create(path: PathBuf) -> Result<Self> {
        fs::create_dir_all(&path)
            .with_context(|| format!("无法创建临时目录 {}", path.display()))?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn run(request: &ExportRequest, emit: &mut dyn FnMut(ExportEvent)) -> Result<ExportOutcome> {
    tracing::info!(
        notebook = %request.notebook_name,
        note = %request.note_rel,
        format = ?request.format,
        output = %request.output.display(),
        "开始导出"
    );
    let result = run_inner(request, emit);
    match &result {
        Ok(outcome) => tracing::info!(
            format = ?request.format,
            pages = outcome.pages,
            files = outcome.paths.len(),
            "导出完成"
        ),
        Err(err) => tracing::error!(format = ?request.format, error = %err, "导出失败"),
    }
    result
}

fn run_inner(request: &ExportRequest, emit: &mut dyn FnMut(ExportEvent)) -> Result<ExportOutcome> {
    request.validate()?;
    emit(ExportEvent::Stage {
        stage: "read".to_string(),
    });
    let (markdown, root) = load_markdown(request)?;
    let ir = PreviewIr::from_markdown(&markdown);

    let title = if !request.title.trim().is_empty() {
        request.title.trim().to_string()
    } else {
        ir.outline()
            .first()
            .map(|item| item.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| {
                Path::new(&request.note_rel)
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "未命名笔记".to_string())
            })
    };
    let meta = DocMeta {
        title,
        notebook: request.notebook_name.clone(),
        note_rel: request.note_rel.replace('\\', "/"),
        date: typst::today_iso(),
    };

    emit(ExportEvent::Stage {
        stage: "compile".to_string(),
    });
    // Long raster exports use height: auto so the note becomes one continuous
    // image instead of a single clipped A4 page.
    let continuous = request.format.is_raster() && !request.options.png_page_files;
    let document = typst::convert_with(&ir, &request.options, &meta, &root, continuous);
    for warning in &document.warnings {
        emit(ExportEvent::Warning {
            message: warning.clone(),
        });
    }
    let compiled = pdf::compile_typst(&document)?;
    let pages = compiled.pages().len();

    let temp_path = request.temp_dir.clone().unwrap_or_else(|| {
        let parent = request
            .output
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        parent.join(format!(
            ".mdwork-export-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ))
    });
    let temp = TempDir::create(temp_path)?;

    let paths = match request.format {
        ExportFormat::Pdf => {
            emit(ExportEvent::Stage {
                stage: "write".to_string(),
            });
            let bytes = pdf::export_pdf_pages(&compiled)?;
            let staged = temp.path().join("out.pdf");
            fs::write(&staged, &bytes)?;
            let output = ensure_parent(&request.output)?;
            replace_file(&staged, &output)?;
            emit(ExportEvent::Page { page: pages, pages });
            emit(ExportEvent::Done {
                paths: vec![output.display().to_string()],
                pages,
            });
            vec![output]
        }
        format @ (ExportFormat::Png | ExportFormat::Webp) => {
            emit(ExportEvent::Stage {
                stage: "render".to_string(),
            });
            let dpi = request.options.png_dpi.clamp(36, 600);
            let gap = ((12.0 * f64::from(dpi) / 72.0).round() as u32).max(1);
            let max_height = max_raster_height(format);
            let output = ensure_raster_extension(&ensure_parent(&request.output)?, format);
            if request.options.png_page_files {
                let mut staged = Vec::new();
                for (index, page) in compiled.pages().iter().enumerate() {
                    let image = png::render_page(page, dpi);
                    let file =
                        temp.path()
                            .join(format!("page-{:03}.{}", index + 1, format.extension()));
                    fs::write(&file, png::encode_page(&image, format)?)?;
                    staged.push(file);
                    emit(ExportEvent::Page {
                        page: index + 1,
                        pages,
                    });
                }
                let targets = numbered_targets(&output, staged.len(), true);
                move_files(&staged, &targets)?;
                emit(ExportEvent::Done {
                    paths: targets
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                    pages,
                });
                return Ok(ExportOutcome {
                    paths: targets,
                    pages,
                });
            }

            let mut stitcher = Stitcher::new(max_height, gap);
            let mut staged = Vec::new();
            for (index, page) in compiled.pages().iter().enumerate() {
                let image = png::render_page(page, dpi);
                for band in png::split_tall(image, max_height) {
                    if let Some(chunk) = stitcher.push(band) {
                        let file = temp.path().join(format!(
                            "chunk-{:03}.{}",
                            staged.len() + 1,
                            format.extension()
                        ));
                        fs::write(&file, png::encode_page(&chunk, format)?)?;
                        staged.push(file);
                    }
                }
                emit(ExportEvent::Page {
                    page: index + 1,
                    pages,
                });
            }
            if let Some(chunk) = stitcher.finish() {
                let file = temp.path().join(format!(
                    "chunk-{:03}.{}",
                    staged.len() + 1,
                    format.extension()
                ));
                fs::write(&file, png::encode_page(&chunk, format)?)?;
                staged.push(file);
            }
            if staged.is_empty() {
                return Err(anyhow!("没有可导出的页面"));
            }
            let targets = numbered_targets(&output, staged.len(), false);
            move_files(&staged, &targets)?;
            emit(ExportEvent::Done {
                paths: targets
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect(),
                pages,
            });
            targets
        }
    };

    drop(temp);
    Ok(ExportOutcome { paths, pages })
}

fn load_markdown(request: &ExportRequest) -> Result<(String, PathBuf)> {
    if let Some(markdown) = &request.markdown {
        return Ok((markdown.clone(), request.notebook_root.clone()));
    }
    let notebook = Notebook::open(&request.notebook_root)
        .with_context(|| format!("无法打开笔记本 {}", request.notebook_root.display()))?;
    let doc = notebook.read_note(&request.note_rel)?;
    Ok((doc.text, notebook.root))
}

fn ensure_parent(path: &Path) -> Result<PathBuf> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建导出目录 {}", parent.display()))?;
    }
    Ok(path.to_path_buf())
}

fn ensure_raster_extension(path: &Path, format: ExportFormat) -> PathBuf {
    let extension = format.extension();
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|current| current.eq_ignore_ascii_case(extension))
    {
        path.to_path_buf()
    } else {
        path.with_extension(extension)
    }
}

fn numbered_targets(output: &Path, count: usize, always_numbered: bool) -> Vec<PathBuf> {
    if count <= 1 && !always_numbered {
        return vec![output.to_path_buf()];
    }
    let stem = output
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".to_string());
    let extension = output
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("png");
    let parent = output.parent().unwrap_or_else(|| Path::new(""));
    (1..=count.max(1))
        .map(|index| parent.join(format!("{stem}-{index:03}.{extension}")))
        .collect()
}

fn move_files(staged: &[PathBuf], targets: &[PathBuf]) -> Result<()> {
    for (source, target) in staged.iter().zip(targets) {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        replace_file(source, target)?;
    }
    Ok(())
}

/// Atomically replace `target` with `source`. Removing the old file first can
/// leave the user with no output at all when the rename then fails.
fn replace_file(source: &Path, target: &Path) -> Result<()> {
    mdwork_core::atomic::replace_atomic(source, target)
        .with_context(|| format!("无法写入 {}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ExportRequest {
        ExportRequest {
            notebook_root: PathBuf::from("."),
            note_rel: "笔记.md".to_string(),
            notebook_name: String::new(),
            title: String::new(),
            format: ExportFormat::Pdf,
            output: PathBuf::from("out.pdf"),
            options: ExportOptions::default(),
            markdown: None,
            temp_dir: None,
        }
    }

    #[test]
    fn validates_output_extension() {
        let mut request = request();
        assert!(request.validate().is_ok());
        request.output.set_extension("txt");
        assert!(request.validate().is_err());
    }

    #[test]
    fn validates_numeric_options() {
        let mut zero_dpi = request();
        zero_dpi.options.png_dpi = 0;
        assert!(zero_dpi.validate().is_err());

        let mut zero_width = request();
        zero_width.options.image_max_width_pct = 0;
        assert!(zero_width.validate().is_err());
    }
}
