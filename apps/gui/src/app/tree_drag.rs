//! 笔记树侧边栏的自定义拖放逻辑。
//!
//! 与标签拖拽思路相同：由窗口跟踪指针，Windows 就
//! 不会显示复制光标。笔记本在笔记本之间移动，笔记或
//! 文件夹在同级之间排序，也可以拖到文件夹行的中间区域
//! 放入其中；上下四分之一仍表示在其前后排序。

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DragSource {
    Notebook(Uuid),
    Entry { notebook_id: Uuid, rel_path: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DragInsert {
    Notebook {
        before: Option<Uuid>,
    },
    Entry {
        notebook_id: Uuid,
        parent: String,
        /// `None` appends after the last sibling.
        before_rel: Option<String>,
    },
    /// 把拖动的条目放入文件夹；`notebook_id` 为目标笔记本，
    /// 它可以与源笔记本不同。
    IntoFolder {
        notebook_id: Uuid,
        folder_rel: String,
    },
}

pub(super) struct TreeDrag {
    pub(super) source: DragSource,
    pointer_y: f32,
    pub(super) started: bool,
    insert: Option<DragInsert>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DropEdge {
    Before(usize),
    After(usize),
    /// 整行都是放置目标（表示放入该文件夹）。
    Inside(usize),
}

struct LaidOut {
    top: Option<f32>,
    height: f32,
    kind: LaidKind,
}

#[derive(Clone)]
enum LaidKind {
    Notebook(Uuid),
    Entry {
        notebook_id: Uuid,
        rel_path: String,
        is_folder: bool,
    },
}

impl AppShell {
    pub(super) fn begin_tree_drag(&mut self, source: DragSource, y: f32, cx: &mut Context<Self>) {
        self.tree_click_suppressed = false;
        self.tree_drag = Some(TreeDrag {
            source,
            pointer_y: y,
            started: false,
            insert: None,
        });
        cx.notify();
    }

    pub(super) fn cancel_tree_drag_if_idle(&mut self, cx: &mut Context<Self>) {
        if self.tree_drag.as_ref().is_some_and(|drag| !drag.started) {
            self.tree_drag = None;
            cx.notify();
        }
    }

    /// True once, after a drag that actually moved. The row click that follows
    /// mouse-up must not also open the note.
    pub(super) fn take_tree_click(&mut self) -> bool {
        let suppressed = self.tree_click_suppressed;
        self.tree_click_suppressed = false;
        suppressed
    }

    pub(super) fn update_tree_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.tree_drag.as_ref() else {
            return;
        };
        let started = drag.started || (y - drag.pointer_y).abs() > 6.0;
        if !started {
            return;
        }
        let source = drag.source.clone();
        let insert = self.insert_at_y(&source, y);
        let drag = self.tree_drag.as_mut().expect("tree drag checked above");
        let changed = !drag.started || drag.insert != insert;
        drag.started = true;
        drag.pointer_y = y;
        drag.insert = insert;
        if changed {
            cx.notify();
        }
    }

    pub(super) fn end_tree_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.tree_drag.take() else {
            return;
        };
        if !drag.started {
            cx.notify();
            return;
        }
        self.tree_click_suppressed = true;
        if let Some(insert) = drag.insert {
            match (drag.source, insert) {
                (DragSource::Notebook(id), DragInsert::Notebook { before }) => {
                    self.reorder_notebooks(id, before, cx);
                }
                (
                    DragSource::Entry {
                        notebook_id,
                        rel_path,
                    },
                    DragInsert::Entry {
                        notebook_id: dest,
                        parent,
                        before_rel,
                    },
                ) if notebook_id == dest && parent_rel(&rel_path) == parent => {
                    self.reorder_siblings(
                        notebook_id,
                        &parent,
                        &rel_path,
                        before_rel.as_deref(),
                        cx,
                    );
                }
                (
                    DragSource::Entry {
                        notebook_id,
                        rel_path,
                    },
                    DragInsert::Entry {
                        notebook_id: dest,
                        parent,
                        before_rel,
                    },
                ) if notebook_id == dest => {
                    self.move_entry_to_parent(
                        notebook_id,
                        &rel_path,
                        &parent,
                        before_rel.as_deref(),
                        cx,
                    );
                }
                (
                    DragSource::Entry {
                        notebook_id,
                        rel_path,
                    },
                    DragInsert::IntoFolder {
                        notebook_id: dest,
                        folder_rel,
                    },
                ) => {
                    self.move_entry_into_folder(notebook_id, &rel_path, dest, &folder_rel, cx);
                }
                _ => {}
            }
        }
        cx.notify();
    }

    pub(super) fn tree_drop_edge(&self) -> Option<DropEdge> {
        let drag = self.tree_drag.as_ref()?;
        if !drag.started {
            return None;
        }
        let insert = drag.insert.as_ref()?;
        match insert {
            DragInsert::Notebook { before: Some(id) } => self
                .sidebar_rows
                .iter()
                .position(
                    |row| matches!(row, SidebarRow::Notebook { id: row_id, .. } if row_id == id),
                )
                .map(DropEdge::Before),
            DragInsert::Notebook { before: None } => {
                self.sidebar_rows.len().checked_sub(1).map(DropEdge::After)
            }
            DragInsert::Entry {
                notebook_id,
                before_rel: Some(rel),
                ..
            } => self
                .sidebar_rows
                .iter()
                .position(|row| {
                    matches!(
                        row,
                        SidebarRow::Entry { notebook_id: id, row }
                            if id == notebook_id && row.rel_path == *rel
                    )
                })
                .map(DropEdge::Before),
            DragInsert::Entry {
                notebook_id,
                parent,
                before_rel: None,
            } => last_child_block(&self.sidebar_rows, *notebook_id, parent).map(DropEdge::After),
            DragInsert::IntoFolder {
                notebook_id,
                folder_rel,
            } => self
                .sidebar_rows
                .iter()
                .position(|row| {
                    matches!(
                        row,
                        SidebarRow::Entry { notebook_id: id, row }
                            if id == notebook_id && row.rel_path == *folder_rel
                    )
                })
                .map(DropEdge::Inside),
        }
    }

    pub(super) fn render_tree_drag_listeners(&self, _cx: &mut Context<Self>) -> AnyElement {
        let shell = _cx.entity().downgrade();
        canvas(
            |_bounds, _window, _cx| {},
            move |_bounds, _state, window, _cx| {
                let mover = shell.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _phase, _window, cx| {
                    let y = f32::from(event.position.y);
                    if let Some(shell) = mover.upgrade() {
                        shell.update(cx, |this, cx| this.update_tree_drag(y, cx));
                    }
                });
                let ender = shell.clone();
                window.on_mouse_event(move |_event: &MouseUpEvent, _phase, _window, cx| {
                    if let Some(shell) = ender.upgrade() {
                        shell.update(cx, |this, cx| this.end_tree_drag(cx));
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .w(px(0.))
        .h(px(0.))
        .into_any_element()
    }

    fn insert_at_y(&self, source: &DragSource, y: f32) -> Option<DragInsert> {
        let Ok(bounds) = self.tree_bounds.lock() else {
            return None;
        };
        let mut by_index = std::collections::HashMap::new();
        for (index, bound) in bounds.iter() {
            by_index.insert(
                *index,
                (f32::from(bound.origin.y), f32::from(bound.size.height)),
            );
        }
        let laid: Vec<LaidOut> = self
            .sidebar_rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let (top, height) = by_index
                    .get(&index)
                    .map(|(top, height)| (Some(*top), *height))
                    .unwrap_or((None, 0.0));
                let kind = match row {
                    SidebarRow::Notebook { id, .. } => LaidKind::Notebook(*id),
                    SidebarRow::Entry { notebook_id, row } => LaidKind::Entry {
                        notebook_id: *notebook_id,
                        rel_path: row.rel_path.clone(),
                        is_folder: row.kind == EntryKind::Folder,
                    },
                };
                LaidOut { top, height, kind }
            })
            .collect();
        let insert = match source {
            DragSource::Notebook(_) => notebook_insert(&laid, y),
            DragSource::Entry {
                notebook_id,
                rel_path,
            } => entry_insert(&laid, *notebook_id, rel_path, y),
        };
        insert.filter(|insert| self.drag_insert_allowed(source, insert))
    }

    /// 排除磁盘上必然失败的目标（文件夹不存在、重名等），
    /// 避免放置提示给出无法完成的移动。
    fn drag_insert_allowed(&self, source: &DragSource, insert: &DragInsert) -> bool {
        let DragSource::Entry {
            notebook_id: source_notebook,
            rel_path,
        } = source
        else {
            return true;
        };
        let (dest, parent) = match insert {
            DragInsert::IntoFolder {
                notebook_id,
                folder_rel,
            } => (*notebook_id, folder_rel.as_str()),
            DragInsert::Entry {
                notebook_id,
                parent,
                ..
            } if *notebook_id == *source_notebook && parent.as_str() != parent_rel(rel_path) => {
                (*notebook_id, parent.as_str())
            }
            _ => return true,
        };
        let Some(target) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == dest)
        else {
            return false;
        };
        // 目标父必须存在；空串表示笔记本根目录。
        if !parent.is_empty()
            && !target
                .tree
                .iter()
                .any(|item| item.rel_path == parent && item.kind == EntryKind::Folder)
        {
            return false;
        }
        // 只有同一笔记本内的路径才可能构成「文件夹移入自身子树」。
        if dest == *source_notebook && parent.starts_with(&format!("{rel_path}/")) {
            return false;
        }
        let name = file_name(rel_path);
        let dest_rel = if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        };
        if dest == *source_notebook && dest_rel.eq_ignore_ascii_case(rel_path) {
            return false;
        }
        !target
            .tree
            .iter()
            .any(|item| item.rel_path.eq_ignore_ascii_case(&dest_rel))
    }

    /// 把条目移入 `folder_rel`，复用「移动到…」对话框的
    /// 同一写入线程路径，保证打开的标签、书签与排序记录同步。
    fn move_entry_into_folder(
        &mut self,
        notebook_id: Uuid,
        from: &str,
        dest_notebook_id: Uuid,
        folder_rel: &str,
        cx: &mut Context<Self>,
    ) {
        if notebook_id == dest_notebook_id {
            self.move_entry_to_parent(notebook_id, from, folder_rel, None, cx);
        } else {
            self.begin_cross_notebook_move(
                notebook_id,
                from.to_string(),
                dest_notebook_id,
                Some(folder_rel.to_string()),
                false,
                cx,
            );
        }
    }

    /// 把条目移到 `parent`（空串为笔记本根目录），并放在
    /// `before_rel` 之前；`None` 表示追加到该层末尾。
    fn move_entry_to_parent(
        &mut self,
        notebook_id: Uuid,
        from: &str,
        parent: &str,
        before_rel: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            return;
        };
        let name = file_name(from).to_string();
        let dest_rel = if parent.is_empty() {
            name.clone()
        } else {
            format!("{parent}/{name}")
        };
        let mut names = sibling_file_names(&entry.tree, parent, &entry.order);
        names.retain(|item| !item.eq_ignore_ascii_case(&name));
        let insert_at = before_rel
            .map(file_name)
            .and_then(|before| {
                names
                    .iter()
                    .position(|item| item.eq_ignore_ascii_case(before))
            })
            .unwrap_or(names.len());
        names.insert(insert_at, name);
        if !parent.is_empty() {
            // 立即展示移动后的条目，避免它被收起的文件夹隐藏。
            self.ensure_folder_expanded(notebook_id, parent, cx);
        }
        self.begin_move_entry(
            notebook_id,
            from.to_string(),
            dest_rel,
            Some((parent.to_string(), names)),
            cx,
        );
    }

    fn reorder_notebooks(&mut self, from: Uuid, before: Option<Uuid>, cx: &mut Context<Self>) {
        let ids: Vec<Uuid> = self
            .notebooks
            .iter()
            .map(|entry| entry.notebook.marker.id)
            .collect();
        let Some(ids) = order::move_before(&ids, &from, before.as_ref()) else {
            return;
        };
        let mut next = Vec::with_capacity(self.notebooks.len());
        for id in &ids {
            if let Some(pos) = self
                .notebooks
                .iter()
                .position(|entry| entry.notebook.marker.id == *id)
            {
                next.push(self.notebooks.remove(pos));
            }
        }
        next.append(&mut self.notebooks);
        self.notebooks = next;
        self.rebuild_sidebar_rows();
        let workspace = self.workspace.clone();
        let saved = ids;
        let receiver = disk::submit(move || -> mdwork_core::Result<()> {
            let Some(workspace) = workspace else {
                return Ok(());
            };
            mdwork_core::order::save_notebook_ids(workspace.root(), &saved)
        });
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(Err(err)) = result {
                    this.notify_error(format!("调整顺序失败：{err}"), cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn reorder_siblings(
        &mut self,
        notebook_id: Uuid,
        parent: &str,
        from_rel: &str,
        before_rel: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .notebooks
            .iter_mut()
            .find(|entry| entry.notebook.marker.id == notebook_id)
        else {
            return;
        };
        let siblings: Vec<String> = entry
            .rows
            .iter()
            .filter(|row| parent_rel(&row.rel_path) == parent)
            .map(|row| row.rel_path.clone())
            .collect();
        let from = from_rel.to_string();
        let before = before_rel.map(str::to_string);
        let Some(next) = order::move_before(&siblings, &from, before.as_ref()) else {
            return;
        };
        let names: Vec<String> = next.iter().map(|rel| file_name(rel).to_string()).collect();
        entry.order.set_names(parent, names);
        let order = entry.order.clone();
        let expanded = entry.expanded.clone();
        entry.rows = build_rows(&entry.tree, &expanded, &order);
        let notebook = entry.notebook.clone();
        self.rebuild_sidebar_rows();
        let receiver = disk::submit(move || notebook.save_tree_order(&order));
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(Err(err)) = result {
                    this.notify_error(format!("调整顺序失败：{err}"), cx);
                }
            });
        })
        .detach();
        cx.notify();
    }
}

