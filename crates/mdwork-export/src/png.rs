//! Raster rendering, long-image stitching, and PNG/WebP encoding.

use std::io::Cursor;

use anyhow::{Result, anyhow};
use image::{ImageEncoder, ImageFormat, RgbaImage};
use typst::layout::Abs;
use typst::utils::Scalar;
use typst_layout::Page;
use typst_render::{RenderOptions, render, render_merged};

use crate::exporter::ExportFormat;
use crate::pdf::compile_document;

/// Renders a standalone Typst snippet to one merged PNG (used by examples).
pub fn export_png(source: &str, dpi: u32) -> Result<Vec<u8>> {
    let document = compile_document(source)?;
    let options = RenderOptions {
        pixel_per_pt: Scalar::new(f64::from(dpi.clamp(36, 600)) / 72.0),
        ..Default::default()
    };
    let pixmap = render_merged(
        &document,
        &options,
        Abs::pt(12.0),
        Some(typst::visualize::Color::WHITE),
    );
    let image = RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixmap.data().to_vec())
        .ok_or_else(|| anyhow!("invalid pixmap buffer"))?;
    encode_rgba(&image, ExportFormat::Png)
}

/// A rendered page as RGBA rows.
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub fn render_page(page: &Page, dpi: u32) -> PageImage {
    let dpi = dpi.clamp(36, 600);
    let options = RenderOptions {
        pixel_per_pt: Scalar::new(f64::from(dpi) / 72.0),
        ..Default::default()
    };
    let pixmap = render(page, &options);
    PageImage {
        width: pixmap.width(),
        height: pixmap.height(),
        data: pixmap.data().to_vec(),
    }
}

pub fn encode_png(page: &PageImage) -> Result<Vec<u8>> {
    encode_page(page, ExportFormat::Png)
}

pub fn encode_page(page: &PageImage, format: ExportFormat) -> Result<Vec<u8>> {
    if page.width == 0 || page.height == 0 {
        return Err(anyhow!("图片尺寸无效（{}×{}）", page.width, page.height));
    }
    if format == ExportFormat::Webp && (page.width > MAX_WEBP_EDGE || page.height > MAX_WEBP_EDGE) {
        return Err(anyhow!(
            "WebP 单张图片不能超过 {MAX_WEBP_EDGE} 像素（当前 {}×{}），请勾选「每页一个文件」或改用 PNG",
            page.width,
            page.height
        ));
    }
    let image = RgbaImage::from_raw(page.width, page.height, page.data.clone())
        .ok_or_else(|| anyhow!("无效的图片缓冲区"))?;
    encode_rgba(&image, format)
}

fn encode_rgba(image: &RgbaImage, format: ExportFormat) -> Result<Vec<u8>> {
    let mut out = Cursor::new(Vec::new());
    match format {
        ExportFormat::Png => {
            image::DynamicImage::ImageRgba8(image.clone()).write_to(&mut out, ImageFormat::Png)?;
        }
        ExportFormat::Webp => {
            // image 0.25 ships lossless WebP only; still far smaller than PNG
            // for document pages with large flat backgrounds. Catch panics from
            // the encoder so the GUI never sees raw OS backtrace frames.
            let width = image.width();
            let height = image.height();
            let pixels = image.as_raw().to_vec();
            let encoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut buffer = Cursor::new(Vec::new());
                let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut buffer);
                encoder.write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)?;
                Ok::<Vec<u8>, image::ImageError>(buffer.into_inner())
            }));
            match encoded {
                Ok(Ok(bytes)) => return Ok(bytes),
                Ok(Err(err)) => {
                    return Err(anyhow!("WebP 编码失败：{err}"));
                }
                Err(_) => {
                    return Err(anyhow!(
                        "WebP 编码异常中断。请降低 DPI、勾选「每页一个文件」，或改用 PNG"
                    ));
                }
            }
        }
        ExportFormat::Pdf => {
            return Err(anyhow!("PDF 不是位图导出格式"));
        }
    }
    Ok(out.into_inner())
}

/// Splits an image taller than `max_height` into vertical bands.
pub fn split_tall(page: PageImage, max_height: u32) -> Vec<PageImage> {
    let max_height = max_height.max(1);
    if page.height <= max_height {
        return vec![page];
    }
    let mut bands = Vec::new();
    let row_bytes = page.width as usize * 4;
    let mut y = 0u32;
    while y < page.height {
        let height = (page.height - y).min(max_height);
        let start = y as usize * row_bytes;
        let end = (y + height) as usize * row_bytes;
        bands.push(PageImage {
            width: page.width,
            height,
            data: page.data[start..end].to_vec(),
        });
        y += height;
    }
    bands
}

/// Stacks rendered pages into chunks no taller than `max_height` pixels.
pub struct Stitcher {
    width: u32,
    used: u32,
    max_height: u32,
    capacity: u32,
    gap: u32,
    buffer: Vec<u8>,
    has_page: bool,
}

