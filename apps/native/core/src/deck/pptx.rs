//! Reads a PowerPoint presentation (`.pptx` and its template and show forms) into a `Deck`: a
//! zip of XML parts located through their relationship files, opened with the workbook
//! reader's zip and XML readers.
//!
//! A slide is drawn over its layout and its layout's master. A placeholder left empty of a
//! position or a look takes it from the layout's placeholder, then the master's, then the
//! master's text styles. Colours named by role ("accent 1") come from the master's theme.
//! Shapes in groups are placed where the group puts them, and a table becomes one box per
//! cell.
//!
//! Left out: what `deck` says no deck draws, and also pictures used as fills, per-slide colour
//! maps, cell borders, the real looks of table styles (a styled table is drawn as the default
//! one: a header in accent 1 over banded rows), and numbered lists in anything but "1.".

use super::{
    read_bytes, Align, Anchor, Body, Colour, Deck, Fit, Form, Item, Paragraph, Placed, ReadError,
    Result, Run, Shape, Slide, Source, Spacing, Stroke,
};
use crate::geometry::Rect;
use crate::package::{find_relationship, read_relationships, require_part, XmlElement, ZipArchive};
use std::collections::HashMap;
use std::rc::Rc;

/// English Metric Units, the file's unit of length, in a point.
const EMU_PER_POINT: f64 = 12_700.0;
/// The file's unit for a share: 100,000 is the whole.
const WHOLE: f64 = 100_000.0;
/// A slide's size when the file gives none: 4:3 at 10 by 7.5 inches.
const DEFAULT_SLIDE_POINTS: (f64, f64) = (720.0, 540.0);
/// Text's size when nothing sets one.
const DEFAULT_TEXT_POINTS: f64 = 18.0;
/// The space kept clear inside a text frame when the file gives none: sides, then top and
/// bottom.
const DEFAULT_SIDE_INSET_POINTS: f64 = 7.2;
const DEFAULT_END_INSET_POINTS: f64 = 3.6;
/// A line's thickness when the file gives none.
const DEFAULT_STROKE_POINTS: f64 = 0.75;
/// A rounded box's corner radius as a share of its shorter side when the file gives none.
const DEFAULT_ROUNDING: f64 = 0.166_67;
/// The deepest list level.
const LEVELS: usize = 9;
/// The mark shown for a bullet set in a symbol face, whose own character the app's face lacks.
const PLAIN_BULLET: &str = "•";
/// How far a styled table's rows are lightened from its accent colour: the darker and the
/// lighter band.
const BAND_TINTS: (f64, f64) = (0.6, 0.8);
/// The thickness of the lines between a styled table's cells.
const TABLE_STROKE_POINTS: f64 = 1.0;

// --- Numbers and parts -----------------------------------------------------------------------

fn number(element: &XmlElement, attribute: &str) -> Option<f64> {
    element
        .attribute(attribute)?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// An attribute in English Metric Units, as points.
fn points(element: &XmlElement, attribute: &str) -> Option<f64> {
    number(element, attribute).map(|emu| emu / EMU_PER_POINT)
}

/// An attribute counted in 100,000ths, as a share.
fn share(element: &XmlElement, attribute: &str) -> Option<f64> {
    number(element, attribute).map(|value| value / WHOLE)
}

/// An XML part and what it is related to.
struct Part {
    root: XmlElement,
    relations: Vec<(String, String, String)>,
}

impl Part {
    fn open(archive: &ZipArchive, path: &str) -> Result<Self> {
        Ok(Self {
            root: require_part(archive, path)?,
            relations: read_relationships(archive, path)?,
        })
    }

    /// The part a relationship id names.
    fn target(&self, id: &str) -> Option<&str> {
        self.relations
            .iter()
            .find(|(known, _, _)| known == id)
            .map(|(_, _, path)| path.as_str())
    }

    /// The first part related as a kind ending in `suffix`.
    fn related(&self, suffix: &str) -> Option<&str> {
        find_relationship(&self.relations, suffix)
    }

    /// The tree of shapes the part draws.
    fn tree(&self) -> Option<&XmlElement> {
        self.root.child("cSld")?.child("spTree")
    }
}

// --- Colours ---------------------------------------------------------------------------------

/// A master's colours: its theme's scheme by name, and which scheme colour each role uses.
#[derive(Default)]
struct Palette {
    scheme: HashMap<String, Colour>,
    roles: HashMap<String, String>,
}

/// The roles a master maps to scheme colours; the accents and links keep their own names.
const ROLES: [&str; 4] = ["bg1", "tx1", "bg2", "tx2"];

impl Palette {
    fn read(theme: Option<&XmlElement>, master: &XmlElement) -> Self {
        let mut palette = Palette::default();
        let scheme = theme
            .and_then(|theme| theme.child("themeElements"))
            .and_then(|elements| elements.child("clrScheme"));
        for entry in scheme.into_iter().flat_map(|scheme| &scheme.children) {
            if let Some(colour) = read_colour(entry, &Palette::default()) {
                palette.scheme.insert(entry.name.clone(), colour);
            }
        }
        let Some(map) = master.child("clrMap") else {
            return palette;
        };
        for role in ROLES {
            if let Some(named) = map.attribute(role) {
                palette.roles.insert(role.to_owned(), named.to_owned());
            }
        }
        palette
    }

    fn named(&self, name: &str) -> Option<Colour> {
        let scheme = self.roles.get(name).map_or(name, String::as_str);
        self.scheme.get(scheme).copied()
    }

    /// The colour text has when nothing says: the first text role, else black.
    fn text(&self) -> Colour {
        self.named("tx1").unwrap_or(Colour::BLACK)
    }
}

/// A colour by one of the names the format presets; the common ones only.
fn preset_colour(name: &str) -> Option<Colour> {
    Colour::hex(match name {
        "black" => "000000",
        "white" => "FFFFFF",
        "red" => "FF0000",
        "green" => "008000",
        "blue" => "0000FF",
        "yellow" => "FFFF00",
        "gray" | "grey" => "808080",
        "orange" => "FFA500",
        _ => return None,
    })
}

/// The colour an element such as `srgbClr` or `schemeClr` names, before its adjustments.
fn base_colour(element: &XmlElement, palette: &Palette) -> Option<Colour> {
    let value = element.attribute("val");
    match element.name.as_str() {
        "srgbClr" => Colour::hex(value?),
        "schemeClr" => palette.named(value?),
        "sysClr" => element
            .attribute("lastClr")
            .and_then(Colour::hex)
            .or_else(|| preset_colour(if value? == "window" { "white" } else { "black" })),
        "prstClr" => preset_colour(value?),
        _ => None,
    }
}

/// A colour's lightness from 0 to 1 moved by `change`, its hue and saturation kept.
fn with_lightness(colour: Colour, change: impl Fn(f64) -> f64) -> Colour {
    let [red, green, blue] = [colour.red, colour.green, colour.blue].map(|c| f64::from(c) / 255.0);
    let (high, low) = (red.max(green).max(blue), red.min(green).min(blue));
    let lightness = (high + low) / 2.0;
    let moved = change(lightness).clamp(0.0, 1.0);
    if high == low {
        let grey = (moved * 255.0).round() as u8;
        return Colour {
            alpha: colour.alpha,
            ..Colour::opaque(grey, grey, grey)
        };
    }
    // Scale each channel's distance from the lightness so the hue and saturation stay.
    let reach = 1.0 - (2.0 * lightness - 1.0).abs();
    let saturation = (high - low) / reach;
    let moved_reach = (1.0 - (2.0 * moved - 1.0).abs()) * saturation;
    let channel = |value: f64| {
        let offset = (value - lightness) / (high - low);
        ((moved + offset * moved_reach).clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Colour {
        red: channel(red),
        green: channel(green),
        blue: channel(blue),
        alpha: colour.alpha,
    }
}

/// One adjustment a colour element holds, applied.
fn adjust(colour: Colour, change: &XmlElement) -> Colour {
    let Some(amount) = share(change, "val") else {
        return colour;
    };
    match change.name.as_str() {
        "alpha" => Colour {
            alpha: amount.clamp(0.0, 1.0),
            ..colour
        },
        "lumMod" => with_lightness(colour, |lightness| lightness * amount),
        "lumOff" => with_lightness(colour, |lightness| lightness + amount),
        "tint" => colour.toward(Colour::WHITE, 1.0 - amount),
        "shade" => colour.toward(Colour::BLACK, 1.0 - amount),
        _ => colour,
    }
}

/// The colour held by `holder` (a `solidFill`, a style reference, a scheme entry), adjusted.
fn read_colour(holder: &XmlElement, palette: &Palette) -> Option<Colour> {
    holder.children.iter().find_map(|named| {
        let base = base_colour(named, palette)?;
        Some(named.children.iter().fold(base, adjust))
    })
}

/// What a fill or line says about its paint.
enum Paint {
    /// It says nothing; a style or a parent decides.
    Unset,
    /// It says there is none.
    Empty,
    Solid(Colour),
}

/// The paint one child of a shape's properties gives, if it is a fill. A gradient is its first
/// colour and a pattern its foreground.
fn paint_of(fill: &XmlElement, palette: &Palette) -> Option<Paint> {
    let colour = match fill.name.as_str() {
        "noFill" => return Some(Paint::Empty),
        "solidFill" => read_colour(fill, palette),
        "gradFill" => fill
            .child("gsLst")
            .and_then(|stops| stops.child("gs"))
            .and_then(|stop| read_colour(stop, palette)),
        "pattFill" => fill
            .child("fgClr")
            .and_then(|foreground| read_colour(foreground, palette)),
        _ => return None,
    };
    Some(colour.map_or(Paint::Unset, Paint::Solid))
}

fn read_paint(holder: &XmlElement, palette: &Palette) -> Paint {
    holder
        .children
        .iter()
        .find_map(|child| paint_of(child, palette))
        .unwrap_or(Paint::Unset)
}

/// The colour a shape's style gives for `reference` (`fillRef`, `lnRef`, `fontRef`); none when
/// the reference is to nothing (index 0).
fn style_colour(style: Option<&XmlElement>, reference: &str, palette: &Palette) -> Option<Colour> {
    let reference = style?.child(reference)?;
    if reference.attribute("idx") == Some("0") {
        return None;
    }
    read_colour(reference, palette)
}

// --- Text looks ------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Bullet {
    None,
    Mark(String),
    Number,
}

/// What one source says about how text looks; `None` where it says nothing and the next
/// source decides.
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
    bullet: Option<Bullet>,
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
            bullet: self.bullet.or_else(|| under.bullet.clone()),
        }
    }
}

