use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    if !Path::new("../../assets/icon.ico").exists() {
        println!("cargo:warning=assets/icon.ico missing; run `cargo run -p xtask -- icon`");
        return;
    }

    let version = 0u64 << 48 | 1u64 << 32;
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/icon.ico")
        .set_language(0x0409)
        .set("FileVersion", "0.1.0.0")
        .set("ProductVersion", "0.1.0.0")
        .set("FileDescription", "墨笺 Markdown 笔记本")
        .set("ProductName", "墨笺 (md-work)")
        .set("OriginalFilename", "md-work.exe")
        .set("InternalName", "md-work")
        .set("LegalCopyright", "Copyright © 2026 lidawei. MIT License.")
        .set("Comments", "Windows-first Markdown notebook written in Rust")
        .set_version_info(winresource::VersionInfo::FILEVERSION, version)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, version)
        .set_version_info(winresource::VersionInfo::FILEOS, 0x0004_0004)
        .set_version_info(winresource::VersionInfo::FILETYPE, 0x0000_0001);

    if let Err(err) = res.compile() {
        println!("cargo:warning=failed to embed icon/version info: {err}");
    }
}
