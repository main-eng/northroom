//! What OpenDocument text and presentations share: lengths, styles and their families, and
//! paragraphs with their spans, lists and marks. `odt` and `odp` read their documents through
//! this.
//!
//! A style's look comes from itself, then its parents, then its family's default style. Named
//! styles live in `styles.xml` and automatic ones in either part; a name is looked up in the
//! content first.

use super::{Align, Colour, Paragraph, Run, Spacing};
use crate::package::{Piece, XmlElement};
use std::collections::HashMap;

/// Points per inch.
const POINTS_PER_INCH: f64 = 72.0;
/// Text's size when no style sets one.
pub const DEFAULT_SIZE_POINTS: f64 = 12.0;
/// How far each list level steps in, and how wide its mark's space is, when the list's style
/// does not say.
const LIST_STEP_POINTS: f64 = 18.0;
/// How a tab is drawn, without tab stops.
const TAB: &str = "    ";
/// How far a style's parents are followed; a longer chain is a loop.
const STYLE_DEPTH: usize = 32;
const LEVELS: usize = 10;
/// The mark shown for a bullet whose character the app's face lacks.
const PLAIN_BULLET: &str = "•";

/// A length such as `2.54cm` in points; `None` for a unit this reader does not know or a share
/// (`120%`), which depends on what it is a share of.
pub fn length(text: &str) -> Option<f64> {
    let text = text.trim();
    let digits = text
        .find(|character: char| character.is_ascii_alphabetic() || character == '%')
        .unwrap_or(text.len());
    let value: f64 = text[..digits].trim().parse().ok()?;
    let per_inch = match &text[digits..] {
        "in" | "inch" => 1.0,
        "cm" => 2.54,
        "mm" => 25.4,
        "pt" => POINTS_PER_INCH,
        "pc" => 6.0,
        "px" => 96.0,
        _ => return None,
    };
    value
        .is_finite()
        .then_some(value / per_inch * POINTS_PER_INCH)
}

/// A length attribute of `element`, in points.
pub fn length_of(element: &XmlElement, attribute: &str) -> Option<f64> {
    element.attribute(attribute).and_then(length)
}

/// What a style says about text and paragraphs; `None` where it says nothing.
#[derive(Clone, Debug, Default)]
pub struct Look {
    pub size: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub colour: Option<Colour>,
    pub align: Option<Align>,
    pub left: Option<f64>,
    pub indent: Option<f64>,
    pub line: Option<Spacing>,
    pub before: Option<Spacing>,
    pub after: Option<Spacing>,
    pub break_before: Option<bool>,
}

impl Look {
    /// This look, with what it leaves unsaid taken from `under`.
    pub fn over(self, under: &Look) -> Look {
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
            break_before: self.break_before.or(under.break_before),
        }
    }

    pub fn run(&self, text: &str) -> Run {
        Run {
            text: text.to_owned(),
            size: self.size.unwrap_or(DEFAULT_SIZE_POINTS),
            bold: self.bold.unwrap_or(false),
            italic: self.italic.unwrap_or(false),
            underline: self.underline.unwrap_or(false),
            colour: self.colour.unwrap_or(Colour::BLACK),
        }
    }
}

fn read_align(align: &str) -> Option<Align> {
    match align {
        "start" | "left" | "justify" => Some(Align::Start),
        "center" => Some(Align::Center),
        "end" | "right" => Some(Align::End),
        _ => None,
    }
}

/// A `fo:line-height`: a share (`115%`) or a length.
fn read_line(height: &str) -> Option<Spacing> {
    if let Some(share) = height.strip_suffix('%') {
        return share
            .trim()
            .parse::<f64>()
            .ok()
            .map(|share| Spacing::Share(share / 100.0));
    }
    length(height).map(Spacing::Points)
}

fn read_text_properties(look: &mut Look, text: &XmlElement) {
    look.size = length_of(text, "fo:font-size");
    look.bold = text
        .attribute("fo:font-weight")
        .map(|weight| weight == "bold" || weight.parse::<u32>().is_ok_and(|weight| weight >= 600));
    look.italic = text
        .attribute("fo:font-style")
        .map(|style| matches!(style, "italic" | "oblique"));
    look.underline = text
        .attribute("style:text-underline-style")
        .map(|line| line != "none");
    look.colour = text.attribute("fo:color").and_then(Colour::hex);
}

