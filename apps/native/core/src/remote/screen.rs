//! A terminal's visible grid as a phone draws it: each line as runs of text that share a look,
//! in the colours this computer's theme gives them, so the phone needs no palette of its own.

use crate::terminal::cells::{Cell, Cursor, CursorShape, GridSize, Rgb, Underline};
use crate::terminal::palette::Palette;
use serde::Serialize;

fn is_false(flag: &bool) -> bool {
    !flag
}

/// Text in one look, starting at `column`. A wide character takes two columns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Run {
    pub column: usize,
    pub text: String,
    /// The text's colour as `#rrggbb`; the screen's `ink` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ink: Option<String>,
    /// The colour behind the text; the screen's `ground` when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub dim: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub underline: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub strikeout: bool,
}

impl Run {
    /// Whether `other` looks the same, so its text can join this run's.
    fn looks_like(&self, other: &Run) -> bool {
        (&self.ink, &self.ground, self.bold, self.italic, self.dim)
            == (
                &other.ink,
                &other.ground,
                other.bold,
                other.italic,
                other.dim,
            )
            && (self.underline, self.strikeout) == (other.underline, other.strikeout)
    }

    /// Whether the run shows nothing: spaces on the screen's own ground.
    fn is_blank(&self) -> bool {
        self.ground.is_none() && !self.underline && self.text.chars().all(|c| c == ' ')
    }
}

/// What a watched terminal shows now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Screen {
    pub columns: usize,
    pub lines: usize,
    /// The default text colour and the colour behind everything, as `#rrggbb`.
    pub ink: String,
    pub ground: String,
    /// One entry a line, top to bottom. Blank ends of lines are left out.
    pub rows: Vec<Vec<Run>>,
    /// The cursor's line and column, when it is on screen and shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<[usize; 2]>,
}

fn hex(colour: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", colour.r, colour.g, colour.b)
}

/// A cell's text and background colours as drawn, `None` for the terminal's own.
fn colours(cell: &Cell<'_>, palette: &Palette) -> (Option<Rgb>, Option<Rgb>) {
    let ink = palette
        .resolve(cell.foreground, cell.style.bold)
        .filter(|ink| *ink != palette.foreground);
    let ground = palette.resolve(cell.background, false);
    if cell.style.inverse {
        (
            Some(ground.unwrap_or(palette.background)),
            Some(ink.unwrap_or(palette.foreground)),
        )
    } else {
        (ink, ground)
    }
}

/// `cell` as a run of its own.
fn run_of(cell: &Cell<'_>, palette: &Palette) -> Run {
    let (ink, ground) = colours(cell, palette);
    let mut text = String::new();
    if cell.style.hidden {
        text.push(' ');
    } else {
        text.push(cell.character);
        text.extend(cell.marks);
    }
    Run {
        column: cell.column,
        text,
        ink: ink.map(hex),
        ground: ground.map(hex),
        bold: cell.style.bold,
        italic: cell.style.italic,
        dim: cell.style.dim,
        underline: cell.style.underline != Underline::None,
        strikeout: cell.style.strikeout,
    }
}

/// Adds `run` to `row`, joined to the run before it when the two look alike.
fn add(row: &mut Vec<Run>, run: Run) {
    match row.last_mut() {
        Some(last) if last.looks_like(&run) => last.text.push_str(&run.text),
        _ => row.push(run),
    }
}

/// Drops what shows nothing: blank runs, and the spaces a line ends in.
fn trim(row: &mut Vec<Run>) {
    while row.last().is_some_and(Run::is_blank) {
        row.pop();
    }
    if let Some(last) = row.last_mut().filter(|last| last.ground.is_none()) {
        last.text.truncate(last.text.trim_end_matches(' ').len());
    }
    row.retain(|run| !run.is_blank());
}

/// Where the cursor shows on a grid of `grid`, when it does.
fn cursor_place(cursor: Cursor, grid: GridSize) -> Option<[usize; 2]> {
    let line = usize::try_from(cursor.line).ok()?;
    (cursor.shape != CursorShape::Hidden && line < grid.lines).then_some([line, cursor.column])
}

