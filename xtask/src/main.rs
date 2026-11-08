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

/// 随包发布的官方说明文档：打包后的文件名 + 仓库内的源文件。
/// 两个包（安装版、zip 解压版）都必须带上，否则用户电脑里没有文档可打开。
fn official_doc_sources(root: &Path, version: &str) -> Vec<(&'static str, PathBuf)> {
    vec![
        (
            "changelog.md",
            root.join("doc")
                .join("changelog")
                .join(format!("v{version}.md")),
        ),
        ("mcp.md", root.join("doc").join("mcp.md")),
    ]
}

fn is_official_doc_name(name: &str) -> bool {
    matches!(name, "changelog.md" | "mcp.md")
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
    copy_official_docs(root, bundle, VERSION)
}

/// 把当前版本发版说明与 MCP 说明复制进便携目录；NSIS 与 zip 都从这里取文件。
fn copy_official_docs(root: &Path, bundle: &Path, version: &str) -> Result<()> {
    for (name, source) in official_doc_sources(root, version) {
        if !source.is_file() {
            bail!(
                "缺少随包发布的官方说明文档 {}；请先补齐当前版本发版说明与 doc/mcp.md",
                source.display()
            );
        }
        fs::copy(&source, bundle.join(name))
            .with_context(|| format!("copy {} -> {}", source.display(), name))?;
    }
    Ok(())
}

/// 递归收集便携目录里的所有文件。原来只遍历顶层，子目录会被静默丢掉；
/// 官方说明文档将来若移入资源子目录，也不会再漏包。
fn collect_bundle_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, PathBuf)>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            collect_bundle_files(root, &path, out)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .with_context(|| format!("strip {} from {}", root.display(), path.display()))?;
            out.push((relative.to_string_lossy().replace('\\', "/"), path));
        }
    }
    Ok(())
}

/// 高位是 Unix `0444`，低位 `0x01` 是 DOS 只读位；解压后官方说明文档
/// 默认只读。真正的安全边界仍是应用内的 SHA-256 校验与只读标签页。
fn zip_options_for(relative: &str) -> zip::write::SimpleFileOptions {
    let mut options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    if is_official_doc_name(relative) {
        options = options.external_attributes((0o100444 << 16) | 0x01);
    }
    options
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
    let prefix = name;

    let mut files = Vec::new();
    collect_bundle_files(bundle, bundle, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    for (relative, source_path) in files {
        writer.start_file(format!("{prefix}/{relative}"), zip_options_for(&relative))?;
        let mut source = fs::File::open(&source_path)
            .with_context(|| format!("open {}", source_path.display()))?;
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
    use std::io::{Cursor, Write};

    use super::*;

    #[test]
    fn official_doc_sources_exist_for_the_current_version() {
        let root = workspace_root().expect("workspace root");
        let sources = official_doc_sources(&root, VERSION);
        assert_eq!(sources.len(), 2);
        assert_eq!(
            sources.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec!["changelog.md", "mcp.md"]
        );
        for (name, source) in sources {
            assert!(source.is_file(), "缺少官方说明文档 {name}: {}", source.display());
        }
    }

    #[test]
    fn official_docs_land_in_the_bundle_and_zip() {
        let root = workspace_root().expect("workspace root");
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let bundle = std::env::temp_dir().join(format!("mdwork-docs-{unique}"));
        fs::create_dir_all(&bundle).unwrap();
        copy_official_docs(&root, &bundle, VERSION).unwrap();
        assert!(bundle.join("changelog.md").is_file());
        assert!(bundle.join("mcp.md").is_file());

        let zip_path = zip_bundle(&bundle).unwrap();
        let archive_file = fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(archive_file).unwrap();
        let prefix = bundle.file_name().unwrap().to_string_lossy();
        for name in ["changelog.md", "mcp.md"] {
            let entry = archive
                .by_name(&format!("{prefix}/{name}"))
                .unwrap_or_else(|_| panic!("zip 中缺少 {name}"));
            assert_eq!(entry.external_attributes() & 0x01, 0x01);
        }
        drop(archive);
        fs::remove_file(&zip_path).unwrap();
        fs::remove_dir_all(&bundle).unwrap();
    }

    #[test]
    fn official_docs_are_readonly_in_the_zip() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            writer
                .start_file("changelog.md", zip_options_for("changelog.md"))
                .unwrap();
            writer.write_all(b"official changelog").unwrap();
            writer
                .start_file("md-work.exe", zip_options_for("md-work.exe"))
                .unwrap();
            writer.write_all(b"binary").unwrap();
            writer.finish().unwrap();
        }
        buf.set_position(0);
        let mut archive = zip::ZipArchive::new(buf).unwrap();
        let official = archive.by_name("changelog.md").unwrap().external_attributes();
        let executable = archive.by_name("md-work.exe").unwrap().external_attributes();
        assert_eq!(official & 0x01, 0x01, "DOS 只读位必须置位");
        assert_eq!(official >> 16, 0o100444, "Unix 权限为只读");
        assert_eq!(executable, 0o100644 << 16, "普通文件保持可写");
    }

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