fn last_child_block(rows: &[SidebarRow], notebook_id: Uuid, parent: &str) -> Option<usize> {
    let mut header = None;
    let mut last_entry = None;
    let mut passed = false;
    for (index, row) in rows.iter().enumerate() {
        match row {
            SidebarRow::Notebook { id, .. } if *id == notebook_id => {
                header = Some(index);
                passed = true;
            }
            SidebarRow::Notebook { .. } if passed => break,
            SidebarRow::Entry {
                notebook_id: id,
                row,
            } if passed && *id == notebook_id => {
                let inside = if parent.is_empty() {
                    true
                } else {
                    row.rel_path == parent || row.rel_path.starts_with(&format!("{parent}/"))
                };
                if inside {
                    last_entry = Some(index);
                } else if !parent.is_empty() && last_entry.is_some() {
                    break;
                }
            }
            _ => {}
        }
    }
    last_entry.or(header)
}

fn notebook_insert(rows: &[LaidOut], y: f32) -> Option<DragInsert> {
    let mut saw = false;
    for row in rows {
        let LaidKind::Notebook(id) = row.kind else {
            continue;
        };
        let Some(top) = row.top else {
            continue;
        };
        saw = true;
        if y < top + row.height * 0.5 {
            return Some(DragInsert::Notebook { before: Some(id) });
        }
    }
    saw.then_some(DragInsert::Notebook { before: None })
}

