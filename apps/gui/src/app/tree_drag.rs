//! Custom drag-reorder for the notes sidebar.
//!
//! Same idea as tab drag: the pointer is tracked by the window, so Windows
//! does not show the copy cursor. A notebook moves among notebooks. A note or
//! folder moves among its own siblings.

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
}

struct LaidOut {
    top: Option<f32>,
    height: f32,
    kind: LaidKind,
}

#[derive(Clone)]
enum LaidKind {
    Notebook(Uuid),
    Entry { notebook_id: Uuid, rel_path: String },
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
                        let _ = shell.update(cx, |this, cx| this.update_tree_drag(y, cx));
                    }
                });
                let ender = shell.clone();
                window.on_mouse_event(move |_event: &MouseUpEvent, _phase, _window, cx| {
                    if let Some(shell) = ender.upgrade() {
                        let _ = shell.update(cx, |this, cx| this.end_tree_drag(cx));
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
                    },
                };
                LaidOut { top, height, kind }
            })
            .collect();
        match source {
            DragSource::Notebook(_) => notebook_insert(&laid, y),
            DragSource::Entry {
                notebook_id,
                rel_path,
            } => entry_insert(&laid, *notebook_id, rel_path, y),
        }
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
    let parent = parent_rel(source_rel).to_string();
    let siblings: Vec<(usize, String)> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| match &row.kind {
            LaidKind::Entry {
                notebook_id: id,
                rel_path,
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
            },
        }
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
}
