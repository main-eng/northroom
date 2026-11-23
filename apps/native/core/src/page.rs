//! A page: blocks of text, held as blocks rather than Markdown. Markdown is only the import and
//! export format (a `.md` file, a paste), converted at the boundary. It is parsed with
//! `pulldown-cmark`, and recorded pages pin the parser's output (`tests/page_media_parity.rs`).
//!
//! A block's `text` keeps its inline marks written out (`**bold**`, `*italic*`, `~~strike~~`,
//! `` `code` ``, `[label](url)`), exactly as typed.

pub mod edit;
pub mod editor;
pub mod host;
pub mod inline;
pub mod inline_edit;
pub mod keys;
pub mod layout;
pub mod media;
pub mod players;
pub mod watch;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// The deepest a list item can nest.
pub const MAX_LIST_INDENT: u8 = 4;
/// The URL scheme of a board media asset: `![caption](asset:<asset id>)`.
const MEDIA_ASSET_SCHEME: &str = "asset:";
/// The fence language that marks an HTML block; ordinary fenced HTML stays an inert code example.
const HTML_ARTIFACT_LANGUAGE: &str = "northroom-html";
const LIST_INDENT: &str = "  ";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BlockKind {
    Paragraph {
        text: String,
    },
    Heading {
        level: u8,
        text: String,
    },
    Bullet {
        indent: u8,
        text: String,
    },
    Numbered {
        indent: u8,
        text: String,
    },
    Todo {
        indent: u8,
        checked: bool,
        text: String,
    },
    Quote {
        text: String,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Html {
        text: String,
    },
    Divider,
    /// The first row is the header row; every row has the same number of cells.
    Table {
        rows: Vec<Vec<String>>,
        #[serde(rename = "columnWidthsPx")]
        column_widths_px: Option<Vec<f64>>,
    },
    /// An image, video or audio clip from the board, by asset id.
    Media {
        #[serde(rename = "assetId")]
        asset_id: String,
        caption: String,
    },
    /// A block that is only a `<video>` or `<audio>` element (`page::media::player_tag`). It plays
    /// in the page and is written back verbatim.
    Player {
        html: String,
    },
}

/// One block. Its id is stable for the block's lifetime, so an edit can refer to it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    #[serde(flatten)]
    pub kind: BlockKind,
}

type Events<'a> = [(Event<'a>, Range<usize>)];

fn is_block(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::Item
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::MetadataBlock(_)
    )
}

/// The index of the `End` that closes the `Start` at `start`.
fn end_of(events: &Events<'_>, start: usize) -> usize {
    let mut depth = 0_usize;
    for (index, (event, _)) in events.iter().enumerate().skip(start) {
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
    }
    events.len() - 1
}

/// The source between the first and last event strictly inside `start..end`.
fn inner_source<'a>(source: &'a str, events: &Events<'_>, start: usize, end: usize) -> &'a str {
    if end <= start + 1 {
        return "";
    }
    &source[events[start + 1].1.start..events[end - 1].1.end]
}

/// Strips the container's indentation from continuation lines of a list item or quote.
fn dedent_lines(text: &str) -> String {
    text.trim()
        .split('\n')
        .enumerate()
        .map(|(index, line)| if index == 0 { line } else { line.trim_start() })
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_quote_markers(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let unindented = line.trim_start_matches(' ');
            match unindented.strip_prefix('>') {
                Some(rest) => rest.strip_prefix([' ', '\t']).unwrap_or(rest),
                None => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// The raw text between the brackets of `![alt](address)`, marks included, with only escaped
/// brackets unescaped.
fn raw_alt(image: &str) -> String {
    let inner = image.strip_prefix("![").unwrap_or(image);
    let mut alt = String::new();
    let mut depth = 0_usize;
    let mut characters = inner.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some(bracket @ ('[' | ']')) => alt.push(bracket),
                Some(other) => {
                    alt.push('\\');
                    alt.push(other);
                }
                None => alt.push('\\'),
            },
            '[' => {
                depth += 1;
                alt.push('[');
            }
            ']' if depth == 0 => break,
            ']' => {
                depth -= 1;
                alt.push(']');
            }
            other => alt.push(other),
        }
    }
    alt
}

/// A player when `raw` is a single `<video>` or `<audio>` element, else the raw text.
fn player_or_words(raw: &str) -> BlockKind {
    if crate::page::media::player_tag(raw).is_some() {
        BlockKind::Player {
            html: raw.to_owned(),
        }
    } else {
        BlockKind::Paragraph {
            text: raw.to_owned(),
        }
    }
}

fn heading_level(level: pulldown_cmark::HeadingLevel) -> u8 {
    match level {
        pulldown_cmark::HeadingLevel::H1 => 1,
        pulldown_cmark::HeadingLevel::H2 => 2,
        _ => 3,
    }
}

/// The `(Start(Item), End)` index pairs of the list from `start` to `end`.
fn list_items(events: &Events<'_>, start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut items = Vec::new();
    let mut index = start + 1;
    while index < end {
        if let (Event::Start(Tag::Item), _) = &events[index] {
            let item_end = end_of(events, index);
            items.push((index, item_end));
            index = item_end + 1;
        } else {
            index += 1;
        }
    }
    items
}

/// The blocks one top-level construct produced, and its source range.
struct SpanAt {
    range: Range<usize>,
    blocks: Vec<Block>,
}

struct Importer<'a, 'ids> {
    source: &'a str,
    create_id: &'ids mut dyn FnMut() -> String,
    blocks: Vec<Block>,
    spans: Vec<SpanAt>,
}

impl Importer<'_, '_> {
    fn push(&mut self, kind: BlockKind) {
        let id = (self.create_id)();
        self.blocks.push(Block { id, kind });
    }

