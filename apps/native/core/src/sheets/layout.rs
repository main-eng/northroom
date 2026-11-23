//! Layout of a spreadsheet file's view. A sheet is a grid with lettered columns along the top
//! and numbered rows down the left, both pinned while the sheet scrolls. Rows are 26 pt tall and
//! the number column 52 pt wide; text is 14 pt with 8 pt padding, bold cells are semibold,
//! numbers align to the end in tabular figures, and long text runs over the empty cells after
//! it. A workbook with several sheets shows them as tabs in a bar under the grid. The data
//! comes from `spreadsheet` and `workbook`; a shell draws backgrounds and each cell's bottom and
//! right lines at these frames.

use crate::board::card::Font;
use crate::dialog::layout::{Line, Measure, UNBOUNDED};
use crate::geometry::Rect;
use crate::sheets::spreadsheet::{
    shown_row, Alignment, Cell as SheetCell, Sheet, Workbook, COLUMN_MAX_WIDTH, COLUMN_MIN_WIDTH,
    EMPTY_DATABASE, EMPTY_SHEET, SHOWN_COLUMNS, UNREADABLE, UNREADABLE_DATABASE,
};
use crate::theme::Token;

const ROW: f64 = 26.0;
const NUMBERS: f64 = 52.0;
/// Cell text sits 8 pt from either side (plus 1 pt for the right line), 2.5 pt down on a 20 pt
/// line.
const CELL_PADDING: f64 = 8.0;
const CELL_DOWN: f64 = 2.5;
const CELL_LINE: f64 = 20.0;
const CELL_FONT: Font = Font::sans(14.0, Font::REGULAR);
const BOLD_FONT: Font = Font::sans(14.0, Font::SEMIBOLD);
/// Heading text is centred in the space left of its right line, 4.5 pt down on a 16 pt line.
const HEADING_FONT: Font = Font::sans(12.0, Font::REGULAR);
const HEADING_DOWN: f64 = 4.5;
const HEADING_LINE: f64 = 16.0;
/// The message shown instead of a grid: 16 pt in, on a 20 pt line.
const MESSAGE_IN: f64 = 16.0;
const MESSAGE_LINE: f64 = 20.0;
const MESSAGE_FONT: Font = Font::sans(14.0, Font::REGULAR);
/// The sheet tab bar: 26 pt tall, each tab with 12 pt either side of its name and a line on its
/// right. The open tab has a 2 pt underline.
const TAB_BAR: f64 = 26.0;
const TAB_ROOM: f64 = 12.0;
const TAB_LINE: f64 = 16.0;
const OPEN_UNDERLINE: f64 = 2.0;
const TAB_FONT: Font = Font::sans(12.0, Font::REGULAR);
const OPEN_TAB_FONT: Font = Font::sans(12.0, Font::MEDIUM);

/// The kind of file a table is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Workbook,
    Database,
}

/// The open table file as read.
#[derive(Clone, Debug, PartialEq)]
pub enum Table {
    /// A spreadsheet file read whole: delimited text, or a workbook's sheets.
    Sheets(Workbook),
    /// A database (`database`): each table is a sheet with the table's column names and no rows.
    /// Rows are read as they scroll into view (`windowed`).
    Database(Workbook),
    /// A file of this kind that cannot be read.
    Unreadable(Held),
}

impl Table {
    fn sheets(&self) -> &[Sheet] {
        match self {
            Self::Sheets(workbook) | Self::Database(workbook) => &workbook.sheets,
            Self::Unreadable(_) => &[],
        }
    }

    /// Whether sheet `open` shows its grid: a sheet with rows, or any database table, whose column
    /// names are useful before it has rows.
    fn shows_grid(&self, open: &Sheet) -> bool {
        match self {
            Self::Sheets(_) => open.row_count > 0,
            Self::Database(_) => true,
            Self::Unreadable(_) => false,
        }
    }
}

