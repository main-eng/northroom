//! Line breaking that matches Core Text's typesetter (`CTTypesetterSuggestLineBreak`). A line
//! runs to the first character past its width. Spaces there hang at the end; otherwise the line
//! ends at the last break Unicode allows (ICU's rules, as Core Text uses: after spaces, after a
//! hyphen or dash, not inside quotation marks), or between characters when no word fits. A line
//! end always breaks. Widths come from `typeface`, so every shell sets the same lines and draws
//! each glyph where it was measured.

use crate::board::card::{Font, Measured, Run, Words};
use crate::text::typeface::{self, Placed};
use icu_segmenter::options::LineBreakOptions;
use icu_segmenter::LineSegmenter;
use std::ops::Range;

/// Core Text's default tab stops: twelve, every 28 pt. A tab past the last takes no room.
const DEFAULT_TAB_INTERVAL: f64 = 28.0;
const DEFAULT_TAB_STOPS: f64 = 12.0;
/// How far text may overrun a line and still fit, to absorb rounding in summed advances.
const FIT_SLACK: f64 = 1e-9;

/// A run of text in one face (`board::card::Run`), or an empty gap `space` wide that follows the
/// previous letter, or leads when first. `tracking` is space after each glyph.
#[derive(Clone, Copy, Debug)]
pub struct Stretch<'a> {
    pub text: &'a str,
    pub font: Font,
    pub space: f64,
    pub tracking: f64,
}

/// The part of `stretch` on a line: bytes `start..end` (empty for a gap), at `x`, `width` wide,
/// with glyphs positioned relative to the piece.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub stretch: usize,
    pub start: usize,
    pub end: usize,
    pub x: f64,
    pub width: f64,
    pub glyphs: Vec<Placed>,
}

/// One set line: its pieces in order and its width, including trailing spaces.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub pieces: Vec<Piece>,
    pub width: f64,
}

/// The stretches for a card's runs (`Words::runs`).
pub fn stretches(runs: &[Run]) -> Vec<Stretch<'_>> {
    runs.iter()
        .map(|run| match run {
            Run::Words { text, style } => Stretch {
                text,
                font: style.font,
                space: 0.0,
                tracking: style.tracking,
            },
            Run::Space { width, style } => Stretch {
                text: "",
                font: style.font,
                space: *width,
                tracking: style.tracking,
            },
        })
        .collect()
}

/// Tab stops: every `tab_spaces` spaces in the first stretch's face, or Core Text's defaults
/// for 0.
#[derive(Clone, Copy)]
struct Tabs {
    interval: f64,
    last: f64,
}

impl Tabs {
    fn of(stretches: &[Stretch], tab_spaces: u32) -> Self {
        match stretches.first() {
            Some(first) if tab_spaces > 0 => Self {
                interval: typeface::width(&" ".repeat(tab_spaces as usize), first.font),
                last: f64::INFINITY,
            },
            _ => Self {
                interval: DEFAULT_TAB_INTERVAL,
                last: DEFAULT_TAB_INTERVAL * DEFAULT_TAB_STOPS,
            },
        }
    }

    /// The stop a tab at `x` advances to.
    fn after(self, x: f64) -> f64 {
        let stop = ((x / self.interval).floor() + 1.0) * self.interval;
        if stop > self.last {
            x
        } else {
            stop
        }
    }
}

/// Break opportunities in `text`, each with whether it is mandatory (after a line end, and at
/// the end). ICU's rules plus Core Text's additions: a break after a closing double quote before
/// a letter, and Unicode 15.1's number rule (a currency sign or closing bracket binds to what
/// follows only when a number follows).
fn breaks(text: &str) -> Vec<(usize, bool)> {
    const LINE_ENDS: [char; 7] = [
        '\n', '\r', '\u{b}', '\u{c}', '\u{85}', '\u{2028}', '\u{2029}',
    ];
    let mut breaks: Vec<usize> =
        LineSegmenter::new_for_non_complex_scripts(LineBreakOptions::default())
            .segment_str(text)
            .filter(|&at| at > 0)
            .collect();
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    for pair in 1..characters.len() {
        let ((_, before), (at, after)) = (characters[pair - 1], characters[pair]);
        let earlier = pair.checked_sub(2).map(|index| characters[index].1);
        let then = characters.get(pair + 1).map(|&(_, c)| c);
        if core_text_breaks_between(earlier, before, after, then) {
            breaks.push(at);
        }
    }
    breaks.sort_unstable();
    breaks.dedup();
    breaks
        .into_iter()
        .map(|at| (at, at == text.len() || text[..at].ends_with(LINE_ENDS)))
        .collect()
}

