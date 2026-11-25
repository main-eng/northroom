//! Reads an OpenDocument text (`.odt` and its template form) into a `Flow`: a zip whose
//! `content.xml` holds the body and `styles.xml` the named styles and the page.
//!
//! Left out: headers, footers, notes, frames placed on the page rather than in the text (a
//! picture is a block of its own after its paragraph), text boxes, tab stops (a tab is drawn as
//! spaces), and indexes beyond their text.

use super::flow::{self, Block, Cell, Flow, Page, Table};
use super::odf::{
    self, each_paragraph, each_paragraph_in, length_of, InList, Look, Numbering, Styles,
};
use super::{read_bytes, Colour, ReadError, Result, Source};
use crate::package::{read_part, XmlElement, ZipArchive, PART_MAX_BYTES};

const CONTENT: &str = "content.xml";
const STYLES: &str = "styles.xml";

/// The page the document's first master page gives, through its page layout.
fn read_page(styles_part: Option<&XmlElement>) -> Page {
    let layout_name = styles_part
        .and_then(|part| part.child("master-styles"))
        .and_then(|masters| masters.child("master-page"))
        .and_then(|master| master.attribute("style:page-layout-name"));
    let layout = styles_part.and_then(|part| {
        part.children
            .iter()
            .flat_map(|list| list.children_named("page-layout"))
            .find(|layout| layout.attribute("style:name") == layout_name)
    });
    let Some(properties) = layout.and_then(|layout| layout.child("page-layout-properties")) else {
        return flow::LETTER;
    };
    let side = |name: &str, fallback: f64| length_of(properties, name).unwrap_or(fallback);
    let [left, top, right, bottom] = flow::LETTER.margins;
    Page {
        width: side("fo:page-width", flow::LETTER.width),
        height: side("fo:page-height", flow::LETTER.height),
        margins: [
            side("fo:margin-left", left),
            side("fo:margin-top", top),
            side("fo:margin-right", right),
            side("fo:margin-bottom", bottom),
        ],
    }
}

/// Whether the paragraphs read are what an element holds or the element itself (a list).
#[derive(Clone, Copy)]
enum Holder {
    Children,
    Itself,
}

struct Reader<'a> {
    archive: &'a ZipArchive<'a>,
    styles: Styles,
    numbering: Numbering,
    pictures: Vec<Source>,
}

