use super::*;

impl AppShell {
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
