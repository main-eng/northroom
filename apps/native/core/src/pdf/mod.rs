//! Northroom's own PDF renderer (ADR-0015): reads a PDF and draws its pages, the same on every
//! system.
//!
//! A PDF opens as a deck of pages. Each page is planned when the file opens (`render::Planner`):
//! its drawn marks fall into layers, and each plain JPEG or JPEG 2000 picture sits between them,
//! decoded by the system (ADR-0004) and cut to a rectangular clip. A JPEG that is see-through,
//! masked, turned or clipped to a shape is drawn into its layer instead. A layer is drawn by
//! the core's own rasterizer into a picture when a view first shows it (`render_layer`), and
//! kept in the app's caches like any deck picture.
//!
//! Left out: text selection and search, links and forms' interaction, page zoom beyond the
//! page view's, and what `render`, `image`, `font` and `raster` say they leave out.

pub mod blend;
pub mod ccitt;
pub mod cff;
pub mod cmap;
pub mod colour;
pub mod crypt;
pub mod encoding;
pub mod filter;
pub mod font;
pub mod image;
pub mod layers;
pub mod object;
pub mod passwords;
pub mod raster;
pub mod render;
pub mod shading;
pub mod stroke;
pub mod type1;

use crate::deck::{Colour, Deck, Fit, Item, Placed, Slide, Source};
use crate::geometry::Rect;
use object::{Dict, Document, Object};
use render::{Fonts, Interpreter, Matrix, Part};

/// Why a PDF can't be read; the words are for logs and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfError {
    /// The file is damaged, or uses something this reader lacks; the words say what.
    Damaged(String),
    /// The file is encrypted, and neither an empty password nor one given opens it.
    Locked,
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::Damaged(why) => formatter.write_str(why),
            PdfError::Locked => formatter.write_str("The PDF needs a password to open."),
        }
    }
}

