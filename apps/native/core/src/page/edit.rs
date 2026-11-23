//! Editing steps over a page's blocks: Enter, Backspace, Tab, the `/` menu, typed Markdown
//! prefixes, and undo. Each step changes the blocks and returns where the caret goes.
//!
//! Offsets are UTF-16 code units, as a shell's text view counts them. Formatted text is split
//! and joined by `page::inline_edit`, so a mark the split crosses stays intact on both sides;
//! source text (code, HTML) is split verbatim.

use crate::glyph::Glyph;
use crate::page::{create_block_id, Block, BlockKind, Page, MAX_LIST_INDENT};
use crate::strings::{self, key, Key};

const TABLE_START_ROWS: usize = 3;
const TABLE_START_COLUMNS: usize = 3;
const MAX_HISTORY_STEPS: usize = 200;

/// Where the caret goes after a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caret {
    Start,
    End,
    At(usize),
}

/// The block index and position where the caret lands; `None` leaves it in place.
pub type Landing = Option<(usize, Caret)>;

/// What a line can be turned into from the `/` menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Paragraph,
    Heading(u8),
    Bullet,
    Numbered,
    Todo,
    Quote,
    Code,
    Divider,
    Table,
}

/// One entry of the `/` menu.
pub struct Command {
    /// Its label, shown in the interface language (`strings::text`).
    pub label: Key,
    /// The Markdown a person can type instead, shown beside the label.
    pub shortcut: &'static str,
    keywords: &'static [&'static str],
    pub kind: Kind,
    /// The glyph beside it in the `/` menu and the block menu.
    pub icon: Glyph,
}

/// The heading over the `/` menu's blocks, in `language`.
pub fn commands_heading(language: &str) -> &'static str {
    strings::text(language, key::PAGE_BLOCK_BASIC)
}

/// The `/` menu's blocks, in menu order.
pub const COMMANDS: [Command; 11] = [
    Command {
        label: key::PAGE_BLOCK_TEXT,
        shortcut: "",
        keywords: &["paragraph", "plain"],
        kind: Kind::Paragraph,
        icon: Glyph::TextT,
    },
    Command {
        label: key::PAGE_BLOCK_HEADING_ONE,
        shortcut: "#",
        keywords: &["h1", "title", "big"],
        kind: Kind::Heading(1),
        icon: Glyph::TextHOne,
    },
    Command {
        label: key::PAGE_BLOCK_HEADING_TWO,
        shortcut: "##",
        keywords: &["h2", "subtitle", "medium"],
        kind: Kind::Heading(2),
        icon: Glyph::TextHTwo,
    },
    Command {
        label: key::PAGE_BLOCK_HEADING_THREE,
        shortcut: "###",
        keywords: &["h3", "small"],
        kind: Kind::Heading(3),
        icon: Glyph::TextHThree,
    },
    Command {
        label: key::PAGE_BLOCK_BULLETED_LIST,
        shortcut: "-",
        keywords: &["bullet", "unordered", "ul"],
        kind: Kind::Bullet,
        icon: Glyph::ListBullets,
    },
    Command {
        label: key::PAGE_BLOCK_NUMBERED_LIST,
        shortcut: "1.",
        keywords: &["number", "ordered", "ol"],
        kind: Kind::Numbered,
        icon: Glyph::ListNumbers,
    },
    Command {
        label: key::PAGE_BLOCK_TODO_LIST,
        shortcut: "[]",
        keywords: &["todo", "task", "checkbox", "check"],
        kind: Kind::Todo,
        icon: Glyph::CheckSquare,
    },
    Command {
        label: key::PAGE_BLOCK_QUOTE,
        shortcut: ">",
        keywords: &["blockquote", "citation"],
        kind: Kind::Quote,
        icon: Glyph::Quotes,
    },
    Command {
        label: key::PAGE_BLOCK_CODE,
        shortcut: "```",
        keywords: &["code", "snippet", "pre"],
        kind: Kind::Code,
        icon: Glyph::Code,
    },
    Command {
        label: key::PAGE_BLOCK_DIVIDER,
        shortcut: "---",
        keywords: &["hr", "rule", "line", "separator"],
        kind: Kind::Divider,
        icon: Glyph::Minus,
    },
    Command {
        label: key::PAGE_BLOCK_TABLE,
        shortcut: "",
        keywords: &["grid", "rows", "columns"],
        kind: Kind::Table,
        icon: Glyph::Table,
    },
];

/// Indices into `COMMANDS`, in menu order, of entries whose label or a keyword starts with a
/// word of the query. The English label is searched like the keywords.
pub fn filter_commands(query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    let matches = |term: &str| {
        let lowered = term.to_lowercase();
        lowered.starts_with(&needle)
            || lowered
                .split_whitespace()
                .any(|word| word.starts_with(&needle))
    };
    COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, command)| {
            needle.is_empty()
                || matches(strings::text(strings::LANGUAGES[0], command.label))
                || command.keywords.iter().any(|term| matches(term))
        })
        .map(|(index, _)| index)
        .collect()
}

/// The longest query the `/` menu follows.
const MAX_SLASH_QUERY_LENGTH: usize = 24;

