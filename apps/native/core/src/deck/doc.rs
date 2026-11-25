//! Reads a Word 97–2003 document (`.doc`): a compound file whose `WordDocument` stream holds the
//! text and whose table stream (`0Table` or `1Table`) says where the text's pieces are and how
//! it looks.
//!
//! The text is read through the piece table, so a document saved quickly (with its edits
//! appended) reads in order. A character's look comes from the formatting runs that cover its
//! place in the file, over its paragraph's style; a paragraph's from its own runs over the same
//! style. Styles follow their bases. Fields show their result, not their instructions.
//!
//! Left out: pictures and drawn objects, headers, footers, notes, lists' marks (a list item
//! reads as its text), table layout (a cell is its text and a tab, a row a line), and anything
//! older than Word 97.

use super::flow::{self, Block, Flow};
use super::{read_bytes, Align, Colour, Paragraph, ReadError, Result, Run, Spacing};
use crate::compound::Compound;

/// The first bytes of a Word 97 and later document's information block.
const WORD_IDENTIFIER: u16 = 0xa5ec;
/// Word's unit for lengths on the page: twentieths of a point.
const TWIPS_PER_POINT: f64 = 20.0;
/// Line spacing as a share: 240 is single.
const SINGLE_LINE: f64 = 240.0;
/// Text's size when nothing sets one: Word's own default, 10 points, in half points.
const DEFAULT_HALF_POINTS: u16 = 20;
/// A formatting page's size in the document stream.
const PAGE_BYTES: usize = 512;
/// How a tab, and a table cell's end, are drawn.
const TAB: &str = "    ";
/// A style index that names no style.
const NO_STYLE: u16 = 0x0fff;
/// How far a style's bases are followed; a longer chain is a loop.
const STYLE_DEPTH: usize = 16;

/// Characters with meanings of their own in the text.
const PARAGRAPH_END: u16 = 0x0d;
const CELL_END: u16 = 0x07;
const LINE_BREAK: u16 = 0x0b;
const PAGE_BREAK: u16 = 0x0c;
const FIELD_START: u16 = 0x13;
const FIELD_SEPARATOR: u16 = 0x14;
const FIELD_END: u16 = 0x15;
const TAB_CHARACTER: u16 = 0x09;

/// Formatting instructions read (sprms).
const BOLD: u16 = 0x0835;
const ITALIC: u16 = 0x0836;
const SIZE: u16 = 0x4a43;
const UNDERLINE: u16 = 0x2a3e;
const COLOUR_INDEX: u16 = 0x2a42;
const COLOUR: u16 = 0x6870;
const ALIGN_OLD: u16 = 0x2403;
const ALIGN: u16 = 0x2461;
const SPACE_BEFORE: u16 = 0xa413;
const SPACE_AFTER: u16 = 0xa414;
const LEFT_OLD: u16 = 0x840f;
const LEFT: u16 = 0x845e;
const FIRST_LINE_OLD: u16 = 0x8411;
const FIRST_LINE: u16 = 0x8460;
const LINE_SPACING: u16 = 0x6412;
const TABLE_DEFINITION: u16 = 0xd608;

