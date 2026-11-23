//! The board's side of the boundary (ADR-0001): card and folder-art drawing (`board::card`,
//! `board::face`, `board::art`), pointer, wheel and key handling (`board::pointer`,
//! `board::view`), and the layout of an opened 3D model's still (`nr_model_view`). Cards are
//! named by their index in the board as last loaded (`nr_board_load`); `-1` means no card.
//! Text and shapes handed out are borrowed and stay valid until the next call that hands out
//! the same kind.

use crate::app;
use crate::board::card::{Font, Ink, Mark, Measure, Preview, Run, Words};
use crate::board::pointer::{Card, Key, KeyAnswer, Pointer, Press, ShowStep};
use crate::board::Frame;
use crate::ffi::{
    board_card_face, board_layout, board_picture_size, board_text_of, file_row, ids_of,
    running_sessions, text_of, with_columns, NrKey, NrKeyPress, NrRect, NrText, PAGE_MAX_BYTES,
};
use crate::list::row_keys::Held;
use crate::workspace::{Entry, EntryKind};
use crate::Relock;
use std::collections::HashMap;
use std::ffi::c_char;
use std::sync::{Arc, Mutex};

fn lend(text: String) -> NrText {
    static LENT: Mutex<String> = Mutex::new(String::new());
    NrText::lend_into(&LENT, text)
}

const NO_TEXT: NrText = NrText {
    bytes: std::ptr::null(),
    length: 0,
};

fn rect(frame: Frame) -> NrRect {
    NrRect {
        x: frame.x,
        y: frame.y,
        width: frame.width,
        height: frame.height,
    }
}

fn frame(rect: NrRect) -> Frame {
    Frame {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

fn card_index(card: i64) -> Option<usize> {
    usize::try_from(card).ok()
}

fn card_number(index: Option<usize>) -> i64 {
    index.map_or(-1, |index| index as i64)
}

// Cards.

/// A face at a size (`board::card::Font`): Inter at `weight`, or the monospace; `slanted` leans
/// the upright face 0.2 of its height forward; `tabular` sets its digits all one width.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NrCardFont {
    pub size: f64,
    pub weight: f64,
    pub mono: bool,
    pub slanted: bool,
    pub tabular: bool,
}

/// A stretch of a card's words. `color` and `underline` are theme colour names, drawn at
/// `opacity` and `underline_opacity` (`underline` empty for none). `strike` draws a line
/// through; `code` draws inline code's rounded ground behind. A `space` above 0 is empty room
/// that wide in place of words, set in `font`. `tracking` is room after each letter.
#[repr(C)]
pub struct NrCardRun {
    pub text: NrText,
    pub font: NrCardFont,
    pub color: NrText,
    pub opacity: f64,
    pub strike: bool,
    pub code: bool,
    pub underline: NrText,
    pub underline_opacity: f64,
    pub space: f64,
    pub tracking: f64,
}

/// Words set in lines from (`x`, `top`), each `line_height` tall with `line_font`'s height
/// centred in it (`board::card::Words`). `wraps` false keeps each line whole; its own line ends
/// still break. A tab is `tab_spaces` spaces of the first run's font (0: the platform's stops).
/// With `clips`, nothing is drawn outside `clip`.
#[repr(C)]
pub struct NrCardWords {
    pub x: f64,
    pub top: f64,
    pub width: f64,
    pub wraps: bool,
    pub line_height: f64,
    pub line_font: NrCardFont,
    pub tab_spaces: u32,
    pub clips: bool,
    pub clip: NrRect,
    pub runs: *const NrCardRun,
    pub run_count: usize,
}

/// Measures `text` the app's faces have no glyphs for (◦, ▪, ⌘) on one line in the platform's
/// own faces at `font`'s size.
pub type NrMeasureFallback = extern "C" fn(text: NrText, font: NrCardFont) -> f64;

/// Registers the platform's fallback measure once at startup (`typeface::set_fallback`). The
/// core sets every other word itself (`typeface`, `typeset`), so text measures the same on
/// every platform.
#[no_mangle]
pub extern "C" fn nr_set_fallback_measure(measure: NrMeasureFallback) {
    crate::text::typeface::set_fallback(Box::new(move |text, font| {
        measure(NrText::borrowing(text), nr_font(font))
    }));
}

/// What a card mark draws (`NrCardMark::kind`). A colour is a theme colour by name, or a
/// `#rrggbb`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMarkKind {
    /// The glyph named `text` (`glyph::Glyph`) in `frame`, in `color`.
    Glyph,
    /// One line of `text` in `font` and the theme colour `color`, its line box `frame.height`
    /// tall from (`frame.x`, `frame.y`), cut with an ellipsis past `frame.width`.
    Line,
    /// `words`.
    Words,
    /// `frame` filled with `color` at `opacity` of its own opacity, corners rounded `radius`
    /// (0: a plain rectangle).
    Fill,
    /// A to-do's box standing on the baseline of the `line_height` line at `line_top`, set for
    /// `line_font` (`baseline::browser`); `frame.y` is how far its top sits above that
    /// baseline. Filled with `color`, or outlined `stroke` wide when `stroke` is above 0,
    /// `inset` inside. Corners rounded `radius`.
    TodoBox,
    /// `frame` fades what is under it into `color`, clear at its top and solid at its bottom.
    Fade,
    /// The oval inside `frame` filled with `color` at `opacity` of its own opacity.
    Oval,
    /// The card's folder art (`nr_board_folder_art`) in `frame`, in `color`.
    FolderArt,
    /// The card's file as a picture (a video's first frame), at most `pixels` on a side,
    /// covering `frame` about its middle.
    Picture,
    /// The video frame `seconds` in, at most `pixels` on a side, covering `frame` about its
    /// middle.
    VideoMoment,
    /// A picture the core keeps (not a drive file) at path `text`, at most `pixels` on a side,
    /// covering `frame` about its middle.
    PictureAt,
    /// The logo of the terminal program named `text` in `frame`, in its own colours.
    Logo,
    /// The outline of `frame` in `color` at `opacity`, `stroke` wide about its edge, corners
    /// rounded `radius` (0: a plain rectangle).
    Outline,
    /// The picture file with id `text`, at most `pixels` on a side, filling `frame` on the
    /// theme colour `color`, corners rounded `radius`.
    PictureFile,
    /// `words` set as the open page sets them (`board::card::Mark::PageWords`).
    PageWords,
    /// The system thumbnail of the card's file, at most `pixels` on a side, covering `frame`
    /// from its top edge, with an inner border so a white page keeps its edge on a light board.
    /// When a thumbnail exists the card's marks end here; while loading, or without one, the
    /// marks after it draw (`board::card::Mark::SystemPicture`).
    SystemPicture,
    /// A session's dot inside `frame` in the shape named `text` (`terminal::activity::Sign`),
    /// in `color` at `opacity` of its own opacity.
    Sign,
}