fn entry_insert(
    rows: &[LaidOut],
    notebook_id: Uuid,
    source_rel: &str,
    y: f32,
) -> Option<DragInsert> {
    // 文件夹行中间一半表示「放入其中」，上下四分之一
    // 仍保持原有的前后排序行为。
    if let Some(insert) = folder_insert(rows, notebook_id, source_rel, y) {
        return Some(insert);
    }
    // 指针落在父级、祖先级或笔记本标题上时，把条目移出当前文件夹。
    if let Some(insert) = outside_insert(rows, notebook_id, source_rel, y) {
        return Some(insert);
    }
    // 指针位于其他笔记本上时，不能据此重排源笔记本的
    // 同级条目，它们的布局位置完全不同。
    if hovered_foreign_notebook(rows, notebook_id, y) {
        return None;
    }
    let parent = parent_rel(source_rel).to_string();
    let siblings: Vec<(usize, String)> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| match &row.kind {
            LaidKind::Entry {
                notebook_id: id,
                rel_path,
                ..
            } if *id == notebook_id && parent_rel(rel_path) == parent => {
                Some((index, rel_path.clone()))
            }
            _ => None,
        })
        .collect();
    if siblings.is_empty() {
        return None;
    }
    for (n, (index, rel)) in siblings.iter().enumerate() {
        let Some(top) = rows[*index].top else {
            continue;
        };
        let mid = top + rows[*index].height * 0.5;
        let boundary = next_boundary(rows, &siblings, n, notebook_id, &parent);
        if y < mid {
            return Some(entry_at(notebook_id, parent, Some(rel.clone())));
        }
        if y < boundary {
            let before = siblings.get(n + 1).map(|(_, next)| next.clone());
            return Some(entry_at(notebook_id, parent, before));
        }
    }
    None
}

