//! Reads a Word document (`.docx` and its macro and template forms) into a `Flow`: a zip whose
//! `document.xml` holds the body, found through the package's relationships.
//!
//! A paragraph's look comes from the document's defaults, then its style and the styles that
//! style is based on, then its list level, then its own settings; a run's from its paragraph's,
//! then its character style, then its own. Lists are numbered or bulleted as their level says.
//! The last section's page size and margins are every page's.
//!
//! Left out: headers, footers, notes and comments, text boxes, floating placement (a picture is
//! a block of its own where it is anchored), theme colours and fonts, tab stops (a tab is
//! drawn as spaces), and tracked deletions.

use super::flow::{self, Block, Cell, Flow, Page, Table};
use super::{read_bytes, Align, Colour, Paragraph, ReadError, Result, Run, Source, Spacing};
use crate::package::{
    find_relationship, read_part, read_relationships, require_part, XmlElement, ZipArchive,
    PART_MAX_BYTES,
};
use std::collections::HashMap;

/// Twentieths of a point: Word's unit for lengths on the page.
const TWIPS_PER_POINT: f64 = 20.0;
/// English Metric Units in a point: the unit of a drawing's size.
const EMU_PER_POINT: f64 = 12_700.0;
/// Word's measure of line spacing as a share: 240 is single.
const SINGLE_LINE: f64 = 240.0;
/// Text's size when the document sets none: Word's own default, 10 points.
const DEFAULT_SIZE_POINTS: f64 = 10.0;
/// How a tab is drawn, without tab stops.
const TAB: &str = "    ";
/// The mark shown for a bullet set in a symbol face, whose own character the app's face lacks.
const PLAIN_BULLET: &str = "•";
const LEVELS: usize = 9;

/// What one source says about how a paragraph and its text look; `None` where it says nothing
/// and the next source decides.
#[derive(Clone, Debug, Default)]
struct Look {
    size: Option<f64>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    colour: Option<Colour>,
    align: Option<Align>,
    left: Option<f64>,
    indent: Option<f64>,
    line: Option<Spacing>,
    before: Option<Spacing>,
    after: Option<Spacing>,
    list: Option<ListMark>,
    break_before: Option<bool>,
}

/// A paragraph's place in a list: which list and at what level.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ListMark {
    id: String,
    level: usize,
}

impl Look {
    /// This look, with what it leaves unsaid taken from `under`.
    fn over(self, under: &Look) -> Look {
        Look {
            size: self.size.or(under.size),
            bold: self.bold.or(under.bold),
            italic: self.italic.or(under.italic),
            underline: self.underline.or(under.underline),
            colour: self.colour.or(under.colour),
            align: self.align.or(under.align),
            left: self.left.or(under.left),
            indent: self.indent.or(under.indent),
            line: self.line.or(under.line),
            before: self.before.or(under.before),
            after: self.after.or(under.after),
            list: self.list.or_else(|| under.list.clone()),
            break_before: self.break_before.or(under.break_before),
        }
    }
}

fn value<'a>(element: &'a XmlElement, child: &str) -> Option<&'a str> {
    element.child(child)?.attribute("w:val")
}