/// One mark of a card, drawn in order.
#[repr(C)]
pub struct NrCardMark {
    pub kind: NrMarkKind,
    pub frame: NrRect,
    pub color: NrText,
    pub opacity: f64,
    pub radius: f64,
    pub text: NrText,
    pub font: NrCardFont,
    pub line_top: f64,
    pub line_height: f64,
    pub line_font: NrCardFont,
    pub stroke: f64,
    pub inset: f64,
    pub words: NrCardWords,
    pub video: bool,
    pub pixels: u32,
    /// How far into a video its `VideoMoment` is.
    pub seconds: f64,
    /// A `Picture` or `VideoMoment` shown whole inside `frame`, on the card's ground, rather
    /// than covering it (`board::shown_whole`).
    pub whole: bool,
}

#[repr(C)]
pub struct NrCardMarks {
    pub marks: *const NrCardMark,
    pub count: usize,
}

fn nr_font(font: Font) -> NrCardFont {
    NrCardFont {
        size: font.size,
        weight: font.weight,
        mono: font.mono,
        slanted: font.slanted,
        tabular: font.tabular,
    }
}

const NO_FONT: NrCardFont = NrCardFont {
    size: 0.0,
    weight: 0.0,
    mono: false,
    slanted: false,
    tabular: false,
};

fn nr_run(run: &Run) -> NrCardRun {
    let (text, style, space) = match run {
        Run::Words { text, style } => (NrText::borrowing(text), style, 0.0),
        Run::Space { width, style } => (NO_TEXT, style, *width),
    };
    NrCardRun {
        text,
        font: nr_font(style.font),
        color: NrText::borrowing(style.color.name()),
        opacity: style.opacity,
        strike: style.strike,
        code: style.code,
        underline: style
            .underline
            .map_or(NO_TEXT, |ink| NrText::borrowing(ink.paint.name())),
        underline_opacity: style.underline.map_or(0.0, |ink| ink.opacity),
        space,
        tracking: style.tracking,
    }
}

/// `words` for the boundary, its runs in `runs` (from `nr_run`).
fn nr_words(words: &Words, runs: &[NrCardRun]) -> NrCardWords {
    NrCardWords {
        x: words.x,
        top: words.top,
        width: words.width,
        wraps: words.wraps,
        line_height: words.line_height,
        line_font: nr_font(words.line_font),
        tab_spaces: words.tab_spaces,
        clips: words.clip.is_some(),
        clip: rect(words.clip.unwrap_or(Frame {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        })),
        runs: runs.as_ptr(),
        run_count: runs.len(),
    }
}

fn no_words() -> NrCardWords {
    nr_words(
        &Words {
            x: 0.0,
            top: 0.0,
            width: 0.0,
            wraps: false,
            line_height: 0.0,
            line_font: Font::sans(0.0, 0.0),
            tab_spaces: 0,
            clip: None,
            runs: Vec::new(),
        },
        &[],
    )
}

fn nr_mark(mark: &Mark, runs: &[NrCardRun]) -> NrCardMark {
    let blank = NrCardMark {
        kind: NrMarkKind::Glyph,
        frame: rect(Frame {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        }),
        color: NO_TEXT,
        opacity: 1.0,
        radius: 0.0,
        text: NO_TEXT,
        font: NO_FONT,
        line_top: 0.0,
        line_height: 0.0,
        line_font: NO_FONT,
        stroke: 0.0,
        inset: 0.0,
        words: no_words(),
        video: false,
        pixels: 0,
        seconds: 0.0,
        whole: false,
    };
    let inked = |ink: &Ink| (NrText::borrowing(ink.paint.name()), ink.opacity);
    match mark {
        Mark::Glyph { frame, icon, color } => NrCardMark {
            kind: NrMarkKind::Glyph,
            frame: rect(*frame),
            text: NrText::borrowing(icon.name()),
            color: NrText::borrowing(color.name()),
            ..blank
        },
        Mark::Line {
            text,
            font,
            color,
            x,
            top,
            line_height,
            max_width,
        } => NrCardMark {
            kind: NrMarkKind::Line,
            frame: rect(Frame {
                x: *x,
                y: *top,
                width: *max_width,
                height: *line_height,
            }),
            text: NrText::borrowing(text),
            font: nr_font(*font),
            color: NrText::borrowing(color.name()),
            ..blank
        },
        Mark::Words(words) => NrCardMark {
            kind: NrMarkKind::Words,
            words: nr_words(words, runs),
            ..blank
        },
        Mark::PageWords(words) => NrCardMark {
            kind: NrMarkKind::PageWords,
            words: nr_words(words, runs),
            ..blank
        },
        Mark::Fill { frame, radius, ink } => {
            let (color, opacity) = inked(ink);
            NrCardMark {
                kind: NrMarkKind::Fill,
                frame: rect(*frame),
                radius: *radius,
                color,
                opacity,
                ..blank
            }
        }
        Mark::Box {
            frame,
            line_top,
            line_height,
            line_font,
            radius,
            ink,
            stroke,
            inset,
        } => {
            let (color, opacity) = inked(ink);
            NrCardMark {
                kind: NrMarkKind::TodoBox,
                frame: rect(*frame),
                line_top: *line_top,
                line_height: *line_height,
                line_font: nr_font(*line_font),
                radius: *radius,
                color,
                opacity,
                stroke: *stroke,
                inset: *inset,
                ..blank
            }
        }
        Mark::Fade { frame, token } => NrCardMark {
            kind: NrMarkKind::Fade,
            frame: rect(*frame),
            color: NrText::borrowing(token.name()),
            ..blank
        },
        Mark::Oval { frame, ink } => {
            let (color, opacity) = inked(ink);
            NrCardMark {
                kind: NrMarkKind::Oval,
                frame: rect(*frame),
                color,
                opacity,
                ..blank
            }
        }
        Mark::Sign { frame, sign, ink } => {
            let (color, opacity) = inked(ink);
            NrCardMark {
                kind: NrMarkKind::Sign,
                frame: rect(*frame),
                text: NrText::borrowing(sign.name()),
                color,
                opacity,
                ..blank
            }
        }
        Mark::Art { frame, color } => NrCardMark {
            kind: NrMarkKind::FolderArt,
            frame: rect(*frame),
            color: NrText::borrowing(color),
            ..blank
        },
        Mark::Picture {
            frame,
            video,
            pixels,
            whole,
        } => NrCardMark {
            kind: NrMarkKind::Picture,
            frame: rect(*frame),
            video: *video,
            pixels: *pixels,
            whole: *whole,
            ..blank
        },
        Mark::VideoMoment {
            frame,
            seconds,
            pixels,
            whole,
        } => NrCardMark {
            kind: NrMarkKind::VideoMoment,
            frame: rect(*frame),
            seconds: *seconds,
            pixels: *pixels,
            whole: *whole,
            ..blank
        },
        Mark::PictureAt {
            frame,
            path,
            pixels,
        } => NrCardMark {
            kind: NrMarkKind::PictureAt,
            frame: rect(*frame),
            text: NrText::borrowing(path),
            pixels: *pixels,
            ..blank
        },
        Mark::SystemPicture { frame, pixels } => NrCardMark {
            kind: NrMarkKind::SystemPicture,
            frame: rect(*frame),
            pixels: *pixels,
            ..blank
        },
        Mark::Logo { frame, name } => NrCardMark {
            kind: NrMarkKind::Logo,
            frame: rect(*frame),
            text: NrText::borrowing(name),
            ..blank
        },
        Mark::Stroke {
            frame,
            radius,
            ink,
            width,
        } => {
            let (color, opacity) = inked(ink);
            NrCardMark {
                kind: NrMarkKind::Outline,
                frame: rect(*frame),
                radius: *radius,
                color,
                opacity,
                stroke: *width,
                ..blank
            }
        }
        Mark::Image {
            frame,
            id,
            radius,
            ground,
            pixels,
        } => NrCardMark {
            kind: NrMarkKind::PictureFile,
            frame: rect(*frame),
            text: NrText::borrowing(id),
            radius: *radius,
            color: NrText::borrowing(ground.name()),
            pixels: *pixels,
            ..blank
        },
    }
}