/// Whether Core Text allows a break between `before` and `after` where ICU does not (`earlier`
/// precedes `before`, `then` follows `after`): after a closing double quote before a letter, or
/// where a currency sign or closing bracket binds only because no number follows.
fn core_text_breaks_between(
    earlier: Option<char>,
    before: char,
    after: char,
    then: Option<char>,
) -> bool {
    let prefix = |c: char| {
        matches!(
            c,
            '$' | '+' | '\\' | '£' | '¥' | '€' | '₩' | '₹' | '₽' | '±' | '№'
        )
    };
    let postfix = |c: char| matches!(c, '%' | '‰' | '¢' | '°' | '′' | '″');
    let opening = |c: char| matches!(c, '(' | '[' | '{');
    let closing = |c: char| matches!(c, ')' | ']' | '}');
    let number_follows = then.is_some_and(|c| c.is_ascii_digit());
    let number_before = earlier.is_some_and(|c| c.is_ascii_digit());
    (before == '\u{201d}' && after.is_alphanumeric())
        || ((prefix(before) || postfix(before)) && opening(after) && !number_follows)
        || (closing(before) && (postfix(after) || prefix(after)) && !number_before)
}

/// The stretches set on one unbroken line, restarting at each line end.
struct Placing {
    /// Every stretch's text, joined.
    joined: String,
    /// Where each stretch starts in `joined`.
    starts: Vec<usize>,
    /// The x where each stretch's gap starts.
    rooms: Vec<f64>,
    /// The x of each byte boundary in `joined`, and the end, including any gap set before it.
    xs: Vec<f64>,
    /// Each stretch's glyphs, positioned along the unbroken line.
    glyphs: Vec<Vec<Placed>>,
}

fn placing(stretches: &[Stretch], tabs: Tabs) -> Placing {
    let joined: String = stretches.iter().map(|stretch| stretch.text).collect();
    let mut xs = vec![0.0; joined.len() + 1];
    let mut starts = Vec::with_capacity(stretches.len());
    let mut rooms = Vec::with_capacity(stretches.len());
    let mut glyphs = Vec::with_capacity(stretches.len());
    let mut x = 0.0;
    let mut at = 0;
    for stretch in stretches {
        starts.push(at);
        rooms.push(x);
        x += stretch.space;
        xs[at] = x;
        let (own, after) = place_stretch(stretch, tabs, x, &mut xs[at..]);
        x = after;
        at += stretch.text.len();
        glyphs.push(own);
    }
    Placing {
        joined,
        starts,
        rooms,
        xs,
        glyphs,
    }
}

/// Places `stretch` from `x`, writing each byte's x into `xs`. Returns its glyphs and the x
/// after it.
fn place_stretch(stretch: &Stretch, tabs: Tabs, mut x: f64, xs: &mut [f64]) -> (Vec<Placed>, f64) {
    let mut own = Vec::new();
    let text = stretch.text;
    let mut start = 0;
    // Shape each part between line ends and tabs alone: Core Text jumps a tab to the next stop
    // and restarts after a line end.
    loop {
        let end = text[start..]
            .find(['\n', '\t'])
            .map_or(text.len(), |found| start + found);
        x = place_part(stretch, start..end, x, xs, &mut own);
        if end == text.len() {
            return (own, x);
        }
        x = if text.as_bytes()[end] == b'\t' {
            tabs.after(x)
        } else {
            0.0
        };
        start = end + 1;
        xs[start] = x;
    }
}

