//! Edits to a block's text that keep its inline Markdown intact. The text is raw Markdown, but
//! people edit it formatted with the marks hidden, so an edit must never leave half a `**` pair
//! behind. Recorded cases pin the rules (`tests/inline_edit_parity.rs`).
//!
//! Offsets are UTF-16 code units into the raw text, as a shell's text view counts them, so the
//! text is held here as those units.

use crate::board::card::{marked_face, Font, CODE_PADDING};
use std::collections::HashMap;

/// A piece of syntax that renders nothing itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Construct {
    Bold,
    Italic,
    Strike,
    Code,
    Link,
    Image,
    Escape,
}

impl Construct {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bold => "bold",
            Self::Italic => "italic",
            Self::Strike => "strike",
            Self::Code => "code",
            Self::Link => "link",
            Self::Image => "image",
            Self::Escape => "escape",
        }
    }

    /// Marks that stop being marks when a space sits just inside them: `** bold**`.
    fn space_sensitive(self) -> bool {
        matches!(self, Self::Bold | Self::Italic | Self::Strike)
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Bold => "**",
            Self::Italic => "*",
            Self::Strike => "~~",
            _ => "`",
        }
    }
}

/// A token of a block's raw text, either visible text or syntax. Tokens are in raw order and
/// cover every character. An escape closes with an empty marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Text {
        start: usize,
        end: usize,
        marks: Vec<Construct>,
        /// The token renders something even when empty: an image.
        image: bool,
        /// The text is a link's label.
        in_link: bool,
    },
    Open {
        start: usize,
        end: usize,
        construct: usize,
        kind: Construct,
    },
    Close {
        start: usize,
        end: usize,
        construct: usize,
        kind: Construct,
    },
}

impl Token {
    fn span(&self) -> (usize, usize) {
        match self {
            Self::Text { start, end, .. }
            | Self::Open { start, end, .. }
            | Self::Close { start, end, .. } => (*start, *end),
        }
    }

    fn is_marker(&self) -> bool {
        !matches!(self, Self::Text { .. })
    }

    fn is_shown(&self) -> bool {
        matches!(self, Self::Text { start, end, image, .. } if end > start || *image)
    }
}

type Units = [u16];

fn unit(raw: &Units, index: usize) -> Option<u16> {
    raw.get(index).copied()
}

fn is(raw: &Units, index: usize, character: char) -> bool {
    unit(raw, index) == Some(character as u16)
}

/// One code unit as a `char`, unless it is half a surrogate pair.
fn as_char(code: Option<u16>) -> Option<char> {
    code.and_then(|code| char::from_u32(u32::from(code)))
}

fn is_word(code: Option<u16>) -> bool {
    as_char(code).is_some_and(char::is_alphanumeric)
}

/// Matches JavaScript's `\s`.
fn is_space(code: Option<u16>) -> bool {
    as_char(code).is_some_and(|c| c.is_whitespace() || c == '\u{feff}')
}

fn find(raw: &Units, needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() || from > raw.len() || raw.len() < needle.len() {
        return None;
    }
    (from..=raw.len() - needle.len()).find(|&index| raw[index..index + needle.len()] == *needle)
}

fn find_char(raw: &Units, character: char, from: usize) -> Option<usize> {
    find(raw, &[character as u16], from)
}

const ESCAPABLE: &str = "\\`*_~[]()#+-.!>|";

/// The closing marker's index for a pair whose content starts at `from`, if it closes.
fn closing_marker(raw: &Units, marker: &[u16], from: usize, end: usize) -> Option<usize> {
    let close = find(raw, marker, from)?;
    if close + marker.len() > end || close == from {
        return None;
    }
    if is_space(unit(raw, from))
        || is_space(close.checked_sub(1).and_then(|before| unit(raw, before)))
    {
        return None;
    }
    Some(close)
}

#[derive(Clone)]
struct Scope {
    marks: Vec<Construct>,
    in_link: bool,
}

impl Scope {
    fn with(&self, mark: Construct) -> Scope {
        let mut scope = self.clone();
        if !scope.marks.contains(&mark) {
            scope.marks.push(mark);
        }
        scope
    }
}

/// A matcher's result, before anything is emitted.
#[derive(Clone, Copy)]
enum Found {
    Escape {
        index: usize,
    },
    Code {
        index: usize,
        close: usize,
    },
    Double {
        index: usize,
        close: usize,
        kind: Construct,
    },
    Italic {
        index: usize,
        close: usize,
    },
    Image {
        index: usize,
        alt_end: usize,
        src_end: usize,
    },
    Link {
        index: usize,
        label_end: usize,
        href_end: usize,
    },
}

struct ParserState<'a> {
    raw: &'a Units,
    tokens: Vec<Token>,
    next_construct: usize,
}

impl ParserState<'_> {
    fn plain(&mut self, from: usize, until: usize, scope: &Scope) {
        if until > from {
            self.tokens.push(Token::Text {
                start: from,
                end: until,
                marks: scope.marks.clone(),
                image: false,
                in_link: scope.in_link,
            });
        }
    }

    fn range(&mut self, start: usize, end: usize, scope: &Scope) {
        let (mut plain_start, mut index) = (start, start);
        while index < end {
            let Some(found) = self.find_at(index, end, scope) else {
                index += 1;
                continue;
            };
            self.plain(plain_start, index, scope);
            index = self.emit(found, scope);
            plain_start = index;
        }
        self.plain(plain_start, end, scope);
    }

    /// The construct that starts at `index` and ends before `end`, if one does. Images and links
    /// do not nest in a link's label.
    fn find_at(&self, index: usize, end: usize, scope: &Scope) -> Option<Found> {
        let raw = self.raw;
        let found = escape_at(raw, index, end)
            .or_else(|| code_at(raw, index, end))
            .or_else(|| double_at(raw, index, end))
            .or_else(|| italic_at(raw, index, end));
        if found.is_some() || scope.in_link {
            return found;
        }
        image_at(raw, index, end).or_else(|| link_at(raw, index, end))
    }

    fn open(&mut self, kind: Construct, start: usize, end: usize) -> usize {
        let construct = self.next_construct;
        self.next_construct += 1;
        self.tokens.push(Token::Open {
            start,
            end,
            construct,
            kind,
        });
        construct
    }

    fn close(&mut self, kind: Construct, construct: usize, start: usize, end: usize) {
        self.tokens.push(Token::Close {
            start,
            end,
            construct,
            kind,
        });
    }

    /// Emits what was found and returns where it ends.
    fn emit(&mut self, found: Found, scope: &Scope) -> usize {
        match found {
            Found::Escape { index } => {
                let construct = self.open(Construct::Escape, index, index + 1);
                self.plain(index + 1, index + 2, scope);
                self.close(Construct::Escape, construct, index + 2, index + 2);
                index + 2
            }
            Found::Code { index, close } => {
                let construct = self.open(Construct::Code, index, index + 1);
                self.plain(index + 1, close, &scope.with(Construct::Code));
                self.close(Construct::Code, construct, close, close + 1);
                close + 1
            }
            Found::Double { index, close, kind } => {
                let construct = self.open(kind, index, index + 2);
                self.range(index + 2, close, &scope.with(kind));
                self.close(kind, construct, close, close + 2);
                close + 2
            }
            Found::Italic { index, close } => {
                let construct = self.open(Construct::Italic, index, index + 1);
                self.range(index + 1, close, &scope.with(Construct::Italic));
                self.close(Construct::Italic, construct, close, close + 1);
                close + 1
            }
            Found::Image {
                index,
                alt_end,
                src_end,
            } => {
                let construct = self.open(Construct::Image, index, index + 2);
                self.tokens.push(Token::Text {
                    start: index + 2,
                    end: alt_end,
                    marks: scope.marks.clone(),
                    image: true,
                    in_link: false,
                });
                self.close(Construct::Image, construct, alt_end, src_end + 1);
                src_end + 1
            }
            Found::Link {
                index,
                label_end,
                href_end,
            } => {
                let construct = self.open(Construct::Link, index, index + 1);
                let inside = Scope {
                    marks: scope.marks.clone(),
                    in_link: true,
                };
                self.range(index + 1, label_end, &inside);
                self.close(Construct::Link, construct, label_end, href_end + 1);
                href_end + 1
            }
        }
    }
}