/// The `/` menu query in a line being typed: the text between the caret and the last slash before
/// it that starts the line or follows white space, containing no space or slash and at most 24
/// units long. Offsets are UTF-16 units, like the caret. Returns the slash's offset and the
/// query, or `None` when no menu is open.
pub fn find_slash_query(text: &str, caret: usize) -> Option<(usize, String)> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let caret = caret.min(units.len());
    let start = units[..caret]
        .iter()
        .rposition(|&unit| unit == u16::from(b'/'))?;
    if start > 0 {
        let before = char::decode_utf16([units[start - 1]])
            .next()
            .and_then(Result::ok);
        if !before.is_some_and(char::is_whitespace) {
            return None;
        }
    }
    let query_units = &units[start + 1..caret];
    let query = String::from_utf16(query_units).ok()?;
    (query_units.len() <= MAX_SLASH_QUERY_LENGTH
        && !query.chars().any(|c| c.is_whitespace() || c == '/'))
    .then_some((start, query))
}

fn text_of(kind: &BlockKind) -> Option<&str> {
    match kind {
        BlockKind::Paragraph { text }
        | BlockKind::Heading { text, .. }
        | BlockKind::Bullet { text, .. }
        | BlockKind::Numbered { text, .. }
        | BlockKind::Todo { text, .. }
        | BlockKind::Quote { text }
        | BlockKind::Code { text, .. }
        | BlockKind::Html { text } => Some(text),
        // Media has a caption instead of text, so no step joins or splits it.
        BlockKind::Media { .. }
        | BlockKind::Player { .. }
        | BlockKind::Divider
        | BlockKind::Table { .. } => None,
    }
}

fn list_indent(kind: &BlockKind) -> Option<u8> {
    match kind {
        BlockKind::Bullet { indent, .. }
        | BlockKind::Numbered { indent, .. }
        | BlockKind::Todo { indent, .. } => Some(*indent),
        _ => None,
    }
}

fn is_source(kind: &BlockKind) -> bool {
    matches!(kind, BlockKind::Code { .. } | BlockKind::Html { .. })
}

fn paragraph(text: &str) -> Block {
    Block {
        id: create_block_id(),
        kind: BlockKind::Paragraph {
            text: text.to_owned(),
        },
    }
}

/// The block converted to another kind, keeping its text (and a list item's depth).
fn convert(kind: &BlockKind, to: Kind) -> BlockKind {
    let text = text_of(kind).unwrap_or("").to_owned();
    let indent = list_indent(kind).unwrap_or(0);
    match to {
        Kind::Paragraph => BlockKind::Paragraph { text },
        Kind::Heading(level) => BlockKind::Heading { level, text },
        Kind::Bullet => BlockKind::Bullet { indent, text },
        Kind::Numbered => BlockKind::Numbered { indent, text },
        Kind::Todo => BlockKind::Todo {
            indent,
            checked: false,
            text,
        },
        Kind::Quote => BlockKind::Quote { text },
        Kind::Code => BlockKind::Code {
            language: None,
            text,
        },
        Kind::Divider => BlockKind::Divider,
        Kind::Table => BlockKind::Table {
            rows: vec![vec![String::new(); TABLE_START_COLUMNS]; TABLE_START_ROWS],
            column_widths_px: None,
        },
    }
}

/// The block a new line continues as: lists continue, everything else starts plain text.
fn continuation(kind: &BlockKind, text: &str) -> Block {
    let text = text.to_owned();
    Block {
        id: create_block_id(),
        kind: match kind {
            BlockKind::Bullet { indent, .. } => BlockKind::Bullet {
                indent: *indent,
                text,
            },
            BlockKind::Numbered { indent, .. } => BlockKind::Numbered {
                indent: *indent,
                text,
            },
            BlockKind::Todo { indent, .. } => BlockKind::Todo {
                indent: *indent,
                checked: false,
                text,
            },
            _ => BlockKind::Paragraph { text },
        },
    }
}

/// The byte offset of UTF-16 offset `units` in `text`, at a character boundary.
fn byte_offset(text: &str, units: usize) -> usize {
    let mut seen = 0;
    for (index, character) in text.char_indices() {
        if seen >= units {
            return index;
        }
        seen += character.len_utf16();
    }
    text.len()
}

fn utf16_length(text: &str) -> usize {
    text.encode_utf16().count()
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Whether nothing visible precedes `offset`; formatted text may open with hidden marks.
fn at_start(kind: &BlockKind, text: &str, offset: usize) -> bool {
    if is_source(kind) {
        offset == 0
    } else {
        crate::page::inline_edit::is_at_start(&utf16(text), offset)
    }
}

/// A block's text before and after a selection. In formatted text a mark the split crosses stays
/// intact on both sides; source text is split verbatim.
fn split_text(kind: &BlockKind, text: &str, start: usize, end: usize) -> (String, String) {
    if is_source(kind) {
        return (
            text[..byte_offset(text, start)].to_owned(),
            text[byte_offset(text, end)..].to_owned(),
        );
    }
    let (before, after) = crate::page::inline_edit::split(&utf16(text), (start, end));
    (
        String::from_utf16_lossy(&before),
        String::from_utf16_lossy(&after),
    )
}

/// Two blocks' text joined into one line, and the offset of the seam.
fn join_texts(first: &BlockKind, left: &str, second: &BlockKind, right: &str) -> (String, usize) {
    if is_source(first) || is_source(second) {
        return (format!("{left}{right}"), utf16_length(left));
    }
    let joined = crate::page::inline_edit::join(&utf16(left), &utf16(right));
    (String::from_utf16_lossy(&joined.text), joined.caret)
}

/// What a typed Markdown prefix turns a line into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shortcut {
    Divider,
    /// The kind, and whether a to-do starts ticked.
    Turn(Kind, bool),
}

