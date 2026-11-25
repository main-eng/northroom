//! Reads a PowerPoint 97–2003 presentation (`.ppt`, `.pps`, `.pot`): a compound file whose
//! `PowerPoint Document` stream is a tree of records. The current version of each part is found
//! through the persist directory the `Current User` stream leads to, so a file saved quickly
//! (with its edits appended) reads as last saved.
//!
//! Each slide's shapes are read from its drawing: where each sits, its outline, its fill and line
//! when they are plain colours, and its text, held in the shape or in the slide's text list.
//! Text takes its bold, italic, underline, size and colour from its character runs; where none
//! is given, a title is 40 points and centred and body text 28 points with bullets, as
//! PowerPoint's own masters set them.
//!
//! Left out: masters and their shapes and backgrounds, colours from the colour scheme, pictures,
//! groups' own placement (a shape in a group is placed by its own anchor), and paragraph
//! formatting (indent levels, spacing, alignment beyond the defaults).

use super::{
    read_bytes, Align, Anchor, Body, Colour, Deck, Fit, Form, Item, Paragraph, ReadError, Result,
    Run, Shape, Slide, Spacing, Stroke,
};
use crate::compound::Compound;
use crate::geometry::Rect;
use std::collections::HashMap;

/// Master units, the file's unit of length, in a point.
const UNITS_PER_POINT: f64 = 8.0;
/// A slide's size when the file gives none: 10 by 7.5 inches.
const DEFAULT_SLIDE_POINTS: (f64, f64) = (720.0, 540.0);
const RECORD_HEADER_BYTES: usize = 8;
/// A record's version that marks it as a container of records.
const CONTAINER: u16 = 0x0f;

/// Record types read.
const DOCUMENT: u16 = 0x03e8;
const DOCUMENT_ATOM: u16 = 0x03e9;
const SLIDE: u16 = 0x03ee;
const SLIDE_LIST: u16 = 0x0ff0;
const SLIDE_PERSIST: u16 = 0x03f3;
const TEXT_HEADER: u16 = 0x0f9f;
const TEXT_CHARACTERS: u16 = 0x0fa0;
const TEXT_BYTES: u16 = 0x0fa8;
const TEXT_STYLE: u16 = 0x0fa1;
const TEXT_REFERENCE: u16 = 0x0f9e;
const USER_EDIT: u16 = 0x0ff5;
const PERSIST_DIRECTORY: u16 = 0x1772;
const SHAPE_CONTAINER: u16 = 0xf004;
const SHAPE: u16 = 0xf00a;
const SHAPE_PROPERTIES: u16 = 0xf00b;
const CLIENT_ANCHOR: u16 = 0xf010;
const CHILD_ANCHOR: u16 = 0xf00f;
const CLIENT_TEXT: u16 = 0xf00d;

/// Shape properties read, and the shapes their kinds name.
const FILL_COLOUR: u16 = 0x0181;
const FILL_FLAGS: u16 = 0x01bf;
const LINE_COLOUR: u16 = 0x01c0;
const LINE_WIDTH: u16 = 0x01cb;
const LINE_FLAGS: u16 = 0x01ff;
const ROUNDED_BOX: u16 = 2;
const OVAL: u16 = 3;
const LINE_SHAPE: u16 = 20;
/// English Metric Units in a point, for a line's width.
const EMU_PER_POINT: f64 = 12_700.0;
/// A rounded box's corner radius as a share of its shorter side.
const ROUNDING: f64 = 0.1666;

/// Text's look where its runs say nothing, by what the text is.
const TITLE_POINTS: f64 = 40.0;
const BODY_POINTS: f64 = 28.0;
const OTHER_POINTS: f64 = 18.0;
const BULLET: &str = "•";
/// A body paragraph's hanging bullet: the text's start and the bullet's place before it.
const BULLET_INSET: f64 = 27.0;
const INSETS: [f64; 4] = [7.2, 3.6, 7.2, 3.6];

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

/// One record: its type, instance (a number some types use), whether it holds records, and its
/// data.
#[derive(Clone, Copy)]
struct Record<'a> {
    kind: u16,
    instance: u16,
    container: bool,
    data: &'a [u8],
}

fn record_at(stream: &[u8], at: usize) -> Option<Record<'_>> {
    let header = u16_at(stream, at);
    let length = u32_at(stream, at + 4) as usize;
    let start = at + RECORD_HEADER_BYTES;
    let data = stream.get(start..start.checked_add(length)?)?;
    Some(Record {
        kind: u16_at(stream, at + 2),
        instance: header >> 4,
        container: header & 0x0f == CONTAINER,
        data,
    })
}