/// Column widths for a database table: each fits its name and the longest text in `rows`, as the
/// grid renders it, within the usual column limits. Delimited files count characters instead
/// (`spreadsheet::measure_column_widths`), which truncates a column of long numbers.
pub fn fitting_widths(
    names: &[String],
    rows: &[Vec<Option<SheetCell>>],
    measure: &mut Measure,
) -> Vec<f64> {
    names
        .iter()
        .enumerate()
        .map(|(column, name)| {
            let longest = rows
                .iter()
                .filter_map(|row| row.get(column).and_then(Option::as_ref))
                .max_by_key(|cell| cell.text.chars().count());
            let text = longest.map_or(0.0, |cell| measure(&cell.text, cell_font(cell)));
            let room = text.max(measure(name, HEADING_FONT)).ceil();
            (room + 2.0 * CELL_PADDING + 1.0).clamp(COLUMN_MIN_WIDTH, COLUMN_MAX_WIDTH)
        })
        .collect()
}

/// The rows of a `row_count`-row sheet that the `visible` part of the grid shows.
pub fn rows_in_view(visible: Rect, row_count: usize) -> std::ops::Range<usize> {
    let first = ((visible.y - ROW) / ROW).trunc().max(0.0) as usize;
    let last = row_count.min((visible.max_y() / ROW).ceil() as usize);
    first..last.max(first)
}

/// Database table `sheet` holding `rows` from row `first` on, ready for `grid` to draw.
pub fn windowed(sheet: &Sheet, first: usize, rows: Vec<Vec<Option<SheetCell>>>) -> Sheet {
    Sheet {
        first_row: first,
        rows,
        ..sheet.clone()
    }
}

fn columns(sheet: &Sheet) -> usize {
    sheet.column_count.min(SHOWN_COLUMNS)
}

/// The row-number column's width: `NUMBERS`, or wider if the last row's number needs it (a
/// database table with a million rows).
fn numbers_width(sheet: &Sheet, measure: &mut Measure) -> f64 {
    let widest = measure(&sheet.row_count.to_string(), HEADING_FONT.tabular());
    NUMBERS.max((widest + 2.0 * CELL_PADDING + 1.0).ceil())
}

/// A tab in the sheet bar, positioned from the bar's top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub frame: Rect,
    pub open: bool,
    pub hovered: bool,
    pub words: Line,
}

/// The sheet bar: its frame in the view, its tabs, and the line along the rest of its top.
#[derive(Clone, Debug, PartialEq)]
pub struct Tabs {
    pub frame: Rect,
    pub tabs: Vec<Tab>,
    pub rest: Rect,
}

/// The table's view with sheet `sheet` shown.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    /// The view's background colour; an unreadable workbook sits on the panel colour.
    pub ground: Token,
    /// Why there is no grid: the workbook cannot be read, or the sheet is empty.
    pub message: Option<Line>,
    /// The grid's scroll view frame, and whether it is shown.
    pub scroll: Rect,
    pub grid_shown: bool,
    /// The full scrolling grid's size.
    pub grid_width: f64,
    pub grid_height: f64,
    /// Sheet tabs, present when the workbook has several sheets.
    pub tabs: Option<Tabs>,
}

fn tab_frames(sheets: &[Sheet], open: usize, measure: &mut Measure) -> Vec<Rect> {
    let mut x = 0.0;
    sheets
        .iter()
        .enumerate()
        .map(|(index, sheet)| {
            let font = if index == open {
                OPEN_TAB_FONT
            } else {
                TAB_FONT
            };
            let width = TAB_ROOM + measure(&sheet.name, font) + TAB_ROOM + 1.0;
            let frame = Rect::new(x, 0.0, width, TAB_BAR);
            x += width;
            frame
        })
        .collect()
}

/// Whether `rect` contains the point, excluding its far edges.
fn holds(rect: &Rect, x: f64, y: f64) -> bool {
    x >= rect.x && x < rect.max_x() && y >= rect.y && y < rect.max_y()
}