/// An escape: a backslash before a character Markdown gives meaning to.
fn escape_at(raw: &Units, index: usize, end: usize) -> Option<Found> {
    let escapes = is(raw, index, '\\')
        && index + 1 < end
        && as_char(unit(raw, index + 1)).is_some_and(|c| ESCAPABLE.contains(c));
    escapes.then_some(Found::Escape { index })
}

/// Inline code: a backtick, content, and the next backtick.
fn code_at(raw: &Units, index: usize, end: usize) -> Option<Found> {
    if !is(raw, index, '`') {
        return None;
    }
    let close = find_char(raw, '`', index + 1)?;
    (close > index + 1 && close < end).then_some(Found::Code { index, close })
}

/// A doubled marker: `**` or `__` for bold, `~~` for struck.
fn double_at(raw: &Units, index: usize, end: usize) -> Option<Found> {
    let first = unit(raw, index)?;
    if unit(raw, index + 1) != Some(first) {
        return None;
    }
    let kind = match as_char(Some(first)) {
        Some('*' | '_') => Construct::Bold,
        Some('~') => Construct::Strike,
        _ => return None,
    };
    let mut close = closing_marker(raw, &[first, first], index + 2, end)?;
    // In a run like `***`, the pair closes last so an inner `*` closes first.
    while close + 2 < end && unit(raw, close + 2) == Some(first) {
        close += 1;
    }
    Some(Found::Double { index, close, kind })
}

/// A single `*` or `_`. `_` only opens and closes at a word edge, so snake_case stays plain.
fn italic_at(raw: &Units, index: usize, end: usize) -> Option<Found> {
    let marker = unit(raw, index)?;
    let underscore = match as_char(Some(marker)) {
        Some('_') => true,
        Some('*') => false,
        _ => return None,
    };
    if unit(raw, index + 1) == Some(marker) {
        return None;
    }
    let touches_word = |beside: Option<u16>| underscore && is_word(beside);
    if touches_word(index.checked_sub(1).and_then(|before| unit(raw, before))) {
        return None;
    }
    let close = closing_marker(raw, &[marker], index + 1, end)?;
    (!touches_word(unit(raw, close + 1))).then_some(Found::Italic { index, close })
}

/// `![alt](src)`.
fn image_at(raw: &Units, index: usize, end: usize) -> Option<Found> {
    if !(is(raw, index, '!') && is(raw, index + 1, '[')) {
        return None;
    }
    let alt_end = find(raw, &[']' as u16, '(' as u16], index + 2)?;
    let src_end = find_char(raw, ')', alt_end + 2)?;
    (src_end > alt_end + 2 && src_end < end).then_some(Found::Image {
        index,
        alt_end,
        src_end,
    })
}

/// `[label](href)`, with a label.
fn link_at(raw: &Units, index: usize, end: usize) -> Option<Found> {
    if !is(raw, index, '[') {
        return None;
    }
    let label_end = find(raw, &[']' as u16, '(' as u16], index + 1)?;
    let href_end = find_char(raw, ')', label_end + 2)?;
    (label_end > index + 1 && href_end > label_end + 2 && href_end < end).then_some(Found::Link {
        index,
        label_end,
        href_end,
    })
}

/// Tokenizes `raw` into visible text and the syntax around it.
pub fn tokens(raw: &Units) -> Vec<Token> {
    let mut parser = ParserState {
        raw,
        tokens: Vec::new(),
        next_construct: 0,
    };
    parser.range(
        0,
        raw.len(),
        &Scope {
            marks: Vec::new(),
            in_link: false,
        },
    );
    parser.tokens
}

/// The result of an edit: the new raw text and where the caret sits in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub text: Vec<u16>,
    pub caret: usize,
}

/// A construct as an edit sees it. Pieces refer to one by its index in the edit's list.
#[derive(Clone, Copy)]
struct Seen {
    kind: Construct,
    /// The edit removed or inserted text inside it.
    changed: bool,
    /// The range covers the whole construct, so its markers go with its text.
    removed: bool,
}

/// The raw text as editable pieces: visible text, a construct's two markers, and the caret.
#[derive(Clone)]
enum Piece {
    Text(Vec<u16>),
    Open(Vec<u16>, usize),
    Close(Vec<u16>, usize),
    Caret,
}

fn trim_end(text: &Units) -> &Units {
    let mut end = text.len();
    while end > 0 && is_space(Some(text[end - 1])) {
        end -= 1;
    }
    &text[..end]
}

fn trim_start(text: &Units) -> &Units {
    let mut start = 0;
    while start < text.len() && is_space(Some(text[start])) {
        start += 1;
    }
    &text[start..]
}

/// The marker the range is strictly inside, which only a revealed link address allows.
fn marker_around(tokens: &[Token], start: usize, end: usize) -> Option<&Token> {
    tokens.iter().find(|token| {
        let (from, until) = token.span();
        token.is_marker() && from < start && end < until
    })
}

/// The pieces left after removing a range from a block's text, with `inserted` and the caret
/// placed where the range began. A construct the range partly covers keeps both markers; one it
/// covers entirely is removed with its text. This text's constructs are appended to `seen`.
fn cut_pieces(
    raw: &Units,
    tokens: &[Token],
    range: (usize, usize),
    inserted: &Units,
    seen: &mut Vec<Seen>,
) -> Vec<Piece> {
    let mut cutter = Cutter {
        raw,
        start: range.0,
        end: range.1,
        seen,
        pieces: Vec::new(),
        open: Vec::new(),
        places: HashMap::new(),
        close_ends: tokens
            .iter()
            .filter_map(|token| match token {
                Token::Close { construct, end, .. } => Some((*construct, *end)),
                _ => None,
            })
            .collect(),
        insertion: None,
    };
    for token in tokens {
        match token {
            Token::Text { start, end, .. } => cutter.cut_text(*start, *end),
            Token::Open {
                start,
                end,
                construct,
                kind,
            } => cutter.cut_open((*start, *end), *construct, *kind),
            Token::Close {
                start,
                end,
                construct,
                ..
            } => cutter.cut_close((*start, *end), *construct),
        }
    }
    cutter.finish(inserted)
}

