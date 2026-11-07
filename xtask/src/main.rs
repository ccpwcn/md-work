use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use resvg::{tiny_skia, usvg};

const APP_ICON_SIZES: [u32; 5] = [16, 32, 64, 128, 256];
const INSTALLER_ICON_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("icon") => icon(),
        Some("package") => package(),
        Some("installer") => installer(),
        Some("all") => {
            package()?;
            installer()
        }
        _ => {
            println!("usage: xtask <icon|package|installer|all>");
            println!("  icon       regenerate app and installer .ico files from assets/*.svg");
            println!("  package    release build + dist/md-work-{VERSION}-win64(.zip)");
            println!("  installer  compile the per-user NSIS installer (needs makensis)");
            println!("  all        package then installer");
            Ok(())
        }
    }
}

fn workspace_root() -> Result<PathBuf> {
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .context("xtask must live inside the workspace")?
        .to_path_buf())
}

fn bundle_dir(root: &Path) -> PathBuf {
    root.join("dist").join(format!("md-work-{VERSION}-win64"))
}

fn parse_icon(svg: &[u8]) -> Result<usvg::Tree> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    usvg::Tree::from_data(svg, &options).context("parse icon svg")
}

fn render_icon(tree: &usvg::Tree, size: u32) -> Result<tiny_skia::Pixmap> {
    let mut pixmap = tiny_skia::Pixmap::new(size, size)
        .with_context(|| format!("allocate {size}x{size} pixmap"))?;
    // ICO 帧是正方形；竖版画板居中放置，两侧留白。
    let width = tree.size().width();
    let height = tree.size().height();
    let scale = size as f32 / width.max(height);
    let tx = (size as f32 - width * scale) / 2.0;
    let ty = (size as f32 - height * scale) / 2.0;
    resvg::render(
        tree,
        tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, tx, ty),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

fn write_ico(tree: &usvg::Tree, ico_path: &Path, sizes: &[u32]) -> Result<()> {
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in sizes {
        let pixmap = render_icon(tree, size)?;
        // ICO bitmaps store straight alpha. Demultiply tiny-skia's pixels so the
        // white contour stays white along antialiased edges.
        let image = ico::IconImage::from_rgba_data(size, size, pixmap.take_demultiplied());
        icon_dir.add_entry(ico::IconDirEntry::encode(&image)?);
    }
    let file =
        fs::File::create(ico_path).with_context(|| format!("create {}", ico_path.display()))?;
    icon_dir.write(file)?;
    println!("wrote {} ({sizes:?})", ico_path.display());
    Ok(())
}

fn icon() -> Result<()> {
    let root = workspace_root()?;
    for (svg_name, ico_name, sizes) in [
        ("icon.svg", "icon.ico", APP_ICON_SIZES.as_slice()),
        (
            "installer-icon.svg",
            "installer-icon.ico",
            INSTALLER_ICON_SIZES.as_slice(),
        ),
    ] {
        let svg_path = root.join("assets").join(svg_name);
        let svg = fs::read(&svg_path).with_context(|| format!("read {}", svg_path.display()))?;
        write_ico(
            &parse_icon(&svg)?,
            &root.join("assets").join(ico_name),
            sizes,
        )?;
    }
    Ok(())
}

fn build_release(root: &Path) -> Result<()> {
    println!("building release binaries…");
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "md-work-gui",
            "-p",
            "md-work-cli",
        ])
        .current_dir(root)
        .status()
        .context("spawn cargo build")?;
    if !status.success() {
        bail!("cargo build --release failed with {status}");
    }
    Ok(())
}

fn copy_binaries(root: &Path, bundle: &Path) -> Result<()> {
    let release = root.join("target").join("release");
    for name in ["md-work.exe", "md-work-cli.exe"] {
        let source = release.join(name);
        if !source.exists() {
            bail!("missing {}; run the release build first", source.display());
        }
        fs::copy(&source, bundle.join(name))
            .with_context(|| format!("copy {}", source.display()))?;
    }
    for name in ["LICENSE", "README.md", "THIRD-PARTY-NOTICES.md"] {
        let source = root.join(name);
        if source.exists() {
            fs::copy(&source, bundle.join(name))
                .with_context(|| format!("copy {}", source.display()))?;
        }
    }
    Ok(())
}

fn zip_bundle(bundle: &Path) -> Result<PathBuf> {
    let name = bundle
        .file_name()
        .context("bundle dir has no name")?
        .to_string_lossy()
        .into_owned();
    // `with_extension` would truncate "1.0.0-win64" at the last dot.
    let zip_path = bundle.with_file_name(format!("{name}.zip"));
    let file = fs::File::create(&zip_path)?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let prefix = bundle
        .file_name()
        .context("bundle dir has no name")?
        .to_string_lossy()
        .into_owned();
    for entry in fs::read_dir(bundle)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        writer.start_file(format!("{prefix}/{name}"), options)?;
        let mut source = fs::File::open(entry.path())?;
        std::io::copy(&mut source, &mut writer)?;
    }
    writer.finish()?;
    Ok(zip_path)
}