fn flag(element: &XmlElement, attribute: &str) -> Option<bool> {
    element
        .attribute(attribute)
        .map(|value| matches!(value, "1" | "true"))
}

/// What run properties (`rPr`, `defRPr`, `endParaRPr`) say.
fn read_run_look(properties: &XmlElement, palette: &Palette) -> Look {
    Look {
        // Hundredths of a point.
        size: number(properties, "sz").map(|size| size / 100.0),
        bold: flag(properties, "b"),
        italic: flag(properties, "i"),
        underline: properties.attribute("u").map(|line| line != "none"),
        colour: match read_paint(properties, palette) {
            Paint::Solid(colour) => Some(colour),
            Paint::Empty | Paint::Unset => None,
        },
        ..Look::default()
    }
}

/// A distance element (`lnSpc`, `spcBef`, `spcAft`): a share of the line or points.
fn read_spacing(holder: &XmlElement) -> Option<Spacing> {
    if let Some(by_share) = holder.child("spcPct") {
        return share(by_share, "val").map(Spacing::Share);
    }
    // Hundredths of a point.
    let by_points = holder.child("spcPts")?;
    number(by_points, "val").map(|points| Spacing::Points(points / 100.0))
}

/// The bullet a paragraph's properties set, if they set one. A mark in a symbol face or the
/// private-use range has no glyph in the app's face, so it shows as a plain bullet.
fn read_bullet(properties: &XmlElement) -> Option<Bullet> {
    if properties.child("buNone").is_some() {
        return Some(Bullet::None);
    }
    if properties.child("buAutoNum").is_some() {
        return Some(Bullet::Number);
    }
    let mark = properties.child("buChar")?.attribute("char")?;
    let symbol_face = properties
        .child("buFont")
        .and_then(|face| face.attribute("typeface"))
        .is_some_and(|face| face.starts_with("Wingdings") || face == "Symbol");
    let private = mark
        .chars()
        .any(|character| ('\u{e000}'..='\u{f8ff}').contains(&character));
    Some(Bullet::Mark(if symbol_face || private {
        PLAIN_BULLET.to_owned()
    } else {
        mark.to_owned()
    }))
}

fn read_align(align: &str) -> Option<Align> {
    match align {
        "l" | "just" | "dist" => Some(Align::Start),
        "ctr" => Some(Align::Center),
        "r" => Some(Align::End),
        _ => None,
    }
}

/// What paragraph properties (`pPr`, `lvl1pPr`…) say, with the run look they default to.
fn read_paragraph_look(properties: &XmlElement, palette: &Palette) -> Look {
    let runs = properties
        .child("defRPr")
        .map(|runs| read_run_look(runs, palette))
        .unwrap_or_default();
    Look {
        align: properties.attribute("algn").and_then(read_align),
        left: points(properties, "marL"),
        indent: points(properties, "indent"),
        line: properties.child("lnSpc").and_then(read_spacing),
        before: properties.child("spcBef").and_then(read_spacing),
        after: properties.child("spcAft").and_then(read_spacing),
        bullet: read_bullet(properties),
        ..runs
    }
}

