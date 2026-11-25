//! Reads an OpenDocument spreadsheet (`.ods`) for display: a zip whose `content.xml` holds
//! every sheet. Each visible table becomes a sheet of cell text with the file's bold, alignment
//! and column widths. A cell shows the text the file saved for it, which is what its
//! application displayed, so formulas show their saved result and nothing is calculated or
//! formatted here; a cell saved with a value and no text shows the value as written. Fills,
//! colours, charts, pictures, merged cells, notes and conditional formatting are ignored.
//!
//! A file can say "this cell 16,384 times" or "this row a million times" in a few bytes, so
//! empty runs are skipped without being built and the cells built are counted against
//! `CELL_LIMIT`.

use crate::sheets::spreadsheet::{self, Alignment, Cell, Sheet, Workbook};
use crate::sheets::workbook::{
    fail, js_number, js_to_string, place_at, read_part, Piece, Result, WorkbookError, XmlElement,
    ZipArchive, COLUMN_LIMIT, ROW_LIMIT,
};
use std::collections::HashMap;

/// The part holding the sheets, and the part holding the styles the file names.
const CONTENT: &str = "content.xml";
const STYLES: &str = "styles.xml";
/// The media type an OpenDocument spreadsheet or its template declares in its `mimetype` entry.
const MEDIA_TYPE: &[u8] = b"application/vnd.oasis.opendocument.spreadsheet";
/// The most cells one file is shown with. Repeats let a small file ask for billions.
const CELL_LIMIT: usize = 4_000_000;
/// How far a style's parents are followed; a longer chain is a loop.
const STYLE_DEPTH: usize = 32;
/// Points per inch, for lengths the file gives in physical units.
const POINTS_PER_INCH: f64 = 96.0;

/// Whether the zip is an OpenDocument spreadsheet: it says so in its `mimetype` entry, or it
/// has no such entry, holds `content.xml` and is not an Office file (no `_rels/.rels`).
pub fn is_spreadsheet(archive: &ZipArchive) -> bool {
    match archive.read_entry("mimetype", 256) {
        Ok(Some(declared)) => declared.starts_with(MEDIA_TYPE),
        _ => {
            let mut names = archive.entry_names();
            let (mut content, mut office) = (false, false);
            for name in &mut names {
                content |= name == CONTENT;
                office |= name == "_rels/.rels";
            }
            content && !office
        }
    }
}

/// What a style says about the things this reader shows; `None` where it says nothing and its
/// parent decides.
#[derive(Clone, Debug, Default)]
struct Style {
    parent: Option<String>,
    bold: Option<bool>,
    alignment: Option<Alignment>,
    /// A table style's `table:display="false"`: the sheet is hidden.
    hidden: bool,
    /// A column style's width in points.
    width: Option<f64>,
}

/// Styles by family and name (`key`).
type Styles = HashMap<String, Style>;

fn key(family: &str, name: &str) -> String {
    format!("{family}/{name}")
}

/// A length such as `2.258cm` in points; `None` for a unit this reader does not know.
fn read_length(length: &str) -> Option<f64> {
    let digits = length
        .find(|character: char| character.is_ascii_alphabetic())
        .unwrap_or(length.len());
    let value: f64 = length[..digits].trim().parse().ok()?;
    let per_inch = match &length[digits..] {
        "in" => 1.0,
        "cm" => 2.54,
        "mm" => 25.4,
        "pt" => 72.0,
        "pc" => 6.0,
        "px" => POINTS_PER_INCH,
        _ => return None,
    };
    (value.is_finite() && value >= 0.0).then_some(value / per_inch * POINTS_PER_INCH)
}

/// Whether a `fo:font-weight` is bold: the word, or a weight of 600 and up.
fn is_bold(weight: &str) -> bool {
    weight == "bold" || weight.parse::<u32>().is_ok_and(|weight| weight >= 600)
}

