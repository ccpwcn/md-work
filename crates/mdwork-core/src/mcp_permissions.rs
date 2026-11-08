//! MCP 笔记树权限：四级累积等级、祖先链取最小的有效权限、授权传播。
//!
//! 这里是纯数据与确定性规则，GUI（权限编辑器）与 MCP 服务端（执行）共用。
//! 无记录的节点一律视为隐藏，因此新建笔记本/文件夹/笔记天然默认隐藏。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::permissions::normalize_acl_path;

/// 权限文件格式标识。
pub const PERMISSIONS_FORMAT: &str = "md-work/mcp-permissions@1";

/// 节点权限等级，逐级累积：隐藏 < 只读 < 修改 < 删除。
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PermLevel {
    /// 外部 Agent 不可见、不可访问。
    #[default]
    Hidden,
    /// 列举、读取、搜索、导出。
    Read,
    /// 只读 + 新建、写入、追加、移动、书签。
    Modify,
    /// 修改 + 删除（永久删除仍受令牌 delete:permanent 约束）。
    Delete,
}

impl PermLevel {
    /// 配置文件中使用的稳定标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Read => "read",
            Self::Modify => "modify",
            Self::Delete => "delete",
        }
    }

    /// 界面显示用的中文标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "隐藏",
            Self::Read => "只读",
            Self::Modify => "修改",
            Self::Delete => "删除",
        }
    }
}

/// 授权方式：只影响权限编辑器的传播行为，服务端执行不依赖它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantMode {
    /// 单独授权：改一个节点不影响其他节点的存储值。
    Single,
    /// 父子联动：改父节点整棵子树跟随，改子节点其子树跟随。
    #[default]
    Linked,
}

impl GrantMode {
    /// 界面显示用的中文标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "单独授权",
            Self::Linked => "父子联动",
        }
    }
}

/// 权限树节点标识：笔记本 UUID + 笔记本内相对路径（None 表示笔记本本身）。
///
/// 路径一律规范化为小写正斜杠（Windows 语义），序列化为 `"uuid"` 或
/// `"uuid/相对/路径"` 字符串，作为 TOML  map 的键。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeKey {
    /// 所属笔记本。
    pub notebook: Uuid,
    /// 笔记本内相对路径；None 表示笔记本节点。
    pub path: Option<String>,
}

impl NodeKey {
    /// 笔记本节点。
    pub fn notebook(id: Uuid) -> Self {
        Self {
            notebook: id,
            path: None,
        }
    }

    /// 文件夹或笔记节点；路径会被规范化。
    pub fn entry(id: Uuid, rel_path: &str) -> Self {
        let path = normalize_acl_path(rel_path);
        Self {
            notebook: id,
            path: (!path.is_empty()).then_some(path),
        }
    }

    /// 解析 `"uuid"` 或 `"uuid/相对/路径"`；非法输入返回 None。
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        match raw.split_once('/') {
            Some((id, path)) => {
                let key = Self::entry(Uuid::parse_str(id).ok()?, path);
                // 规范化后路径为空（如 "uuid/"、"uuid/./"）不是合法节点键。
                key.path.is_some().then_some(key)
            }
            None => Some(Self::notebook(Uuid::parse_str(raw).ok()?)),
        }
    }

    /// 节点的直接父节点；笔记本节点返回 None。
    pub fn parent(&self) -> Option<Self> {
        let path = self.path.as_ref()?;
        Some(match path.rsplit_once('/') {
            Some((parent, _)) => Self::entry(self.notebook, parent),
            None => Self::notebook(self.notebook),
        })
    }

    /// key 是否为 self 的后代（不含自身）。
    pub fn is_ancestor_of(&self, key: &Self) -> bool {
        if self.notebook != key.notebook || self == key {
            return false;
        }
        match (&self.path, &key.path) {
            (None, Some(_)) => true,
            (Some(prefix), Some(path)) => path.starts_with(&format!("{prefix}/")),
            _ => false,
        }
    }
}

impl std::fmt::Display for NodeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.path {
            Some(path) => write!(f, "{}/{path}", self.notebook),
            None => write!(f, "{}", self.notebook),
        }
    }
}

impl Serialize for NodeKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for NodeKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom(format!("非法权限节点键: {raw}")))
    }
}

/// 联动模式下父节点对子项授权状态的聚合展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildAggregate {
    /// 子项全部隐藏。
    AllHidden,
    /// 子项授权完全相同。
    Uniform(PermLevel),
    /// 子项授权部分不同。
    Mixed,
}