/// A card's marks as last handed out, and what their pointers point into.
struct LentCard {
    _marks: Vec<Mark>,
    _runs: Vec<Vec<NrCardRun>>,
    lent: Vec<NrCardMark>,
}

// SAFETY: the pointers in `lent` and `_runs` point only into the strings and vectors this
// value owns, and are read only while the mutex holding it is not changed.
unsafe impl Send for LentCard {}

/// Each page card's marks as last set, by id, with the frame they were set for: a card is set
/// again only when its frame changes or the board loads again.
static CARDS: Mutex<Option<HashMap<String, (Frame, LentCard)>>> = Mutex::new(None);

/// What each page card last loaded previews, read once per load.
static PREVIEWS: Mutex<Option<HashMap<String, Arc<Preview>>>> = Mutex::new(None);

/// The board was loaded again: every page card reads its file afresh.
pub(crate) fn forget_previews() {
    *PREVIEWS.relock() = None;
    *CARDS.relock() = None;
}

/// What the card of `entry`, whose face is `face`, previews: a code file's source, a text
/// file's paragraphs, or a page file's first blocks.
fn read_preview(entry: &Entry, face: crate::board::CardFace) -> Preview {
    use crate::board::{code_preview, page_preview, text_card, TextCard};
    if face == crate::board::CardFace::PageCode {
        return board_text_of(entry)
            .and_then(|text| code_preview(&text))
            .map_or(Preview::Empty, Preview::Code);
    }
    if entry.kind == EntryKind::File {
        return match board_text_of(entry).map(|text| text_card(&entry.title, &text, false)) {
            Some(TextCard::Page(page)) => Preview::Page(page),
            Some(TextCard::Code(_) | TextCard::Empty) | None => Preview::Empty,
        };
    }
    let read = || {
        let path = with_columns(|columns| columns.workspace().path_of(&entry.id).ok()).flatten()?;
        if std::fs::metadata(&path).ok()?.len() > PAGE_MAX_BYTES {
            return None;
        }
        page_preview(
            &entry.title,
            &std::fs::read_to_string(&path).ok()?,
            crate::ffi::interface_language(),
        )
    };
    read().map_or(Preview::Empty, Preview::Page)
}

fn preview_of(entry: &Entry, face: crate::board::CardFace) -> Arc<Preview> {
    if let Some(preview) = PREVIEWS
        .relock()
        .as_ref()
        .and_then(|cached| cached.get(&entry.id))
    {
        return preview.clone();
    }
    let preview = Arc::new(read_preview(entry, face));
    PREVIEWS
        .relock()
        .get_or_insert_with(HashMap::new)
        .insert(entry.id.clone(), preview.clone());
    preview
}

/// `marks` with the boundary's view of them, which points into them.
fn lent_card(marks: Vec<Mark>) -> LentCard {
    let runs: Vec<Vec<NrCardRun>> = marks
        .iter()
        .map(|mark| match mark {
            Mark::Words(words) | Mark::PageWords(words) => words.runs.iter().map(nr_run).collect(),
            _ => Vec::new(),
        })
        .collect();
    let lent: Vec<NrCardMark> = marks
        .iter()
        .zip(&runs)
        .map(|(mark, runs)| nr_mark(mark, runs))
        .collect();
    LentCard {
        _marks: marks,
        _runs: runs,
        lent,
    }
}

fn handed(held: &LentCard) -> NrCardMarks {
    NrCardMarks {
        marks: held.lent.as_ptr(),
        count: held.lent.len(),
    }
}

/// The marks of the last card that is not a page's handed out.
static FACE: Mutex<Option<LentCard>> = Mutex::new(None);

/// The marks of a page card (`board::card::page_card`): a page file's, a text file's or a code
/// file's, set again only when its frame changes or the board loads again.
fn page_marks(
    entry: &Entry,
    face: crate::board::CardFace,
    place: Frame,
    measure: &mut Measure,
) -> NrCardMarks {
    if let Some((set_for, held)) = CARDS
        .relock()
        .as_ref()
        .and_then(|cards| cards.get(&entry.id))
    {
        if *set_for == place {
            return handed(held);
        }
    }
    let preview = preview_of(entry, face);
    let marks = crate::board::card::page_card(
        place,
        crate::board::card_unit(place.width),
        &entry.title,
        face == crate::board::CardFace::PageCode,
        &preview,
        crate::ffi::interface_language(),
        measure,
    );
    let mut cards = CARDS.relock();
    let cards = cards.get_or_insert_with(HashMap::new);
    cards.insert(entry.id.clone(), (place, lent_card(marks)));
    handed(&cards[&entry.id].1)
}