/// The shortcut `text` holds before the caret (UTF-16 offset `caret`) in a block of `kind`.
/// Only plain lines and list items take shortcuts, and only plain lines take a divider.
fn shortcut(kind: &BlockKind, text: &str, caret: usize) -> Option<Shortcut> {
    let plain = matches!(kind, BlockKind::Paragraph { .. });
    if !plain && list_indent(kind).is_none() {
        return None;
    }
    let typed = &text[..byte_offset(text, caret)];
    if typed == "---" && plain {
        return Some(Shortcut::Divider);
    }
    let (to, checked) = match typed {
        "### " => (Kind::Heading(3), false),
        "## " => (Kind::Heading(2), false),
        "# " => (Kind::Heading(1), false),
        "[x] " | "[X] " => (Kind::Todo, true),
        "[] " | "[ ] " => (Kind::Todo, false),
        "- " | "* " | "+ " => (Kind::Bullet, false),
        "1. " | "1) " => (Kind::Numbered, false),
        "> " => (Kind::Quote, false),
        "```" => (Kind::Code, false),
        _ => return None,
    };
    Some(Shortcut::Turn(to, checked))
}

/// Whether `text`, typed into block `kind` with the caret at UTF-16 offset `caret`, holds a
/// Markdown prefix that converts the block (`Page::apply_shortcut`). Checked on every keystroke
/// so the line is saved only when it does.
pub fn takes_shortcut(kind: &BlockKind, text: &str, caret: usize) -> bool {
    shortcut(kind, text, caret).is_some()
}

fn outdent_or_unlist(page: &mut Page, index: usize) -> Landing {
    let kind = &mut page.blocks[index].kind;
    match kind {
        BlockKind::Bullet { indent, .. }
        | BlockKind::Numbered { indent, .. }
        | BlockKind::Todo { indent, .. }
            if *indent > 0 =>
        {
            *indent -= 1;
        }
        _ => *kind = convert(kind, Kind::Paragraph),
    }
    Some((index, Caret::Start))
}

impl Page {
    /// Enter: splits the block at the selection. An empty list item leaves the list instead, and
    /// Enter at the very start of a non-empty line opens a line above it.
    pub fn split(&mut self, index: usize, start: usize, end: usize) -> Landing {
        let block = self.blocks.get(index)?;
        let text = text_of(&block.kind)?.to_owned();
        let landing = if list_indent(&block.kind).is_some() && text.is_empty() {
            outdent_or_unlist(self, index)
        } else if start == end && !text.is_empty() && at_start(&block.kind, &text, start) {
            let above = continuation(&block.kind, "");
            self.blocks.insert(index, above);
            Some((index + 1, Caret::Start))
        } else {
            let (before, after) = split_text(&block.kind, &text, start, end.max(start));
            let next = continuation(&block.kind, &after);
            let _ = self.set_text(index, &before);
            self.blocks.insert(index + 1, next);
            Some((index + 1, Caret::Start))
        };
        self.derive();
        landing
    }

    /// Backspace at the start of a block: first undoes its formatting (outdent, then plain
    /// text), then joins it onto the line above.
    pub fn merge_backward(&mut self, index: usize) -> Landing {
        let kind = self.blocks.get(index)?.kind.clone();
        let landing = if list_indent(&kind).is_some() {
            outdent_or_unlist(self, index)
        } else if !matches!(kind, BlockKind::Paragraph { .. })
            && !is_source(&kind)
            && text_of(&kind).is_some()
        {
            self.blocks[index].kind = convert(&kind, Kind::Paragraph);
            Some((index, Caret::Start))
        } else if index == 0 {
            Some((index, Caret::Start))
        } else {
            let text = text_of(&kind).unwrap_or("").to_owned();
            let previous = self.blocks[index - 1].kind.clone();
            match (&previous, text_of(&previous)) {
                (BlockKind::Divider, _) => {
                    self.blocks.remove(index - 1);
                    Some((index - 1, Caret::Start))
                }
                // A table above: an empty line is removed, a non-empty one stays.
                (_, None) if text.is_empty() => {
                    self.blocks.remove(index);
                    None
                }
                (_, None) => Some((index, Caret::Start)),
                (_, Some(left)) => {
                    let (joined, seam) = join_texts(&previous, left, &kind, &text);
                    self.blocks.remove(index);
                    let _ = self.set_text(index - 1, &joined);
                    Some((index - 1, Caret::At(seam)))
                }
            }
        };
        self.derive();
        landing
    }

    /// Delete at the end of a block: pulls the next line's text up into this one.
    pub fn merge_forward(&mut self, index: usize) -> Landing {
        let text = text_of(&self.blocks.get(index)?.kind)?.to_owned();
        let next = self.blocks.get(index + 1)?.kind.clone();
        let end = utf16_length(&text);
        if matches!(next, BlockKind::Divider) {
            self.blocks.remove(index + 1);
        } else if let Some(right) = text_of(&next) {
            let kind = self.blocks[index].kind.clone();
            let (joined, seam) = join_texts(&kind, &text, &next, right);
            self.blocks.remove(index + 1);
            let _ = self.set_text(index, &joined);
            self.derive();
            return Some((index, Caret::At(seam)));
        }
        self.derive();
        Some((index, Caret::At(end)))
    }

