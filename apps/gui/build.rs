use std::path::{Path, PathBuf};

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

/// 把随包发布的三份官方说明文档在编译期嵌入可执行文件。
///
/// 运行时显示的是这份内置正文，磁盘上的 `changelog.md` / `mcp.md` /
/// `欢迎使用墨笺.md` 只用做 SHA-256 校验。这样即使用户改了安装目录里的
/// 文件，也改不了程序里显示的权威内容，更不会通过编辑器写回磁盘。
fn generate_official_docs() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let version = env!("CARGO_PKG_VERSION");
    let changelog = root
        .join("doc")
        .join("changelog")
        .join(format!("v{version}.md"));
    let mcp = root.join("doc").join("mcp.md");
    let welcome = root.join("doc").join("欢迎使用墨笺.md");
    println!("cargo:rerun-if-changed={}", changelog.display());
    println!("cargo:rerun-if-changed={}", mcp.display());
    println!("cargo:rerun-if-changed={}", welcome.display());

    let changelog_text = read_official_doc(&changelog, "changelog");
    let mcp_text = read_official_doc(&mcp, "mcp");
    let welcome_text = read_official_doc(&welcome, "欢迎使用墨笺.md");
    // `{:?}` 生成的是合法的 Rust 字符串字面量（含转义），源文件里的
    // 引号、反斜杠、换行都不需要额外处理。
    let generated = format!(
        "pub const CHANGELOG_VERSION: &str = {version:?};\n\
         pub const CHANGELOG_MD: &str = {changelog_text:?};\n\
         pub const MCP_MD: &str = {mcp_text:?};\n\
         pub const WELCOME_MD: &str = {welcome_text:?};\n"
    );
    std::fs::write(out.join("official_docs_data.rs"), generated)
        .expect("write generated official docs");
}

/// 把仓库最后一次提交的哈希编译进主程序。
///
/// 发布物里没有 `.git` 目录，运行时读不到提交信息，只能在编译期固化。打包流程
/// （`xtask package` / CI）会把 `MDWORK_GIT_COMMIT` 传给 release 构建；开发态没有
/// 这个变量时直接问 git。变量值变化会让 Cargo 重跑本脚本，保证每次打包记录的
/// 都是打包那一刻的 HEAD。
fn emit_git_commit() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    println!("cargo:rerun-if-env-changed=MDWORK_GIT_COMMIT");

    let from_env = std::env::var("MDWORK_GIT_COMMIT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let commit = from_env
        .or_else(|| git_output(&root, &["rev-parse", "HEAD"]))
        .unwrap_or_default();
    println!("cargo:rustc-env=MDWORK_GIT_COMMIT={commit}");

    // 开发态提交 / 切分支后也刷新；发布态以上面的环境变量为准。
    let git_dir = git_output(&root, &["rev-parse", "--absolute-git-dir"])
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".git"));
    for path in [
        git_dir.join("HEAD"),
        git_dir.join("packed-refs"),
        git_dir.join("refs/heads"),
    ] {
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

/// 静默执行 git；源码 zip、未安装 git 等场景返回 `None`，不阻塞构建。
fn git_output(root: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn read_official_doc(path: &Path, label: &str) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => {
            // 版本刚升、发版说明还没写时不阻塞开发态编译；`xtask package`
            // 会再次检查并中止打包，保证发布物一定带上正确的文档。
            println!(
                "cargo:warning=官方发布文档 {label} 读取失败（{}）：{err}；打包时会再次校验并中止",
                path.display()
            );
            String::new()
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    generate_official_docs();
    emit_git_commit();

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
