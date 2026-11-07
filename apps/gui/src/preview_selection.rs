//! 参与 GPUI 窗口级文本选区的预览富文本片段。
//!
//! 整篇预览只注册【一个】选区参与者：每个片段在 `prepaint` 时
//! 把自己的布局写入本帧共享的 `PreviewFrameSelection`，由
//! [`SelectionOverlay`] 统一注册几何并调用 `update_runs` 投影选区，
//! 片段在 `paint` 时按自己的下标绘制高亮。
//!
//! 性能是关键约束：预览文档可能很大（上百 KB），而拖选时鼠标每移动
//! 一下就要求重绘。若让选区高亮和正文混在同一个元素树里，每次重绘
//! 都要重新排版整篇正文，拖动会明显滞后、滚动与翻页也会卡顿。
//!
//! 因此把两者拆开：
//! - 正文（[`SelectableStyledText`]）只负责排版，并把每段布局写进共享的
//!   `PreviewFrameSelection`，**不画高亮**；正文层在文档不变时由上层用
//!   `Entity::cached` 整段缓存、跳过重绘。
//! - 选区由独立的 [`SelectionOverlay`] 负责：它订阅窗口选区变化，只在
//!   自身被通知时重绘，用共享的 `runs` 投影并画高亮。覆盖层很轻——即使
//!   全选也只是若干个矩形。
//!
//! 这样拖选大文档时，正文层保持缓存不动，每帧只重绘覆盖层。

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::{TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};
use gpui_kit::{Context, Render, Subscription, *};

/// 一帧内所有预览片段共享的选区数据。每帧重建，句柄由代理元素
/// 通过 GPUI 元素状态跨帧保留。
#[derive(Default)]
pub struct PreviewFrameSelection {
    runs: Vec<TextSelectionRun>,
}

impl PreviewFrameSelection {
    /// 清空本帧收集的选区片段。正文层每次全量重建前调用：缓存失效
    /// （滚动、缩放、资产加载）会重新 prepaint 并追加 runs，不清空会让
    /// 覆盖层把同一选区按重复片段画多次（高亮叠色）且投影越来越慢。
    pub fn clear(&mut self) {
        self.runs.clear();
    }

    /// 当前收集的选区片段数量（用于回归测试验证重绘不会累积 runs）。
    pub fn len(&self) -> usize {
        self.runs.len()
    }
}

pub type SharedPreviewSelection = Rc<RefCell<PreviewFrameSelection>>;

pub fn new_frame_selection() -> SharedPreviewSelection {
    Rc::new(RefCell::new(PreviewFrameSelection::default()))
}

/// 选区覆盖层：不参与正文排版，只把窗口选区投影到本篇预览的文字
/// 片段上并画出高亮。它与「正文内容」是分开的实体——正文层在文档
/// 不变时可由上层用 `Entity::cached` 整段缓存、跳过重绘，而选区变化
/// 只需重绘这个轻量的覆盖层，拖选大文档时才不会每帧重排上千个文本
/// 元素。
pub struct SelectionOverlay {
    frame: SharedPreviewSelection,
    handle: TextSelectionHandle,
    color: Hsla,
    _subscription: Subscription,
}

impl SelectionOverlay {
    pub fn new(frame: SharedPreviewSelection, color: Hsla, cx: &mut Context<Self>) -> Self {
        let handle = TextSelectionHandle::new("", cx);
        // 选区每次变化（mouse_move、滚动都会发布新快照）只重绘覆盖层
        // 自身，而不是整窗：正文层因此可以保持缓存、不被拖选拖垮。
        let entity = cx.entity().clone();
        let _subscription =
            handle.subscribe(move |_, cx: &mut App| cx.notify(entity.entity_id()), cx);
        Self {
            frame,
            handle,
            color,
            _subscription,
        }
    }
}

impl Render for SelectionOverlay {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SelectionOverlayElement::new(self.frame.clone(), self.color, self.handle.clone())
    }
}

/// 覆盖层实际绘制的元素。独立成元素后，测试也能直接挂载它。
pub struct SelectionOverlayElement {
    frame: SharedPreviewSelection,
    handle: TextSelectionHandle,
    color: Hsla,
}

impl SelectionOverlayElement {
    pub fn new(frame: SharedPreviewSelection, color: Hsla, handle: TextSelectionHandle) -> Self {
        Self {
            frame,
            handle,
            color,
        }
    }
}

