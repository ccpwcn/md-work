use super::*;

/// 每个笔记本中「整篇笔记被收藏」的相对路径，供笔记树标记使用。
fn note_bookmark_paths(bookmarks: &Bookmarks) -> HashSet<String> {
    bookmarks
        .entries
        .iter()
        .filter(|entry| entry.heading.is_none())
        .map(|entry| entry.rel_path.clone())
        .collect()
}

impl AppShell {
    /// 把活动笔记本的内存书签状态同步进树标记快照。
    /// `save_bookmarks` 是所有写操作的统一入口，因此这里能覆盖切换、
    /// 删除、重命名等全部变更，无需重新读盘。
    fn sync_active_bookmark_marks(&mut self) {
        let Some(id) = self.bookmarks_notebook else {
            return;
        };
        if self.bookmarks_loading == Some(id) {
            return;
        }
        let paths = note_bookmark_paths(&self.bookmarks);
        if self.bookmark_marks.get(&id) == Some(&paths) {
            return;
        }
        Arc::make_mut(&mut self.bookmark_marks).insert(id, paths);
    }

    /// 笔记树会同时展开多个笔记本，而 `self.bookmarks` 只缓存活动笔记本；
    /// 这里在后台补齐其余笔记本的收藏路径，只加载缺失的那些。
    pub(super) fn ensure_bookmark_marks(&mut self, cx: &mut Context<Self>) {
        let pending: Vec<(Uuid, Notebook)> = self
            .notebooks
            .iter()
            .filter(|entry| {
                let id = entry.notebook.marker.id;
                !self.bookmark_marks.contains_key(&id) && !self.bookmark_marks_loading.contains(&id)
            })
            .map(|entry| (entry.notebook.marker.id, entry.notebook.clone()))
            .collect();
        if pending.is_empty() {
            return;
        }
        for (id, _) in &pending {
            self.bookmark_marks_loading.insert(*id);
        }
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    pending
                        .into_iter()
                        .map(|(id, notebook)| {
                            let paths = Bookmarks::load(&notebook)
                                .map(|bookmarks| note_bookmark_paths(&bookmarks))
                                .unwrap_or_default();
                            (id, paths)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (id, _) in &loaded {
                    this.bookmark_marks_loading.remove(id);
                }
                let marks = Arc::make_mut(&mut this.bookmark_marks);
                for (id, paths) in loaded {
                    marks.insert(id, paths);
                }
                // 活动笔记本以内存状态为准，避免后台读盘覆盖刚发生的收藏切换。
                this.sync_active_bookmark_marks();
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn ensure_bookmarks(&mut self, cx: &mut Context<Self>) {
        let target = self.active_notebook;
        if self.bookmarks_notebook == target && self.bookmarks_loading.is_none() {
            return;
        }
        if self.bookmarks_notebook == target && self.bookmarks_loading == target {
            return;
        }
        self.bookmarks_notebook = target;
        self.bookmarks = Bookmarks::default();
        self.bookmarks_loading = None;
        let Some(target) = target else {
            cx.notify();
            return;
        };
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == target)
            .map(|entry| entry.notebook.clone())
        else {
            cx.notify();
            return;
        };
        self.bookmarks_loading = Some(target);
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move { Bookmarks::load(&notebook).unwrap_or_default() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.bookmarks_loading != Some(target) || this.bookmarks_notebook != Some(target)
                {
                    return;
                }
                this.bookmarks_loading = None;
                this.bookmarks = loaded;
                this.sync_active_bookmark_marks();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn bookmarks_ready(&self) -> bool {
        self.bookmarks_loading.is_none()
    }

    pub(super) fn save_bookmarks(&mut self, cx: &mut Context<Self>) {
        let _ = cx;
        let Some(notebook_id) = self.bookmarks_notebook else {
            return;
        };
        if self.bookmarks_loading == Some(notebook_id) {
            return;
        }
        self.sync_active_bookmark_marks();
        let Some(notebook) = self
            .notebooks
            .iter()
            .find(|entry| entry.notebook.marker.id == notebook_id)
            .map(|entry| entry.notebook.clone())
        else {
            return;
        };
        let bookmarks = self.bookmarks.clone();
        disk::enqueue(move || {
            if let Err(err) = bookmarks.save(&notebook) {
                tracing::warn!(error = %err, "保存书签失败");
            }
        });
    }

    pub(super) fn toggle_note_bookmark(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        let (notebook_id, rel_path, title) = {
            let doc = doc.read(cx);
            (doc.notebook_id, doc.rel_path.clone(), doc.title())
        };
        if notebook_id.is_none() {
            self.notify_warning(t!("bookmark.external").to_string(), cx);
            return;
        }
        self.ensure_bookmarks(cx);
        if !self.bookmarks_ready() {
            return;
        }
        self.bookmarks.toggle_note(&rel_path, &title);
        self.save_bookmarks(cx);
        cx.notify();
    }

    pub(super) fn toggle_heading_bookmark(
        &mut self,
        rel_path: &str,
        heading: &str,
        line: usize,
        cx: &mut Context<Self>,
    ) {
        self.ensure_bookmarks(cx);
        if !self.bookmarks_ready() {
            return;
        }
        let title = Path::new(rel_path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| rel_path.to_string());
        self.bookmarks
            .toggle_heading(rel_path, &title, heading, line);
        self.save_bookmarks(cx);
        cx.notify();
    }

    pub(super) fn remove_bookmark(
        &mut self,
        rel_path: &str,
        heading: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.ensure_bookmarks(cx);
        if !self.bookmarks_ready() {
            return;
        }
        self.bookmarks.remove(rel_path, heading);
        self.save_bookmarks(cx);
        cx.notify();
    }
}