    /// A paragraph that is only `![caption](asset:<id>)` is a board media block.
    fn media_of(&self, events: &Events<'_>, start: usize, end: usize) -> Option<BlockKind> {
        let (Event::Start(Tag::Image { dest_url, .. }), image) = events.get(start + 1)? else {
            return None;
        };
        if end_of(events, start + 1) != end - 1 {
            return None;
        }
        let asset_id = dest_url.strip_prefix(MEDIA_ASSET_SCHEME)?.trim();
        if asset_id.is_empty() {
            return None;
        }
        // Spaces before or after the image on its line count as text, so the line stays a
        // paragraph.
        let line_start = self.source[..image.start]
            .rfind('\n')
            .map_or(0, |at| at + 1);
        let line_end = self.source[image.end..]
            .find('\n')
            .map_or(self.source.len(), |at| image.end + at);
        if line_start != image.start || line_end != image.end {
            return None;
        }
        Some(BlockKind::Media {
            asset_id: asset_id.to_owned(),
            caption: raw_alt(&self.source[image.clone()]),
        })
    }

    /// Appends the block that starts at `start` and returns the index after it.
    fn append_block(&mut self, events: &Events<'_>, start: usize) -> usize {
        let (event, range) = &events[start];
        let Event::Start(tag) = event else {
            if matches!(event, Event::Rule) {
                self.push(BlockKind::Divider);
            }
            return start + 1;
        };
        let end = end_of(events, start);
        match tag {
            Tag::Paragraph => {
                // CommonMark parses `<video src="a.mp4" controls></video>` on one line as a
                // paragraph of inline HTML, since the open tag is not alone on the line.
                let kind = self
                    .media_of(events, start, end)
                    .unwrap_or_else(|| player_or_words(self.source[range.clone()].trim_end()));
                self.push(kind);
            }
            Tag::Heading { level, .. } => self.push(BlockKind::Heading {
                level: heading_level(*level),
                text: inner_source(self.source, events, start, end).to_owned(),
            }),
            Tag::BlockQuote(_) => self.push(BlockKind::Quote {
                text: strip_quote_markers(&self.source[range.clone()]),
            }),
            Tag::CodeBlock(kind) => self.push(code_block(events, start, end, kind)),
            Tag::List(first) => self.append_list(events, start, end, first.is_some(), 0),
            Tag::Table(_) => {
                let rows = self.table_rows(events, start, end);
                if !rows.is_empty() {
                    self.push(BlockKind::Table {
                        rows,
                        column_widths_px: None,
                    });
                }
            }
            // Anything else (HTML, unknown syntax) is kept as plain text and never run; only a
            // block that is a single `<video>` or `<audio>` plays.
            _ => {
                let raw = self.source[range.clone()].trim();
                if !raw.is_empty() {
                    self.push(player_or_words(raw));
                }
            }
        }
        end + 1
    }

    /// The rows of the table from `start` to `end`, header first, each cell's source trimmed.
    fn table_rows(&self, events: &Events<'_>, start: usize, end: usize) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut index = start + 1;
        while index < end {
            if let (Event::Start(Tag::TableHead | Tag::TableRow), _) = &events[index] {
                let row_end = end_of(events, index);
                rows.push(self.table_cells(events, index, row_end));
                index = row_end + 1;
            } else {
                index += 1;
            }
        }
        rows
    }

    fn table_cells(&self, events: &Events<'_>, row: usize, row_end: usize) -> Vec<String> {
        let mut cells = Vec::new();
        let mut cell = row + 1;
        while cell < row_end {
            let cell_end = end_of(events, cell);
            cells.push(
                inner_source(self.source, events, cell, cell_end)
                    .trim()
                    .replace("\\|", "|"),
            );
            cell = cell_end + 1;
        }
        cells
    }

    fn append_list(
        &mut self,
        events: &Events<'_>,
        start: usize,
        end: usize,
        ordered: bool,
        indent: u8,
    ) {
        for (index, item_end) in list_items(events, start, end) {
            self.append_list_item(events, (index, item_end), ordered, indent);
        }
    }

    /// The list item from `start` to `item_end`, with its nested blocks.
    fn append_list_item(
        &mut self,
        events: &Events<'_>,
        (start, item_end): (usize, usize),
        ordered: bool,
        indent: u8,
    ) {
        let mut item = ItemWords {
            indent: indent.min(MAX_LIST_INDENT),
            ordered,
            task: None,
            has_list_block: false,
            parts: Vec::new(),
        };
        self.append_item(events, (start, item_end), &mut item, indent);
    }

    /// Runs `append` and records the blocks it made as coming from `range` of the source.
    fn append_span(&mut self, range: Range<usize>, append: impl FnOnce(&mut Self)) {
        let first = self.blocks.len();
        append(self);
        if self.blocks.len() == first {
            return;
        }
        let end = range.start + self.source[range.clone()].trim_end_matches('\n').len();
        self.spans.push(SpanAt {
            range: range.start..end,
            blocks: self.blocks[first..].to_vec(),
        });
    }

    /// Appends the top-level construct starting at `start` and records its spans, one per list
    /// item, so unedited items keep their original source.
    fn append_top_level(&mut self, events: &Events<'_>, start: usize) -> usize {
        let (event, range) = &events[start];
        let Event::Start(Tag::List(first)) = event else {
            let mut next = start + 1;
            self.append_span(range.clone(), |importer| {
                next = importer.append_block(events, start);
            });
            return next;
        };
        let ordered = first.is_some();
        let end = end_of(events, start);
        for (item, item_end) in list_items(events, start, end) {
            self.append_span(events[item].1.clone(), |importer| {
                importer.append_list_item(events, (item, item_end), ordered, 0);
            });
        }
        end + 1
    }

    /// The list item from `start` to `item_end` at depth `indent`: its text as one list block, then
    /// each nested block, then any later text as paragraphs.
    fn append_item(
        &mut self,
        events: &Events<'_>,
        (start, item_end): (usize, usize),
        item: &mut ItemWords,
        indent: u8,
    ) {
        let mut child = start + 1;
        loop {
            if !closes_words(events, child, item_end) {
                child = self.gather_words(events, child, item_end, item);
                continue;
            }
            if let Some(kind) = item.take_block() {
                self.push(kind);
            }
            if child >= item_end {
                break;
            }
            child = self.append_nested(events, child, indent);
        }
    }

    /// A block inside a list item at depth `indent`: a list one level deeper, or any other block.
    /// Returns the index after it.
    fn append_nested(&mut self, events: &Events<'_>, child: usize, indent: u8) -> usize {
        let (Event::Start(Tag::List(first)), _) = &events[child] else {
            return self.append_block(events, child);
        };
        let list_end = end_of(events, child);
        self.append_list(
            events,
            child,
            list_end,
            first.is_some(),
            indent.saturating_add(1),
        );
        list_end + 1
    }