/// The marks of card `card` drawn in `frame`, for the face the core chooses
/// (`board::face::kind_of`): a page card (`board::card::page_card`) or another
/// (`board::face::face_marks`). Empty for no card. A page card's marks stay valid until the
/// board loads again; any other's until the next call.
#[no_mangle]
pub extern "C" fn nr_board_card(card: usize, frame_rect: NrRect) -> NrCardMarks {
    use crate::board::face::{face_marks, kind_of, Face, Kind};
    let entry = app().board_rows.relock().get(card).cloned();
    let Some(entry) = entry else {
        return NrCardMarks {
            marks: std::ptr::null(),
            count: 0,
        };
    };
    let mut set = crate::text::typeset::measure;
    let measure: &mut Measure = &mut set;
    let place = frame(frame_rect);
    let face = board_card_face(card);
    let path = || {
        with_columns(|columns| columns.workspace().path_of(&entry.id).ok())
            .flatten()
            .map(|path| path.to_string_lossy().into_owned())
    };
    let (lead, length, still);
    let skim = pointer(|pointer| pointer.skimmed(&entry.id));
    let shown = match kind_of(entry.kind, face) {
        Kind::Page => return page_marks(&entry, face, place, measure),
        Kind::Folder => Face::Folder {
            title: &entry.title,
        },
        Kind::Terminal => {
            use crate::terminal::activity::{label_of, look, Activity};
            let session = crate::ffi::running_session(&entry.id);
            let activity = session
                .as_ref()
                .and_then(|session| Activity::numbered(session.activity));
            let agent = session.and_then(|session| session.agent);
            lead = crate::board::terminal_card_lead(agent, &entry.title);
            Face::Terminal {
                title: &entry.title,
                look: look(activity),
                doing: label_of(activity, crate::ffi::interface_language()),
                lead: &lead,
                logo: agent.and_then(crate::terminal::agent::Agent::logo),
            }
        }
        Kind::Picture => Face::Picture {
            video: false,
            whole: crate::board::shown_whole(board_picture_size(card)),
        },
        Kind::Video => {
            let seconds = path()
                .and_then(|path| {
                    crate::media::probe_once(std::path::Path::new(&path)).duration_seconds
                })
                .filter(|seconds| *seconds > 0.0);
            length = seconds.map(crate::preview::player::format_timestamp);
            Face::Plays {
                at: skim
                    .zip(seconds)
                    .map(|(across, seconds)| crate::board::skim_fraction(across) * seconds),
                whole: crate::board::shown_whole(board_picture_size(card)),
                length: length.as_deref(),
                skim,
            }
        }
        Kind::File => Face::File {
            title: &entry.title,
            family: entry.family,
        },
        Kind::SystemPicture => Face::SystemPicture {
            title: &entry.title,
            family: entry.family,
        },
        Kind::Model => {
            still = path()
                .and_then(|path| crate::model_still::picture(std::path::Path::new(&path)))
                .map(|file| file.to_string_lossy().into_owned());
            Face::Model {
                title: &entry.title,
                family: entry.family,
                still: still.as_deref(),
            }
        }
    };
    let marks = face_marks(place, crate::board::card_unit(place.width), &shown, measure);
    let mut held = FACE.relock();
    handed(held.insert(lent_card(marks)))
}

/// The layout of an opened model's `width` × `height` view (`model_still::view_layout`). With
/// `shows`, the still goes in `still` on `backdrop` (`#rrggbb`); otherwise the model's glyph
/// goes in `glyph` while the still renders or if it cannot.
#[repr(C)]
pub struct NrModelView {
    pub shows: bool,
    pub still: NrRect,
    pub glyph: NrRect,
    pub backdrop: NrText,
}

#[no_mangle]
pub extern "C" fn nr_model_view(width: f64, height: f64) -> NrModelView {
    let (still, glyph) = crate::model_still::view_layout(width, height);
    NrModelView {
        shows: still.is_some(),
        still: rect(still.unwrap_or(glyph)),
        glyph: rect(glyph),
        backdrop: NrText::borrowing(crate::board::face::MODEL_BACKDROP.name()),
    }
}

// The board's view.

/// Lays out the last loaded cards on a board `width` pt wide at the board's zoom and returns
/// the scrollable height for a view `visible_height` tall (`board::view::canvas_height`).
#[no_mangle]
pub extern "C" fn nr_board_place(width: f64, visible_height: f64) -> f64 {
    crate::board::view::canvas_height(board_layout(width, 0), visible_height)
}

/// One wheel event over a board `width` pt wide. `delta_y` is positive towards the person, in
/// `unit` (0 pixels, 1 lines, 2 pages, 3 wheel clicks; `board::view::wheel_travel`). Answers as
/// `board_zoom_wheel`; a wheel that does not zoom (-1) goes to the board's scroller
/// (`scroll::route::wheel_scroll`).
#[no_mangle]
pub extern "C" fn nr_board_wheel(
    delta_y: f64,
    unit: u32,
    ctrl: bool,
    meta: bool,
    width: f64,
) -> i64 {
    let (travel, unit) = crate::board::view::wheel_travel(delta_y, unit);
    crate::ffi::board_zoom_wheel(travel, unit, ctrl, meta, width)
}

// Folder art.

/// A colour in sRGB, 0 to 1.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NrRgba {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

#[repr(C)]
pub struct NrPoint {
    pub x: f64,
    pub y: f64,
}

/// One outline of a shape: with `rounded`, `frame` with its corners rounded `radius_x` across
/// and `radius_y` down; otherwise `points` joined by straight lines and closed.
#[repr(C)]
pub struct NrArtContour {
    pub rounded: bool,
    pub frame: NrRect,
    pub radius_x: f64,
    pub radius_y: f64,
    pub points: *const NrPoint,
    pub point_count: usize,
}

/// Outlines drawn through `transform`, filled together in `fill`, then, when `stroked`,
/// outlined in `stroke` `stroke_width` wide (in transformed units). `transform` is an affine
/// `[a, b, c, d, tx, ty]` that maps (x, y) to (a·x + c·y + tx, b·x + d·y + ty).
#[repr(C)]
pub struct NrArtShape {
    pub transform: [f64; 6],
    pub contours: *const NrArtContour,
    pub contour_count: usize,
    pub fill: NrRgba,
    pub stroked: bool,
    pub stroke: NrRgba,
    pub stroke_width: f64,
}

#[repr(C)]
pub struct NrArt {
    pub shapes: *const NrArtShape,
    pub count: usize,
}

struct LentArt {
    _points: Vec<Vec<Vec<NrPoint>>>,
    _contours: Vec<Vec<NrArtContour>>,
    shapes: Vec<NrArtShape>,
}

// SAFETY: the pointers in `shapes` and `_contours` point only into the vectors this value
// owns, and are read only while the mutex holding it is not changed.
unsafe impl Send for LentArt {}

static ART: Mutex<Option<LentArt>> = Mutex::new(None);

fn rgba(color: crate::board::art::Rgba) -> NrRgba {
    NrRgba {
        red: color.red,
        green: color.green,
        blue: color.blue,
        alpha: color.alpha,
    }
}