impl IntoElement for SelectionOverlayElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectionOverlayElement {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some("preview-selection-overlay".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // 选区句柄必须跨帧稳定：鼠标手势在某一帧建立选区，锚点/光标端点指向的
        // 是当时的参与者实体。下一帧若重建新句柄，端点就匹配不到任何已注册
        // 参与者，`participates` 失败、整篇预览失去选区（`selected_text` 返回
        // 空）。用元素状态把句柄保留下来——生产里它由 `SelectionOverlay` 实体
        // 持有、天然稳定，这里让裸元素形式（测试）也保持一致。
        let fallback = self.handle.clone();
        let handle = window.with_element_state(
            id.expect("SelectionOverlayElement::id 必须返回稳定的元素 id"),
            |retained: Option<TextSelectionHandle>, _| {
                let handle = retained.unwrap_or(fallback);
                (handle.clone(), handle)
            },
        );
        self.handle = handle.clone();
        (window.request_layout(Style::default(), [], cx), handle)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let viewport = window.content_mask().bounds;
        window.insert_hitbox(viewport, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let frame = self.frame.borrow();
        let runs = frame.runs.clone();
        drop(frame);
        if runs.is_empty() {
            return;
        }
        // 文字矩形的并集作为参与者坐标原点：滚动时它与文字一起移动，
        // 选区端点跟着文字走，不会滞留在屏幕上的旧位置让后续滚动继续
        // "长"出别的选区。text_bounds 只覆盖真正的文字块，空白处的按压
        // 也不再被当作拖选起点。
        let text_bounds = runs
            .iter()
            .map(|run| run.bounds())
            .collect::<Vec<Bounds<Pixels>>>();
        let content_bounds = text_bounds
            .iter()
            .copied()
            .reduce(|acc, bounds| acc.union(&bounds))
            .unwrap_or(hitbox.bounds);
        let registration = TextSelectionRegistration::new(hitbox.clone(), content_bounds)
            .with_text_bounds(text_bounds)
            .with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);

        // 注册会重算并发布快照（例如滚动改变了端点的窗口坐标），投影
        // 必须在其后执行，本帧高亮与复制文本才来自同一个快照。
        let projection = handle.update_runs(&runs, cx);
        for (run, range) in runs.iter().zip(projection.ranges()) {
            if let Some(range) = range {
                let layout = run.layout().clone();
                paint_selection(&layout, range.clone(), self.color, window);
            }
        }
    }
}

pub struct SelectableStyledText {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
    order: u64,
    frame: SharedPreviewSelection,
}

impl SelectableStyledText {
    pub fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        styled: StyledText,
        order: u64,
        frame: SharedPreviewSelection,
    ) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            styled,
            order,
            frame,
        }
    }
}

impl IntoElement for SelectableStyledText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableStyledText {
    type RequestLayoutState = ();
    type PrepaintState = usize;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (layout_id, ()) = self.styled.request_layout(id, inspector_id, window, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled
            .prepaint(id, inspector_id, bounds, &mut (), window, cx);
        let layout = self.styled.layout().clone();
        let mut frame = self.frame.borrow_mut();
        let index = frame.runs.len();
        frame.runs.push(
            TextSelectionRun::new(self.text.clone(), layout, bounds)
                .with_document_order(self.order),
        );
        index
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _index: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // 高亮交给独立的 `SelectionOverlay` 绘制：正文片段只负责排版并把
        // 布局写入共享的 `frame.runs`，不在这里画高亮，这样正文层才能被
        // 整段缓存而跳过重绘。
        self.styled
            .paint(id, inspector_id, bounds, &mut (), &mut (), window, cx);
    }
}

