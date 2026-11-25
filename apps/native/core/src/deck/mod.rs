//! Files drawn as pages (ADR-0015): a deck is fixed-size pages of shapes, text and pictures in
//! points from each page's top-left. A presentation's slides are a deck as they are; a flowing
//! document (Word, OpenDocument text, rich text) is laid out into pages first (`flow`). Each
//! format's reader makes the same types, and `layout` places them for a view.
//!
//! A deck reads right, not exactly: text is set in the app's own face whatever face the file
//! names, shapes other than boxes, ovals and straight lines draw as boxes when they hold text
//! and are left out when they hold none, and rotation, shadows, gradients (drawn as their
//! first colour), charts, animations and transitions are not drawn.

pub mod doc;
pub mod docx;
pub mod ffi;
pub mod flow;
pub mod font;
pub mod iwork;
pub mod layout;
mod odf;
pub mod odp;
pub mod odt;
pub mod picture;
pub mod ppt;
pub mod pptx;
pub mod rtf;

use crate::geometry::Rect;
use crate::package::PackageError;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Why a file is not drawn. The view only says it could not be read; the words are for logs
/// and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The file, its package or one of its parts is damaged, missing or unreadable.
    Damaged(String),
    /// The file is readable, but not the kind its name says.
    NotItsKind,
    /// A part names another that is not there: a slide with no layout, a layout with no master.
    Missing(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Damaged(why) => formatter.write_str(why),
            ReadError::NotItsKind => formatter.write_str("The file is not the kind its name says."),
            ReadError::Missing(part) => {
                write!(formatter, "\"{part}\" names a part that is missing.")
            }
        }
    }
}

impl From<PackageError> for ReadError {
    fn from(error: PackageError) -> Self {
        ReadError::Damaged(error.0)
    }
}

pub type Result<T> = std::result::Result<T, ReadError>;

/// Reads a whole file, saying why when it cannot.
pub fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path)
        .map_err(|error| ReadError::Damaged(format!("The file could not be read ({error})")))
}

/// The kinds of file drawn as pages, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// PowerPoint: `.pptx` and its macro, show and template forms.
    Presentation,
    /// PowerPoint 97–2003: `.ppt` and its show and template forms.
    LegacyPresentation,
    /// Word: `.docx` and its macro and template forms.
    WordDocument,
    /// Word 97–2003: `.doc` and its template form.
    LegacyWordDocument,
    /// OpenDocument text: `.odt` and its template form.
    OpenDocumentText,
    /// OpenDocument presentation: `.odp` and its template form.
    OpenDocumentPresentation,
    /// Rich text: `.rtf`, and `.rtfd`, a folder holding it with its pictures.
    RichText,
    /// Photoshop: `.psd` and its large-document form.
    Photoshop,
    /// A camera's raw photo.
    RawPhoto,
    /// Keynote, Pages or Numbers, shown as the preview it carries.
    AppleDocument,
    /// A font file, shown as a specimen of each face it holds.
    Font,
}

/// The largest document drawn; a larger one shows its details. A file is read whole when it
/// opens, on the thread that draws.
const DOCUMENT_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The largest picture drawn: raw photos and layered Photoshop files are large.
const PICTURE_MAX_BYTES: u64 = 512 * 1024 * 1024;

impl Kind {
    /// Whether the file at `path`, whose name says this kind, is laid out as one. Only names
    /// that other files share are checked: a `.key` may be a private key, not a Keynote.
    pub fn claims(self, path: &Path) -> bool {
        match self {
            Kind::AppleDocument => iwork::is_document(path),
            Kind::Presentation
            | Kind::LegacyPresentation
            | Kind::WordDocument
            | Kind::LegacyWordDocument
            | Kind::OpenDocumentText
            | Kind::OpenDocumentPresentation
            | Kind::RichText
            | Kind::Photoshop
            | Kind::RawPhoto
            | Kind::Font => true,
        }
    }

    /// The largest file of this kind drawn.
    pub fn max_bytes(self) -> u64 {
        match self {
            Kind::Photoshop | Kind::RawPhoto | Kind::AppleDocument => PICTURE_MAX_BYTES,
            Kind::Presentation
            | Kind::LegacyPresentation
            | Kind::WordDocument
            | Kind::LegacyWordDocument
            | Kind::OpenDocumentText
            | Kind::OpenDocumentPresentation
            | Kind::RichText
            | Kind::Font => DOCUMENT_MAX_BYTES,
        }
    }

