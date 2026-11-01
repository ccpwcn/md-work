//! Sibling order for notebooks in a workspace and for notes and folders inside
//! a notebook.
//!
//! A missing file keeps the historical name sort. Once a sibling list is
//! written, names in that list stay in the saved order and anything else
//! follows them.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::paths::{ASSETS_DIR, META_DIR, workspace_meta_dir};
use crate::toml_io;
use crate::{Error, Result};

pub const TREE_ORDER_FORMAT: &str = "md-work/order@1";
pub const TREE_ORDER_FILE: &str = "order.toml";
pub const NOTEBOOK_ORDER_FORMAT: &str = "md-work/notebook-order@1";
pub const NOTEBOOK_ORDER_FILE: &str = "notebook-order.toml";

const TREE_ORDER_HEADER: &str = "# 墨笺 md-work · 笔记本内同级顺序（删除后按名称恢复）\n";
const NOTEBOOK_ORDER_HEADER: &str = "# 墨笺 md-work · 笔记本顺序（删除后按名称恢复）\n";

/// Last path segment of a notebook-relative path. Notes keep their extension.
pub fn file_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// Parent relative path. The notebook root is `""`.
pub fn parent_rel(rel: &str) -> &str {
    match rel.rfind('/') {
        Some(index) => &rel[..index],
        None => "",
    }
}

/// Index of `file_name` in `saved`, or [`usize::MAX`] when it is not listed.
/// A missing list (`None`) ranks every name last so the caller’s fallback
/// sort stays in charge.
pub fn sibling_rank(saved: Option<&[String]>, file_name: &str) -> usize {
    let Some(saved) = saved else {
        return usize::MAX;
    };
    saved
        .iter()
        .position(|item| item.eq_ignore_ascii_case(file_name))
        .unwrap_or(usize::MAX)
}

/// Moves `from` to sit immediately before `before`.
///
/// `before == None` appends. Returns `None` when `from` is missing or already
/// in that slot.
#[must_use]
pub fn move_before<T: Clone + PartialEq>(
    items: &[T],
    from: &T,
    before: Option<&T>,
) -> Option<Vec<T>> {
    let from_index = items.iter().position(|item| item == from)?;
    let dest_before_removal = match before {
        Some(anchor) => items
            .iter()
            .position(|item| item == anchor)
            .unwrap_or(items.len()),
        None => items.len(),
    };
    if dest_before_removal == from_index || dest_before_removal == from_index + 1 {
        return None;
    }
    let mut next = items.to_vec();
    let item = next.remove(from_index);
    let dest = if dest_before_removal > from_index {
        dest_before_removal - 1
    } else {
        dest_before_removal
    };
    next.insert(dest, item);
    Some(next)
}

/// Per-parent child order inside one notebook. Keys are parent relative paths
/// (`""` is the notebook root). Values are directory names, with the extension
/// kept for notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeOrder {
    children: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TreeOrderFile {
    format: String,
    #[serde(default)]
    siblings: Vec<SiblingRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SiblingRecord {
    parent: String,
    names: Vec<String>,
}

impl TreeOrder {
    pub fn load(root: &Path) -> Self {
        let path = tree_order_path(root);
        match toml_io::load::<TreeOrderFile>(&path) {
            Ok(Some(file)) if file.format == TREE_ORDER_FORMAT || file.format.is_empty() => {
                let mut children = BTreeMap::new();
                for record in file.siblings {
                    children.insert(record.parent, record.names);
                }
                Self { children }
            }
            Ok(Some(file)) => {
                tracing::warn!(
                    path = %path.display(),
                    format = %file.format,
                    "顺序文件版本无法识别，按名称排序"
                );
                Self::default()
            }
            Ok(None) => Self::default(),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "顺序文件损坏，按名称排序");
                Self::default()
            }
        }
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let file = TreeOrderFile {
            format: TREE_ORDER_FORMAT.to_string(),
            siblings: self
                .children
                .iter()
                .map(|(parent, names)| SiblingRecord {
                    parent: parent.clone(),
                    names: names.clone(),
                })
                .collect(),
        };
        toml_io::save_with_header(&tree_order_path(root), TREE_ORDER_HEADER, &file)
    }

    pub fn names(&self, parent: &str) -> Option<&[String]> {
        self.children.get(parent).map(Vec::as_slice)
    }

    pub fn set_names(&mut self, parent: impl Into<String>, names: Vec<String>) {
        self.children.insert(parent.into(), names);
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

fn tree_order_path(root: &Path) -> PathBuf {
    root.join(META_DIR).join(TREE_ORDER_FILE)
}

struct DiskChild {
    name: String,
    is_folder: bool,
}

fn parent_dir(root: &Path, parent: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    if !parent.is_empty() {
        for segment in parent.split('/') {
            path.push(segment);
        }
    }
    path
}

fn read_children(root: &Path, parent: &str) -> Result<Vec<DiskChild>> {
    let dir = parent_dir(root, parent);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(Error::Io(err)),
    };
    let mut children = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if parent.is_empty() && name.eq_ignore_ascii_case(ASSETS_DIR) {
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            children.push(DiskChild {
                name,
                is_folder: true,
            });
        } else if kind.is_file() && is_markdown_name(&name) {
            children.push(DiskChild {
                name,
                is_folder: false,
            });
        }
    }
    Ok(children)
}

