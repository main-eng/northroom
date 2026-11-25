//! Window layout: which view draws the open file and whether it takes the keyboard, plus the
//! frames of the title bar, the columns, the open file and the folder's other views. The shell
//! creates the views and places them as this says.

pub mod ffi;
pub mod screen_layout;
pub mod tiles;
pub mod title_bar;
pub mod tooltip;

use crate::board::Frame;
use crate::workspace::EntryKind;

/// The title bar's height.
pub const TITLE_BAR_HEIGHT: f64 = 42.0;
/// A column's width, its hairline edge included.
pub const COLUMN_WIDTH: f64 = 240.0;
/// The window's initial and minimum size.
pub const WINDOW_SIZE: (f64, f64) = (1280.0, 800.0);
pub const WINDOW_MIN_SIZE: (f64, f64) = (1024.0, 700.0);
/// Inset of a full-tab terminal from the content area.
const INSET: f64 = 4.0;

/// Tab width in spaces for source files, matching a browser's `pre` (`tab-size: 8`).
pub const SOURCE_TAB_SPACES: u32 = 8;

/// The view that draws the open file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenedView {
    /// A terminal's session.
    Terminal,
    /// An HTML file run as a live page.
    Html,
    /// A page, or a plain-text file read as one.
    Page,
    /// Delimited text or a workbook, as its table.
    Table,
    /// A presentation, as its slides.
    Deck,
    /// A 3D model's still.
    Model,
    /// Source code and any other text, as it is.
    Source,
    /// Any other file: its picture and details.
    Details,
}

impl OpenedView {
    /// The views tried, in order, for a file that is not a terminal.
    const TRIED: [Self; 6] = [
        Self::Html,
        Self::Page,
        Self::Table,
        Self::Deck,
        Self::Model,
        Self::Source,
    ];

    /// The view for an open file of `kind`: the terminal view for a terminal, else the first of
    /// `TRIED` that `shows` it, else details.
    pub fn of(kind: EntryKind, shows: impl Fn(Self) -> bool) -> Self {
        if kind == EntryKind::Terminal {
            return Self::Terminal;
        }
        Self::TRIED
            .into_iter()
            .find(|&view| shows(view))
            .unwrap_or(Self::Details)
    }

    /// Whether the view takes the keyboard when created: a terminal always, a live page or page
    /// when the file fills the tab. Beside its column a file leaves the keyboard with the column,
    /// whose row stays accented.
    pub fn takes_keys(self, fills_tab: bool) -> bool {
        match self {
            Self::Terminal => true,
            Self::Html | Self::Page => fills_tab,
            Self::Table | Self::Deck | Self::Model | Self::Source | Self::Details => false,
        }
    }
}

/// The window's current content, as the shell built it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holds {
    /// Columns drawn.
    pub columns: usize,
    /// The open file's view, if any.
    pub opened: Option<OpenedView>,
    pub settings: bool,
    /// The active tab is a Changes tab, which holds it in place of the folder.
    pub changes: bool,
}

impl Holds {
    /// Whether settings or a Changes tab show in the folder's place.
    fn covers_folder(self) -> bool {
        self.settings || self.changes
    }
}

/// Frames in window points from the top-left, except the columns and a file beside them, which
/// are in the columns' content coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub title_bar: Frame,
    /// Under the title bar: the list, the board, settings, onboarding.
    pub content: Frame,
    /// The horizontal scroller for the columns and a file beside them, and its content.
    pub columns_scroll: Frame,
    pub columns_content: Frame,
    /// The column at index `n` sits `n × column_width` in.
    pub column_width: f64,
    /// The open file's frame, in the columns' content when `beside`.
    pub opened: Frame,
    pub beside: bool,
    /// The list and board are hidden because a file, settings or a Changes tab replace them.
    pub folder_hidden: bool,
    pub columns_hidden: bool,
    pub opened_hidden: bool,
    /// The details pane fills the tab, as when opened from the board or list.
    pub details_fills: bool,
    /// The span to scroll into view after opening a column or file: the file beside the
    /// columns, else the last column. `None` with no columns.
    pub reveal: Option<(f64, f64)>,
}

