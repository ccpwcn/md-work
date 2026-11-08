//! 随安装包 / zip 包发布的官方说明文档。
//!
//! `changelog.md` 与 `mcp.md` 的权威正文在编译期就嵌入了可执行文件；运行时
//! 打开的是这份内置正文，磁盘上的副本只用来做 SHA-256 校验。因此：
//!
//! - 用户可以阅读、复制、打印，但无法切换编辑模式、无法保存、不会写回磁盘；
//! - 安装目录里的副本被改动时，弹窗提醒并继续显示内置的权威版本；
//! - 开发环境没有随包文档时也能正常预览（直接用内置正文）。

use std::path::{Path, PathBuf};

use mdwork_core::doc::sha256_hex;
use rust_i18n::t;

include!(concat!(env!("OUT_DIR"), "/official_docs_data.rs"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OfficialDocKind {
    /// 当前版本的发版说明（`doc/changelog/vX.Y.Z.md`）。
    Changelog,
    /// MCP 服务使用说明（`doc/mcp.md`）。
    Mcp,
}

impl OfficialDocKind {
    pub const ALL: [Self; 2] = [Self::Changelog, Self::Mcp];

    /// 部署到 exe 同目录后的固定文件名。
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Changelog => "changelog.md",
            Self::Mcp => "mcp.md",
        }
    }

    /// 编译期嵌入的权威正文。
    pub fn embedded(self) -> &'static str {
        match self {
            Self::Changelog => CHANGELOG_MD,
            Self::Mcp => MCP_MD,
        }
    }

    /// 标签页标题；不依赖磁盘文件名，便于本地化。
    pub fn title(self) -> String {
        match self {
            Self::Changelog => {
                t!("official_docs.changelog_title", version = CHANGELOG_VERSION).to_string()
            }
            Self::Mcp => t!("official_docs.mcp_title").to_string(),
        }
    }

    pub fn expected_sha256(self) -> String {
        sha256_hex(self.embedded().as_bytes())
    }

    /// exe 同目录里的副本路径；安装版与 zip 解压版都在这个位置。
    pub fn deployed_path(self) -> Option<PathBuf> {
        deployed_dir().map(|dir| dir.join(self.file_name()))
    }

    /// 校验磁盘副本与内置权威正文是否一致。
    pub fn verify(self) -> OfficialDocStatus {
        let Some(path) = self.deployed_path() else {
            return OfficialDocStatus::Missing(PathBuf::new());
        };
        match std::fs::read(&path) {
            Ok(bytes) => {
                let actual = sha256_hex(&bytes);
                let expected = self.expected_sha256();
                if actual == expected {
                    OfficialDocStatus::Intact
                } else {
                    OfficialDocStatus::Modified { expected, actual }
                }
            }
            Err(_) => OfficialDocStatus::Missing(path),
        }
    }
}

pub fn deployed_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// 判断一个外部路径是不是随包发布的官方说明文档。用 canonicalize 比较，
/// 避免相对路径、大小写或 `..` 绕过判定。
pub fn kind_for_path(path: &Path) -> Option<OfficialDocKind> {
    let canonical = std::fs::canonicalize(path).ok()?;
    OfficialDocKind::ALL.into_iter().find(|kind| {
        kind.deployed_path()
            .and_then(|deployed| std::fs::canonicalize(deployed).ok())
            .is_some_and(|deployed| deployed == canonical)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficialDocStatus {
    Intact,
    /// 磁盘副本的 SHA-256 与内置权威正文不一致。
    Modified { expected: String, actual: String },
    Missing(PathBuf),
}

#[derive(Debug, Clone)]
pub struct OfficialDocAlert {
    pub kind: OfficialDocKind,
    pub status: OfficialDocStatus,
}

fn should_alert(status: &OfficialDocStatus) -> bool {
    match status {
        OfficialDocStatus::Intact => false,
        OfficialDocStatus::Modified { .. } => true,
        // 开发态（cargo run）本来就没有随包文档，不打扰开发；
        // 发布版缺文件说明部署被破坏，需要提醒。
        OfficialDocStatus::Missing(_) => !cfg!(debug_assertions),
    }
}

fn log_alert(alert: &OfficialDocAlert) {
    tracing::warn!(
        doc = alert.kind.file_name(),
        status = ?alert.status,
        "官方发布文档校验未通过，将使用程序内置的权威版本"
    );
}

/// 启动时校验两份官方说明文档。
pub fn check_deployed_docs() -> Vec<OfficialDocAlert> {
    OfficialDocKind::ALL
        .into_iter()
        .filter_map(|kind| {
            let status = kind.verify();
            if !should_alert(&status) {
                return None;
            }
            let alert = OfficialDocAlert { kind, status };
            log_alert(&alert);
            Some(alert)
        })
        .collect()
}

/// 打开单份文档时再校验一次（用户可能在程序运行期间改了文件）。
pub fn check_deployed_doc(kind: OfficialDocKind) -> Option<OfficialDocAlert> {
    let status = kind.verify();
    if !should_alert(&status) {
        return None;
    }
    let alert = OfficialDocAlert { kind, status };
    log_alert(&alert);
    Some(alert)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_docs_match_the_current_version() {
        assert_eq!(CHANGELOG_VERSION, mdwork_core::version());
        assert!(!CHANGELOG_MD.trim().is_empty(), "当前版本发版说明不能为空");
        assert!(!MCP_MD.trim().is_empty(), "mcp.md 不能为空");
    }

    #[test]
    fn sha256_detects_a_tampered_copy() {
        let expected = OfficialDocKind::Mcp.expected_sha256();
        assert_eq!(expected, sha256_hex(MCP_MD.as_bytes()));
        let tampered = format!("{MCP_MD}<!-- changed -->");
        assert_ne!(sha256_hex(tampered.as_bytes()), expected);
    }

    #[test]
    fn deployed_file_names_are_stable() {
        assert_eq!(OfficialDocKind::Changelog.file_name(), "changelog.md");
        assert_eq!(OfficialDocKind::Mcp.file_name(), "mcp.md");
    }

    #[test]
    fn official_doc_titles_resolve_their_i18n_keys() {
        let changelog = OfficialDocKind::Changelog.title();
        assert!(
            changelog.contains(CHANGELOG_VERSION),
            "更新说明标题应带版本号，实际为 {changelog}"
        );
        assert!(!OfficialDocKind::Mcp.title().is_empty());
    }
}
