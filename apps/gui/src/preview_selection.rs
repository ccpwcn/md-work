//! 参与 GPUI 窗口级文本选区的预览富文本片段。
//!
//! 整篇预览只注册【一个】选区参与者：每个片段在 `prepaint` 时
//! 把自己的布局写入本帧共享的 `PreviewFrameSelection`，由
//! [`SelectionProxy`] 统一注册几何并调用 `update_runs` 投影选区，
//! 片段在 `paint` 时按自己的下标绘制高亮。
//!
//! 早先每个片段各持有一个 `TextSelectionHandle`，窗口有活动选区时
//! 每次注册都会触发 `publish_snapshots` 遍历全部参与者，一次重绘
//! 退化为 O(n²)，大文档预览会长期占用 CPU。

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::{TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};
use gpui_kit::*;

/// 一帧内所有预览片段共享的选区数据。每帧重建，句柄由代理元素
/// 通过 GPUI 元素状态跨帧保留。
#[derive(Default)]
pub struct PreviewFrameSelection {
    runs: Vec<TextSelectionRun>,
    ranges: Vec<Option<Range<usize>>>,
}

pub type SharedPreviewSelection = Rc<RefCell<PreviewFrameSelection>>;

pub fn new_frame_selection() -> SharedPreviewSelection {
    Rc::new(RefCell::new(PreviewFrameSelection::default()))
}

/// 整篇预览唯一的选区参与者。必须作为预览内容的第一个子元素挂载，
/// 这样 `prepaint` 时先清空上一帧数据、`paint` 时先完成投影。
pub struct SelectionProxy {
    frame: SharedPreviewSelection,
}

impl SelectionProxy {
    pub fn new(frame: SharedPreviewSelection) -> Self {
        Self { frame }
    }
}

impl IntoElement for SelectionProxy {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectionProxy {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some("preview-selection-proxy".into())
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
        let handle = window.with_element_state(
            id.expect("SelectionProxy::id() 必须返回稳定的元素 id"),
            |retained: Option<TextSelectionHandle>, _| {
                let handle = retained.unwrap_or_else(|| TextSelectionHandle::new("", cx));
                (handle.clone(), handle)
            },
        );
        (window.request_layout(Style::default(), [], cx), handle)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // 代理本身不占布局空间，用当前滚动视口作为选区命中区域。
        let viewport = window.content_mask().bounds;
        let hitbox = window.insert_hitbox(viewport, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), viewport)
            .with_text_bounds(vec![viewport])
            .with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        _hitbox: &mut Self::PrepaintState,
        _window: &mut Window,
        cx: &mut App,
    ) {
        let mut frame = self.frame.borrow_mut();
        let runs = std::mem::take(&mut frame.runs);
        let projection = handle.update_runs(&runs, cx);
        frame.ranges = projection.ranges().to_vec();
    }
}

pub struct SelectableStyledText {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
    order: u64,
    selection_color: Hsla,
    frame: SharedPreviewSelection,
}

impl SelectableStyledText {
    pub fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        styled: StyledText,
        order: u64,
        selection_color: Hsla,
        frame: SharedPreviewSelection,
    ) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            styled,
            order,
            selection_color,
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
        index: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        let range = self
            .frame
            .borrow()
            .ranges
            .get(*index)
            .and_then(|range| range.clone());
        if let Some(range) = range {
            paint_selection(&layout, range, self.selection_color, window);
        }
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
        SelectableStyledText, SelectionProxy, SharedPreviewSelection, new_frame_selection,
    };
    use gpui_kit::base::{TextSelection, TextSelectionLayer};
    use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
    use gpui_kit::{
        Context, InteractiveElement, IntoElement, Modifiers, MouseButton, ParentElement as _,
        Render, Styled as _, StyledText, TestAppContext, Window, div, point, px,
    };

    fn run(
        id: &'static str,
        text: &'static str,
        order: u64,
        frame: SharedPreviewSelection,
    ) -> SelectableStyledText {
        SelectableStyledText::new(
            id,
            text,
            StyledText::new(text),
            order,
            gpui_kit::rgb(0x3366cc).into(),
            frame,
        )
    }

    struct TwoRunView;

    impl Render for TwoRunView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let frame = new_frame_selection();
            div().size_full().child(TextSelectionLayer).child(
                div()
                    .id("preview-root")
                    .size_full()
                    .relative()
                    .context_menu(|menu, _, _| menu.item(PopupMenuItem::new("Copy")))
                    .child(SelectionProxy::new(frame.clone()))
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
}