fn read_paragraph_properties(look: &mut Look, paragraph: &XmlElement) {
    look.align = paragraph.attribute("fo:text-align").and_then(read_align);
    look.left = length_of(paragraph, "fo:margin-left");
    look.indent = length_of(paragraph, "fo:text-indent");
    look.line = paragraph.attribute("fo:line-height").and_then(read_line);
    look.before = length_of(paragraph, "fo:margin-top").map(Spacing::Points);
    look.after = length_of(paragraph, "fo:margin-bottom").map(Spacing::Points);
    look.break_before = paragraph
        .attribute("fo:break-before")
        .map(|place| place == "page");
}

/// The look a style element's properties give.
pub fn read_look(style: &XmlElement) -> Look {
    let mut look = Look::default();
    if let Some(text) = style.child("text-properties") {
        read_text_properties(&mut look, text);
    }
    if let Some(paragraph) = style.child("paragraph-properties") {
        read_paragraph_properties(&mut look, paragraph);
    }
    look
}

/// A style: its parent's name and the element itself, for properties other than text's.
pub struct Style {
    pub parent: Option<String>,
    pub look: Look,
    pub element: XmlElement,
}

/// A document's styles by family and name, its default styles by family, and its list styles
/// by name.
#[derive(Default)]
pub struct Styles {
    named: HashMap<(String, String), Style>,
    defaults: HashMap<String, Style>,
    lists: HashMap<String, XmlElement>,
}

/// The lists in a part that hold styles: its named, automatic and master styles.
const STYLE_LISTS: [&str; 3] = ["styles", "automatic-styles", "master-styles"];

impl Styles {
    /// Adds the styles a part defines. A name already known is kept: the content's own styles
    /// are read first and win.
    pub fn add(&mut self, part: &XmlElement) {
        let lists = part
            .children
            .iter()
            .filter(|child| STYLE_LISTS.contains(&child.name.as_str()));
        for element in lists.flat_map(|list| &list.children) {
            self.add_one(element);
        }
    }

    fn add_one(&mut self, element: &XmlElement) {
        match element.name.as_str() {
            "style" => {
                let (Some(family), Some(name)) = (
                    element.attribute("style:family"),
                    element.attribute("style:name"),
                ) else {
                    return;
                };
                self.named
                    .entry((family.to_owned(), name.to_owned()))
                    .or_insert_with(|| Style {
                        parent: element
                            .attribute("style:parent-style-name")
                            .map(str::to_owned),
                        look: read_look(element),
                        element: element.clone(),
                    });
            }
            "default-style" => {
                if let Some(family) = element.attribute("style:family") {
                    self.defaults
                        .entry(family.to_owned())
                        .or_insert_with(|| Style {
                            parent: None,
                            look: read_look(element),
                            element: element.clone(),
                        });
                }
            }
            "list-style" => {
                if let Some(name) = element.attribute("style:name") {
                    self.lists
                        .entry(name.to_owned())
                        .or_insert_with(|| element.clone());
                }
            }
            _ => {}
        }
    }

    pub fn get(&self, family: &str, name: &str) -> Option<&Style> {
        self.named.get(&(family.to_owned(), name.to_owned()))
    }

    /// The chain from a style up its parents, nearest first.
    pub fn chain(&self, family: &str, name: Option<&str>) -> Vec<&Style> {
        let mut chain = Vec::new();
        let mut next = name.map(str::to_owned);
        for _ in 0..STYLE_DEPTH {
            let Some(style) = next.as_deref().and_then(|name| self.get(family, name)) else {
                break;
            };
            chain.push(style);
            next = style.parent.clone();
        }
        chain
    }

    /// A style's look with its parents' under it, without its family's default.
    pub fn own_look(&self, family: &str, name: Option<&str>) -> Look {
        self.chain(family, name)
            .into_iter()
            .fold(Look::default(), |look, style| look.over(&style.look))
    }

    /// The look a family's default style gives.
    pub fn default_look(&self, family: &str) -> Look {
        self.defaults
            .get(family)
            .map(|style| style.look.clone())
            .unwrap_or_default()
    }

