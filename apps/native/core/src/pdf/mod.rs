//! Northroom's own PDF renderer (ADR-0015): reads a PDF and draws its pages, the same on every
//! system.
//!
//! A PDF opens as a deck of pages. Each page is planned when the file opens (`render::Planner`):
//! its drawn marks fall into layers, and each JPEG or JPEG 2000 picture sits between them, since
//! those pictures are decoded by the system (ADR-0004). A layer is drawn by the core's own
//! rasterizer into a picture when a view first shows it (`render_layer`), and kept in the app's
//! caches like any deck picture.
//!
//! Left out: text selection and search, links and forms' interaction, page zoom beyond the
//! page view's, and what `render`, `image`, `font` and `raster` say they leave out.

pub mod colour;
pub mod crypt;
pub mod encoding;
pub mod filter;
pub mod font;
pub mod image;
pub mod object;
pub mod raster;
pub mod render;
pub mod type1;

use crate::deck::{Colour, Deck, Fit, Item, Placed, Slide, Source};
use crate::geometry::Rect;
use object::{Dict, Document, Object};
use render::{Fonts, Interpreter, Matrix, Part};

/// Why a PDF can't be read; the words are for logs and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfError(pub String);

impl std::fmt::Display for PdfError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

type Result<T> = std::result::Result<T, PdfError>;

/// A page's layers are drawn this many pixels to a point, so they stay sharp at the page view's
/// largest size on a high-density display.
const PIXELS_PER_POINT: f64 = 3.0;
/// The longest side a layer is drawn at, in pixels.
const LAYER_SIDE_MAX: f64 = 4096.0;
/// A page's size when the file gives none: US Letter.
const LETTER: [f64; 4] = [0.0, 0.0, 612.0, 792.0];
/// How deep the page tree is followed.
const TREE_DEPTH_MAX: usize = 64;
/// Annotation flags that hide an annotation from view.
const HIDDEN: i64 = 2 | 32;

/// A page as drawing needs it: its box, its turn, its resources and content.
struct Page {
    crop: [f64; 4],
    rotate: i64,
    resources: Option<Dict>,
    contents: Object,
    annotations: Object,
}

impl Page {
    /// Its size in points as shown, turned.
    fn size(&self) -> (f64, f64) {
        let (width, height) = (
            (self.crop[2] - self.crop[0]).abs(),
            (self.crop[3] - self.crop[1]).abs(),
        );
        if self.rotate % 180 == 0 {
            (width, height)
        } else {
            (height, width)
        }
    }

    /// From the page's user space to pixels `scale` to a point, top-left origin, turned.
    fn matrix(&self, scale: f64) -> Matrix {
        let [x0, y0, x1, y1] = self.crop;
        let (left, bottom) = (x0.min(x1), y0.min(y1));
        let (width, height) = ((x1 - x0).abs(), (y1 - y0).abs());
        let turn: Matrix = match self.rotate {
            90 => [0.0, -1.0, 1.0, 0.0, 0.0, width],
            180 => [-1.0, 0.0, 0.0, -1.0, width, height],
            270 => [0.0, 1.0, -1.0, 0.0, height, 0.0],
            _ => render::IDENTITY,
        };
        let (_, shown_height) = self.size();
        let to_pixels = [scale, 0.0, 0.0, -scale, 0.0, shown_height * scale];
        render::multiply(
            render::multiply([1.0, 0.0, 0.0, 1.0, -left, -bottom], turn),
            to_pixels,
        )
    }
}

fn read_box(document: &Document, object: &Object) -> Option<[f64; 4]> {
    let values: Vec<f64> = document
        .resolve(object)
        .as_array()?
        .iter()
        .filter_map(|item| document.resolve(item).as_f64())
        .collect();
    let values: [f64; 4] = values.try_into().ok()?;
    ((values[2] - values[0]).abs() > 0.0 && (values[3] - values[1]).abs() > 0.0).then_some(values)
}

/// What a page inherits from the nodes above it.
#[derive(Clone, Default)]
struct Inherited {
    media: Option<[f64; 4]>,
    crop: Option<[f64; 4]>,
    rotate: Option<i64>,
    resources: Option<Dict>,
}