    /// Gathers an item's text starting at `child` into `item` and returns the index after it: a
    /// task marker, a paragraph, or a tight item's inline run.
    fn gather_words(
        &self,
        events: &Events<'_>,
        child: usize,
        item_end: usize,
        item: &mut ItemWords,
    ) -> usize {
        match &events[child] {
            (Event::TaskListMarker(checked), _) => {
                item.task = Some(*checked);
                child + 1
            }
            (Event::Start(Tag::Paragraph), range) => {
                let paragraph_end = end_of(events, child);
                let text = if let Some((Event::TaskListMarker(checked), _)) = events.get(child + 1)
                {
                    item.task = Some(*checked);
                    inner_source(self.source, events, child + 1, paragraph_end)
                } else {
                    &self.source[range.clone()]
                };
                item.parts.push(dedent_lines(text));
                paragraph_end + 1
            }
            _ => {
                let run_end = tight_run_end(events, child, item_end);
                let text = &self.source[events[child].1.start..events[run_end - 1].1.end];
                item.parts.push(dedent_lines(text));
                run_end
            }
        }
    }
}

/// A list item's text as it is gathered, and the block it makes.
struct ItemWords {
    indent: u8,
    ordered: bool,
    /// The item is a task, ticked or not.
    task: Option<bool>,
    /// The item's own block exists, so later text becomes paragraphs.
    has_list_block: bool,
    parts: Vec<String>,
}

impl ItemWords {
    /// The block made from the text gathered so far: the item's own block first, then paragraphs.
    /// `None` when nothing was gathered after its own block.
    fn take_block(&mut self) -> Option<BlockKind> {
        if self.parts.is_empty() && self.has_list_block {
            return None;
        }
        let text = std::mem::take(&mut self.parts).join("\n");
        let indent = self.indent;
        let kind = match (self.has_list_block, self.task, self.ordered) {
            (true, _, _) => BlockKind::Paragraph { text },
            (false, Some(checked), _) => BlockKind::Todo {
                indent,
                checked,
                text,
            },
            (false, None, true) => BlockKind::Numbered { indent, text },
            (false, None, false) => BlockKind::Bullet { indent, text },
        };
        self.has_list_block = true;
        Some(kind)
    }
}

/// Whether an item's gathered text ends at `child`: the item's end, or a nested block.
fn closes_words(events: &Events<'_>, child: usize, item_end: usize) -> bool {
    if child >= item_end {
        return true;
    }
    match &events[child].0 {
        Event::Start(tag) => is_block(tag) && !matches!(tag, Tag::Paragraph),
        Event::Rule => true,
        _ => false,
    }
}

/// Where a tight item's inline run from `child` ends: at the next block or the item's end.
fn tight_run_end(events: &Events<'_>, mut child: usize, item_end: usize) -> usize {
    while child < item_end
        && !matches!(&events[child].0, Event::Start(tag) if is_block(tag))
        && !matches!(&events[child].0, Event::Rule | Event::TaskListMarker(_))
    {
        child = match &events[child].0 {
            Event::Start(_) => end_of(events, child) + 1,
            _ => child + 1,
        };
    }
    child
}

/// A code block's source without its final newline; fenced as `northroom-html`, an HTML block.
fn code_block(events: &Events<'_>, start: usize, end: usize, kind: &CodeBlockKind) -> BlockKind {
    let mut text: String = events[start + 1..end]
        .iter()
        .filter_map(|(event, _)| match event {
            Event::Text(text) => Some(text.as_ref()),
            _ => None,
        })
        .collect();
    if text.ends_with('\n') {
        text.pop();
    }
    let language = match kind {
        CodeBlockKind::Fenced(info) if !info.trim().is_empty() => Some(info.trim().to_owned()),
        _ => None,
    };
    if language.as_deref() == Some(HTML_ARTIFACT_LANGUAGE) {
        BlockKind::Html { text }
    } else {
        BlockKind::Code { language, text }
    }
}

/// Markdown in, blocks out. `create_id` names each new block.
pub fn blocks_from_markdown(markdown: &str, create_id: &mut dyn FnMut() -> String) -> Vec<Block> {
    read_markdown(markdown, create_id).0
}

/// Parses Markdown into blocks plus the source layout around them.
fn read_markdown(
    markdown: &str,
    create_id: &mut dyn FnMut() -> String,
) -> (Vec<Block>, PageSource) {
    let source = markdown.replace("\r\n", "\n").replace('\r', "\n");
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH;
    let events: Vec<(Event<'_>, Range<usize>)> = Parser::new_ext(&source, options)
        .into_offset_iter()
        .collect();
    let mut importer = Importer {
        source: &source,
        create_id,
        blocks: Vec::new(),
        spans: Vec::new(),
    };
    let mut index = 0;
    while index < events.len() {
        index = match &events[index].0 {
            Event::Start(_) | Event::Rule => importer.append_top_level(&events, index),
            _ => index + 1,
        };
    }
    let written = PageSource::of_spans(&source, importer.spans);
    (importer.blocks, written)
}

/// A span of a page's file that produced some of its blocks, written back verbatim while those
/// blocks are unchanged.
#[derive(Clone, Debug, PartialEq)]
struct Span {
    /// The text between the previous span and this one: blank lines, and anything that made no
    /// block (a link reference definition).
    gap: String,
    text: String,
    blocks: Vec<Block>,
}

/// The file's source around and between its blocks, so writing the page back changes only what
/// was edited. A page created in the app has none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageSource {
    /// The file's front matter verbatim, `---` lines included. Preserved but never shown.
    front_matter: Option<String>,
    /// The `# Title` line verbatim (with any blank lines before it), and the title text.
    title_line: Option<(String, String)>,
    spans: Vec<Span>,
    /// What follows the last span.
    tail: String,
}

impl PageSource {
    fn of_spans(source: &str, spans: Vec<SpanAt>) -> Self {
        let mut written = Vec::with_capacity(spans.len());
        let mut previous_end = 0;
        for span in spans {
            let start = span.range.start.max(previous_end);
            written.push(Span {
                gap: source[previous_end..start].to_owned(),
                text: source[start..span.range.end].to_owned(),
                blocks: span.blocks,
            });
            previous_end = span.range.end.max(start);
        }
        Self {
            front_matter: None,
            title_line: None,
            spans: written,
            tail: source[previous_end..].to_owned(),
        }
    }
}