fn is_markdown_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".md") || lower.ends_with(".markdown")
}

fn ordered_names(mut children: Vec<DiskChild>, saved: Option<&[String]>) -> Vec<String> {
    children.sort_by(|left, right| {
        sibling_rank(saved, &left.name)
            .cmp(&sibling_rank(saved, &right.name))
            .then_with(|| right.is_folder.cmp(&left.is_folder))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    children.into_iter().map(|child| child.name).collect()
}

/// Remembers `child_name` among the children of `parent` (`""` is the root).
///
/// `after_name` is a sibling directory name. `None` appends. The current
/// on-disk siblings are materialized first, so the first explicit placement
/// freezes today’s order and then inserts.
pub fn place_child(
    root: &Path,
    parent: &str,
    child_name: &str,
    after_name: Option<&str>,
) -> Result<()> {
    let mut order = TreeOrder::load(root);
    let mut names = ordered_names(read_children(root, parent)?, order.names(parent));
    names.retain(|name| !name.eq_ignore_ascii_case(child_name));
    let insert_at = match after_name {
        Some(anchor) => names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(anchor))
            .map(|index| index + 1)
            .unwrap_or(names.len()),
        None => names.len(),
    };
    names.insert(insert_at, child_name.to_string());
    order.set_names(parent, names);
    order.save(root)
}

fn rename_listed(names: &mut [String], from: &str, to: &str) -> bool {
    let Some(slot) = names
        .iter_mut()
        .find(|name| name.eq_ignore_ascii_case(from))
    else {
        return false;
    };
    *slot = to.to_string();
    true
}

fn rekey_prefix(children: &mut BTreeMap<String, Vec<String>>, from: &str, to: &str) -> bool {
    let keys: Vec<String> = children.keys().cloned().collect();
    let prefix = format!("{from}/");
    let mut changed = false;
    for key in keys {
        let renamed = if key == from {
            Some(to.to_string())
        } else {
            key.strip_prefix(&prefix).map(|rest| format!("{to}/{rest}"))
        };
        if let Some(renamed) = renamed
            && let Some(value) = children.remove(&key)
        {
            children.insert(renamed, value);
            changed = true;
        }
    }
    changed
}

/// Keeps saved order across a rename or a move. A move into another parent
/// lands at the end of that parent.
pub fn sync_after_rename(root: &Path, from: &str, to: &str, is_dir: bool) -> Result<()> {
    let from_parent = parent_rel(from);
    let to_parent = parent_rel(to);
    let from_name = file_name(from);
    let to_name = file_name(to);
    let mut order = TreeOrder::load(root);
    let mut changed = false;
    if from_parent == to_parent {
        if let Some(names) = order.children.get_mut(from_parent) {
            changed |= rename_listed(names, from_name, to_name);
        }
    } else if let Some(names) = order.children.get_mut(from_parent) {
        let before = names.len();
        names.retain(|name| !name.eq_ignore_ascii_case(from_name));
        changed |= names.len() != before;
    }
    if is_dir {
        changed |= rekey_prefix(&mut order.children, from, to);
    }
    if changed {
        order.save(root)?;
    }
    if from_parent != to_parent {
        place_child(root, to_parent, to_name, None)?;
    }
    Ok(())
}

/// Drops a deleted note or folder from the saved order.
pub fn forget_child(root: &Path, rel: &str, is_dir: bool) -> Result<()> {
    let mut order = TreeOrder::load(root);
    if order.is_empty() {
        return Ok(());
    }
    let parent = parent_rel(rel);
    let name = file_name(rel);
    let mut changed = false;
    if let Some(names) = order.children.get_mut(parent) {
        let before = names.len();
        names.retain(|item| !item.eq_ignore_ascii_case(name));
        changed |= names.len() != before;
    }
    if is_dir {
        let prefix = format!("{rel}/");
        let before = order.children.len();
        order
            .children
            .retain(|key, _| key != rel && !key.starts_with(&prefix));
        changed |= order.children.len() != before;
    }
    if changed {
        order.save(root)?;
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct NotebookOrderFile {
    format: String,
    ids: Vec<Uuid>,
}

pub fn load_notebook_ids(workspace_root: &Path) -> Vec<Uuid> {
    let path = workspace_meta_dir(workspace_root).join(NOTEBOOK_ORDER_FILE);
    match toml_io::load::<NotebookOrderFile>(&path) {
        Ok(Some(file)) if file.format == NOTEBOOK_ORDER_FORMAT || file.format.is_empty() => {
            file.ids
        }
        Ok(Some(file)) => {
            tracing::warn!(
                path = %path.display(),
                format = %file.format,
                "笔记本顺序文件版本无法识别，按名称排序"
            );
            Vec::new()
        }
        Ok(None) => Vec::new(),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "笔记本顺序文件损坏，按名称排序"
            );
            Vec::new()
        }
    }
}

pub fn save_notebook_ids(workspace_root: &Path, ids: &[Uuid]) -> Result<()> {
    let file = NotebookOrderFile {
        format: NOTEBOOK_ORDER_FORMAT.to_string(),
        ids: ids.to_vec(),
    };
    toml_io::save_with_header(
        &workspace_meta_dir(workspace_root).join(NOTEBOOK_ORDER_FILE),
        NOTEBOOK_ORDER_HEADER,
        &file,
    )
}

/// Sorts `items` by `order`. Ids missing from `order` follow, compared by
/// `name`. An empty `order` sorts only by `name`.
pub fn sort_by_ids<T>(
    items: &mut [T],
    order: &[Uuid],
    id: impl Fn(&T) -> Uuid,
    name: impl Fn(&T) -> String,
) {
    if order.is_empty() {
        items.sort_by_key(name);
        return;
    }
    items.sort_by(|left, right| {
        let left_index = order
            .iter()
            .position(|saved| *saved == id(left))
            .unwrap_or(usize::MAX);
        let right_index = order
            .iter()
            .position(|saved| *saved == id(right))
            .unwrap_or(usize::MAX);
        left_index
            .cmp(&right_index)
            .then_with(|| name(left).cmp(&name(right)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::Notebook;

    #[test]
    fn move_before_reorders_and_ignores_a_noop() {
        let items = ["a", "b", "c"];
        assert_eq!(
            move_before(&items, &"c", Some(&"a")).as_deref(),
            Some(["c", "a", "b"].as_slice())
        );
        assert_eq!(
            move_before(&items, &"a", None).as_deref(),
            Some(["b", "c", "a"].as_slice())
        );
        assert!(move_before(&items, &"a", Some(&"b")).is_none());
        assert!(move_before(&items, &"c", None).is_none());
        assert!(move_before(&items, &"z", Some(&"a")).is_none());
    }

    #[test]
    fn place_child_appends_and_inserts_after_a_sibling() {
        let root = std::env::temp_dir().join(format!("mdwork-order-{}", Uuid::new_v4()));
        let notebook = Notebook::create(&root, "顺序").unwrap();
        notebook.create_folder("文件夹").unwrap();
        notebook.create_note("甲.md", "").unwrap();
        notebook.create_note("乙.md", "").unwrap();

        place_child(&root, "", "丙.md", None).unwrap();
        notebook.create_note("丙.md", "").unwrap();
        let order = TreeOrder::load(&root);
        assert_eq!(
            order.names("").unwrap(),
            ["文件夹", "乙.md", "甲.md", "丙.md"]
        );

        notebook.create_note("丁.md", "").unwrap();
        place_child(&root, "", "丁.md", Some("甲.md")).unwrap();
        let order = TreeOrder::load(&root);
        assert_eq!(
            order.names("").unwrap(),
            ["文件夹", "乙.md", "甲.md", "丁.md", "丙.md"]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rename_keeps_position_and_move_goes_to_the_end() {
        let root = std::env::temp_dir().join(format!("mdwork-order-mv-{}", Uuid::new_v4()));
        let notebook = Notebook::create(&root, "顺序").unwrap();
        notebook.create_note("甲.md", "").unwrap();
        notebook.create_note("乙.md", "").unwrap();
        notebook.create_folder("文件夹").unwrap();
        place_child(&root, "", "甲.md", None).unwrap();

        let renamed = notebook.rename_entry("甲.md", "甲二.md").unwrap();
        assert_eq!(renamed, "甲二.md");
        let order = TreeOrder::load(&root);
        assert_eq!(order.names("").unwrap(), ["文件夹", "乙.md", "甲二.md"]);

        notebook.rename_entry("甲二.md", "文件夹/甲二.md").unwrap();
        let order = TreeOrder::load(&root);
        assert_eq!(order.names("").unwrap(), ["文件夹", "乙.md"]);
        assert_eq!(order.names("文件夹").unwrap(), ["甲二.md"]);

        notebook.delete_entry("乙.md").unwrap();
        let order = TreeOrder::load(&root);
        assert_eq!(order.names("").unwrap(), ["文件夹"]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folder_rename_rekeys_descendant_lists() {
        let root = std::env::temp_dir().join(format!("mdwork-order-rekey-{}", Uuid::new_v4()));
        let notebook = Notebook::create(&root, "顺序").unwrap();
        notebook.create_folder("父/子").unwrap();
        notebook.create_note("父/子/笔记.md", "").unwrap();
        place_child(&root, "父/子", "笔记.md", None).unwrap();
        notebook.rename_entry("父", "父亲").unwrap();
        let order = TreeOrder::load(&root);
        assert_eq!(order.names("父亲/子").unwrap(), ["笔记.md"]);
        assert!(order.names("父/子").is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