impl Stitcher {
    pub fn new(max_height: u32, gap: u32) -> Self {
        Self {
            width: 0,
            used: 0,
            max_height: max_height.max(1),
            capacity: 0,
            gap,
            buffer: Vec::new(),
            has_page: false,
        }
    }

    /// Adds a page, returning a completed chunk when the next page no longer
    /// fits within the height limit.
    pub fn push(&mut self, page: PageImage) -> Option<PageImage> {
        if page.width == 0 || page.height == 0 {
            return None;
        }
        let widened = page.width > self.width;
        if self.buffer.is_empty() {
            self.reset_buffer(page.width.max(1), page.height);
        } else if widened {
            // Preserve what we already stitched before growing the canvas.
            let finished = self.take_chunk();
            self.reset_buffer(page.width, page.height);
            let y = 0;
            self.blit(&page, y);
            self.used = page.height;
            self.has_page = true;
            return finished;
        }
        if page.height > self.capacity {
            self.capacity = page.height;
            self.buffer
                .resize(self.width as usize * self.capacity as usize * 4, 255);
        }
        let offset = if self.has_page { self.gap } else { 0 };
        let needed = self.used as u64 + u64::from(offset) + u64::from(page.height);
        let finished = if self.has_page && needed > u64::from(self.max_height) {
            self.take_chunk()
        } else {
            None
        };
        let y = self.used + if self.has_page { self.gap } else { 0 };
        // After flushing a chunk, capacity may still be max_height; grow if needed.
        if y + page.height > self.capacity {
            self.capacity = y + page.height;
            self.buffer
                .resize(self.width as usize * self.capacity as usize * 4, 255);
        }
        self.blit(&page, y);
        self.used = y + page.height;
        self.has_page = true;
        finished
    }

    pub fn finish(&mut self) -> Option<PageImage> {
        self.take_chunk()
    }

    fn reset_buffer(&mut self, width: u32, first_page_height: u32) {
        self.width = width;
        self.capacity = self.max_height.max(first_page_height);
        self.buffer = vec![255; self.width as usize * self.capacity as usize * 4];
        self.used = 0;
        self.has_page = false;
    }

    fn take_chunk(&mut self) -> Option<PageImage> {
        if !self.has_page {
            return None;
        }
        let height = self.used;
        let len = self.width as usize * height as usize * 4;
        let page = PageImage {
            width: self.width,
            height,
            data: self.buffer[..len].to_vec(),
        };
        self.used = 0;
        self.has_page = false;
        Some(page)
    }

    fn blit(&mut self, page: &PageImage, y: u32) {
        let row_bytes = page.width as usize * 4;
        let stride = self.width as usize * 4;
        for row in 0..page.height as usize {
            let src = row * row_bytes;
            let dst = (y as usize + row) * stride;
            let dest = &mut self.buffer[dst..dst + row_bytes];
            dest.copy_from_slice(&page.data[src..src + row_bytes]);
        }
    }
}

pub const MAX_PNG_HEIGHT: u32 = 16_384;
/// WebP VP8L practical edge limit used when splitting long images.
pub const MAX_WEBP_EDGE: u32 = 16_383;

pub fn max_raster_height(format: ExportFormat) -> u32 {
    match format {
        ExportFormat::Webp => MAX_WEBP_EDGE,
        ExportFormat::Png | ExportFormat::Pdf => MAX_PNG_HEIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(width: u32, height: u32, color: u8) -> PageImage {
        PageImage {
            width,
            height,
            data: vec![color; (width * height * 4) as usize],
        }
    }

    #[test]
    fn stitches_pages_with_gap() {
        let mut stitcher = Stitcher::new(100, 2);
        assert!(stitcher.push(page(10, 40, 1)).is_none());
        assert!(stitcher.push(page(10, 40, 2)).is_none());
        let chunk = stitcher.push(page(10, 40, 3)).expect("chunk");
        assert_eq!(chunk.width, 10);
        assert_eq!(chunk.height, 82);
        let last = stitcher.finish().expect("tail");
        assert_eq!(last.height, 40);
    }

    #[test]
    fn widened_page_flushes_previous_chunk() {
        let mut stitcher = Stitcher::new(200, 0);
        assert!(stitcher.push(page(10, 40, 1)).is_none());
        let flushed = stitcher
            .push(page(20, 40, 2))
            .expect("flushed narrow pages");
        assert_eq!(flushed.width, 10);
        assert_eq!(flushed.height, 40);
        let last = stitcher.finish().expect("wide page");
        assert_eq!(last.width, 20);
        assert_eq!(last.height, 40);
    }

    #[test]
    fn split_tall_respects_max_height() {
        let bands = split_tall(page(4, 50, 9), 20);
        assert_eq!(bands.len(), 3);
        assert_eq!(bands[0].height, 20);
        assert_eq!(bands[1].height, 20);
        assert_eq!(bands[2].height, 10);
    }

    #[test]
    fn encodes_single_page() {
        let bytes = encode_png(&page(2, 2, 128)).unwrap();
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn encodes_webp() {
        let bytes = encode_page(&page(2, 2, 128), ExportFormat::Webp).unwrap();
        assert!(bytes.len() > 12);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
    }
}