const FRONT_MATTER_OPEN: &str = "---";
const FRONT_MATTER_CLOSES: [&str; 2] = ["---", "..."];

/// The front matter `text` opens with (a `---` line, then lines up to and including a closing
/// `---` or `...` line), and the text after it.
fn split_front_matter(text: &str) -> (Option<&str>, &str) {
    let Some((first, mut rest)) = text.split_once('\n') else {
        return (None, text);
    };
    if first.trim_end() != FRONT_MATTER_OPEN {
        return (None, text);
    }
    let mut end = first.len() + 1;
    while !rest.is_empty() {
        let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
        end += rest.len() - after.len();
        if FRONT_MATTER_CLOSES.contains(&line.trim_end()) {
            return (Some(&text[..end]), &text[end..]);
        }
        rest = after;
    }
    (None, text)
}

/// One stretch of the written page: a verbatim span of the file, or freshly rendered blocks.
#[derive(Clone, Copy, PartialEq)]
enum Piece {
    Kept(usize),
    Written,
}

/// Which of `blocks` are written as verbatim spans and which are rendered. A span is kept while
/// all its blocks remain, unchanged and in order, and no rendered block nests into its last
/// item.
fn plan_pieces(blocks: &[Block], spans: &[Span]) -> Vec<(Piece, Range<usize>)> {
    let starts: std::collections::HashMap<&str, usize> = spans
        .iter()
        .enumerate()
        .map(|(index, span)| (span.blocks[0].id.as_str(), index))
        .collect();
    let mut pieces = Vec::new();
    let mut index = 0;
    while index < blocks.len() {
        let kept = starts
            .get(blocks[index].id.as_str())
            .copied()
            .filter(|span| span_kept_at(blocks, index, &spans[*span]));
        let (piece, length) = match kept {
            Some(span) => (Piece::Kept(span), spans[span].blocks.len()),
            None => (Piece::Written, 1),
        };
        pieces.push((piece, index..index + length));
        index += length;
    }
    pieces
}

fn span_kept_at(blocks: &[Block], index: usize, span: &Span) -> bool {
    let end = index + span.blocks.len();
    let nested_after = blocks
        .get(end)
        .and_then(|block| list_indent(&block.kind))
        .is_some_and(|indent| indent > 0);
    blocks.get(index..end) == Some(span.blocks.as_slice()) && !nested_after
}

/// The span starting with `block`, when the gap before it in the file holds more than blank
/// lines.
fn gap_with_words<'a>(spans: &'a [Span], block: &Block) -> Option<(usize, &'a str)> {
    spans.iter().enumerate().find_map(|(index, span)| {
        let words = span.gap.trim_matches('\n');
        (span.blocks[0].id == block.id && !words.trim().is_empty()).then_some((index, words))
    })
}

/// The page's blocks as the file body, ending in a newline. `opening` goes before the body when
/// its start is rendered (a blank line under a title or front matter).
fn write_body(blocks: &[Block], source: &PageSource, opening: &str) -> String {
    let visible: Vec<Block> = blocks
        .iter()
        .filter(
            |block| !matches!(&block.kind, BlockKind::Paragraph { text } if text.trim().is_empty()),
        )
        .cloned()
        .collect();
    let pieces = plan_pieces(&visible, &source.spans);
    let mut writer = BodyWriter {
        blocks: &visible,
        source,
        numbers: list_numbers(&visible),
        columns: Vec::new(),
        gaps_written: vec![false; source.spans.len()],
        out: String::new(),
    };
    let mut previous = None;
    for piece in &pieces {
        writer.separate(previous, piece, opening);
        writer.write_piece(piece);
        previous = Some(piece);
    }
    writer.finish(previous, opening);
    writer.out
}

/// Writes a page's blocks back into its file's shape.
struct BodyWriter<'a> {
    blocks: &'a [Block],
    source: &'a PageSource,
    numbers: Vec<Option<u32>>,
    /// The content column of the last rendered list item at each indent.
    columns: Vec<usize>,
    /// Which spans' gaps are written.
    gaps_written: Vec<bool>,
    out: String,
}

type PlannedPiece = (Piece, Range<usize>);