fn number(element: &XmlElement, attribute: &str) -> Option<f64> {
    element
        .attribute(attribute)?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn twips(element: &XmlElement, attribute: &str) -> Option<f64> {
    number(element, attribute).map(|twips| twips / TWIPS_PER_POINT)
}

/// An on-off setting such as `<w:b/>`: on unless its value says off.
fn toggle(element: &XmlElement, child: &str) -> Option<bool> {
    let set = element.child(child)?;
    Some(!matches!(
        set.attribute("w:val"),
        Some("0" | "false" | "off" | "none")
    ))
}

fn read_run_look(properties: &XmlElement) -> Look {
    Look {
        // Half points.
        size: properties
            .child("sz")
            .and_then(|size| number(size, "w:val"))
            .map(|half| half / 2.0),
        bold: toggle(properties, "b"),
        italic: toggle(properties, "i"),
        underline: value(properties, "u").map(|line| line != "none"),
        colour: value(properties, "color").and_then(Colour::hex),
        ..Look::default()
    }
}

fn read_align(align: &str) -> Option<Align> {
    match align {
        "left" | "start" | "both" | "distribute" => Some(Align::Start),
        "center" => Some(Align::Center),
        "right" | "end" => Some(Align::End),
        _ => None,
    }
}

/// A paragraph's line spacing: a share of single when Word sizes it, else exact points.
fn read_line(spacing: &XmlElement) -> Option<Spacing> {
    let line = number(spacing, "w:line")?;
    Some(match spacing.attribute("w:lineRule") {
        Some("exact" | "atLeast") => Spacing::Points(line / TWIPS_PER_POINT),
        _ => Spacing::Share(line / SINGLE_LINE),
    })
}

/// Indents: the paragraph's left edge, and its first line's from it (negative when hanging).
fn read_indents(look: &mut Look, indents: &XmlElement) {
    look.left = twips(indents, "w:left").or_else(|| twips(indents, "w:start"));
    look.indent = twips(indents, "w:hanging")
        .map(|hanging| -hanging)
        .or_else(|| twips(indents, "w:firstLine"));
}

fn read_list_mark(properties: &XmlElement) -> Option<ListMark> {
    let numbering = properties.child("numPr")?;
    let id = value(numbering, "numId")?.to_owned();
    let level = value(numbering, "ilvl")
        .and_then(|level| level.parse::<usize>().ok())
        .unwrap_or(0)
        .min(LEVELS - 1);
    Some(ListMark { id, level })
}

fn read_paragraph_look(properties: &XmlElement) -> Look {
    let mut look = Look {
        align: value(properties, "jc").and_then(read_align),
        list: read_list_mark(properties),
        break_before: toggle(properties, "pageBreakBefore"),
        ..Look::default()
    };
    if let Some(spacing) = properties.child("spacing") {
        look.line = read_line(spacing);
        look.before = twips(spacing, "w:before").map(Spacing::Points);
        look.after = twips(spacing, "w:after").map(Spacing::Points);
    }
    if let Some(indents) = properties.child("ind") {
        read_indents(&mut look, indents);
    }
    look
}

/// A style's paragraph and run looks together.
fn read_both(holder: &XmlElement) -> Look {
    let runs = holder.child("rPr").map(read_run_look).unwrap_or_default();
    let paragraph = holder
        .child("pPr")
        .map(read_paragraph_look)
        .unwrap_or_default();
    paragraph.over(&runs)
}

// --- Styles --------------------------------------------------------------------------------

struct Style {
    based_on: Option<String>,
    look: Look,
}

/// The document's styles by id, its defaults and its default paragraph style.
#[derive(Default)]
struct Styles {
    by_id: HashMap<String, Style>,
    defaults: Look,
    default_paragraph: Option<String>,
}

/// How far a style's parents are followed; a longer chain is a loop.
const STYLE_DEPTH: usize = 32;

impl Styles {
    fn read(root: Option<&XmlElement>) -> Self {
        let mut styles = Styles::default();
        let Some(root) = root else {
            return styles;
        };
        if let Some(defaults) = root.child("docDefaults") {
            let runs = defaults
                .child("rPrDefault")
                .map(read_both)
                .unwrap_or_default();
            let paragraphs = defaults
                .child("pPrDefault")
                .map(read_both)
                .unwrap_or_default();
            styles.defaults = paragraphs.over(&runs);
        }
        for style in root.children_named("style") {
            let Some(id) = style.attribute("w:styleId") else {
                continue;
            };
            let is_default = matches!(style.attribute("w:default"), Some("1" | "true"));
            if is_default && style.attribute("w:type") == Some("paragraph") {
                styles.default_paragraph = Some(id.to_owned());
            }
            let based_on = value(style, "basedOn").map(str::to_owned);
            let look = read_both(style);
            styles.by_id.insert(id.to_owned(), Style { based_on, look });
        }
        styles
    }

    /// A style's look with its parents' under it.
    fn look(&self, id: Option<&str>) -> Look {
        let mut look = Look::default();
        let mut next = id.map(str::to_owned);
        for _ in 0..STYLE_DEPTH {
            let Some(style) = next.as_ref().and_then(|id| self.by_id.get(id)) else {
                break;
            };
            look = look.over(&style.look);
            next = style.based_on.clone();
        }
        look
    }
}

// --- Lists ---------------------------------------------------------------------------------

/// One list level: how its mark is written and the indents it gives.
#[derive(Clone, Debug, Default)]
struct Level {
    format: String,
    text: String,
    look: Look,
}

/// The document's lists: each list's levels, by the id paragraphs name.
#[derive(Default)]
struct Lists {
    levels: HashMap<String, Vec<Level>>,
    counts: HashMap<String, [usize; LEVELS]>,
}

fn read_levels(abstract_list: &XmlElement) -> Vec<Level> {
    abstract_list
        .children_named("lvl")
        .take(LEVELS)
        .map(|level| Level {
            format: value(level, "numFmt").unwrap_or("decimal").to_owned(),
            text: value(level, "lvlText").unwrap_or("").to_owned(),
            look: level
                .child("pPr")
                .map(read_paragraph_look)
                .unwrap_or_default(),
        })
        .collect()
}

impl Lists {
    fn read(root: Option<&XmlElement>) -> Self {
        let mut lists = Lists::default();
        let Some(root) = root else {
            return lists;
        };
        let abstracts: HashMap<&str, Vec<Level>> = root
            .children_named("abstractNum")
            .filter_map(|list| Some((list.attribute("w:abstractNumId")?, read_levels(list))))
            .collect();
        for list in root.children_named("num") {
            let (Some(id), Some(of)) = (list.attribute("w:numId"), value(list, "abstractNumId"))
            else {
                continue;
            };
            if let Some(levels) = abstracts.get(of) {
                lists.levels.insert(id.to_owned(), levels.clone());
            }
        }
        lists
    }

    fn level(&self, mark: &ListMark) -> Option<&Level> {
        self.levels.get(&mark.id)?.get(mark.level)
    }

    /// The mark for the next paragraph at `mark`'s level, counting it.
    fn next_mark(&mut self, mark: &ListMark) -> Option<String> {
        let level = self.level(mark)?.clone();
        let counts = self.counts.entry(mark.id.clone()).or_default();
        counts[mark.level] += 1;
        for deeper in &mut counts[mark.level + 1..] {
            *deeper = 0;
        }
        if level.format == "bullet" {
            return Some(bullet_of(&level.text));
        }
        if level.format == "none" {
            return None;
        }
        let formats: Vec<String> = self.levels[&mark.id]
            .iter()
            .map(|level| level.format.clone())
            .collect();
        Some(numbered(&level.text, counts, &formats))
    }
}

fn bullet_of(text: &str) -> String {
    let private = text
        .chars()
        .any(|character| ('\u{e000}'..='\u{f8ff}').contains(&character));
    if text.trim().is_empty() || private {
        PLAIN_BULLET.to_owned()
    } else {
        text.to_owned()
    }
}

/// `%1.%2.`-style level text with each level's count written in its format.
fn numbered(text: &str, counts: &[usize; LEVELS], formats: &[String]) -> String {
    let mut written = text.to_owned();
    for level in (0..LEVELS).rev() {
        let placeholder = format!("%{}", level + 1);
        if written.contains(&placeholder) {
            let format = formats.get(level).map_or("decimal", String::as_str);
            written = written.replace(&placeholder, &count_in(counts[level].max(1), format));
        }
    }
    written
}

fn letters(count: usize) -> String {
    let letter = char::from(b'a' + ((count - 1) % 26) as u8);
    letter.to_string().repeat((count - 1) / 26 + 1)
}

fn roman(mut count: usize) -> String {
    const NUMERALS: [(usize, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut written = String::new();
    for (worth, numeral) in NUMERALS {
        while count >= worth {
            written.push_str(numeral);
            count -= worth;
        }
    }
    written
}

fn count_in(count: usize, format: &str) -> String {
    match format {
        "lowerLetter" => letters(count),
        "upperLetter" => letters(count).to_uppercase(),
        "lowerRoman" => roman(count),
        "upperRoman" => roman(count).to_uppercase(),
        _ => count.to_string(),
    }
}

// --- Body ----------------------------------------------------------------------------------

/// What the body is read with, and where its pictures go.
struct Reader<'a> {
    archive: &'a ZipArchive<'a>,
    relations: Vec<(String, String, String)>,
    styles: Styles,
    lists: Lists,
    pictures: Vec<Source>,
}

/// A paragraph being read: its runs, and the pictures and page breaks found among them.
#[derive(Default)]
struct Gathered {
    runs: Vec<Run>,
    after: Vec<Block>,
}

fn run_of(text: &str, look: &Look) -> Run {
    Run {
        text: text.to_owned(),
        size: look.size.unwrap_or(DEFAULT_SIZE_POINTS),
        bold: look.bold.unwrap_or(false),
        italic: look.italic.unwrap_or(false),
        underline: look.underline.unwrap_or(false),
        colour: look.colour.unwrap_or(Colour::BLACK),
    }
}

/// The first element named `name` at any depth under `element`.
fn find<'a>(element: &'a XmlElement, name: &str) -> Option<&'a XmlElement> {
    element.children.iter().find_map(|child| {
        if child.name == name {
            Some(child)
        } else {
            find(child, name)
        }
    })
}