/// The art of folder card `card` in `frame` (its 321:270 box) in `color`, posed as at `now`
/// seconds on the shell's clock (`board::art::folder_art`).
#[no_mangle]
pub extern "C" fn nr_board_folder_art(
    card: usize,
    frame_rect: NrRect,
    color: NrRgba,
    now: f64,
) -> NrArt {
    use crate::board::art::{folder_art, Contour, Rgba};
    let id = app()
        .board_rows
        .relock()
        .get(card)
        .map(|entry| entry.id.clone());
    let pose = pointer(|pointer| {
        id.map_or(crate::board::FolderTarget::Rest.pose(), |id| {
            pointer.pose(&id, now)
        })
    });
    let color = Rgba {
        red: color.red,
        green: color.green,
        blue: color.blue,
        alpha: color.alpha,
    };
    let shapes = folder_art(frame(frame_rect), color, &pose);
    let points: Vec<Vec<Vec<NrPoint>>> = shapes
        .iter()
        .map(|shape| {
            shape
                .contours
                .iter()
                .map(|contour| match contour {
                    Contour::Polygon(points) => points
                        .iter()
                        .map(|(x, y)| NrPoint { x: *x, y: *y })
                        .collect(),
                    Contour::Rounded { .. } => Vec::new(),
                })
                .collect()
        })
        .collect();
    let contours: Vec<Vec<NrArtContour>> = shapes
        .iter()
        .zip(&points)
        .map(|(shape, points)| {
            shape
                .contours
                .iter()
                .zip(points)
                .map(|(contour, points)| match contour {
                    Contour::Rounded {
                        frame,
                        radius_x,
                        radius_y,
                    } => NrArtContour {
                        rounded: true,
                        frame: rect(*frame),
                        radius_x: *radius_x,
                        radius_y: *radius_y,
                        points: std::ptr::null(),
                        point_count: 0,
                    },
                    Contour::Polygon(_) => NrArtContour {
                        rounded: false,
                        frame: rect(Frame {
                            x: 0.0,
                            y: 0.0,
                            width: 0.0,
                            height: 0.0,
                        }),
                        radius_x: 0.0,
                        radius_y: 0.0,
                        points: points.as_ptr(),
                        point_count: points.len(),
                    },
                })
                .collect()
        })
        .collect();
    let lent: Vec<NrArtShape> = shapes
        .iter()
        .zip(&contours)
        .map(|(shape, contours)| NrArtShape {
            transform: shape.transform,
            contours: contours.as_ptr(),
            contour_count: contours.len(),
            fill: rgba(shape.fill),
            stroked: shape.stroke.is_some(),
            stroke: rgba(shape.stroke.unwrap_or(shape.fill)),
            stroke_width: shape.stroke_width,
        })
        .collect();
    let mut art = ART.relock();
    let held = art.insert(LentArt {
        _points: points,
        _contours: contours,
        shapes: lent,
    });
    NrArt {
        shapes: held.shapes.as_ptr(),
        count: held.shapes.len(),
    }
}

// The pointer and the keys.

static POINTER: Mutex<Option<Pointer>> = Mutex::new(None);

fn pointer<T>(action: impl FnOnce(&mut Pointer) -> T) -> T {
    action(POINTER.relock().get_or_insert_with(Pointer::default))
}

/// `action` with the board's pointer and its cards as last loaded.
fn board<T>(action: impl FnOnce(&mut Pointer, &[Card]) -> T) -> T {
    let running = running_sessions();
    let rows = app().board_rows.relock();
    let cards: Vec<Card> = rows
        .iter()
        .map(|entry| Card {
            id: &entry.id,
            folder: entry.kind == EntryKind::Folder,
            takes_drops: file_row(entry, &running).takes_drops,
            protected: entry.protection.is_some(),
        })
        .collect();
    pointer(|pointer| action(pointer, &cards))
}

fn held_of(held: u32) -> Held {
    Held {
        command: held & crate::menus::COMMAND != 0,
        control: held & crate::menus::CONTROL != 0,
        option: held & crate::menus::OPTION != 0,
        shift: held & crate::menus::SHIFT != 0,
    }
}

/// A board was made: nothing on it is selected, has the keys, is under the pointer or moving.
#[no_mangle]
pub extern "C" fn nr_board_reset() {
    *POINTER.relock() = None;
}

/// What a press on the board did.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrBoardPress {
    /// Nothing is selected or has the keys any more.
    Cleared,
    /// The card is selected and has the keys (the press may become a drag).
    Pressed,
    /// The card is to be opened.
    Open,
}

/// A press on `card` (-1: the board's empty space), the `clicks`-th of a run. The board
/// selects one card at a time, whatever keys are held.
#[no_mangle]
pub extern "C" fn nr_board_press(card: i64, clicks: u32) -> NrBoardPress {
    let pressed = board(|pointer, cards| {
        pointer.press(
            card_index(card)
                .and_then(|index| cards.get(index))
                .map(|card| card.id),
            clicks,
        )
    });
    match pressed {
        Press::Cleared => NrBoardPress::Cleared,
        Press::Pressed => NrBoardPress::Pressed,
        Press::Open => NrBoardPress::Open,
    }
}

/// What a key asks of the board.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrBoardKeyAnswer {
    /// The board leaves it to whoever is next.
    PassOn,
    /// The board took it and is drawn again.
    Taken,
    /// The keys leave the board for the next view.
    Leave,
    /// `card` is to be opened.
    Open,
    /// The selected card goes to the trash: `nr_drive_edit` with `NrDriveEdit::Trash`.
    Trash,
}

/// What a key asks of the board, and the card it names.
#[repr(C)]
pub struct NrBoardKey {
    pub answer: NrBoardKeyAnswer,
    pub card: i64,
}

/// A key pressed on the board: Tab, Space and Return are its own.
#[no_mangle]
pub extern "C" fn nr_board_key(press: NrKeyPress) -> NrBoardKey {
    let key = match press.key {
        NrKey::Tab => Key::Tab,
        NrKey::Space => Key::Space,
        NrKey::Return => Key::Return,
        NrKey::Delete => Key::Delete,
        NrKey::Backspace => Key::Backspace,
        _ => Key::Other,
    };
    let (answer, card) = match board(|pointer, cards| pointer.key(key, held_of(press.held), cards))
    {
        KeyAnswer::NotTaken => (NrBoardKeyAnswer::PassOn, -1),
        KeyAnswer::Taken => (NrBoardKeyAnswer::Taken, -1),
        KeyAnswer::Leave => (NrBoardKeyAnswer::Leave, -1),
        KeyAnswer::Open(index) => (NrBoardKeyAnswer::Open, index as i64),
        KeyAnswer::Trash => (NrBoardKeyAnswer::Trash, -1),
    };
    NrBoardKey { answer, card }
}

/// The id of the card the board has selected: one at a time, kept by the board's own pointer
/// rather than by its column's selection. `None` when none is, or it is no longer on the board.
pub(crate) fn board_selected_id() -> Option<String> {
    board(|pointer, cards| {
        pointer
            .selected(cards)
            .map(|index| cards[index].id.to_owned())
    })
}

/// The card selected, the card with the keys, the card wearing the keys' ring (shown while the
/// board has the keys) and the card a drag is over; -1 for none.
#[repr(C)]
pub struct NrBoardFocus {
    pub selected: i64,
    pub focused: i64,
    pub ring: i64,
    pub drop: i64,
}

#[no_mangle]
pub extern "C" fn nr_board_focus() -> NrBoardFocus {
    board(|pointer, cards| NrBoardFocus {
        selected: card_number(pointer.selected(cards)),
        focused: card_number(pointer.focused(cards)),
        ring: card_number(pointer.ring(cards)),
        drop: card_number(pointer.drop_card(cards)),
    })
}

/// The pointer is over `card` (-1: no card, or off the board). Returns whether the folder
/// under it changed, so the folders' art should move (`nr_board_folders_retarget`).
#[no_mangle]
pub extern "C" fn nr_board_hover(card: i64) -> bool {
    board(|pointer, cards| {
        pointer.hover(card_index(card).and_then(|index| cards.get(index)).copied())
    })
}