/// Lays out a `width` × `height` view of `table` with sheet `sheet` open and the pointer over tab
/// `hovered`.
pub fn view(
    table: &Table,
    sheet: usize,
    width: f64,
    height: f64,
    hovered: Option<usize>,
    measure: &mut Measure,
) -> View {
    let sheets = table.sheets();
    let open = sheets.get(sheet);
    let bar = if sheets.len() > 1 { TAB_BAR } else { 0.0 };
    let tabs = (bar > 0.0).then(|| sheet_tabs(sheets, sheet, hovered, (width, height), measure));
    let (grid_width, grid_height) = open.map_or((NUMBERS, ROW), |open| {
        (
            numbers_width(open, measure) + open.column_widths[..columns(open)].iter().sum::<f64>(),
            (open.row_count + 1) as f64 * ROW,
        )
    });
    View {
        ground: if matches!(table, Table::Unreadable(_)) {
            Token::Panel
        } else {
            Token::Background
        },
        message: message(table, open, width),
        scroll: Rect::new(0.0, 0.0, width, height - bar),
        grid_shown: open.is_some_and(|open| table.shows_grid(open)),
        grid_width,
        grid_height,
        tabs,
    }
}

/// The message a `width`-wide view shows instead of a grid: the file is unreadable, the open
/// sheet is empty, or the database has no tables.
fn message(table: &Table, open: Option<&Sheet>, width: f64) -> Option<Line> {
    let line = |text: &str, color| {
        Line::new(
            text,
            MESSAGE_IN,
            MESSAGE_IN,
            width - 2.0 * MESSAGE_IN,
            MESSAGE_LINE,
            MESSAGE_FONT,
            color,
        )
    };
    match (table, open) {
        (Table::Unreadable(Held::Workbook), _) => Some(line(UNREADABLE, Token::Foreground)),
        (Table::Unreadable(Held::Database), _) => {
            Some(line(UNREADABLE_DATABASE, Token::Foreground))
        }
        (Table::Sheets(_), Some(open)) if open.row_count == 0 => {
            Some(line(EMPTY_SHEET, Token::MutedForeground))
        }
        (Table::Database(_), None) => Some(line(EMPTY_DATABASE, Token::MutedForeground)),
        (Table::Sheets(_) | Table::Database(_), _) => None,
    }
}

/// The sheet bar along the bottom of a `size` view, with sheet `sheet` open and the pointer over
/// tab `hovered`.
fn sheet_tabs(
    sheets: &[Sheet],
    sheet: usize,
    hovered: Option<usize>,
    size: (f64, f64),
    measure: &mut Measure,
) -> Tabs {
    let (width, height) = size;
    let frames = tab_frames(sheets, sheet, measure);
    let end = frames.last().map_or(0.0, Rect::max_x);
    let tabs = frames
        .into_iter()
        .zip(sheets)
        .enumerate()
        .map(|(index, (frame, shown))| tab(&shown.name, frame, index, sheet, hovered))
        .collect();
    Tabs {
        frame: Rect::new(0.0, height - TAB_BAR, width, TAB_BAR),
        tabs,
        rest: Rect::new(end, 0.0, (width - end).max(0.0), 1.0),
    }
}

/// Tab `index`, named `name`, in `frame`, with sheet `open_sheet` open and the pointer over tab
/// `hovered`. The open tab's name sits above its underline; it and a hovered tab use the
/// foreground colour.
fn tab(name: &str, frame: Rect, index: usize, open_sheet: usize, hovered: Option<usize>) -> Tab {
    let open = index == open_sheet;
    let hovered = hovered == Some(index);
    let top = if open {
        (TAB_BAR - OPEN_UNDERLINE - TAB_LINE) / 2.0
    } else {
        1.0 + (TAB_BAR - 1.0 - TAB_LINE) / 2.0
    };
    let color = if open || hovered {
        Token::Foreground
    } else {
        Token::MutedForeground
    };
    let font = if open { OPEN_TAB_FONT } else { TAB_FONT };
    let words = Line::new(
        name,
        frame.x + TAB_ROOM,
        top,
        UNBOUNDED,
        TAB_LINE,
        font,
        color,
    );
    Tab {
        frame,
        open,
        hovered,
        words,
    }
}