    /// The kind a file name says, by its extension in any case.
    pub fn of_name(name: &str) -> Option<Self> {
        let extension = Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())?
            .to_ascii_lowercase();
        Some(match extension.as_str() {
            "pptx" | "pptm" | "ppsx" | "ppsm" | "potx" | "potm" => Kind::Presentation,
            "ppt" | "pps" | "pot" => Kind::LegacyPresentation,
            "docx" | "docm" | "dotx" | "dotm" => Kind::WordDocument,
            "doc" | "dot" => Kind::LegacyWordDocument,
            "odt" | "ott" => Kind::OpenDocumentText,
            "odp" | "otp" => Kind::OpenDocumentPresentation,
            "rtf" | "rtfd" => Kind::RichText,
            "psd" | "psb" => Kind::Photoshop,
            "key" | "pages" | "numbers" => Kind::AppleDocument,
            "ttf" | "otf" | "ttc" | "otc" | "dfont" => Kind::Font,
            "dng" | "cr2" | "cr3" | "nef" | "nrw" | "arw" | "raf" | "orf" | "rw2" | "pef"
            | "srw" => Kind::RawPhoto,
            _ => return None,
        })
    }
}

/// Reads the file at `path` as `kind`. `measure` sets text where a document is laid out into
/// pages.
pub fn read(path: &Path, kind: Kind, measure: &mut crate::dialog::layout::Measure) -> Result<Deck> {
    match kind {
        Kind::Presentation => pptx::read(path),
        Kind::LegacyPresentation => ppt::read(path),
        Kind::WordDocument => Ok(flow::paginate(docx::read(path)?, measure)),
        Kind::LegacyWordDocument => Ok(flow::paginate(doc::read(path)?, measure)),
        Kind::OpenDocumentText => Ok(flow::paginate(odt::read(path)?, measure)),
        Kind::OpenDocumentPresentation => odp::read(path),
        Kind::RichText => Ok(flow::paginate(rtf::read(path)?, measure)),
        Kind::Photoshop => picture::read_photoshop_file(path),
        Kind::RawPhoto => picture::read_raw_file(path),
        Kind::AppleDocument => iwork::read(path),
        Kind::Font => font::read(path),
    }
}

/// The extension kept for a picture whose name has none; the system decoder reads its bytes.
const UNKNOWN_EXTENSION: &str = "bin";

/// Where a picture's bytes are.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// An entry of the zip the file is, by its name, which ends in its format's extension.
    Entry(String),
    /// A file of its own, such as a picture in a rich-text package.
    File(PathBuf),
    /// Bytes the reader took out of the file, in the format `extension` names.
    Bytes {
        bytes: Arc<Vec<u8>>,
        extension: &'static str,
    },
}

impl Source {
    /// The extension that says how the picture is decoded.
    pub fn extension(&self) -> String {
        let named = |name: &Path| {
            name.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or(UNKNOWN_EXTENSION)
                .to_ascii_lowercase()
        };
        match self {
            Source::Entry(name) => named(Path::new(name)),
            Source::File(path) => named(path),
            Source::Bytes { extension, .. } => (*extension).to_owned(),
        }
    }

    /// The picture's bytes, `file` being the file the deck was read from.
    pub fn read(&self, file: &Path) -> Result<Vec<u8>> {
        match self {
            Source::Entry(name) => {
                let bytes = read_bytes(file)?;
                let archive = crate::package::ZipArchive::open(&bytes)?;
                archive
                    .read_entry(name, crate::package::PART_MAX_BYTES)?
                    .ok_or_else(|| ReadError::Missing(name.clone()))
            }
            Source::File(path) => read_bytes(path),
            Source::Bytes { bytes, .. } => Ok(bytes.as_ref().clone()),
        }
    }
}

/// How a deck's pages fit a view: slides whole, each as large as the view holds; a document's
/// pages across the view's width, read by scrolling; a picture whole, but never larger than its
/// own size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    Whole,
    Width,
    Picture,
}

/// A colour as a file gives it, with its opacity from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colour {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: f64,
}

impl Colour {
    pub const BLACK: Colour = Colour::opaque(0, 0, 0);
    pub const WHITE: Colour = Colour::opaque(255, 255, 255);

    pub const fn opaque(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }

    /// The colour written as six hexadecimal digits, with or without `#`.
    pub fn hex(digits: &str) -> Option<Self> {
        let digits = digits.strip_prefix('#').unwrap_or(digits);
        if digits.len() != 6 || !digits.is_ascii() {
            return None;
        }
        let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
        Some(Self::opaque(channel(0)?, channel(2)?, channel(4)?))
    }