fn paint_selection(layout: &TextLayout, range: Range<usize>, color: Hsla, window: &mut Window) {
    let (Some(start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return;
    };
    for bounds in selection_quad_bounds(start, end, layout.bounds(), layout.line_height()) {
        window.paint_quad(PaintQuad {
            bounds,
            background: color.into(),
            corner_radii: Corners::default(),
            border_widths: Edges::default(),
            border_color: transparent_black(),
            border_style: BorderStyle::default(),
        });
    }
}

fn selection_quad_bounds(
    start: Point<Pixels>,
    end: Point<Pixels>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
) -> Vec<Bounds<Pixels>> {
    if start.y == end.y {
        return vec![Bounds::from_corners(
            start,
            Point::new(end.x, end.y + line_height),
        )];
    }
    let mut quads = vec![Bounds::from_corners(
        start,
        Point::new(bounds.right(), start.y + line_height),
    )];
    if end.y > start.y + line_height {
        quads.push(Bounds::from_corners(
            Point::new(bounds.left(), start.y + line_height),
            Point::new(bounds.right(), end.y),
        ));
    }
    quads.push(Bounds::from_corners(
        Point::new(bounds.left(), end.y),
        Point::new(end.x, end.y + line_height),
    ));
    quads
}

#[cfg(test)]
mod tests {
    use super::{
        SelectableStyledText, SelectionOverlayElement, SharedPreviewSelection, new_frame_selection,
    };
    use gpui_kit::base::{StyledExt as _, TextSelection, TextSelectionHandle, TextSelectionLayer};
    use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
    use gpui_kit::{
        Background, Context, ElementId, Hsla, InteractiveElement, IntoElement, Modifiers,
        MouseButton, ParentElement as _, Render, SharedString, StatefulInteractiveElement,
        Styled as _, StyledText, TestAppContext, Window, div, point, px,
    };

    fn selection_color() -> Hsla {
        gpui_kit::rgb(0x3366cc).into()
    }

    fn has_selection_quad(window: &Window) -> bool {
        let background: Background = selection_color().into();
        window
            .painted_quads()
            .iter()
            .any(|quad| quad.background == background)
    }

    fn run(
        id: impl Into<ElementId>,
        text: impl Into<SharedString> + Clone,
        order: u64,
        frame: SharedPreviewSelection,
    ) -> SelectableStyledText {
        let text = text.into();
        SelectableStyledText::new(id, text.clone(), StyledText::new(text), order, frame)
    }

    struct TwoRunView;

    impl Render for TwoRunView {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let frame = new_frame_selection();
            let handle = TextSelectionHandle::new("", cx);
            div().size_full().child(TextSelectionLayer).child(
                div()
                    .id("preview-root")
                    .size_full()
                    .relative()
                    .context_menu(|menu, _, _| menu.item(PopupMenuItem::new("Copy")))
                    // 测试里用元素形式的覆盖层并手动重绘；生产里由独立的
                    // `SelectionOverlay` 实体订阅选区变化、只重绘自身。
                    .child(SelectionOverlayElement::new(
                        frame.clone(),
                        selection_color(),
                        handle,
                    ))
                    .child(div().w(px(300.)).h(px(20.)).child(run(
                        "first",
                        "alpha beta",
                        0,
                        frame.clone(),
                    )))
                    .child(div().w(px(300.)).h(px(20.)).child(run(
                        "second",
                        "gamma delta",
                        1,
                        frame,
                    ))),
            )
        }
    }

    /// 回归用例：整篇预览共用一个选区句柄时，前面的片段
    /// 仍然可以被选中并参与复制。
    #[gpui::test]
    fn an_earlier_run_is_selectable(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| TwoRunView);
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        cx.simulate_mouse_down(
            point(px(1.), px(8.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(120.), px(8.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(120.), px(8.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let selected = TextSelection::selected_text(window, cx);
            assert!(
                selected.contains("alpha"),
                "first run must be selectable, got {selected:?}"
            );
        });
    }

    /// 拖动过程中（尚未松手）就要看到高亮：窗口选区每次
    /// mouse_move 都会发布新快照，代理必须让窗口跟着重绘。
    #[gpui::test]
    fn drag_paints_selection_before_mouse_up(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| TwoRunView);
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(!has_selection_quad(window));
        });

        cx.simulate_mouse_down(
            point(px(1.), px(8.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(120.), px(8.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        // 快照变化后由生产 `SelectionOverlay` 订阅触发自动重绘；测试里用
        // 元素形式的覆盖层，这里手动重绘以验证高亮确实被画出来。
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(has_selection_quad(window), "拖动过程中应立即绘制选中高亮");
            assert!(
                TextSelection::selected_text(window, cx).contains("alpha"),
                "拖动过程中就应能取到选区文本"
            );
        });
    }

    /// 下一次按下要立刻清掉上一次的高亮，避免快速连续拖选时
    /// 旧选区粘在屏幕上。
    #[gpui::test]
    fn a_new_press_clears_the_previous_highlight(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| TwoRunView);
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_mouse_down(
            point(px(1.), px(8.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(120.), px(8.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(120.), px(8.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(has_selection_quad(window));
        });

        cx.simulate_mouse_down(
            point(px(1.), px(28.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(!has_selection_quad(window), "新的一次按下应立即清除旧高亮");
        });
    }

    /// 文字块之外的空白不应被当成可选文本，否则一次落在空白处
    /// 的按压也会把整个视口算进选区。
    #[gpui::test]
    fn blank_space_is_not_selectable_text(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| TwoRunView);
        cx.simulate_mouse_move(point(px(10.), px(200.)), None, Modifiers::default());
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(
                !TextSelection::is_selectable_at(point(px(10.), px(200.)), window, cx),
                "文字块之外的空白不应命中选区"
            );
        });
    }

    struct ScrollablePreviewView;

    impl Render for ScrollablePreviewView {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let frame = new_frame_selection();
            let handle = TextSelectionHandle::new("", cx);
            let mut column = div().v_flex().w(px(300.));
            for index in 0..12 {
                let text = SharedString::from(format!("line{index} alpha beta gamma"));
                column = column.child(div().h(px(20.)).child(run(
                    ElementId::Name(format!("row-{index}").into()),
                    text,
                    index as u64,
                    frame.clone(),
                )));
            }
            div().size_full().child(TextSelectionLayer).child(
                div()
                    .id("preview-scroll")
                    .h(px(60.))
                    .w(px(300.))
                    .overflow_y_scroll()
                    .child(SelectionOverlayElement::new(
                        frame,
                        selection_color(),
                        handle,
                    ))
                    .child(column),
            )
        }
    }

    /// 松手后滚动页面，选区必须仍落在原来的文字上，而不是随着
    /// 滚动重新投影到别的行、看起来自己在变大变小。
    #[gpui::test]
    fn scrolling_keeps_selection_on_the_same_text(cx: &mut TestAppContext) {
        use gpui::{ScrollDelta, ScrollWheelEvent};

        let (_, cx) = cx.add_window_view(|_, _| ScrollablePreviewView);
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        cx.simulate_mouse_down(
            point(px(1.), px(8.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(120.), px(8.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.simulate_mouse_up(
            point(px(120.), px(8.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let before = cx.update(|window, cx| TextSelection::selected_text(window, cx));
        assert!(
            before.starts_with("line0"),
            "expected the first line to be selected, got {before:?}"
        );

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(10.), px(30.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
            ..Default::default()
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let after = cx.update(|window, cx| TextSelection::selected_text(window, cx));
        assert_eq!(after, before, "滚动后选区不应换到别的文字上");
    }

    /// 回归用例：共享选区帧由内容实体跨帧持有，每次全量重建（缓存失效）
    /// 都会重新 prepaint 并追加 runs。`render()` 必须先清空再重建，否则
    /// runs 随重绘次数累积——覆盖层把同一选区按重复片段画多次，半透明
    /// 高亮越叠越深，投影也越来越慢。这里连续重绘两次，断言片段数恒定。
    #[gpui::test]
    fn rebuilding_content_does_not_accumulate_runs(cx: &mut TestAppContext) {
        use mdwork_render::preview::PreviewIr;
        use std::collections::HashMap;

        struct PersistentPreview {
            frame: SharedPreviewSelection,
        }
        impl Render for PersistentPreview {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let ir = PreviewIr::from_markdown("alpha beta gamma");
                crate::preview::render(
                    &ir,
                    None,
                    &HashMap::new(),
                    &HashMap::new(),
                    false,
                    "",
                    16.0,
                    self.frame.clone(),
                    None,
                    cx,
                )
            }
        }

        let frame = new_frame_selection();
        let probe = frame.clone();
        cx.update(|cx| gpui_kit::init(cx));
        let (_, cx) = cx.add_window_view(|_, _| PersistentPreview { frame });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let first = probe.borrow().len();
        assert!(first > 0, "正文应至少收集一个选区片段");
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(probe.borrow().len(), first, "重绘不得累积选区片段");
    }

    /// 回归用例：预览标题位置帧与选区帧同样由内容实体跨帧持有，缓存失效
    /// 重建会重新 prepaint 并追加标题位置。`render()` 必须先清空再重建，
    /// 否则标题位置随重绘累积，大纲联动会读到重复/错位的坐标。这里连续
    /// 重绘两次，断言两个标题各记录一次且数量恒定。
    #[gpui::test]
    fn preview_headings_recorded_without_accumulation(cx: &mut TestAppContext) {
        use crate::preview::{PreviewOutlineTracker, new_frame_headings};
        use gpui_kit::ScrollHandle;
        use mdwork_render::preview::PreviewIr;
        use std::collections::HashMap;

        struct HeadingPreview {
            frame: SharedPreviewSelection,
            tracker: PreviewOutlineTracker,
        }
        impl Render for HeadingPreview {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let ir = PreviewIr::from_markdown("# One\n\ntext\n\n## Two\n\nmore\n");
                crate::preview::render(
                    &ir,
                    None,
                    &HashMap::new(),
                    &HashMap::new(),
                    false,
                    "",
                    16.0,
                    self.frame.clone(),
                    Some(self.tracker.clone()),
                    cx,
                )
            }
        }

        let frame = new_frame_selection();
        let headings = new_frame_headings();
        let tracker = PreviewOutlineTracker {
            headings: headings.clone(),
            scroll_handle: ScrollHandle::new(),
        };
        let probe = headings.clone();
        cx.update(|cx| gpui_kit::init(cx));
        let (_, cx) = cx.add_window_view(|_, _| HeadingPreview { frame, tracker });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let first = probe.borrow().entries().len();
        assert_eq!(first, 2, "两个标题应各记录一次位置");
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        assert_eq!(
            probe.borrow().entries().len(),
            first,
            "重绘不得累积标题位置"
        );
    }
}