/// Cuts the range `start..end` out of a block's tokens, one token at a time.
struct Cutter<'a> {
    raw: &'a Units,
    start: usize,
    end: usize,
    seen: &'a mut Vec<Seen>,
    pieces: Vec<Piece>,
    /// The constructs open around the token being cut, by index in `seen`.
    open: Vec<usize>,
    /// Each token construct's index in `seen`, and where each closing marker ends.
    places: HashMap<usize, usize>,
    close_ends: HashMap<usize, usize>,
    /// Where the typed text goes, and the constructs it lands inside.
    insertion: Option<(usize, Vec<usize>)>,
}

impl Cutter<'_> {
    /// Puts the insertion point here, unless an earlier one was found.
    fn mark_insertion(&mut self) {
        if self.insertion.is_none() {
            self.insertion = Some((self.pieces.len(), self.open.clone()));
        }
    }

    /// A marker at or after the range's start goes after the inserted text.
    fn mark_insertion_before(&mut self, (from, until): (usize, usize)) {
        if until > self.start && from >= self.start {
            self.mark_insertion();
        }
    }

    /// The range began inside this marker or before it ends, so the inserted text goes after it.
    fn mark_insertion_after(&mut self, until: usize) {
        if until > self.start {
            self.mark_insertion();
        }
    }

    /// Visible text from `from` to `until`: what lies outside the range stays, and constructs
    /// around removed text are marked changed.
    fn cut_text(&mut self, from: usize, until: usize) {
        let (start, end) = (self.start, self.end);
        if from < start {
            self.pieces
                .push(Piece::Text(self.raw[from..until.min(start)].to_vec()));
        }
        self.mark_insertion_after(until);
        if from.max(start) < until.min(end) {
            for place in &self.open {
                self.seen[*place].changed = true;
            }
        }
        if until > end {
            self.pieces
                .push(Piece::Text(self.raw[from.max(end)..until].to_vec()));
        }
    }

    /// An opening marker: kept unless the range covers its whole construct.
    fn cut_open(&mut self, (from, until): (usize, usize), construct: usize, kind: Construct) {
        let (start, end) = (self.start, self.end);
        self.mark_insertion_before((from, until));
        let close_end = self.close_ends.get(&construct).copied().unwrap_or(until);
        let place = self.seen.len();
        let removed = start < end && from >= start && close_end <= end;
        self.seen.push(Seen {
            kind,
            changed: false,
            removed,
        });
        self.places.insert(construct, place);
        self.open.push(place);
        if !removed {
            self.pieces
                .push(Piece::Open(self.raw[from..until].to_vec(), place));
        }
        self.mark_insertion_after(until);
    }

    /// A closing marker: kept when its opening one was.
    fn cut_close(&mut self, (from, until): (usize, usize), construct: usize) {
        self.mark_insertion_before((from, until));
        self.open.pop();
        let kept = self
            .places
            .get(&construct)
            .copied()
            .filter(|place| !self.seen[*place].removed);
        if let Some(place) = kept {
            self.pieces
                .push(Piece::Close(self.raw[from..until].to_vec(), place));
        }
        self.mark_insertion_after(until);
    }

    /// The pieces with `inserted` and the caret where the range began.
    fn finish(self, inserted: &Units) -> Vec<Piece> {
        let Cutter {
            mut pieces,
            seen,
            insertion,
            ..
        } = self;
        let (mut index, around) = insertion.unwrap_or((pieces.len(), Vec::new()));
        // Nothing goes between a backslash and the character it escapes.
        let escape = match index.checked_sub(1).and_then(|before| pieces.get(before)) {
            Some(Piece::Open(_, place)) if seen[*place].kind == Construct::Escape => Some(*place),
            _ => None,
        };
        if escape.is_some() {
            index -= 1;
        }
        let mut added = Vec::new();
        if !inserted.is_empty() {
            for place in around.into_iter().filter(|place| Some(*place) != escape) {
                seen[place].changed = true;
            }
            added.push(Piece::Text(inserted.to_vec()));
        }
        added.push(Piece::Caret);
        pieces.splice(index..index, added);
        pieces
    }
}

fn marker_index(pieces: &[Piece], closing: bool, place: usize) -> Option<usize> {
    pieces.iter().position(|piece| match piece {
        Piece::Open(_, at) => !closing && *at == place,
        Piece::Close(_, at) => closing && *at == place,
        _ => false,
    })
}

/// Moves trailing spaces in a mark's content to after its closing marker.
fn move_trailing_space_out(pieces: &mut Vec<Piece>, place: usize) {
    let Some(mut close) = marker_index(pieces, true, place) else {
        return;
    };
    let mut space_from: Option<usize> = None;
    let mut scan = close;
    while scan > 0 {
        scan -= 1;
        let text = match &pieces[scan] {
            Piece::Caret => continue,
            Piece::Text(text) => text.clone(),
            _ => break,
        };
        let words = trim_end(&text).len();
        if words == text.len() {
            break;
        }
        if words > 0 {
            pieces.splice(
                scan..=scan,
                [
                    Piece::Text(text[..words].to_vec()),
                    Piece::Text(text[words..].to_vec()),
                ],
            );
            close += 1;
            space_from = Some(scan + 1);
            break;
        }
        space_from = Some(scan);
    }
    let Some(space_from) = space_from else {
        return;
    };
    let moved: Vec<Piece> = pieces.drain(space_from..close).collect();
    pieces.splice(space_from + 1..space_from + 1, moved);
}

/// Moves leading spaces in a mark's content to before its opening marker.
fn move_leading_space_out(pieces: &mut Vec<Piece>, place: usize) {
    let Some(open) = marker_index(pieces, false, place) else {
        return;
    };
    let mut space_until: Option<usize> = None;
    let mut scan = open + 1;
    while scan < pieces.len() {
        let text = match &pieces[scan] {
            Piece::Caret => {
                scan += 1;
                continue;
            }
            Piece::Text(text) => text.clone(),
            _ => break,
        };
        let words = trim_start(&text).len();
        if words == text.len() {
            break;
        }
        if words > 0 {
            let space = text.len() - words;
            pieces.splice(
                scan..=scan,
                [
                    Piece::Text(text[..space].to_vec()),
                    Piece::Text(text[space..].to_vec()),
                ],
            );
            space_until = Some(scan + 1);
            break;
        }
        space_until = Some(scan + 1);
        scan += 1;
    }
    let Some(space_until) = space_until else {
        return;
    };
    let moved: Vec<Piece> = pieces.drain(open + 1..space_until).collect();
    pieces.splice(open..open, moved);
}