/// A look for each list level, as a list style gives them.
#[derive(Clone, Debug)]
struct Levels([Look; LEVELS]);

impl Levels {
    fn read(list: Option<&XmlElement>, palette: &Palette) -> Self {
        Levels(std::array::from_fn(|level| {
            list.and_then(|list| list.child(&format!("lvl{}pPr", level + 1)))
                .map(|properties| read_paragraph_look(properties, palette))
                .unwrap_or_default()
        }))
    }

    /// The same look at every level.
    fn all(look: Look) -> Self {
        Levels(std::array::from_fn(|_| look.clone()))
    }
}

// --- Placeholders ----------------------------------------------------------------------------

/// What a placeholder stands for: a title, body text, or another slot kept by its own name
/// (a date, a footer, a slide number).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Slot {
    Title,
    Body,
    Other(String),
}

impl Slot {
    /// The slot a placeholder type names; a placeholder without a type holds an object, which
    /// is laid out as body text.
    fn of(kind: Option<&str>) -> Self {
        match kind {
            Some("title" | "ctrTitle") => Slot::Title,
            Some("body" | "subTitle" | "obj") | None => Slot::Body,
            Some(other) => Slot::Other(other.to_owned()),
        }
    }
}

/// A shape's placeholder mark: its type as written and its index.
struct Mark<'a> {
    kind: Option<&'a str>,
    index: Option<&'a str>,
}

fn mark_of(shape: &XmlElement) -> Option<Mark<'_>> {
    let named = shape
        .children
        .iter()
        .find(|child| child.name.starts_with("nv"))?;
    let placeholder = named.child("nvPr")?.child("ph")?;
    Some(Mark {
        kind: placeholder.attribute("type"),
        index: placeholder.attribute("idx"),
    })
}

/// A layout's or master's placeholder: what a slide's placeholder takes from it.
struct Held {
    kind: Option<String>,
    index: Option<String>,
    frame: Option<Rect>,
    levels: Levels,
    body: Option<XmlElement>,
}

fn read_held(tree: Option<&XmlElement>, palette: &Palette) -> Vec<Held> {
    let shapes = tree.into_iter().flat_map(|tree| tree.children_named("sp"));
    shapes
        .filter_map(|shape| {
            let mark = mark_of(shape)?;
            let text = shape.child("txBody");
            Some(Held {
                kind: mark.kind.map(str::to_owned),
                index: mark.index.map(str::to_owned),
                frame: shape_frame(shape),
                levels: Levels::read(text.and_then(|text| text.child("lstStyle")), palette),
                body: text.and_then(|text| text.child("bodyPr")).cloned(),
            })
        })
        .collect()
}

/// The layout placeholder a slide's placeholder fills: the one with its index, else the first
/// of its type.
fn find_in_layout<'a>(held: &'a [Held], mark: &Mark) -> Option<&'a Held> {
    let by_index = mark.index.and_then(|index| {
        held.iter()
            .find(|held| held.index.as_deref() == Some(index))
    });
    by_index.or_else(|| held.iter().find(|held| held.kind.as_deref() == mark.kind))
}

/// The master placeholder for a slot.
fn find_in_master<'a>(held: &'a [Held], slot: &Slot) -> Option<&'a Held> {
    held.iter()
        .find(|held| Slot::of(held.kind.as_deref()) == *slot)
}

/// A master: its part, colours, placeholders and the text styles its slides start from.
struct Master {
    part: Part,
    palette: Palette,
    held: Vec<Held>,
    title: Levels,
    body: Levels,
    other: Levels,
    /// The look text has when nothing else says: the default size in the text colour.
    fallback: Levels,
}

impl Master {
    fn open(archive: &ZipArchive, path: &str) -> Result<Self> {
        let part = Part::open(archive, path)?;
        let theme = match part.related("/theme") {
            Some(theme) => Some(require_part(archive, theme)?),
            None => None,
        };
        let palette = Palette::read(theme.as_ref(), &part.root);
        let styles = part.root.child("txStyles");
        let levels =
            |name: &str| Levels::read(styles.and_then(|styles| styles.child(name)), &palette);
        let fallback = Levels::all(Look {
            size: Some(DEFAULT_TEXT_POINTS),
            colour: Some(palette.text()),
            ..Look::default()
        });
        Ok(Self {
            held: read_held(part.tree(), &palette),
            title: levels("titleStyle"),
            body: levels("bodyStyle"),
            other: levels("otherStyle"),
            fallback,
            palette,
            part,
        })
    }

    fn levels_for(&self, slot: &Slot) -> &Levels {
        match slot {
            Slot::Title => &self.title,
            Slot::Body => &self.body,
            Slot::Other(_) => &self.other,
        }
    }
}

struct Layout {
    part: Part,
    held: Vec<Held>,
    master: Rc<Master>,
}

/// The layouts and masters read so far, by part: slides share them.
struct Templates<'a> {
    archive: &'a ZipArchive<'a>,
    masters: HashMap<String, Rc<Master>>,
    layouts: HashMap<String, Rc<Layout>>,
}

impl Templates<'_> {
    fn master(&mut self, path: &str) -> Result<Rc<Master>> {
        if let Some(known) = self.masters.get(path) {
            return Ok(known.clone());
        }
        let master = Rc::new(Master::open(self.archive, path)?);
        self.masters.insert(path.to_owned(), master.clone());
        Ok(master)
    }

    fn layout(&mut self, path: &str) -> Result<Rc<Layout>> {
        if let Some(known) = self.layouts.get(path) {
            return Ok(known.clone());
        }
        let part = Part::open(self.archive, path)?;
        let master_path = part
            .related("/slideMaster")
            .ok_or_else(|| ReadError::Missing(path.to_owned()))?
            .to_owned();
        let master = self.master(&master_path)?;
        let layout = Rc::new(Layout {
            held: read_held(part.tree(), &master.palette),
            part,
            master,
        });
        self.layouts.insert(path.to_owned(), layout.clone());
        Ok(layout)
    }
}

// --- Frames ----------------------------------------------------------------------------------

/// A point of a frame: `off` or `chOff`.
fn read_point(transform: &XmlElement, name: &str) -> Option<(f64, f64)> {
    let point = transform.child(name)?;
    Some((points(point, "x")?, points(point, "y")?))
}

/// A size of a frame: `ext` or `chExt`.
fn read_size(transform: &XmlElement, name: &str) -> Option<(f64, f64)> {
    let size = transform.child(name)?;
    Some((points(size, "cx")?, points(size, "cy")?))
}

fn read_frame(transform: &XmlElement) -> Option<Rect> {
    let (x, y) = read_point(transform, "off")?;
    let (width, height) = read_size(transform, "ext")?;
    Some(Rect::new(x, y, width, height))
}