fn package() -> Result<()> {
    let root = workspace_root()?;
    build_release(&root)?;
    let bundle = bundle_dir(&root);
    if bundle.exists() {
        fs::remove_dir_all(&bundle).with_context(|| format!("clean {}", bundle.display()))?;
    }
    fs::create_dir_all(&bundle)?;
    copy_binaries(&root, &bundle)?;
    let zip = zip_bundle(&bundle)?;
    println!("portable bundle: {}", bundle.display());
    println!("zip:            {}", zip.display());
    Ok(())
}

fn find_makensis() -> Option<PathBuf> {
    if Command::new("makensis")
        .arg("/VERSION")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        return Some(PathBuf::from("makensis"));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("NSIS_HOME") {
        candidates.push(PathBuf::from(home).join("makensis.exe"));
    }
    candidates.extend(
        [
            r"E:\apps\nsis\makensis.exe",
            r"C:\Program Files (x86)\NSIS\makensis.exe",
            r"C:\Program Files\NSIS\makensis.exe",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    candidates.into_iter().find(|path| path.is_file())
}

fn installer() -> Result<()> {
    let root = workspace_root()?;
    let bundle = bundle_dir(&root);
    if !bundle.join("md-work.exe").exists() {
        bail!(
            "run `cargo xtask package` first (missing {})",
            bundle.display()
        );
    }
    let Some(makensis) = find_makensis() else {
        bail!(
            "makensis not found. Install NSIS 3 (https://nsis.sourceforge.io/) \
             and re-run `cargo xtask installer`, or compile \
             packaging/nsis/md-work.nsi manually with \
             /DVERSION={VERSION} /DDIST={}",
            bundle.display()
        );
    };
    let script = root.join("packaging").join("nsis").join("md-work.nsi");
    let status = Command::new(makensis)
        .arg("/V2")
        .arg(format!("/DVERSION={VERSION}"))
        .arg(format!("/DDIST={}", bundle.display()))
        .arg(&script)
        .current_dir(root.join("packaging").join("nsis"))
        .status()
        .context("spawn makensis")?;
    if !status.success() {
        bail!("makensis failed with {status}");
    }
    println!(
        "installer: {}",
        root.join("dist")
            .join(format!("md-work-{VERSION}-setup.exe"))
            .display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_reads_against_its_own_blue() {
        let root = workspace_root().expect("workspace root");
        let svg = fs::read(root.join("assets").join("icon.svg")).expect("read icon.svg");
        let pixmap = render_icon(&parse_icon(&svg).expect("parse icon"), 32).expect("render icon");
        let corner = pixmap.pixel(0, 0).expect("corner pixel");
        assert_eq!(corner.alpha(), 0, "icon keeps a transparent margin");

        let background = [0x12_u8, 0x96, 0xdb];
        let mut separated = 0_u32;
        let mut blue = 0_u32;
        for pixel in pixmap.pixels() {
            if pixel.alpha() < 200 {
                continue;
            }
            let color = pixel.demultiply();
            let distance = (i32::from(color.red()) - i32::from(background[0])).unsigned_abs()
                + (i32::from(color.green()) - i32::from(background[1])).unsigned_abs()
                + (i32::from(color.blue()) - i32::from(background[2])).unsigned_abs();
            if distance > 90 {
                separated += 1;
            }
            if color.blue() > color.red().saturating_add(40) && color.blue() > 100 {
                blue += 1;
            }
        }
        assert!(
            separated > 80,
            "white contour is missing against #1296db ({separated} pixels)"
        );
        assert!(blue > 40, "blue glyph is missing ({blue} pixels)");
    }

    #[test]
    fn installer_icon_is_a_carton_holding_the_app_mark() {
        let root = workspace_root().expect("workspace root");
        let svg = fs::read(root.join("assets").join("installer-icon.svg")).expect("read svg");
        let pixmap = render_icon(&parse_icon(&svg).expect("parse icon"), 64).expect("render icon");
        let corner = pixmap.pixel(0, 0).expect("corner pixel");
        assert_eq!(
            corner.alpha(),
            0,
            "installer icon keeps a transparent margin"
        );

        let mut carton = 0_u32;
        let mut blue = 0_u32;
        for pixel in pixmap.pixels() {
            if pixel.alpha() < 200 {
                continue;
            }
            let color = pixel.demultiply();
            let (red, green, blue_channel) = (color.red(), color.green(), color.blue());
            if red > green
                && green > blue_channel
                && red > 150
                && u16::from(red) > u16::from(blue_channel) + 50
            {
                carton += 1;
            }
            if blue_channel > red.saturating_add(40) && blue_channel > 100 {
                blue += 1;
            }
        }
        assert!(carton > 400, "carton is missing ({carton} pixels)");
        assert!(
            blue > 20,
            "app mark is missing from the carton ({blue} pixels)"
        );
    }
}