/// Removes a construct's markers once it has no content left.
fn remove_empty_construct(pieces: &mut Vec<Piece>, place: usize) {
    let (Some(open), Some(close)) = (
        marker_index(pieces, false, place),
        marker_index(pieces, true, place),
    ) else {
        return;
    };
    if open < close
        && pieces[open + 1..close]
            .iter()
            .any(|piece| matches!(piece, Piece::Text(_)))
    {
        return;
    }
    pieces.remove(close);
    pieces.remove(open);
}

/// Joins a mark that closes right where the same mark opens again, at the caret or beside a
/// changed construct: `**a****b**` would otherwise show its asterisks.
fn join_touching_marks(pieces: &mut Vec<Piece>, seen: &mut [Seen]) {
    let mut index = 0;
    while index < pieces.len() {
        match touching_marks(pieces, seen, index) {
            Some(touching) => {
                join_marks(pieces, seen, touching);
                index = 0;
            }
            None => index += 1,
        }
    }
}

/// A closing marker and the opening marker of the same mark right after it.
#[derive(Clone, Copy)]
struct Touching {
    close: usize,
    open: usize,
    /// The indices in `seen` of the mark that closes and the one that reopens.
    joined: usize,
    opened: usize,
}

/// The touching marks to join at piece `index`: it closes a mark, the same mark reopens right
/// after it (past the caret), and the two meet at the caret or one of them changed.
fn touching_marks(pieces: &[Piece], seen: &[Seen], index: usize) -> Option<Touching> {
    let Piece::Close(close_text, joined) = &pieces[index] else {
        return None;
    };
    let joined = *joined;
    if !seen[joined].kind.space_sensitive() {
        return None;
    }
    let mut next = index + 1;
    while matches!(pieces.get(next), Some(Piece::Caret)) {
        next += 1;
    }
    let Some(Piece::Open(text, opened)) = pieces.get(next) else {
        return None;
    };
    let opened = *opened;
    if text != close_text || seen[opened].kind != seen[joined].kind {
        return None;
    }
    let at_caret = next > index + 1;
    (at_caret || seen[joined].changed || seen[opened].changed).then_some(Touching {
        close: index,
        open: next,
        joined,
        opened,
    })
}

/// Removes the touching markers, so the first mark extends to where the second closes.
fn join_marks(pieces: &mut Vec<Piece>, seen: &mut [Seen], touching: Touching) {
    seen[touching.joined].changed = true;
    for piece in pieces.iter_mut() {
        match piece {
            Piece::Close(_, place) if *place == touching.opened => *place = touching.joined,
            _ => {}
        }
    }
    pieces.remove(touching.open);
    pieces.remove(touching.close);
}

fn tidy(pieces: &mut Vec<Piece>, seen: &mut [Seen]) {
    // A construct closes before its enclosing ones, so this runs innermost first.
    let changed: Vec<usize> = pieces
        .iter()
        .filter_map(|piece| match piece {
            Piece::Close(_, place) if seen[*place].changed => Some(*place),
            _ => None,
        })
        .collect();
    for place in changed {
        if seen[place].kind.space_sensitive() {
            move_trailing_space_out(pieces, place);
            move_leading_space_out(pieces, place);
        }
        remove_empty_construct(pieces, place);
    }
    join_touching_marks(pieces, seen);
}

fn write(pieces: &[Piece]) -> Edit {
    let mut text: Vec<u16> = Vec::new();
    let mut caret: Option<usize> = None;
    for piece in pieces {
        match piece {
            Piece::Caret => {
                caret.get_or_insert(text.len());
            }
            Piece::Text(units) | Piece::Open(units, _) | Piece::Close(units, _) => {
                text.extend_from_slice(units)
            }
        }
    }
    let caret = caret.unwrap_or(text.len());
    Edit { text, caret }
}

/// Replaces the range with `inserted`, as typing, deleting and pasting do. Inserted text is
/// Markdown, so `**word**` typed by hand becomes bold once it closes.
pub fn replace_range(raw: &Units, range: (usize, usize), inserted: &Units) -> Edit {
    let start = range.0.min(range.1).min(raw.len());
    let end = range.0.max(range.1).min(raw.len());
    let parsed = tokens(raw);
    if marker_around(&parsed, start, end).is_some() {
        // Inside a revealed link address, the raw source is edited directly.
        let mut text = raw[..start].to_vec();
        text.extend_from_slice(inserted);
        text.extend_from_slice(&raw[end..]);
        return Edit {
            text,
            caret: start + inserted.len(),
        };
    }
    let mut seen = Vec::new();
    let mut pieces = cut_pieces(raw, &parsed, (start, end), inserted, &mut seen);
    tidy(&mut pieces, &mut seen);
    write(&pieces)
}

/// The text inside the range, with every mark it crosses closed again.
pub fn slice(raw: &Units, range: (usize, usize)) -> Vec<u16> {
    let tail = replace_range(raw, (range.1, raw.len()), &[]).text;
    let until = range.0.min(tail.len());
    replace_range(&tail, (0, until), &[]).text
}

/// The text before and after the range, each with its marks intact (Enter).
pub fn split(raw: &Units, range: (usize, usize)) -> (Vec<u16>, Vec<u16>) {
    (
        replace_range(raw, (range.0, raw.len()), &[]).text,
        replace_range(raw, (0, range.1), &[]).text,
    )
}

fn read_pieces(raw: &Units, seen: &mut Vec<Seen>) -> Vec<Piece> {
    cut_pieces(raw, &tokens(raw), (0, 0), &[], seen)
        .into_iter()
        .filter(|piece| !matches!(piece, Piece::Caret))
        .collect()
}

/// Joins two texts into one line with the caret at the seam (Backspace at a line's start).
pub fn join(left: &Units, right: &Units) -> Edit {
    let mut seen = Vec::new();
    let mut pieces = read_pieces(left, &mut seen);
    pieces.push(Piece::Caret);
    pieces.extend(read_pieces(right, &mut seen));
    join_touching_marks(&mut pieces, &mut seen);
    write(&pieces)
}

fn remove_markers(raw: &Units, kind: Construct) -> Vec<u16> {
    let mut text = Vec::new();
    for token in tokens(raw) {
        let keep = match &token {
            Token::Text { .. } => true,
            Token::Open { kind: found, .. } | Token::Close { kind: found, .. } => *found != kind,
        };
        if keep {
            let (start, end) = token.span();
            text.extend_from_slice(&raw[start..end]);
        }
    }
    text
}

fn shown_text(raw: &Units) -> Vec<u16> {
    let mut text = Vec::new();
    for token in tokens(raw) {
        if let Token::Text { start, end, .. } = token {
            text.extend_from_slice(&raw[start..end]);
        }
    }
    text
}