/// The `xfrm` in a shape's or picture's properties.
fn transform_of(shape: &XmlElement) -> Option<&XmlElement> {
    shape.child("spPr")?.child("xfrm")
}

fn shape_frame(shape: &XmlElement) -> Option<Rect> {
    transform_of(shape).and_then(read_frame)
}

/// Where a group puts what it holds: each child's frame is scaled, then moved.
#[derive(Clone, Copy)]
struct Placing {
    scale: (f64, f64),
    shift: (f64, f64),
}

impl Placing {
    const UNMOVED: Placing = Placing {
        scale: (1.0, 1.0),
        shift: (0.0, 0.0),
    };

    fn place(self, frame: Rect) -> Rect {
        Rect::new(
            frame.x * self.scale.0 + self.shift.0,
            frame.y * self.scale.1 + self.shift.1,
            frame.width * self.scale.0,
            frame.height * self.scale.1,
        )
    }

    /// The placing for the children of a group inside this one. The group draws its children's
    /// space (`chOff`, `chExt`) into its own frame (`off`, `ext`); a group that gives no
    /// frames, or an empty child space, places its children as they are.
    fn within(self, group: Option<&XmlElement>) -> Placing {
        let frames = group.and_then(|group| {
            Some((
                read_point(group, "off")?,
                read_size(group, "ext")?,
                read_point(group, "chOff")?,
                read_size(group, "chExt")?,
            ))
        });
        let Some((at, size, child_at, child_size)) = frames else {
            return self;
        };
        if child_size.0 <= 0.0 || child_size.1 <= 0.0 {
            return self;
        }
        let scale = (size.0 / child_size.0, size.1 / child_size.1);
        let shift = (at.0 - child_at.0 * scale.0, at.1 - child_at.1 * scale.1);
        Placing {
            scale: (self.scale.0 * scale.0, self.scale.1 * scale.1),
            shift: (
                shift.0 * self.scale.0 + self.shift.0,
                shift.1 * self.scale.1 + self.shift.1,
            ),
        }
    }
}

// --- Text ------------------------------------------------------------------------------------

/// The number each list level has reached, for numbered paragraphs.
#[derive(Default)]
struct Numbering([usize; LEVELS]);

impl Numbering {
    /// The mark for a paragraph at `level`: its bullet, or its number, or none. A paragraph
    /// that is not numbered ends the numbering at its level and below.
    fn mark(&mut self, level: usize, bullet: Option<&Bullet>) -> Option<String> {
        for deeper in &mut self.0[level + 1..] {
            *deeper = 0;
        }
        match bullet {
            Some(Bullet::Number) => {
                self.0[level] += 1;
                Some(format!("{}.", self.0[level]))
            }
            Some(Bullet::Mark(mark)) => {
                self.0[level] = 0;
                Some(mark.clone())
            }
            Some(Bullet::None) | None => {
                self.0[level] = 0;
                None
            }
        }
    }
}

fn run_of(text: &str, look: &Look) -> Run {
    Run {
        text: text.to_owned(),
        size: look.size.unwrap_or(DEFAULT_TEXT_POINTS),
        bold: look.bold.unwrap_or(false),
        italic: look.italic.unwrap_or(false),
        underline: look.underline.unwrap_or(false),
        colour: look.colour.unwrap_or(Colour::BLACK),
    }
}

/// The run a paragraph's child is: text, a field's text, or a line break. `None` for its
/// properties.
fn read_run(child: &XmlElement, paragraph: &Look, palette: &Palette) -> Option<Run> {
    match child.name.as_str() {
        "r" | "fld" => {
            let own = child
                .child("rPr")
                .map(|properties| read_run_look(properties, palette))
                .unwrap_or_default();
            let text = child.child("t").map_or("", |text| text.text.as_str());
            Some(run_of(text, &own.over(paragraph)))
        }
        "br" => Some(run_of("\n", paragraph)),
        _ => None,
    }
}

/// A paragraph in the look its own properties give over each of `sources`, nearest first.
fn read_paragraph(
    paragraph: &XmlElement,
    sources: &[&Levels],
    palette: &Palette,
    numbering: &mut Numbering,
) -> Paragraph {
    let own = paragraph.child("pPr");
    let level = own
        .and_then(|own| number(own, "lvl"))
        .map_or(0, |level| (level.max(0.0) as usize).min(LEVELS - 1));
    let own_look = own
        .map(|own| read_paragraph_look(own, palette))
        .unwrap_or_default();
    let look = sources
        .iter()
        .fold(own_look, |look, levels| look.over(&levels.0[level]));
    let runs: Vec<Run> = paragraph
        .children
        .iter()
        .filter_map(|child| read_run(child, &look, palette))
        .collect();
    let written = runs.iter().any(|run| !run.text.trim().is_empty());
    let bullet = numbering.mark(level, look.bullet.as_ref().filter(|_| written));
    Paragraph {
        align: look.align.unwrap_or_default(),
        left: look.left.unwrap_or(0.0),
        indent: look.indent.unwrap_or(0.0),
        line: look.line.unwrap_or(Spacing::Share(1.0)),
        before: look.before.unwrap_or(Spacing::Points(0.0)),
        after: look.after.unwrap_or(Spacing::Points(0.0)),
        bullet,
        size: look.size.unwrap_or(DEFAULT_TEXT_POINTS),
        runs,
    }
}

/// The first of the body properties, nearest first, that sets `attribute`.
fn body_setting<'a>(bodies: &[&'a XmlElement], attribute: &str) -> Option<&'a str> {
    bodies.iter().find_map(|body| body.attribute(attribute))
}

fn read_anchor(anchor: Option<&str>) -> Anchor {
    match anchor {
        Some("ctr") => Anchor::Middle,
        Some("b") => Anchor::Bottom,
        _ => Anchor::Top,
    }
}

/// The insets a text frame keeps clear, from the attributes named in `names` (left, top,
/// right, bottom) of the first of `bodies` that sets each.
fn read_insets(bodies: &[&XmlElement], names: [&str; 4]) -> [f64; 4] {
    let defaults = [
        DEFAULT_SIDE_INSET_POINTS,
        DEFAULT_END_INSET_POINTS,
        DEFAULT_SIDE_INSET_POINTS,
        DEFAULT_END_INSET_POINTS,
    ];
    std::array::from_fn(|side| {
        bodies
            .iter()
            .find_map(|body| points(body, names[side]))
            .unwrap_or(defaults[side])
    })
}