    /// Tab and Shift+Tab on a list item; an item nests at most one level below the item above.
    pub fn indent(&mut self, index: usize, inward: bool) -> bool {
        let deepest = index
            .checked_sub(1)
            .and_then(|above| list_indent(&self.blocks[above].kind))
            .map_or(0, |indent| indent + 1);
        let Some(block) = self.blocks.get_mut(index) else {
            return false;
        };
        let (BlockKind::Bullet { indent, .. }
        | BlockKind::Numbered { indent, .. }
        | BlockKind::Todo { indent, .. }) = &mut block.kind
        else {
            return false;
        };
        let next = if inward {
            (*indent + 1).min(deepest).min(MAX_LIST_INDENT)
        } else {
            indent.saturating_sub(1)
        };
        let changed = next != *indent;
        *indent = next;
        self.derive();
        changed
    }

    /// Typing `# `, `- `, `1. `, `[] `, `> ` or ``` at the start of a plain line or list item
    /// converts it to that block; `---` on a plain line becomes a divider. `caret` is the caret's
    /// offset in the block's text. `None` when the text is not a shortcut.
    pub fn apply_shortcut(&mut self, index: usize, caret: usize) -> Landing {
        let kind = self.blocks.get(index)?.kind.clone();
        let text = text_of(&kind)?.to_owned();
        let cut = byte_offset(&text, caret);
        let rest = &text[cut..];
        match shortcut(&kind, &text, caret)? {
            Shortcut::Divider => {
                self.blocks[index].kind = BlockKind::Divider;
                self.blocks.insert(index + 1, paragraph(rest));
                self.derive();
                Some((index + 1, Caret::Start))
            }
            Shortcut::Turn(to, checked) => {
                let mut rest_only = kind.clone();
                if let Some(slot) = text_mut(&mut rest_only) {
                    *slot = rest.to_owned();
                }
                let mut converted = convert(&rest_only, to);
                if let BlockKind::Todo {
                    checked: ticked, ..
                } = &mut converted
                {
                    *ticked = checked;
                }
                self.blocks[index].kind = converted;
                self.derive();
                Some((index, Caret::Start))
            }
        }
    }

    /// Converts a line to the block chosen from the `/` menu. A divider or table replaces an empty
    /// line, or goes below a non-empty one.
    pub fn set_kind(&mut self, index: usize, to: Kind) -> Landing {
        let kind = self.blocks.get(index)?.kind.clone();
        if !matches!(to, Kind::Divider | Kind::Table) {
            self.blocks[index].kind = convert(&kind, to);
            self.derive();
            return Some((index, Caret::End));
        }
        let empty = text_of(&kind).unwrap_or("").is_empty();
        let inserted = Block {
            id: create_block_id(),
            kind: convert(
                &BlockKind::Paragraph {
                    text: String::new(),
                },
                to,
            ),
        };
        let at = if empty {
            self.blocks[index] = inserted;
            index
        } else {
            self.blocks.insert(index + 1, inserted);
            index + 1
        };
        self.blocks.insert(at + 1, paragraph(""));
        self.derive();
        // A new table starts in its first cell; after a divider, typing continues below it.
        Some((if to == Kind::Table { at } else { at + 1 }, Caret::Start))
    }

    /// ⇧⌘↑ and ⇧⌘↓: swaps blocks `first..=last` with the adjacent block. Returns the range's new
    /// first index, or `None` at the page's edge.
    pub fn move_by_step(&mut self, first: usize, last: usize, up: bool) -> Option<usize> {
        if first > last || last >= self.blocks.len() {
            return None;
        }
        let moved = if up {
            let neighbour = self.blocks.remove(first.checked_sub(1)?);
            self.blocks.insert(last, neighbour);
            first - 1
        } else {
            if last + 1 >= self.blocks.len() {
                return None;
            }
            let neighbour = self.blocks.remove(last + 1);
            self.blocks.insert(first, neighbour);
            first + 1
        };
        self.derive();
        Some(moved)
    }

    /// A drag by the ⋮⋮ handle: moves blocks `first..=last` just before or after block `target`.
    /// Returns the range's new first index, or `None` when nothing moves.
    pub fn move_next_to(
        &mut self,
        first: usize,
        last: usize,
        target: usize,
        after: bool,
    ) -> Option<usize> {
        if first > last
            || last >= self.blocks.len()
            || target >= self.blocks.len()
            || (first..=last).contains(&target)
        {
            return None;
        }
        let moved: Vec<Block> = self.blocks.drain(first..=last).collect();
        // The target's index among the remaining blocks.
        let target = if target > last {
            target - moved.len()
        } else {
            target
        };
        let at = if after { target + 1 } else { target };
        if at == first {
            self.blocks.splice(first..first, moved);
            return None;
        }
        self.blocks.splice(at..at, moved);
        self.derive();
        Some(at)
    }

    /// ⌘D: inserts copies of blocks `first..=last`, with new ids, right after them.
    pub fn duplicate(&mut self, first: usize, last: usize) -> bool {
        if first > last || last >= self.blocks.len() {
            return false;
        }
        let copies: Vec<Block> = self.blocks[first..=last]
            .iter()
            .map(|block| Block {
                id: create_block_id(),
                kind: block.kind.clone(),
            })
            .collect();
        self.blocks.splice(last + 1..last + 1, copies);
        self.derive();
        true
    }