/// Shapes the bytes `part` of `stretch` (no line end or tab) from `x`, appending glyphs to `own`
/// and each byte's x to `xs`. Returns the x where it ends.
fn place_part(
    stretch: &Stretch,
    part: Range<usize>,
    mut x: f64,
    xs: &mut [f64],
    own: &mut Vec<Placed>,
) -> f64 {
    let words = &stretch.text[part.clone()];
    let set = typeface::place(words, stretch.font);
    let mut glyph = 0;
    let mut along = x;
    for (byte, _) in words.char_indices() {
        // A byte inside a glyph's text (a ligature's second letter) counts as after it.
        while glyph < set.len() && set[glyph].start < byte {
            along += set[glyph].advance + tracked(&set[glyph], stretch);
            glyph += 1;
        }
        xs[part.start + byte] = along;
    }
    for placed in &set {
        let advance = placed.advance + tracked(placed, stretch);
        own.push(Placed {
            start: part.start + placed.start,
            x,
            advance,
            ..placed.clone()
        });
        x += advance;
    }
    xs[part.end] = x;
    x
}

/// The tracking space after `placed`, once per glyph it counts.
fn tracked(placed: &Placed, stretch: &Stretch) -> f64 {
    stretch.tracking * placed.letters as f64
}

/// Sets `stretches` in lines `width` wide, without wrapping unless `wraps` (line ends still
/// break). Tabs stop every `tab_spaces` spaces (0 for Core Text's defaults). Empty text or zero
/// width gives no lines.
pub fn set(stretches: &[Stretch], width: f64, wraps: bool, tab_spaces: u32) -> Vec<Line> {
    if width <= 0.0
        || stretches
            .iter()
            .all(|stretch| stretch.text.is_empty() && stretch.space == 0.0)
    {
        return Vec::new();
    }
    let tabs = Tabs::of(stretches, tab_spaces);
    let placing = placing(stretches, tabs);
    let limit = if wraps { width } else { f64::INFINITY };
    let breaks = breaks(&placing.joined);
    let mut lines = Vec::new();
    let mut start = 0;
    loop {
        let end = placing.line_end(&breaks, start, limit);
        lines.push(line(stretches, &placing, start, end, tabs));
        start = end;
        if start >= placing.joined.len() {
            break;
        }
    }
    lines
}

impl Placing {
    /// The x a line starting at `start` begins at. A gap there belongs to the previous line
    /// unless nothing comes before it.
    fn line_base(&self, start: usize) -> f64 {
        if start == 0 {
            0.0
        } else {
            self.xs[start]
        }
    }

    /// Where the line from byte `start`, `limit` wide, ends among `breaks`.
    fn line_end(&self, breaks: &[(usize, bool)], start: usize, limit: f64) -> usize {
        let joined = self.joined.as_str();
        let must = breaks
            .iter()
            .find(|&&(at, must)| at > start && must)
            .map_or(joined.len(), |&(at, _)| at);
        let base = self.line_base(start);
        // The first character that overruns the width.
        let over = joined[start..must]
            .char_indices()
            .map(|(at, character)| (start + at, character))
            .find(|&(at, character)| self.xs[at + character.len_utf8()] - base > limit + FIT_SLACK);
        match over {
            None => must,
            // Overrunning spaces hang at the line's end.
            Some((at, character)) if character.is_whitespace() => {
                at + joined[at..must]
                    .find(|character: char| !character.is_whitespace())
                    .unwrap_or(must - at)
            }
            Some((at, _)) => break_before(joined, breaks, start, at),
        }
    }
}

/// Where a line from byte `start` ends when the character at `at` overruns: the last allowed
/// break up to it.
fn break_before(joined: &str, breaks: &[(usize, bool)], start: usize, at: usize) -> usize {
    if let Some(&(place, _)) = breaks
        .iter()
        .rev()
        .find(|&&(place, _)| place > start && place <= at)
    {
        return place;
    }
    // No word fits: take as many characters as fit, at least one.
    if at > start {
        at
    } else {
        at + joined[at..].chars().next().map_or(0, char::len_utf8)
    }
}

/// The line from byte `start` to `end` of `placing`, set as Core Text sets it: text from the
/// line's start to the end of its paragraph is reshaped, so earlier text no longer affects the
/// glyphs while later text still does and the last glyph keeps its kerning. Includes the gaps
/// that belong to the line.
fn line(stretches: &[Stretch], placing: &Placing, start: usize, end: usize, tabs: Tabs) -> Line {
    let joined = placing.joined.as_str();
    let paragraph = joined[end..]
        .find('\n')
        .map_or(joined.len(), |at| end + at + 1);
    let parts: Vec<Part> = stretches
        .iter()
        .zip(&placing.starts)
        .enumerate()
        .filter_map(|(index, (stretch, &at))| Part::of(index, stretch, at, start, paragraph))
        .collect();
    let own: Vec<Stretch> = parts.iter().map(|part| part.words).collect();
    let set = self::placing(&own, tabs);
    let on_line = end - start;
    let pieces = parts
        .iter()
        .enumerate()
        .filter_map(|(index, part)| part.piece(&set, index, on_line))
        .collect();
    // The line's terminating line end takes no room.
    let last = on_line - usize::from(joined[..end].ends_with('\n'));
    Line {
        pieces,
        width: set.xs[last],
    }
}