/// A shape's text in the look its own list style gives over `sources`, with the frame settings
/// its own body properties give over `inherited`. `None` when it holds no text to show.
fn read_body(
    text: &XmlElement,
    sources: &[&Levels],
    inherited: &[&XmlElement],
    palette: &Palette,
) -> Option<Body> {
    let own_levels = Levels::read(text.child("lstStyle"), palette);
    let mut all_sources = vec![&own_levels];
    all_sources.extend_from_slice(sources);
    let mut numbering = Numbering::default();
    let paragraphs: Vec<Paragraph> = text
        .children_named("p")
        .map(|paragraph| read_paragraph(paragraph, &all_sources, palette, &mut numbering))
        .collect();
    let written = paragraphs
        .iter()
        .flat_map(|paragraph| &paragraph.runs)
        .any(|run| !run.text.trim().is_empty());
    if !written {
        return None;
    }
    let own = text.child("bodyPr");
    let bodies: Vec<&XmlElement> = own.into_iter().chain(inherited.iter().copied()).collect();
    // The shrink that made the text fit is saved only on the shape itself.
    let fitted = own.and_then(|own| own.child("normAutofit"));
    Some(Body {
        insets: read_insets(&bodies, ["lIns", "tIns", "rIns", "bIns"]),
        anchor: read_anchor(body_setting(&bodies, "anchor")),
        wraps: body_setting(&bodies, "wrap") != Some("none"),
        scale: fitted
            .and_then(|fitted| share(fitted, "fontScale"))
            .unwrap_or(1.0),
        spacing_scale: 1.0
            - fitted
                .and_then(|fitted| share(fitted, "lnSpcReduction"))
                .unwrap_or(0.0),
        paragraphs,
    })
}

// --- Shapes ----------------------------------------------------------------------------------

/// The pictures the slides show, each named once however many slides show it.
#[derive(Default)]
struct Pictures(Vec<String>);

impl Pictures {
    /// The index of the picture at `path`. A picture the file does not hold is logged and not
    /// drawn.
    fn index_of(&mut self, archive: &ZipArchive, path: &str) -> Option<usize> {
        if let Some(known) = self.0.iter().position(|name| name == path) {
            return Some(known);
        }
        if !archive.entry_names().any(|name| name == path) {
            eprintln!("the presentation has no picture \"{path}\"");
            return None;
        }
        self.0.push(path.to_owned());
        Some(self.0.len() - 1)
    }
}

/// What a tree of shapes is read with: the file, the colours and relationships of the part
/// being read, and, on a slide, the layout and master its placeholders fill from. A layout's or
/// master's own placeholders are slots, not content, so without `fills_from` they are skipped.
struct Scope<'a> {
    archive: &'a ZipArchive<'a>,
    part: &'a Part,
    master: &'a Master,
    fills_from: Option<&'a Layout>,
    /// The presentation's default text style, for text outside placeholders.
    defaults: &'a Levels,
}

/// Where what is read goes.
struct Read<'a> {
    items: &'a mut Vec<Item>,
    pictures: &'a mut Pictures,
}

/// What a shape takes from the placeholders it fills.
#[derive(Default)]
struct Inherited<'a> {
    frame: Option<Rect>,
    levels: Vec<&'a Levels>,
    bodies: Vec<&'a XmlElement>,
}

impl<'a> Scope<'a> {
    /// What a slide's placeholder takes from its layout's and master's.
    fn inherited(&self, mark: &Mark, layout: &'a Layout) -> Inherited<'a> {
        let slot = Slot::of(mark.kind);
        let from_layout = find_in_layout(&layout.held, mark);
        // A layout's placeholder names its own type, which finds the master's.
        let master_slot = from_layout.map_or(slot.clone(), |held| Slot::of(held.kind.as_deref()));
        let from_master = find_in_master(&self.master.held, &master_slot);
        let found: Vec<&'a Held> = from_layout.into_iter().chain(from_master).collect();
        let mut levels: Vec<&'a Levels> = found.iter().map(|held| &held.levels).collect();
        levels.push(self.master.levels_for(&master_slot));
        Inherited {
            frame: found.iter().find_map(|held| held.frame),
            levels,
            bodies: found.iter().filter_map(|held| held.body.as_ref()).collect(),
        }
    }

    /// What a shape starts from: its placeholders' settings on a slide, the presentation's
    /// defaults elsewhere. `None` for a placeholder that is only a slot.
    fn starts_from(&self, shape: &XmlElement) -> Option<Inherited<'a>> {
        let mut inherited = match (mark_of(shape), self.fills_from) {
            (Some(_), None) => return None,
            (Some(mark), Some(layout)) => self.inherited(&mark, layout),
            (None, _) => Inherited {
                levels: vec![self.defaults],
                ..Inherited::default()
            },
        };
        inherited.levels.push(&self.master.fallback);
        Some(inherited)
    }
}

/// A rounded box's radius: the share its adjustment gives of the frame's shorter side.
fn rounding(geometry: &XmlElement, frame: Rect) -> f64 {
    let adjusted = geometry
        .child("avLst")
        .and_then(|list| list.child("gd"))
        .and_then(|guide| guide.attribute("fmla"))
        .and_then(|formula| formula.strip_prefix("val "))
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value / WHOLE);
    adjusted.unwrap_or(DEFAULT_ROUNDING).clamp(0.0, 0.5) * frame.width.min(frame.height)
}

/// A shape's outline. A shape this reader has no outline for is a box when it holds text and
/// nothing when it does not.
fn read_form(shape: &XmlElement, frame: Rect, written: bool) -> Option<Form> {
    let properties = shape.child("spPr");
    let Some(geometry) = properties.and_then(|properties| properties.child("prstGeom")) else {
        let custom = properties.is_some_and(|properties| properties.child("custGeom").is_some());
        return (!custom || written).then_some(Form::Box);
    };
    match geometry.attribute("prst") {
        Some("rect") => Some(Form::Box),
        Some("roundRect" | "round1Rect" | "round2SameRect" | "round2DiagRect") => {
            Some(Form::Rounded(rounding(geometry, frame)))
        }
        Some("ellipse") => Some(Form::Oval),
        Some("line" | "straightConnector1") => {
            let flipped = |attribute: &str| {
                transform_of(shape)
                    .and_then(|transform| flag(transform, attribute))
                    .unwrap_or(false)
            };
            Some(Form::Line {
                rising: flipped("flipH") != flipped("flipV"),
            })
        }
        _ => written.then_some(Form::Box),
    }
}

fn read_fill(shape: &XmlElement, palette: &Palette) -> Option<Colour> {
    let own = shape
        .child("spPr")
        .map_or(Paint::Unset, |properties| read_paint(properties, palette));
    match own {
        Paint::Solid(colour) => Some(colour),
        Paint::Empty => None,
        Paint::Unset => style_colour(shape.child("style"), "fillRef", palette),
    }
}