impl Reader<'_> {
    /// The picture a relationship id names, as an index into the flow's pictures, and its size
    /// read from its header.
    fn picture(&mut self, id: &str) -> Option<(usize, (f64, f64))> {
        let path = self
            .relations
            .iter()
            .find(|(known, _, _)| known == id)
            .map(|(_, _, path)| path.clone())?;
        let bytes = self.archive.read_entry(&path, PART_MAX_BYTES).ok()??;
        let size = flow::picture_points(&bytes);
        let source = Source::Entry(path);
        let index = match self.pictures.iter().position(|known| *known == source) {
            Some(known) => known,
            None => {
                self.pictures.push(source);
                self.pictures.len() - 1
            }
        };
        Some((index, size))
    }

    /// A drawing or an old-style picture, as a block at the size the document gives it.
    fn read_drawing(&mut self, drawing: &XmlElement) -> Option<Block> {
        let id = find(drawing, "blip")
            .and_then(|blip| blip.attribute("r:embed"))
            .or_else(|| find(drawing, "imagedata").and_then(|data| data.attribute("r:id")))?;
        let (picture, natural) = self.picture(id)?;
        let extent = find(drawing, "extent").and_then(|extent| {
            Some((
                number(extent, "cx")? / EMU_PER_POINT,
                number(extent, "cy")? / EMU_PER_POINT,
            ))
        });
        let (width, height) = extent
            .filter(|(w, h)| *w > 0.0 && *h > 0.0)
            .unwrap_or(natural);
        Some(Block::Picture {
            picture,
            width,
            height,
        })
    }

    /// One piece of a run, in order: text, a tab, a break, a picture.
    fn read_run_part(&mut self, part: &XmlElement, look: &Look, gathered: &mut Gathered) {
        match part.name.as_str() {
            "t" => gathered.runs.push(run_of(&part.text, look)),
            "tab" => gathered.runs.push(run_of(TAB, look)),
            "br" | "cr" if part.attribute("w:type") == Some("page") => {
                gathered.after.push(Block::PageBreak);
            }
            "br" | "cr" => gathered.runs.push(run_of("\n", look)),
            "drawing" | "pict" | "object" => {
                if let Some(picture) = self.read_drawing(part) {
                    gathered.after.push(picture);
                }
            }
            _ => {}
        }
    }

    fn read_run(&mut self, run: &XmlElement, paragraph: &Look, gathered: &mut Gathered) {
        let properties = run.child("rPr");
        let style = properties.and_then(|properties| value(properties, "rStyle"));
        let own = properties.map(read_run_look).unwrap_or_default();
        let look = own.over(&self.styles.look(style)).over(paragraph);
        for part in &run.children {
            self.read_run_part(part, &look, gathered);
        }
    }

    /// The runs of a paragraph or of what holds runs inside it: links, insertions, fields and
    /// content controls. Deleted text is left out.
    fn gather(&mut self, holder: &XmlElement, look: &Look, gathered: &mut Gathered) {
        for child in &holder.children {
            match child.name.as_str() {
                "r" => self.read_run(child, look, gathered),
                "hyperlink" | "ins" | "smartTag" | "fldSimple" | "sdtContent" | "customXml"
                | "sdt" => self.gather(child, look, gathered),
                _ => {}
            }
        }
    }

    /// A paragraph as blocks: a page break when it asks for one, itself, then what it holds
    /// that is drawn apart from its text. A paragraph that holds only a picture is the picture.
    fn read_paragraph(&mut self, paragraph: &XmlElement, blocks: &mut Vec<Block>) {
        let properties = paragraph.child("pPr");
        let style = properties
            .and_then(|properties| value(properties, "pStyle"))
            .or(self.styles.default_paragraph.as_deref());
        let own = properties.map(read_paragraph_look).unwrap_or_default();
        let styled = self.styles.look(style).over(&self.styles.defaults);
        let list = own.list.clone().or_else(|| styled.list.clone());
        let level = list
            .as_ref()
            .and_then(|mark| self.lists.level(mark))
            .map(|level| level.look.clone())
            .unwrap_or_default();
        let look = own.over(&level.over(&styled));
        if look.break_before == Some(true) {
            blocks.push(Block::PageBreak);
        }
        let mut gathered = Gathered::default();
        self.gather(paragraph, &look, &mut gathered);
        let written = gathered.runs.iter().any(|run| !run.text.trim().is_empty());
        if written || gathered.after.is_empty() {
            let bullet = list
                .filter(|_| written)
                .and_then(|mark| self.lists.next_mark(&mark));
            blocks.push(Block::Paragraph(paragraph_of(&look, bullet, gathered.runs)));
        }
        blocks.extend(gathered.after);
    }

    fn read_cell(&mut self, cell: &XmlElement) -> Cell {
        let properties = cell.child("tcPr");
        let span = properties
            .and_then(|properties| value(properties, "gridSpan"))
            .and_then(|span| span.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        let continued = properties
            .and_then(|properties| properties.child("vMerge"))
            .is_some_and(|merge| merge.attribute("w:val") != Some("restart"));
        let fill = properties
            .and_then(|properties| properties.child("shd"))
            .and_then(|shading| shading.attribute("w:fill"))
            .and_then(Colour::hex);
        let mut blocks = Vec::new();
        if !continued {
            self.read_blocks(cell, &mut blocks);
        }
        let paragraphs = blocks
            .into_iter()
            .filter_map(|block| match block {
                Block::Paragraph(paragraph) => Some(paragraph),
                Block::Table(_) | Block::Picture { .. } | Block::PageBreak => None,
            })
            .collect();
        Cell {
            span,
            fill,
            paragraphs,
        }
    }

    fn read_table(&mut self, table: &XmlElement) -> Block {
        let columns = table
            .child("tblGrid")
            .into_iter()
            .flat_map(|grid| grid.children_named("gridCol"))
            .map(|column| twips(column, "w:w").unwrap_or(0.0))
            .collect();
        let rows = table
            .children_named("tr")
            .map(|row| {
                row.children_named("tc")
                    .map(|cell| self.read_cell(cell))
                    .collect()
            })
            .collect();
        Block::Table(Table { columns, rows })
    }

    /// The blocks a body, a cell or a content control holds, in order.
    fn read_blocks(&mut self, holder: &XmlElement, blocks: &mut Vec<Block>) {
        for child in &holder.children {
            match child.name.as_str() {
                "p" => self.read_paragraph(child, blocks),
                "tbl" => blocks.push(self.read_table(child)),
                "sdt" | "sdtContent" | "customXml" => self.read_blocks(child, blocks),
                _ => {}
            }
        }
    }
}

