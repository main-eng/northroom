//! What the emulator shows, in Northroom's own terms: the grid's size, each visible cell's
//! character, colours and style, and the cursor. The emulator (`engine`) is the only module that
//! knows the library behind it; everything else, the frame the shell draws included, reads these
//! types, so the emulator can be replaced inside `engine` alone.

/// The grid in whole cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridSize {
    pub columns: usize,
    pub lines: usize,
}

/// An sRGB colour, one byte a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

/// A cell's colour as the program set it, before the theme's palette resolves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colour {
    /// The terminal's text colour.
    Foreground,
    /// The terminal's own background.
    Background,
    /// One of the 256 xterm colours; 0 to 15 are the named ones.
    Indexed(u8),
    Rgb(Rgb),
}

/// How a cell is underlined.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Underline {
    #[default]
    None,
    /// Single, curly, dotted and dashed underlines are drawn as one line.
    Single,
    Double,
}

/// The attributes a program set on a cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    /// Text and background swapped.
    pub inverse: bool,
    /// Drawn as background only.
    pub hidden: bool,
    pub strikeout: bool,
    pub underline: Underline,
}

/// One visible cell. The second half of a wide character is not a cell of its own: the wide
/// character's cell covers both.
#[derive(Clone, Copy, Debug)]
pub struct Cell<'a> {
    /// The visible line, from the top of the view.
    pub line: usize,
    pub column: usize,
    pub character: char,
    /// Combining marks after `character`.
    pub marks: &'a [char],
    /// Two cells wide.
    pub wide: bool,
    pub foreground: Colour,
    pub background: Colour,
    /// An underline in its own colour; `None` underlines in the text colour.
    pub underline_colour: Option<Colour>,
    pub style: Style,
    /// Inside the selection; selecting either half of a wide character selects it.
    pub selected: bool,
}

/// How the program asked the cursor to look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    HollowBlock,
    Beam,
    Underline,
    Hidden,
}

/// The cursor on the visible grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// The visible line it is on: negative or past the last line when the view is scrolled away
    /// from it.
    pub line: i32,
    pub column: usize,
    pub shape: CursorShape,
    pub blinking: bool,
    /// On a wide character, so it covers two cells.
    pub wide: bool,
}

/// Which half of a cell a point is in, for where a selection starts and ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// How a program wants the mouse reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseTracking {
    /// Presses and releases only.
    Clicks,
    /// Presses, releases and moves with a button held (or any move).
    Drags,
}

/// The modes a program set that change what keys, the mouse and pastes send.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    /// Application cursor keys (`DECCKM`).
    pub app_cursor: bool,
    /// The mouse reported to the program, when it asked.
    pub mouse: Option<MouseTracking>,
    /// Mouse reports in the SGR encoding.
    pub sgr_mouse: bool,
    /// The alternate screen of a full-screen program.
    pub alt_screen: bool,
    /// The wheel sends arrow keys on the alternate screen.
    pub alternate_scroll: bool,
    pub bracketed_paste: bool,
    /// The program asked to hear when the terminal gains and loses focus.
    pub focus_reports: bool,
}

/// How the view moves through the scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    /// Lines towards older output when positive.
    Lines(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

/// How a press selects: cells, a word (double click) or whole lines (triple click).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Cells,
    Word,
    Lines,
}

impl SelectionKind {
    /// The kind a press with `clicks` clicks starts.
    pub fn of_clicks(clicks: u32) -> Self {
        match clicks {
            0 | 1 => Self::Cells,
            2 => Self::Word,
            _ => Self::Lines,
        }
    }
}