/// The records a container's data holds, in order.
fn children(data: &[u8]) -> Vec<Record<'_>> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(record) = record_at(data, at) {
        at += RECORD_HEADER_BYTES + record.data.len();
        found.push(record);
    }
    found
}

fn child(data: &[u8], kind: u16) -> Option<Record<'_>> {
    children(data)
        .into_iter()
        .find(|record| record.kind == kind)
}

// --- Where each part is --------------------------------------------------------------------

/// Each part's place in the stream by its persist id, newest edit winning.
fn read_persist_directory(stream: &[u8], current_user: &[u8]) -> (HashMap<u32, usize>, u32) {
    let mut edits = Vec::new();
    let mut at = u32_at(current_user, 16) as usize;
    while let Some(edit) = record_at(stream, at).filter(|record| record.kind == USER_EDIT) {
        if edits.len() > 1024 {
            break;
        }
        edits.push(edit);
        let previous = u32_at(edit.data, 8) as usize;
        if previous == 0 || previous == at {
            break;
        }
        at = previous;
    }
    let document = edits.first().map_or(0, |edit| u32_at(edit.data, 16));
    let mut places = HashMap::new();
    for edit in edits.iter().rev() {
        let directory = u32_at(edit.data, 12) as usize;
        if let Some(record) =
            record_at(stream, directory).filter(|record| record.kind == PERSIST_DIRECTORY)
        {
            read_directory_entries(record.data, &mut places);
        }
    }
    (places, document)
}

fn read_directory_entries(data: &[u8], places: &mut HashMap<u32, usize>) {
    let mut at = 0;
    while at + 4 <= data.len() {
        let entry = u32_at(data, at);
        let (first, count) = (entry & 0x000f_ffff, (entry >> 20) as usize);
        for index in 0..count {
            places.insert(
                first + index as u32,
                u32_at(data, at + 4 + index * 4) as usize,
            );
        }
        at += 4 + count * 4;
    }
}

// --- Text ----------------------------------------------------------------------------------

/// What a block of text is, from its header: it sets the text's defaults.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Title,
    Body,
    CentredBody,
    Notes,
    Other,
}

impl Role {
    fn of(code: u32) -> Self {
        match code {
            0 | 6 => Role::Title,
            1 | 7 | 8 => Role::Body,
            5 => Role::CentredBody,
            2 => Role::Notes,
            _ => Role::Other,
        }
    }

    fn size(self) -> f64 {
        match self {
            Role::Title => TITLE_POINTS,
            Role::Body | Role::CentredBody => BODY_POINTS,
            Role::Notes | Role::Other => OTHER_POINTS,
        }
    }

    fn align(self) -> Align {
        match self {
            Role::Title | Role::CentredBody => Align::Center,
            Role::Body | Role::Notes | Role::Other => Align::Start,
        }
    }
}

/// A run of characters in one look.
#[derive(Clone, Copy, Default)]
struct Look {
    bold: bool,
    italic: bool,
    underline: bool,
    size: Option<f64>,
    colour: Option<Colour>,
}

/// A block of text: what it is, its characters, and its looks over runs of them.
#[derive(Clone)]
struct Text {
    role: Role,
    units: Vec<u16>,
    looks: Vec<(usize, Look)>,
}

/// Fields of a paragraph run's exception, by mask bit, and their sizes; read only to skip them.
const PARAGRAPH_FIELDS: [(u32, usize); 14] = [
    (0x0000_000f, 2),
    (1 << 7, 2),
    (1 << 4, 2),
    (1 << 6, 2),
    (1 << 5, 4),
    (1 << 11, 2),
    (1 << 12, 2),
    (1 << 13, 2),
    (1 << 14, 2),
    (1 << 8, 2),
    (1 << 10, 2),
    (1 << 15, 2),
    (1 << 16, 2),
    (0x000e_0000, 2),
];
const TAB_STOPS: u32 = 1 << 20;
const TEXT_DIRECTION: u32 = 1 << 21;

/// Skips a paragraph run's fields after its mask, returning where the next run starts.
fn skip_paragraph_fields(data: &[u8], mut at: usize, masks: u32) -> usize {
    for (bits, size) in PARAGRAPH_FIELDS {
        // Tab stops come between the tab size and the font alignment.
        if bits == 1 << 16 && masks & TAB_STOPS != 0 {
            at += 2 + usize::from(u16_at(data, at)) * 4;
        }
        if masks & bits != 0 {
            at += size;
        }
    }
    if masks & TEXT_DIRECTION != 0 {
        at += 2;
    }
    at
}

