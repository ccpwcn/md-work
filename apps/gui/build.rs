use std::path::Path;

const VERSION_MAJOR: u64 = 1;
const VERSION_MINOR: u64 = 0;
const VERSION_PATCH: u64 = 0;

// gpui-pre already embeds a Win32 manifest (dpiAware true/pm, PerMonitorV2,
// Common-Controls v6, SegmentHeap). Embedding a second one here makes link.exe
// fail with CVT1100 (duplicate MANIFEST resource), so this script only adds
// icon and version-info resources.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let version = VERSION_MAJOR << 48 | VERSION_MINOR << 32 | VERSION_PATCH << 16;
    let version_text = format!("{VERSION_MAJOR}.{VERSION_MINOR}.{VERSION_PATCH}");
    let version_quad = format!("{version_text}.0");

    let mut res = winresource::WindowsResource::new();
    res.set_language(0x0409)
        .set("FileVersion", &version_quad)
        .set("ProductVersion", &version_quad)
        .set("FileDescription", "墨笺 Markdown 笔记本")
        .set("ProductName", "墨笺 (MoJiān)")
        .set("OriginalFilename", "md-work.exe")
        .set("InternalName", "md-work")
        .set("LegalCopyright", "Copyright © 2026 lidawei. MIT License.")
        .set(
            "Comments",
            "Windows-first Markdown notebook written in Rust",
        )
        .set_version_info(winresource::VersionInfo::FILEVERSION, version)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, version)
        .set_version_info(winresource::VersionInfo::FILEOS, 0x0004_0004)
        .set_version_info(winresource::VersionInfo::FILETYPE, 0x0000_0001);

    if Path::new("../../assets/icon.ico").exists() {
        res.set_icon("../../assets/icon.ico");
    } else {
        println!("cargo:warning=assets/icon.ico missing; run `cargo run -p xtask -- icon`");
    }

    if let Err(err) = res.compile() {
        println!("cargo:warning=failed to embed icon/version info: {err}");
    }
}