    /// Deletes blocks `first..=last`, landing the caret at the end of the block above. A page
    /// always keeps one line.
    pub fn delete_range(&mut self, first: usize, last: usize) -> Landing {
        if first > last || last >= self.blocks.len() {
            return None;
        }
        self.blocks.drain(first..=last);
        let landing = if self.blocks.is_empty() {
            self.blocks.push(paragraph(""));
            Some((0, Caret::Start))
        } else if first > 0 {
            Some((first - 1, Caret::End))
        } else {
            Some((0, Caret::Start))
        };
        self.derive();
        landing
    }

    /// ⌘B and the other mark shortcuts on a selection in block `index`: adds the mark, or removes
    /// it when the whole selection has it. Returns the new selection, or `None` when there is no
    /// text to mark or the block holds source text.
    pub fn toggle_mark(
        &mut self,
        index: usize,
        start: usize,
        end: usize,
        mark: crate::page::inline_edit::Construct,
    ) -> Option<(usize, usize)> {
        let kind = &self.blocks.get(index)?.kind;
        if is_source(kind) {
            return None;
        }
        let text = utf16(text_of(kind)?);
        let (marked, from, until) =
            crate::page::inline_edit::toggle_mark(&text, (start.min(end), start.max(end)), mark)?;
        let _ = self.set_text(index, &String::from_utf16_lossy(&marked));
        Some((from, until))
    }

    /// ⌘K in block `index`: inside a link, moves the caret to the end of its address; elsewhere,
    /// makes the selection a link label and puts the caret where the address goes.
    pub fn link_selection(&mut self, index: usize, start: usize, end: usize) -> Option<usize> {
        let kind = &self.blocks.get(index)?.kind;
        if is_source(kind) {
            return None;
        }
        let text = utf16(text_of(kind)?);
        let range = (start.min(end), start.max(end));
        if let Some(link) = crate::page::inline_edit::find_link(&text, range) {
            return Some(link.href_end);
        }
        let linked = crate::page::inline_edit::link_range(&text, range, &[]);
        let _ = self.set_text(index, &String::from_utf16_lossy(&linked.text));
        Some(linked.caret)
    }

    /// Blocks `first..=last` as Markdown source, for copying.
    pub fn markdown_of(&self, first: usize, last: usize) -> String {
        match self
            .blocks
            .get(first..=last.min(self.blocks.len().saturating_sub(1)))
        {
            Some(blocks) if first <= last => crate::page::markdown_from_blocks(blocks),
            _ => String::new(),
        }
    }

    /// Pastes multi-line Markdown as blocks between the two halves of the caret's line. Returns
    /// where the caret lands, at the end of the last pasted block.
    pub fn paste_markdown(
        &mut self,
        index: usize,
        start: usize,
        end: usize,
        markdown: &str,
    ) -> Landing {
        let block = self.blocks.get(index)?.clone();
        let text = text_of(&block.kind)?.to_owned();
        let pasted = crate::page::blocks_from_markdown(markdown, &mut create_block_id);
        if pasted.is_empty() {
            return None;
        }
        let (before, after) = (
            &text[..byte_offset(&text, start)],
            &text[byte_offset(&text, end.max(start))..],
        );
        let mut replacement: Vec<Block> = Vec::new();
        if !before.is_empty() {
            let mut head = block.clone();
            if let Some(slot) = text_mut(&mut head.kind) {
                *slot = before.to_owned();
            }
            replacement.push(head);
        }
        let last_pasted = replacement.len() + pasted.len() - 1;
        replacement.extend(pasted);
        if !after.is_empty() {
            replacement.push(paragraph(after));
        }
        self.blocks.splice(index..=index, replacement);
        self.derive();
        Some((index + last_pasted, Caret::End))
    }

    /// Inserts pasted Markdown as blocks after block `after`, for a paste over selected blocks.
    pub fn insert_markdown_after(
        &mut self,
        after: usize,
        markdown: &str,
    ) -> Option<(usize, usize)> {
        let pasted = crate::page::blocks_from_markdown(markdown, &mut create_block_id);
        if pasted.is_empty() {
            return None;
        }
        let at = (after + 1).min(self.blocks.len());
        let count = pasted.len();
        self.blocks.splice(at..at, pasted);
        self.derive();
        Some((at, at + count - 1))
    }

    /// Clicking below the last block: keeps typing in an empty last line, adding one if needed.
    pub fn ensure_trailing_paragraph(&mut self) -> usize {
        let empty_last = matches!(self.blocks.last().map(|block| &block.kind), Some(BlockKind::Paragraph { text }) if text.is_empty());
        if !empty_last {
            self.blocks.push(paragraph(""));
            self.derive();
        }
        self.blocks.len() - 1
    }
}

/// A table block's rows and, when it was resized, its column widths.
type TableParts<'a> = (&'a mut Vec<Vec<String>>, &'a mut Option<Vec<f64>>);

/// A table column is never narrower than this, in points.
const MIN_TABLE_COLUMN_WIDTH: f64 = 60.0;