/// The style of a character run: its fields, in the order the file writes them.
fn read_character_fields(data: &[u8], mut at: usize, masks: u32) -> (Look, usize) {
    let mut look = Look::default();
    if masks & 0x3eb7 != 0 {
        let style = u16_at(data, at);
        look.bold = masks & 1 != 0 && style & 1 != 0;
        look.italic = masks & 2 != 0 && style & 2 != 0;
        look.underline = masks & 4 != 0 && style & 4 != 0;
        at += 2;
    }
    for bit in [16, 21, 22, 23] {
        if masks & (1 << bit) != 0 {
            at += 2;
        }
    }
    if masks & (1 << 17) != 0 {
        look.size = Some(f64::from(u16_at(data, at)));
        at += 2;
    }
    if masks & (1 << 18) != 0 {
        // Red, green, blue, then 0xFE when those are the colour (else a scheme colour's index).
        let colour = data.get(at..at + 4).unwrap_or(&[0; 4]);
        look.colour = (colour[3] == 0xfe).then(|| Colour::opaque(colour[0], colour[1], colour[2]));
        at += 4;
    }
    if masks & (1 << 19) != 0 {
        at += 2;
    }
    (look, at)
}

/// The character looks a text style record gives, as the character each run starts at.
fn read_text_style(data: &[u8], length: usize) -> Vec<(usize, Look)> {
    let total = length + 1;
    let mut at = 0;
    let mut covered = 0;
    while covered < total && at + 10 <= data.len() {
        covered += u32_at(data, at) as usize;
        at = skip_paragraph_fields(data, at + 10, u32_at(data, at + 6));
    }
    let mut looks = Vec::new();
    let mut start = 0;
    while start < total && at + 8 <= data.len() {
        let count = u32_at(data, at) as usize;
        let (look, next) = read_character_fields(data, at + 8, u32_at(data, at + 4));
        looks.push((start, look));
        start += count.max(1);
        at = next;
    }
    looks
}

/// A text atom's characters: single bytes, or UTF-16.
fn units_of(record: &Record) -> Vec<u16> {
    if record.kind == TEXT_BYTES {
        record.data.iter().map(|byte| u16::from(*byte)).collect()
    } else {
        record
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|two| u16::from_le_bytes(*two))
            .collect()
    }
}

/// The text blocks a list of records holds: each header, its characters and its style.
fn read_texts(records: &[Record]) -> Vec<Text> {
    let mut texts: Vec<Text> = Vec::new();
    for record in records {
        if record.kind == TEXT_HEADER {
            texts.push(Text {
                role: Role::of(u32_at(record.data, 0)),
                units: Vec::new(),
                looks: Vec::new(),
            });
            continue;
        }
        let Some(text) = texts.last_mut() else {
            continue;
        };
        match record.kind {
            TEXT_CHARACTERS | TEXT_BYTES => text.units = units_of(record),
            TEXT_STYLE => text.looks = read_text_style(record.data, text.units.len()),
            _ => {}
        }
    }
    texts
}

fn look_at(looks: &[(usize, Look)], index: usize) -> Look {
    let found = looks.partition_point(|(start, _)| *start <= index);
    found
        .checked_sub(1)
        .map_or_else(Look::default, |at| looks[at].1)
}

fn run_of(text: String, look: Look, role: Role) -> Run {
    Run {
        text,
        size: look.size.unwrap_or(role.size()),
        bold: look.bold,
        italic: look.italic,
        underline: look.underline,
        colour: look.colour.unwrap_or(Colour::BLACK),
    }
}

/// A text block's paragraphs: one per carriage return, a vertical tab breaking a line.
fn paragraphs_of(text: &Text) -> Vec<Paragraph> {
    let mut lines: Vec<Vec<(String, Look)>> = vec![Vec::new()];
    for (index, unit) in text.units.iter().enumerate() {
        if *unit == 0x0d {
            lines.push(Vec::new());
            continue;
        }
        let character = match *unit {
            0x0b => '\n',
            unit => char::from_u32(u32::from(unit)).unwrap_or('\u{fffd}'),
        };
        let look = look_at(&text.looks, index);
        let line = lines.last_mut().expect("a line");
        match line.last_mut() {
            Some((words, last)) if same(last, &look) => words.push(character),
            _ => line.push((character.to_string(), look)),
        }
    }
    lines
        .into_iter()
        .map(|line| {
            let runs = line
                .into_iter()
                .map(|(words, look)| run_of(words, look, text.role))
                .collect();
            paragraph_of(runs, text.role)
        })
        .collect()
}