/// The pointer is over card `card` (-1: no card, or off the board), `across` it from 0 at its
/// left edge to 1 at its right. A video card shows the moment there and its play button.
/// Returns whether the card needs redrawing.
#[no_mangle]
pub extern "C" fn nr_board_skim(card: i64, across: f64) -> bool {
    let playing = card_index(card).and_then(|index| {
        let entry = app().board_rows.relock().get(index).cloned()?;
        plays(&entry).then_some(entry.id)
    });
    pointer(|pointer| pointer.skim(playing.map(|id| crate::board::pointer::Skim { id, across })))
}

/// Whether `entry`'s card plays: a video's.
fn plays(entry: &Entry) -> bool {
    entry.kind == EntryKind::Video
}

/// The card whose play button is under (`x`, `y`), a point on the board as the cards' frames
/// are (`nr_board_frame`): its number, or -1. The button shows on the card the pointer skims
/// (`nr_board_skim`); a press there plays its video (`nr_folder_play`).
#[no_mangle]
pub extern "C" fn nr_board_play_at(x: f64, y: f64) -> i64 {
    let frames = app().board_frames.relock();
    let rows = app().board_rows.relock();
    let found = frames.iter().zip(rows.iter()).position(|(card, entry)| {
        pointer(|pointer| pointer.skimmed(&entry.id).is_some()) && {
            let unit = crate::board::card_unit(card.width);
            let at = crate::board::face::play_frame(*card, unit);
            let (dx, dy) = (x - (at.x + at.width / 2.0), y - (at.y + at.height / 2.0));
            dx * dx + dy * dy <= (at.width / 2.0).powi(2)
        }
    });
    card_number(found)
}

/// A press on a card became a drag. Returns the ids of the selected cards, one per line,
/// leaving out system folders; empty when nothing can be dragged. Cards do not move, since
/// the board is in name order.
#[no_mangle]
pub extern "C" fn nr_board_pick_up() -> NrText {
    let ids: Vec<String> = board(|pointer, cards| {
        pointer
            .pick_up(cards)
            .into_iter()
            .map(str::to_owned)
            .collect()
    });
    lend(ids.join("\n"))
}

/// The files `ids` (one per line; empty for files from outside the app) are dragged over
/// `card` (-1 for none). Returns whether the drop target changed, so the folders' art should
/// move (`nr_board_folders_retarget`).
///
/// # Safety
/// `ids` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_board_drag_over(ids: *const c_char, card: i64) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    let dragged = ids_of(&unsafe { text_of(ids) });
    board(|pointer, cards| {
        pointer.drag_over(
            card_index(card).and_then(|index| cards.get(index)).copied(),
            &dragged,
        )
    })
}

/// The drag left the board or ended; no card takes it. Returns whether the drop target
/// changed.
#[no_mangle]
pub extern "C" fn nr_board_drag_leave() -> bool {
    pointer(|pointer| pointer.set_drop(None))
}

/// Where a drop on the board goes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrBoardDropKind {
    /// Files from outside the app, to be copied into the folder shown.
    Outside,
    /// Onto `card`, a folder to move into or a terminal to hand them to.
    Card,
    /// Onto no card: nothing moves.
    Nowhere,
}

/// Where a drop on the board goes. `folders` is true when the drop target changed.
#[repr(C)]
pub struct NrBoardDrop {
    pub kind: NrBoardDropKind,
    pub card: i64,
    pub folders: bool,
}

/// The files `ids` (one per line, as `nr_board_drag_over`) are dropped where they were last
/// dragged.
///
/// # Safety
/// `ids` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_board_drop(ids: *const c_char) -> NrBoardDrop {
    // SAFETY: the caller passes a NUL-terminated string.
    let dragged = ids_of(&unsafe { text_of(ids) });
    let (target, folders) =
        board(|pointer, cards| (pointer.drop_card(cards), pointer.set_drop(None)));
    let (kind, card) = match target {
        _ if dragged.is_empty() => (NrBoardDropKind::Outside, -1),
        Some(index) => (NrBoardDropKind::Card, index as i64),
        None => (NrBoardDropKind::Nowhere, -1),
    };
    NrBoardDrop {
        kind,
        card,
        folders,
    }
}

/// Sends each folder's art towards the pose for its current hover or drag state
/// (`board::pointer::Pointer::retarget`). `now` is the shell's clock in seconds; with motion
/// `reduced` the art jumps there. Returns whether to draw the folders every
/// `nr_frame_seconds(Moving)` (`nr_board_folders_step`).
#[no_mangle]
pub extern "C" fn nr_board_folders_retarget(now: f64, reduced: bool) -> bool {
    pointer(|pointer| pointer.retarget(now, reduced))
}

/// Called after drawing a frame of folder motion at `now`. Forgets art that has come to rest
/// and returns whether any is still moving.
#[no_mangle]
pub extern "C" fn nr_board_folders_step(now: f64) -> bool {
    pointer(|pointer| pointer.step(now))
}

/// What `NR_BOARD_SHOW` changed: `staged` means anything (lay out again), `zoomed` the zoom
/// (show the zoom label and keep it up), `folders` a hover or drag
/// (`nr_board_folders_retarget`).
#[repr(C)]
pub struct NrBoardStaged {
    pub staged: bool,
    pub zoomed: bool,
    pub folders: bool,
}

/// Puts the board, once it has a width, in the states `NR_BOARD_SHOW` names, for a picture of
/// states only a pointer reaches (`board::pointer::show_steps`).
#[no_mangle]
pub extern "C" fn nr_board_stage() -> NrBoardStaged {
    let steps =
        crate::board::pointer::show_steps(&std::env::var("NR_BOARD_SHOW").unwrap_or_default());
    let ids: Vec<String> = app()
        .board_rows
        .relock()
        .iter()
        .map(|entry| entry.id.clone())
        .collect();
    let mut staged = NrBoardStaged {
        staged: !steps.is_empty(),
        zoomed: false,
        folders: false,
    };
    for step in steps {
        match step {
            ShowStep::Zoom(count) => {
                with_columns(|columns| columns.set_board_columns(count));
                staged.zoomed = true;
            }
            ShowStep::Hover(index) if index < ids.len() => {
                staged.folders |= pointer(|pointer| pointer.set_hovered(Some(ids[index].clone())));
            }
            ShowStep::Drop(index) if index < ids.len() => {
                staged.folders |= pointer(|pointer| pointer.set_drop(Some(ids[index].clone())));
            }
            ShowStep::Hover(_) | ShowStep::Drop(_) => {}
        }
    }
    staged
}

// The open page: its layout and drawing model (`page::layout`), drawn with the cards' marks.

/// A player block as the shell placed it (`page::layout::PlayerPlaced`): where its room ends, its
/// box, and whether it is drawn as a player.
#[repr(C)]
pub struct NrPagePlayerPlaced {
    pub end: f64,
    pub frame: NrRect,
    pub drawn: bool,
}

/// How the shell places player block `block` with its box's top at `top`, in a column from
/// `side`, `column` wide.
pub type NrPlacePlayer =
    extern "C" fn(block: usize, top: f64, side: f64, column: f64) -> NrPagePlayerPlaced;

