//! Reads rich text (`.rtf`) and a TextEdit package of it (`.rtfd`: a folder holding `TXT.rtf`
//! and the pictures it shows) into a `Flow`.
//!
//! RTF is a stream of groups, control words and text. A group keeps the character and paragraph
//! settings it started with, so leaving it brings them back. Text outside Unicode escapes is read
//! as Windows-1252, as almost every writer declares. Pictures embedded as PNG or JPEG and the
//! package's own pictures are drawn; other embedded pictures (metafiles, bitmaps) are not.
//!
//! Left out: fonts (text is in the app's face), styles from the style sheet, tab stops (a tab is
//! drawn as spaces), table layout (a table's cells are a line each, separated by spaces), headers,
//! footers, notes and fields' instructions (their shown result is kept).

use super::flow::{self, Block, Flow, Page};
use super::{read_bytes, Align, Colour, Paragraph, ReadError, Result, Run, Source, Spacing};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The text part of a rich text package.
pub const PACKAGE_TEXT: &str = "TXT.rtf";
/// RTF's unit for lengths: twentieths of a point.
const TWIPS_PER_POINT: f64 = 20.0;
/// Line spacing as a share: 240 is single.
const SINGLE_LINE: f64 = 240.0;
/// Text's size when the document sets none: RTF's own default, 12 points, in half points.
const DEFAULT_HALF_POINTS: i32 = 24;
/// How a tab is drawn, without tab stops.
const TAB: &str = "    ";
/// The characters an attachment leaves in the text where its picture goes.
const ATTACHMENT_MARKS: [char; 2] = ['\u{fffc}', '\u{ac}'];

/// Windows-1252's characters for bytes 0x80 to 0x9F; the rest of the range is Latin-1.
const CP1252_HIGH: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

/// Whether two runs look the same, whatever their text.
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

fn cp1252(byte: u8) -> char {
    match byte {
        0x80..=0x9f => CP1252_HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// One piece of the stream.
#[derive(Clone, Debug, PartialEq)]
enum Token<'a> {
    Open,
    Close,
    /// A control word and its number, if it has one.
    Word(&'a str, Option<i32>),
    /// A control symbol: a backslash and one character that is not a letter.
    Symbol(u8),
    /// A byte given as `\'hh`.
    Hex(u8),
    /// Text, as bytes in the document's code page.
    Text(&'a [u8]),
}

struct Lexer<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Lexer<'a> {
    fn word(&mut self) -> Token<'a> {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_alphabetic) {
            self.at += 1;
        }
        let name = std::str::from_utf8(&self.bytes[start..self.at]).unwrap_or("");
        let number_start = self.at;
        if self.bytes.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        let number = std::str::from_utf8(&self.bytes[number_start..self.at])
            .ok()
            .and_then(|digits| digits.parse::<i64>().ok())
            .map(|value| value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
        // One space ends a control word and is part of it.
        if self.bytes.get(self.at) == Some(&b' ') {
            self.at += 1;
        }
        Token::Word(name, number)
    }

    fn control(&mut self) -> Option<Token<'a>> {
        let next = *self.bytes.get(self.at)?;
        if next.is_ascii_alphabetic() {
            return Some(self.word());
        }
        self.at += 1;
        if next != b'\'' {
            return Some(Token::Symbol(next));
        }
        let digits = self.bytes.get(self.at..self.at + 2)?;
        self.at += 2;
        let value = std::str::from_utf8(digits)
            .ok()
            .and_then(|digits| u8::from_str_radix(digits, 16).ok())?;
        Some(Token::Hex(value))
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let byte = *self.bytes.get(self.at)?;
        self.at += 1;
        match byte {
            b'{' => Some(Token::Open),
            b'}' => Some(Token::Close),
            b'\\' => self.control().or(Some(Token::Text(&[]))),
            _ => {
                let start = self.at - 1;
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|byte| !matches!(byte, b'{' | b'}' | b'\\'))
                {
                    self.at += 1;
                }
                Some(Token::Text(&self.bytes[start..self.at]))
            }
        }
    }
}

/// What a group's text is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Destination {
    /// The document's text.
    Text,
    /// Something not shown: a font table, style sheet, header, field instruction...
    Skipped,
    ColourTable,
    /// An embedded picture's data.
    Picture,
    /// A package's picture, named by the group's text.
    Attachment,
}