fn same(one: &Look, other: &Look) -> bool {
    (one.bold, one.italic, one.underline, one.size, one.colour)
        == (
            other.bold,
            other.italic,
            other.underline,
            other.size,
            other.colour,
        )
}

fn paragraph_of(runs: Vec<Run>, role: Role) -> Paragraph {
    let written = runs.iter().any(|run| !run.text.trim().is_empty());
    let bulleted = role == Role::Body && written;
    Paragraph {
        align: role.align(),
        left: if bulleted { BULLET_INSET } else { 0.0 },
        indent: if bulleted { -BULLET_INSET } else { 0.0 },
        line: Spacing::Share(1.0),
        before: Spacing::Points(0.0),
        after: Spacing::Points(0.0),
        bullet: bulleted.then(|| BULLET.to_owned()),
        size: runs.iter().map(|run| run.size).fold(role.size(), f64::max),
        runs,
    }
}

// --- Shapes --------------------------------------------------------------------------------

/// A plain colour from a shape property, `None` for a scheme or system colour.
fn property_colour(value: u32) -> Option<Colour> {
    let [red, green, blue, flags] = value.to_le_bytes();
    (flags == 0).then(|| Colour::opaque(red, green, blue))
}

/// A flag a boolean property sets, `None` when the property does not set it.
fn property_flag(value: u32, bit: u32) -> Option<bool> {
    (value & (1 << (bit + 16)) != 0).then(|| value & (1 << bit) != 0)
}

/// The properties of a shape, by id.
fn properties(record: Option<Record>) -> HashMap<u16, u32> {
    let Some(record) = record else {
        return HashMap::new();
    };
    (0..usize::from(record.instance))
        .map(|index| {
            let at = index * 6;
            (
                u16_at(record.data, at) & 0x3fff,
                u32_at(record.data, at + 2),
            )
        })
        .collect()
}

fn read_anchor(records: &[Record]) -> Option<Rect> {
    let anchor = records
        .iter()
        .find(|record| matches!(record.kind, CLIENT_ANCHOR | CHILD_ANCHOR))?;
    let (top, left, right, bottom) = if anchor.data.len() >= 16 {
        let at = |index: usize| f64::from(u32_at(anchor.data, index * 4) as i32);
        (at(1), at(0), at(2), at(3))
    } else {
        let at = |index: usize| f64::from(u16_at(anchor.data, index * 2) as i16);
        (at(0), at(1), at(2), at(3))
    };
    Some(Rect::new(
        left / UNITS_PER_POINT,
        top / UNITS_PER_POINT,
        (right - left).max(0.0) / UNITS_PER_POINT,
        (bottom - top).max(0.0) / UNITS_PER_POINT,
    ))
}

/// A shape's text: what it holds itself, or the slide's text it points to.
fn read_shape_text(records: &[Record], slide_texts: &[Text]) -> Option<Text> {
    let holder = records.iter().find(|record| record.kind == CLIENT_TEXT)?;
    let inside = children(holder.data);
    if let Some(reference) = inside.iter().find(|record| record.kind == TEXT_REFERENCE) {
        return slide_texts.get(u32_at(reference.data, 0) as usize).cloned();
    }
    read_texts(&inside).into_iter().next()
}

fn form_of(kind: u16, frame: Rect) -> Form {
    match kind {
        OVAL => Form::Oval,
        ROUNDED_BOX => Form::Rounded(ROUNDING * frame.width.min(frame.height)),
        LINE_SHAPE => Form::Line { rising: false },
        _ => Form::Box,
    }
}

fn read_shape(container: &[u8], slide_texts: &[Text]) -> Option<Shape> {
    let records = children(container);
    let frame = read_anchor(&records)?;
    let kind = records.iter().find(|record| record.kind == SHAPE)?.instance;
    let properties = properties(
        records
            .iter()
            .copied()
            .find(|record| record.kind == SHAPE_PROPERTIES),
    );
    let filled = properties
        .get(&FILL_FLAGS)
        .and_then(|flags| property_flag(*flags, 4));
    let fill = properties
        .get(&FILL_COLOUR)
        .and_then(|colour| property_colour(*colour))
        .filter(|_| filled != Some(false));
    let lined = properties
        .get(&LINE_FLAGS)
        .and_then(|flags| property_flag(*flags, 3));
    let stroke = properties
        .get(&LINE_COLOUR)
        .and_then(|colour| property_colour(*colour))
        .filter(|_| lined == Some(true))
        .map(|colour| Stroke {
            colour,
            width: properties
                .get(&LINE_WIDTH)
                .map_or(0.75, |width| f64::from(*width) / EMU_PER_POINT),
        });
    let text = read_shape_text(&records, slide_texts)
        .filter(|text| text.role != Role::Notes)
        .map(|text| body_of(&text))
        .filter(|body| {
            body.paragraphs
                .iter()
                .flat_map(|p| &p.runs)
                .any(|run| !run.text.trim().is_empty())
        });
    if fill.is_none() && stroke.is_none() && text.is_none() {
        return None;
    }
    Some(Shape {
        frame,
        form: form_of(kind, frame),
        fill,
        stroke,
        text,
    })
}