fn entry_at(notebook_id: Uuid, parent: String, before_rel: Option<String>) -> DragInsert {
    DragInsert::Entry {
        notebook_id,
        parent,
        before_rel,
    }
}

/// 找到 `y` 落在其中间一半区域内的可见文件夹。
///
/// 文件夹不能接收自身、自身的后代，
/// 也不能接收它当前所在的父文件夹（空操作且会遮蔽排序）。
fn folder_insert(
    rows: &[LaidOut],
    source_notebook_id: Uuid,
    source_rel: &str,
    y: f32,
) -> Option<DragInsert> {
    let source_parent = parent_rel(source_rel);
    let descendant_prefix = format!("{source_rel}/");
    for row in rows {
        let LaidKind::Entry {
            notebook_id,
            rel_path,
            is_folder,
        } = &row.kind
        else {
            continue;
        };
        if !*is_folder {
            continue;
        }
        let Some(top) = row.top else {
            continue;
        };
        if row.height <= 0.0 || y < top + row.height * 0.25 || y > top + row.height * 0.75 {
            continue;
        }
        if *notebook_id == source_notebook_id
            && (rel_path.as_str() == source_rel
                || rel_path.starts_with(descendant_prefix.as_str())
                || rel_path.as_str() == source_parent)
        {
            continue;
        }
        return Some(DragInsert::IntoFolder {
            notebook_id: *notebook_id,
            folder_rel: rel_path.clone(),
        });
    }
    None
}