/// The settings a group carries.
#[derive(Clone, Debug)]
struct State {
    destination: Destination,
    bold: bool,
    italic: bool,
    underline: bool,
    half_points: i32,
    colour: usize,
    /// How many characters follow a `\u` escape for readers without Unicode.
    fallback: usize,
    align: Align,
    left: f64,
    indent: f64,
    before: f64,
    after: f64,
    line: Option<Spacing>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            destination: Destination::Text,
            bold: false,
            italic: false,
            underline: false,
            half_points: DEFAULT_HALF_POINTS,
            colour: 0,
            fallback: 1,
            align: Align::Start,
            left: 0.0,
            indent: 0.0,
            before: 0.0,
            after: 0.0,
            line: None,
        }
    }
}

impl State {
    fn reset_paragraph(&mut self) {
        let fresh = State::default();
        self.align = fresh.align;
        self.left = fresh.left;
        self.indent = fresh.indent;
        self.before = fresh.before;
        self.after = fresh.after;
        self.line = fresh.line;
    }

    fn reset_characters(&mut self) {
        let fresh = State::default();
        self.bold = fresh.bold;
        self.italic = fresh.italic;
        self.underline = fresh.underline;
        self.half_points = fresh.half_points;
        self.colour = fresh.colour;
    }
}

/// Control words that start a group whose text is not shown.
const SKIPPED: [&str; 22] = [
    "fonttbl",
    "stylesheet",
    "info",
    "header",
    "headerl",
    "headerr",
    "headerf",
    "footer",
    "footerl",
    "footerr",
    "footerf",
    "footnote",
    "annotation",
    "fldinst",
    "object",
    "listtable",
    "listoverridetable",
    "revtbl",
    "rsidtbl",
    "generator",
    "xmlnstbl",
    "themedata",
];

/// An embedded picture being read: its format and size, and its hexadecimal data.
#[derive(Default)]
struct Embedded {
    extension: Option<&'static str>,
    size: (Option<f64>, Option<f64>),
    hex: Vec<u8>,
}

/// The document being read.
struct Reader {
    folder: Option<PathBuf>,
    stack: Vec<State>,
    state: State,
    /// Set by `\*`: the group is skipped unless its first word is known.
    optional: bool,
    /// Characters still to skip after a `\u` escape.
    skipping: usize,
    colours: Vec<Option<Colour>>,
    building: [Option<u8>; 3],
    runs: Vec<Run>,
    blocks: Vec<Block>,
    pictures: Vec<Source>,
    embedded: Embedded,
    attachment: String,
    drop_mark: bool,
    page: Page,
}

impl Reader {
    fn run_of(&self, text: &str) -> Run {
        Run {
            text: text.to_owned(),
            size: f64::from(self.state.half_points) / 2.0,
            bold: self.state.bold,
            italic: self.state.italic,
            underline: self.state.underline,
            colour: self
                .colours
                .get(self.state.colour)
                .copied()
                .flatten()
                .unwrap_or(Colour::BLACK),
        }
    }

    fn add_text(&mut self, text: &str) {
        let mut kept = String::new();
        for character in text.chars() {
            if self.skipping > 0 {
                self.skipping -= 1;
                continue;
            }
            if self.drop_mark && ATTACHMENT_MARKS.contains(&character) {
                self.drop_mark = false;
                continue;
            }
            match character {
                '\r' | '\n' => {}
                '\u{2028}' => kept.push('\n'),
                _ => kept.push(character),
            }
        }
        self.add_kept(&kept);
    }

    fn add_kept(&mut self, text: &str) {
        match self.state.destination {
            Destination::Text if !text.is_empty() => {
                let run = self.run_of(text);
                // Text that comes in pieces (escapes, words between it) is one run per look.
                match self.runs.last_mut() {
                    Some(last) if same_look(last, &run) => last.text.push_str(text),
                    _ => self.runs.push(run),
                }
            }
            Destination::Attachment => self.attachment.push_str(text),
            Destination::ColourTable => {
                for _ in text.chars().filter(|character| *character == ';') {
                    self.end_colour();
                }
            }
            Destination::Picture => self
                .embedded
                .hex
                .extend(text.bytes().filter(u8::is_ascii_hexdigit)),
            Destination::Text | Destination::Skipped => {}
        }
    }