/// Word's sixteen named colours, by their index (1 onward; 0 is automatic).
const NAMED_COLOURS: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (0, 0, 255),
    (0, 255, 255),
    (0, 255, 0),
    (255, 0, 255),
    (255, 0, 0),
    (255, 255, 0),
    (255, 255, 255),
    (0, 0, 128),
    (0, 128, 128),
    (0, 128, 0),
    (128, 0, 128),
    (128, 0, 0),
    (128, 128, 0),
    (128, 128, 128),
    (192, 192, 192),
];

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    bytes
        .get(at..at + 2)
        .map_or(0, |two| u16::from_le_bytes([two[0], two[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    bytes.get(at..at + 4).map_or(0, |four| {
        u32::from_le_bytes(four.try_into().expect("four bytes"))
    })
}

fn damaged<T>(why: &str) -> Result<T> {
    Err(ReadError::Damaged(why.to_owned()))
}

// --- Looks ---------------------------------------------------------------------------------

/// How text and paragraphs look; `None` where nothing has said.
#[derive(Clone, Debug, Default)]
struct Look {
    half_points: Option<u16>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    colour: Option<Colour>,
    align: Option<Align>,
    left: Option<f64>,
    indent: Option<f64>,
    before: Option<f64>,
    after: Option<f64>,
    line: Option<Spacing>,
}

/// An on-off setting: on, off, as the style says, or the opposite of what the style says.
fn toggle(operand: u8, styled: Option<bool>) -> Option<bool> {
    match operand {
        0 => Some(false),
        1 => Some(true),
        0x80 => styled,
        0x81 => Some(!styled.unwrap_or(false)),
        _ => None,
    }
}

fn twips(operand: &[u8]) -> f64 {
    f64::from(u16_at(operand, 0) as i16) / TWIPS_PER_POINT
}

fn read_colour(operand: &[u8]) -> Option<Colour> {
    // Red, green, blue, then a byte that is 0xFF for automatic.
    (operand.get(3) != Some(&0xff) && operand.len() >= 3)
        .then(|| Colour::opaque(operand[0], operand[1], operand[2]))
}

fn read_line(operand: &[u8]) -> Spacing {
    let line = u16_at(operand, 0) as i16;
    if u16_at(operand, 2) != 0 {
        Spacing::Share(f64::from(line) / SINGLE_LINE)
    } else {
        Spacing::Points(f64::from(line.unsigned_abs()) / TWIPS_PER_POINT)
    }
}

impl Look {
    /// Applies one formatting instruction, `styled` being the look it is over.
    fn apply(&mut self, sprm: u16, operand: &[u8], styled: &Look) {
        let byte = operand.first().copied().unwrap_or(0);
        match sprm {
            BOLD => self.bold = toggle(byte, styled.bold),
            ITALIC => self.italic = toggle(byte, styled.italic),
            SIZE => self.half_points = Some(u16_at(operand, 0).max(1)),
            UNDERLINE => self.underline = Some(byte != 0),
            COLOUR_INDEX => {
                self.colour = NAMED_COLOURS
                    .get(usize::from(byte).wrapping_sub(1))
                    .map(|&(red, green, blue)| Colour::opaque(red, green, blue));
            }
            COLOUR => self.colour = read_colour(operand),
            ALIGN_OLD | ALIGN => {
                self.align = Some(match byte {
                    1 => Align::Center,
                    2 => Align::End,
                    _ => Align::Start,
                });
            }
            SPACE_BEFORE => self.before = Some(twips(operand)),
            SPACE_AFTER => self.after = Some(twips(operand)),
            LEFT_OLD | LEFT => self.left = Some(twips(operand)),
            FIRST_LINE_OLD | FIRST_LINE => self.indent = Some(twips(operand)),
            LINE_SPACING => self.line = Some(read_line(operand)),
            _ => {}
        }
    }

    fn over(self, under: &Look) -> Look {
        Look {
            half_points: self.half_points.or(under.half_points),
            bold: self.bold.or(under.bold),
            italic: self.italic.or(under.italic),
            underline: self.underline.or(under.underline),
            colour: self.colour.or(under.colour),
            align: self.align.or(under.align),
            left: self.left.or(under.left),
            indent: self.indent.or(under.indent),
            before: self.before.or(under.before),
            after: self.after.or(under.after),
            line: self.line.or(under.line),
        }
    }
}

/// How many bytes an instruction's operand takes, from its kind in the instruction's top bits.
fn operand_length(sprm: u16, rest: &[u8]) -> usize {
    match sprm >> 13 {
        0 | 1 => 1,
        2 | 4 | 5 => 2,
        3 => 4,
        7 => 3,
        _ if sprm == TABLE_DEFINITION => usize::from(u16_at(rest, 0)) + 1,
        _ => usize::from(rest.first().copied().unwrap_or(0)) + 1,
    }
}

/// Each instruction in a list of them, with its operand.
fn instructions(list: &[u8]) -> Vec<(u16, &[u8])> {
    let mut found = Vec::new();
    let mut at = 0;
    while at + 2 <= list.len() {
        let sprm = u16_at(list, at);
        let rest = &list[at + 2..];
        let length = operand_length(sprm, rest).min(rest.len());
        let operand = if sprm >> 13 == 6 && sprm != TABLE_DEFINITION {
            rest.get(1..length).unwrap_or(&[])
        } else {
            &rest[..length]
        };
        found.push((sprm, operand));
        at += 2 + length;
    }
    found
}

fn look_of(list: &[u8], styled: &Look) -> Look {
    let mut look = Look::default();
    for (sprm, operand) in instructions(list) {
        look.apply(sprm, operand, styled);
    }
    look
}

// --- Styles --------------------------------------------------------------------------------

/// A style: the style it is based on, and its paragraph and character instructions.
#[derive(Default, Clone)]
struct Style {
    base: u16,
    paragraph: Vec<u8>,
    characters: Vec<u8>,
}

/// The styles, by index.
struct Styles(Vec<Style>);

impl Styles {
    fn read(table: &[u8], at: usize, length: usize) -> Self {
        let Some(sheet) = table.get(at..at + length) else {
            return Styles(Vec::new());
        };
        let header_length = usize::from(u16_at(sheet, 0));
        let count = usize::from(u16_at(sheet, 2));
        let base_length = usize::from(u16_at(sheet, 4));
        let mut styles = Vec::with_capacity(count);
        let mut cursor = 2 + header_length;
        for _ in 0..count {
            let length = usize::from(u16_at(sheet, cursor));
            let entry = sheet.get(cursor + 2..cursor + 2 + length).unwrap_or(&[]);
            styles.push(read_style(entry, base_length));
            cursor += 2 + length;
        }
        Styles(styles)
    }

    /// The look style `index` gives, its bases under it.
    fn look(&self, index: u16) -> Look {
        let mut chain = Vec::new();
        let mut next = index;
        for _ in 0..STYLE_DEPTH {
            let Some(style) = self.0.get(usize::from(next)) else {
                break;
            };
            chain.push(style);
            if style.base == NO_STYLE {
                break;
            }
            next = style.base;
        }
        // Each style's instructions apply over its base's look.
        chain
            .into_iter()
            .rev()
            .fold(Look::default(), |under, style| {
                let paragraph = look_of(&style.paragraph, &under);
                let characters = look_of(&style.characters, &under);
                characters.over(&paragraph.over(&under))
            })
    }
}

fn read_style(entry: &[u8], base_length: usize) -> Style {
    if entry.is_empty() {
        return Style {
            base: NO_STYLE,
            ..Style::default()
        };
    }
    let kind = u16_at(entry, 2) & 0x000f;
    let base = u16_at(entry, 2) >> 4;
    let name_length = usize::from(u16_at(entry, base_length));
    // The name: its length, its characters and a terminating zero, all two bytes each.
    let mut at = base_length + 2 + name_length * 2 + 2;
    let mut parts = Vec::new();
    for _ in 0..2 {
        at += at % 2;
        let length = usize::from(u16_at(entry, at));
        parts.push(entry.get(at + 2..at + 2 + length).unwrap_or(&[]).to_vec());
        at += 2 + length;
    }
    let (paragraph, characters) = match kind {
        // A paragraph style: its paragraph instructions after its own index, then characters'.
        1 => (parts[0].get(2..).unwrap_or(&[]).to_vec(), parts[1].clone()),
        // A character style has only characters'.
        2 => (Vec::new(), parts[0].clone()),
        _ => (Vec::new(), Vec::new()),
    };
    Style {
        base,
        paragraph,
        characters,
    }
}

// --- Formatting runs -----------------------------------------------------------------------

/// A stretch of the document stream, by file offset, with its instructions and, for a
/// paragraph's, its style.
struct Formatted {
    start: u32,
    end: u32,
    style: u16,
    instructions: Vec<u8>,
}

/// The kinds of formatting page: characters' and paragraphs'.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Characters,
    Paragraphs,
}

fn read_page_runs(page: &[u8], kind: Kind, runs: &mut Vec<Formatted>) {
    let count = usize::from(page[PAGE_BYTES - 1]);
    let entry_length = match kind {
        Kind::Characters => 1,
        Kind::Paragraphs => 13,
    };
    let offsets_end = (count + 1) * 4;
    for index in 0..count {
        let (start, end) = (u32_at(page, index * 4), u32_at(page, index * 4 + 4));
        let at = usize::from(page[offsets_end + index * entry_length]) * 2;
        let (style, instructions) = if at == 0 {
            (0, Vec::new())
        } else {
            read_properties(page, at, kind)
        };
        runs.push(Formatted {
            start,
            end,
            style,
            instructions,
        });
    }
}

fn read_properties(page: &[u8], at: usize, kind: Kind) -> (u16, Vec<u8>) {
    let first = usize::from(page.get(at).copied().unwrap_or(0));
    let slice = |from: usize, length: usize| page.get(from..from + length).unwrap_or(&[]).to_vec();
    match kind {
        Kind::Characters => (0, slice(at + 1, first)),
        Kind::Paragraphs => {
            // A length of zero means the real length, in words, is in the next byte.
            let (start, length) = if first == 0 {
                (
                    at + 2,
                    usize::from(page.get(at + 1).copied().unwrap_or(0)) * 2,
                )
            } else {
                (at + 1, first * 2 - 1)
            };
            let list = slice(start, length);
            (u16_at(&list, 0), list.get(2..).unwrap_or(&[]).to_vec())
        }
    }
}

/// The formatting runs a bin table lists, in file order.
fn read_runs(
    document: &[u8],
    table: &[u8],
    at: usize,
    length: usize,
    kind: Kind,
) -> Vec<Formatted> {
    let Some(bins) = table.get(at..at + length) else {
        return Vec::new();
    };
    let count = length.saturating_sub(4) / 8;
    let mut runs = Vec::new();
    for index in 0..count {
        let page_number = (u32_at(bins, (count + 1) * 4 + index * 4) & 0x003f_ffff) as usize;
        let start = page_number * PAGE_BYTES;
        if let Some(page) = document.get(start..start + PAGE_BYTES) {
            read_page_runs(page, kind, &mut runs);
        }
    }
    runs.sort_by_key(|run| run.start);
    runs
}

fn run_at(runs: &[Formatted], offset: u32) -> Option<&Formatted> {
    let index = runs.partition_point(|run| run.end <= offset);
    runs.get(index).filter(|run| run.start <= offset)
}

// --- Text ----------------------------------------------------------------------------------

/// A character of the text and where it is in the document stream.
struct Placed {
    unit: u16,
    offset: u32,
}

/// The piece table the CLX holds, after the property lists stored before it.
fn piece_table(clx: &[u8]) -> Result<&[u8]> {
    let mut at = 0;
    while clx.get(at) == Some(&1) {
        at += 3 + usize::from(u16_at(clx, at + 1));
    }
    if clx.get(at) != Some(&2) {
        return damaged("The document's piece table is missing.");
    }
    Ok(clx
        .get(at + 5..at + 5 + u32_at(clx, at + 1) as usize)
        .unwrap_or(&[]))
}

/// Character `index` of a piece stored at `place`: one byte each when compressed, else two.
fn piece_character(document: &[u8], place: u32, index: u32) -> Placed {
    let offset = place & 0x3fff_ffff;
    if place & 0x4000_0000 != 0 {
        let at = offset / 2 + index;
        let byte = document.get(at as usize).copied().unwrap_or(0);
        Placed {
            unit: byte_unit(byte),
            offset: at,
        }
    } else {
        let at = offset + index * 2;
        Placed {
            unit: u16_at(document, at as usize),
            offset: at,
        }
    }
}

/// The first `length` characters of the text, read through the piece table.
fn read_text(document: &[u8], clx: &[u8], length: usize) -> Result<Vec<Placed>> {
    let table = piece_table(clx)?;
    let pieces = table.len().saturating_sub(4) / 12;
    let mut text = Vec::with_capacity(length);
    for index in 0..pieces {
        let (start, end) = (u32_at(table, index * 4), u32_at(table, index * 4 + 4));
        let place = u32_at(table, (pieces + 1) * 4 + index * 8 + 2);
        let room = length.saturating_sub(text.len());
        let count = (end.saturating_sub(start) as usize).min(room);
        text.extend((0..count as u32).map(|character| piece_character(document, place, character)));
    }
    Ok(text)
}

/// A byte of compressed text: Windows-1252, whose 0x80 to 0x9F differ from Unicode's.
fn byte_unit(byte: u8) -> u16 {
    const HIGH: [u16; 32] = [
        0x20ac, 0x81, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x8d, 0x017d, 0x8f, 0x90, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013,
        0x2014, 0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0x9d, 0x017e, 0x0178,
    ];
    match byte {
        0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
        _ => u16::from(byte),
    }
}

/// What the document is read with.
struct Reader<'a> {
    styles: Styles,
    characters: Vec<Formatted>,
    paragraphs: Vec<Formatted>,
    text: &'a [Placed],
}

impl Reader<'_> {
    /// The paragraph that ends at the mark placed at `offset`: its style's look under its own.
    fn paragraph_look(&self, offset: u32) -> Look {
        let Some(run) = run_at(&self.paragraphs, offset) else {
            return self.styles.look(0);
        };
        let styled = self.styles.look(run.style);
        look_of(&run.instructions, &styled).over(&styled)
    }

    fn run_of(&self, text: String, offset: u32, paragraph: &Look) -> Run {
        let look = run_at(&self.characters, offset)
            .map(|run| look_of(&run.instructions, paragraph).over(paragraph))
            .unwrap_or_else(|| paragraph.clone());
        Run {
            text,
            size: f64::from(look.half_points.unwrap_or(DEFAULT_HALF_POINTS)) / 2.0,
            bold: look.bold.unwrap_or(false),
            italic: look.italic.unwrap_or(false),
            underline: look.underline.unwrap_or(false),
            colour: look.colour.unwrap_or(Colour::BLACK),
        }
    }

    /// The text of a paragraph, from `start` up to its mark at `end`, as runs.
    fn read_runs(&self, start: usize, end: usize, paragraph: &Look) -> (Vec<Run>, bool) {
        let mut runs: Vec<Run> = Vec::new();
        let mut breaks = false;
        let mut field_depth = 0usize;
        let mut shown = Vec::new();
        for placed in &self.text[start..end] {
            match placed.unit {
                FIELD_START => field_depth += 1,
                FIELD_SEPARATOR => shown.push(field_depth),
                FIELD_END => {
                    shown.retain(|depth| *depth != field_depth);
                    field_depth = field_depth.saturating_sub(1);
                }
                // An instruction inside a field before its separator is not shown.
                _ if field_depth > 0 && !shown.contains(&field_depth) => {}
                PAGE_BREAK => breaks = true,
                unit => {
                    let piece = match unit {
                        LINE_BREAK => "\n".to_owned(),
                        TAB_CHARACTER | CELL_END => TAB.to_owned(),
                        // Objects, pictures and their placeholders, and optional hyphens.
                        0x01..=0x08 | 0x1f | 0xfffc => continue,
                        0x1e => "-".to_owned(),
                        _ => String::from_utf16_lossy(&[unit]),
                    };
                    let run = self.run_of(piece, placed.offset, paragraph);
                    match runs.last_mut() {
                        Some(last) if same_look(last, &run) => last.text.push_str(&run.text),
                        _ => runs.push(run),
                    }
                }
            }
        }
        (runs, breaks)
    }

    fn read_blocks(&self) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut start = 0;
        for (index, placed) in self.text.iter().enumerate() {
            if placed.unit != PARAGRAPH_END {
                continue;
            }
            let look = self.paragraph_look(placed.offset);
            let (runs, breaks) = self.read_runs(start, index, &look);
            if breaks {
                blocks.push(Block::PageBreak);
            }
            let size = f64::from(look.half_points.unwrap_or(DEFAULT_HALF_POINTS)) / 2.0;
            blocks.push(Block::Paragraph(Paragraph {
                align: look.align.unwrap_or_default(),
                left: look.left.unwrap_or(0.0),
                indent: look.indent.unwrap_or(0.0),
                line: look.line.unwrap_or(Spacing::Share(1.0)),
                before: Spacing::Points(look.before.unwrap_or(0.0)),
                after: Spacing::Points(look.after.unwrap_or(0.0)),
                bullet: None,
                size: runs.iter().map(|run| run.size).fold(size, f64::max),
                runs,
            }));
            start = index + 1;
        }
        blocks
    }
}