/// The screen of a grid of `grid` whose visible cells are `cells`, coloured by `palette`.
pub fn build(
    cells: &mut dyn Iterator<Item = Cell<'_>>,
    cursor: Cursor,
    grid: GridSize,
    palette: &Palette,
) -> Screen {
    let mut rows = vec![Vec::new(); grid.lines];
    for cell in cells {
        if let Some(row) = rows.get_mut(cell.line) {
            add(row, run_of(&cell, palette));
        }
    }
    rows.iter_mut().for_each(trim);
    Screen {
        columns: grid.columns,
        lines: grid.lines,
        ink: hex(palette.foreground),
        ground: hex(palette.background),
        rows,
        cursor: cursor_place(cursor, grid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::cells::{rgb, Colour, Style};

    fn palette() -> Palette {
        Palette::on([10, 10, 10], [240, 240, 240], [60, 60, 60])
    }

    fn cell(line: usize, column: usize, character: char) -> Cell<'static> {
        Cell {
            line,
            column,
            character,
            marks: &[],
            wide: false,
            foreground: Colour::Foreground,
            background: Colour::Background,
            underline_colour: None,
            style: Style::default(),
            selected: false,
        }
    }

    fn line(line: usize, text: &str) -> Vec<Cell<'static>> {
        text.chars()
            .enumerate()
            .map(|(column, character)| cell(line, column, character))
            .collect()
    }

    fn cursor(line: i32, column: usize, shape: CursorShape) -> Cursor {
        Cursor {
            line,
            column,
            shape,
            blinking: false,
            wide: false,
        }
    }

    fn grid(columns: usize, lines: usize) -> GridSize {
        GridSize { columns, lines }
    }

    #[test]
    fn plain_text_is_one_run_without_the_spaces_a_line_ends_in() {
        let mut cells = line(0, "ls -la   ");
        cells.extend(line(2, "  next  "));
        let screen = build(
            &mut cells.into_iter(),
            cursor(2, 6, CursorShape::Block),
            grid(9, 3),
            &palette(),
        );
        let texts: Vec<Vec<&str>> = screen
            .rows
            .iter()
            .map(|row| row.iter().map(|run| run.text.as_str()).collect())
            .collect();
        assert_eq!(texts, [vec!["ls -la"], vec![], vec!["  next"]]);
        assert_eq!(screen.cursor, Some([2, 6]));
        assert_eq!(
            (screen.ink.as_str(), screen.ground.as_str()),
            ("#f0f0f0", "#0a0a0a")
        );
    }

    #[test]
    fn a_change_of_look_starts_a_run_at_its_column() {
        let mut cells = line(0, "ok no");
        cells[3].foreground = Colour::Rgb(rgb(255, 0, 0));
        cells[4].foreground = Colour::Rgb(rgb(255, 0, 0));
        cells[3].style.bold = true;
        cells[4].style.bold = true;
        let screen = build(
            &mut cells.into_iter(),
            cursor(0, 0, CursorShape::Hidden),
            grid(5, 1),
            &palette(),
        );
        let row = &screen.rows[0];
        assert_eq!((row[0].column, row[0].text.as_str()), (0, "ok "));
        assert_eq!((row[1].column, row[1].text.as_str()), (3, "no"));
        assert_eq!(row[1].ink.as_deref(), Some("#ff0000"));
        assert!(row[1].bold && !row[0].bold);
        assert_eq!(screen.cursor, None);
    }

    #[test]
    fn inverse_text_swaps_its_colours_and_keeps_its_spaces() {
        let mut cells = line(0, "a  ");
        for cell in &mut cells {
            cell.style.inverse = true;
        }
        let screen = build(
            &mut cells.into_iter(),
            cursor(0, 0, CursorShape::Hidden),
            grid(3, 1),
            &palette(),
        );
        let run = &screen.rows[0][0];
        assert_eq!(run.text, "a  ");
        assert_eq!(run.ink.as_deref(), Some("#0a0a0a"));
        assert_eq!(run.ground.as_deref(), Some("#f0f0f0"));
    }

    #[test]
    fn a_cursor_scrolled_out_of_view_is_not_shown() {
        let screen = build(
            &mut Vec::new().into_iter(),
            cursor(-3, 0, CursorShape::Block),
            grid(4, 2),
            &palette(),
        );
        assert_eq!(screen.cursor, None);
        assert_eq!(screen.rows, vec![Vec::new(), Vec::new()]);
    }

    #[test]
    fn what_is_absent_is_left_out_of_the_message() {
        let screen = build(
            &mut line(0, "hi").into_iter(),
            cursor(0, 0, CursorShape::Hidden),
            grid(2, 1),
            &palette(),
        );
        assert_eq!(
            serde_json::to_string(&screen.rows).unwrap(),
            r#"[[{"column":0,"text":"hi"}]]"#
        );
    }
}