    /// A family's default style, for properties other than text's.
    pub fn default_style(&self, family: &str) -> Option<&Style> {
        self.defaults.get(family)
    }

    pub fn list(&self, name: &str) -> Option<&XmlElement> {
        self.lists.get(name)
    }
}

// --- Paragraphs ------------------------------------------------------------------------------

/// Something a paragraph holds that is drawn apart from its text: a frame, found among its
/// spans, which a reader turns into a picture.
pub type Frames<'a> = Vec<&'a XmlElement>;

/// A paragraph's text as it is put together. White space in the file is not content: a run of
/// it is one space, and none starts a paragraph. Spaces that are content are written as
/// `<text:s>`.
struct Gathering<'a, 'b> {
    styles: &'b Styles,
    runs: Vec<Run>,
    spaced: bool,
    frames: Frames<'a>,
}

impl<'a> Gathering<'a, '_> {
    fn started(&self) -> bool {
        self.runs.iter().any(|run| !run.text.is_empty())
    }

    fn add_text(&mut self, text: &str, look: &Look) {
        let mut kept = String::new();
        for character in text.chars() {
            if matches!(character, ' ' | '\t' | '\r' | '\n') {
                if (self.started() || !kept.is_empty()) && !self.spaced {
                    kept.push(' ');
                    self.spaced = true;
                }
            } else {
                kept.push(character);
                self.spaced = false;
            }
        }
        if !kept.is_empty() {
            self.runs.push(look.run(&kept));
        }
    }

    fn add_content(&mut self, content: &str, look: &Look) {
        self.runs.push(look.run(content));
        self.spaced = false;
    }

    fn add_child(&mut self, child: &'a XmlElement, look: &Look) {
        match child.name.as_str() {
            "span" => {
                let own = self
                    .styles
                    .own_look("text", child.attribute("text:style-name"));
                self.add(child, &own.over(look));
            }
            "a" | "ruby-base" | "meta" => self.add(child, look),
            "s" => {
                let count = child
                    .attribute("text:c")
                    .and_then(|count| count.parse::<usize>().ok())
                    .unwrap_or(1)
                    .min(1024);
                self.add_content(&" ".repeat(count), look);
            }
            "tab" => self.add_content(TAB, look),
            "line-break" => self.add_content("\n", look),
            "frame" => self.frames.push(child),
            _ => {}
        }
    }

    fn add(&mut self, element: &'a XmlElement, look: &Look) {
        for piece in element.pieces() {
            match piece {
                Piece::Text(text) => self.add_text(text, look),
                Piece::Child(child) => self.add_child(child, look),
            }
        }
    }
}

/// A list's place: how deep a paragraph is and the list style it is under.
#[derive(Clone, Default)]
pub struct InList<'a> {
    pub depth: usize,
    pub style: Option<&'a str>,
}

/// The numbers each list level has reached.
#[derive(Default)]
pub struct Numbering([usize; LEVELS]);

/// A list level's mark, its text's start and its first line's place, from the list style.
struct LevelMark {
    mark: Option<String>,
    left: f64,
    indent: f64,
}

fn level_style(list: &XmlElement, depth: usize) -> Option<&XmlElement> {
    let level = (depth + 1).to_string();
    list.children
        .iter()
        .find(|level_style| level_style.attribute("text:level") == Some(level.as_str()))
}

/// The indents a list level's properties give: its label's start and width, or its text's
/// margin and first line.
fn level_indents(level: Option<&XmlElement>, depth: usize) -> (f64, f64) {
    let fallback = (LIST_STEP_POINTS * (depth + 1) as f64, -LIST_STEP_POINTS);
    let Some(properties) = level.and_then(|level| level.child("list-level-properties")) else {
        return fallback;
    };
    if let Some(aligned) = properties.child("list-level-label-alignment") {
        let left = length_of(aligned, "fo:margin-left").unwrap_or(fallback.0);
        let indent = length_of(aligned, "fo:text-indent").unwrap_or(fallback.1);
        return (left, indent);
    }
    let before =
        length_of(properties, "text:space-before").unwrap_or(fallback.0 - LIST_STEP_POINTS);
    let label = length_of(properties, "text:min-label-width").unwrap_or(LIST_STEP_POINTS);
    (before + label, -label)
}