impl BodyWriter<'_> {
    /// Writes the separator between the previous piece (none at the start) and `next`.
    fn separate(&mut self, previous: Option<&PlannedPiece>, next: &PlannedPiece, opening: &str) {
        let spans = &self.source.spans;
        // Spans that were adjacent in the file keep the file's own gap.
        let adjacent = match (previous, next.0) {
            (None, Piece::Kept(span)) => span == 0,
            (Some((Piece::Kept(before), _)), Piece::Kept(span)) => before + 1 == span,
            (Some((Piece::Written, _)), Piece::Kept(_)) | (_, Piece::Written) => false,
        };
        if let (true, Piece::Kept(span)) = (adjacent, next.0) {
            self.gaps_written[span] = true;
            self.out.push_str(&spans[span].gap);
            return;
        }
        let between = match previous {
            None => opening,
            Some((_, range)) => {
                let same = same_list(
                    &self.blocks[range.end - 1].kind,
                    &self.blocks[next.1.start].kind,
                );
                if same {
                    "\n"
                } else {
                    "\n\n"
                }
            }
        };
        self.out.push_str(between);
        if let Some((span, words)) = gap_with_words(spans, &self.blocks[next.1.start]) {
            self.gaps_written[span] = true;
            if between == "\n" {
                self.out.push('\n');
            }
            self.out.push_str(words);
            self.out.push_str("\n\n");
        }
    }

    fn write_piece(&mut self, (piece, range): &PlannedPiece) {
        match piece {
            Piece::Kept(span) => {
                self.columns.clear();
                self.out.push_str(&self.source.spans[*span].text);
            }
            Piece::Written => {
                let rendered = self.render(range.start);
                self.out.push_str(&rendered);
            }
        }
    }

    /// Renders one block; a list item is placed under its parent item.
    fn render(&mut self, index: usize) -> String {
        let kind = &self.blocks[index].kind;
        let number = self.numbers[index].unwrap_or(1);
        let Some(indent) = list_indent(kind) else {
            self.columns.clear();
            return render_block(kind, number);
        };
        let level = usize::from(indent);
        let start = match level {
            0 => 0,
            _ => self
                .columns
                .get(level - 1)
                .or(self.columns.last())
                .copied()
                .unwrap_or(0),
        };
        let Some((text, content_column)) = render_list_item(kind, number, start) else {
            return render_block(kind, number);
        };
        self.columns.truncate(level);
        self.columns.push(content_column);
        text
    }

    /// Ends the body with the file's own ending when its last span is kept, else a newline, then
    /// any gap text not yet written, so none is lost.
    fn finish(&mut self, last: Option<&PlannedPiece>, opening: &str) {
        let spans = &self.source.spans;
        let kept_tail = matches!(last, Some((Piece::Kept(span), _)) if span + 1 == spans.len());
        let mut left: Vec<&str> = spans
            .iter()
            .zip(&self.gaps_written)
            .filter(|(_, written)| !**written)
            .map(|(span, _)| span.gap.trim_matches('\n'))
            .collect();
        if kept_tail {
            self.out.push_str(&self.source.tail);
        } else {
            left.push(self.source.tail.trim_matches('\n'));
            if !self.out.is_empty() {
                self.out.push('\n');
            }
        }
        for words in left.into_iter().filter(|words| !words.trim().is_empty()) {
            self.append_after_blank_line(words, opening);
        }
    }

    /// Appends `words` as their own paragraph, after a blank line or, at the start of the body,
    /// after `opening`.
    fn append_after_blank_line(&mut self, words: &str, opening: &str) {
        if self.out.is_empty() {
            self.out.push_str(opening);
        } else {
            if !self.out.ends_with('\n') {
                self.out.push('\n');
            }
            self.out.push('\n');
        }
        self.out.push_str(words);
        self.out.push('\n');
    }
}

/// Whether `block` goes on the line right after `previous` as the next item of the same list.
/// Anything else needs a blank line first.
fn same_list(previous: &BlockKind, block: &BlockKind) -> bool {
    is_list(previous)
        && is_list(block)
        && (list_indent(block).unwrap_or(0) > 0
            || matches!(previous, BlockKind::Numbered { .. })
                == matches!(block, BlockKind::Numbered { .. }))
}

/// A title's or heading's normalized text, to detect a heading that repeats the title.
fn title_words(text: &str) -> String {
    let shown: String = crate::page::inline::parse(text)
        .into_iter()
        .map(|segment| segment.text)
        .collect();
    shown
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

const TITLE_PREFIX: &str = "# ";

/// A page read from a Markdown file. Its title comes from the opening `# Title` line, or else
/// the file name.
pub struct Page {
    pub title: String,
    pub blocks: Vec<Block>,
    /// Whether the file opens with a `# Title` line, so writing it back keeps one. A file without
    /// one never gains one.
    pub states_title: bool,
    /// Each block's inline segments, in block order (empty for a block without text).
    pub segments: Vec<Vec<crate::page::inline::Segment>>,
    pub numbers: Vec<Option<u32>>,
    /// The file's source around and between the blocks.
    pub source: PageSource,
}

impl Page {
    /// Reads a file's text as a page. `name` is the listed file name without `.md`. A page with
    /// neither a title line nor a name is titled "Untitled" in `language`.
    pub fn from_file(
        name: &str,
        text: &str,
        language: &str,
        create_id: &mut dyn FnMut() -> String,
    ) -> Self {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let (front_matter, body) = split_front_matter(&normalized);
        let lines: Vec<&str> = body.split('\n').collect();
        let first = lines.iter().position(|line| !line.trim().is_empty());
        let stated = first.filter(|index| lines[*index].starts_with(TITLE_PREFIX));
        let (title, markdown, title_line) = match stated {
            Some(index) => {
                let own = lines[index][TITLE_PREFIX.len()..].trim();
                (
                    if own.is_empty() { name } else { own }.to_owned(),
                    lines[index + 1..].join("\n"),
                    Some(lines[..=index].join("\n")),
                )
            }
            None => (name.to_owned(), body.to_owned(), None),
        };
        let title = match title.trim() {
            "" => crate::strings::text(language, crate::strings::key::PAGE_UNTITLED).to_owned(),
            trimmed => trimmed.to_owned(),
        };
        let states_title = title_line.is_some();
        let (mut blocks, mut source) = read_markdown(&markdown, create_id);
        source.front_matter = front_matter.map(str::to_owned);
        source.title_line = title_line.map(|line| (line, title.clone()));
        // A page shows its title once. Markdown often repeats the title as an opening
        // heading; that heading is dropped instead of becoming a block.
        let repeats = matches!(
            blocks.first().map(|block| &block.kind),
            Some(BlockKind::Heading { text, .. }) if title_words(text) == title_words(&title)
        );
        if repeats {
            blocks.remove(0);
        }
        let mut page = Self {
            title,
            blocks,
            states_title,
            segments: Vec::new(),
            numbers: Vec::new(),
            source,
        };
        page.derive();
        page
    }

    /// Recomputes what is derived from the blocks, after they change.
    pub fn derive(&mut self) {
        self.segments = self
            .blocks
            .iter()
            .map(|block| match &block.kind {
                BlockKind::Paragraph { text }
                | BlockKind::Heading { text, .. }
                | BlockKind::Bullet { text, .. }
                | BlockKind::Numbered { text, .. }
                | BlockKind::Todo { text, .. }
                | BlockKind::Quote { text } => crate::page::inline::parse(text),
                _ => Vec::new(),
            })
            .collect();
        self.numbers = list_numbers(&self.blocks);
    }

    /// A block's text as typed, inline marks included; `None` for a block without a single text
    /// (a divider, a table).
    pub fn text(&self, index: usize) -> Option<&str> {
        match &self.blocks.get(index)?.kind {
            BlockKind::Paragraph { text }
            | BlockKind::Heading { text, .. }
            | BlockKind::Bullet { text, .. }
            | BlockKind::Numbered { text, .. }
            | BlockKind::Todo { text, .. }
            | BlockKind::Quote { text }
            | BlockKind::Code { text, .. }
            | BlockKind::Html { text } => Some(text),
            // Media is atomic with its own caption (`page/media.rs`); a player is atomic and its
            // HTML is never typed in.
            BlockKind::Media { .. }
            | BlockKind::Player { .. }
            | BlockKind::Divider
            | BlockKind::Table { .. } => None,
        }
    }

    /// Replaces a block's text. Returns whether anything changed.
    pub fn set_text(&mut self, index: usize, new_text: &str) -> bool {
        let Some(block) = self.blocks.get_mut(index) else {
            return false;
        };
        let slot = match &mut block.kind {
            BlockKind::Paragraph { text }
            | BlockKind::Heading { text, .. }
            | BlockKind::Bullet { text, .. }
            | BlockKind::Numbered { text, .. }
            | BlockKind::Todo { text, .. }
            | BlockKind::Quote { text }
            | BlockKind::Code { text, .. }
            | BlockKind::Html { text } => text,
            BlockKind::Media { .. }
            | BlockKind::Player { .. }
            | BlockKind::Divider
            | BlockKind::Table { .. } => return false,
        };
        if slot == new_text {
            return false;
        }
        *slot = new_text.to_owned();
        self.derive();
        true
    }

    /// Ticks or unticks a to-do. Returns whether the block was one.
    pub fn toggle_todo(&mut self, index: usize) -> bool {
        match self.blocks.get_mut(index).map(|block| &mut block.kind) {
            Some(BlockKind::Todo { checked, .. }) => {
                *checked = !*checked;
                true
            }
            _ => false,
        }
    }

    /// Adds an empty block after `index`, continuing a list as a list item of the same kind
    /// and depth and anything else as a paragraph. Returns the new block's index.
    pub fn insert_after(&mut self, index: usize) -> usize {
        let kind = match self.blocks.get(index).map(|block| &block.kind) {
            Some(BlockKind::Bullet { indent, .. }) => BlockKind::Bullet {
                indent: *indent,
                text: String::new(),
            },
            Some(BlockKind::Numbered { indent, .. }) => BlockKind::Numbered {
                indent: *indent,
                text: String::new(),
            },
            Some(BlockKind::Todo { indent, .. }) => BlockKind::Todo {
                indent: *indent,
                checked: false,
                text: String::new(),
            },
            _ => BlockKind::Paragraph {
                text: String::new(),
            },
        };
        let at = (index + 1).min(self.blocks.len());
        self.blocks.insert(
            at,
            Block {
                id: create_block_id(),
                kind,
            },
        );
        self.derive();
        at
    }

    /// Removes a block.
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.blocks.len() {
            return false;
        }
        self.blocks.remove(index);
        self.derive();
        true
    }

    /// The page's file contents: the original front matter, the `# Title` line if the file had
    /// one, then Markdown ending in a newline. Unedited parts are written verbatim.
    pub fn to_file(&self) -> String {
        let mut file = self.source.front_matter.clone().unwrap_or_default();
        if self.states_title {
            match &self.source.title_line {
                Some((line, title)) if *title == self.title => file.push_str(line),
                Some(_) | None => {
                    file.push_str(TITLE_PREFIX);
                    file.push_str(&self.title);
                }
            }
            file.push('\n');
        }
        let opening = if self.states_title || self.source.front_matter.is_some() {
            "\n"
        } else {
            ""
        };
        file.push_str(&write_body(&self.blocks, &self.source, opening));
        file
    }
}