impl Reader<'_> {
    /// A frame's picture as a block at the size the frame gives, or its picture's own.
    fn read_frame(&mut self, frame: &XmlElement) -> Option<Block> {
        let path = frame.child("image")?.attribute("xlink:href")?;
        let bytes = self.archive.read_entry(path, PART_MAX_BYTES).ok()??;
        let natural = flow::picture_points(&bytes);
        let size = (
            length_of(frame, "svg:width").unwrap_or(natural.0),
            length_of(frame, "svg:height").unwrap_or(natural.1),
        );
        let source = Source::Entry(path.to_owned());
        let picture = match self.pictures.iter().position(|known| *known == source) {
            Some(known) => known,
            None => {
                self.pictures.push(source);
                self.pictures.len() - 1
            }
        };
        Some(Block::Picture {
            picture,
            width: size.0,
            height: size.1,
        })
    }

    fn read_paragraph(
        &mut self,
        element: &XmlElement,
        list: Option<&InList>,
        blocks: &mut Vec<Block>,
    ) {
        let read = odf::read_paragraph(
            element,
            &self.styles,
            &Look::default(),
            list,
            &mut self.numbering,
        );
        if read.break_before {
            blocks.push(Block::PageBreak);
        }
        let written = read
            .paragraph
            .runs
            .iter()
            .any(|run| !run.text.trim().is_empty());
        if written || read.frames.is_empty() {
            blocks.push(Block::Paragraph(read.paragraph));
        }
        for frame in read.frames {
            blocks.extend(self.read_frame(frame));
        }
    }

    /// The paragraphs a cell holds, or a list is, read as blocks.
    fn read_paragraphs<'h>(
        &mut self,
        holder: &'h XmlElement,
        whole: Holder,
        blocks: &mut Vec<Block>,
    ) {
        let mut found: Vec<(&'h XmlElement, Option<InList<'h>>)> = Vec::new();
        let mut keep = |element: &'h XmlElement, list: Option<&InList<'h>>| {
            found.push((element, list.cloned()));
        };
        match whole {
            Holder::Children => each_paragraph(holder, None, &mut keep),
            Holder::Itself => each_paragraph_in(holder, None, &mut keep),
        }
        for (element, list) in found {
            self.read_paragraph(element, list.as_ref(), blocks);
        }
    }

    fn read_cell(&mut self, cell: &XmlElement) -> Cell {
        let span = cell
            .attribute("table:number-columns-spanned")
            .and_then(|span| span.parse::<usize>().ok())
            .unwrap_or(1)
            .max(1);
        let fill = self
            .styles
            .get(
                "table-cell",
                cell.attribute("table:style-name").unwrap_or(""),
            )
            .and_then(|style| style.element.child("table-cell-properties"))
            .and_then(|properties| properties.attribute("fo:background-color"))
            .and_then(Colour::hex);
        let mut blocks = Vec::new();
        self.read_paragraphs(cell, Holder::Children, &mut blocks);
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

    /// A row's cells. A cell covered by one spanning columns before it is left out; one covered
    /// by a cell above is an empty cell.
    fn read_row(&mut self, row: &XmlElement) -> Vec<Cell> {
        let mut cells = Vec::new();
        let mut spanned = 0;
        for cell in &row.children {
            match cell.name.as_str() {
                "table-cell" => {
                    let read = self.read_cell(cell);
                    spanned = read.span - 1;
                    cells.push(read);
                }
                "covered-table-cell" if spanned > 0 => spanned -= 1,
                "covered-table-cell" => cells.push(Cell {
                    span: 1,
                    fill: None,
                    paragraphs: Vec::new(),
                }),
                _ => {}
            }
        }
        cells
    }

    fn column_widths(&self, table: &XmlElement) -> Vec<f64> {
        let mut widths = Vec::new();
        let columns = table
            .children
            .iter()
            .flat_map(|child| match child.name.as_str() {
                "table-column" => vec![child],
                "table-columns" | "table-header-columns" => {
                    child.children_named("table-column").collect()
                }
                _ => Vec::new(),
            });
        for column in columns {
            let count = column
                .attribute("table:number-columns-repeated")
                .and_then(|count| count.parse::<usize>().ok())
                .unwrap_or(1)
                .min(1024);
            let width = self
                .styles
                .get(
                    "table-column",
                    column.attribute("table:style-name").unwrap_or(""),
                )
                .and_then(|style| style.element.child("table-column-properties"))
                .and_then(|properties| length_of(properties, "style:column-width"))
                .unwrap_or(0.0);
            widths.extend(std::iter::repeat_n(width, count));
        }
        if widths.iter().all(|width| *width <= 0.0) {
            widths.clear();
        }
        widths
    }

    fn read_table(&mut self, table: &XmlElement) -> Block {
        let columns = self.column_widths(table);
        let rows: Vec<&XmlElement> = table
            .children
            .iter()
            .flat_map(|child| match child.name.as_str() {
                "table-row" => vec![child],
                "table-header-rows" | "table-rows" => child.children_named("table-row").collect(),
                _ => Vec::new(),
            })
            .collect();
        let rows = rows.into_iter().map(|row| self.read_row(row)).collect();
        Block::Table(Table { columns, rows })
    }

    /// The blocks the body or a section holds, in order.
    fn read_blocks(&mut self, holder: &XmlElement, blocks: &mut Vec<Block>) {
        for child in &holder.children {
            match child.name.as_str() {
                "p" | "h" => self.read_paragraph(child, None, blocks),
                "list" => self.read_paragraphs(child, Holder::Itself, blocks),
                "table" => blocks.push(self.read_table(child)),
                "section" | "index-body" | "table-of-content" | "alphabetical-index"
                | "illustration-index" => self.read_blocks(child, blocks),
                "frame" => blocks.extend(self.read_frame(child)),
                _ => {}
            }
        }
    }
}

/// Reads an OpenDocument text held in memory.
pub fn parse(bytes: &[u8]) -> Result<Flow> {
    let archive = ZipArchive::open(bytes)?;
    let content = read_part(&archive, CONTENT)?.ok_or(ReadError::NotItsKind)?;
    let text = content
        .child("body")
        .and_then(|body| body.child("text"))
        .ok_or(ReadError::NotItsKind)?;
    let styles_part = read_part(&archive, STYLES).ok().flatten();
    let mut styles = Styles::default();
    styles.add(&content);
    if let Some(part) = &styles_part {
        styles.add(part);
    }
    let page = read_page(styles_part.as_ref());
    let mut reader = Reader {
        archive: &archive,
        styles,
        numbering: Numbering::default(),
        pictures: Vec::new(),
    };
    let mut blocks = Vec::new();
    reader.read_blocks(text, &mut blocks);
    Ok(Flow {
        page,
        blocks,
        pictures: reader.pictures,
    })
}

/// Reads the OpenDocument text at `path`.
pub fn read(path: &std::path::Path) -> Result<Flow> {
    parse(&read_bytes(path)?)
}