    fn end_colour(&mut self) {
        let [red, green, blue] = self.building;
        let colour = match (red, green, blue) {
            (None, None, None) => None,
            _ => Some(Colour::opaque(
                red.unwrap_or(0),
                green.unwrap_or(0),
                blue.unwrap_or(0),
            )),
        };
        self.colours.push(colour);
        self.building = [None; 3];
    }

    fn end_paragraph(&mut self) {
        let runs = std::mem::take(&mut self.runs);
        let size = runs
            .iter()
            .map(|run| run.size)
            .fold(f64::from(self.state.half_points) / 2.0, f64::max);
        self.blocks.push(Block::Paragraph(Paragraph {
            align: self.state.align,
            left: self.state.left,
            indent: self.state.indent,
            line: self.state.line.unwrap_or(Spacing::Share(1.0)),
            before: Spacing::Points(self.state.before),
            after: Spacing::Points(self.state.after),
            bullet: None,
            size,
            runs,
        }));
    }

    fn add_picture(&mut self, source: Source, size: (f64, f64)) {
        if !self.runs.is_empty() {
            self.end_paragraph();
        }
        self.pictures.push(source);
        self.blocks.push(Block::Picture {
            picture: self.pictures.len() - 1,
            width: size.0,
            height: size.1,
        });
    }

    fn open(&mut self) {
        self.stack.push(self.state.clone());
        self.optional = false;
    }

    /// Ends a group, finishing the picture or attachment it held.
    fn close(&mut self) {
        let ended = self.state.destination;
        let outer = self.stack.pop();
        let finished_here = outer
            .as_ref()
            .is_some_and(|outer| outer.destination != ended);
        if finished_here {
            match ended {
                Destination::Picture => self.finish_embedded(),
                Destination::Attachment => self.finish_attachment(),
                Destination::Text | Destination::Skipped | Destination::ColourTable => {}
            }
        }
        if let Some(outer) = outer {
            self.state = outer;
        }
    }

    fn finish_embedded(&mut self) {
        let embedded = std::mem::take(&mut self.embedded);
        let Some(extension) = embedded.extension else {
            return;
        };
        let bytes: Vec<u8> = embedded
            .hex
            .as_chunks::<2>()
            .0
            .iter()
            .filter_map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect();
        let natural = flow::picture_points(&bytes);
        let size = (
            embedded.size.0.unwrap_or(natural.0),
            embedded.size.1.unwrap_or(natural.1),
        );
        let source = Source::Bytes {
            bytes: Arc::new(bytes),
            extension,
        };
        self.add_picture(source, size);
    }

    fn finish_attachment(&mut self) {
        let name = std::mem::take(&mut self.attachment);
        self.drop_mark = true;
        let Some(path) = self.folder.as_ref().map(|folder| folder.join(name.trim())) else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let size = flow::picture_points(&bytes);
        self.add_picture(Source::File(path), size);
    }

    /// Settings of a picture's group.
    fn picture_word(&mut self, word: &str, number: Option<i32>) {
        let twips = || number.map(|twips| f64::from(twips) / TWIPS_PER_POINT);
        match word {
            "pngblip" => self.embedded.extension = Some("png"),
            "jpegblip" => self.embedded.extension = Some("jpeg"),
            "picwgoal" => self.embedded.size.0 = twips(),
            "pichgoal" => self.embedded.size.1 = twips(),
            "bin" => self.state.destination = Destination::Skipped,
            _ => {}
        }
    }

    fn colour_word(&mut self, word: &str, number: Option<i32>) {
        let channel = number.map(|value| value.clamp(0, 255) as u8);
        match word {
            "red" => self.building[0] = channel,
            "green" => self.building[1] = channel,
            "blue" => self.building[2] = channel,
            _ => {}
        }
    }

    /// A word that starts a destination, if `word` is one.
    fn destination_word(&mut self, word: &str) -> bool {
        let destination = match word {
            "colortbl" => Destination::ColourTable,
            "pict" => {
                self.embedded = Embedded::default();
                Destination::Picture
            }
            "NeXTGraphic" => {
                self.attachment.clear();
                Destination::Attachment
            }
            _ if SKIPPED.contains(&word) || self.optional => Destination::Skipped,
            _ => return false,
        };
        self.state.destination = destination;
        self.optional = false;
        true
    }