/// The alignment a `fo:text-align` asks for. Justified text starts where unaligned text does.
fn read_alignment(align: &str) -> Option<Alignment> {
    match align {
        "start" | "left" | "justify" => Some(Alignment::Start),
        "end" | "right" => Some(Alignment::End),
        "center" => Some(Alignment::Center),
        _ => None,
    }
}

fn read_style(style: &XmlElement) -> Style {
    // `text-align-source="value-type"` says the cell's type decides, whatever `text-align` says.
    let by_type = style
        .child("table-cell-properties")
        .and_then(|cell| cell.attribute("style:text-align-source"))
        == Some("value-type");
    Style {
        parent: style
            .attribute("style:parent-style-name")
            .map(str::to_owned),
        bold: style
            .child("text-properties")
            .and_then(|text| text.attribute("fo:font-weight"))
            .map(is_bold),
        alignment: style
            .child("paragraph-properties")
            .and_then(|paragraph| paragraph.attribute("fo:text-align"))
            .filter(|_| !by_type)
            .and_then(read_alignment),
        hidden: style
            .child("table-properties")
            .and_then(|table| table.attribute("table:display"))
            == Some("false"),
        width: style
            .child("table-column-properties")
            .and_then(|column| column.attribute("style:column-width"))
            .and_then(read_length),
    }
}

/// Adds the styles a part defines, named and automatic. A name already known is kept: the
/// content's own styles are read first and win.
fn add_styles(styles: &mut Styles, part: &XmlElement) {
    let lists = part
        .children
        .iter()
        .filter(|child| child.name == "styles" || child.name == "automatic-styles");
    for style in lists.flat_map(|list| list.children_named("style")) {
        let (Some(family), Some(name)) = (
            style.attribute("style:family"),
            style.attribute("style:name"),
        ) else {
            continue;
        };
        styles
            .entry(key(family, name))
            .or_insert_with(|| read_style(style));
    }
}

/// How a cell with a style looks: bold or not, and the alignment the style fixes, if any. Each
/// comes from the nearest style up the parents that says.
fn look(styles: &Styles, name: Option<&str>) -> (bool, Option<Alignment>) {
    let (mut bold, mut alignment) = (None, None);
    let mut next = name.map(|name| key("table-cell", name));
    for _ in 0..STYLE_DEPTH {
        let Some(style) = next.as_ref().and_then(|name| styles.get(name)) else {
            break;
        };
        bold = bold.or(style.bold);
        alignment = alignment.or(style.alignment);
        next = style
            .parent
            .as_ref()
            .map(|parent| key("table-cell", parent));
    }
    (bold.unwrap_or(false), alignment)
}

// --- Cell text -----------------------------------------------------------------------------

/// A paragraph's text as it is being put together. White space in the file's text is not
/// content: a run of it is one space, and none starts a paragraph. Spaces that are content are
/// written as `<text:s>`.
#[derive(Default)]
struct Paragraph {
    text: String,
    /// The text ends in a space that came from the file's white space.
    spaced: bool,
}

impl Paragraph {
    fn add_text(&mut self, text: &str) {
        for character in text.chars() {
            if matches!(character, ' ' | '\t' | '\r' | '\n') {
                if !self.text.is_empty() && !self.spaced {
                    self.text.push(' ');
                    self.spaced = true;
                }
            } else {
                self.text.push(character);
                self.spaced = false;
            }
        }
    }

    fn add_content(&mut self, content: &str) {
        self.text.push_str(content);
        self.spaced = false;
    }

    /// Adds an element's content: text, the spans and links that hold more of it, and the
    /// elements that stand for a space, a tab and a line break. Notes and anything else inside
    /// a paragraph are not its text.
    fn add(&mut self, element: &XmlElement) {
        for piece in element.pieces() {
            match piece {
                Piece::Text(text) => self.add_text(text),
                Piece::Child(child) => match child.name.as_str() {
                    "span" | "a" => self.add(child),
                    "s" => {
                        let count = child
                            .attribute("text:c")
                            .and_then(|count| count.parse::<usize>().ok())
                            .unwrap_or(1);
                        // A count is bounded by the columns a row holds, as a cell's text is.
                        self.add_content(&" ".repeat(count.min(COLUMN_LIMIT as usize)));
                    }
                    "tab" => self.add_content("\t"),
                    "line-break" => self.add_content("\n"),
                    _ => {}
                },
            }
        }
    }
}

