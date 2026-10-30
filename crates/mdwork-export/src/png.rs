//! PNG rendering and long-image stitching.

use std::io::Cursor;

use anyhow::{Result, anyhow};
use image::{ImageFormat, RgbaImage};
use typst::layout::Abs;
use typst::utils::Scalar;
use typst_layout::Page;
use typst_render::{RenderOptions, render, render_merged};

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
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
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
    let image = RgbaImage::from_raw(page.width, page.height, page.data.clone())
        .ok_or_else(|| anyhow!("invalid pixmap buffer"))?;
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image).write_to(&mut out, ImageFormat::Png)?;
    Ok(out.into_inner())
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
        if widened || self.buffer.is_empty() {
            self.width = self.width.max(page.width);
            self.capacity = self.max_height.max(page.height);
            self.buffer = vec![255; self.width as usize * self.capacity as usize * 4];
            self.used = 0;
            self.has_page = false;
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
        self.blit(&page, y);
        self.used = y + page.height;
        self.has_page = true;
        finished
    }

    pub fn finish(&mut self) -> Option<PageImage> {
        self.take_chunk()
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
    fn encodes_single_page() {
        let bytes = encode_png(&page(2, 2, 128)).unwrap();
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    }
}