fn read_stroke(shape: &XmlElement, palette: &Palette) -> Option<Stroke> {
    let line = shape
        .child("spPr")
        .and_then(|properties| properties.child("ln"));
    let colour = match line.map_or(Paint::Unset, |line| read_paint(line, palette)) {
        Paint::Solid(colour) => Some(colour),
        Paint::Empty => None,
        Paint::Unset => style_colour(shape.child("style"), "lnRef", palette),
    }?;
    Some(Stroke {
        colour,
        width: line
            .and_then(|line| points(line, "w"))
            .unwrap_or(DEFAULT_STROKE_POINTS),
    })
}

/// Reads a shape or a connector. One with no frame, no outline, or nothing to show is left
/// out.
fn read_shape(shape: &XmlElement, scope: &Scope, placing: Placing, read: &mut Read) {
    let Some(inherited) = scope.starts_from(shape) else {
        return;
    };
    let Some(frame) = shape_frame(shape).or(inherited.frame) else {
        return;
    };
    let palette = &scope.master.palette;
    // A shape's style colours its text unless the text says otherwise.
    let styled = style_colour(shape.child("style"), "fontRef", palette).map(|colour| {
        Levels::all(Look {
            colour: Some(colour),
            ..Look::default()
        })
    });
    let sources: Vec<&Levels> = styled.iter().chain(inherited.levels).collect();
    let text = shape
        .child("txBody")
        .and_then(|text| read_body(text, &sources, &inherited.bodies, palette));
    let Some(form) = read_form(shape, frame, text.is_some()) else {
        return;
    };
    let (fill, stroke) = (read_fill(shape, palette), read_stroke(shape, palette));
    if fill.is_none() && stroke.is_none() && text.is_none() {
        return;
    }
    read.items.push(Item::Shape(Shape {
        frame: placing.place(frame),
        form,
        fill,
        stroke,
        text,
    }));
}

fn read_placed(picture: &XmlElement, scope: &Scope, placing: Placing, read: &mut Read) {
    let Some(frame) = shape_frame(picture) else {
        return;
    };
    let Some(fill) = picture.child("blipFill") else {
        return;
    };
    let path = fill
        .child("blip")
        .and_then(|blip| blip.attribute("r:embed"))
        .and_then(|id| scope.part.target(id));
    let Some(index) = path.and_then(|path| read.pictures.index_of(scope.archive, path)) else {
        return;
    };
    let cut = fill.child("srcRect");
    let crop = ["l", "t", "r", "b"].map(|side| {
        cut.and_then(|cut| share(cut, side))
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    });
    read.items.push(Item::Picture(Placed {
        frame: placing.place(frame),
        picture: index,
        crop,
    }));
}

// --- Tables ----------------------------------------------------------------------------------

/// How a table's style colours it. The file names a style by id and keeps its looks elsewhere;
/// every style is drawn as the default one.
struct TableLook {
    styled: bool,
    header: bool,
    banded: bool,
    accent: Colour,
    light: Colour,
}

impl TableLook {
    fn read(table: &XmlElement, palette: &Palette) -> Self {
        let properties = table.child("tblPr");
        let set = |attribute: &str| {
            properties
                .and_then(|properties| flag(properties, attribute))
                .unwrap_or(false)
        };
        let styled = properties
            .and_then(|properties| properties.child("tableStyleId"))
            .is_some_and(|id| !id.text.trim().is_empty());
        TableLook {
            styled,
            header: styled && set("firstRow"),
            banded: styled && set("bandRow"),
            accent: palette.named("accent1").unwrap_or(Colour::BLACK),
            light: palette.named("bg1").unwrap_or(Colour::WHITE),
        }
    }

    fn is_header(&self, row: usize) -> bool {
        self.header && row == 0
    }

    /// The fill a cell in `row` has when it sets none.
    fn fill(&self, row: usize) -> Option<Colour> {
        if !self.styled {
            return None;
        }
        if self.is_header(row) {
            return Some(self.accent);
        }
        let band = row - usize::from(self.header);
        let tint = if self.banded && band.is_multiple_of(2) {
            BAND_TINTS.0
        } else {
            BAND_TINTS.1
        };
        Some(self.accent.toward(Colour::WHITE, tint))
    }

    /// The look a header cell's text starts from.
    fn header_text(&self) -> Levels {
        Levels::all(Look {
            bold: Some(true),
            colour: Some(self.light),
            ..Look::default()
        })
    }

    fn stroke(&self) -> Option<Stroke> {
        self.styled.then_some(Stroke {
            colour: self.light,
            width: TABLE_STROKE_POINTS,
        })
    }
}

/// How many grid columns or rows a cell covers.
fn span(cell: &XmlElement, attribute: &str) -> usize {
    number(cell, attribute).map_or(1, |span| (span as usize).max(1))
}

/// The sum of `sizes` from `first` over `count` of them.
fn spanned(sizes: &[f64], first: usize, count: usize) -> f64 {
    sizes.iter().skip(first).take(count).sum()
}

/// A table being read: its grid in points, its look and where it sits.
struct Grid<'a> {
    frame: Rect,
    widths: Vec<f64>,
    heights: Vec<f64>,
    look: TableLook,
    scope: &'a Scope<'a>,
    placing: Placing,
}

impl Grid<'_> {
    /// One cell as a box with its text. A cell merged into another draws nothing of its own.
    fn read_cell(&self, cell: &XmlElement, row: usize, column: usize, read: &mut Read) {
        let merged = |attribute: &str| flag(cell, attribute).unwrap_or(false);
        if merged("hMerge") || merged("vMerge") {
            return;
        }
        let palette = &self.scope.master.palette;
        let properties = cell.child("tcPr");
        let frame = Rect::new(
            self.frame.x + spanned(&self.widths, 0, column),
            self.frame.y + spanned(&self.heights, 0, row),
            spanned(&self.widths, column, span(cell, "gridSpan")),
            spanned(&self.heights, row, span(cell, "rowSpan")),
        );
        let own = properties.map_or(Paint::Unset, |properties| read_paint(properties, palette));
        let fill = match own {
            Paint::Solid(colour) => Some(colour),
            Paint::Empty => None,
            Paint::Unset => self.look.fill(row),
        };
        let header = self.look.is_header(row).then(|| self.look.header_text());
        let sources: Vec<&Levels> = header
            .iter()
            .chain([self.scope.defaults, &self.scope.master.fallback])
            .collect();
        let text = cell
            .child("txBody")
            .and_then(|text| read_body(text, &sources, &[], palette))
            .map(|body| Body {
                // A cell keeps its margins and anchor in its own properties.
                insets: read_insets(properties.as_slice(), ["marL", "marT", "marR", "marB"]),
                anchor: read_anchor(
                    properties.and_then(|properties| properties.attribute("anchor")),
                ),
                wraps: true,
                ..body
            });
        read.items.push(Item::Shape(Shape {
            frame: self.placing.place(frame),
            form: Form::Box,
            fill,
            stroke: self.look.stroke(),
            text,
        }));
    }

    fn read_row(&self, row: &XmlElement, index: usize, read: &mut Read) {
        let mut column = 0;
        for cell in row.children_named("tc") {
            self.read_cell(cell, index, column, read);
            column += span(cell, "gridSpan");
        }
    }
}