impl Numbering {
    fn mark(&mut self, styles: &Styles, list: &InList) -> LevelMark {
        let depth = list.depth.min(LEVELS - 1);
        for deeper in &mut self.0[depth + 1..] {
            *deeper = 0;
        }
        let level = list
            .style
            .and_then(|name| styles.list(name))
            .and_then(|style| level_style(style, depth));
        let (left, indent) = level_indents(level, depth);
        let mark = match level.map(|level| level.name.as_str()) {
            Some("list-level-style-number") => {
                self.0[depth] += 1;
                let level = level.expect("a level");
                let format = level.attribute("style:num-format").unwrap_or("1");
                (!format.is_empty()).then(|| {
                    let prefix = level.attribute("style:num-prefix").unwrap_or("");
                    let suffix = level.attribute("style:num-suffix").unwrap_or("");
                    format!("{prefix}{}{suffix}", numeral(self.0[depth], format))
                })
            }
            Some("list-level-style-image" | "list-level-style-bullet") | None => {
                let character = level
                    .and_then(|level| level.attribute("text:bullet-char"))
                    .filter(|mark| {
                        !mark
                            .chars()
                            .any(|character| ('\u{e000}'..='\u{f8ff}').contains(&character))
                    });
                Some(character.unwrap_or(PLAIN_BULLET).to_owned())
            }
            Some(_) => None,
        };
        LevelMark { mark, left, indent }
    }
}

/// A count written in an OpenDocument number format: `1`, `a`, `A`, `i` or `I`.
fn numeral(count: usize, format: &str) -> String {
    let letters = || {
        let letter = char::from(b'a' + ((count.max(1) - 1) % 26) as u8);
        letter.to_string().repeat((count.max(1) - 1) / 26 + 1)
    };
    match format {
        "a" => letters(),
        "A" => letters().to_uppercase(),
        "i" => roman(count),
        "I" => roman(count).to_uppercase(),
        _ => count.to_string(),
    }
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

/// A paragraph or heading read: the paragraph, whether it asks to start a page, and the frames
/// it holds.
pub struct Read<'a> {
    pub paragraph: Paragraph,
    pub break_before: bool,
    pub frames: Frames<'a>,
}

/// Reads a `text:p` or `text:h` in its style over `under`, then the default paragraph style, as
/// a list item when `list` says.
pub fn read_paragraph<'a>(
    element: &'a XmlElement,
    styles: &Styles,
    under: &Look,
    list: Option<&InList>,
    numbering: &mut Numbering,
) -> Read<'a> {
    let look = styles
        .own_look("paragraph", element.attribute("text:style-name"))
        .over(under)
        .over(&styles.default_look("paragraph"));
    let mut gathering = Gathering {
        styles,
        runs: Vec::new(),
        spaced: false,
        frames: Vec::new(),
    };
    gathering.add(element, &look);
    let runs = gathering.runs;
    let written = runs.iter().any(|run| !run.text.trim().is_empty());
    let marked = list
        .filter(|_| written)
        .map(|list| numbering.mark(styles, list));
    let size = runs
        .iter()
        .map(|run| run.size)
        .fold(None, |largest: Option<f64>, size| {
            Some(largest.map_or(size, |largest| largest.max(size)))
        });
    let paragraph = Paragraph {
        align: look.align.unwrap_or_default(),
        left: marked
            .as_ref()
            .map_or(look.left.unwrap_or(0.0), |marked| marked.left),
        indent: marked
            .as_ref()
            .map_or(look.indent.unwrap_or(0.0), |marked| marked.indent),
        line: look.line.unwrap_or(Spacing::Share(1.0)),
        before: look.before.unwrap_or(Spacing::Points(0.0)),
        after: look.after.unwrap_or(Spacing::Points(0.0)),
        bullet: marked.and_then(|marked| marked.mark),
        size: size.or(look.size).unwrap_or(DEFAULT_SIZE_POINTS),
        runs,
    };
    Read {
        paragraph,
        break_before: look.break_before == Some(true),
        frames: gathering.frames,
    }
}

