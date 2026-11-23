//! The list header: column positions and press handling. Dragging the edge before a column
//! resizes it (a double click restores its width). Dragging a column moves it, clicking sorts by
//! it, and the button at the end of Name opens the list options. `Columns` decides which columns
//! exist and their widths; a shell draws the header and forwards presses here.

use crate::board::Frame;

/// Inset from each end of a row to its first and last column.
pub const INSET: f64 = 8.0;
/// Gap between columns.
pub const GAP: f64 = 12.0;
/// Minimum width of Name, whatever the others take.
pub const NAME_MIN: f64 = 60.0;
/// Hit distance either side of a column edge for resizing.
pub const EDGE_REACH: f64 = 5.0;
/// Drag distance before a column press becomes a move instead of a click.
pub const MOVE_AFTER: f64 = 8.0;
/// Hit slop either side of a column for a right click.
pub const MENU_REACH: f64 = 6.0;

/// The header sits 4 pt below the title bar and is 28 pt tall; rows start below it.
const HEADER_TOP: f64 = 4.0;
const HEADER_HEIGHT: f64 = 28.0;

/// Frames of the header and the rows below it in a `width` × `height` list view.
pub fn place(width: f64, height: f64) -> (Frame, Frame) {
    let rows_top = HEADER_TOP + HEADER_HEIGHT;
    (
        Frame {
            x: 0.0,
            y: HEADER_TOP,
            width,
            height: HEADER_HEIGHT,
        },
        Frame {
            x: 0.0,
            y: rows_top,
            width,
            height: (height - rows_top).max(0.0),
        },
    )
}

/// A column after Name and its position across the row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub key: u32,
    pub x: f64,
    pub width: f64,
}

impl Placed {
    /// The middle of the gap before it, which is the resize edge.
    pub fn edge(self) -> f64 {
        self.x - GAP / 2.0
    }

    fn mid(self) -> f64 {
        self.x + self.width / 2.0
    }

    fn contains(self, x: f64) -> bool {
        x >= self.x && x < self.x + self.width
    }
}

/// Columns across a row `width` wide. Columns after Name are packed from the right, `GAP` apart
/// and `INSET` from the ends; Name takes the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub name_x: f64,
    pub name_width: f64,
    pub others: Vec<Placed>,
}

/// Lays out the columns after Name (`(key, width)`, in the order shown) across `width`.
pub fn lay_out(width: f64, columns: &[(u32, f64)]) -> Layout {
    let mut right = width - INSET;
    let mut others = Vec::with_capacity(columns.len());
    for (key, column_width) in columns.iter().rev() {
        let x = right - column_width;
        others.insert(
            0,
            Placed {
                key: *key,
                x,
                width: *column_width,
            },
        );
        right = x - GAP;
    }
    Layout {
        name_x: INSET,
        name_width: (right - INSET).max(NAME_MIN),
        others,
    }
}

impl Layout {
    /// The button at the end of Name that opens the list options.
    pub fn chooser(&self) -> Frame {
        Frame {
            x: self.name_x + self.name_width - 20.0,
            y: 3.5,
            width: 20.0,
            height: 20.0,
        }
    }

    /// The column whose edge is under `x`.
    fn edge_at(&self, x: f64) -> Option<Placed> {
        self.others
            .iter()
            .copied()
            .find(|column| (x - column.edge()).abs() <= EDGE_REACH)
    }

    /// The column a right click at `x` targets, for its Move items.
    pub fn menu_column(&self, x: f64) -> Option<u32> {
        self.others
            .iter()
            .find(|column| x >= column.x - MENU_REACH && x < column.x + column.width + MENU_REACH)
            .map(|column| column.key)
    }

    /// The position for `key` dropped at `x`: after every other column whose middle `x` has
    /// passed (1 is the first after Name).
    fn place_for(&self, key: u32, x: f64) -> usize {
        1 + self
            .others
            .iter()
            .filter(|column| column.key != key && column.mid() < x)
            .count()
    }