/// The sheet tab at (`x`, `y`) in the bar, with sheet `sheet` open.
pub fn tab_at(table: &Table, sheet: usize, x: f64, y: f64, measure: &mut Measure) -> Option<usize> {
    let sheets = table.sheets();
    if sheets.len() < 2 {
        return None;
    }
    tab_frames(sheets, sheet, measure)
        .iter()
        .position(|frame| holds(frame, x, y))
}

/// A grid cell as drawn: lines along its bottom and right, a heading background, and its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub frame: Rect,
    /// A column letter, row number or the corner, on the muted background.
    pub heading: bool,
    pub words: Option<Line>,
}

fn heading_cell(words: &str, frame: Rect, font: Font, measure: &mut Measure) -> Cell {
    let words = (!words.is_empty()).then(|| {
        let wide = measure(words, font).min(frame.width - 2.0 * CELL_PADDING - 1.0);
        Line::new(
            words,
            frame.x + (frame.width - 1.0 - wide) / 2.0,
            frame.y + HEADING_DOWN,
            frame.width - 2.0 * CELL_PADDING,
            HEADING_LINE,
            font,
            Token::MutedForeground,
        )
    });
    Cell {
        frame,
        heading: true,
        words,
    }
}

/// The cells of `sheet` within `visible` (the shown part of the grid, from its top-left), in
/// draw order: row cells, then row numbers pinned at the left, then column letters pinned at
/// the top over them, then the corner.
pub fn grid(sheet: &Sheet, visible: Rect, measure: &mut Measure) -> Vec<Cell> {
    let columns = columns(sheet);
    let widths = &sheet.column_widths[..columns];
    let numbers = numbers_width(sheet, measure);
    let lefts: Vec<f64> = widths
        .iter()
        .scan(numbers, |x, width| {
            let left = *x;
            *x += width;
            Some(left)
        })
        .collect();
    let shown = rows_in_view(visible, sheet.row_count);
    let mut cells = Vec::new();
    for index in shown.clone() {
        let top = (index + 1) as f64 * ROW;
        // A row the sheet lacks (a database table that lost rows since it was counted) is drawn
        // empty.
        let row = index
            .checked_sub(sheet.first_row)
            .and_then(|held| sheet.rows.get(held))
            .map_or(&[][..], Vec::as_slice);
        row_cells(row, top, (&lefts, widths), measure, &mut cells);
    }
    for index in shown {
        let frame = Rect::new(visible.x, (index + 1) as f64 * ROW, numbers, ROW);
        cells.push(heading_cell(
            &(index + 1).to_string(),
            frame,
            HEADING_FONT.tabular(),
            measure,
        ));
    }
    for (column, left) in lefts.iter().enumerate() {
        let name = sheet.column_names.get(column).map_or("", String::as_str);
        cells.push(heading_cell(
            name,
            Rect::new(*left, visible.y, widths[column], ROW),
            HEADING_FONT,
            measure,
        ));
    }
    cells.push(heading_cell(
        "",
        Rect::new(visible.x, visible.y, numbers, ROW),
        HEADING_FONT,
        measure,
    ));
    cells
}

/// Rebases `cells` (from `grid`, for `visible`) onto the visible part's top-left, which is what a
/// shell draws at. A table with millions of rows is tens of millions of points tall, beyond what
/// the shell's `f32` coordinates hold to the point; relative to the view's corner, row five
/// million is placed as exactly as the first.
pub fn from_view(mut cells: Vec<Cell>, visible: Rect) -> Vec<Cell> {
    for cell in &mut cells {
        cell.frame.x -= visible.x;
        cell.frame.y -= visible.y;
        if let Some(words) = &mut cell.words {
            words.frame.x -= visible.x;
            words.frame.y -= visible.y;
        }
    }
    cells
}