/// Reads a graphic frame that holds a table; a chart or diagram in one is not drawn.
fn read_table(holder: &XmlElement, scope: &Scope, placing: Placing, read: &mut Read) {
    let table = holder
        .child("graphic")
        .and_then(|graphic| graphic.child("graphicData"))
        .and_then(|data| data.child("tbl"));
    let (Some(table), Some(frame)) = (table, holder.child("xfrm").and_then(read_frame)) else {
        return;
    };
    let widths = table
        .child("tblGrid")
        .into_iter()
        .flat_map(|grid| grid.children_named("gridCol"))
        .map(|column| points(column, "w").unwrap_or(0.0))
        .collect();
    let grid = Grid {
        frame,
        widths,
        heights: table
            .children_named("tr")
            .map(|row| points(row, "h").unwrap_or(0.0))
            .collect(),
        look: TableLook::read(table, &scope.master.palette),
        scope,
        placing,
    };
    for (index, row) in table.children_named("tr").enumerate() {
        grid.read_row(row, index, read);
    }
}

// --- Slides ----------------------------------------------------------------------------------

/// Reads one thing a tree holds. Content saved twice for older readers (`AlternateContent`) is
/// read in its plain form.
fn read_item(item: &XmlElement, scope: &Scope, placing: Placing, read: &mut Read) {
    match item.name.as_str() {
        "sp" | "cxnSp" => read_shape(item, scope, placing, read),
        "pic" => read_placed(item, scope, placing, read),
        "graphicFrame" => read_table(item, scope, placing, read),
        "grpSp" => {
            let group = item
                .child("grpSpPr")
                .and_then(|properties| properties.child("xfrm"));
            read_tree(Some(item), scope, placing.within(group), read);
        }
        "AlternateContent" => {
            let shown = item.child("Fallback").or_else(|| item.child("Choice"));
            read_tree(shown, scope, placing, read);
        }
        _ => {}
    }
}

fn read_tree(tree: Option<&XmlElement>, scope: &Scope, placing: Placing, read: &mut Read) {
    for item in tree.into_iter().flat_map(|tree| &tree.children) {
        read_item(item, scope, placing, read);
    }
}

/// The background a slide, layout or master sets, if it sets a colour.
fn read_ground(root: &XmlElement, palette: &Palette) -> Option<Colour> {
    let ground = root.child("cSld")?.child("bg")?;
    if let Some(reference) = ground.child("bgRef") {
        return read_colour(reference, palette);
    }
    match read_paint(ground.child("bgPr")?, palette) {
        Paint::Solid(colour) => Some(colour),
        Paint::Empty | Paint::Unset => None,
    }
}

/// Whether a slide or layout draws the shapes of what it is built on.
fn shows_template_shapes(root: &XmlElement) -> bool {
    flag(root, "showMasterSp").unwrap_or(true)
}

fn read_slide(
    path: &str,
    templates: &mut Templates,
    defaults: &Levels,
    pictures: &mut Pictures,
) -> Result<Slide> {
    let archive = templates.archive;
    let part = Part::open(archive, path)?;
    let layout_path = part
        .related("/slideLayout")
        .ok_or_else(|| ReadError::Missing(path.to_owned()))?
        .to_owned();
    let layout = templates.layout(&layout_path)?;
    let master = &layout.master;
    let ground = [&part.root, &layout.part.root, &master.part.root]
        .into_iter()
        .find_map(|root| read_ground(root, &master.palette))
        .unwrap_or(Colour::WHITE);
    let mut items = Vec::new();
    let mut read = Read {
        items: &mut items,
        pictures,
    };
    let scope_of = |part, fills_from| Scope {
        archive,
        part,
        master,
        fills_from,
        defaults,
    };
    if shows_template_shapes(&part.root) {
        if shows_template_shapes(&layout.part.root) {
            let scope = scope_of(&master.part, None);
            read_tree(master.part.tree(), &scope, Placing::UNMOVED, &mut read);
        }
        let scope = scope_of(&layout.part, None);
        read_tree(layout.part.tree(), &scope, Placing::UNMOVED, &mut read);
    }
    let scope = scope_of(&part, Some(&layout));
    read_tree(part.tree(), &scope, Placing::UNMOVED, &mut read);
    Ok(Slide { ground, items })
}

/// Reads a presentation held in memory.
pub fn parse(bytes: &[u8]) -> Result<Deck> {
    let archive = ZipArchive::open(bytes)?;
    let package = read_relationships(&archive, "")?;
    let path = find_relationship(&package, "/officeDocument").ok_or(ReadError::NotItsKind)?;
    let presentation = Part::open(&archive, path)?;
    if presentation.root.name != "presentation" {
        return Err(ReadError::NotItsKind);
    }
    let (width, height) = presentation
        .root
        .child("sldSz")
        .and_then(|size| Some((points(size, "cx")?, points(size, "cy")?)))
        .filter(|(width, height)| *width > 0.0 && *height > 0.0)
        .unwrap_or(DEFAULT_SLIDE_POINTS);
    // Text outside placeholders is coloured by role before any master is known; roles there
    // are rare, so they are left to the master's fallback.
    let defaults = Levels::read(
        presentation.root.child("defaultTextStyle"),
        &Palette::default(),
    );
    let mut templates = Templates {
        archive: &archive,
        masters: HashMap::new(),
        layouts: HashMap::new(),
    };
    let mut pictures = Pictures::default();
    let mut slides = Vec::new();
    let listed = presentation
        .root
        .child("sldIdLst")
        .into_iter()
        .flat_map(|list| list.children_named("sldId"));
    for slide in listed {
        let Some(path) = slide
            .attribute("r:id")
            .and_then(|id| presentation.target(id))
        else {
            continue;
        };
        slides.push(read_slide(path, &mut templates, &defaults, &mut pictures)?);
    }
    Ok(Deck {
        width,
        height,
        slides,
        fit: Fit::Whole,
        pictures: pictures.0.into_iter().map(Source::Entry).collect(),
    })
}