    /// Position of `key` among the columns after Name.
    pub fn index_of(&self, key: u32) -> Option<usize> {
        self.others.iter().position(|column| column.key == key)
    }
}

/// The immediate result of a press on the header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pressed {
    Nothing,
    /// A double click on an edge restores the column's default width.
    ResetWidth(u32),
    /// Open the list options under the chooser.
    Options,
    /// A sort by this column (Name is 0).
    Sort(u32),
    /// Track the press with `Press::drag` and `Press::release` until it ends.
    Follow(Press),
}

/// A press being tracked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Press {
    /// On an edge. Dragging left widens the column, within its limits.
    Resize {
        key: u32,
        start_x: f64,
        start_width: f64,
    },
    /// On a column. Past `MOVE_AFTER` it becomes a move to where it is released.
    Column {
        key: u32,
        start_x: f64,
        travelled: Option<f64>,
    },
}

/// The result of releasing a tracked press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Released {
    /// The column keeps the width the drag left.
    Resized,
    Sort(u32),
    /// The column moves to this position (1 is the first after Name).
    Move(u32, usize),
}

/// A press at `x`, `y`, where `clicks` is the click count.
pub fn press(layout: &Layout, x: f64, y: f64, clicks: u32) -> Pressed {
    if let Some(column) = layout.edge_at(x) {
        if clicks == 2 {
            return Pressed::ResetWidth(column.key);
        }
        return Pressed::Follow(Press::Resize {
            key: column.key,
            start_x: x,
            start_width: column.width,
        });
    }
    let chooser = layout.chooser();
    if x >= chooser.x
        && x < chooser.x + chooser.width
        && y >= chooser.y
        && y < chooser.y + chooser.height
    {
        return Pressed::Options;
    }
    if x >= layout.name_x && x < layout.name_x + layout.name_width {
        return Pressed::Sort(0);
    }
    match layout.others.iter().find(|column| column.contains(x)) {
        Some(column) => Pressed::Follow(Press::Column {
            key: column.key,
            start_x: x,
            travelled: None,
        }),
        None => Pressed::Nothing,
    }
}

impl Press {
    /// The pointer moved to `x` (also called at release). A resize returns the column's new
    /// width, at least 1 pt; a move records how far it travelled.
    pub fn drag(&mut self, x: f64) -> Option<(u32, f64)> {
        match self {
            Press::Resize {
                key,
                start_x,
                start_width,
            } => Some((*key, (*start_width - (x - *start_x)).max(1.0))),
            Press::Column {
                start_x, travelled, ..
            } => {
                if (x - *start_x).abs() > MOVE_AFTER {
                    *travelled = Some(x);
                }
                None
            }
        }
    }

