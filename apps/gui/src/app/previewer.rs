use super::util::*;
use super::*;

impl AppShell {
    pub(super) fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_mode = !self.preview_mode;
        if !self.preview_mode {
            if let Some(index) = self.active_doc {
                if let Some(doc) = self.docs.get(index).cloned() {
                    let (line, column) = doc.read(cx).cursor(cx);
                    doc.update(cx, |doc, cx| doc.set_cursor(line, column, window, cx));
                }
            }
        }
        cx.notify();
    }

    pub(super) fn wrap_selection(
        &mut self,
        prefix: &str,
        suffix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview_mode {
            return;
        }
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        let selection = doc.read(cx).editor.read(cx).selected_value().to_string();
        let (text, caret_back) = if selection.is_empty() {
            (format!("{prefix}{suffix}"), suffix.chars().count() as u32)
        } else {
            (format!("{prefix}{selection}{suffix}"), 0)
        };
        doc.update(cx, |doc, cx| doc.insert_text(&text, window, cx));
        if caret_back > 0 {
            let (line, column) = doc.read(cx).cursor(cx);
            let target = column.saturating_sub(caret_back);
            doc.update(cx, |doc, cx| doc.set_cursor(line, target, window, cx));
        }
        cx.notify();
    }

    pub(super) fn ensure_preview(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            self.preview_cache = None;
            return;
        };
        let (id, revision) = {
            let doc = doc.read(cx);
            (doc.editor.entity_id(), doc.revision())
        };
        if self
            .preview_cache
            .as_ref()
            .is_some_and(|cache| cache.doc == id && cache.revision == revision)
        {
            return;
        }
        let text = doc.read(cx).text(cx);
        let doc_changed = self.preview_cache.as_ref().map(|cache| cache.doc) != Some(id);
        self.preview_cache = Some(PreviewCache {
            doc: id,
            revision,
            ir: PreviewIr::from_markdown(&text),
        });
        if doc_changed {
            self.image_paths.clear();
            self.images_pending.clear();
        }
        self.queue_math(cx);
        self.queue_images(cx);
    }

    /// Rasterises any equation in the current preview that has no cached SVG yet.
    pub(super) fn queue_math(&mut self, cx: &mut Context<Self>) {
        let Some(cache) = &self.preview_cache else {
            return;
        };
        let mut formulas = Vec::new();
        preview::collect_math(&cache.ir, &mut |tex, display| {
            formulas.push((tex.to_string(), display));
        });

        let fill = preview::fill_hex(cx.theme().foreground);
        let mut pending = Vec::new();
        for (tex, display) in formulas {
            let key = preview::math_key(&tex, display, &fill, preview::MATH_SIZE_PT);
            if self.math.contains_key(&key) || !self.math_pending.insert(key) {
                continue;
            }
            pending.push((key, tex, display));
        }

        for (key, tex, display) in pending {
            let fill = fill.clone();
            cx.spawn(async move |this, cx| {
                let path = cx
                    .background_executor()
                    .spawn(async move {
                        mdwork_export::math::math_svg(
                            &tex,
                            &mdwork_export::math::MathOptions {
                                fill: &fill,
                                size_pt: preview::MATH_SIZE_PT,
                                display,
                            },
                        )
                        .ok()
                        .and_then(|svg| write_math_cache(key, &svg))
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.math_pending.remove(&key);
                    this.math.insert(key, path);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// Resolves preview image paths off the render thread so paint never stats
    /// the filesystem.
    pub(super) fn queue_images(&mut self, cx: &mut Context<Self>) {
        let Some(cache) = &self.preview_cache else {
            return;
        };
        let mut sources = Vec::new();
        preview::collect_images(&cache.ir, &mut |src| sources.push(src.to_string()));
        if sources.is_empty() {
            return;
        }
        let base = self
            .active_doc_entity()
            .map(|doc| doc.read(cx).path.parent().map(Path::to_path_buf))
            .flatten();
        let mut pending = Vec::new();
        for src in sources {
            let key = preview::image_key(&src, base.as_deref());
            if self.image_paths.contains_key(&key) || !self.images_pending.insert(key) {
                continue;
            }
            pending.push((key, src));
        }
        for (key, src) in pending {
            let base = base.clone();
            cx.spawn(async move |this, cx| {
                let path = cx
                    .background_executor()
                    .spawn(async move {
                        preview::resolve_image_path(&src, base.as_deref())
                            .filter(|path| path.is_file())
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.images_pending.remove(&key);
                    this.image_paths.insert(key, path);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub(super) fn toggle_code_wrap(&mut self, cx: &mut Context<Self>) {
        self.config.code_wrap = !self.config.code_wrap;
        self.save_config(cx);
        cx.notify();
    }

    pub(super) fn copy_preview(&mut self, cx: &mut Context<Self>) {
        self.ensure_preview(cx);
        let Some(cache) = &self.preview_cache else {
            return;
        };
        let text = cache.ir.to_plain_text();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            });
        })
        .detach();
    }
}