/// 笔记树权限配置：授权方式 + 各节点等级，无记录即隐藏。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionTree {
    /// 文件格式标识。
    pub format: String,
    /// 授权方式（仅编辑器使用）。
    pub mode: GrantMode,
    /// 节点等级表；缺省节点为隐藏。
    pub nodes: BTreeMap<NodeKey, PermLevel>,
}

impl Default for PermissionTree {
    fn default() -> Self {
        Self {
            format: PERMISSIONS_FORMAT.to_string(),
            mode: GrantMode::default(),
            nodes: BTreeMap::new(),
        }
    }
}

impl PermissionTree {
    /// 节点的存储等级，无记录为隐藏。
    pub fn level(&self, key: &NodeKey) -> PermLevel {
        self.nodes.get(key).copied().unwrap_or_default()
    }

    /// 节点的有效权限：自身与全部祖先（含笔记本）存储等级的最小值。
    pub fn effective(&self, key: &NodeKey) -> PermLevel {
        let mut level = self.level(&NodeKey::notebook(key.notebook));
        let Some(path) = &key.path else {
            return level;
        };
        let mut ancestor = String::new();
        for segment in path.split('/') {
            if level == PermLevel::Hidden {
                break;
            }
            if ancestor.is_empty() {
                ancestor.push_str(segment);
            } else {
                ancestor.push('/');
                ancestor.push_str(segment);
            }
            level = level.min(self.level(&NodeKey {
                notebook: key.notebook,
                path: Some(ancestor.clone()),
            }));
        }
        level
    }

    /// 有效权限是否达到 `required`。
    pub fn allows(&self, key: &NodeKey, required: PermLevel) -> bool {
        self.effective(key) >= required
    }

    /// 按当前授权方式设置节点等级；联动模式下整棵子树跟随。
    ///
    /// `descendants` 是该节点在真实笔记树中的全部后代（由调用方枚举），
    /// 其中只有 `key` 的真正后代会被设置，单独授权模式下被忽略。
    /// 设为隐藏的节点会从表中移除以保持最小形式。
    pub fn set_level(&mut self, key: &NodeKey, level: PermLevel, descendants: &[NodeKey]) {
        let linked = self.mode == GrantMode::Linked;
        let mut targets = vec![key.clone()];
        if linked {
            targets.extend(
                descendants
                    .iter()
                    .filter(|candidate| key.is_ancestor_of(candidate))
                    .cloned(),
            );
        }
        for target in targets {
            if level == PermLevel::Hidden {
                self.nodes.remove(&target);
            } else {
                self.nodes.insert(target, level);
            }
        }
    }

    /// 节点重命名/移动后迁移权限记录：`from` 自身及其整棵子树的等级跟随到 `to`。
    ///
    /// 返回被迁移的记录数。迁移后可能因新的祖先链而收紧（有效权限取最小），
    /// 这与手工授权语义一致，不会放宽。
    pub fn rekey(&mut self, from: &NodeKey, to: &NodeKey) -> usize {
        let (Some(from_path), Some(to_path)) = (from.path.as_deref(), to.path.as_deref()) else {
            // 笔记本节点不会被重命名，重命名只发生在笔记本内。
            return 0;
        };
        if from.notebook != to.notebook || from_path == to_path {
            return 0;
        }
        let moving: Vec<(NodeKey, PermLevel)> = self
            .nodes
            .iter()
            .filter(|(key, _)| *key == from || from.is_ancestor_of(key))
            .map(|(key, level)| (key.clone(), *level))
            .collect();
        if moving.is_empty() {
            return 0;
        }
        for (key, _) in &moving {
            self.nodes.remove(key);
        }
        for (key, level) in &moving {
            let migrated = match key.path.as_deref() {
                // 被移动的节点自身。
                Some(path) if path == from_path => NodeKey::entry(to.notebook, to_path),
                // 子树：保留 from 之后的相对路径。
                Some(path) => {
                    let suffix = path
                        .strip_prefix(from_path)
                        .unwrap_or_default()
                        .trim_start_matches('/');
                    NodeKey::entry(to.notebook, &format!("{to_path}/{suffix}"))
                }
                None => to.clone(),
            };
            self.nodes.insert(migrated, *level);
        }
        moving.len()
    }

