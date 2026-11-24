//! 笔记改名记录：让长连接的 MCP 会话在 Agent 使用旧路径时自动跟随到新路径。
//!
//! MCP 工具参数始终是笔记本内相对路径（没有标题→路径映射）；服务端每次都重新
//! 扫盘，但 Agent 的上下文里可能还留着改名前的路径。写类工具默认 `create=true`，
//! 旧路径一旦缺失就会新建空笔记，表现为「旧名称 + No Content」。改名时写一条
//! 轻量日志，服务端在路径缺失时查表重定向即可避免该问题。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::permissions::{normalize_acl_path, now_unix};
use crate::{Result, paths, toml_io};

const RENAME_FORMAT: &str = "md-work/renames@1";
const MAX_ENTRIES: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenameRecord {
    pub notebook: Uuid,
    pub from: String,
    pub to: String,
    pub at: u64,
}

impl Default for RenameRecord {
    fn default() -> Self {
        Self {
            notebook: Uuid::nil(),
            from: String::new(),
            to: String::new(),
            at: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenameJournal {
    pub format: String,
    /// `[[rename]]` 条目，最新追加在末尾。
    #[serde(rename = "rename")]
    pub entries: Vec<RenameRecord>,
}

impl Default for RenameJournal {
    fn default() -> Self {
        Self {
            format: RENAME_FORMAT.to_string(),
            entries: Vec::new(),
        }
    }
}

impl RenameJournal {
    pub fn default_path() -> PathBuf {
        paths::app_data_dir().join("renames.toml")
    }

    pub fn load() -> Self {
        match toml_io::load::<Self>(&Self::default_path()) {
            Ok(Some(journal)) if journal.format == RENAME_FORMAT => journal,
            Ok(Some(_)) => Self::default(),
            Ok(None) => Self::default(),
            Err(err) => {
                tracing::warn!(error = %err, "读取笔记改名记录失败，按空记录处理");
                Self::default()
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        toml_io::save(&Self::default_path(), self)
    }

    /// 记录一次改名（文件夹改名会同时覆盖其后代）。在写线程调用。
    pub fn record(notebook: Uuid, from: &str, to: &str) {
        let from = normalize_acl_path(from);
        let to = normalize_acl_path(to);
        if from.is_empty() || to.is_empty() || from == to {
            return;
        }
        let mut journal = Self::load();
        let prefix = format!("{from}/");
        journal.entries.retain(|entry| {
            entry.notebook != notebook
                || (entry.from != from && !entry.from.starts_with(&prefix) && entry.to != to)
        });
        journal.entries.push(RenameRecord {
            notebook,
            from,
            to,
            at: now_unix(),
        });
        if journal.entries.len() > MAX_ENTRIES {
            let excess = journal.entries.len() - MAX_ENTRIES;
            journal.entries.drain(..excess);
        }
        if let Err(err) = journal.save() {
            tracing::warn!(error = %err, "保存笔记改名记录失败");
        }
    }

    /// 把旧路径解析为当前路径；无记录时返回 `None`。
    /// 文件夹改名时 `from/descendant` 也会映射到 `to/descendant`。
    pub fn resolve(&self, notebook: Uuid, path: &str) -> Option<String> {
        let path = normalize_acl_path(path);
        let mut best: Option<(usize, String)> = None;
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.notebook == notebook)
        {
            if path == entry.from {
                return Some(entry.to.clone());
            }
            let prefix = format!("{}/", entry.from);
            if let Some(rest) = path.strip_prefix(&prefix) {
                let candidate = format!("{}/{}", entry.to, rest);
                if best
                    .as_ref()
                    .is_none_or(|(longest, _)| entry.from.len() > *longest)
                {
                    best = Some((entry.from.len(), candidate));
                }
            }
        }
        best.map(|(_, to)| to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_exact_and_descendant_paths() {
        let id = Uuid::new_v4();
        let journal = RenameJournal {
            entries: vec![RenameRecord {
                notebook: id,
                from: "旧目录/旧笔记".to_string(),
                to: "新目录/新笔记".to_string(),
                at: 1,
            }],
            ..Default::default()
        };
        assert_eq!(
            journal.resolve(id, "旧目录/旧笔记").as_deref(),
            Some("新目录/新笔记")
        );
        assert_eq!(
            journal.resolve(id, "旧目录/旧笔记/子.md").as_deref(),
            Some("新目录/新笔记/子.md")
        );
        assert_eq!(journal.resolve(id, "别的笔记"), None);
        assert_eq!(journal.resolve(Uuid::nil(), "旧目录/旧笔记"), None);
    }
}