/// The text a cell's paragraphs show, one line each.
fn read_paragraphs(cell: &XmlElement) -> String {
    let lines: Vec<String> = cell
        .children_named("p")
        .map(|paragraph| {
            let mut read = Paragraph::default();
            read.add(paragraph);
            read.text
        })
        .collect();
    lines.join("\n")
}

/// A duration such as `PT12H30M05S` as `12:30:05`; as written when it is not one.
fn read_time(value: &str) -> String {
    let parts = value.strip_prefix("PT").and_then(|rest| {
        let (hours, rest) = rest.split_once('H')?;
        let (minutes, rest) = rest.split_once('M')?;
        let seconds = rest.strip_suffix('S')?;
        let whole = seconds.split('.').next().unwrap_or(seconds);
        let numbers = [hours, minutes, whole];
        numbers
            .iter()
            .all(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| format!("{hours}:{minutes:0>2}:{whole:0>2}"))
    });
    parts.unwrap_or_else(|| value.to_owned())
}

/// The text for a cell saved with a value and no paragraph: the value as the file wrote it.
fn read_value(cell: &XmlElement, value_type: Option<&str>) -> String {
    let number = || cell.attribute("office:value").map(js_number);
    match value_type {
        Some("float" | "currency") => number().map(js_to_string).unwrap_or_default(),
        Some("percentage") => number()
            .map(|value| format!("{}%", js_to_string(value * 100.0)))
            .unwrap_or_default(),
        Some("boolean") => match cell.attribute("office:boolean-value") {
            Some("true") => "TRUE".to_owned(),
            Some(_) => "FALSE".to_owned(),
            None => String::new(),
        },
        Some("date") => cell
            .attribute("office:date-value")
            .map(|date| date.replace('T', " "))
            .unwrap_or_default(),
        Some("time") => cell
            .attribute("office:time-value")
            .map(read_time)
            .unwrap_or_default(),
        Some("string") => cell
            .attribute("office:string-value")
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

/// A cell's text and its type's alignment: numbers, dates and times at the end, true, false and
/// errors in the middle, text at the start.
fn read_cell_text(cell: &XmlElement) -> (String, Alignment) {
    let value_type = cell.attribute("office:value-type");
    let alignment = match value_type {
        Some("float" | "currency" | "percentage" | "date" | "time") => Alignment::End,
        Some("boolean") => Alignment::Center,
        _ if cell.attribute("calcext:value-type") == Some("error") => Alignment::Center,
        _ => Alignment::Start,
    };
    let shown = read_paragraphs(cell);
    if shown.is_empty() {
        (read_value(cell, value_type), alignment)
    } else {
        (shown, alignment)
    }
}

// --- Sheets --------------------------------------------------------------------------------

/// How many times an element says it repeats (1 when it does not say, or says nonsense).
fn repeats(element: &XmlElement, attribute: &str) -> usize {
    element
        .attribute(attribute)
        .and_then(|count| count.parse::<usize>().ok())
        .filter(|&count| count > 0)
        .unwrap_or(1)
}

/// A table's column or row elements in order, out of the groups and header blocks that wrap
/// them.
fn flattened<'a>(table: &'a XmlElement, name: &str, wrappers: &[&str]) -> Vec<&'a XmlElement> {
    let mut found = Vec::new();
    for child in &table.children {
        if child.name == name {
            found.push(child);
        } else if wrappers.contains(&child.name.as_str()) {
            found.extend(flattened(child, name, wrappers));
        }
    }
    found
}

/// A column the table declares: how many columns it stands for, the width its style gives and
/// the cell style its cells use when they name none.
struct Columns<'a> {
    count: usize,
    width: Option<f64>,
    cell_style: Option<&'a str>,
}