fn hovered_foreign_notebook(rows: &[LaidOut], notebook_id: Uuid, y: f32) -> bool {
    rows.iter().any(|row| {
        let Some(top) = row.top else {
            return false;
        };
        if y < top || y >= top + row.height {
            return false;
        }
        match &row.kind {
            LaidKind::Notebook(id) => *id != notebook_id,
            LaidKind::Entry {
                notebook_id: id, ..
            } => *id != notebook_id,
        }
    })
}

/// 指针落在祖先层级时，生成「移出当前文件夹」的落点。
/// 同级排序仍由 `entry_insert` 原有的逻辑负责。
fn outside_insert(
    rows: &[LaidOut],
    notebook_id: Uuid,
    source_rel: &str,
    y: f32,
) -> Option<DragInsert> {
    let source_parent = parent_rel(source_rel);
    if source_parent.is_empty() {
        return None;
    }
    let descendant_prefix = format!("{source_rel}/");
    for (index, row) in rows.iter().enumerate() {
        let Some(top) = row.top else {
            continue;
        };
        if row.height <= 0.0 || y < top || y >= top + row.height {
            continue;
        }
        match &row.kind {
            LaidKind::Notebook(id) if *id == notebook_id => {
                // 拖到笔记本标题上：移到根目录最前面。
                let first = rows.iter().find_map(|row| match &row.kind {
                    LaidKind::Entry {
                        notebook_id: id,
                        rel_path,
                        ..
                    } if *id == notebook_id && parent_rel(rel_path).is_empty() => {
                        Some(rel_path.clone())
                    }
                    _ => None,
                });
                return Some(entry_at(notebook_id, String::new(), first));
            }
            LaidKind::Notebook(_) => return None,
            LaidKind::Entry {
                notebook_id: id,
                rel_path,
                ..
            } => {
                if *id != notebook_id
                    || rel_path.as_str() == source_rel
                    || rel_path.starts_with(descendant_prefix.as_str())
                {
                    return None;
                }
                let parent = parent_rel(rel_path);
                if !is_strict_ancestor(parent, source_parent) {
                    return None;
                }
                let before = if y < top + row.height * 0.5 {
                    Some(rel_path.clone())
                } else {
                    next_sibling_rel(rows, notebook_id, parent, index)
                };
                return Some(entry_at(notebook_id, parent.to_string(), before));
            }
        }
    }
    // 指针落在源笔记本最后一行之后的空白处时，追加到根目录末尾。
    let mut in_source_notebook = false;
    let mut last_bottom = None;
    let mut next_notebook_top = None;
    for row in rows {
        match &row.kind {
            LaidKind::Notebook(id) if *id == notebook_id => {
                in_source_notebook = true;
                continue;
            }
            LaidKind::Notebook(_) if in_source_notebook => {
                next_notebook_top = row.top;
                break;
            }
            _ => {}
        }
        if let Some(top) = row.top {
            last_bottom = Some(top + row.height);
        }
    }
    if in_source_notebook
        && last_bottom.is_some_and(|bottom| y >= bottom)
        && next_notebook_top.map_or(true, |top| y < top)
    {
        return Some(entry_at(notebook_id, String::new(), None));
    }
    None
}