    /// 联动模式下子项授权的聚合状态；无后代时返回 None（展示自身等级）。
    pub fn subtree_aggregate(&self, descendants: &[NodeKey]) -> Option<ChildAggregate> {
        let mut levels = descendants.iter().map(|key| self.level(key));
        let first = levels.next()?;
        let mut uniform = true;
        let mut all_hidden = first == PermLevel::Hidden;
        for level in levels {
            uniform &= level == first;
            all_hidden &= level == PermLevel::Hidden;
        }
        Some(if all_hidden {
            ChildAggregate::AllHidden
        } else if uniform {
            ChildAggregate::Uniform(first)
        } else {
            ChildAggregate::Mixed
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> Uuid {
        Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    fn tree() -> PermissionTree {
        PermissionTree::default()
    }

    #[test]
    fn default_tree_hides_everything_and_defaults_to_linked() {
        let t = tree();
        assert_eq!(t.mode, GrantMode::Linked);
        assert_eq!(t.level(&NodeKey::notebook(id())), PermLevel::Hidden);
        assert_eq!(
            t.effective(&NodeKey::entry(id(), "工作/设计.md")),
            PermLevel::Hidden
        );
        assert!(!t.allows(&NodeKey::entry(id(), "工作/设计.md"), PermLevel::Read));
    }

    #[test]
    fn levels_are_cumulative() {
        assert!(PermLevel::Hidden < PermLevel::Read);
        assert!(PermLevel::Read < PermLevel::Modify);
        assert!(PermLevel::Modify < PermLevel::Delete);
        let mut t = tree();
        let key = NodeKey::notebook(id());
        t.set_level(&key, PermLevel::Modify, &[]);
        assert!(t.allows(&key, PermLevel::Read));
        assert!(t.allows(&key, PermLevel::Modify));
        assert!(!t.allows(&key, PermLevel::Delete));
    }

    #[test]
    fn effective_takes_min_across_ancestors() {
        let mut t = tree();
        let nb = NodeKey::notebook(id());
        let folder = NodeKey::entry(id(), "工作");
        let note = NodeKey::entry(id(), "工作/设计.md");
        t.set_level(&nb, PermLevel::Read, &[]);
        t.set_level(&folder, PermLevel::Delete, std::slice::from_ref(&note));
        t.set_level(&note, PermLevel::Delete, &[]);
        assert_eq!(t.effective(&note), PermLevel::Read);
        assert_eq!(t.effective(&folder), PermLevel::Read);

        // 隐藏祖先即隐藏整棵子树，即使子节点单独授权更高。
        t.set_level(&folder, PermLevel::Hidden, std::slice::from_ref(&note));
        t.set_level(&note, PermLevel::Delete, &[]);
        assert_eq!(t.level(&note), PermLevel::Delete);
        assert_eq!(t.effective(&note), PermLevel::Hidden);
    }

    #[test]
    fn single_mode_does_not_propagate() {
        let mut t = tree();
        t.mode = GrantMode::Single;
        let nb = NodeKey::notebook(id());
        let folder = NodeKey::entry(id(), "工作");
        let note = NodeKey::entry(id(), "工作/设计.md");
        t.set_level(&nb, PermLevel::Delete, &[folder.clone(), note.clone()]);
        t.set_level(&folder, PermLevel::Read, std::slice::from_ref(&note));
        assert_eq!(t.level(&note), PermLevel::Hidden);
        assert_eq!(t.level(&nb), PermLevel::Delete);
        t.set_level(&note, PermLevel::Modify, &[]);
        assert_eq!(t.level(&folder), PermLevel::Read);
    }

    #[test]
    fn linked_mode_propagates_to_descendants_only() {
        let mut t = tree();
        let nb = NodeKey::notebook(id());
        let folder = NodeKey::entry(id(), "工作");
        let sub = NodeKey::entry(id(), "工作/项目");
        let note = NodeKey::entry(id(), "工作/项目/设计.md");
        let other = NodeKey::entry(id(), "生活/日记.md");
        t.set_level(&nb, PermLevel::Read, &[]);
        t.set_level(
            &folder,
            PermLevel::Modify,
            &[sub.clone(), note.clone(), other.clone()],
        );
        assert_eq!(t.level(&sub), PermLevel::Modify);
        assert_eq!(t.level(&note), PermLevel::Modify);
        assert_eq!(t.level(&other), PermLevel::Hidden);
        assert_eq!(t.level(&nb), PermLevel::Read);

        // 改子节点：其子树跟随，祖先存储值不变（仅展示聚合态）。
        t.set_level(&sub, PermLevel::Read, std::slice::from_ref(&note));
        assert_eq!(t.level(&note), PermLevel::Read);
        assert_eq!(t.level(&folder), PermLevel::Modify);
    }

    #[test]
    fn linked_aggregate_states() {
        let mut t = tree();
        let a = NodeKey::entry(id(), "a.md");
        let b = NodeKey::entry(id(), "b.md");
        assert_eq!(t.subtree_aggregate(&[]), None);
        assert_eq!(
            t.subtree_aggregate(&[a.clone(), b.clone()]),
            Some(ChildAggregate::AllHidden)
        );
        t.set_level(&a, PermLevel::Read, &[]);
        t.set_level(&b, PermLevel::Read, &[]);
        assert_eq!(
            t.subtree_aggregate(&[a.clone(), b.clone()]),
            Some(ChildAggregate::Uniform(PermLevel::Read))
        );
        t.set_level(&b, PermLevel::Delete, &[]);
        assert_eq!(
            t.subtree_aggregate(&[a.clone(), b.clone()]),
            Some(ChildAggregate::Mixed)
        );
        t.set_level(&a, PermLevel::Hidden, &[]);
        t.set_level(&b, PermLevel::Hidden, &[]);
        assert_eq!(
            t.subtree_aggregate(&[a, b]),
            Some(ChildAggregate::AllHidden)
        );
    }

    #[test]
    fn setting_hidden_removes_entry() {
        let mut t = tree();
        let key = NodeKey::entry(id(), "a.md");
        t.set_level(&key, PermLevel::Read, &[]);
        assert!(t.nodes.contains_key(&key));
        t.set_level(&key, PermLevel::Hidden, &[]);
        assert!(!t.nodes.contains_key(&key));
        assert_eq!(t.effective(&key), PermLevel::Hidden);
    }

    #[test]
    fn rekey_moves_node_and_subtree_records() {
        let mut t = tree();
        let folder = NodeKey::entry(id(), "工作");
        let note = NodeKey::entry(id(), "工作/设计.md");
        let sibling = NodeKey::entry(id(), "生活/日记.md");
        t.set_level(&folder, PermLevel::Read, &[]);
        t.set_level(&note, PermLevel::Delete, &[]);
        t.set_level(&sibling, PermLevel::Modify, &[]);

        assert_eq!(t.rekey(&folder, &NodeKey::entry(id(), "项目")), 2);
        assert_eq!(t.level(&NodeKey::entry(id(), "项目")), PermLevel::Read);
        assert_eq!(
            t.level(&NodeKey::entry(id(), "项目/设计.md")),
            PermLevel::Delete
        );
        assert_eq!(t.level(&folder), PermLevel::Hidden);
        assert_eq!(t.level(&note), PermLevel::Hidden);
        // 兄弟节点不受影响。
        assert_eq!(t.level(&sibling), PermLevel::Modify);

        // 单篇笔记改名。
        let renamed = NodeKey::entry(id(), "生活/日记 2024.md");
        assert_eq!(t.rekey(&sibling, &renamed), 1);
        assert_eq!(t.level(&renamed), PermLevel::Modify);
        assert_eq!(t.level(&sibling), PermLevel::Hidden);

        // 无记录或未变化时不写入。
        assert_eq!(t.rekey(&NodeKey::entry(id(), "空.md"), &renamed), 0);
        assert_eq!(t.rekey(&renamed, &renamed), 0);
    }

    #[test]
    fn node_key_round_trip_and_normalization() {
        let nb = NodeKey::notebook(id());
        assert_eq!(NodeKey::parse(&nb.to_string()), Some(nb.clone()));
        let note = NodeKey::entry(id(), r"工作\子 目录/设计.MD");
        assert_eq!(note.path.as_deref(), Some("工作/子 目录/设计.md"));
        assert_eq!(NodeKey::parse(&note.to_string()), Some(note.clone()));
        assert_eq!(note.parent(), Some(NodeKey::entry(id(), "工作/子 目录")));
        assert_eq!(
            NodeKey::entry(id(), "工作").parent(),
            Some(NodeKey::notebook(id()))
        );
        assert_eq!(NodeKey::notebook(id()).parent(), None);
        assert!(NodeKey::entry(id(), "工作").is_ancestor_of(&note));
        assert!(!NodeKey::entry(id(), "生活").is_ancestor_of(&note));
        assert!(!note.is_ancestor_of(&note));
        assert!(NodeKey::parse("not-a-uuid").is_none());
        assert!(NodeKey::parse(&format!("{}/", id())).is_none());
    }

    #[test]
    fn toml_round_trip_and_reject_unknown_level() {
        let mut t = tree();
        t.set_level(&NodeKey::notebook(id()), PermLevel::Read, &[]);
        t.set_level(
            &NodeKey::entry(id(), "工作/设计.md"),
            PermLevel::Modify,
            &[],
        );
        let text = toml::to_string_pretty(&t).unwrap();
        assert!(text.contains("[nodes]"));
        let back: PermissionTree = toml::from_str(&text).unwrap();
        assert_eq!(back, t);

        let bad = text.replace("\"modify\"", "\"admin\"");
        assert!(toml::from_str::<PermissionTree>(&bad).is_err());
    }
}