fn wrap_words(text: &Units, marker: &Units) -> Vec<u16> {
    let words = trim_start(trim_end(text));
    if words.is_empty() {
        return text.to_vec();
    }
    let lead = text.len() - trim_start(text).len();
    let trail = trim_end(text).len();
    let mut wrapped = text[..lead].to_vec();
    wrapped.extend_from_slice(marker);
    wrapped.extend_from_slice(words);
    wrapped.extend_from_slice(marker);
    wrapped.extend_from_slice(&text[trail..]);
    wrapped
}

/// Whether all visible text carries the mark.
fn wholly_marked(raw: &Units, mark: Construct) -> bool {
    tokens(raw).iter().all(|token| match token {
        Token::Text {
            start, end, marks, ..
        } => trim_start(&raw[*start..*end]).is_empty() || marks.contains(&mark),
        _ => true,
    })
}

fn contains(haystack: &Units, needle: &Units) -> bool {
    find(haystack, needle, 0).is_some()
}

/// ⌘B and the other mark shortcuts on a selection: adds the mark, or removes it when the whole
/// selection has it. Returns the text and new selection, or `None` when there is nothing to
/// mark.
pub fn toggle_mark(
    raw: &Units,
    range: (usize, usize),
    mark: Construct,
) -> Option<(Vec<u16>, usize, usize)> {
    let before = slice(raw, (0, range.0));
    let selected = slice(raw, range);
    let after = slice(raw, (range.1, raw.len()));
    if trim_start(&shown_text(&selected)).is_empty() {
        return None;
    }
    let marker: Vec<u16> = mark.marker().encode_utf16().collect();
    let unmarked = remove_markers(&selected, mark);
    let marked = if wholly_marked(&selected, mark) {
        unmarked
    } else if mark == Construct::Code {
        // Code shows its text verbatim, so nothing inside it stays formatted.
        let shown = shown_text(&selected);
        if contains(&shown, &marker) {
            return None;
        }
        let mut code = marker.clone();
        code.extend_from_slice(&shown);
        code.extend_from_slice(&marker);
        code
    } else {
        wrap_words(&unmarked, &marker)
    };
    let head = join(&before, &marked);
    let whole = join(&head.text, &after);
    Some((whole.text, head.caret, whole.caret))
}

/// Where a link's address starts and ends inside `[label](address)`, and just past its `)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkSpan {
    pub href_start: usize,
    pub href_end: usize,
    pub end: usize,
}

/// The link whose label holds the whole range.
pub fn find_link(raw: &Units, range: (usize, usize)) -> Option<LinkSpan> {
    let parsed = tokens(raw);
    for token in &parsed {
        let Token::Open {
            end: open_end,
            construct,
            kind: Construct::Link,
            ..
        } = token
        else {
            continue;
        };
        let close = parsed.iter().find_map(|other| match other {
            Token::Close {
                start,
                end,
                construct: closing,
                ..
            } if closing == construct => Some((*start, *end)),
            _ => None,
        });
        let Some((close_start, close_end)) = close else {
            continue;
        };
        if *open_end > range.0 || range.1 > close_start {
            continue;
        }
        return Some(LinkSpan {
            href_start: close_start + 2,
            href_end: close_end - 1,
            end: close_end,
        });
    }
    None
}

/// Links the range to `url` as `[text](url)`. Inside an existing link it changes that link's
/// address. With an empty `url` the caret lands where the address goes.
pub fn link_range(raw: &Units, range: (usize, usize), url: &Units) -> Edit {
    let address = trim_start(trim_end(url));
    if let (Some(existing), false) = (find_link(raw, range), address.is_empty()) {
        let mut text = raw[..existing.href_start].to_vec();
        text.extend_from_slice(address);
        text.extend_from_slice(&raw[existing.href_end..]);
        return Edit {
            text,
            caret: existing.href_start + address.len() + 1,
        };
    }
    let mut linked = slice(raw, (0, range.0));
    let label = remove_markers(&slice(raw, range), Construct::Link);
    let after = slice(raw, (range.1, raw.len()));
    linked.push('[' as u16);
    linked.extend_from_slice(&label);
    linked.extend("](".encode_utf16());
    linked.extend_from_slice(address);
    let caret = if address.is_empty() {
        linked.len()
    } else {
        linked.len() + 1
    };
    linked.push(')' as u16);
    linked.extend_from_slice(&after);
    Edit {
        text: linked,
        caret,
    }
}

/// Whether nothing visible precedes the offset, so the caret is at the line's start.
pub fn is_at_start(raw: &Units, offset: usize) -> bool {
    let parsed = tokens(raw);
    marker_around(&parsed, offset, offset).is_none()
        && !parsed
            .iter()
            .any(|token| token.is_shown() && token.span().0 < offset)
}

/// Whether nothing visible follows the offset, so the caret is at the line's end.
pub fn is_at_end(raw: &Units, offset: usize) -> bool {
    let parsed = tokens(raw);
    marker_around(&parsed, offset, offset).is_none()
        && !parsed
            .iter()
            .any(|token| token.is_shown() && token.span().1 > offset)
}

/// Whether the offset is in text that carries the mark.
pub fn has_mark_at(raw: &Units, offset: usize, mark: Construct) -> bool {
    tokens(raw)
        .iter()
        .any(|token| matches!(token, Token::Text { start, end, marks, .. } if marks.contains(&mark) && *start <= offset && offset <= *end))
}

/// The end of the link whose address contains the offset, or `None` outside any address.
pub fn revealed_link_end(raw: &Units, offset: usize) -> Option<usize> {
    match marker_around(&tokens(raw), offset, offset) {
        Some(Token::Close {
            kind: Construct::Link,
            end,
            ..
        }) => Some(*end),
        _ => None,
    }
}

/// Moves an offset out of a hidden marker, to the side its text is on.
pub fn snap_offset(raw: &Units, offset: usize) -> usize {
    let clamped = offset.min(raw.len());
    match marker_around(&tokens(raw), clamped, clamped) {
        None
        | Some(Token::Close {
            kind: Construct::Link,
            ..
        }) => clamped,
        Some(Token::Open { end, .. }) => *end,
        Some(Token::Close { start, .. }) => *start,
        Some(Token::Text { .. }) => clamped,
    }
}

/// The character a collapsed Backspace or Delete removes, skipping hidden marks.
pub fn delete_range(raw: &Units, caret: usize, backward: bool) -> Option<(usize, usize)> {
    let parsed = tokens(raw);
    let mut shown = parsed.iter().filter(|token| token.is_shown());
    if backward {
        let (start, end) = shown.rfind(|token| token.span().0 < caret)?.span();
        let end = caret.min(end);
        // A character outside the BMP is a surrogate pair; remove both units.
        let pair = end >= start + 2 && (0xdc00..=0xdfff).contains(&raw[end - 1]);
        return Some((end - if pair { 2 } else { 1 }, end));
    }
    let (start, end) = shown.find(|token| token.span().1 > caret)?.span();
    let start = caret.max(start);
    let pair = start + 2 <= end && (0xd800..=0xdbff).contains(&raw[start]);
    Some((start, start + if pair { 2 } else { 1 }))
}