/// A paragraph in `look`. An empty line is as tall as its largest run says, as the mark
/// that ends it is.
fn paragraph_of(look: &Look, bullet: Option<String>, runs: Vec<Run>) -> Paragraph {
    let size = runs
        .iter()
        .map(|run| run.size)
        .fold(None, |largest: Option<f64>, size| {
            Some(largest.map_or(size, |l| l.max(size)))
        });
    Paragraph {
        align: look.align.unwrap_or_default(),
        left: look.left.unwrap_or(0.0),
        indent: look.indent.unwrap_or(0.0),
        line: look.line.unwrap_or(Spacing::Share(1.0)),
        before: look.before.unwrap_or(Spacing::Points(0.0)),
        after: look.after.unwrap_or(Spacing::Points(0.0)),
        bullet,
        size: size.or(look.size).unwrap_or(DEFAULT_SIZE_POINTS),
        runs,
    }
}

/// The page the body's last section gives.
fn read_page(body: &XmlElement) -> Page {
    let section = body.child("sectPr");
    let size = section.and_then(|section| section.child("pgSz"));
    let margins = section.and_then(|section| section.child("pgMar"));
    let side = |name: &str, fallback: f64| {
        margins
            .and_then(|margins| twips(margins, name))
            .unwrap_or(fallback)
    };
    let [left, top, right, bottom] = flow::LETTER.margins;
    Page {
        width: size
            .and_then(|size| twips(size, "w:w"))
            .unwrap_or(flow::LETTER.width),
        height: size
            .and_then(|size| twips(size, "w:h"))
            .unwrap_or(flow::LETTER.height),
        margins: [
            side("w:left", left),
            side("w:top", top),
            side("w:right", right),
            side("w:bottom", bottom),
        ],
    }
}