fn same_look(one: &Run, other: &Run) -> bool {
    (one.size, one.bold, one.italic, one.underline, one.colour)
        == (
            other.size,
            other.bold,
            other.italic,
            other.underline,
            other.colour,
        )
}

/// Where a table-stream structure is and how long it is, by its place in the information
/// block's list.
fn located(document: &[u8], list_at: usize, index: usize) -> (usize, usize) {
    let at = list_at + index * 8;
    (
        u32_at(document, at) as usize,
        u32_at(document, at + 4) as usize,
    )
}

/// The places in the information block (FIB) read: the text's length and where the lists of
/// structures start.
struct Information {
    text_length: usize,
    table_stream: &'static str,
    list_at: usize,
}

fn read_information(document: &[u8]) -> Result<Information> {
    if u16_at(document, 0) != WORD_IDENTIFIER {
        return Err(ReadError::NotItsKind);
    }
    let flags = u16_at(document, 0x0a);
    let shorts = usize::from(u16_at(document, 32));
    let longs_at = 34 + shorts * 2;
    let longs = usize::from(u16_at(document, longs_at));
    let text_length = u32_at(document, longs_at + 2 + 3 * 4) as usize;
    let list_at = longs_at + 2 + longs * 4 + 2;
    Ok(Information {
        text_length,
        table_stream: if flags & 0x0200 != 0 {
            "1Table"
        } else {
            "0Table"
        },
        list_at,
    })
}