/// One row's cells, with its top at `top`, across `columns` (their lefts and widths). A cell
/// whose text runs over the empty cells after it becomes one cell spanning them.
fn row_cells(
    row: &[Option<SheetCell>],
    top: f64,
    columns: (&[f64], &[f64]),
    measure: &mut Measure,
    cells: &mut Vec<Cell>,
) {
    let (lefts, widths) = columns;
    let row = shown_row(row, widths.len());
    let mut column = 0;
    while column < widths.len() {
        let cell = row.get(column).and_then(Option::as_ref);
        let span = cell.map_or(1, |cell| cell.span);
        let frame = Rect::new(
            lefts[column],
            top,
            widths[column..column + span].iter().sum(),
            ROW,
        );
        cells.push(Cell {
            frame,
            heading: false,
            words: cell.map(|shown| cell_words(shown.cell, frame, measure)),
        });
        column += span;
    }
}

/// The font for `cell`: semibold when bold, with tabular figures for numbers.
fn cell_font(cell: &SheetCell) -> Font {
    match (cell.alignment, cell.bold) {
        (Alignment::End, false) => CELL_FONT.tabular(),
        (Alignment::End, true) => BOLD_FONT.tabular(),
        (_, true) => BOLD_FONT,
        (_, false) => CELL_FONT,
    }
}

