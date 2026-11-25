//! Reads an OpenDocument presentation (`.odp` and its template form) into a `Deck`: a zip whose
//! `content.xml` holds the slides and `styles.xml` the master pages, their size and their
//! shapes.
//!
//! A shape's fill, line, text place and insets come from its presentation style, then its
//! graphic style and their parents, then the graphic default; its text's look from each
//! paragraph's style over the same chain. A master page's own shapes are drawn under each slide
//! that uses it; its placeholders are slots, not content, and are not.
//!
//! Left out: what `deck` says no deck draws, and also notes, transforms (rotation, skew),
//! gradients and bitmaps as fills (drawn as their first colour or not at all), and shapes other
//! than boxes, rounded boxes, ovals and lines.

use super::odf::{self, each_paragraph, length_of, InList, Look, Numbering, Styles};
use super::{
    read_bytes, Anchor, Body, Colour, Deck, Fit, Form, Item, Placed, ReadError, Result, Shape,
    Slide, Source, Stroke,
};
use crate::geometry::Rect;
use crate::package::{read_part, XmlElement, ZipArchive};

const CONTENT: &str = "content.xml";
const STYLES: &str = "styles.xml";
/// A slide's size when the file gives none: 4:3 at 10 by 7.5 inches.
const DEFAULT_SLIDE_POINTS: (f64, f64) = (720.0, 540.0);
/// The space kept clear inside a text frame when its style gives none: sides, then top and
/// bottom.
const DEFAULT_SIDE_INSET_POINTS: f64 = 7.2;
const DEFAULT_END_INSET_POINTS: f64 = 3.6;
/// A line's thickness when its style gives none: a hairline.
const DEFAULT_STROKE_POINTS: f64 = 0.75;
/// A rounded box's corner radius as a share of its shorter side.
const ROUNDING: f64 = 0.1;

/// The styles a shape names, nearest first: presentation style and parents, then graphic style
/// and parents, then the graphic default.
fn chain_of<'a>(shape: &XmlElement, styles: &'a Styles) -> Vec<&'a XmlElement> {
    let mut chain: Vec<&XmlElement> = styles
        .chain("presentation", shape.attribute("presentation:style-name"))
        .into_iter()
        .chain(styles.chain("graphic", shape.attribute("draw:style-name")))
        .map(|style| &style.element)
        .collect();
    chain.extend(styles.default_style("graphic").map(|style| &style.element));
    chain
}

/// The first value the chain's graphic properties give for `attribute`.
fn setting<'a>(chain: &[&'a XmlElement], attribute: &str) -> Option<&'a str> {
    chain.iter().find_map(|style| {
        style
            .child("graphic-properties")
            .and_then(|properties| properties.attribute(attribute))
    })
}

fn read_fill(chain: &[&XmlElement]) -> Option<Colour> {
    match setting(chain, "draw:fill") {
        Some("solid" | "gradient") => setting(chain, "draw:fill-color").and_then(Colour::hex),
        _ => None,
    }
}

fn read_stroke(chain: &[&XmlElement]) -> Option<Stroke> {
    if !matches!(setting(chain, "draw:stroke"), Some("solid" | "dash")) {
        return None;
    }
    Some(Stroke {
        colour: setting(chain, "svg:stroke-color")
            .and_then(Colour::hex)
            .unwrap_or(Colour::BLACK),
        width: setting(chain, "svg:stroke-width")
            .and_then(odf::length)
            .filter(|width| *width > 0.0)
            .unwrap_or(DEFAULT_STROKE_POINTS),
    })
}

/// The look a shape's text starts from: its presentation style's, then its graphic style's,
/// then the graphic default's.
fn shape_look(shape: &XmlElement, styles: &Styles) -> Look {
    styles
        .own_look("presentation", shape.attribute("presentation:style-name"))
        .over(&styles.own_look("graphic", shape.attribute("draw:style-name")))
        .over(&styles.default_look("graphic"))
}

fn frame_of(shape: &XmlElement) -> Option<Rect> {
    Some(Rect::new(
        length_of(shape, "svg:x").unwrap_or(0.0),
        length_of(shape, "svg:y").unwrap_or(0.0),
        length_of(shape, "svg:width")?,
        length_of(shape, "svg:height")?,
    ))
}

/// What a slide's shapes are read with.
struct Reader<'a> {
    archive: &'a ZipArchive<'a>,
    styles: Styles,
    pictures: Vec<Source>,
}