/// Where words of the open page are typed in (`page::layout::Slot`): block `block` (-1 the
/// title), its words in `frame` in `font` on `line_height` lines, `tracking` after each letter.
/// `row` and `column` name a table's cell (-1 for any other); `caption` a board media block's
/// caption field; `ground` the theme colour the words sit on when it is not the page's own
/// (empty for the page's).
#[repr(C)]
pub struct NrPageSlot {
    pub block: i64,
    pub frame: NrRect,
    pub font: NrCardFont,
    pub line_height: f64,
    pub tracking: f64,
    pub row: i64,
    pub column: i64,
    pub caption: bool,
    pub ground: NrText,
}

/// One block's room (`page::layout::Placed`): its `kind` as `page::layout::kind_code` numbers it
/// (0 paragraph, 1 heading, 2 bullet, 3 numbered, 4 to-do, 5 quote, 6 code, 7 HTML artifact,
/// 8 divider, 9 table, 10 media, 11 a player), a heading's `level`, and whether it is the
/// page's `first`.
#[repr(C)]
pub struct NrPagePlaced {
    pub frame: NrRect,
    pub kind: u32,
    pub level: u32,
    pub first: bool,
}

/// A block's part of the page: a to-do's marker, or a board media block's frame.
#[repr(C)]
pub struct NrPagePart {
    pub block: usize,
    pub frame: NrRect,
}

/// When a page mark is drawn (`page::layout::Shown`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMarkWhen {
    Always,
    /// While slot `slot` is not typed in.
    Resting,
    /// While it is.
    Typing,
    /// While it is and holds nothing.
    TypingEmpty,
}

/// When mark `marks[i]` is drawn, and the room it draws in.
#[repr(C)]
pub struct NrPageMarkShown {
    pub when: NrMarkWhen,
    pub slot: i64,
    pub bounds: NrRect,
}

/// The open page laid out (`page::layout`). `marks` (with `shown`, as many) are drawn in
/// order, as a card's are (`NrCardMark`); `side` is the page's side margin, `height` its whole
/// height.
#[repr(C)]
pub struct NrPageLayout {
    pub marks: *const NrCardMark,
    pub shown: *const NrPageMarkShown,
    pub mark_count: usize,
    pub slots: *const NrPageSlot,
    pub slot_count: usize,
    pub blocks: *const NrPagePlaced,
    pub block_count: usize,
    pub todos: *const NrPagePart,
    pub todo_count: usize,
    pub media: *const NrPagePart,
    pub media_count: usize,
    pub side: f64,
    pub height: f64,
}

/// The open page as last laid out, and the boundary's view of it.
struct LentPage {
    layout: crate::page::layout::Layout,
    marks: LentCard,
    shown: Vec<NrPageMarkShown>,
    slots: Vec<NrPageSlot>,
    blocks: Vec<NrPagePlaced>,
    todos: Vec<NrPagePart>,
    media: Vec<NrPagePart>,
    overlay: Option<(LentCard, LentCard)>,
}

// SAFETY: the pointers handed out point only into the strings and vectors this value owns, and
// are read only while the mutex holding it is not changed.
unsafe impl Send for LentPage {}

/// Each page view's layout, by the key the view gives (`page::layout`): a window may show
/// more than one, and each draws from its own until it lays out again or is forgotten.
static PAGES: Mutex<Option<HashMap<usize, LentPage>>> = Mutex::new(None);

fn pages<T>(action: impl FnOnce(&mut HashMap<usize, LentPage>) -> T) -> T {
    action(PAGES.relock().get_or_insert_with(HashMap::new))
}

fn lent_page(layout: crate::page::layout::Layout) -> LentPage {
    use crate::page::layout::Shown;
    let marks = lent_card(
        layout
            .marks
            .iter()
            .map(|(mark, _, _)| mark.clone())
            .collect(),
    );
    let shown = layout
        .marks
        .iter()
        .map(|(_, shown, bounds)| {
            let (when, slot) = match shown {
                Shown::Always => (NrMarkWhen::Always, -1),
                Shown::Resting(slot) => (NrMarkWhen::Resting, *slot as i64),
                Shown::Typing(slot) => (NrMarkWhen::Typing, *slot as i64),
                Shown::TypingEmpty(slot) => (NrMarkWhen::TypingEmpty, *slot as i64),
            };
            NrPageMarkShown {
                when,
                slot,
                bounds: rect(*bounds),
            }
        })
        .collect();
    let slots = layout
        .slots
        .iter()
        .map(|slot| NrPageSlot {
            block: slot.block,
            frame: rect(slot.frame),
            font: nr_font(slot.font),
            line_height: slot.line_height,
            tracking: slot.tracking,
            row: slot.cell.map_or(-1, |(row, _)| row as i64),
            column: slot.cell.map_or(-1, |(_, column)| column as i64),
            caption: slot.caption,
            ground: slot
                .ground
                .map_or(NO_TEXT, |ground| NrText::borrowing(ground.name())),
        })
        .collect();
    let blocks = layout
        .blocks
        .iter()
        .map(|placed| NrPagePlaced {
            frame: rect(placed.frame),
            kind: placed.kind,
            level: placed.level,
            first: placed.first,
        })
        .collect();
    let part = |(block, frame): &(usize, Frame)| NrPagePart {
        block: *block,
        frame: rect(*frame),
    };
    let todos = layout.todos.iter().map(part).collect();
    let media = layout.media.iter().map(part).collect();
    LentPage {
        layout,
        marks,
        shown,
        slots,
        blocks,
        todos,
        media,
        overlay: None,
    }
}