/// A short random id: unique within one page, and cheap for an assistant to quote.
pub fn create_block_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_owned()
}

fn is_list(kind: &BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Bullet { .. } | BlockKind::Numbered { .. } | BlockKind::Todo { .. }
    )
}

fn list_indent(kind: &BlockKind) -> Option<u8> {
    match kind {
        BlockKind::Bullet { indent, .. }
        | BlockKind::Numbered { indent, .. }
        | BlockKind::Todo { indent, .. } => Some(*indent),
        _ => None,
    }
}

/// The number each numbered item shows, by block index. Numbers count up within an unbroken run
/// of list items at the same indent and restart after anything else.
pub fn list_numbers(blocks: &[Block]) -> Vec<Option<u32>> {
    let mut counts: Vec<u32> = Vec::new();
    blocks
        .iter()
        .map(|block| {
            let Some(indent) = list_indent(&block.kind) else {
                counts.clear();
                return None;
            };
            counts.resize(usize::from(indent) + 1, 0);
            if !matches!(block.kind, BlockKind::Numbered { .. }) {
                counts[usize::from(indent)] = 0;
                return None;
            }
            counts[usize::from(indent)] += 1;
            Some(counts[usize::from(indent)])
        })
        .collect()
}

fn code_fence(text: &str) -> String {
    let mut longest = 2;
    let mut run = 0;
    for character in text.chars() {
        run = if character == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat(longest + 1)
}

/// A list item's marker (numbered `number`), its text, and the content offset from the
/// marker's start; `None` for a block that is not a list item.
fn list_marker(kind: &BlockKind, number: u32) -> Option<(String, &str, usize)> {
    const BULLET: &str = "- ";
    match kind {
        BlockKind::Bullet { text, .. } => Some((BULLET.to_owned(), text, BULLET.len())),
        BlockKind::Numbered { text, .. } => {
            let marker = format!("{number}. ");
            let width = marker.len();
            Some((marker, text, width))
        }
        // Markdown treats the box as part of the item's text, so blocks nested under a to-do
        // align with its box.
        BlockKind::Todo { checked, text, .. } => {
            let mark = if *checked { "[x]" } else { "[ ]" };
            Some((format!("{BULLET}{mark} "), text, BULLET.len()))
        }
        BlockKind::Paragraph { .. }
        | BlockKind::Heading { .. }
        | BlockKind::Quote { .. }
        | BlockKind::Code { .. }
        | BlockKind::Html { .. }
        | BlockKind::Divider
        | BlockKind::Table { .. }
        | BlockKind::Media { .. }
        | BlockKind::Player { .. } => None,
    }
}

/// A list item rendered with its marker at column `start`, and the content column where
/// nested blocks go.
fn render_list_item(kind: &BlockKind, number: u32, start: usize) -> Option<(String, usize)> {
    let (marker, text, content_offset) = list_marker(kind, number)?;
    let content = start + content_offset;
    let continued = text.replace('\n', &format!("\n{}", " ".repeat(content)));
    Some((format!("{}{marker}{continued}", " ".repeat(start)), content))
}

fn render_block(kind: &BlockKind, number: u32) -> String {
    match kind {
        BlockKind::Paragraph { text } => text.clone(),
        BlockKind::Heading { level, text } => format!("{} {text}", "#".repeat(usize::from(*level))),
        BlockKind::Bullet { indent, .. }
        | BlockKind::Numbered { indent, .. }
        | BlockKind::Todo { indent, .. } => {
            let start = LIST_INDENT.len() * usize::from(*indent);
            render_list_item(kind, number, start).map_or_else(String::new, |(text, _)| text)
        }
        BlockKind::Quote { text } => text
            .split('\n')
            .map(|line| {
                if line.is_empty() {
                    ">".to_owned()
                } else {
                    format!("> {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        BlockKind::Code { language, text } => {
            let fence = code_fence(text);
            format!(
                "{fence}{}\n{text}\n{fence}",
                language.as_deref().unwrap_or("")
            )
        }
        BlockKind::Html { text } => {
            let fence = code_fence(text);
            format!("{fence}{HTML_ARTIFACT_LANGUAGE}\n{text}\n{fence}")
        }
        BlockKind::Divider => "---".to_owned(),
        BlockKind::Media { asset_id, caption } => {
            format!(
                "![{}]({MEDIA_ASSET_SCHEME}{asset_id})",
                caption.replace(['[', ']'], "")
            )
        }
        BlockKind::Player { html } => html.clone(),
        BlockKind::Table { rows, .. } => {
            let cell = |text: &String| {
                text.replace('|', "\\|")
                    .replace("\r\n", " ")
                    .replace('\n', " ")
            };
            let row = |cells: &Vec<String>| {
                format!(
                    "| {} |",
                    cells.iter().map(cell).collect::<Vec<_>>().join(" | ")
                )
            };
            let header = rows.first().cloned().unwrap_or_default();
            let mut lines = vec![
                row(&header),
                format!(
                    "| {} |",
                    header.iter().map(|_| "---").collect::<Vec<_>>().join(" | ")
                ),
            ];
            lines.extend(rows.iter().skip(1).map(row));
            lines.join("\n")
        }
    }
}

/// The page as Markdown. Empty paragraphs are editor spacing and are left out.
pub fn markdown_from_blocks(blocks: &[Block]) -> String {
    let mut markdown = write_body(blocks, &PageSource::default(), "");
    if markdown.ends_with('\n') {
        markdown.pop();
    }
    markdown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(markdown: &str) -> Vec<BlockKind> {
        let mut next = 0;
        let mut ids = || {
            next += 1;
            format!("b{next}")
        };
        blocks_from_markdown(markdown, &mut ids)
            .into_iter()
            .map(|block| block.kind)
            .collect()
    }

    fn paragraph(text: &str) -> BlockKind {
        BlockKind::Paragraph {
            text: text.to_owned(),
        }
    }

    #[test]
    fn reads_the_ordinary_blocks_keeping_inline_marks_as_typed() {
        let blocks = parse(
            "# Title\n\nSome **bold** and `code`\nsecond line.\n\n## Part ##\n\n#### Deep\n\n> quoted\n> more\n\n---\n\n```rust\nfn main() {}\n```\n",
        );
        assert_eq!(
            blocks,
            [
                BlockKind::Heading {
                    level: 1,
                    text: "Title".into()
                },
                paragraph("Some **bold** and `code`\nsecond line."),
                BlockKind::Heading {
                    level: 2,
                    text: "Part".into()
                },
                BlockKind::Heading {
                    level: 3,
                    text: "Deep".into()
                },
                BlockKind::Quote {
                    text: "quoted\nmore".into()
                },
                BlockKind::Divider,
                BlockKind::Code {
                    language: Some("rust".into()),
                    text: "fn main() {}".into()
                },
            ]
        );
    }

    #[test]
    fn reads_lists_with_nesting_tasks_and_what_follows_inside_an_item() {
        let blocks = parse("- one\n- two\n  - inner\n    1. deep\n- [ ] open\n- [x] done\n\n1. first\n2. second\n\n   after\n");
        assert_eq!(
            blocks,
            [
                BlockKind::Bullet {
                    indent: 0,
                    text: "one".into()
                },
                BlockKind::Bullet {
                    indent: 0,
                    text: "two".into()
                },
                BlockKind::Bullet {
                    indent: 1,
                    text: "inner".into()
                },
                BlockKind::Numbered {
                    indent: 2,
                    text: "deep".into()
                },
                BlockKind::Todo {
                    indent: 0,
                    checked: false,
                    text: "open".into()
                },
                BlockKind::Todo {
                    indent: 0,
                    checked: true,
                    text: "done".into()
                },
                BlockKind::Numbered {
                    indent: 0,
                    text: "first".into()
                },
                BlockKind::Numbered {
                    indent: 0,
                    text: "second\nafter".into()
                },
            ]
        );
    }

    #[test]
    fn reads_tables_media_and_marked_html() {
        let blocks = parse(
            "| A | B |\n| --- | --- |\n| 1 | x \\| y |\n\n![A shot](asset:abc-123)\n\n```northroom-html\n<b>hi</b>\n```\n\n![web](https://example.com/a.png)\n",
        );
        assert_eq!(
            blocks,
            [
                BlockKind::Table {
                    rows: vec![
                        vec!["A".into(), "B".into()],
                        vec!["1".into(), "x | y".into()]
                    ],
                    column_widths_px: None,
                },
                BlockKind::Media {
                    asset_id: "abc-123".into(),
                    caption: "A shot".into()
                },
                BlockKind::Html {
                    text: "<b>hi</b>".into()
                },
                paragraph("![web](https://example.com/a.png)"),
            ]
        );
    }

    #[test]
    fn writes_markdown_that_reads_back_the_same() {
        let markdown = "# Title\n\nWords.\n\n- one\n  - inner\n- two\n\n1. first\n2. second\n\n- [x] done\n\n> quoted\n> more\n\n---\n\n```rust\nlet tick = \"`\";\n```\n\n| A | B |\n| --- | --- |\n| 1 | 2 |";
        let mut next = 0;
        let mut ids = || {
            next += 1;
            format!("b{next}")
        };
        let blocks = blocks_from_markdown(markdown, &mut ids);
        assert_eq!(markdown_from_blocks(&blocks), markdown);
    }

    fn page(name: &str, text: &str) -> Page {
        let mut next = 0;
        let mut ids = || {
            next += 1;
            format!("b{next}")
        };
        Page::from_file(name, text, "en", &mut ids)
    }

    #[test]
    fn a_page_is_titled_once_by_its_first_line_or_else_its_name() {
        let stated = page("plan", "\n# The Plan\n\n# the   plan\n\nBody **here**.\n");
        assert_eq!(
            (stated.title.as_str(), stated.states_title),
            ("The Plan", true)
        );
        assert_eq!(
            stated.blocks.len(),
            1,
            "the repeated heading is not a block"
        );
        assert_eq!(stated.segments[0].len(), 3);

        let named = page("Notes", "Body.\n\n## Part\n");
        assert_eq!((named.title.as_str(), named.states_title), ("Notes", false));
        assert_eq!(named.blocks.len(), 2);

        // Neither: "Untitled", in the interface language.
        assert_eq!(page("", "# \n").title, "Untitled");
    }

    #[test]
    fn edits_change_the_blocks_and_what_is_written() {
        let mut edited = page("x", "# T\n\n- [ ] buy milk\n- [x] call\n\nText.\n");
        assert!(edited.toggle_todo(0));
        assert!(!edited.toggle_todo(2), "a paragraph is not a to-do");
        assert!(edited.set_text(2, "Text **now**."));
        assert!(
            !edited.set_text(2, "Text **now**."),
            "the same words are no change"
        );
        assert_eq!(edited.segments[2].len(), 3);
        let added = edited.insert_after(1);
        assert_eq!(added, 2);
        assert!(matches!(
            edited.blocks[2].kind,
            BlockKind::Todo { checked: false, .. }
        ));
        edited.set_text(2, "write");
        assert_eq!(
            edited.to_file(),
            "# T\n\n- [x] buy milk\n- [x] call\n- [ ] write\n\nText **now**.\n"
        );
        assert!(edited.remove(2));
        assert_eq!(edited.blocks.len(), 3);
    }

    #[test]
    fn a_file_keeps_its_title_line_and_never_gains_one() {
        assert_eq!(
            page("x", "# Title\n\nBody.\n").to_file(),
            "# Title\n\nBody.\n"
        );
        assert_eq!(page("x", "# Title\n").to_file(), "# Title\n");
        assert_eq!(
            page("x", "Body.\n\n- a\n- b\n").to_file(),
            "Body.\n\n- a\n- b\n"
        );
        assert_eq!(page("x", "").to_file(), "");
    }

    #[test]
    fn front_matter_is_kept_as_written_and_is_not_a_block() {
        let file = "---\nname: notes\nmetadata:\n  type: reference\n---\n\nBody.\n";
        let mut read = page("notes", file);
        assert_eq!(read.title, "notes");
        assert_eq!(
            read.blocks.len(),
            1,
            "the front matter is neither a divider nor a heading"
        );
        assert_eq!(read.to_file(), file);
        assert!(read.set_text(0, "New body."));
        assert_eq!(
            read.to_file(),
            "---\nname: notes\nmetadata:\n  type: reference\n---\n\nNew body.\n"
        );

        let titled = page("x", "---\na: b\n...\n# Plan\n\nBody.\n");
        assert_eq!((titled.title.as_str(), titled.blocks.len()), ("Plan", 1));
        assert_eq!(titled.to_file(), "---\na: b\n...\n# Plan\n\nBody.\n");

        // A rule with no closing line is a divider, not front matter.
        let unclosed = page("x", "---\n\nBody.\n");
        assert_eq!(unclosed.blocks[0].kind, BlockKind::Divider);
    }

    #[test]
    fn an_edit_rewrites_only_the_block_it_changed() {
        let file = "# T\n\n* one\n* two\n\n|A|B|\n|:-|-:|\n|1|2|\n\nSee [the site][site].\n\n[site]: https://example.com\n";
        let mut read = page("x", file);
        assert_eq!(read.to_file(), file);
        assert!(read.set_text(1, "three"));
        assert_eq!(
            read.to_file(),
            "# T\n\n* one\n- three\n\n|A|B|\n|:-|-:|\n|1|2|\n\nSee [the site][site].\n\n[site]: https://example.com\n"
        );
        assert!(read.set_text(3, "See [it][site]."));
        assert!(
            read.to_file()
                .ends_with("See [it][site].\n\n[site]: https://example.com\n"),
            "a link definition after an edited block is kept: {}",
            read.to_file()
        );
    }

    #[test]
    fn words_that_make_no_block_are_kept_when_their_neighbours_go() {
        let mut read = page("x", "Intro.\n\n[a]: https://a.example\n\nMiddle.\n\nEnd.\n");
        assert!(read.remove(1));
        assert!(read.remove(0));
        assert_eq!(read.to_file(), "End.\n\n[a]: https://a.example\n");
    }

    #[test]
    fn a_nested_item_lines_up_with_the_words_of_its_parent() {
        let blocks: Vec<Block> = [(0, true, "first"), (1, false, "inner"), (2, true, "deep")]
            .into_iter()
            .map(|(indent, numbered, text)| Block {
                id: text.to_owned(),
                kind: if numbered {
                    BlockKind::Numbered {
                        indent,
                        text: text.into(),
                    }
                } else {
                    BlockKind::Bullet {
                        indent,
                        text: text.into(),
                    }
                },
            })
            .collect();
        let written = markdown_from_blocks(&blocks);
        assert_eq!(written, "1. first\n   - inner\n     1. deep");
        assert_eq!(
            parse(&written),
            blocks
                .into_iter()
                .map(|block| block.kind)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn numbers_restart_after_anything_that_is_not_a_list() {
        let blocks: Vec<Block> = parse("1. a\n2. b\n\ntext\n\n1. c\n")
            .into_iter()
            .map(|kind| Block {
                id: String::new(),
                kind,
            })
            .collect();
        assert_eq!(list_numbers(&blocks), [Some(1), Some(2), None, Some(1)]);
    }
}