fn read_columns<'a>(table: &'a XmlElement, styles: &Styles) -> Vec<Columns<'a>> {
    const WRAPPERS: [&str; 3] = [
        "table-columns",
        "table-header-columns",
        "table-column-group",
    ];
    flattened(table, "table-column", &WRAPPERS)
        .into_iter()
        .map(|column| Columns {
            count: repeats(column, "table:number-columns-repeated"),
            width: column
                .attribute("table:style-name")
                .and_then(|name| styles.get(&key("table-column", name)))
                .and_then(|style| style.width),
            cell_style: column.attribute("table:default-cell-style-name"),
        })
        .collect()
}

/// The declared column holding column `index`.
fn column_at<'a, 'b>(columns: &'b [Columns<'a>], index: usize) -> Option<&'b Columns<'a>> {
    let mut first = 0;
    columns.iter().find(|column| {
        first += column.count;
        index < first
    })
}

/// A cell element's text and its type's alignment; a cell covered by a merged one has no text.
/// `None` for an element that is not a cell.
fn read_cell(element: &XmlElement) -> Option<(String, Alignment)> {
    match element.name.as_str() {
        "table-cell" => Some(read_cell_text(element)),
        "covered-table-cell" => Some((String::new(), Alignment::Start)),
        _ => None,
    }
}

/// A row's cells by column, without the empty ones after its last text. A cell uses its own
/// style, else its row's, else its column's.
fn read_row_cells(
    sheet: &str,
    row: &XmlElement,
    columns: &[Columns],
    styles: &Styles,
) -> Result<Vec<Option<Cell>>> {
    let row_style = row.attribute("table:default-cell-style-name");
    let mut cells: Vec<Option<Cell>> = Vec::new();
    let mut column = 0usize;
    for element in &row.children {
        let Some((text, by_type)) = read_cell(element) else {
            continue;
        };
        let count = repeats(element, "table:number-columns-repeated");
        let first = column;
        column = column.saturating_add(count);
        if text.is_empty() {
            continue;
        }
        if column > COLUMN_LIMIT as usize {
            return fail(format!(
                "Sheet \"{sheet}\" has a cell past the last column."
            ));
        }
        let style = element
            .attribute("table:style-name")
            .or(row_style)
            .or_else(|| column_at(columns, first).and_then(|column| column.cell_style));
        let (bold, fixed) = look(styles, style);
        let cell = Cell {
            text,
            alignment: fixed.unwrap_or(by_type),
            bold,
        };
        // Cells are read in column order, so the row so far ends at or before this one.
        cells.resize(first, None);
        cells.extend(std::iter::repeat_n(Some(cell), count));
    }
    Ok(cells)
}

/// The declared widths laid over the widths that fit the text. A column is never narrower than
/// its text needs: every column of a file declares a width, mostly the default one, and a
/// default-width column must not cut its text off.
fn apply_column_widths(widths: &mut [f64], columns: &[Columns]) {
    let mut index = 0;
    for column in columns {
        for width in widths.iter_mut().skip(index).take(column.count) {
            *width = width.max(column.width.unwrap_or(0.0));
        }
        index = index.saturating_add(column.count);
        if index >= widths.len() {
            break;
        }
    }
}