impl From<PdfError> for crate::deck::ReadError {
    fn from(error: PdfError) -> Self {
        match error {
            PdfError::Damaged(why) => crate::deck::ReadError::Damaged(why),
            PdfError::Locked => crate::deck::ReadError::Locked,
        }
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

/// What a page inherits once `dict`, a node of the page tree, has added its own.
fn inherited_from(document: &Document, dict: &Dict, inherited: Inherited) -> Inherited {
    Inherited {
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
    }
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
    let inherited = inherited_from(document, dict, inherited);
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
        let in_hidden_layer = dict.get("OC").is_some_and(|layer| interpreter.hides(layer));
        if document.entry(dict, "F").as_i64().unwrap_or(0) & HIDDEN != 0 || in_hidden_layer {
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
        let (source, frame, crop) = match part {
            Part::Layer => {
                layers += 1;
                (
                    Source::PdfLayer {
                        page: number,
                        layer: layers - 1,
                    },
                    [0.0, 0.0, width, height],
                    [0.0; 4],
                )
            }
            Part::Encoded {
                extension,
                frame,
                crop,
            } => {
                encoded += 1;
                (
                    Source::PdfPicture {
                        page: number,
                        picture: encoded - 1,
                        extension,
                    },
                    frame,
                    crop,
                )
            }
        };
        pictures.push(source);
        items.push(Item::Picture(Placed {
            frame: Rect::new(frame[0], frame[1], frame[2], frame[3]),
            picture: pictures.len() - 1,
            crop,
        }));
    }
    Slide {
        ground: Colour::WHITE,
        items,
    }
}

/// The pages of a document that lists none in its page tree: every page object in it, each
/// inheriting from the parents it names, as pdf.js recovers them.
fn loose_pages(document: &Document) -> Vec<Page> {
    let mut pages = Vec::new();
    for number in document.numbers() {
        let object = document.get(number);
        let Some(dict) = object.as_dict() else {
            continue;
        };
        if document.entry(dict, "Type").as_name() != Some("Page") {
            continue;
        }
        let mut ancestors = Vec::new();
        let mut parent = document.entry(dict, "Parent");
        while let (Some(node), true) = (parent.as_dict().cloned(), ancestors.len() < TREE_DEPTH_MAX)
        {
            parent = document.entry(&node, "Parent");
            ancestors.push(node);
        }
        let inherited = ancestors
            .iter()
            .rev()
            .fold(Inherited::default(), |inherited, node| {
                inherited_from(document, node, inherited)
            });
        collect_pages(
            document,
            &Object::Ref(object::Ref(number, 0)),
            inherited,
            0,
            &mut pages,
        );
    }
    pages
}

/// Opens a document and finds its pages: through its page tree; else after finding its
/// objects again by scanning, as damaged cross-references hide them; else every page object.
fn open_pages(bytes: Vec<u8>, password: Option<&[u8]>) -> Result<(Document, Vec<Page>)> {
    let mut document = Document::open_with(bytes, password)?;
    let mut pages = pages_of(&document);
    if pages.is_empty() {
        document.rescan();
        pages = pages_of(&document);
    }
    if pages.is_empty() {
        pages = loose_pages(&document);
    }
    if pages.is_empty() {
        return Err(PdfError::Damaged("The PDF has no pages.".into()));
    }
    Ok((document, pages))
}

/// Reads a PDF held in memory as a deck of its pages, opening it with `password` if it needs
/// one.
pub fn parse(bytes: Vec<u8>, password: Option<&[u8]>) -> Result<Deck> {
    let (document, pages) = open_pages(bytes, password)?;
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
        .map_err(|error| PdfError::Damaged(format!("The PDF could not be read ({error})")))?;
    parse(bytes, passwords::of(path).as_deref())
}

/// Draws layer `layer` of page `number` as a PNG.
pub fn render_layer(
    bytes: Vec<u8>,
    password: Option<&[u8]>,
    number: usize,
    layer: usize,
) -> Result<Vec<u8>> {
    let (document, pages) = open_pages(bytes, password)?;
    let page = pages
        .get(number)
        .ok_or_else(|| PdfError::Damaged("The PDF has no such page.".into()))?;
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
pub fn extract_picture(
    bytes: Vec<u8>,
    password: Option<&[u8]>,
    number: usize,
    picture: usize,
) -> Result<Vec<u8>> {
    let (document, pages) = open_pages(bytes, password)?;
    let page = pages
        .get(number)
        .ok_or_else(|| PdfError::Damaged("The PDF has no such page.".into()))?;
    let mut extractor = render::Extractor::new(picture);
    run_page(&document, page, &mut extractor, 1.0);
    extractor
        .found
        .ok_or_else(|| PdfError::Damaged("The page has no such picture.".into()))
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
        let deck = parse(sample(), None).expect("a deck");
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

    /// A 16-pixel blue JPEG as an image stream, written in hex so the PDF stays text, with
    /// `extra` entries.
    fn blue_jpeg(extra: &str) -> String {
        let hex: String = include_bytes!("../../tests/fixtures/pdf/blue.jpg")
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>()
            + ">";
        format!(
            "<< /Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceRGB \
             /BitsPerComponent 8 /Filter [/ASCIIHexDecode /DCTDecode] {extra} /Length {} >>\n\
             stream\n{hex}\nendstream",
            hex.len()
        )
    }

    /// Four pages of the same JPEG: plain and cut by a rectangle, half see-through, clipped to
    /// a triangle, and under a soft mask clear on its left half.
    fn pictures() -> Vec<u8> {
        pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 /MediaBox [0 0 200 100] \
             /Resources << /XObject << /Im0 11 0 R /Im1 12 0 R >> /ExtGState << /Half 14 0 R >> >> >>"
                .into(),
            "<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents 8 0 R >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents 9 0 R >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents 10 0 R >>".into(),
            stream("q 0 0 100 100 re W n 200 0 0 100 0 0 cm /Im0 Do Q"),
            stream("q /Half gs 100 0 0 100 0 0 cm /Im0 Do Q"),
            stream("q 0 0 m 100 0 l 0 100 l h W n 100 0 0 100 0 0 cm /Im0 Do Q"),
            stream("q 100 0 0 100 0 0 cm /Im1 Do Q"),
            blue_jpeg(""),
            blue_jpeg("/SMask 13 0 R"),
            "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length 5 >>\nstream\n00FF>\nendstream"
                .into(),
            "<< /Type /ExtGState /ca 0.5 >>".into(),
        ])
    }

    fn drawn(page: usize) -> raster::Canvas {
        let document = Document::open(pictures()).expect("open");
        let pages = pages_of(&document);
        let mut renderer = render::Renderer::new(200, 100, 0);
        run_page(&document, &pages[page], &mut renderer, 1.0);
        renderer.canvas
    }

    fn is_blue(pixel: [u8; 4]) -> bool {
        pixel[0] < 16 && pixel[1] < 16 && pixel[2] > 235 && pixel[3] == 255
    }

    #[test]
    fn a_plain_jpeg_is_left_to_the_system_cut_by_its_rectangular_clip() {
        let deck = parse(pictures(), None).expect("a deck");
        let [Item::Picture(placed)] = deck.slides[0].items.as_slice() else {
            panic!("one picture: {:?}", deck.slides[0].items);
        };
        assert_eq!(
            deck.pictures[placed.picture],
            Source::PdfPicture {
                page: 0,
                picture: 0,
                extension: "jpeg"
            }
        );
        assert_eq!(placed.frame, Rect::new(0.0, 0.0, 100.0, 100.0));
        assert_eq!(placed.crop, [0.0, 0.0, 0.5, 0.0]);
    }

    #[test]
    fn a_see_through_masked_or_shaped_jpeg_is_drawn_into_its_page() {
        let deck = parse(pictures(), None).expect("a deck");
        for slide in &deck.slides[1..] {
            let [Item::Picture(placed)] = slide.items.as_slice() else {
                panic!("one layer: {:?}", slide.items);
            };
            assert!(matches!(
                deck.pictures[placed.picture],
                Source::PdfLayer { .. }
            ));
        }
        // Half see-through: premultiplied blue at half alpha.
        let half = drawn(1).pixel(50, 50);
        assert!(
            (120..=136).contains(&half[3]) && half[2].abs_diff(half[3]) < 8,
            "{half:?}"
        );
        // Clipped to the triangle below the diagonal from top left to bottom right.
        let triangle = drawn(2);
        assert!(is_blue(triangle.pixel(10, 90)));
        assert_eq!(triangle.pixel(90, 10)[3], 0);
        // The soft mask hides the left half.
        let masked = drawn(3);
        assert_eq!(masked.pixel(25, 50)[3], 0);
        assert!(is_blue(masked.pixel(75, 50)));
    }

    #[test]
    fn a_dashed_line_draws_its_dashes_and_skips_its_gaps() {
        let bytes = pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 40 20] >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".into(),
            stream("[4 4] 0 d 2 w 1 J 0 10 m 40 10 l S"),
        ]);
        let document = Document::open(bytes).expect("open");
        let pages = pages_of(&document);
        let mut renderer = render::Renderer::new(40, 20, 0);
        run_page(&document, &pages[0], &mut renderer, 1.0);
        let canvas = renderer.canvas;
        // Dashes at 0–4, 8–12…; round caps reach a unit past each end.
        assert_eq!(canvas.pixel(2, 10)[3], 255);
        assert_eq!(canvas.pixel(10, 10)[3], 255);
        assert_eq!(canvas.pixel(6, 10)[3], 0);
        assert_eq!(canvas.pixel(14, 10)[3], 0);
    }

    #[test]
    fn a_hidden_layer_draws_nothing_and_a_form_stays_in_its_box() {
        let form = "0 0 40 20 re f";
        let bytes = pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] \
             /D << /OFF [5 0 R] >> >> >>"
                .into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 40 20] >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << \
             /Properties << /Off 5 0 R /On 6 0 R >> /XObject << /Box 7 0 R >> >> >>"
                .into(),
            stream(
                "/OC /Off BDC 0 0 10 10 re f EMC /OC /On BDC 10 0 10 10 re f EMC \
                 q 1 0 0 1 20 0 cm /Box Do Q",
            ),
            "<< /Type /OCG /Name (Off) >>".into(),
            "<< /Type /OCG /Name (On) >>".into(),
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length {} >>\n\
                 stream\n{form}\nendstream",
                form.len()
            ),
        ]);
        let document = Document::open(bytes).expect("open");
        let pages = pages_of(&document);
        let mut renderer = render::Renderer::new(40, 20, 0);
        run_page(&document, &pages[0], &mut renderer, 1.0);
        let canvas = renderer.canvas;
        // Bottom row of the page is pixel row 19.
        assert_eq!(canvas.pixel(5, 15)[3], 0, "the hidden layer");
        assert_eq!(canvas.pixel(15, 15)[3], 255, "the shown layer");
        assert_eq!(canvas.pixel(25, 15)[3], 255, "the form in its box");
        assert_eq!(canvas.pixel(35, 15)[3], 0, "the form past its box");
    }

    #[test]
    fn a_soft_mask_and_a_blend_mode_apply_to_what_paints_after_them() {
        let mask = "1 g 0 0 20 20 re f";
        let bytes = pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 40 40] >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /ExtGState << \
             /Masked << /SMask << /S /Luminosity /G 5 0 R >> >> /Multiply << /BM /Multiply >> >> >> >>"
                .into(),
            // Top half: red through a mask white only on its left. Bottom half: red, then half
            // gray multiplied over it.
            stream(
                "q 1 0 0 1 0 20 cm /Masked gs 1 0 0 rg 0 0 40 20 re f Q \
                 1 0 0 rg 0 0 40 20 re f /Multiply gs 0.5 g 0 0 40 20 re f",
            ),
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 40 20] \
                 /Group << /S /Transparency /CS /DeviceGray >> /Length {} >>\n\
                 stream\n{mask}\nendstream",
                mask.len()
            ),
        ]);
        let document = Document::open(bytes).expect("open");
        let pages = pages_of(&document);
        let mut renderer = render::Renderer::new(40, 40, 0);
        run_page(&document, &pages[0], &mut renderer, 1.0);
        let canvas = renderer.canvas;
        assert_eq!(
            canvas.pixel(10, 10),
            [255, 0, 0, 255],
            "red where the mask is white"
        );
        assert_eq!(canvas.pixel(30, 10)[3], 0, "nothing where it is black");
        let multiplied = canvas.pixel(20, 30);
        assert!(
            (126..=129).contains(&multiplied[0]) && multiplied[1] == 0,
            "{multiplied:?}"
        );
    }

    #[test]
    fn a_tiling_pattern_repeats_its_cell_across_what_it_fills() {
        let cell = "0 0 5 10 re f";
        let bytes = pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 40 10] >>".into(),
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Pattern << /Bars 5 0 R >> >> >>"
                .into(),
            stream("/Pattern cs /Bars scn 0 0 40 10 re f"),
            format!(
                "<< /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] \
                 /XStep 10 /YStep 10 /Resources << >> /Length {} >>\nstream\n{cell}\nendstream",
                cell.len()
            ),
        ]);
        let document = Document::open(bytes).expect("open");
        let pages = pages_of(&document);
        let mut renderer = render::Renderer::new(40, 10, 0);
        run_page(&document, &pages[0], &mut renderer, 1.0);
        let canvas = renderer.canvas;
        for (x, filled) in [(2, true), (7, false), (12, true), (17, false), (32, true)] {
            assert_eq!(canvas.pixel(x, 5)[3] == 255, filled, "column {x}");
        }
    }

    #[test]
    fn an_encrypted_pdf_opens_with_its_user_or_owner_password_and_no_other() {
        let files: [(&str, &[u8]); 4] = [
            (
                "RC4",
                include_bytes!("../../tests/fixtures/pdf/locked-rc4-user.pdf"),
            ),
            (
                "AES-128",
                include_bytes!("../../tests/fixtures/pdf/locked-r4-user.pdf"),
            ),
            (
                "AES-256",
                include_bytes!("../../tests/fixtures/pdf/locked-r6-user.pdf"),
            ),
            (
                "AES-256, no user password",
                include_bytes!("../../tests/fixtures/pdf/locked-r6-empty.pdf"),
            ),
        ];
        for (name, bytes) in files {
            let opens = |password: Option<&[u8]>| {
                parse(bytes.to_vec(), password).map(|deck| deck.slides.len())
            };
            assert_eq!(opens(Some(b"secret")), Ok(1), "{name}, the user's password");
            assert_eq!(opens(Some(b"owner")), Ok(1), "{name}, the owner's password");
            if name.ends_with("no user password") {
                assert_eq!(opens(None), Ok(1), "{name}, no password");
            } else {
                assert_eq!(opens(None), Err(PdfError::Locked), "{name}, no password");
                assert_eq!(
                    opens(Some(b"wrong")),
                    Err(PdfError::Locked),
                    "{name}, a wrong one"
                );
            }
        }
    }

    #[test]
    fn damaged_bytes_are_an_error_not_a_panic() {
        assert!(parse(b"%PDF-1.4\nnot really".to_vec(), None).is_err());
        assert!(parse(Vec::new(), None).is_err());
        let mut cut = sample();
        cut.truncate(cut.len() / 2);
        let _ = parse(cut, None);
    }
}