/// `parent` 是否为 `source_parent` 的严格祖先（空串代表笔记本根目录）。
fn is_strict_ancestor(parent: &str, source_parent: &str) -> bool {
    parent.is_empty() || source_parent.starts_with(&format!("{parent}/"))
}

/// 展开的文件夹会把子行夹在同层条目之间，因此要跳过它们，
/// 找到该层真正相邻的下一个同级条目。
fn next_sibling_rel(
    rows: &[LaidOut],
    notebook_id: Uuid,
    parent: &str,
    after_index: usize,
) -> Option<String> {
    for row in rows.iter().skip(after_index + 1) {
        match &row.kind {
            LaidKind::Notebook(id) if *id != notebook_id => return None,
            LaidKind::Entry {
                notebook_id: id,
                rel_path,
                ..
            } if *id == notebook_id && parent_rel(rel_path) == parent => {
                return Some(rel_path.clone());
            }
            _ => {}
        }
    }
    None
}

fn next_boundary(
    rows: &[LaidOut],
    siblings: &[(usize, String)],
    n: usize,
    notebook_id: Uuid,
    parent: &str,
) -> f32 {
    if let Some((next_index, _)) = siblings.get(n + 1)
        && let Some(top) = rows[*next_index].top
    {
        return top;
    }
    let index = siblings[n].0;
    let mut last = index;
    for (offset, row) in rows.iter().enumerate().skip(index + 1) {
        match &row.kind {
            LaidKind::Entry {
                notebook_id: id,
                rel_path,
                ..
            } if *id == notebook_id && under_parent(rel_path, parent) => {
                if row.top.is_some() {
                    last = offset;
                }
            }
            _ => break,
        }
    }
    if let Some(top) = rows.get(last + 1).and_then(|row| row.top) {
        top
    } else {
        rows[last].top.unwrap_or(0.0) + rows[last].height
    }
}

fn under_parent(rel: &str, parent: &str) -> bool {
    if parent.is_empty() {
        return true;
    }
    rel == parent || rel.starts_with(&format!("{parent}/"))
}

#[cfg(test)]
mod tests {
    use super::{DragInsert, LaidKind, LaidOut, entry_at, entry_insert, notebook_insert};
    use uuid::Uuid;

    fn notebook(id: u128, top: f32) -> LaidOut {
        LaidOut {
            top: Some(top),
            height: 28.0,
            kind: LaidKind::Notebook(Uuid::from_u128(id)),
        }
    }