fn body_of(text: &Text) -> Body {
    Body {
        insets: INSETS,
        anchor: if text.role == Role::Title {
            Anchor::Middle
        } else {
            Anchor::Top
        },
        wraps: true,
        scale: 1.0,
        spacing_scale: 1.0,
        paragraphs: paragraphs_of(text),
    }
}

/// Every shape container under `data`, through groups and the drawing's own containers.
fn shape_containers<'a>(data: &'a [u8], found: &mut Vec<&'a [u8]>) {
    for record in children(data) {
        if record.kind == SHAPE_CONTAINER {
            found.push(record.data);
        }
        if record.container {
            shape_containers(record.data, found);
        }
    }
}

fn read_slide(stream: &[u8], place: usize, texts: &[Text]) -> Slide {
    let mut containers = Vec::new();
    if let Some(slide) = record_at(stream, place).filter(|record| record.kind == SLIDE) {
        shape_containers(slide.data, &mut containers);
    }
    let items = containers
        .into_iter()
        .filter_map(|container| read_shape(container, texts))
        .map(Item::Shape)
        .collect();
    Slide {
        ground: Colour::WHITE,
        items,
    }
}

/// The slides in order: each slide's persist id and its text list.
fn slide_list(document: &[u8]) -> Vec<(u32, Vec<Text>)> {
    let Some(list) = children(document)
        .into_iter()
        .find(|record| record.kind == SLIDE_LIST && record.instance == 0)
    else {
        return Vec::new();
    };
    let mut slides: Vec<(u32, Vec<Record>)> = Vec::new();
    for record in children(list.data) {
        match (record.kind, slides.last_mut()) {
            (SLIDE_PERSIST, _) => slides.push((u32_at(record.data, 0), Vec::new())),
            (_, Some((_, records))) => records.push(record),
            (_, None) => {}
        }
    }
    slides
        .into_iter()
        .map(|(persist, records)| (persist, read_texts(&records)))
        .collect()
}

/// Reads a PowerPoint 97–2003 presentation held in memory.
pub fn parse(bytes: &[u8]) -> Result<Deck> {
    let compound = Compound::open(bytes).map_err(|error| ReadError::Damaged(error.0))?;
    let stream = |name: &str| -> Result<Vec<u8>> {
        compound
            .stream(name)
            .map_err(|error| ReadError::Damaged(error.0))?
            .ok_or_else(|| ReadError::Missing(name.to_owned()))
    };
    let document_stream = stream("PowerPoint Document")?;
    let current_user = stream("Current User")?;
    let (places, document_id) = read_persist_directory(&document_stream, &current_user);
    let document = places
        .get(&document_id)
        .and_then(|place| record_at(&document_stream, *place))
        .filter(|record| record.kind == DOCUMENT)
        .ok_or(ReadError::NotItsKind)?;
    let (width, height) = child(document.data, DOCUMENT_ATOM)
        .map(|atom| {
            let at =
                |index: usize| f64::from(u32_at(atom.data, index * 4) as i32) / UNITS_PER_POINT;
            (at(0), at(1))
        })
        .filter(|(width, height)| *width > 0.0 && *height > 0.0)
        .unwrap_or(DEFAULT_SLIDE_POINTS);
    let slides = slide_list(document.data)
        .into_iter()
        .filter_map(|(persist, texts)| {
            let place = places.get(&persist)?;
            Some(read_slide(&document_stream, *place, &texts))
        })
        .collect();
    Ok(Deck {
        width,
        height,
        fit: Fit::Whole,
        slides,
        pictures: Vec::new(),
    })
}

/// Reads the PowerPoint 97–2003 presentation at `path`.
pub fn read(path: &std::path::Path) -> Result<Deck> {
    parse(&read_bytes(path)?)
}