/// Each image in `raw`, `![alt](src)` from its `!` to its `)`. While a line is being edited, an
/// image is drawn as a single unit the caret never enters, never as Markdown.
pub fn image_spans(raw: &Units) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut open = None;
    for token in tokens(raw) {
        match token {
            Token::Open {
                start,
                construct,
                kind: Construct::Image,
                ..
            } => open = Some((construct, start)),
            Token::Close {
                end,
                construct,
                kind: Construct::Image,
                ..
            } => {
                if let Some((opened, start)) = open.take() {
                    if opened == construct {
                        spans.push((start, end));
                    }
                }
            }
            _ => {}
        }
    }
    spans
}

/// Where the caret goes for a press at `offset` on the line being edited: after the image it
/// hits, or `None` when it hits none. Typing then follows the image instead of going into its
/// hidden alt text.
pub fn press_on_image(raw: &Units, offset: usize) -> Option<usize> {
    image_spans(raw)
        .into_iter()
        .find(|&(start, end)| start <= offset && offset < end)
        .map(|(_, end)| end)
}

/// Where a caret placed at `offset` may rest while a line is edited: never inside a hidden mark
/// (`snap_offset`) or inside an image, which it skips as a whole.
pub fn snap_caret(raw: &Units, offset: usize) -> usize {
    let clamped = offset.min(raw.len());
    match image_spans(raw)
        .into_iter()
        .find(|&(start, end)| start < clamped && clamped < end)
    {
        Some((_, end)) => end,
        None => snap_offset(raw, clamped),
    }
}

/// What a collapsed Backspace (`backward`) or Delete removes while a line is edited: one visible
/// character (`delete_range`), or a whole image when it is nearest on that side, never its
/// hidden alt text one character at a time.
pub fn delete_unit(raw: &Units, caret: usize, backward: bool) -> Option<(usize, usize)> {
    let images = image_spans(raw);
    let character = delete_range(raw, caret, backward);
    // Any character of an image removes the whole image.
    let around = character.and_then(|(start, _)| {
        images
            .iter()
            .copied()
            .find(|&(from, until)| from <= start && start < until)
    });
    if around.is_some() {
        return around;
    }
    // The nearest image on that side, when nothing visible lies between it and the caret.
    let image = if backward {
        images
            .iter()
            .rev()
            .find(|&&(_, end)| end <= caret)
            .filter(|image| character.is_none_or(|(_, end)| image.0 >= end))
    } else {
        images
            .iter()
            .find(|&&(start, _)| start >= caret)
            .filter(|image| character.is_none_or(|(start, _)| image.1 <= start))
    };
    image.copied().or(character)
}

/// A piece of a block's raw text while the block is edited.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    /// Visible text, with its marks, and whether it is a link's label.
    Text {
        start: usize,
        end: usize,
        marks: Vec<Construct>,
        link: bool,
    },
    /// Syntax that is hidden.
    Mark { start: usize, end: usize },
    /// A link's `](address)`: hidden unless the caret is inside it.
    Address { start: usize, end: usize },
    /// A whole `![alt](src)`, drawn as the image, never as Markdown.
    Image { start: usize, end: usize },
}

fn parts(raw: &Units) -> Vec<Part> {
    let images = image_spans(raw);
    tokens(raw)
        .iter()
        .filter_map(|token| {
            let (from, until) = token.span();
            if let Some(&(start, end)) = images
                .iter()
                .find(|&&(start, end)| start <= from && until <= end)
            {
                return (from == start && token.is_marker()).then_some(Part::Image { start, end });
            }
            Some(match token {
                Token::Text {
                    start,
                    end,
                    marks,
                    in_link,
                    ..
                } => Part::Text {
                    start: *start,
                    end: *end,
                    marks: marks.clone(),
                    link: *in_link,
                },
                Token::Close {
                    start,
                    end,
                    kind: Construct::Link,
                    ..
                } => Part::Address {
                    start: *start,
                    end: *end,
                },
                Token::Open { start, end, .. } | Token::Close { start, end, .. } => Part::Mark {
                    start: *start,
                    end: *end,
                },
            })
        })
        .collect()
}

/// How each character of a line being edited is shown. Every character is in exactly one
/// stretch; anything a stretch leaves unset uses the line's own type.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    pub start: usize,
    pub end: usize,
    /// Present in the text but takes no room; its glyphs are omitted from the line.
    pub hidden: bool,
    /// One of an image's source characters, which are not text.
    pub image: bool,
    /// The character standing in for image `n` (of `Style::images`), 1 pt narrower than the line,
    /// so nothing shares the image's line.
    pub stand_in: Option<usize>,
    /// The character ends its line and takes no room, so an image gets its own line.
    pub line_break: bool,
    /// Space after the character: inline code's padding, or a stand-in's width.
    pub kern: Option<f64>,
    /// Its font, when it differs from the line's.
    pub face: Option<Font>,
    pub code: bool,
    pub strike: bool,
    /// A link's label, underlined at 40% of the text colour.
    pub underline: bool,
    /// A link's address while the caret is inside it, shown in the muted colour.
    pub muted: bool,
}

/// A line being edited, as displayed: its stretches and its images, each `![alt](src)` from its
/// `!` to its `)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub spans: Vec<Shown>,
    pub images: Vec<(usize, usize)>,
}

/// How a block's raw text displays while edited, in `size` and `weight` type on a `width`-wide
/// line with `selection` selected. Marks are hidden; bold, italic, struck, code and link text
/// use the same fonts as the page at rest. An image draws as it does at rest, on its own line:
/// its `[` stands in for it, its `)` ends its line, and its `!` ends the previous line when
/// anything visible precedes it there. A link's address shows only while the caret is strictly
/// inside it. The text stays the raw Markdown, so every offset indexes into it.
pub fn style(raw: &Units, selection: (usize, usize), size: f64, weight: f64, width: f64) -> Style {
    let mut styler = Styler {
        units: vec![Shown::default(); raw.len()],
        images: Vec::new(),
        code: Vec::new(),
    };
    for piece in parts(raw) {
        let (start, end) = piece.span();
        if end <= start {
            continue;
        }
        match piece {
            Part::Image { .. } => styler.show_image(start, end, width),
            Part::Mark { .. } => styler.units[start..end]
                .iter_mut()
                .for_each(|unit| unit.hidden = true),
            Part::Address { .. } => styler.show_address(start, end, selection),
            Part::Text { marks, link, .. } => {
                styler.show_words((start, end), &marks, link, (size, weight));
            }
        }
    }
    styler.break_before_images(raw);
    styler.pad_code();
    Style {
        spans: stretches(styler.units),
        images: styler.images,
    }
}

impl Part {
    fn span(&self) -> (usize, usize) {
        match self {
            Part::Text { start, end, .. }
            | Part::Mark { start, end }
            | Part::Address { start, end }
            | Part::Image { start, end } => (*start, *end),
        }
    }
}