impl Reader<'_> {
    /// The text a shape or text box holds, set in its style; `None` when it holds none.
    fn read_body(&self, holder: &XmlElement, shape: &XmlElement, anchor: Anchor) -> Option<Body> {
        let chain = chain_of(shape, &self.styles);
        let under = shape_look(shape, &self.styles);
        let mut numbering = Numbering::default();
        let mut found = Vec::new();
        each_paragraph(holder, None, &mut |element, list: Option<&InList>| {
            found.push(
                odf::read_paragraph(element, &self.styles, &under, list, &mut numbering).paragraph,
            );
        });
        let written = found
            .iter()
            .flat_map(|paragraph| &paragraph.runs)
            .any(|run| !run.text.trim().is_empty());
        if !written {
            return None;
        }
        let inset = |name: &str, fallback: f64| {
            setting(&chain, name)
                .and_then(odf::length)
                .unwrap_or(fallback)
        };
        Some(Body {
            insets: [
                inset("fo:padding-left", DEFAULT_SIDE_INSET_POINTS),
                inset("fo:padding-top", DEFAULT_END_INSET_POINTS),
                inset("fo:padding-right", DEFAULT_SIDE_INSET_POINTS),
                inset("fo:padding-bottom", DEFAULT_END_INSET_POINTS),
            ],
            anchor: match setting(&chain, "draw:textarea-vertical-align") {
                Some("middle") => Anchor::Middle,
                Some("bottom") => Anchor::Bottom,
                Some("top") => Anchor::Top,
                _ => anchor,
            },
            wraps: setting(&chain, "fo:wrap-option") != Some("no-wrap"),
            scale: 1.0,
            spacing_scale: 1.0,
            paragraphs: found,
        })
    }

    fn picture(&mut self, path: &str) -> Option<usize> {
        if !self.archive.entry_names().any(|name| name == path) {
            return None;
        }
        let source = Source::Entry(path.to_owned());
        Some(
            match self.pictures.iter().position(|known| *known == source) {
                Some(known) => known,
                None => {
                    self.pictures.push(source);
                    self.pictures.len() - 1
                }
            },
        )
    }

    /// A frame: its picture, or its text box drawn in the frame's own fill and line.
    fn read_frame(&mut self, frame: &XmlElement, items: &mut Vec<Item>) {
        let Some(place) = frame_of(frame) else {
            return;
        };
        let image = frame
            .child("image")
            .and_then(|image| image.attribute("xlink:href"));
        if let Some(picture) = image.and_then(|path| self.picture(path)) {
            items.push(Item::Picture(Placed {
                frame: place,
                picture,
                crop: [0.0; 4],
            }));
            return;
        }
        let chain = chain_of(frame, &self.styles);
        let text = frame
            .child("text-box")
            .and_then(|text| self.read_body(text, frame, Anchor::Top));
        push_shape(items, place, Form::Box, &chain, text);
    }

    /// A drawn shape: a box, rounded box or oval by its geometry, with its text centred unless
    /// its style says otherwise.
    fn read_shape(&mut self, shape: &XmlElement, items: &mut Vec<Item>) {
        let Some(place) = frame_of(shape) else {
            return;
        };
        let text = self.read_body(shape, shape, Anchor::Middle);
        let geometry = shape
            .child("enhanced-geometry")
            .and_then(|geometry| geometry.attribute("draw:type"));
        let form = match (shape.name.as_str(), geometry) {
            ("rect", _) | (_, Some("rectangle" | "rect")) => Form::Box,
            ("ellipse" | "circle", _) | (_, Some("ellipse" | "circle")) => Form::Oval,
            (_, Some("round-rectangle")) => Form::Rounded(ROUNDING * place.width.min(place.height)),
            _ if text.is_some() => Form::Box,
            _ => return,
        };
        let chain = chain_of(shape, &self.styles);
        push_shape(items, place, form, &chain, text);
    }

    fn read_line(&mut self, line: &XmlElement, items: &mut Vec<Item>) {
        let ends = ["svg:x1", "svg:y1", "svg:x2", "svg:y2"].map(|name| length_of(line, name));
        let [Some(x1), Some(y1), Some(x2), Some(y2)] = ends else {
            return;
        };
        let place = Rect::new(x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs());
        let chain = chain_of(line, &self.styles);
        let stroke = read_stroke(&chain).or(Some(Stroke {
            colour: Colour::BLACK,
            width: DEFAULT_STROKE_POINTS,
        }));
        items.push(Item::Shape(Shape {
            frame: place,
            form: Form::Line {
                rising: (x2 - x1) * (y2 - y1) < 0.0,
            },
            fill: None,
            stroke,
            text: None,
        }));
    }

    /// Reads what a page or group holds, back to front. `slots` says whether placeholders are
    /// content (on a slide) or slots (on a master page).
    fn read_items(&mut self, holder: &XmlElement, slots: Slots, items: &mut Vec<Item>) {
        for child in &holder.children {
            let placeholder = child.attribute("presentation:class").is_some()
                && (slots == Slots::Skipped
                    || child.attribute("presentation:placeholder") == Some("true"));
            if placeholder {
                continue;
            }
            match child.name.as_str() {
                "frame" => self.read_frame(child, items),
                "custom-shape" | "rect" | "ellipse" | "circle" => self.read_shape(child, items),
                "line" | "connector" => self.read_line(child, items),
                "g" => self.read_items(child, slots, items),
                _ => {}
            }
        }
    }
}