    fn entry(id: u128, rel: &str, top: f32) -> LaidOut {
        LaidOut {
            top: Some(top),
            height: 26.0,
            kind: LaidKind::Entry {
                notebook_id: Uuid::from_u128(id),
                rel_path: rel.to_string(),
                is_folder: false,
            },
        }
    }

    fn folder(id: u128, rel: &str, top: f32) -> LaidOut {
        let mut row = entry(id, rel, top);
        if let LaidKind::Entry { is_folder, .. } = &mut row.kind {
            *is_folder = true;
        }
        row
    }

    fn into_folder(id: u128, rel: &str) -> Option<DragInsert> {
        Some(DragInsert::IntoFolder {
            notebook_id: Uuid::from_u128(id),
            folder_rel: rel.to_string(),
        })
    }

    #[test]
    fn notebook_drag_splits_on_the_header_midpoint() {
        let rows = vec![notebook(1, 0.0), entry(1, "a.md", 28.0), notebook(2, 54.0)];
        assert_eq!(
            notebook_insert(&rows, 10.0),
            Some(DragInsert::Notebook {
                before: Some(Uuid::from_u128(1))
            })
        );
        assert_eq!(
            notebook_insert(&rows, 40.0),
            Some(DragInsert::Notebook {
                before: Some(Uuid::from_u128(2))
            })
        );
        assert_eq!(
            notebook_insert(&rows, 80.0),
            Some(DragInsert::Notebook { before: None })
        );
    }