/// Lays out a `width` × `height` window holding `holds`. A file beside the columns takes all
/// of the window but one column's width, whatever its kind and however many columns are open.
/// Its own column stays visible beside it for navigation; earlier columns scroll off to the
/// left and scroll back.
pub fn layout(width: f64, height: f64, holds: Holds) -> Layout {
    let covered = holds.covers_folder();
    let top = TITLE_BAR_HEIGHT;
    let below = height - top;
    let content = Frame {
        x: 0.0,
        y: top,
        width,
        height: below,
    };
    let across = holds.columns as f64 * COLUMN_WIDTH;
    // The core decides whether the file is beside its column or fills the tab
    // (`Columns::drawn_columns`). While columns are drawn the file is only selected, so any
    // kind of file, terminals included, sits beside them.
    let beside = holds.columns > 0 && holds.opened.is_some();
    let pane = if beside {
        (width - COLUMN_WIDTH).max(0.0)
    } else {
        0.0
    };
    let opened = match holds.opened {
        _ if beside => Frame {
            x: across,
            y: 0.0,
            width: pane,
            height: below,
        },
        Some(OpenedView::Terminal) => Frame {
            x: INSET,
            y: top + INSET,
            width: (width - 2.0 * INSET).max(0.0),
            height: (below - 2.0 * INSET).max(0.0),
        },
        _ => content,
    };
    let last_column = holds
        .columns
        .checked_sub(1)
        .map(|last| last as f64 * COLUMN_WIDTH);
    Layout {
        title_bar: Frame {
            x: 0.0,
            y: 0.0,
            width,
            height: top,
        },
        content,
        columns_scroll: Frame {
            x: 0.0,
            y: top,
            width: if holds.columns > 0 { width } else { 0.0 },
            height: below,
        },
        columns_content: Frame {
            x: 0.0,
            y: 0.0,
            width: across + pane,
            height: below,
        },
        column_width: COLUMN_WIDTH,
        opened,
        beside,
        folder_hidden: holds.opened.is_some() || covered,
        columns_hidden: covered || holds.columns == 0,
        opened_hidden: covered,
        details_fills: holds.columns == 0,
        reveal: if beside {
            Some((across, across + pane))
        } else {
            last_column.map(|x| (x, x + COLUMN_WIDTH))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_is_its_own_and_other_files_take_the_first_view_that_shows_them() {
        assert_eq!(
            OpenedView::of(EntryKind::Terminal, |_| true),
            OpenedView::Terminal
        );
        // The order: live page, page, table, deck, model, source; else details.
        let only = |shown: OpenedView| move |view: OpenedView| view == shown;
        for view in OpenedView::TRIED {
            assert_eq!(OpenedView::of(EntryKind::File, only(view)), view);
        }
        assert_eq!(
            OpenedView::of(EntryKind::File, |view| view == OpenedView::Page
                || view == OpenedView::Source),
            OpenedView::Page
        );
        assert_eq!(
            OpenedView::of(EntryKind::Image, |_| false),
            OpenedView::Details
        );
    }

    #[test]
    fn a_file_takes_the_keys_when_it_fills_the_tab_and_a_terminal_always() {
        assert!(OpenedView::Terminal.takes_keys(false));
        for view in [OpenedView::Html, OpenedView::Page] {
            assert!(view.takes_keys(true) && !view.takes_keys(false));
        }
        for view in [
            OpenedView::Table,
            OpenedView::Deck,
            OpenedView::Model,
            OpenedView::Source,
            OpenedView::Details,
        ] {
            assert!(!view.takes_keys(true));
        }
    }

    fn holds(columns: usize, opened: Option<OpenedView>) -> Holds {
        Holds {
            columns,
            opened,
            settings: false,
            changes: false,
        }
    }

    #[test]
    fn a_file_beside_the_columns_takes_all_but_one_column() {
        // One column in 1280: the file takes the 1040 beside it and nothing scrolls.
        let one = layout(1280.0, 800.0, holds(1, Some(OpenedView::Details)));
        assert!(one.beside);
        assert_eq!(
            one.opened,
            Frame {
                x: 240.0,
                y: 0.0,
                width: 1040.0,
                height: 758.0
            }
        );
        assert_eq!(one.columns_content.width, 1280.0);
        assert_eq!(one.reveal, Some((240.0, 1280.0)));
        // Two columns would leave 800, but the file still takes 1040: the first column scrolls
        // off and the file's own column alone stays beside it.
        let two = layout(1280.0, 800.0, holds(2, Some(OpenedView::Page)));
        assert_eq!(
            (two.opened.x, two.opened.width, two.columns_content.width),
            (480.0, 1040.0, 1520.0)
        );
        assert_eq!(two.reveal, Some((480.0, 1520.0)));
        // The same in a wide window, where the columns would leave more than half.
        let wide = layout(2000.0, 800.0, holds(2, Some(OpenedView::Page)));
        assert_eq!((wide.opened.x, wide.opened.width), (480.0, 1760.0));
        // And for any kind of file with more columns open.
        let three = layout(1280.0, 800.0, holds(3, Some(OpenedView::Terminal)));
        assert_eq!(
            (
                three.opened.x,
                three.opened.width,
                three.columns_content.width
            ),
            (720.0, 1040.0, 1760.0)
        );
        assert_eq!(three.reveal, Some((720.0, 1760.0)));
        // As do four, and the smallest window still gives it 784.
        let four = layout(1024.0, 700.0, holds(4, Some(OpenedView::Page)));
        assert_eq!(
            (four.opened.x, four.opened.width, four.columns_content.width),
            (960.0, 784.0, 1744.0)
        );
        assert!(four.folder_hidden && !four.details_fills && !four.columns_hidden);
    }

    #[test]
    fn every_kind_of_file_gets_the_same_frame_beside_the_columns() {
        let kinds = [
            OpenedView::Terminal,
            OpenedView::Html,
            OpenedView::Page,
            OpenedView::Table,
            OpenedView::Deck,
            OpenedView::Model,
            OpenedView::Source,
            OpenedView::Details,
        ];
        for columns in 1..=5 {
            for kind in kinds {
                let laid = layout(1600.0, 900.0, holds(columns, Some(kind)));
                assert!(laid.beside, "{kind:?} beside {columns}");
                assert_eq!(
                    laid.opened,
                    Frame {
                        x: columns as f64 * COLUMN_WIDTH,
                        y: 0.0,
                        width: 1600.0 - COLUMN_WIDTH,
                        height: 900.0 - TITLE_BAR_HEIGHT
                    },
                    "{kind:?} beside {columns}"
                );
            }
        }
    }

    #[test]
    fn columns_alone_fill_the_scroller_and_show_the_last() {
        let three = layout(1280.0, 800.0, holds(3, None));
        assert_eq!(
            three.columns_scroll,
            Frame {
                x: 0.0,
                y: 42.0,
                width: 1280.0,
                height: 758.0
            }
        );
        assert_eq!(three.columns_content.width, 720.0);
        assert_eq!(three.reveal, Some((480.0, 720.0)));
        assert!(!three.columns_hidden && !three.folder_hidden);
    }

    #[test]
    fn a_file_over_the_folder_fills_the_tab_and_a_terminal_sits_inside() {
        let page = layout(1280.0, 800.0, holds(0, Some(OpenedView::Page)));
        assert!(!page.beside && page.folder_hidden && page.columns_hidden && page.details_fills);
        assert_eq!(
            page.opened,
            Frame {
                x: 0.0,
                y: 42.0,
                width: 1280.0,
                height: 758.0
            }
        );
        assert_eq!(page.columns_scroll.width, 0.0);
        assert_eq!(page.reveal, None);
        assert_eq!(
            layout(1280.0, 800.0, holds(0, Some(OpenedView::Terminal))).opened,
            Frame {
                x: 4.0,
                y: 46.0,
                width: 1272.0,
                height: 750.0
            }
        );
    }

    #[test]
    fn a_changes_tab_shows_in_the_folder_s_place() {
        let shown = layout(
            1280.0,
            800.0,
            Holds {
                changes: true,
                ..holds(2, Some(OpenedView::Details))
            },
        );
        assert!(shown.folder_hidden && shown.columns_hidden && shown.opened_hidden);
    }

    #[test]
    fn settings_cover_the_folder() {
        let shown = layout(
            1280.0,
            800.0,
            Holds {
                settings: true,
                ..holds(2, Some(OpenedView::Details))
            },
        );
        assert!(shown.folder_hidden && shown.columns_hidden && shown.opened_hidden);
        assert_eq!(
            shown.content,
            Frame {
                x: 0.0,
                y: 42.0,
                width: 1280.0,
                height: 758.0
            }
        );
    }
}