/// A stretch's share of a line, reshaped for it: the stretch, its text from the line's start to
/// the paragraph's end, and the stretch byte where that text starts.
#[derive(Clone, Copy)]
struct Part<'a> {
    stretch: usize,
    words: Stretch<'a>,
    offset: usize,
}

impl<'a> Part<'a> {
    /// Stretch `index`'s part (starting at byte `at` of the joined text) of the bytes from
    /// `start` to `paragraph`, if any.
    fn of(
        index: usize,
        stretch: &Stretch<'a>,
        at: usize,
        start: usize,
        paragraph: usize,
    ) -> Option<Self> {
        if stretch.space > 0.0 {
            // A gap belongs to the line whose last letter it follows, or to the first line when
            // nothing precedes it.
            let goes = (at > start || start == 0) && at < paragraph;
            return goes.then_some(Self {
                stretch: index,
                words: *stretch,
                offset: 0,
            });
        }
        let (from, to) = (start.max(at), paragraph.min(at + stretch.text.len()));
        if from >= to {
            return None;
        }
        Some(Self {
            stretch: index,
            words: Stretch {
                text: &stretch.text[from - at..to - at],
                ..*stretch
            },
            offset: from - at,
        })
    }

    /// The piece this part (the `index`th placed in `set`) contributes to the line's first
    /// `on_line` bytes, if any.
    fn piece(&self, set: &Placing, index: usize, on_line: usize) -> Option<Piece> {
        let at = set.starts[index];
        if self.words.space > 0.0 {
            return (at <= on_line).then(|| Piece {
                stretch: self.stretch,
                start: 0,
                end: 0,
                x: set.rooms[index],
                width: self.words.space,
                glyphs: Vec::new(),
            });
        }
        let length = self.words.text.len().min(on_line.saturating_sub(at));
        if length == 0 {
            return None;
        }
        let x = set.xs[at];
        let glyphs: Vec<Placed> = set.glyphs[index]
            .iter()
            .filter(|glyph| glyph.start < length)
            .map(|glyph| Placed {
                start: self.offset + glyph.start,
                x: glyph.x - x,
                ..glyph.clone()
            })
            .collect();
        Some(Piece {
            stretch: self.stretch,
            start: self.offset,
            end: self.offset + length,
            x,
            width: glyphs.iter().map(|glyph| glyph.advance).sum(),
            glyphs,
        })
    }
}

/// Measures `words`: line count and widest line width.
pub fn measure(words: &Words) -> Measured {
    let lines = set(
        &stretches(&words.runs),
        words.width,
        words.wraps,
        words.tab_spaces,
    );
    Measured {
        lines: lines.len(),
        widest: lines.iter().map(|line| line.width).fold(0.0, f64::max),
    }
}

/// The width of `text` in `font`: the widest line, with line ends and tabs handled as `set`
/// handles them.
pub fn width(text: &str, font: Font) -> f64 {
    let stretch = Stretch {
        text,
        font,
        space: 0.0,
        tracking: 0.0,
    };
    set(&[stretch], f64::INFINITY, false, 0)
        .iter()
        .map(|line| line.width)
        .fold(0.0, f64::max)
}

/// A font-independent measure shared by layout tests.
#[cfg(test)]
pub(crate) mod test_measure {
    use crate::board::card::Font;

    /// Every character 7 wide in any font.
    pub(crate) fn sevens(text: &str, _: Font) -> f64 {
        text.chars().count() as f64 * 7.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn words(text: &str, font: Font) -> Stretch<'_> {
        Stretch {
            text,
            font,
            space: 0.0,
            tracking: 0.0,
        }
    }