fn collect_pages(
    document: &Document,
    node: &Object,
    inherited: Inherited,
    depth: usize,
    pages: &mut Vec<Page>,
) {
    if depth > TREE_DEPTH_MAX {
        return;
    }
    let node = document.resolve(node);
    let Some(dict) = node.as_dict() else {
        return;
    };
    let inherited = Inherited {
        media: dict
            .get("MediaBox")
            .and_then(|object| read_box(document, object))
            .or(inherited.media),
        crop: dict
            .get("CropBox")
            .and_then(|object| read_box(document, object))
            .or(inherited.crop),
        rotate: document.entry(dict, "Rotate").as_i64().or(inherited.rotate),
        resources: document
            .entry(dict, "Resources")
            .as_dict()
            .cloned()
            .or(inherited.resources),
    };
    match document.entry(dict, "Kids") {
        Object::Array(kids) => {
            for kid in kids {
                collect_pages(document, &kid, inherited.clone(), depth + 1, pages);
            }
        }
        _ => {
            let media = inherited.media.unwrap_or(LETTER);
            pages.push(Page {
                crop: inherited.crop.map_or(media, |crop| intersect(crop, media)),
                rotate: inherited.rotate.unwrap_or(0).rem_euclid(360) / 90 * 90,
                resources: inherited.resources,
                contents: dict.get("Contents").cloned().unwrap_or(Object::Null),
                annotations: document.entry(dict, "Annots"),
            });
        }
    }
}

fn intersect(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let (ax0, ay0, ax1, ay1) = (
        a[0].min(a[2]),
        a[1].min(a[3]),
        a[0].max(a[2]),
        a[1].max(a[3]),
    );
    let (bx0, by0, bx1, by1) = (
        b[0].min(b[2]),
        b[1].min(b[3]),
        b[0].max(b[2]),
        b[1].max(b[3]),
    );
    let joined = [ax0.max(bx0), ay0.max(by0), ax1.min(bx1), ay1.min(by1)];
    if joined[2] > joined[0] && joined[3] > joined[1] {
        joined
    } else {
        b
    }
}

fn pages_of(document: &Document) -> Vec<Page> {
    let catalog = document.entry(&document.trailer, "Root");
    let mut pages = Vec::new();
    if let Some(catalog) = catalog.as_dict() {
        collect_pages(
            document,
            catalog.get("Pages").unwrap_or(&Object::Null),
            Inherited::default(),
            0,
            &mut pages,
        );
    }
    pages
}

/// A page's content: its streams joined.
fn content_of(document: &Document, contents: &Object) -> Vec<u8> {
    let streams: Vec<Object> = match document.resolve(contents) {
        Object::Array(items) => items.iter().map(|item| document.resolve(item)).collect(),
        other => vec![other],
    };
    let mut content = Vec::new();
    for stream in streams {
        if let Object::Stream(stream) = stream {
            content.extend(document.stream_data(&stream).unwrap_or_default());
            content.push(b'\n');
        }
    }
    content
}

/// Runs a page's content and visible annotations' appearances on `sink`.
fn run_page<S: render::Sink>(document: &Document, page: &Page, sink: &mut S, scale: f64) {
    let mut fonts = Fonts::default();
    let mut interpreter = Interpreter::new(document, sink, &mut fonts, page.matrix(scale));
    interpreter.run_page(
        &content_of(document, &page.contents),
        page.resources.clone(),
    );
    for annotation in page.annotations.as_array().unwrap_or(&[]) {
        let annotation = document.resolve(annotation);
        let Some(dict) = annotation.as_dict() else {
            continue;
        };
        if document.entry(dict, "F").as_i64().unwrap_or(0) & HIDDEN != 0 {
            continue;
        }
        let Some(rectangle) = dict.get("Rect").and_then(|rect| read_box(document, rect)) else {
            continue;
        };
        let appearance = appearance_of(document, dict);
        if let Some(appearance) = appearance {
            let ordered = [
                rectangle[0].min(rectangle[2]),
                rectangle[1].min(rectangle[3]),
                rectangle[0].max(rectangle[2]),
                rectangle[1].max(rectangle[3]),
            ];
            interpreter.run_appearance(&appearance, ordered);
        }
    }
}