    fn page_word(&mut self, word: &str, points: Option<f64>) -> bool {
        let Some(points) = points else {
            return false;
        };
        match word {
            "paperw" => self.page.width = points,
            "paperh" => self.page.height = points,
            "margl" => self.page.margins[0] = points,
            "margt" => self.page.margins[1] = points,
            "margr" => self.page.margins[2] = points,
            "margb" => self.page.margins[3] = points,
            _ => return false,
        }
        true
    }

    fn paragraph_word(&mut self, word: &str, number: Option<i32>) -> bool {
        let points = number.map(|twips| f64::from(twips) / TWIPS_PER_POINT);
        let state = &mut self.state;
        match word {
            "pard" => state.reset_paragraph(),
            "ql" | "qj" => state.align = Align::Start,
            "qc" => state.align = Align::Center,
            "qr" => state.align = Align::End,
            "li" => state.left = points.unwrap_or(0.0),
            "fi" => state.indent = points.unwrap_or(0.0),
            "sb" => state.before = points.unwrap_or(0.0),
            "sa" => state.after = points.unwrap_or(0.0),
            "sl" => {
                state.line = number
                    .filter(|line| *line != 0)
                    .map(|line| Spacing::Points(f64::from(line.abs()) / TWIPS_PER_POINT));
            }
            "slmult" if number == Some(1) => {
                if let Some(Spacing::Points(points)) = state.line {
                    state.line = Some(Spacing::Share(points * TWIPS_PER_POINT / SINGLE_LINE));
                }
            }
            _ => return false,
        }
        true
    }

    fn character_word(&mut self, word: &str, number: Option<i32>) -> bool {
        let on = number != Some(0);
        let state = &mut self.state;
        match word {
            "plain" => state.reset_characters(),
            "b" => state.bold = on,
            "i" => state.italic = on,
            "ul" => state.underline = on,
            "ulnone" => state.underline = false,
            "fs" => state.half_points = number.unwrap_or(DEFAULT_HALF_POINTS).max(1),
            "cf" => state.colour = number.unwrap_or(0).max(0) as usize,
            "uc" => state.fallback = number.unwrap_or(1).max(0) as usize,
            _ => return false,
        }
        true
    }

    /// A word that adds to the text: breaks, tabs, Unicode characters.
    fn text_word(&mut self, word: &str, number: Option<i32>) {
        match word {
            "par" | "row" => self.end_paragraph(),
            "line" => self.add_kept("\n"),
            "tab" | "cell" => self.add_kept(TAB),
            "page" => {
                if !self.runs.is_empty() {
                    self.end_paragraph();
                }
                self.blocks.push(Block::PageBreak);
            }
            "u" => {
                let code = number.unwrap_or(0);
                let code = if code < 0 { code + 0x1_0000 } else { code };
                let character = char::from_u32(code as u32).unwrap_or('\u{fffd}');
                self.add_text(&character.to_string());
                self.skipping = self.state.fallback;
            }
            "emdash" => self.add_kept("—"),
            "endash" => self.add_kept("–"),
            "bullet" => self.add_kept("•"),
            "lquote" => self.add_kept("‘"),
            "rquote" => self.add_kept("’"),
            "ldblquote" => self.add_kept("“"),
            "rdblquote" => self.add_kept("”"),
            _ => {}
        }
    }

    fn word(&mut self, word: &str, number: Option<i32>) {
        if self.destination_word(word) {
            return;
        }
        match self.state.destination {
            Destination::Picture => return self.picture_word(word, number),
            Destination::ColourTable => return self.colour_word(word, number),
            Destination::Skipped | Destination::Attachment => return,
            Destination::Text => {}
        }
        let points = number.map(|twips| f64::from(twips) / TWIPS_PER_POINT);
        if self.page_word(word, points)
            || self.paragraph_word(word, number)
            || self.character_word(word, number)
        {
            return;
        }
        self.text_word(word, number);
    }

    fn symbol(&mut self, symbol: u8) {
        match symbol {
            b'*' => self.optional = true,
            b'\n' | b'\r' => self.text_word("par", None),
            b'~' => self.add_text("\u{a0}"),
            b'_' => self.add_text("‑"),
            b'-' => {}
            other => self.add_text(&char::from(other).to_string()),
        }
    }

    fn take(&mut self, token: Token) {
        match token {
            Token::Open => self.open(),
            Token::Close => self.close(),
            Token::Word(word, number) => self.word(word, number),
            Token::Symbol(symbol) => self.symbol(symbol),
            Token::Hex(byte) => self.add_text(&cp1252(byte).to_string()),
            Token::Text(bytes) => {
                let text: String = bytes.iter().map(|byte| cp1252(*byte)).collect();
                self.add_text(&text);
            }
        }
    }
}

