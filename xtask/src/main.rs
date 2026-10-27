use std::path::PathBuf;

use anyhow::{Context, Result};
use resvg::{tiny_skia, usvg};

const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("icon") => icon(),
        _ => {
            println!("usage: xtask icon");
            Ok(())
        }
    }
}

fn icon() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask must live inside the workspace")?
        .to_path_buf();
    let svg_path = root.join("assets").join("icon.svg");
    let svg = std::fs::read(&svg_path)
        .with_context(|| format!("read {}", svg_path.display()))?;

    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(&svg, &options)
        .with_context(|| format!("parse {}", svg_path.display()))?;

    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in SIZES {
        let mut pixmap = tiny_skia::Pixmap::new(size, size)
            .with_context(|| format!("allocate {size}x{size} pixmap"))?;
        let scale = size as f32 / tree.size().width().max(tree.size().height());
        let transform = tiny_skia::Transform::from_scale(scale, scale);
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let image = ico::IconImage::from_rgba_data(size, size, pixmap.data().to_vec());
        icon_dir
            .add_entry(ico::IconDirEntry::encode(&image)?);
    }

    let ico_path = root.join("assets").join("icon.ico");
    let file = std::fs::File::create(&ico_path)
        .with_context(|| format!("create {}", ico_path.display()))?;
    icon_dir.write(file)?;
    println!("wrote {} ({} sizes)", ico_path.display(), SIZES.len());
    Ok(())
}