fn read_sheet(name: &str, table: &XmlElement, styles: &Styles, built: &mut usize) -> Result<Sheet> {
    const WRAPPERS: [&str; 3] = ["table-rows", "table-header-rows", "table-row-group"];
    let columns = read_columns(table, styles);
    let mut rows: Vec<Vec<Option<Cell>>> = Vec::new();
    let mut column_count = 0;
    let mut index = 0usize;
    for row in flattened(table, "table-row", &WRAPPERS) {
        let count = repeats(row, "table:number-rows-repeated");
        let cells = read_row_cells(name, row, &columns, styles)?;
        if cells.is_empty() {
            index = index.saturating_add(count);
            continue;
        }
        if index.saturating_add(count) > ROW_LIMIT as usize {
            return fail(format!("Sheet \"{name}\" has a row past the last row."));
        }
        *built = built.saturating_add(cells.len().saturating_mul(count));
        if *built > CELL_LIMIT {
            return fail("The spreadsheet has more cells than this reader shows.");
        }
        column_count = column_count.max(cells.len());
        for _ in 0..count {
            place_at(&mut rows, index, cells.clone());
            index += 1;
        }
    }
    let mut widths = spreadsheet::measure_column_widths(&rows, column_count);
    apply_column_widths(&mut widths, &columns);
    Ok(spreadsheet::sheet(name, rows, column_count, widths))
}