/// `cell`'s text line in `frame`: numbers at the end, 8 pt from the side it aligns to.
fn cell_words(cell: &SheetCell, frame: Rect, measure: &mut Measure) -> Line {
    let font = cell_font(cell);
    let room = frame.width - 2.0 * CELL_PADDING - 1.0;
    let wide = measure(&cell.text, font).min(room);
    let x = match cell.alignment {
        Alignment::Start => frame.x + CELL_PADDING,
        Alignment::Center => frame.x + CELL_PADDING + (room - wide) / 2.0,
        Alignment::End => frame.max_x() - 1.0 - CELL_PADDING - wide,
    };
    Line::new(
        &cell.text,
        x,
        frame.y + CELL_DOWN,
        room,
        CELL_LINE,
        font,
        Token::Foreground,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheets::spreadsheet::Cell as Field;
    use crate::text::typeset::test_measure::sevens;

    fn field(text: &str, alignment: Alignment) -> Option<Field> {
        Some(Field {
            text: text.into(),
            alignment,
            bold: false,
        })
    }

    fn sheet(name: &str, rows: Vec<Vec<Option<Field>>>) -> Sheet {
        Sheet {
            name: name.into(),
            column_count: 3,
            column_widths: vec![100.0, 80.0, 60.0],
            column_names: vec!["A".into(), "B".into(), "C".into()],
            first_row: 0,
            row_count: rows.len(),
            rows,
        }
    }

    #[test]
    fn one_sheet_has_no_tabs_and_an_empty_or_unreadable_one_says_so() {
        let one = Table::Sheets(Workbook {
            sheets: vec![sheet("Sheet1", vec![vec![field("a", Alignment::Start)]])],
        });
        let shown = view(&one, 0, 800.0, 600.0, None, &mut sevens);
        assert_eq!(
            (shown.ground, shown.message.clone(), shown.tabs.clone()),
            (Token::Background, None, None)
        );
        assert_eq!(
            (shown.scroll, shown.grid_shown),
            (Rect::new(0.0, 0.0, 800.0, 600.0), true)
        );
        assert_eq!((shown.grid_width, shown.grid_height), (52.0 + 240.0, 52.0));
        let empty = Table::Sheets(Workbook {
            sheets: vec![sheet("Sheet1", Vec::new())],
        });
        let shown = view(&empty, 0, 800.0, 600.0, None, &mut sevens);
        assert_eq!(
            shown.message,
            Some(Line::new(
                EMPTY_SHEET,
                16.0,
                16.0,
                768.0,
                20.0,
                MESSAGE_FONT,
                Token::MutedForeground
            ))
        );
        assert!(!shown.grid_shown);
        let unreadable = Table::Unreadable(Held::Workbook);
        let broken = view(&unreadable, 0, 800.0, 600.0, None, &mut sevens);
        assert_eq!(
            (
                broken.ground,
                broken.message.map(|line| line.color),
                broken.grid_shown
            ),
            (Token::Panel, Some(Token::Foreground), false)
        );
    }

    #[test]
    fn several_sheets_are_tabs_under_the_grid_the_open_one_medium() {
        let two = Table::Sheets(Workbook {
            sheets: vec![sheet("Orders", Vec::new()), sheet("Q2", Vec::new())],
        });
        let shown = view(&two, 1, 800.0, 600.0, Some(0), &mut sevens);
        let tabs = shown.tabs.expect("tabs");
        assert_eq!(
            (tabs.frame, shown.scroll.height),
            (Rect::new(0.0, 574.0, 800.0, 26.0), 574.0)
        );
        // "Orders" is 42 pt plus 12 pt either side and its line, 67; "Q2" is 14 pt, so 39.
        assert_eq!(tabs.tabs[0].frame, Rect::new(0.0, 0.0, 67.0, 26.0));
        assert_eq!(
            tabs.tabs[0].words,
            Line::new(
                "Orders",
                12.0,
                5.5,
                UNBOUNDED,
                16.0,
                TAB_FONT,
                Token::Foreground
            )
        );
        assert_eq!(
            (tabs.tabs[1].frame, tabs.tabs[1].open),
            (Rect::new(67.0, 0.0, 39.0, 26.0), true)
        );
        assert_eq!(
            tabs.tabs[1].words,
            Line::new(
                "Q2",
                79.0,
                4.0,
                UNBOUNDED,
                16.0,
                OPEN_TAB_FONT,
                Token::Foreground
            )
        );
        assert_eq!(tabs.rest, Rect::new(106.0, 0.0, 694.0, 1.0));
        assert_eq!(
            view(&two, 0, 800.0, 600.0, None, &mut sevens)
                .tabs
                .expect("tabs")
                .tabs[1]
                .words
                .color,
            Token::MutedForeground
        );
        assert_eq!(
            (
                tab_at(&two, 1, 70.0, 10.0, &mut sevens),
                tab_at(&two, 1, 106.0, 10.0, &mut sevens)
            ),
            (Some(1), None)
        );
    }

    #[test]
    fn the_grid_draws_the_rows_in_view_and_holds_its_headings_at_the_edges() {
        let rows = vec![
            vec![
                field("a long text that runs on", Alignment::Start),
                None,
                field("1", Alignment::End),
            ],
            vec![
                None,
                field("mid", Alignment::Center),
                field("22", Alignment::End),
            ],
            vec![field("x", Alignment::Start)],
        ];
        let sheet = sheet("Sheet1", rows);
        let cells = grid(&sheet, Rect::new(0.0, 0.0, 400.0, 50.0), &mut sevens);
        // The first two rows are in view (26 to 78), and the text runs over the empty cell after
        // it.
        assert_eq!(cells[0].frame, Rect::new(52.0, 26.0, 180.0, 26.0));
        let text = cells[0].words.clone().expect("words");
        assert_eq!(
            (text.frame, text.font),
            (Rect::new(60.0, 28.5, 163.0, 20.0), CELL_FONT)
        );
        let number = cells[1].words.clone().expect("words");
        assert_eq!(
            (number.frame.x, number.font),
            (232.0 + 60.0 - 1.0 - 8.0 - 7.0, CELL_FONT.tabular())
        );
        assert_eq!(
            (cells[2].frame, cells[2].words.is_none()),
            (Rect::new(52.0, 52.0, 100.0, 26.0), true)
        );
        assert_eq!(
            cells[3].words.clone().map(|line| line.frame.x),
            Some(152.0 + 8.0 + (63.0 - 21.0) / 2.0)
        );
        // Then the numbers, the letters and the corner.
        let headings: Vec<String> = cells[5..]
            .iter()
            .map(|cell| cell.words.clone().map_or(String::new(), |line| line.text))
            .collect();
        assert_eq!(headings, ["1", "2", "A", "B", "C", ""]);
        assert_eq!(cells[5].frame, Rect::new(0.0, 26.0, 52.0, 26.0));
        assert_eq!(
            cells[5]
                .words
                .clone()
                .map(|line| (line.frame.x, line.frame.y, line.font)),
            Some((22.0, 30.5, HEADING_FONT.tabular()))
        );
        assert!(cells[5..].iter().all(|cell| cell.heading) && !cells[0].heading);

        // Scrolled down and across, the headings stay pinned at the view's edges.
        let cells = grid(&sheet, Rect::new(30.0, 40.0, 400.0, 60.0), &mut sevens);
        let numbers: Vec<Rect> = cells
            .iter()
            .filter(|cell| cell.heading && cell.frame.width == 52.0 && cell.frame.y > 40.0)
            .map(|cell| cell.frame)
            .collect();
        assert_eq!(
            numbers,
            [
                Rect::new(30.0, 52.0, 52.0, 26.0),
                Rect::new(30.0, 78.0, 52.0, 26.0)
            ]
        );
        assert_eq!(
            cells.last().map(|cell| cell.frame),
            Some(Rect::new(30.0, 40.0, 52.0, 26.0))
        );
    }

    #[test]
    fn a_table_s_columns_fit_their_names_and_longest_texts_as_written() {
        let names = [
            "id".to_owned(),
            "name".to_owned(),
            "a long column name".to_owned(),
        ];
        let rows = vec![
            vec![
                field("1", Alignment::End),
                field("Ada", Alignment::Start),
                None,
            ],
            vec![
                field("1000000", Alignment::End),
                field(&"x".repeat(80), Alignment::Start),
            ],
        ];
        // "1000000" is 49 pt plus 17 pt padding; 80 characters exceed the maximum width; a name
        // longer than every value sets the width, 126 plus 17.
        assert_eq!(
            fitting_widths(&names, &rows, &mut sevens),
            [66.0, 360.0, 143.0]
        );
        // An empty column gets the minimum width.
        assert_eq!(fitting_widths(&["n".to_owned()], &[], &mut sevens), [56.0]);
    }

    /// A database table as its view holds it: named columns, `row_count` rows, none loaded.
    fn table_of(row_count: usize) -> Sheet {
        Sheet {
            column_names: vec!["id".into(), "name".into(), "city".into()],
            row_count,
            ..sheet("people", Vec::new())
        }
    }

    #[test]
    fn a_database_shows_its_tables_under_their_columns_names_even_with_no_rows() {
        let database = Table::Database(Workbook {
            sheets: vec![table_of(0), table_of(1_000_000)],
        });
        let empty = view(&database, 0, 800.0, 600.0, None, &mut sevens);
        assert_eq!((empty.grid_shown, empty.message), (true, None));
        assert_eq!(empty.grid_height, 26.0);
        let cells = grid(&table_of(0), Rect::new(0.0, 0.0, 400.0, 100.0), &mut sevens);
        let headings: Vec<String> = cells
            .iter()
            .map(|cell| cell.words.clone().map_or(String::new(), |line| line.text))
            .collect();
        assert_eq!(headings, ["id", "name", "city", ""]);
        // Its height covers all its rows, loaded or not.
        let large = view(&database, 1, 800.0, 600.0, None, &mut sevens);
        assert_eq!(large.grid_height, 1_000_001.0 * 26.0);
        let none = Table::Database(Workbook { sheets: Vec::new() });
        let shown = view(&none, 0, 800.0, 600.0, None, &mut sevens);
        assert_eq!(
            (
                shown.grid_shown,
                shown.ground,
                shown.message.map(|line| (line.text, line.color))
            ),
            (
                false,
                Token::Background,
                Some((EMPTY_DATABASE.to_owned(), Token::MutedForeground))
            )
        );
        let broken = view(
            &Table::Unreadable(Held::Database),
            0,
            800.0,
            600.0,
            None,
            &mut sevens,
        );
        assert_eq!(
            (broken.ground, broken.message.map(|line| line.text)),
            (Token::Panel, Some(UNREADABLE_DATABASE.to_owned()))
        );
    }

    #[test]
    fn a_table_s_rows_in_view_are_drawn_where_they_stand_in_it() {
        // Scrolled to rows 500 and 501 (26 × 501 from the top, under the pinned headings).
        let visible = Rect::new(0.0, 13_026.0, 400.0, 78.0);
        assert_eq!(rows_in_view(visible, 1_000_000), 500..504);
        assert_eq!(rows_in_view(visible, 502), 500..502);
        assert_eq!(rows_in_view(Rect::new(0.0, 0.0, 400.0, 50.0), 0), 0..0);
        let held = windowed(
            &table_of(1_000),
            500,
            vec![
                vec![field("501", Alignment::End), field("Ada", Alignment::Start)],
                vec![field("502", Alignment::End)],
            ],
        );
        let cells = grid(&held, visible, &mut sevens);
        let words: Vec<(String, f64)> = cells
            .iter()
            .filter_map(|cell| cell.words.clone().map(|line| (line.text, cell.frame.y)))
            .collect();
        // Rows 500 and 501 have their cells; 502 and 503 are in view but not loaded, so empty.
        assert_eq!(
            words[..3],
            [
                ("501".to_owned(), 13_026.0),
                ("Ada".to_owned(), 13_026.0),
                ("502".to_owned(), 13_052.0)
            ]
        );
        let numbers: Vec<&str> = words[3..7].iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(numbers, ["501", "502", "503", "504"]);
        let rows_drawn = cells
            .iter()
            .filter(|cell| !cell.heading && cell.frame.x == 52.0)
            .count();
        assert_eq!(rows_drawn, 4);
    }

    #[test]
    fn cells_are_handed_over_from_the_view_s_corner_so_a_deep_row_is_placed_to_the_point() {
        // Row 5,000,000, scrolled 0.5 pt past its top and 10 pt across.
        let visible = Rect::new(10.0, 130_000_026.5, 400.0, 52.0);
        let held = windowed(
            &table_of(6_000_000),
            5_000_000,
            vec![vec![
                field("5000001", Alignment::End),
                field("Ada", Alignment::Start),
            ]],
        );
        let cells = from_view(grid(&held, visible, &mut sevens), visible);
        // The row numbers take what "6000000" needs, 49 plus 17, instead of the usual 52.
        assert_eq!(cells[0].frame, Rect::new(56.0, -0.5, 100.0, 26.0));
        let words = cells[1].words.clone().expect("words");
        assert_eq!(
            (words.text.as_str(), words.frame.x, words.frame.y),
            ("Ada", 164.0, 2.0)
        );
        // The pinned headings sit at the view's edges.
        assert_eq!(
            cells.last().map(|cell| cell.frame),
            Some(Rect::new(0.0, 0.0, 66.0, 26.0))
        );
        let number = cells
            .iter()
            .find(|cell| {
                cell.heading
                    && cell
                        .words
                        .as_ref()
                        .is_some_and(|line| line.text == "5000001")
            })
            .expect("the row's number");
        assert_eq!(number.frame, Rect::new(0.0, -0.5, 66.0, 26.0));
        let whole = view(
            &Table::Database(Workbook {
                sheets: vec![table_of(6_000_000)],
            }),
            0,
            800.0,
            600.0,
            None,
            &mut sevens,
        );
        assert_eq!(whole.grid_width, 66.0 + 240.0);
    }
}
