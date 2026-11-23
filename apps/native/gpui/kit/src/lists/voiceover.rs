//! Accessibility nodes for the painted rows, shaped like AppKit's `NSTableView`: the list or
//! column is a table in a scroll area holding one column, and each row holds one cell named
//! for its file, selected as the core has it. Nodes are built only while assistive tech reads
//! the window (`Window::is_a11y_active`).
//!
//! Roles are AccessKit's grid and tree item, which macOS reads as `AXTable` and `AXRow` and
//! which, unlike table rows, report selection (`AXSelected`, `AXSelectedRows`). AccessKit has
//! no role macOS reads as a scroll area or column, so on macOS such elements carry the AppKit
//! role as a description (`platform::accessibility`).

use gpui::{
    A11ySubtreeBuilder, Bounds, InteractiveElement, IntoElement, ParentElement, Pixels, Point,
    Role, StatefulInteractiveElement, Styled, div,
};

/// A row's accessibility data: file name, selection and window position.
pub struct HeardRow {
    pub name: String,
    pub selected: bool,
    pub frame: Bounds<Pixels>,
}

/// The table role for the list or column that holds the rows.
pub const TABLE: Role = Role::Grid;

/// The role of the table's container, as AppKit tables sit in a scroll view.
pub const SCROLL_AREA: Role = Role::ScrollView;

/// Marks a `SCROLL_AREA` element's node as `AXScrollArea` on macOS.
pub fn scroll_area() -> impl FnOnce(&mut A11ySubtreeBuilder) + 'static {
    |builder| stands_for(builder, "AXScrollArea")
}

/// `scroll_area` for an element larger than the area: sets its bounds to `at` (window points)
/// at `scale` pixels per point.
pub fn scroll_area_at(
    at: Bounds<Pixels>,
    scale: f32,
) -> impl FnOnce(&mut A11ySubtreeBuilder) + 'static {
    move |builder| {
        stands_for(builder, "AXScrollArea");
        let pixel = |value: Pixels| f64::from(f32::from(value) * scale);
        builder.parent_node().set_bounds(gpui::accesskit::Rect {
            x0: pixel(at.origin.x),
            y0: pixel(at.origin.y),
            x1: pixel(at.origin.x + at.size.width),
            y1: pixel(at.origin.y + at.size.height),
        });
    }
}

/// On macOS, describes `builder`'s element as AppKit `role`, which AccessKit has no role for.
fn stands_for(builder: &mut A11ySubtreeBuilder, role: &str) {
    if cfg!(target_os = "macos") {
        builder.parent_node().set_role_description(role);
    }
}

/// The table's single column, spanning the table, with `NSTableView`'s `identifier` (`name` in
/// columns, `row` in the list). macOS only; other platforms' tables have no column nodes.
pub fn column(identifier: &'static str) -> Option<impl IntoElement> {
    cfg!(target_os = "macos").then(|| {
        div()
            .id("voiceover-column")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .role(Role::Unknown)
            .accessibility_id(identifier)
            .a11y_synthetic_children(|builder| stands_for(builder, "AXColumn"))
    })
}

/// The rows' nodes, placed in a table whose top-left is `origin` in the window.
pub fn rows(heard: Vec<HeardRow>, origin: Point<Pixels>) -> impl Iterator<Item = impl IntoElement> {
    heard.into_iter().enumerate().map(move |(index, row)| {
        let at = row.frame.origin - origin;
        div()
            .id(("voiceover-row", index))
            .absolute()
            .left(at.x)
            .top(at.y)
            .w(row.frame.size.width)
            .h(row.frame.size.height)
            .role(Role::TreeItem)
            .aria_selected(row.selected)
            .aria_row_index(index)
            .child(
                div()
                    .id("cell")
                    .size_full()
                    .role(Role::Cell)
                    .aria_label(row.name),
            )
    })
}