    /// This colour moved `share` of the way to `other`, keeping its own opacity.
    pub fn toward(self, other: Colour, share: f64) -> Self {
        let share = share.clamp(0.0, 1.0);
        let mix = |from: u8, to: u8| {
            (f64::from(from) + (f64::from(to) - f64::from(from)) * share).round() as u8
        };
        Self {
            red: mix(self.red, other.red),
            green: mix(self.green, other.green),
            blue: mix(self.blue, other.blue),
            alpha: self.alpha,
        }
    }
}

/// A deck: the size every page has, how its pages fit a view, its pages in order, and where the
/// pictures they show are. A picture's bytes are read only when a view draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Deck {
    pub width: f64,
    pub height: f64,
    pub fit: Fit,
    pub slides: Vec<Slide>,
    pub pictures: Vec<Source>,
}

/// One slide: its background and what is on it, back to front.
#[derive(Clone, Debug, PartialEq)]
pub struct Slide {
    pub ground: Colour,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Shape(Shape),
    Picture(Placed),
    Figure(Figure),
}

/// One drawing command of a path, in points from the page's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Move(f64, f64),
    Line(f64, f64),
    /// A quadratic curve: its control point, then where it ends.
    Quad(f64, f64, f64, f64),
    /// A cubic curve: its two control points, then where it ends.
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}

/// A figure: a filled path, such as a glyph drawn from its font's outline. Overlapping parts fill by
/// the nonzero rule, as fonts expect.
#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    pub commands: Vec<Command>,
    pub colour: Colour,
}

/// A picture on a slide: which of the deck's pictures, where, and how much of each edge is cut
/// off before it is stretched to the frame (left, top, right, bottom, as shares of its size).
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub frame: Rect,
    pub picture: usize,
    pub crop: [f64; 4],
}

/// A shape's outline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Form {
    Box,
    /// A box with corners rounded by this radius, in points.
    Rounded(f64),
    Oval,
    /// A straight line between two corners of the frame: top-left to bottom-right, or
    /// bottom-left to top-right when `rising`.
    Line {
        rising: bool,
    },
}

/// A line's colour and thickness in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub colour: Colour,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub frame: Rect,
    pub form: Form,
    pub fill: Option<Colour>,
    pub stroke: Option<Stroke>,
    pub text: Option<Body>,
}

/// Where text sits in its shape, top to bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Anchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// Where a paragraph's lines sit, side to side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

/// A distance between lines or paragraphs: a share of the line's own height, or points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spacing {
    Share(f64),
    Points(f64),
}

/// A shape's text: the space kept clear inside the frame (left, top, right, bottom), where the
/// text sits, whether lines wrap at the frame, and the share its sizes and line spacing were
/// shrunk to so it fits.
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub insets: [f64; 4],
    pub anchor: Anchor,
    pub wraps: bool,
    pub scale: f64,
    pub spacing_scale: f64,
    pub paragraphs: Vec<Paragraph>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    pub align: Align,
    /// How far the paragraph's lines start from the text's left edge.
    pub left: f64,
    /// How far the first line starts from `left`; negative for a hanging bullet.
    pub indent: f64,
    pub line: Spacing,
    pub before: Spacing,
    pub after: Spacing,
    /// The mark before the first line, drawn like the first run.
    pub bullet: Option<String>,
    /// The size an empty paragraph's line has.
    pub size: f64,
    pub runs: Vec<Run>,
}

/// A stretch of text in one look. A line break is a run whose text is `"\n"`.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub colour: Colour,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_says_whether_it_is_drawn_as_pages() {
        assert_eq!(Kind::of_name("Pitch deck.PPTX"), Some(Kind::Presentation));
        assert_eq!(Kind::of_name("show.ppsx"), Some(Kind::Presentation));
        assert_eq!(Kind::of_name("old.ppt"), Some(Kind::LegacyPresentation));
        assert_eq!(Kind::of_name("pptx"), None);
    }

    #[test]
    fn a_colour_reads_from_hex_and_mixes_toward_another() {
        assert_eq!(Colour::hex("1F6F4A"), Some(Colour::opaque(31, 111, 74)));
        assert_eq!(Colour::hex("#ffffff"), Some(Colour::WHITE));
        assert_eq!(Colour::hex("fff"), None);
        assert_eq!(Colour::hex("zzzzzz"), None);
        assert_eq!(
            Colour::BLACK.toward(Colour::WHITE, 0.5),
            Colour::opaque(128, 128, 128)
        );
    }
}