/// Work-in-progress styling for each character of a line, plus the images and inline code found
/// so far.
struct Styler {
    units: Vec<Shown>,
    images: Vec<(usize, usize)>,
    code: Vec<(usize, usize)>,
}

impl Styler {
    /// Draws an image on its own line: its `[` stands in for it, 1 pt narrower than the line, and
    /// its `)` ends the line.
    fn show_image(&mut self, start: usize, end: usize, width: f64) {
        for unit in &mut self.units[start..end] {
            unit.hidden = true;
            unit.image = true;
        }
        let stand_in = &mut self.units[start + 1];
        stand_in.hidden = false;
        stand_in.stand_in = Some(self.images.len());
        stand_in.face = Some(Font::sans(1.0, Font::REGULAR));
        stand_in.kern = Some(width - 1.0);
        let last = &mut self.units[end - 1];
        last.hidden = false;
        last.line_break = true;
        self.images.push((start, end));
    }

    /// Shows a link's address, muted, only while the selection is strictly inside it.
    fn show_address(&mut self, start: usize, end: usize, selection: (usize, usize)) {
        let inside = selection.0 > start && selection.1 < end;
        for unit in &mut self.units[start..end] {
            if inside {
                unit.muted = true;
            } else {
                unit.hidden = true;
            }
        }
    }

    /// Styles text with `marks` in the page's resting font at `size` and `weight`, underlined when
    /// it is a link's label.
    fn show_words(
        &mut self,
        (start, end): (usize, usize),
        marks: &[Construct],
        link: bool,
        (size, weight): (f64, f64),
    ) {
        let has = |mark| marks.contains(&mark);
        let face = marked_face(
            size,
            weight,
            has(Construct::Code),
            has(Construct::Bold),
            has(Construct::Italic),
        );
        for unit in &mut self.units[start..end] {
            unit.face = Some(face);
            unit.code = has(Construct::Code);
            unit.strike = has(Construct::Strike);
            unit.underline = link;
        }
        if has(Construct::Code) {
            self.code.push((start, end));
        }
    }

    /// An image's `!` ends the previous line when anything visible precedes it there.
    fn break_before_images(&mut self, raw: &Units) {
        for &(start, _) in &self.images {
            let Some(before) = last_shown_before(&self.units, start) else {
                continue;
            };
            if !self.units[before].line_break && raw[before] != u16::from(b'\n') {
                self.units[start].hidden = false;
                self.units[start].line_break = true;
            }
        }
    }

    /// Adds inline code padding after the visible character before the code and after its last
    /// character.
    fn pad_code(&mut self) {
        for &(start, end) in &self.code {
            if let Some(before) = last_shown_before(&self.units, start) {
                self.units[before].kern = Some(CODE_PADDING);
            }
            self.units[end - 1].kern = Some(CODE_PADDING);
        }
    }
}

/// The last visible character before `at`, skipping hidden ones.
fn last_shown_before(units: &[Shown], at: usize) -> Option<usize> {
    (0..at).rev().find(|&index| !units[index].hidden)
}

/// Merges runs of identically styled characters into stretches.
fn stretches(units: Vec<Shown>) -> Vec<Shown> {
    let mut spans: Vec<Shown> = Vec::new();
    for (index, unit) in units.into_iter().enumerate() {
        match spans.last_mut() {
            Some(last)
                if Shown {
                    start: last.start,
                    end: last.end,
                    ..unit.clone()
                } == *last =>
            {
                last.end = index + 1
            }
            _ => spans.push(Shown {
                start: index,
                end: index + 1,
                ..unit
            }),
        }
    }
    spans
}

/// A selection includes an image entirely or not at all, as a browser selects a non-editable
/// element: an end inside one moves to its outer edge.
pub fn widen_to_images(raw: &Units, start: usize, end: usize) -> (usize, usize) {
    let (mut from, mut until) = (start, end);
    for (lower, upper) in image_spans(raw) {
        if lower < from && from < upper {
            from = lower;
        }
        if lower < until && until < upper {
            until = upper;
        }
    }
    (from, until)
}

