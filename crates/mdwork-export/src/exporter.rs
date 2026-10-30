//! Export orchestration shared by the CLI and the GUI subprocess.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use mdwork_core::notebook::Notebook;
use mdwork_render::preview::PreviewIr;
use mdwork_render::typst::{self, DocMeta, ExportOptions};
use serde::{Deserialize, Serialize};

use crate::pdf;
use crate::png::{self, MAX_PNG_HEIGHT, Stitcher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportFormat {
    Pdf,
    Png,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Pdf => "pdf",
            ExportFormat::Png => "png",
        }
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
    let document = typst::convert(&ir, &request.options, &meta, &root);
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
            fs::rename(&staged, &output)?;
            emit(ExportEvent::Page { page: pages, pages });
            emit(ExportEvent::Done {
                paths: vec![output.display().to_string()],
                pages,
            });
            vec![output]
        }
        ExportFormat::Png => {
            emit(ExportEvent::Stage {
                stage: "render".to_string(),
            });
            let dpi = request.options.png_dpi.clamp(36, 600);
            let gap = ((12.0 * f64::from(dpi) / 72.0).round() as u32).max(1);
            let output = ensure_png_extension(&ensure_parent(&request.output)?);
            if request.options.png_page_files {
                let mut staged = Vec::new();
                for (index, page) in compiled.pages().iter().enumerate() {
                    let image = png::render_page(page, dpi);
                    let file = temp.path().join(format!("page-{:03}.png", index + 1));
                    fs::write(&file, png::encode_png(&image)?)?;
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

            let mut stitcher = Stitcher::new(MAX_PNG_HEIGHT, gap);
            let mut staged = Vec::new();
            for (index, page) in compiled.pages().iter().enumerate() {
                let image = png::render_page(page, dpi);
                if let Some(chunk) = stitcher.push(image) {
                    let file = temp
                        .path()
                        .join(format!("chunk-{:03}.png", staged.len() + 1));
                    fs::write(&file, png::encode_png(&chunk)?)?;
                    staged.push(file);
                }
                emit(ExportEvent::Page {
                    page: index + 1,
                    pages,
                });
            }
            if let Some(chunk) = stitcher.finish() {
                let file = temp
                    .path()
                    .join(format!("chunk-{:03}.png", staged.len() + 1));
                fs::write(&file, png::encode_png(&chunk)?)?;
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
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建导出目录 {}", parent.display()))?;
        }
    }
    Ok(path.to_path_buf())
}

fn ensure_png_extension(path: &Path) -> PathBuf {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        path.to_path_buf()
    } else {
        path.with_extension("png")
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
    let parent = output.parent().unwrap_or_else(|| Path::new(""));
    (1..=count.max(1))
        .map(|index| parent.join(format!("{stem}-{index:03}.png")))
        .collect()
}

fn move_files(staged: &[PathBuf], targets: &[PathBuf]) -> Result<()> {
    for (source, target) in staged.iter().zip(targets) {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(source, target).with_context(|| format!("无法写入 {}", target.display()))?;
    }
    Ok(())
}
