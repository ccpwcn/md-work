use std::path::Path;

// gpui-pre already embeds a Win32 manifest (dpiAware true/pm, PerMonitorV2,
// Common-Controls v6, SegmentHeap). Embedding a second one here makes link.exe
// fail with CVT1100 (duplicate MANIFEST resource), so this script only adds
// icon and version-info resources.
//
// VERSIONINFO 与 Cargo.toml `[workspace.package].version`（经 version.workspace）
// 保持同一来源，当前版本见 Cargo.toml `[workspace.package].version`。

fn pkg_version_parts() -> (u64, u64, u64) {
    let text = env!("CARGO_PKG_VERSION");
    let mut parts = text.split('.');
    let major = parts
        .next()
        .and_then(|s| s.parse().ok())
        .expect("CARGO_PKG_VERSION major");
    let minor = parts
        .next()
        .and_then(|s| s.parse().ok())
        .expect("CARGO_PKG_VERSION minor");
    let patch = parts
        .next()
        .and_then(|s| s.parse().ok())
        .expect("CARGO_PKG_VERSION patch");
    (major, minor, patch)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let (major, minor, patch) = pkg_version_parts();
    let version = major << 48 | minor << 32 | patch << 16;
    let version_text = format!("{major}.{minor}.{patch}");
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