impl Page {
    fn table_mut(&mut self, index: usize) -> Option<TableParts<'_>> {
        match &mut self.blocks.get_mut(index)?.kind {
            BlockKind::Table {
                rows,
                column_widths_px,
            } => Some((rows, column_widths_px)),
            _ => None,
        }
    }

    /// Replaces one cell's text. Returns whether anything changed.
    pub fn set_table_cell(&mut self, index: usize, row: usize, column: usize, text: &str) -> bool {
        let Some(cell) = self
            .table_mut(index)
            .and_then(|(rows, _)| rows.get_mut(row)?.get_mut(column))
        else {
            return false;
        };
        // A cell is one line.
        let text = text.replace(['\n', '\r'], " ");
        if *cell == text {
            return false;
        }
        *cell = text;
        true
    }

    /// Adds an empty row after row `after`.
    pub fn add_table_row(&mut self, index: usize, after: usize) -> bool {
        let Some((rows, _)) = self.table_mut(index) else {
            return false;
        };
        let columns = rows.first().map_or(0, Vec::len);
        let at = (after + 1).min(rows.len());
        rows.insert(at, vec![String::new(); columns]);
        true
    }

    /// Removes a body row; the header row stays.
    pub fn delete_table_row(&mut self, index: usize, row: usize) -> bool {
        let Some((rows, _)) = self.table_mut(index) else {
            return false;
        };
        if row == 0 || row >= rows.len() {
            return false;
        }
        rows.remove(row);
        true
    }

    /// Adds an empty column at the end; a resized table gives it the narrowest existing width.
    pub fn add_table_column(&mut self, index: usize) -> bool {
        let Some((rows, widths)) = self.table_mut(index) else {
            return false;
        };
        rows.iter_mut().for_each(|row| row.push(String::new()));
        if let Some(widths) = widths {
            let narrowest = widths.iter().copied().fold(f64::INFINITY, f64::min);
            widths.push(narrowest.max(MIN_TABLE_COLUMN_WIDTH));
        }
        true
    }

    /// Removes a column; a table keeps at least one.
    pub fn delete_table_column(&mut self, index: usize, column: usize) -> bool {
        let Some((rows, widths)) = self.table_mut(index) else {
            return false;
        };
        let columns = rows.first().map_or(0, Vec::len);
        if columns <= 1 || column >= columns {
            return false;
        }
        rows.iter_mut().for_each(|row| {
            row.remove(column);
        });
        if let Some(widths) = widths {
            if column < widths.len() {
                widths.remove(column);
            }
        }
        true
    }
}

fn text_mut(kind: &mut BlockKind) -> Option<&mut String> {
    match kind {
        BlockKind::Paragraph { text }
        | BlockKind::Heading { text, .. }
        | BlockKind::Bullet { text, .. }
        | BlockKind::Numbered { text, .. }
        | BlockKind::Todo { text, .. }
        | BlockKind::Quote { text }
        | BlockKind::Code { text, .. }
        | BlockKind::Html { text } => Some(text),
        // Media has a caption instead of text, so no step joins or splits it.
        BlockKind::Media { .. }
        | BlockKind::Player { .. }
        | BlockKind::Divider
        | BlockKind::Table { .. } => None,
    }
}

/// A page snapshot stored as one undo step.
#[derive(Clone)]
struct Step {
    title: String,
    blocks: Vec<Block>,
}

impl Step {
    fn of(page: &Page) -> Self {
        Self {
            title: page.title.clone(),
            blocks: page.blocks.clone(),
        }
    }

    fn restore(self, page: &mut Page) {
        page.title = self.title;
        page.blocks = self.blocks;
        page.derive();
    }
}