    /// Ends the press after the last `drag`. A column that never travelled sorts; one that did
    /// moves past the middles of the columns it crossed, as laid out when pressed.
    pub fn release(self, layout: &Layout) -> Released {
        match self {
            Press::Resize { .. } => Released::Resized,
            Press::Column {
                key,
                travelled: None,
                ..
            } => Released::Sort(key),
            Press::Column {
                key,
                travelled: Some(x),
                ..
            } => Released::Move(key, layout.place_for(key, x)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_is_28_tall_4_under_the_bar_with_the_rows_under_it() {
        let (header, rows) = place(900.0, 600.0);
        assert_eq!((header.y, header.width, header.height), (4.0, 900.0, 28.0));
        assert_eq!((rows.y, rows.height), (32.0, 568.0));
        assert_eq!(place(900.0, 20.0).1.height, 0.0);
    }

    /// Size 80 and Kind 120 after Name, across 800.
    fn layout() -> Layout {
        lay_out(800.0, &[(3, 80.0), (5, 120.0)])
    }

    #[test]
    fn the_columns_sit_from_the_right_and_name_takes_the_rest() {
        let layout = layout();
        assert_eq!(
            layout.others,
            [
                Placed {
                    key: 3,
                    x: 580.0,
                    width: 80.0
                },
                Placed {
                    key: 5,
                    x: 672.0,
                    width: 120.0
                }
            ]
        );
        assert_eq!((layout.name_x, layout.name_width), (8.0, 560.0));
        assert_eq!(
            layout.chooser(),
            Frame {
                x: 548.0,
                y: 3.5,
                width: 20.0,
                height: 20.0
            }
        );
        assert_eq!(layout.others[0].edge(), 574.0);
        let narrow = lay_out(200.0, &[(3, 150.0)]);
        assert_eq!(narrow.name_width, 60.0, "Name keeps 60 however narrow");
        assert_eq!(lay_out(300.0, &[]).name_width, 284.0);
    }

    #[test]
    fn a_press_resizes_sorts_moves_or_opens_the_options() {
        let layout = layout();
        assert_eq!(
            press(&layout, 570.0, 10.0, 2),
            Pressed::ResetWidth(3),
            "a double click on the edge"
        );
        assert_eq!(
            press(&layout, 579.0, 10.0, 1),
            Pressed::Follow(Press::Resize {
                key: 3,
                start_x: 579.0,
                start_width: 80.0
            })
        );
        assert_eq!(
            press(&layout, 666.0, 10.0, 1),
            Pressed::Follow(Press::Resize {
                key: 5,
                start_x: 666.0,
                start_width: 120.0
            })
        );
        assert_eq!(press(&layout, 558.0, 10.0, 1), Pressed::Options);
        assert_eq!(
            press(&layout, 558.0, 1.0, 1),
            Pressed::Sort(0),
            "above the button is Name"
        );
        assert_eq!(press(&layout, 100.0, 10.0, 1), Pressed::Sort(0));
        assert_eq!(
            press(&layout, 700.0, 10.0, 1),
            Pressed::Follow(Press::Column {
                key: 5,
                start_x: 700.0,
                travelled: None
            })
        );
        assert_eq!(
            press(&layout, 796.0, 10.0, 1),
            Pressed::Nothing,
            "the inset past the last column"
        );
    }

    #[test]
    fn dragging_an_edge_left_widens_its_column() {
        let mut resize = Press::Resize {
            key: 3,
            start_x: 574.0,
            start_width: 80.0,
        };
        assert_eq!(resize.drag(554.0), Some((3, 100.0)));
        assert_eq!(resize.drag(700.0), Some((3, 1.0)), "at least a point");
        assert_eq!(resize.release(&layout()), Released::Resized);
    }

    #[test]
    fn a_column_that_travels_moves_and_one_that_does_not_sorts() {
        let layout = layout();
        let mut still = Press::Column {
            key: 5,
            start_x: 700.0,
            travelled: None,
        };
        assert_eq!(still.drag(707.0), None);
        assert_eq!(still.release(&layout), Released::Sort(5));
        let mut moved = Press::Column {
            key: 5,
            start_x: 700.0,
            travelled: None,
        };
        moved.drag(600.0);
        assert_eq!(
            moved.release(&layout),
            Released::Move(5, 1),
            "before Size's middle"
        );
        let mut back = Press::Column {
            key: 3,
            start_x: 600.0,
            travelled: None,
        };
        back.drag(740.0);
        // Returning near the start keeps the last travelled position.
        back.drag(605.0);
        assert_eq!(
            back.release(&layout),
            Released::Move(3, 2),
            "past Kind's middle"
        );
    }

    #[test]
    fn a_right_click_names_the_column_near_it() {
        let layout = layout();
        assert_eq!(layout.menu_column(575.0), Some(3));
        assert_eq!(
            layout.menu_column(668.0),
            Some(5),
            "the gap's right half is the next column's"
        );
        assert_eq!(layout.menu_column(300.0), None);
        assert_eq!(layout.index_of(5), Some(1));
    }
}