/// Reads the presentation at `path`.
pub fn read(path: &std::path::Path) -> Result<Deck> {
    parse(&read_bytes(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::parse_xml;

    fn element(source: &str) -> XmlElement {
        parse_xml(source).expect("XML")
    }

    fn palette() -> Palette {
        let theme = element(
            r#"<a:theme><a:themeElements><a:clrScheme>
                 <a:dk1><a:sysClr val="windowText" lastClr="111111"/></a:dk1>
                 <a:lt1><a:srgbClr val="FFFFFF"/></a:lt1>
                 <a:accent1><a:srgbClr val="4472C4"/></a:accent1>
               </a:clrScheme></a:themeElements></a:theme>"#,
        );
        let master = element(r#"<p:sldMaster><p:clrMap bg1="lt1" tx1="dk1"/></p:sldMaster>"#);
        Palette::read(Some(&theme), &master)
    }

    #[test]
    fn colours_come_from_hex_roles_and_the_theme_and_take_their_adjustments() {
        let palette = palette();
        let read = |inside: &str| read_colour(&element(&format!("<f>{inside}</f>")), &palette);
        assert_eq!(read(r#"<a:srgbClr val="1F6F4A"/>"#), Colour::hex("1F6F4A"));
        assert_eq!(read(r#"<a:schemeClr val="tx1"/>"#), Colour::hex("111111"));
        assert_eq!(
            read(r#"<a:schemeClr val="accent1"/>"#),
            Colour::hex("4472C4")
        );
        assert_eq!(read(r#"<a:schemeClr val="accent9"/>"#), None);
        assert_eq!(read(r#"<a:prstClr val="red"/>"#), Colour::hex("FF0000"));
        let faded = read(r#"<a:srgbClr val="000000"><a:alpha val="50000"/></a:srgbClr>"#);
        assert_eq!(faded.map(|colour| colour.alpha), Some(0.5));
        // Half the lightness of mid grey, then a tenth added back.
        assert_eq!(
            read(
                r#"<a:srgbClr val="808080"><a:lumMod val="50000"/><a:lumOff val="10000"/></a:srgbClr>"#
            ),
            Some(Colour::opaque(90, 90, 90))
        );
        // A pure hue at three quarters of its lightness keeps its hue.
        assert_eq!(
            read(r#"<a:srgbClr val="FF0000"><a:lumMod val="75000"/></a:srgbClr>"#),
            Some(Colour::opaque(191, 0, 0))
        );
    }

    #[test]
    fn a_group_scales_and_moves_what_it_holds() {
        let group = element(
            r#"<a:xfrm><a:off x="1270000" y="0"/><a:ext cx="2540000" cy="1270000"/>
                 <a:chOff x="0" y="0"/><a:chExt cx="1270000" cy="1270000"/></a:xfrm>"#,
        );
        let placing = Placing::UNMOVED.within(Some(&group));
        assert_eq!(
            placing.place(Rect::new(50.0, 10.0, 25.0, 20.0)),
            Rect::new(200.0, 10.0, 50.0, 20.0)
        );
        // A group inside it halves again.
        let inner = element(
            r#"<a:xfrm><a:off x="0" y="0"/><a:ext cx="635000" cy="635000"/>
                 <a:chOff x="0" y="0"/><a:chExt cx="1270000" cy="1270000"/></a:xfrm>"#,
        );
        assert_eq!(
            placing
                .within(Some(&inner))
                .place(Rect::new(50.0, 10.0, 25.0, 20.0)),
            Rect::new(150.0, 5.0, 25.0, 10.0)
        );
        assert_eq!(
            Placing::UNMOVED
                .within(None)
                .place(Rect::new(1.0, 2.0, 3.0, 4.0)),
            Rect::new(1.0, 2.0, 3.0, 4.0)
        );
    }

    #[test]
    fn text_takes_each_setting_from_the_nearest_source_that_gives_it() {
        let palette = palette();
        let list = element(
            r#"<a:lstStyle>
                 <a:lvl1pPr algn="ctr"><a:buChar char="–"/><a:defRPr sz="2400" b="1"/></a:lvl1pPr>
                 <a:lvl2pPr marL="457200" indent="-228600"><a:buAutoNum type="arabicPeriod"/><a:defRPr sz="2000"/></a:lvl2pPr>
               </a:lstStyle>"#,
        );
        let inherited = Levels::read(Some(&list), &palette);
        let fallback = Levels::all(Look {
            size: Some(DEFAULT_TEXT_POINTS),
            colour: Some(palette.text()),
            ..Look::default()
        });
        let text = element(
            r#"<p:txBody><a:bodyPr anchor="b" lIns="0"><a:normAutofit fontScale="90000" lnSpcReduction="10000"/></a:bodyPr>
                 <a:p><a:r><a:rPr i="1"><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:rPr><a:t>One</a:t></a:r><a:br/><a:r><a:rPr sz="1200" b="0"/><a:t>two</a:t></a:r></a:p>
                 <a:p><a:pPr lvl="1"/><a:r><a:t>first</a:t></a:r></a:p>
                 <a:p><a:pPr lvl="1"/><a:r><a:t>second</a:t></a:r></a:p>
                 <a:p><a:pPr lvl="1"/></a:p>
                 <a:p><a:pPr lvl="1" algn="r"><a:buNone/></a:pPr><a:r><a:t>plain</a:t></a:r></a:p>
               </p:txBody>"#,
        );
        let outer = element(r#"<a:bodyPr wrap="none" rIns="127000" anchor="ctr"/>"#);
        let body = read_body(&text, &[&inherited, &fallback], &[&outer], &palette).expect("text");
        assert_eq!(body.anchor, Anchor::Bottom);
        assert!(!body.wraps);
        assert_eq!(body.insets, [0.0, 3.6, 10.0, 3.6]);
        assert_eq!((body.scale, body.spacing_scale), (0.9, 0.9));

        let first = &body.paragraphs[0];
        assert_eq!(
            (first.align, first.bullet.as_deref()),
            (Align::Center, Some("–"))
        );
        let looks: Vec<(&str, f64, bool, bool, Colour)> = first
            .runs
            .iter()
            .map(|run| {
                (
                    run.text.as_str(),
                    run.size,
                    run.bold,
                    run.italic,
                    run.colour,
                )
            })
            .collect();
        let (accent, dark) = (
            Colour::hex("4472C4").unwrap(),
            Colour::hex("111111").unwrap(),
        );
        assert_eq!(
            looks,
            [
                ("One", 24.0, true, true, accent),
                ("\n", 24.0, true, false, dark),
                ("two", 12.0, false, false, dark)
            ]
        );
        let marks: Vec<Option<&str>> = body.paragraphs[1..]
            .iter()
            .map(|paragraph| paragraph.bullet.as_deref())
            .collect();
        // Numbers count the written paragraphs; an empty one ends the run.
        assert_eq!(marks, [Some("1."), Some("2."), None, None]);
        let second = &body.paragraphs[1];
        assert_eq!(
            (second.left, second.indent, second.size),
            (36.0, -18.0, 20.0)
        );
        assert_eq!(body.paragraphs[4].align, Align::End);

        let empty =
            element(r#"<p:txBody><a:bodyPr/><a:p><a:r><a:t> </a:t></a:r></a:p></p:txBody>"#);
        assert_eq!(read_body(&empty, &[&fallback], &[], &palette), None);
    }
}
