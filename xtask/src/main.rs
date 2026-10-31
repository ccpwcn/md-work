use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use resvg::{tiny_skia, usvg};

const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
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
            println!("  icon       regenerate assets/icon.ico from assets/icon.svg");
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

fn icon() -> Result<()> {
    let root = workspace_root()?;
    let svg_path = root.join("assets").join("icon.svg");
    let svg = fs::read(&svg_path).with_context(|| format!("read {}", svg_path.display()))?;

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
        icon_dir.add_entry(ico::IconDirEntry::encode(&image)?);
    }

    let ico_path = root.join("assets").join("icon.ico");
    let file =
        fs::File::create(&ico_path).with_context(|| format!("create {}", ico_path.display()))?;
    icon_dir.write(file)?;
    println!("wrote {} ({} sizes)", ico_path.display(), SIZES.len());
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
        bail!("run `cargo xtask package` first (missing {})", bundle.display());
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
