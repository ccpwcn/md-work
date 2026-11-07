use super::*;
use gpui_kit::component::input::EditorState;

impl AppShell {
    pub(super) fn refresh_outline(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            self.outline = Arc::default();
            self.outline_cache = None;
            return;
        };
        let (id, revision) = {
            let doc = doc.read(cx);
            (doc.editor.entity_id(), doc.revision())
        };
        if self.outline_cache == Some((id, revision)) {
            return;
        }
        let ir = match &self.preview_cache {
            Some(cache) if cache.doc == id && cache.revision == revision => cache.ir.clone(),
            _ => {
                let text = doc.read(cx).text(cx);
                PreviewIr::from_markdown(&text)
            }
        };
        self.outline = Arc::new(ir.outline());
        self.outline_cache = Some((id, revision));
    }

    /// 点击大纲导航当前活动文档到指定标题（源码行号从 1 开始）。
    /// 不依赖笔记本归属，外部打开的文档同样可用：预览态把标题滚到视口
    /// 顶部附近，编辑态把光标移到标题行并一次性滚到编辑区上沿。
    pub(super) fn jump_to_outline(
        &mut self,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.active_doc_entity().cloned() else {
            return;
        };
        if doc.read(cx).preview_mode {
            self.ensure_preview(cx);
            let Some(view) = self.preview_view.clone() else {
                return;
            };
            let (handle, headings) = {
                let view = view.read(cx);
                (view.scroll_handle(), view.headings())
            };
            if let Some((_, y)) = headings.into_iter().find(|(l, _)| *l == line) {
                handle.set_offset(Point::new(px(0.), -(y - px(8.))));
                // set_offset 只写入共享偏移，实际滚动要在滚动区下次 prepaint
                // 时才应用。正文被 Entity::cached 缓存，仅通知 AppShell 会命中
                // 缓存跳过 prepaint，偏移不生效；必须把 MarkdownPreview 标脏。
                view.update(cx, |_, cx| cx.notify());
            }
        } else {
            let heading_line = line.saturating_sub(1) as u32;
            doc.update(cx, |doc, cx| doc.set_cursor(heading_line, 0, window, cx));
            // set_cursor_position 自带最小揭示（ScrollPadding::Minimal）：
            // 向上跳落到视口顶部，向下跳落到视口底部。这个位置会被立即
            // 画一帧，看起来就是「先跳到反方向，再抖一下」。这里用当前
            // 视口偏移覆盖揭示结果，让中间帧保持原样，等布局产出标题行
            // 坐标后直接滚到最终位置，全程只有一次可见滚动。
            let editor = doc.read(cx).editor.clone();
            let keep = editor.read(cx).scroll_offset();
            editor.update(cx, |editor, cx| editor.set_scroll_offset(keep, cx));
            // on_next_frame 的回调在下一帧请求开始时、布局之前执行，第一次
            // 回调读到的还是点击前的布局（cursor_bounds 仍指向旧光标行），
            // 必须在其内部再排一次，第二次才能读到本帧产出的 cursor_bounds。
            window.on_next_frame(move |window, _cx| {
                window.on_next_frame(move |_, cx| {
                    align_editor_line(&editor, heading_line, cx);
                });
            });
        }
        cx.notify();
    }

    /// 反查当前内容对应的大纲标题下标：预览态按滚动位置取视口顶部所在
    /// 章节（vnote 式即时跟随），编辑态按光标所在行（zed 式光标跟随）。
    fn active_outline_index(&self, cx: &App) -> Option<usize> {
        let items = self.outline.clone();
        if items.is_empty() {
            return None;
        }
        let doc = self.active_doc_entity().cloned()?;
        if doc.read(cx).preview_mode {
            let view = self.preview_view.as_ref()?.read(cx);
            let top_y = -view.scroll_handle().offset().y;
            let headings = view.headings();
            let active_line = headings
                .iter()
                .rev()
                .find(|(_, y)| *y <= top_y + px(2.))
                .map(|(l, _)| *l)
                .or_else(|| headings.first().map(|(l, _)| *l))?;
            items.iter().position(|it| it.line == active_line)
        } else {
            let cursor_line = doc.read(cx).cursor(cx).0 as usize + 1;
            items.iter().rposition(|it| it.line <= cursor_line)
        }
    }

    /// 内容变化时同步大纲高亮与滚动位置。仅在大纲可见时由面板调用；
    /// 下标变化才滚动大纲，避免每帧打断用户手动浏览大纲。
    pub(super) fn sync_outline_to_content(&mut self, cx: &mut Context<Self>) {
        let active = self.active_outline_index(cx);
        if active != self.active_outline {
            self.active_outline = active;
            if let Some(index) = active {
                self.outline_scroll
                    .scroll_to_item(index, ScrollStrategy::Center);
            }
            cx.notify();
        }
    }

    pub(super) fn set_search_scope(&mut self, all: bool, cx: &mut Context<Self>) {
        if self.search_all == all {
            return;
        }
        self.search_all = all;
        self.schedule_search(cx);
    }

    pub(super) fn toggle_search_option(&mut self, option: SearchOption, cx: &mut Context<Self>) {
        match option {
            SearchOption::Case => self.search_case = !self.search_case,
            SearchOption::Regex => self.search_regex = !self.search_regex,
            SearchOption::Filename => self.search_filename = !self.search_filename,
        }
        self.schedule_search(cx);
    }

    pub(super) fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation += 1;
        let generation = self.search_generation;
        self.search_cancel.store(true, Ordering::Relaxed);
        self.search_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.search_cancel.clone();

        let mut query = SearchQuery::new(self.search_input.read(cx).value().to_string());
        query.case_sensitive = self.search_case;
        query.regex = self.search_regex;
        query.filename = self.search_filename;
        if query.is_blank() {
            self.search_results.clear();
            self.search_status = SearchStatus::Idle;
            cx.notify();
            return;
        }

        let notebooks: Vec<Notebook> = if self.search_all {
            self.notebooks
                .iter()
                .map(|entry| entry.notebook.clone())
                .collect()
        } else {
            self.active_notebook
                .and_then(|id| {
                    self.notebooks
                        .iter()
                        .find(|entry| entry.notebook.marker.id == id)
                })
                .map(|entry| entry.notebook.clone())
                .into_iter()
                .collect()
        };
        self.search_status = SearchStatus::Running;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(220))
                .await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    let mut hits = Vec::new();
                    let mut error = None;
                    for notebook in &notebooks {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        match search::search_notebook(
                            notebook,
                            &query,
                            &cancel,
                            search::DEFAULT_MAX_HITS,
                        ) {
                            Ok(result) => {
                                let stop = result.cancelled;
                                hits.extend(result.hits);
                                if stop {
                                    break;
                                }
                            }
                            Err(err) => {
                                error = Some(err.to_string());
                                break;
                            }
                        }
                    }
                    (hits, error)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                match outcome {
                    (_, Some(err)) => {
                        this.search_results.clear();
                        this.search_status = SearchStatus::Failed(err);
                    }
                    (hits, None) => {
                        this.search_results = hits;
                        this.search_status = SearchStatus::Idle;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// 把编辑区光标所在的标题行对齐到视口上沿附近。
///
/// `cursor_layout` 的 y 是内容坐标（未滚动），而 `text_bounds()` 返回的
/// 视口矩形在 paint 时已被滚动偏移平移过；两者相减会多算一份滚动量，
/// 目标偏移随即被钳到文末。这里用保持未滚动的 `input_bounds()` 作基准：
/// 目标偏移 = 上边距 − 内容 y，与滚动方向无关。必须等点击后的布局帧
/// 画完再读，否则 cursor_bounds 还指向旧光标行。
fn align_editor_line(editor: &Entity<EditorState>, line: u32, cx: &mut App) {
    editor.update(cx, |editor, cx| {
        if editor.cursor_position().line != line {
            return;
        }
        let Some((cursor_bounds, _)) = editor.cursor_layout() else {
            return;
        };
        let content_y = cursor_bounds.top() - editor.input_bounds().top();
        let offset = editor.scroll_offset();
        let target_y = (px(8.) - content_y).min(px(0.));
        let delta = offset.y - target_y;
        if delta < px(1.) && delta > px(-1.) {
            return;
        }
        editor.set_scroll_offset(Point::new(offset.x, target_y), cx);
    });
}

#[cfg(test)]
mod tests {
    use super::align_editor_line;
    use gpui_kit::base::input::Position;
    use gpui_kit::component::input::{Editor, EditorState};
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, VisualTestContext, Window, div, px,
    };

    const LINES: u32 = 220;

    fn sample_text() -> String {
        (0..LINES).map(|n| format!("line {n}\n")).collect()
    }

    struct Harness {
        editor: Entity<EditorState>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(Editor::new(&self.editor).h_full().w_full())
        }
    }

    /// 复刻 `jump_to_outline` 的编辑态时序：设置光标并抵消最小揭示 →
    /// 第一回调（布局前）→ 保持原视口的中间帧 → 第二回调对齐 → 应用帧。
    /// 返回点击前的视口偏移，供断言中间帧没有跳动。
    fn jump(
        editor: &Entity<EditorState>,
        view: &Entity<Harness>,
        line: u32,
        cx: &mut VisualTestContext,
    ) -> gpui_kit::Point<gpui_kit::Pixels> {
        let align_editor = editor.clone();
        let pinned = cx.update(|window, cx| {
            let before = editor.read(cx).scroll_offset();
            editor.update(cx, |state, cx| {
                state.set_cursor_position(Position::new(line, 0), window, cx)
            });
            editor.update(cx, |state, cx| state.set_scroll_offset(before, cx));
            view.update(cx, |_, cx| cx.notify());
            window.on_next_frame(move |window, _cx| {
                window.on_next_frame(move |_, cx| {
                    align_editor_line(&align_editor, line, cx);
                });
            });
            before
        });
        // 下一帧请求：先跑第一回调（此时还是旧布局），再画中间帧。
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        // 中间帧必须保持点击前的视口，不能先画出编辑器的最小揭示位置。
        let after_first = cx.read(|cx| editor.read(cx).scroll_offset());
        assert!(
            after_first == pinned,
            "intermediate frame must keep the previous viewport, got {after_first:?} want {pinned:?}"
        );
        // 第二回调读到点击后的布局，设置对齐偏移。（编辑器自身也会排
        // 光标闪烁等帧回调，这里只要求本轮回调都被执行。）
        cx.update(|window, cx| {
            assert!(window.simulate_next_frame(cx) >= 1);
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        pinned
    }

    /// 视口内光标行的顶边相对视口顶边的位置。
    fn heading_viewport_y(editor: &Entity<EditorState>, cx: &TestAppContext) -> gpui_kit::Pixels {
        cx.read(|cx| {
            let state = editor.read(cx);
            let (cursor_bounds, _) = state.cursor_layout().expect("editor must lay out");
            let content_y = cursor_bounds.top() - state.input_bounds().top();
            content_y + state.scroll_offset().y
        })
    }

    fn assert_near_top(editor: &Entity<EditorState>, cx: &TestAppContext, what: &str) {
        let y = heading_viewport_y(editor, cx);
        let delta = y - px(8.);
        assert!(
            delta < px(1.) && delta > px(-1.),
            "{what} should land near the top, got {y:?}"
        );
    }

    /// 回归用例：无论点当前导航位上方还是下方的标题，编辑区都统一停在
    /// 标题行的上沿，而不是「向上贴顶、向下贴底」。
    #[gpui::test]
    fn outline_jump_lands_at_viewport_top(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut editor = None;
        let (view, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|cx| {
                EditorState::new(window, cx)
                    .soft_wrap(true)
                    .default_value(sample_text())
            });
            editor = Some(state.clone());
            Harness { editor: state }
        });
        let editor = editor.unwrap();

        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        // 先跳到远处，再依次测「向下跳」和「向上跳」。
        jump(&editor, &view, 150, cx);
        assert_near_top(&editor, cx, "downward jump");
        jump(&editor, &view, 30, cx);
        assert_near_top(&editor, cx, "upward jump");
    }
}