/// Lays out the open page for page view `view` (any number that stays stable for the view's
/// life), `width` wide, with `viewport` of it visible, in a window `window_width` wide. Returns
/// its marks and rooms, valid until the next `page::layout` or `nr_page_forget` for that view.
/// `typed` is the open field's unsaved text; the block, cell or title being edited is laid out
/// with it, so the field grows and what follows moves down while typing.
/// The core sets words (`typeset`); `place_player` places each player block and may call back
/// into the core. Empty when no page is open.
pub fn page_layout(
    view: usize,
    width: f64,
    viewport: f64,
    window_width: f64,
    place_player: NrPlacePlayer,
    typed: Option<&str>,
) -> NrPageLayout {
    use crate::page::editor::Field;
    use crate::page::layout::{lay_out, PlayerPlaced, Room, Sheet};
    use crate::page::Page;
    let empty = NrPageLayout {
        marks: std::ptr::null(),
        shown: std::ptr::null(),
        mark_count: 0,
        slots: std::ptr::null(),
        slot_count: 0,
        blocks: std::ptr::null(),
        block_count: 0,
        todos: std::ptr::null(),
        todo_count: 0,
        media: std::ptr::null(),
        media_count: 0,
        side: 0.0,
        height: viewport,
    };
    // The page is copied out: placing a player reads it again.
    let read_only = crate::ffi::page_read_only();
    let copied = app().page.relock().as_ref().map(|page| {
        (
            page.title.clone(),
            page.blocks.clone(),
            page.segments.clone(),
            page.numbers.clone(),
        )
    });
    let Some((title, blocks, segments, numbers)) = copied else {
        pages(|pages| pages.remove(&view));
        return empty;
    };
    let mut page = Page {
        title,
        blocks,
        states_title: false,
        segments,
        numbers,
        source: crate::page::PageSource::default(),
    };
    if let (Some(text), Some(field)) = (typed, crate::ffi::page_open_field()) {
        match field {
            // A title is never empty at rest: one cleared while typed in keeps its line.
            Field::Title if !text.trim().is_empty() => page.title = text.to_owned(),
            Field::Title => {}
            Field::Block(block) => {
                page.set_text(block, text);
            }
            Field::Cell { block, row, column } => {
                page.set_table_cell(block, row, column, text);
            }
            Field::Caption(block) => {
                page.set_caption(block, text);
            }
        }
    }
    let Page {
        title,
        blocks,
        segments,
        numbers,
        ..
    } = page;
    let sheet = Sheet {
        title: &title,
        blocks: &blocks,
        segments: &segments,
        numbers: &numbers,
        read_only,
        language: crate::ffi::interface_language(),
    };
    let mut show_image = |block: usize, segment: usize| {
        let image = segments[block][segment]
            .image
            .clone()
            .expect("an image segment");
        crate::ffi::open_page_image_shows(&image)
    };
    let mut place = |block: usize, top: f64, side: f64, column: f64| {
        let placed = place_player(block, top, side, column);
        PlayerPlaced {
            end: placed.end,
            frame: frame(placed.frame),
            drawn: placed.drawn,
        }
    };
    let layout = lay_out(
        &sheet,
        Room {
            width,
            viewport,
            window_width,
        },
        &mut crate::text::typeset::measure,
        &mut show_image,
        &mut place,
    );
    pages(|pages| {
        let held = pages.entry(view).insert_entry(lent_page(layout)).into_mut();
        NrPageLayout {
            marks: held.marks.lent.as_ptr(),
            shown: held.shown.as_ptr(),
            mark_count: held.shown.len(),
            slots: held.slots.as_ptr(),
            slot_count: held.slots.len(),
            blocks: held.blocks.as_ptr(),
            block_count: held.blocks.len(),
            todos: held.todos.as_ptr(),
            todo_count: held.todos.len(),
            media: held.media.as_ptr(),
            media_count: held.media.len(),
            side: held.layout.side,
            height: held.layout.height,
        }
    })
}

/// Lets go of the layout of the page view `view`, which is gone.
#[no_mangle]
pub extern "C" fn nr_page_forget(view: usize) {
    pages(|pages| pages.remove(&view));
}

/// The state of the pointer and the keys the page draws over its blocks
/// (`page::layout::Overlay`). Each block is -1 for none: `chosen_first..=chosen_last` the whole
/// blocks selected, `hovered` the block whose gutter shows with `button` lit (1 the `+`, 2 the
/// handle, 0 none), `drop` where dragged blocks would land (`drop_after` in its lower half),
/// `ringed` the media block gone to with a key. `width` is the page's.
#[repr(C)]
pub struct NrPageOverlayState {
    pub chosen_first: i64,
    pub chosen_last: i64,
    pub hovered: i64,
    pub button: u32,
    pub drop: i64,
    pub drop_after: bool,
    pub ringed: i64,
    pub width: f64,
}

/// What the page draws under its blocks and over them.
#[repr(C)]
pub struct NrPageOverlay {
    pub under: NrCardMarks,
    pub over: NrCardMarks,
}

/// What `view`'s last page layout draws under its blocks (selected rows) and over them (rings,
/// the gutter, the drop line) in `state`. Valid until the next call or `page::layout` for the
/// view.
#[no_mangle]
pub extern "C" fn nr_page_overlay(view: usize, state: NrPageOverlayState) -> NrPageOverlay {
    use crate::page::layout::Overlay;
    let block = |index: i64| card_index(index);
    pages(|pages| {
        let Some(page) = pages.get_mut(&view) else {
            let none = || NrCardMarks {
                marks: std::ptr::null(),
                count: 0,
            };
            return NrPageOverlay {
                under: none(),
                over: none(),
            };
        };
        let (under, over) = page.layout.overlay(&Overlay {
            chosen: block(state.chosen_first).zip(block(state.chosen_last)),
            hovered: block(state.hovered),
            button: state.button,
            drop: block(state.drop).map(|drop| (drop, state.drop_after)),
            ringed: block(state.ringed),
            width: state.width,
        });
        let (under, over) = page.overlay.insert((lent_card(under), lent_card(over)));
        NrPageOverlay {
            under: handed(under),
            over: handed(over),
        }
    })
}

/// What a point of the open page is on (`page::layout::Hit`); each block or slot is -1 for none.
#[repr(C)]
pub struct NrPageHit {
    pub row: i64,
    pub button: u32,
    pub block: i64,
    pub todo: i64,
    pub slot: i64,
    pub cell: i64,
    pub media: i64,
    pub player: i64,
    pub below: bool,
}

/// What the point (`x`, `y`) of the open page as `view` last laid it out is on.
#[no_mangle]
pub extern "C" fn nr_page_hit(view: usize, x: f64, y: f64) -> NrPageHit {
    use crate::page::layout::Hit;
    let hit = pages(|pages| pages.get(&view).map(|page| page.layout.hit(x, y))).unwrap_or(Hit {
        row: -1,
        button: 0,
        block: -1,
        todo: -1,
        slot: -1,
        cell: -1,
        media: -1,
        player: -1,
        below: false,
    });
    NrPageHit {
        row: hit.row,
        button: hit.button,
        block: hit.block,
        todo: hit.todo,
        slot: hit.slot,
        cell: hit.cell,
        media: hit.media,
        player: hit.player,
        below: hit.below,
    }
}

/// Where blocks `first..=last` of `view`'s last layout would land if dropped with the pointer
/// at `y`: the block under it and whether in its lower half. `None` over no block or over the
/// dragged blocks.
pub(crate) fn page_drop_at(
    view: usize,
    y: f64,
    first: usize,
    last: usize,
) -> Option<(usize, bool)> {
    pages(|pages| {
        pages
            .get(&view)
            .and_then(|page| page.layout.drop_at(y, (first, last)))
    })
}

/// The size the picture file `id` is drawn at in a line `width` wide
/// (`page::media::picture_drawn_size`), in `drawn_width` and `drawn_height`; false when it is no
/// picture the platform reads.
///
/// # Safety
/// `id` must point to a NUL-terminated string; the two sizes to writable doubles.
#[no_mangle]
pub unsafe extern "C" fn nr_page_picture_size(
    id: *const c_char,
    width: f64,
    drawn_width: *mut f64,
    drawn_height: *mut f64,
) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    let Some(size) = with_columns(|columns| columns.workspace().path_of(&id).ok())
        .flatten()
        .and_then(|path| crate::media::picture_size_once(&path))
    else {
        return false;
    };
    let (shown_width, shown_height) = crate::page::media::picture_drawn_size(size, width);
    // SAFETY: the caller passes two writable doubles.
    unsafe {
        drawn_width.write(shown_width);
        drawn_height.write(shown_height);
    }
    true
}