/// Whether placeholders on a page are read as content.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slots {
    Read,
    Skipped,
}

fn push_shape(
    items: &mut Vec<Item>,
    place: Rect,
    form: Form,
    chain: &[&XmlElement],
    text: Option<Body>,
) {
    let (fill, stroke) = (read_fill(chain), read_stroke(chain));
    if fill.is_none() && stroke.is_none() && text.is_none() {
        return;
    }
    items.push(Item::Shape(Shape {
        frame: place,
        form,
        fill,
        stroke,
        text,
    }));
}

/// The colour a drawing-page style fills the page with.
fn page_fill(styles: &Styles, name: Option<&str>) -> Option<Colour> {
    let properties = styles
        .get("drawing-page", name?)?
        .element
        .child("drawing-page-properties")?;
    match properties.attribute("draw:fill") {
        Some("solid" | "gradient") | None => properties
            .attribute("draw:fill-color")
            .and_then(Colour::hex),
        Some(_) => None,
    }
}

/// The master pages by name, in `styles.xml`.
fn master<'a>(styles_part: Option<&'a XmlElement>, name: Option<&str>) -> Option<&'a XmlElement> {
    let masters: Vec<&XmlElement> = styles_part?
        .child("master-styles")?
        .children_named("master-page")
        .collect();
    masters
        .iter()
        .find(|master| master.attribute("style:name") == name)
        .or(masters.first())
        .copied()
}

fn page_size(styles_part: Option<&XmlElement>, master: Option<&XmlElement>) -> (f64, f64) {
    let layout_name = master.and_then(|master| master.attribute("style:page-layout-name"));
    let layout = styles_part.and_then(|part| {
        part.children
            .iter()
            .flat_map(|list| list.children_named("page-layout"))
            .find(|layout| layout.attribute("style:name") == layout_name)
    });
    layout
        .and_then(|layout| layout.child("page-layout-properties"))
        .and_then(|properties| {
            Some((
                length_of(properties, "fo:page-width")?,
                length_of(properties, "fo:page-height")?,
            ))
        })
        .filter(|(width, height)| *width > 0.0 && *height > 0.0)
        .unwrap_or(DEFAULT_SLIDE_POINTS)
}

/// Reads an OpenDocument presentation held in memory.
pub fn parse(bytes: &[u8]) -> Result<Deck> {
    let archive = ZipArchive::open(bytes)?;
    let content = read_part(&archive, CONTENT)?.ok_or(ReadError::NotItsKind)?;
    let presentation = content
        .child("body")
        .and_then(|body| body.child("presentation"))
        .ok_or(ReadError::NotItsKind)?;
    let styles_part = read_part(&archive, STYLES).ok().flatten();
    let mut styles = Styles::default();
    styles.add(&content);
    if let Some(part) = &styles_part {
        styles.add(part);
    }
    let pages: Vec<&XmlElement> = presentation.children_named("page").collect();
    let first_master = master(
        styles_part.as_ref(),
        pages
            .first()
            .and_then(|page| page.attribute("draw:master-page-name")),
    );
    let (width, height) = page_size(styles_part.as_ref(), first_master);
    let mut reader = Reader {
        archive: &archive,
        styles,
        pictures: Vec::new(),
    };
    let slides = pages
        .into_iter()
        .map(|page| {
            let master = master(
                styles_part.as_ref(),
                page.attribute("draw:master-page-name"),
            );
            let ground = page_fill(&reader.styles, page.attribute("draw:style-name"))
                .or_else(|| {
                    page_fill(
                        &reader.styles,
                        master.and_then(|master| master.attribute("draw:style-name")),
                    )
                })
                .unwrap_or(Colour::WHITE);
            let mut items = Vec::new();
            if let Some(master) = master {
                reader.read_items(master, Slots::Skipped, &mut items);
            }
            reader.read_items(page, Slots::Read, &mut items);
            Slide { ground, items }
        })
        .collect();
    Ok(Deck {
        width,
        height,
        fit: Fit::Whole,
        slides,
        pictures: reader.pictures,
    })
}

/// Reads the OpenDocument presentation at `path`.
pub fn read(path: &std::path::Path) -> Result<Deck> {
    parse(&read_bytes(path)?)
}