    #[test]
    fn entry_drag_reorders_siblings_and_treats_children_as_the_folder() {
        let id = 7u128;
        let rows = vec![
            notebook(id, 0.0),
            entry(id, "a.md", 28.0),
            entry(id, "dir", 54.0),
            entry(id, "dir/child.md", 80.0),
            entry(id, "b.md", 106.0),
        ];
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "a.md", 60.0),
            Some(entry_at(
                Uuid::from_u128(id),
                String::new(),
                Some("dir".to_string())
            ))
        );
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "a.md", 90.0),
            Some(entry_at(
                Uuid::from_u128(id),
                String::new(),
                Some("b.md".to_string())
            ))
        );
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "a.md", 120.0),
            Some(entry_at(Uuid::from_u128(id), String::new(), None))
        );
    }

    #[test]
    fn entry_drag_into_the_middle_of_a_folder() {
        let id = 7u128;
        let rows = vec![
            notebook(id, 0.0),
            entry(id, "a.md", 28.0),
            folder(id, "dir", 54.0),
            entry(id, "b.md", 80.0),
        ];
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "a.md", 67.0),
            into_folder(id, "dir")
        );
        // 外侧四分之一仍表示「在文件夹前后排序」。
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "a.md", 57.0),
            Some(entry_at(
                Uuid::from_u128(id),
                String::new(),
                Some("dir".to_string())
            ))
        );
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "a.md", 77.0),
            Some(entry_at(
                Uuid::from_u128(id),
                String::new(),
                Some("b.md".to_string())
            ))
        );
    }

    #[test]
    fn entry_drag_never_enters_itself_its_children_or_its_current_parent() {
        let id = 7u128;
        let rows = vec![
            notebook(id, 0.0),
            folder(id, "dir", 28.0),
            folder(id, "dir/sub", 54.0),
            entry(id, "b.md", 80.0),
        ];
        for y in [41.0, 67.0] {
            let insert = entry_insert(&rows, Uuid::from_u128(id), "dir", y);
            assert!(!matches!(insert, Some(DragInsert::IntoFolder { .. })));
        }
        let rows = vec![
            notebook(id, 0.0),
            folder(id, "dir", 28.0),
            entry(id, "dir/child.md", 54.0),
            entry(id, "b.md", 80.0),
        ];
        let insert = entry_insert(&rows, Uuid::from_u128(id), "dir/child.md", 41.0);
        assert!(!matches!(insert, Some(DragInsert::IntoFolder { .. })));
    }

    #[test]
    fn entry_drag_can_enter_a_folder_in_another_notebook() {
        let rows = vec![
            notebook(1, 0.0),
            entry(1, "a.md", 28.0),
            notebook(2, 54.0),
            folder(2, "archive", 82.0),
        ];
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(1), "a.md", 95.0),
            into_folder(2, "archive")
        );
        // 其他笔记本的笔记行不是合法的同级排序目标。
        let rows = vec![
            notebook(1, 0.0),
            entry(1, "a.md", 28.0),
            notebook(2, 54.0),
            entry(2, "b.md", 82.0),
        ];
        assert_eq!(entry_insert(&rows, Uuid::from_u128(1), "a.md", 95.0), None);
    }

    #[test]
    fn entry_drag_out_of_a_folder_to_its_parent_level() {
        let id = 7u128;
        let rows = vec![
            notebook(id, 0.0),
            folder(id, "dir", 28.0),
            entry(id, "dir/child.md", 54.0),
            entry(id, "b.md", 80.0),
        ];
        // 父文件夹行上部：移到根目录，排在 dir 之前。
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "dir/child.md", 35.0),
            Some(entry_at(
                Uuid::from_u128(id),
                String::new(),
                Some("dir".to_string())
            ))
        );
        // 父文件夹行下部：移到根目录，排在 dir 之后。
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "dir/child.md", 48.0),
            Some(entry_at(
                Uuid::from_u128(id),
                String::new(),
                Some("b.md".to_string())
            ))
        );
        // 根层笔记行的上下半部分别是「插到它之前」与「追加到末尾」。
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "dir/child.md", 85.0),
            Some(entry_at(
                Uuid::from_u128(id),
                String::new(),
                Some("b.md".to_string())
            ))
        );
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "dir/child.md", 100.0),
            Some(entry_at(Uuid::from_u128(id), String::new(), None))
        );
    }

    #[test]
    fn entry_drag_out_to_the_notebook_header_or_blank_tail() {
        let id = 7u128;
        let rows = vec![
            notebook(id, 0.0),
            folder(id, "dir", 28.0),
            entry(id, "dir/child.md", 54.0),
        ];
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "dir/child.md", 10.0),
            Some(entry_at(
                Uuid::from_u128(id),
                String::new(),
                Some("dir".to_string())
            ))
        );
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "dir/child.md", 120.0),
            Some(entry_at(Uuid::from_u128(id), String::new(), None))
        );
    }

    #[test]
    fn entry_drag_out_of_a_nested_folder_one_level_or_to_the_root() {
        let id = 7u128;
        let rows = vec![
            notebook(id, 0.0),
            folder(id, "a", 28.0),
            folder(id, "a/b", 54.0),
            entry(id, "a/b/child.md", 80.0),
            entry(id, "root.md", 106.0),
        ];
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "a/b/child.md", 58.0),
            Some(entry_at(
                Uuid::from_u128(id),
                "a".to_string(),
                Some("a/b".to_string())
            ))
        );
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "a/b/child.md", 110.0),
            Some(entry_at(
                Uuid::from_u128(id),
                String::new(),
                Some("root.md".to_string())
            ))
        );
    }

    #[test]
    fn entry_drag_inside_a_folder_still_reorders_and_can_enter_another_folder() {
        let id = 7u128;
        let rows = vec![
            notebook(id, 0.0),
            folder(id, "dir", 28.0),
            entry(id, "dir/child.md", 54.0),
            entry(id, "dir/other.md", 80.0),
            entry(id, "b.md", 106.0),
        ];
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "dir/child.md", 85.0),
            Some(entry_at(
                Uuid::from_u128(id),
                "dir".to_string(),
                Some("dir/other.md".to_string())
            ))
        );
        let rows = vec![
            notebook(id, 0.0),
            folder(id, "dir", 28.0),
            entry(id, "dir/child.md", 54.0),
            folder(id, "archive", 80.0),
            entry(id, "b.md", 106.0),
        ];
        assert_eq!(
            entry_insert(&rows, Uuid::from_u128(id), "dir/child.md", 93.0),
            into_folder(id, "archive")
        );
    }
}