/// An annotation's normal appearance: its stream, or its state's when it has several.
fn appearance_of(document: &Document, annotation: &Dict) -> Option<object::Stream> {
    let appearances = document.entry(annotation, "AP");
    let normal = appearances
        .as_dict()
        .map(|appearances| document.entry(appearances, "N"))?;
    match normal {
        Object::Stream(stream) => Some(stream),
        Object::Dict(states) => {
            let state = document.entry(annotation, "AS");
            match document.entry(&states, state.as_name()?) {
                Object::Stream(stream) => Some(stream),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The pixels to a point a page's layers are drawn at.
fn scale_of(page: &Page) -> f64 {
    let (width, height) = page.size();
    PIXELS_PER_POINT.min(LAYER_SIDE_MAX / width.max(height).max(1.0))
}

/// The slide a page is: its parts as pictures over a white ground.
fn slide_of(document: &Document, page: &Page, number: usize, pictures: &mut Vec<Source>) -> Slide {
    let mut planner = render::Planner::default();
    run_page(document, page, &mut planner, 1.0);
    let (width, height) = page.size();
    let (mut layers, mut encoded) = (0, 0);
    let mut items = Vec::new();
    for part in planner.finish() {
        let (source, frame) = match part {
            Part::Layer => {
                layers += 1;
                (
                    Source::PdfLayer {
                        page: number,
                        layer: layers - 1,
                    },
                    [0.0, 0.0, width, height],
                )
            }
            Part::Encoded { extension, frame } => {
                encoded += 1;
                (
                    Source::PdfPicture {
                        page: number,
                        picture: encoded - 1,
                        extension,
                    },
                    frame,
                )
            }
        };
        pictures.push(source);
        items.push(Item::Picture(Placed {
            frame: Rect::new(frame[0], frame[1], frame[2], frame[3]),
            picture: pictures.len() - 1,
            crop: [0.0; 4],
        }));
    }
    Slide {
        ground: Colour::WHITE,
        items,
    }
}

/// Reads a PDF held in memory as a deck of its pages.
pub fn parse(bytes: Vec<u8>) -> Result<Deck> {
    let document = Document::open(bytes)?;
    let pages = pages_of(&document);
    if pages.is_empty() {
        return Err(PdfError("The PDF has no pages.".into()));
    }
    let mut pictures = Vec::new();
    let slides: Vec<Slide> = pages
        .iter()
        .enumerate()
        .map(|(number, page)| slide_of(&document, page, number, &mut pictures))
        .collect();
    // Pages differ in size; the deck is as wide as the widest, each page drawn at its own size.
    let (width, height) = pages
        .iter()
        .map(Page::size)
        .fold((0.0f64, 0.0f64), |(w, h), (pw, ph)| (w.max(pw), h.max(ph)));
    Ok(Deck {
        width,
        height,
        fit: Fit::Width,
        slides,
        pictures,
    })
}

/// Reads the PDF at `path` as a deck of its pages.
pub fn read(path: &std::path::Path) -> Result<Deck> {
    let bytes = std::fs::read(path)
        .map_err(|error| PdfError(format!("The PDF could not be read ({error})")))?;
    parse(bytes)
}

/// Draws layer `layer` of page `number` as a PNG.
pub fn render_layer(bytes: Vec<u8>, number: usize, layer: usize) -> Result<Vec<u8>> {
    let document = Document::open(bytes)?;
    let pages = pages_of(&document);
    let page = pages
        .get(number)
        .ok_or_else(|| PdfError("The PDF has no such page.".into()))?;
    let scale = scale_of(page);
    let (width, height) = page.size();
    let mut renderer = render::Renderer::new(
        (width * scale).ceil() as usize,
        (height * scale).ceil() as usize,
        layer,
    );
    run_page(&document, page, &mut renderer, scale);
    Ok(renderer.canvas.png())
}

/// The bytes of encoded picture `picture` on page `number`.
pub fn extract_picture(bytes: Vec<u8>, number: usize, picture: usize) -> Result<Vec<u8>> {
    let document = Document::open(bytes)?;
    let pages = pages_of(&document);
    let page = pages
        .get(number)
        .ok_or_else(|| PdfError("The PDF has no such page.".into()))?;
    let mut extractor = render::Extractor::new(picture);
    run_page(&document, page, &mut extractor, 1.0);
    extractor
        .found
        .ok_or_else(|| PdfError("The page has no such picture.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PDF of `objects` (numbered from 1, the first the catalog) with a correct cross-reference
    /// table.
    fn pdf_of(objects: &[String]) -> Vec<u8> {
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend(format!("{} 0 obj\n{object}\nendobj\n", index + 1).into_bytes());
        }
        let table = bytes.len();
        bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).into_bytes());
        for offset in offsets {
            bytes.extend(format!("{offset:010} 00000 n \n").into_bytes());
        }
        bytes.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n",
                objects.len() + 1
            )
            .into_bytes(),
        );
        bytes
    }

    fn stream(content: &str) -> String {
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
    }

    /// Two pages: a red square and a line of text, then a page turned a quarter.
    fn sample() -> Vec<u8> {
        pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /MediaBox [0 0 200 100] \
             /Resources << /Font << /F1 7 0 R >> >> >>"
                .into(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".into(),
            stream("1 0 0 rg 10 10 50 50 re f BT /F1 24 Tf 0 0 1 rg 80 40 Td (Hi) Tj ET"),
            "<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents 6 0 R >>".into(),
            stream("0 g 0 0 20 20 re f"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ])
    }

    fn pixel(canvas: &raster::Canvas, x: usize, y: usize) -> [u8; 4] {
        canvas.pixel(x, y)
    }

    #[test]
    fn pages_read_with_their_inherited_size_and_turn() {
        let deck = parse(sample()).expect("a deck");
        assert_eq!(deck.slides.len(), 2);
        assert_eq!((deck.width, deck.height), (200.0, 200.0));
        assert_eq!(deck.fit, Fit::Width);
        let pages = pages_of(&Document::open(sample()).expect("open"));
        assert_eq!(pages[0].size(), (200.0, 100.0));
        assert_eq!(pages[1].size(), (100.0, 200.0));
        // Each page is one layer of drawn marks.
        for slide in &deck.slides {
            assert_eq!(slide.items.len(), 1);
        }
        assert_eq!(deck.pictures[1], Source::PdfLayer { page: 1, layer: 0 });
    }

    #[test]
    fn a_page_draws_its_shapes_and_text_where_they_stand() {
        let document = Document::open(sample()).expect("open");
        let pages = pages_of(&document);
        let mut renderer = render::Renderer::new(200, 100, 0);
        run_page(&document, &pages[0], &mut renderer, 1.0);
        // The square's middle, 35 points up from the bottom, is red; outside it is clear.
        assert_eq!(pixel(&renderer.canvas, 35, 64), [255, 0, 0, 255]);
        assert_eq!(pixel(&renderer.canvas, 150, 90)[3], 0);
        // "Hi" in the fallback face sets blue pixels right of 80 and above the baseline.
        let blue = (80..120)
            .flat_map(|x| (40..60).map(move |y| (x, y)))
            .filter(|&(x, y)| pixel(&renderer.canvas, x, y) == [0, 0, 255, 255])
            .count();
        assert!(blue > 20, "{blue} blue pixels");
    }

    #[test]
    fn a_turned_page_draws_its_corner_turned() {
        let document = Document::open(sample()).expect("open");
        let pages = pages_of(&document);
        let mut renderer = render::Renderer::new(100, 200, 0);
        run_page(&document, &pages[1], &mut renderer, 1.0);
        // Turned a quarter clockwise, the bottom-left corner shows at the top left.
        assert_eq!(pixel(&renderer.canvas, 5, 5), [0, 0, 0, 255]);
        assert_eq!(pixel(&renderer.canvas, 5, 195)[3], 0);
    }

    #[test]
    fn damaged_bytes_are_an_error_not_a_panic() {
        assert!(parse(b"%PDF-1.4\nnot really".to_vec()).is_err());
        assert!(parse(Vec::new()).is_err());
        let mut cut = sample();
        cut.truncate(cut.len() / 2);
        let _ = parse(cut);
    }
}