/// Reads the visible sheets of an OpenDocument spreadsheet.
pub fn parse(archive: &ZipArchive) -> Result<Workbook> {
    let content = read_part(archive, CONTENT)?
        .ok_or_else(|| WorkbookError(format!("The spreadsheet has no \"{CONTENT}\" part.")))?;
    let mut styles = Styles::new();
    add_styles(&mut styles, &content);
    // The styles part is only looked up for what cells inherit; a file without one still reads.
    if let Ok(Some(named)) = read_part(archive, STYLES) {
        add_styles(&mut styles, &named);
    }
    let Some(spreadsheet) = content
        .child("body")
        .and_then(|body| body.child("spreadsheet"))
    else {
        return fail("The file is an OpenDocument file, but not a spreadsheet.");
    };
    let mut built = 0;
    let mut sheets = Vec::new();
    for table in spreadsheet.children_named("table") {
        let hidden = table
            .attribute("table:style-name")
            .and_then(|name| styles.get(&key("table", name)))
            .is_some_and(|style| style.hidden);
        if hidden {
            continue;
        }
        sheets.push(read_sheet(
            table.attribute("table:name").unwrap_or(""),
            table,
            &styles,
            &mut built,
        )?);
    }
    if sheets.is_empty() {
        return fail("The spreadsheet has no visible sheet.");
    }
    Ok(Workbook { sheets })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheets::workbook::parse_xml;

    const OPEN: &str = r#"<office:document-content xmlns:office="o" xmlns:table="t" xmlns:text="x" xmlns:style="s" xmlns:fo="f" xmlns:calcext="c">"#;

    /// The styles and sheets of `content.xml` written inside its root element.
    fn read(inside: &str) -> Result<Workbook> {
        let content = parse_xml(&format!("{OPEN}{inside}</office:document-content>"))?;
        let mut styles = Styles::new();
        add_styles(&mut styles, &content);
        let spreadsheet = content
            .child("body")
            .and_then(|body| body.child("spreadsheet"))
            .expect("a spreadsheet");
        let mut built = 0;
        let sheets = spreadsheet
            .children_named("table")
            .filter(|table| {
                !table
                    .attribute("table:style-name")
                    .and_then(|name| styles.get(&key("table", name)))
                    .is_some_and(|style| style.hidden)
            })
            .map(|table| {
                read_sheet(
                    table.attribute("table:name").unwrap_or(""),
                    table,
                    &styles,
                    &mut built,
                )
            })
            .collect::<Result<Vec<Sheet>>>()?;
        Ok(Workbook { sheets })
    }

    fn sheet(rows: &str) -> Sheet {
        let inside = format!(
            r#"<office:body><office:spreadsheet><table:table table:name="One">{rows}</table:table></office:spreadsheet></office:body>"#
        );
        read(&inside).expect("read").sheets.remove(0)
    }

    fn texts(sheet: &Sheet) -> Vec<Vec<&str>> {
        sheet
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.as_ref().map_or("", |cell| cell.text.as_str()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_cell_shows_the_text_saved_for_it_and_aligns_by_its_type() {
        let sheet = sheet(
            r#"<table:table-row>
                 <table:table-cell office:value-type="string"><text:p>Coffee</text:p></table:table-cell>
                 <table:table-cell office:value-type="float" office:value="7"><text:p>$7.00</text:p></table:table-cell>
                 <table:table-cell office:value-type="boolean" office:boolean-value="true"><text:p>TRUE</text:p></table:table-cell>
                 <table:table-cell office:value-type="string" calcext:value-type="error"><text:p>#DIV/0!</text:p></table:table-cell>
               </table:table-row>"#,
        );
        assert_eq!(texts(&sheet), [["Coffee", "$7.00", "TRUE", "#DIV/0!"]]);
        let alignments: Vec<Alignment> = sheet.rows[0]
            .iter()
            .map(|cell| cell.as_ref().expect("a cell").alignment)
            .collect();
        assert_eq!(
            alignments,
            [
                Alignment::Start,
                Alignment::End,
                Alignment::Center,
                Alignment::Center
            ]
        );
        assert_eq!(sheet.name, "One");
        assert_eq!(sheet.column_names, ["A", "B", "C", "D"]);
    }

    #[test]
    fn a_paragraph_keeps_its_order_and_only_the_spaces_that_are_content() {
        let sheet = sheet(
            "<table:table-row><table:table-cell>\
               <office:annotation><text:p>a note</text:p></office:annotation>\
               <text:p>\n  Morning <text:span>Tide</text:span>,\n   <text:a>Huila</text:a><text:s text:c=\"2\"/>340<text:tab/>g<text:line-break/>whole</text:p>\
               <text:p>second</text:p>\
             </table:table-cell></table:table-row>",
        );
        assert_eq!(
            texts(&sheet),
            [["Morning Tide, Huila  340\tg\nwhole\nsecond"]]
        );
    }

    #[test]
    fn a_cell_with_a_value_and_no_text_shows_the_value() {
        let sheet = sheet(
            r#"<table:table-row>
                 <table:table-cell office:value-type="float" office:value="1.5"/>
                 <table:table-cell office:value-type="percentage" office:value="0.25"/>
                 <table:table-cell office:value-type="boolean" office:boolean-value="false"/>
                 <table:table-cell office:value-type="date" office:date-value="2026-11-24T09:30:00"/>
                 <table:table-cell office:value-type="time" office:time-value="PT9H5M7S"/>
                 <table:table-cell office:value-type="string" office:string-value="plain"/>
               </table:table-row>"#,
        );
        assert_eq!(
            texts(&sheet),
            [[
                "1.5",
                "25%",
                "FALSE",
                "2026-11-24 09:30:00",
                "9:05:07",
                "plain"
            ]]
        );
    }

    #[test]
    fn repeats_are_filled_where_there_is_text_and_skipped_where_there_is_none() {
        let sheet = sheet(
            r#"<table:table-row>
                 <table:table-cell table:number-columns-repeated="2"><text:p>x</text:p></table:table-cell>
                 <table:covered-table-cell table:number-columns-repeated="3"/>
                 <table:table-cell><text:p>y</text:p></table:table-cell>
                 <table:table-cell table:number-columns-repeated="16378"/>
               </table:table-row>
               <table:table-row table:number-rows-repeated="2"><table:table-cell table:number-columns-repeated="16384"/></table:table-row>
               <table:table-header-rows><table:table-row table:number-rows-repeated="2"><table:table-cell><text:p>z</text:p></table:table-cell></table:table-row></table:table-header-rows>
               <table:table-row table:number-rows-repeated="1048572"><table:table-cell table:number-columns-repeated="16384"/></table:table-row>"#,
        );
        assert_eq!(
            texts(&sheet),
            [
                vec!["x", "x", "", "", "", "y"],
                vec![],
                vec![],
                vec!["z"],
                vec!["z"]
            ]
        );
        assert_eq!((sheet.column_count, sheet.row_count), (6, 5));
    }

    #[test]
    fn a_cell_takes_bold_and_alignment_from_its_own_style_then_its_rows_then_its_columns() {
        let workbook = read(
            r#"<office:automatic-styles>
                 <style:style style:name="base" style:family="table-cell"><style:text-properties fo:font-weight="bold"/></style:style>
                 <style:style style:name="head" style:family="table-cell" style:parent-style-name="base"><style:paragraph-properties fo:text-align="center"/></style:style>
                 <style:style style:name="plain" style:family="table-cell" style:parent-style-name="base"><style:text-properties fo:font-weight="normal"/><style:table-cell-properties style:text-align-source="value-type"/><style:paragraph-properties fo:text-align="end"/></style:style>
                 <style:style style:name="loop" style:family="table-cell" style:parent-style-name="loop"/>
                 <style:style style:name="wide" style:family="table-column"><style:table-column-properties style:column-width="2in"/></style:style>
                 <style:style style:name="gone" style:family="table"><style:table-properties table:display="false"/></style:style>
               </office:automatic-styles>
               <office:body><office:spreadsheet>
                 <table:table table:name="Hidden" table:style-name="gone"><table:table-row><table:table-cell><text:p>no</text:p></table:table-cell></table:table-row></table:table>
                 <table:table table:name="Shown">
                   <table:table-column table:style-name="wide"/>
                   <table:table-column table:default-cell-style-name="head" table:number-columns-repeated="2"/>
                   <table:table-row>
                     <table:table-cell table:style-name="plain"><text:p>a</text:p></table:table-cell>
                     <table:table-cell><text:p>b</text:p></table:table-cell>
                     <table:table-cell table:style-name="loop"><text:p>c</text:p></table:table-cell>
                   </table:table-row>
                   <table:table-row table:default-cell-style-name="base">
                     <table:table-cell/>
                     <table:table-cell><text:p>d</text:p></table:table-cell>
                   </table:table-row>
                 </table:table>
               </office:spreadsheet></office:body>"#,
        )
        .expect("read");
        assert_eq!(workbook.sheets.len(), 1);
        let sheet = &workbook.sheets[0];
        assert_eq!(sheet.name, "Shown");
        let looks: Vec<Vec<Option<(bool, Alignment)>>> = sheet
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.as_ref().map(|cell| (cell.bold, cell.alignment)))
                    .collect()
            })
            .collect();
        assert_eq!(
            looks,
            [
                vec![
                    Some((false, Alignment::Start)),
                    Some((true, Alignment::Center)),
                    Some((false, Alignment::Start))
                ],
                vec![None, Some((true, Alignment::Start))]
            ]
        );
        assert_eq!(sheet.column_widths[0], 192.0);
        assert_eq!(sheet.column_widths[1], spreadsheet::COLUMN_MIN_WIDTH);
    }

    #[test]
    fn a_file_that_asks_for_too_much_is_refused() {
        let table = |rows: &str| {
            format!(
                r#"<office:body><office:spreadsheet><table:table table:name="One">{rows}</table:table></office:spreadsheet></office:body>"#
            )
        };
        let cell = |columns: usize, rows: usize| {
            table(&format!(
                r#"<table:table-row table:number-rows-repeated="{rows}"><table:table-cell table:number-columns-repeated="{columns}"><text:p>x</text:p></table:table-cell></table:table-row>"#
            ))
        };
        assert!(read(&cell(16_384, 1)).is_ok());
        assert!(read(&cell(16_385, 1)).is_err());
        assert!(read(&cell(1, 1_048_577)).is_err());
        assert!(read(&cell(16_384, 1_000)).is_err());
    }

    #[test]
    fn lengths_are_read_in_every_unit_the_format_allows() {
        assert_eq!(read_length("1in"), Some(96.0));
        assert_eq!(read_length("2.54cm"), Some(96.0));
        assert_eq!(read_length("25.4mm"), Some(96.0));
        assert_eq!(read_length("72pt"), Some(96.0));
        assert_eq!(read_length("6pc"), Some(96.0));
        assert_eq!(read_length("96px"), Some(96.0));
        assert_eq!(read_length("3em"), None);
        assert_eq!(read_length("wide"), None);
    }
}