/// Reads a Word document held in memory.
pub fn parse(bytes: &[u8]) -> Result<Flow> {
    let archive = ZipArchive::open(bytes)?;
    let package = read_relationships(&archive, "")?;
    let path = find_relationship(&package, "/officeDocument").ok_or(ReadError::NotItsKind)?;
    let document = require_part(&archive, path)?;
    let body = document
        .child("body")
        .filter(|_| document.name == "document")
        .ok_or(ReadError::NotItsKind)?;
    let relations = read_relationships(&archive, path)?;
    let part = |suffix: &str| -> Result<Option<XmlElement>> {
        match find_relationship(&relations, suffix) {
            Some(path) => Ok(read_part(&archive, path)?),
            None => Ok(None),
        }
    };
    let styles = Styles::read(part("/styles")?.as_ref());
    let lists = Lists::read(part("/numbering")?.as_ref());
    let mut reader = Reader {
        archive: &archive,
        relations,
        styles,
        lists,
        pictures: Vec::new(),
    };
    let mut blocks = Vec::new();
    reader.read_blocks(body, &mut blocks);
    Ok(Flow {
        page: read_page(body),
        blocks,
        pictures: reader.pictures,
    })
}

/// Reads the Word document at `path`.
pub fn read(path: &std::path::Path) -> Result<Flow> {
    parse(&read_bytes(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_marks_count_in_their_format_and_restart_below_their_level() {
        let formats: Vec<String> = ["decimal", "lowerLetter", "upperRoman"]
            .map(str::to_owned)
            .into();
        let mut counts = [0; LEVELS];
        counts[0] = 3;
        counts[1] = 28;
        counts[2] = 14;
        assert_eq!(numbered("%1.%2)", &counts, &formats), "3.bb)");
        assert_eq!(numbered("(%3)", &counts, &formats), "(XIV)");
        assert_eq!(bullet_of("\u{f0b7}"), "•");
        assert_eq!(bullet_of("–"), "–");
        assert_eq!(roman(1994), "mcmxciv");
    }
}