/// Places in the information block's list of structures.
const STYLES_PLACE: usize = 1;
const CHARACTER_BINS_PLACE: usize = 12;
const PARAGRAPH_BINS_PLACE: usize = 13;
const PIECES_PLACE: usize = 33;

/// Reads a Word 97–2003 document held in memory.
pub fn parse(bytes: &[u8]) -> Result<Flow> {
    let compound = Compound::open(bytes).map_err(|error| ReadError::Damaged(error.0))?;
    let stream = |name: &str| -> Result<Vec<u8>> {
        compound
            .stream(name)
            .map_err(|error| ReadError::Damaged(error.0))?
            .ok_or_else(|| ReadError::Missing(name.to_owned()))
    };
    let document = stream("WordDocument")?;
    let information = read_information(&document)?;
    let table = stream(information.table_stream)?;
    let structure = |place| located(&document, information.list_at, place);
    let (pieces_at, pieces_length) = structure(PIECES_PLACE);
    let clx = table
        .get(pieces_at..pieces_at + pieces_length)
        .unwrap_or(&[]);
    let text = read_text(&document, clx, information.text_length)?;
    let (styles_at, styles_length) = structure(STYLES_PLACE);
    let (characters_at, characters_length) = structure(CHARACTER_BINS_PLACE);
    let (paragraphs_at, paragraphs_length) = structure(PARAGRAPH_BINS_PLACE);
    let reader = Reader {
        styles: Styles::read(&table, styles_at, styles_length),
        characters: read_runs(
            &document,
            &table,
            characters_at,
            characters_length,
            Kind::Characters,
        ),
        paragraphs: read_runs(
            &document,
            &table,
            paragraphs_at,
            paragraphs_length,
            Kind::Paragraphs,
        ),
        text: &text,
    };
    Ok(Flow {
        page: flow::LETTER,
        blocks: reader.read_blocks(),
        pictures: Vec::new(),
    })
}

/// Reads the Word 97–2003 document at `path`.
pub fn read(path: &std::path::Path) -> Result<Flow> {
    parse(&read_bytes(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_read_with_their_operands() {
        // Bold on, 14 points, then a colour, then a variable-length instruction.
        let list = [
            0x35, 0x08, 0x01, 0x43, 0x4a, 28, 0, 0x70, 0x68, 0x1f, 0x6f, 0x4a, 0x00, 0x15, 0xc6, 2,
            9, 9,
        ];
        let read = instructions(&list);
        assert_eq!(read.len(), 4);
        let look = look_of(&list, &Look::default());
        assert_eq!(look.bold, Some(true));
        assert_eq!(look.half_points, Some(28));
        assert_eq!(look.colour, Some(Colour::opaque(0x1f, 0x6f, 0x4a)));
        assert_eq!(read[3], (0xc615, &[9, 9][..]));
        // "Opposite of the style" turns a bold style's text plain.
        assert_eq!(toggle(0x81, Some(true)), Some(false));
        assert_eq!(byte_unit(0x93), 0x201c);
    }
}