/// Reads rich text held in memory. `folder` is the package the text is in, whose pictures it
/// names.
pub fn parse(bytes: &[u8], folder: Option<&Path>) -> Result<Flow> {
    if !bytes.starts_with(b"{\\rtf") {
        return Err(ReadError::NotItsKind);
    }
    let mut reader = Reader {
        folder: folder.map(Path::to_path_buf),
        stack: Vec::new(),
        state: State::default(),
        optional: false,
        skipping: 0,
        colours: Vec::new(),
        building: [None; 3],
        runs: Vec::new(),
        blocks: Vec::new(),
        pictures: Vec::new(),
        embedded: Embedded::default(),
        attachment: String::new(),
        drop_mark: false,
        page: flow::LETTER,
    };
    for token in (Lexer { bytes, at: 0 }) {
        reader.take(token);
    }
    if reader.runs.iter().any(|run| !run.text.trim().is_empty()) {
        reader.end_paragraph();
    }
    Ok(Flow {
        page: reader.page,
        blocks: reader.blocks,
        pictures: reader.pictures,
    })
}

/// Reads the rich text at `path`: a file, or a package folder holding `TXT.rtf`.
pub fn read(path: &Path) -> Result<Flow> {
    if path.is_dir() {
        return parse(&read_bytes(&path.join(PACKAGE_TEXT))?, Some(path));
    }
    parse(&read_bytes(path)?, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(flow: &Flow) -> Vec<String> {
        flow.blocks
            .iter()
            .filter_map(|block| match block {
                Block::Paragraph(paragraph) => {
                    Some(paragraph.runs.iter().map(|run| run.text.as_str()).collect())
                }
                Block::Table(_) | Block::Picture { .. } | Block::PageBreak => None,
            })
            .collect()
    }

    #[test]
    fn groups_keep_their_settings_and_escapes_become_characters() {
        let flow = parse(
            br"{\rtf1\ansi{\fonttbl\f0 Times;}{\colortbl;\red31\green111\blue74;}{\*\unknown gone}
\pard\qc\sa120\fs32 Caf\'e9 {\b\cf1 bold} plain\par
\ql\li720\fi-360 Smart \ldblquote quotes\rdblquote\line next\tab end\u8212?\uc0\u8226 x\par
\page after}",
            None,
        )
        .expect("read");
        assert_eq!(
            words(&flow),
            ["Café bold plain", "Smart “quotes”\nnext    end—•x", "after"]
        );
        let Block::Paragraph(first) = &flow.blocks[0] else {
            panic!("a paragraph first");
        };
        assert_eq!(
            (first.align, first.after),
            (Align::Center, Spacing::Points(6.0))
        );
        let bold = &first.runs[1];
        assert_eq!(
            (bold.text.as_str(), bold.bold, bold.colour, bold.size),
            ("bold", true, Colour::opaque(31, 111, 74), 16.0)
        );
        assert!(!first.runs[2].bold);
        let Block::Paragraph(second) = &flow.blocks[1] else {
            panic!("a paragraph second");
        };
        assert_eq!((second.left, second.indent), (36.0, -18.0));
        assert!(matches!(flow.blocks[2], Block::PageBreak));
    }

    #[test]
    fn an_embedded_png_is_a_picture_and_the_page_comes_from_the_document() {
        let flow = parse(
            br"{\rtf1\paperw12240\paperh15840\margl720\margr720\margt720\margb720 Before\par{\pict\pngblip\picwgoal1440\pichgoal720 89504e47}{\pict\wmetafile8 0102}after}",
            None,
        )
        .expect("read");
        assert_eq!(flow.page.margins, [36.0, 36.0, 36.0, 36.0]);
        assert_eq!(flow.pictures.len(), 1);
        let Source::Bytes { bytes, extension } = &flow.pictures[0] else {
            panic!("bytes taken from the file");
        };
        assert_eq!((bytes.as_slice(), *extension), (&b"\x89PNG"[..], "png"));
        assert!(matches!(
            flow.blocks[1],
            Block::Picture {
                picture: 0,
                width: 72.0,
                height: 36.0
            }
        ));
        assert_eq!(words(&flow), ["Before", "after"]);
        assert!(parse(b"plain text", None).is_err());
    }
}