    fn texts(lines: &[Line], stretches: &[Stretch]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.pieces
                    .iter()
                    .map(|piece| &stretches[piece.stretch].text[piece.start..piece.end])
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_line_takes_the_words_that_fit_less_the_spaces_after_them() {
        let font = Font::sans(13.0, Font::REGULAR);
        let whole = typeface::width("two words", font);
        let stretches = [words("two words more", font)];
        let lines = set(&stretches, whole, true, 0);
        assert_eq!(texts(&lines, &stretches), ["two words ", "more"]);
        // The trailing space counts in the line's width.
        assert!((lines[0].width - typeface::width("two words ", font)).abs() < 1e-9);
        // Without wrapping, only line ends break it.
        let stretches = [words("one\ntwo three", font)];
        assert_eq!(
            texts(&set(&stretches, 1.0, false, 0), &stretches),
            ["one\n", "two three"]
        );
    }

    #[test]
    fn a_word_wider_than_its_line_is_broken_between_characters() {
        let font = Font::sans(13.0, Font::REGULAR);
        let stretches = [words("abcdef", font)];
        let lines = set(&stretches, typeface::width("abc", font) + 0.1, true, 0);
        assert_eq!(texts(&lines, &stretches), ["abc", "def"]);
    }

    #[test]
    fn a_tab_goes_to_the_next_stop() {
        let font = Font::sans(13.0, Font::REGULAR);
        let stretches = [words("a\tb", font)];
        let lines = set(&stretches, f64::INFINITY, false, 0);
        assert_eq!(lines[0].pieces[0].glyphs[1].x, DEFAULT_TAB_INTERVAL);
        let four = typeface::width("    ", font);
        let lines = set(&stretches, f64::INFINITY, false, 4);
        assert!((lines[0].width - four - typeface::width("b", font)).abs() < 1e-9);
    }

    #[test]
    fn room_goes_with_the_letter_before_it() {
        let font = Font::sans(13.0, Font::REGULAR);
        let code = Font::mono(11.0);
        let stretches = [
            words("say ", font),
            Stretch {
                text: "",
                font: code,
                space: 4.8,
                tracking: 0.0,
            },
            words("this", code),
            Stretch {
                text: "",
                font: code,
                space: 4.8,
                tracking: 0.0,
            },
            words(" now", font),
        ];
        let lines = set(&stretches, f64::INFINITY, false, 0);
        let pieces = &lines[0].pieces;
        assert_eq!(pieces.len(), 5);
        let say = typeface::width("say ", font);
        assert!((pieces[1].x - say).abs() < 1e-9 && (pieces[2].x - say - 4.8).abs() < 1e-9);
        let this = typeface::width("this", code);
        assert!((pieces[4].x - (say + 9.6 + this)).abs() < 1e-9);
        // The gap after the code belongs to the line before the break.
        let lines = set(&stretches, say + 9.6 + this + 1.0, true, 0);
        assert_eq!(lines.len(), 2);
        assert!((lines[0].width - (say + 9.6 + this + typeface::width(" ", font))).abs() < 1e-9);
    }

    #[test]
    fn tracking_is_room_after_each_glyph() {
        let font = Font::sans(13.0, Font::REGULAR);
        let stretches = [Stretch {
            text: "ab",
            font,
            space: 0.0,
            tracking: 1.0,
        }];
        let lines = set(&stretches, f64::INFINITY, false, 0);
        assert!((lines[0].width - (typeface::width("ab", font) + 2.0)).abs() < 1e-9);
        assert!((lines[0].pieces[0].glyphs[1].x - (typeface::width("a", font) + 1.0)).abs() < 1e-9);
    }

    /// Parses a face from the check's fields: size, weight, mono, tabular, slanted.
    fn face_of(fields: &[&str]) -> Font {
        Font {
            size: fields[0].parse().expect("a size"),
            weight: fields[1].parse().expect("a weight"),
            mono: fields[2] == "1",
            tabular: fields[3] == "1",
            slanted: fields[4] == "1",
        }
    }

    /// Allowed difference from Core Text's width, in points.
    const CORE_TEXT_TOLERANCE: f64 = 1e-3;

    /// The check's text with escaped line ends and tabs restored.
    fn unescaped(text: &str) -> String {
        text.replace("\\n", "\n").replace("\\t", "\t")
    }

    /// Formats a face as the check writes it: size, weight, mono, tabular, slanted,
    /// tab-separated.
    fn face_key(font: Font) -> String {
        let bit = |on: bool| if on { "1" } else { "0" };
        format!(
            "{}\t{}\t{}\t{}\t{}",
            font.size,
            font.weight,
            bit(font.mono),
            bit(font.tabular),
            bit(font.slanted)
        )
    }

    /// Core Text's widths for text neither face covers (the answers' "F" lines), by face and
    /// text.
    fn fallback_widths(answers: &str) -> HashMap<(String, String), f64> {
        answers
            .lines()
            .filter(|line| line.starts_with("F\t"))
            .map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                (
                    (fields[1..6].join("\t"), fields[6].to_owned()),
                    fields[7].parse().expect("a width"),
                )
            })
            .collect()
    }

    /// Running results: case count, the largest difference among passing cases, and each
    /// failure with the core's result.
    #[derive(Default)]
    struct Tally {
        cases: usize,
        worst: f64,
        wrong: Vec<String>,
    }

    impl Tally {
        /// A "W" case: tracked text's width on one line.
        fn width_case(&mut self, line: &str, fields: &[&str], font: Font, answer: &str) {
            let tracking: f64 = fields[6].parse().expect("a tracking");
            let text = unescaped(&fields[7..fields.len() - 1].join("\t"));
            let stretch = Stretch {
                text: &text,
                font,
                space: 0.0,
                tracking,
            };
            let width = set(&[stretch], f64::INFINITY, false, 0)
                .first()
                .map_or(0.0, |line| line.width);
            let core_text: f64 = answer.parse().expect("a width");
            self.worst = self.worst.max((width - core_text).abs());
            if (width - core_text).abs() > CORE_TEXT_TOLERANCE {
                self.wrong.push(format!("{line}: {width}"));
            }
        }

        /// An "L" case: line count in a width, and the widest line.
        fn lines_case(&mut self, line: &str, fields: &[&str], font: Font, answer: &str) {
            let width: f64 = fields[6].parse().expect("a width");
            let (wraps, tabs): (bool, u32) =
                (fields[7] == "1", fields[8].parse().expect("tab spaces"));
            let text = unescaped(&fields[9..fields.len() - 1].join("\t"));
            let lines = set(&[words(&text, font)], width, wraps, tabs);
            let widest = lines.iter().map(|line| line.width).fold(0.0, f64::max);
            let (count, core_text) = answer.split_once(' ').expect("lines and the widest");
            let core_text: f64 = core_text.parse().expect("a width");
            if count.parse::<usize>().expect("a count") != lines.len()
                || (widest - core_text).abs() > CORE_TEXT_TOLERANCE
            {
                self.wrong.push(format!("{line}: {} {widest}", lines.len()));
            } else {
                self.worst = self.worst.max((widest - core_text).abs());
            }
        }
    }

    /// Every case from `scripts/text-widths-check.sh`, against Core Text's answer.
    #[test]
    #[ignore = "the text-widths check runs it with Core Text's answers"]
    fn every_case_sets_as_core_text_sets_it() {
        let answers =
            std::fs::read_to_string(std::env::var("NR_TEXT_WIDTHS").expect("the answers' path"))
                .expect("the answers");
        let fallbacks = fallback_widths(&answers);
        typeface::set_fallback(Box::new(move |text, font| {
            *fallbacks
                .get(&(face_key(font), text.to_owned()))
                .unwrap_or_else(|| panic!("Core Text measured {text:?} in {}", face_key(font)))
        }));
        let mut tally = Tally::default();
        for line in answers.lines().filter(|line| !line.starts_with("F\t")) {
            let fields: Vec<&str> = line.split('\t').collect();
            let font = face_of(&fields[1..6]);
            let answer = fields[fields.len() - 1];
            tally.cases += 1;
            match fields[0] {
                "W" => tally.width_case(line, &fields, font, answer),
                "L" => tally.lines_case(line, &fields, font, answer),
                kind => panic!("no case kind {kind}"),
            }
        }
        println!(
            "{} cases, {} wrong, the widest difference of the rest {:e} points",
            tally.cases,
            tally.wrong.len(),
            tally.worst
        );
        for case in &tally.wrong {
            println!("{case}");
        }
        assert!(tally.wrong.is_empty());
    }
}