/// Something that receives each paragraph found, with the list it is in.
pub type Visit<'v, 'a> = dyn FnMut(&'a XmlElement, Option<&InList<'a>>) + 'v;

/// The paragraphs and headings in `element`, itself one or a list holding them, in order, with
/// the list each is in. `visit` is called for each.
pub fn each_paragraph_in<'a>(
    element: &'a XmlElement,
    list: Option<&InList<'a>>,
    visit: &mut Visit<'_, 'a>,
) {
    match element.name.as_str() {
        "p" | "h" => visit(element, list),
        "list" => {
            let inner = InList {
                depth: list.map_or(0, |list| list.depth + 1),
                style: element
                    .attribute("text:style-name")
                    .or(list.and_then(|list| list.style)),
            };
            each_paragraph(element, Some(&inner), visit);
        }
        "list-item" | "list-header" => each_paragraph(element, list, visit),
        _ => {}
    }
}

/// The paragraphs and headings `holder` holds, through lists and their items.
pub fn each_paragraph<'a>(
    holder: &'a XmlElement,
    list: Option<&InList<'a>>,
    visit: &mut Visit<'_, 'a>,
) {
    for child in &holder.children {
        each_paragraph_in(child, list, visit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::parse_xml;

    #[test]
    fn lengths_read_in_every_unit() {
        assert_eq!(length("1in"), Some(72.0));
        assert_eq!(length("2.54cm"), Some(72.0));
        assert_eq!(length("25.4mm"), Some(72.0));
        assert_eq!(length("12pt"), Some(12.0));
        assert_eq!(length("96px"), Some(72.0));
        assert_eq!(length("120%"), None);
        assert_eq!(length("wide"), None);
    }

    #[test]
    fn a_paragraph_takes_its_style_its_spans_and_its_list_mark() {
        let styles_part = parse_xml(
            r##"<office:document-styles><office:styles>
                 <style:default-style style:family="paragraph"><style:text-properties fo:font-size="11pt"/></style:default-style>
                 <style:style style:name="Heading" style:family="paragraph"><style:text-properties fo:font-size="20pt" fo:font-weight="bold"/></style:style>
                 <style:style style:name="Strong" style:family="text"><style:text-properties fo:font-weight="bold" fo:color="#1f6f4a"/></style:style>
                 <text:list-style style:name="L1">
                   <text:list-level-style-number text:level="1" style:num-format="a" style:num-suffix=")"/>
                 </text:list-style>
               </office:styles></office:document-styles>"##,
        )
        .expect("XML");
        let mut styles = Styles::default();
        styles.add(&styles_part);
        let body = parse_xml(
            "<office:text>\
               <text:h text:style-name=\"Heading\">  Title  </text:h>\
               <text:p>Plain <text:span text:style-name=\"Strong\">strong</text:span><text:s text:c=\"2\"/>end<text:line-break/>next</text:p>\
               <text:list text:style-name=\"L1\"><text:list-item><text:p>one</text:p></text:list-item><text:list-item><text:p>two</text:p></text:list-item></text:list>\
             </office:text>",
        )
        .expect("XML");
        let mut numbering = Numbering::default();
        let mut read = Vec::new();
        each_paragraph(&body, None, &mut |element, list| {
            read.push(
                read_paragraph(element, &styles, &Look::default(), list, &mut numbering).paragraph,
            );
        });
        let words = |paragraph: &Paragraph| -> String {
            paragraph.runs.iter().map(|run| run.text.as_str()).collect()
        };
        assert_eq!(words(&read[0]), "Title ");
        assert_eq!((read[0].runs[0].size, read[0].runs[0].bold), (20.0, true));
        assert_eq!(words(&read[1]), "Plain strong  end\nnext");
        let strong = &read[1].runs[1];
        assert_eq!(
            (strong.bold, strong.colour, strong.size),
            (true, Colour::hex("1f6f4a").unwrap(), 11.0)
        );
        assert_eq!(read[2].bullet.as_deref(), Some("a)"));
        assert_eq!(read[3].bullet.as_deref(), Some("b)"));
        assert_eq!((read[3].left, read[3].indent), (18.0, -18.0));
    }
}