/// Where the caret goes for one press of ← or →: past one visible character or a whole image,
/// skipping the hidden marks around it. `None` at the line's end in that direction.
pub fn step(raw: &Units, caret: usize, forward: bool) -> Option<usize> {
    // Inside a revealed link address, the caret steps through the raw source.
    if let Some(end) = revealed_link_end(raw, caret) {
        let _ = end;
        return if forward {
            Some(caret + 1)
        } else {
            caret.checked_sub(1)
        };
    }
    delete_unit(raw, caret, !forward).map(|(start, end)| if forward { end } else { start })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn the_caret_steps_over_hidden_marks_one_shown_character_at_a_time() {
        let raw = units("a **b** c");
        // From before the space-and-stars, one press lands after "b"'s opening stars' word.
        assert_eq!(step(&raw, 2, true), Some(5));
        assert_eq!(
            step(&raw, 5, true),
            Some(8),
            "past the closing stars and the space"
        );
        assert_eq!(step(&raw, 5, false), Some(4));
        assert_eq!(step(&raw, 4, false), Some(1));
        assert_eq!(step(&raw, 0, false), None);
        assert_eq!(step(&raw, 9, true), None);
    }

    #[test]
    fn an_image_is_one_thing_while_its_line_is_typed_in() {
        let raw = units("ab ![gradient](gradient.png)cd");
        assert_eq!(image_spans(&raw), vec![(3, 28)]);
        // Shown as one image line, never as its alt text or marks.
        assert_eq!(
            parts(&raw),
            [
                Part::Text {
                    start: 0,
                    end: 3,
                    marks: vec![],
                    link: false
                },
                Part::Image { start: 3, end: 28 },
                Part::Text {
                    start: 28,
                    end: 30,
                    marks: vec![],
                    link: false
                }
            ]
        );
        // A press anywhere on it puts the caret after it; a press on text is not affected.
        for offset in 3..28 {
            assert_eq!(press_on_image(&raw, offset), Some(28), "{offset}");
        }
        assert_eq!(press_on_image(&raw, 2), None);
        assert_eq!(press_on_image(&raw, 28), None);
        // A caret never rests inside it.
        assert_eq!(snap_offset(&raw, 5), 5);
        assert_eq!(snap_caret(&raw, 5), 28);
        assert_eq!(snap_caret(&raw, 3), 3);
        assert_eq!(snap_caret(&raw, 28), 28);
        // ← and → pass it whole; Backspace after it and Delete before it take it whole.
        assert_eq!(step(&raw, 28, false), Some(3));
        assert_eq!(step(&raw, 3, true), Some(28));
        assert_eq!(delete_unit(&raw, 28, true), Some((3, 28)));
        assert_eq!(delete_unit(&raw, 3, false), Some((3, 28)));
        assert_eq!(delete_unit(&raw, 3, true), Some((2, 3)));
        assert_eq!(delete_unit(&raw, 29, true), Some((28, 29)));
        assert_eq!(replace_range(&raw, (3, 28), &[]).text, units("ab cd"));
    }

    #[test]
    fn an_image_with_no_alt_text_is_still_taken_whole() {
        let raw = units("a![](clip.mp4)");
        assert_eq!(image_spans(&raw), vec![(1, 14)]);
        assert_eq!(delete_unit(&raw, 14, true), Some((1, 14)));
        assert_eq!(delete_unit(&raw, 1, true), Some((0, 1)));
        assert_eq!(delete_unit(&raw, 1, false), Some((1, 14)));
        assert_eq!(step(&raw, 0, true), Some(1));
        assert_eq!(step(&raw, 1, true), Some(14));
        // Two images side by side, and an image in a link or between marks.
        let raw = units("![a](a.png)![b](b.png)");
        assert_eq!(image_spans(&raw), vec![(0, 11), (11, 22)]);
        assert_eq!(delete_unit(&raw, 22, true), Some((11, 22)));
        assert_eq!(delete_unit(&raw, 11, true), Some((0, 11)));
        // Inside a link's label an image is plain text.
        let raw = units("[see ![in](x.png)](https://example.com) **![b](b.png)**");
        assert_eq!(image_spans(&raw), vec![(42, 53)]);
        assert_eq!(press_on_image(&raw, 45), Some(53));
        assert_eq!(delete_unit(&raw, 55, true), Some((42, 53)));
    }

    #[test]
    fn a_line_is_styled_from_its_tokens() {
        let text = |start, end, marks: &[Construct], link| Part::Text {
            start,
            end,
            marks: marks.to_vec(),
            link,
        };
        assert_eq!(
            parts(&units("a **b** [c](d)")),
            [
                text(0, 2, &[], false),
                Part::Mark { start: 2, end: 4 },
                text(4, 5, &[Construct::Bold], false),
                Part::Mark { start: 5, end: 7 },
                text(7, 8, &[], false),
                Part::Mark { start: 8, end: 9 },
                text(9, 10, &[], true),
                Part::Address { start: 10, end: 14 },
            ]
        );
    }

    fn face(size: f64, weight: f64) -> Option<Font> {
        Some(Font::sans(size, weight))
    }

    #[test]
    fn marks_hide_and_words_take_the_faces_they_have_at_rest() {
        let shown = style(&units("a **b** *c* `d` ~~e~~"), (0, 0), 15.0, 400.0, 600.0).spans;
        let at = |offset: usize| {
            shown
                .iter()
                .find(|span| span.start <= offset && offset < span.end)
                .expect("every character is shown somehow")
                .clone()
        };
        assert!(
            at(2).hidden && at(3).hidden && at(5).hidden,
            "the stars take no room"
        );
        assert_eq!(at(4).face, face(15.0, 600.0), "bold at 600");
        assert_eq!(
            at(0).face,
            face(15.0, 400.0),
            "plain words keep the line's weight"
        );
        assert_eq!(
            at(9).face,
            Some(Font {
                slanted: true,
                ..Font::sans(15.0, Font::REGULAR)
            }),
            "italic leans"
        );
        let code = at(13);
        assert_eq!((code.face, code.code), (Some(Font::mono(12.75)), true));
        assert!(at(18).strike);
        // Code padding goes after the last visible character before it and after its own last.
        assert_eq!(at(11).kern, Some(4.8), "the space before the backtick");
        assert_eq!(at(12).kern, None, "the hidden backtick takes none");
        assert_eq!(at(13).kern, Some(4.8));
        // The stretches cover the line in order, each ending where the next starts.
        assert_eq!(shown.first().map(|span| span.start), Some(0));
        assert!(shown.windows(2).all(|pair| pair[0].end == pair[1].start));
        assert_eq!(
            shown.last().map(|span| span.end),
            Some(units("a **b** *c* `d` ~~e~~").len())
        );
    }

    #[test]
    fn a_links_address_shows_only_while_the_caret_is_inside_it() {
        let raw = units("[c](dd)");
        let address = |selection| {
            style(&raw, selection, 15.0, 400.0, 600.0)
                .spans
                .into_iter()
                .find(|span| span.start <= 4 && 4 < span.end)
                .expect("shown")
        };
        assert!(address((5, 5)).muted && !address((5, 5)).hidden);
        assert!(address((2, 2)).hidden, "at its edge it is hidden");
        assert!(address((7, 7)).hidden);
        assert!(
            address((1, 7)).hidden,
            "a selection reaching past it hides it"
        );
        let label = style(&raw, (0, 0), 15.0, 400.0, 600.0)
            .spans
            .into_iter()
            .find(|span| span.start <= 1 && 1 < span.end)
            .expect("shown");
        assert!(label.underline);
    }

    #[test]
    fn an_image_is_a_line_of_its_own_stood_for_by_its_bracket() {
        let raw = units("ab ![g](g.png)cd");
        let shown = style(&raw, (0, 0), 15.0, 400.0, 600.0);
        assert_eq!(shown.images, vec![(3, 14)]);
        let at = |offset: usize| {
            shown
                .spans
                .iter()
                .find(|span| span.start <= offset && offset < span.end)
                .expect("shown")
                .clone()
        };
        // The `!` ends the text before it, the `[` stands in for the image, the `)` ends its line.
        assert!(at(3).line_break && !at(3).hidden && at(3).image);
        assert_eq!(
            (at(4).stand_in, at(4).kern, at(4).face),
            (Some(0), Some(599.0), face(1.0, 400.0))
        );
        assert!(at(5).hidden && at(12).hidden && at(5).image);
        assert!(at(13).line_break && !at(13).hidden);
        assert!(!at(14).image && !at(14).hidden);
        // At the start of a line, or after a line end, nothing before it needs ending.
        let first = style(&units("![g](g.png)"), (0, 0), 15.0, 400.0, 600.0);
        assert!(first.spans[0].hidden && !first.spans[0].line_break);
        let after_break = style(&units("a\n![g](g.png)"), (0, 0), 15.0, 400.0, 600.0);
        let bang = after_break
            .spans
            .iter()
            .find(|span| span.start <= 2 && 2 < span.end)
            .expect("shown");
        assert!(bang.hidden && !bang.line_break);
        // Two images side by side: the second's `!` follows the first's `)`, which ends its line.
        let pair = style(&units("![a](a.png)![b](b.png)"), (0, 0), 15.0, 400.0, 600.0);
        let second = pair
            .spans
            .iter()
            .find(|span| span.start <= 11 && 11 < span.end)
            .expect("shown");
        assert!(second.hidden && !second.line_break);
    }

    #[test]
    fn a_selection_takes_an_image_whole() {
        let raw = units("ab ![g](g.png)cd");
        assert_eq!(widen_to_images(&raw, 0, 5), (0, 14));
        assert_eq!(widen_to_images(&raw, 6, 16), (3, 16));
        assert_eq!(widen_to_images(&raw, 3, 14), (3, 14), "its own edges stay");
        assert_eq!(widen_to_images(&raw, 0, 2), (0, 2));
        assert_eq!(widen_to_images(&raw, 5, 8), (3, 14));
    }
}