/// Page-level undo. Blocks are split, merged and retyped across many text fields, which a single
/// field's undo cannot track, so the page keeps its own steps.
#[derive(Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    /// Records the page as it is before a change.
    pub fn record(&mut self, page: &Page) {
        self.undo.push(Step::of(page));
        if self.undo.len() > MAX_HISTORY_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Drops the last recorded step when its change turned out to change nothing.
    pub fn forget_last(&mut self) {
        self.undo.pop();
    }

    pub fn undo(&mut self, page: &mut Page) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(Step::of(page));
        previous.restore(page);
        true
    }

    pub fn redo(&mut self, page: &mut Page) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(Step::of(page));
        next.restore(page);
        true
    }

    /// Forgets every step, for when the file changed on disk.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slash_query_is_one_short_word_after_a_slash_that_starts_a_word() {
        assert_eq!(find_slash_query("/", 1), Some((0, String::new())));
        assert_eq!(find_slash_query("/head", 5), Some((0, "head".into())));
        assert_eq!(find_slash_query("Some /to", 8), Some((5, "to".into())));
        assert_eq!(find_slash_query("a/b", 3), None, "a slash inside a word");
        assert_eq!(find_slash_query("/two words", 10), None);
        assert_eq!(
            find_slash_query("/abcdefghijklmnopqrstuvwxy", 26),
            None,
            "25 is too long"
        );
        assert_eq!(find_slash_query("é /x", 4), Some((2, "x".into())));
        assert_eq!(find_slash_query("no slash", 8), None);
    }

    fn page(markdown: &str) -> Page {
        let mut next = 0;
        Page::from_file("Notes", markdown, "en", &mut || {
            next += 1;
            format!("b{next}")
        })
    }

    fn written(page: &Page) -> String {
        page.to_file()
    }

    #[test]
    fn a_dragged_range_lands_next_to_the_block_it_is_dropped_on() {
        let mut page = page("a\n\nb\n\nc\n\nd\n");
        let words = |page: &Page| {
            page.blocks
                .iter()
                .filter_map(|block| text_of(&block.kind))
                .collect::<String>()
        };
        assert_eq!(page.move_next_to(0, 1, 3, true), Some(2));
        assert_eq!(words(&page), "cdab");
        assert_eq!(page.move_next_to(3, 3, 0, false), Some(0));
        assert_eq!(words(&page), "bcda");
        // Dropped on itself, or where it already is: nothing moves.
        assert_eq!(page.move_next_to(1, 2, 2, true), None);
        assert_eq!(page.move_next_to(1, 2, 0, true), None);
        assert_eq!(page.move_next_to(1, 2, 3, false), None);
        assert_eq!(words(&page), "bcda");
    }

    #[test]
    fn enter_splits_a_line_continues_a_list_and_leaves_an_empty_item() {
        let mut page = page("Hello world\n\n- one\n");
        assert_eq!(page.split(0, 5, 6), Some((1, Caret::Start)));
        assert_eq!(written(&page), "Hello\n\nworld\n\n- one\n");
        // At the very start of a non-empty line, a line opens above it.
        assert_eq!(page.split(1, 0, 0), Some((2, Caret::Start)));
        assert_eq!(page.text(1), Some(""));
        // A list item continues as one; an empty item leaves the list.
        assert_eq!(page.split(3, 3, 3), Some((4, Caret::Start)));
        assert!(matches!(page.blocks[4].kind, BlockKind::Bullet { .. }));
        assert_eq!(page.split(4, 0, 0), Some((4, Caret::Start)));
        assert!(matches!(page.blocks[4].kind, BlockKind::Paragraph { .. }));
    }

    #[test]
    fn backspace_undoes_formatting_before_it_joins_lines() {
        let mut page = page("First\n\n## Second\n\n---\n\nThird\n");
        assert_eq!(page.merge_backward(1), Some((1, Caret::Start)));
        assert!(
            matches!(page.blocks[1].kind, BlockKind::Paragraph { .. }),
            "a heading becomes plain first"
        );
        assert_eq!(page.merge_backward(1), Some((0, Caret::At(5))));
        assert_eq!(page.text(0), Some("FirstSecond"));
        // A divider above is removed; the line stays.
        assert_eq!(page.merge_backward(2), Some((1, Caret::Start)));
        assert_eq!(written(&page), "FirstSecond\n\nThird\n");
        assert_eq!(page.merge_forward(0), Some((0, Caret::At(11))));
        assert_eq!(written(&page), "FirstSecondThird\n");
    }

    #[test]
    fn a_typed_markdown_prefix_turns_the_line_into_that_block() {
        let mut page = page("x\n");
        for (typed, expected) in [
            ("## Title", "## Title\n"),
            ("- item", "- item\n"),
            ("1. item", "1. item\n"),
            ("[] task", "- [ ] task\n"),
            ("[x] done", "- [x] done\n"),
            ("> said", "> said\n"),
        ] {
            page.blocks[0].kind = BlockKind::Paragraph {
                text: typed.to_owned(),
            };
            let prefix = typed.find(' ').expect("prefix") + 1;
            assert_eq!(
                page.apply_shortcut(0, prefix),
                Some((0, Caret::Start)),
                "{typed}"
            );
            assert_eq!(written(&page), expected);
        }
        page.blocks[0].kind = BlockKind::Paragraph {
            text: "---".to_owned(),
        };
        assert_eq!(page.apply_shortcut(0, 3), Some((1, Caret::Start)));
        assert!(matches!(page.blocks[0].kind, BlockKind::Divider));
        page.blocks[1].kind = BlockKind::Paragraph {
            text: "plain words".to_owned(),
        };
        assert_eq!(page.apply_shortcut(1, 5), None);
    }

    #[test]
    fn the_slash_menu_filters_by_word_and_sets_the_kind() {
        assert_eq!(filter_commands("").len(), 11);
        let labels = |query: &str| -> Vec<&str> {
            filter_commands(query)
                .into_iter()
                .map(|index| strings::text("en", COMMANDS[index].label))
                .collect()
        };
        assert_eq!(
            labels("h"),
            ["Heading 1", "Heading 2", "Heading 3", "Divider"]
        );
        assert_eq!(
            labels("list"),
            ["Bulleted list", "Numbered list", "To-do list"]
        );
        assert_eq!(labels("check"), ["To-do list"]);
        assert_eq!(commands_heading("en"), "Basic blocks");

        let mut page = page("Words\n");
        assert_eq!(page.set_kind(0, Kind::Quote), Some((0, Caret::End)));
        assert_eq!(written(&page), "> Words\n");
        // A divider goes below a non-empty line, and typing continues below it.
        assert_eq!(page.set_kind(0, Kind::Divider), Some((2, Caret::Start)));
        assert_eq!(page.blocks.len(), 3);
        assert!(matches!(page.blocks[1].kind, BlockKind::Divider));
        // A table takes an empty line's place.
        assert_eq!(page.set_kind(2, Kind::Table), Some((2, Caret::Start)));
        assert!(
            matches!(&page.blocks[2].kind, BlockKind::Table { rows, .. } if rows.len() == 3 && rows[0].len() == 3)
        );
    }

    #[test]
    fn list_items_nest_one_below_the_item_above() {
        let mut page = page("- a\n- b\n- c\n");
        assert!(
            !page.indent(0, true),
            "the first item has nothing to nest under"
        );
        assert!(page.indent(1, true));
        assert!(
            !page.indent(1, true),
            "one level below the item above at most"
        );
        assert!(page.indent(2, true));
        assert!(page.indent(2, true));
        assert_eq!(written(&page), "- a\n  - b\n    - c\n");
        assert!(page.indent(2, false));
    }

    #[test]
    fn blocks_move_copy_and_go_and_a_page_keeps_one_line() {
        let mut page = page("a\n\nb\n\nc\n");
        assert_eq!(page.move_by_step(1, 2, true), Some(0));
        assert_eq!(written(&page), "b\n\nc\n\na\n");
        assert_eq!(page.move_by_step(0, 0, true), None);
        assert_eq!(page.move_by_step(0, 0, false), Some(1));
        assert!(page.duplicate(0, 0));
        assert_eq!(written(&page), "c\n\nc\n\nb\n\na\n");
        assert_ne!(page.blocks[0].id, page.blocks[1].id);
        assert_eq!(page.delete_range(1, 2), Some((0, Caret::End)));
        assert_eq!(page.delete_range(0, 1), Some((0, Caret::Start)));
        assert_eq!(page.blocks.len(), 1);
        assert_eq!(page.ensure_trailing_paragraph(), 0);
    }

    #[test]
    fn a_table_takes_cells_rows_and_columns_and_keeps_its_header() {
        let mut page = page("| Name | Value |\n| --- | --- |\n| alpha | 1 |\n");
        assert!(page.set_table_cell(0, 1, 1, "2"));
        assert!(
            !page.set_table_cell(0, 1, 1, "2"),
            "the same words are no change"
        );
        assert!(page.add_table_row(0, 1));
        assert!(page.set_table_cell(0, 2, 0, "beta\nline"));
        assert!(page.add_table_column(0));
        assert!(page.set_table_cell(0, 0, 2, "Note"));
        assert_eq!(
            written(&page),
            "| Name | Value | Note |\n| --- | --- | --- |\n| alpha | 2 |  |\n| beta line |  |  |\n"
        );
        assert!(!page.delete_table_row(0, 0), "the header row stays");
        assert!(page.delete_table_row(0, 1));
        assert!(page.delete_table_column(0, 1));
        assert!(page.delete_table_column(0, 1));
        assert!(!page.delete_table_column(0, 0), "a table keeps one column");
        assert_eq!(written(&page), "| Name |\n| --- |\n| beta line |\n");
        assert!(!page.add_table_row(5, 0));
    }

    #[test]
    fn several_pasted_lines_become_blocks_between_the_halves_of_the_line() {
        let mut page = page("Hello world\n\nEnd\n");
        assert_eq!(
            page.paste_markdown(0, 5, 6, "## Title\n\n- one\n- two"),
            Some((3, Caret::End))
        );
        assert_eq!(
            written(&page),
            "Hello\n\n## Title\n\n- one\n- two\n\nworld\n\nEnd\n"
        );
        // Copying blocks gives their Markdown source.
        assert_eq!(page.markdown_of(1, 3), "## Title\n\n- one\n- two");
        assert_eq!(page.markdown_of(3, 1), "");
        // Pasted over selected blocks, it goes in after them.
        assert_eq!(page.insert_markdown_after(5, "> said"), Some((6, 6)));
        assert!(written(&page).ends_with("End\n\n> said\n"));
        assert_eq!(page.paste_markdown(0, 0, 0, "   "), None);
    }

    #[test]
    fn marks_stay_whole_through_cuts_joins_and_toggles() {
        use crate::page::inline_edit::Construct;
        let mut page = page("a **boldword** here\n");
        // Enter inside the bold run: both halves stay bold.
        assert_eq!(page.split(0, 8, 8), Some((1, Caret::Start)));
        assert_eq!(written(&page), "a **bold**\n\n**word** here\n");
        // Backspace at the start of the second line joins them and the run is one again.
        assert_eq!(page.merge_backward(1), Some((0, Caret::At(8))));
        assert_eq!(written(&page), "a **boldword** here\n");
        // Hidden opening marks still count as the start of the line.
        let mut bold = super::tests::page("**all bold**\n");
        assert_eq!(bold.split(0, 2, 2), Some((1, Caret::Start)));
        assert_eq!(bold.text(0), Some(""));

        assert_eq!(
            page.toggle_mark(0, 15, 19, Construct::Italic),
            Some((15, 21))
        );
        assert_eq!(written(&page), "a **boldword** *here*\n");
        assert_eq!(
            page.toggle_mark(0, 16, 20, Construct::Italic),
            Some((15, 19))
        );
        assert_eq!(written(&page), "a **boldword** here\n");
        assert_eq!(page.link_selection(0, 15, 19), Some(22));
        assert_eq!(written(&page), "a **boldword** [here]()\n");
        // An address typed where the caret landed makes it a link; ⌘K inside it then goes to
        // the end of that address.
        page.set_text(0, "a **boldword** [here](https://x.y)");
        assert_eq!(page.link_selection(0, 17, 17), Some(33));
        assert_eq!(
            written(&page),
            "a **boldword** [here](https://x.y)\n",
            "nothing is rewritten"
        );
    }

    #[test]
    fn undo_and_redo_walk_the_recorded_steps() {
        let mut page = page("one\n");
        let mut history = History::default();
        history.record(&page);
        page.set_text(0, "two");
        history.record(&page);
        page.set_kind(0, Kind::Heading(2));
        assert!(history.undo(&mut page));
        assert_eq!(written(&page), "two\n");
        assert!(history.undo(&mut page));
        assert_eq!(written(&page), "one\n");
        assert!(!history.undo(&mut page));
        assert!(history.redo(&mut page));
        assert!(history.redo(&mut page));
        assert_eq!(written(&page), "## two\n");
        // A new change ends the way forward.
        assert!(history.undo(&mut page));
        history.record(&page);
        page.set_text(0, "three");
        assert!(!history.redo(&mut page));
    }
}
