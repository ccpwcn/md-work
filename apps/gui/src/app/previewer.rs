use super::util::*;
use super::*;
use mdwork_render::preview::Block;

/// 预览右键复制的内容形式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewCopyKind {
    /// 复制渲染后的文本。
    Text,
    /// 复制选区对应的 Markdown 源码。
    Markdown,
}

impl AppShell {
    pub(super) fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        if doc.read(cx).official.is_some() {
            // 官方发布文档永远停在只读预览，用户点击切换按钮时给出提示。
            self.warn_official_readonly(cx);
            return;
        }
        let preview = !doc.read(cx).preview_mode;
        doc.update(cx, |doc, _| doc.preview_mode = preview);
        if preview {
            self.ensure_preview(cx);
        } else {
            let (line, column) = doc.read(cx).cursor(cx);
            doc.update(cx, |doc, cx| doc.set_cursor(line, column, window, cx));
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
        if self.is_preview_mode(cx) {
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
            self.preview_view = None;
            self.selection_overlay = None;
            self.preview_frame = None;
            self.preview_render_config = None;
            return;
        };
        let (id, revision) = {
            let doc = doc.read(cx);
            (doc.editor.entity_id(), doc.revision())
        };
        let config = (
            self.config.code_wrap,
            self.config.note_font.clone(),
            self.config.note_font_size,
        );
        let cache_ok = self
            .preview_cache
            .as_ref()
            .is_some_and(|cache| cache.doc == id && cache.revision == revision);
        let view_ok = self.preview_view.is_some()
            && self.selection_overlay.is_some()
            && self.preview_render_config.as_ref() == Some(&config);
        if cache_ok && view_ok {
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
        self.preview_render_config = Some(config.clone());

        // 正文与选区覆盖层共享同一份选区帧数据：正文只收集布局，覆盖层
        // 负责投影与绘制高亮。内容层随后由上层用 `Entity::cached` 缓存，
        // 只有选区变化才会重绘覆盖层，拖选大文档时才不会每帧重排正文。
        let frame = crate::preview_selection::new_frame_selection();
        let base = doc.read(cx).path.parent().map(|path| path.to_path_buf());
        let ir = self.preview_cache.as_ref().unwrap().ir.clone();
        let preview_view = cx.new(|_| {
            crate::preview::MarkdownPreview::new(
                ir,
                base,
                self.math.clone(),
                self.image_paths.clone(),
                config.0,
                config.1,
                config.2,
                frame.clone(),
            )
        });
        // 预览滚动只通知 MarkdownPreview 自身，不会像编辑态那样经 Doc 冒泡到
        // AppShell。订阅它，滚动时 AppShell 才会重绘大纲面板并反查当前标题，
        // 实现「内容滚动→大纲跟随」。旧视图被替换 dropping 后订阅自动失效。
        cx.observe(&preview_view, |_, _, cx| cx.notify()).detach();
        let selection_color = cx.theme().selection;
        let overlay = cx.new(|cx| {
            crate::preview_selection::SelectionOverlay::new(frame.clone(), selection_color, cx)
        });
        self.preview_frame = Some(frame);
        self.preview_view = Some(preview_view);
        self.selection_overlay = Some(overlay);
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

        if pending.is_empty() {
            return;
        }
        // 所有公式合并成一个后台任务、完成后只通知一次：
        // 逐个任务各自 `cx.notify()` 会让大文档预览在打开时
        // 反复重建整棵元素树。
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    pending
                        .into_iter()
                        .map(|(key, tex, display)| {
                            let path = mdwork_export::math::math_svg(
                                &tex,
                                &mdwork_export::math::MathOptions {
                                    fill: &fill,
                                    size_pt: preview::MATH_SIZE_PT,
                                    display,
                                },
                            )
                            .ok()
                            .and_then(|svg| write_math_cache(key, &svg));
                            (key, path)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (key, path) in results {
                    this.math_pending.remove(&key);
                    this.math.insert(key, path);
                }
                if let Some(view) = this.preview_view.clone() {
                    view.update(cx, |view, cx| {
                        view.set_assets(this.math.clone(), this.image_paths.clone(), cx);
                    });
                } else {
                    cx.notify();
                }
            });
        })
        .detach();
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
            .and_then(|doc| doc.read(cx).path.parent().map(Path::to_path_buf));
        let mut pending = Vec::new();
        for src in sources {
            let key = preview::image_key(&src, base.as_deref());
            if self.image_paths.contains_key(&key) || !self.images_pending.insert(key) {
                continue;
            }
            pending.push((key, src));
        }
        if pending.is_empty() {
            return;
        }
        // 与公式同理：整批图片只重绘一次，避免 README 这类
        // 含大量徽章图片的文档在打开时连续触发全量重绘。
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    pending
                        .into_iter()
                        .map(|(key, src)| {
                            (key, preview::resolve_preview_image(&src, base.as_deref()))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                for (key, resolved) in results {
                    this.images_pending.remove(&key);
                    this.image_paths.insert(key, resolved);
                }
                if let Some(view) = this.preview_view.clone() {
                    view.update(cx, |view, cx| {
                        view.set_assets(this.math.clone(), this.image_paths.clone(), cx);
                    });
                } else {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn toggle_code_wrap(&mut self, cx: &mut Context<Self>) {
        self.config.code_wrap = !self.config.code_wrap;
        self.save_config(cx);
        cx.notify();
    }

    pub(super) fn copy_preview(&mut self, cx: &mut Context<Self>) {
        self.ensure_preview(cx);
        let Some(text) = self
            .preview_cache
            .as_ref()
            .map(|cache| cache.ir.to_plain_text())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.flash_copied(cx);
    }

    /// 预览区的右键操作：复制选中的渲染文本
    /// （无选中时复制全文），或复制选中内容对应的 Markdown 源码。
    pub(super) fn copy_preview_text(
        &mut self,
        selected: &str,
        kind: PreviewCopyKind,
        cx: &mut Context<Self>,
    ) {
        self.ensure_preview(cx);
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        let source = doc.read(cx).text(cx).to_string();
        let ir = self.preview_cache.as_ref().map(|cache| cache.ir.clone());
        let text = match kind {
            PreviewCopyKind::Markdown => ir
                .as_ref()
                .map(|ir| markdown_for_selection(ir, &source, selected))
                .unwrap_or(source),
            PreviewCopyKind::Text if !selected.is_empty() => selected.to_string(),
            PreviewCopyKind::Text => ir.as_ref().map(|ir| ir.to_plain_text()).unwrap_or(source),
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.flash_copied(cx);
    }

    fn flash_copied(&mut self, cx: &mut Context<Self>) {
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

/// 返回顶层预览块在源码中的行号（从 1 开始）。
fn block_source_line(block: &Block) -> usize {
    match block {
        Block::Heading { line, .. } => *line,
        Block::Paragraph { line, .. } => *line,
        Block::CodeBlock { line, .. } => *line,
        Block::List { line, .. } => *line,
        Block::Quote { line, .. } => *line,
        Block::Table { line, .. } => *line,
        Block::Image { line, .. } => *line,
        Block::Div { line, .. } => *line,
        Block::Rule { line } => *line,
        Block::Blank { line, .. } => *line,
        Block::Math { line, .. } => *line,
        Block::FootnoteDefinition { line, .. } => *line,
    }
}

/// 把渲染文本的选区映射回生成它的 Markdown 源码行。
/// 无法定位选区时（例如跨图片或表格）
/// 退化为复制整篇源码。
fn markdown_for_selection(ir: &PreviewIr, source: &str, selected: &str) -> String {
    let needle = selected.trim();
    if needle.is_empty() {
        return source.to_string();
    }
    let lines: Vec<&str> = source.lines().collect();
    let plain = ir.to_plain_text();
    let first_line = needle
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(needle)
        .trim();
    let (start, end) = match plain.find(needle) {
        Some(pos) => (pos, pos + needle.len()),
        None => match plain.find(first_line) {
            Some(pos) if !first_line.is_empty() => (pos, pos + first_line.len()),
            _ => return source.to_string(),
        },
    };

    let mut cursor = 0usize;
    let mut from_line: Option<usize> = None;
    let mut to_line = 0usize;
    for (index, block) in ir.blocks.iter().enumerate() {
        let segment = PreviewIr {
            blocks: vec![block.clone()],
            styles: Default::default(),
        }
        .to_plain_text();
        if segment.trim().is_empty() {
            continue;
        }
        let seg_start = cursor;
        let seg_end = seg_start + segment.len();
        // `to_plain_text` 用一个换行符连接所有非空块。
        cursor = seg_end + 1;
        if seg_end <= start || seg_start >= end {
            continue;
        }
        let first = block_source_line(block).saturating_sub(1).min(lines.len());
        let next = ir.blocks[index + 1..]
            .iter()
            .map(block_source_line)
            .find(|line| *line > 0)
            .map(|line| line.saturating_sub(1).min(lines.len()))
            .unwrap_or(lines.len());
        if from_line.is_none() {
            from_line = Some(first);
        }
        to_line = to_line.max(next.max(first));
    }
    match from_line {
        Some(from) if to_line > from => lines[from..to_line]
            .join("\n")
            .trim_end_matches('\n')
            .to_string(),
        Some(from) => lines[from..].join("\n").trim_end_matches('\n').to_string(),
        None => source.to_string(),
    }
}

#[cfg(test)]
mod tests {
    // 不能用 `use super::*`：GPUI 的通配导入会遮蔽内置的 `#[test]`。
    use super::markdown_for_selection;
    use mdwork_render::preview::PreviewIr;

    const SOURCE: &str = "# Title\n\nHello **world**\n\nSecond line\n";

    #[test]
    fn empty_selection_copies_whole_source() {
        let ir = PreviewIr::from_markdown(SOURCE);
        assert_eq!(markdown_for_selection(&ir, SOURCE, ""), SOURCE);
        assert_eq!(markdown_for_selection(&ir, SOURCE, "  \n "), SOURCE);
    }

    #[test]
    fn selection_maps_back_to_markdown_source() {
        let ir = PreviewIr::from_markdown(SOURCE);
        assert_eq!(markdown_for_selection(&ir, SOURCE, "Title"), "# Title");
        assert_eq!(
            markdown_for_selection(&ir, SOURCE, "Hello world"),
            "Hello **world**"
        );
    }

    #[test]
    fn unmatched_selection_falls_back_to_whole_source() {
        let ir = PreviewIr::from_markdown(SOURCE);
        assert_eq!(markdown_for_selection(&ir, SOURCE, "not in here"), SOURCE);
    }
}
