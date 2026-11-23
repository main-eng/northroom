//! The board: a folder's files as cards packed into columns. Where each card sits is worked
//! out here, so every shell lays the same board out the same way.

pub mod art;
pub mod card;
pub mod face;
pub mod ffi;
pub mod pointer;
pub mod view;

use crate::page::{BlockKind, Page};
use crate::strings::{self, key};
use crate::workspace::{EntryKind, FileRead, Media};

pub const MIN_COLUMN_COUNT: usize = 1;
pub const MAX_COLUMN_COUNT: usize = 8;
pub const DEFAULT_COLUMN_COUNT: usize = 6;
/// A card never gets narrower than this; a narrower board shows fewer columns.
const MIN_CARD_WIDTH: f64 = 160.0;
/// The space between cards, and between the cards and the board's left, right and top edges.
pub const GAP: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The most columns that fit a board `width` wide with every card at least the minimum width.
pub fn column_cap(width: f64) -> usize {
    (((width + GAP) / (MIN_CARD_WIDTH + GAP)).floor() as usize)
        .clamp(MIN_COLUMN_COUNT, MAX_COLUMN_COUNT)
}

/// The column count the board shows: the zoom's, or fewer when the board is too narrow for it.
pub fn visible_column_count(zoom: usize, width: f64) -> usize {
    zoom.clamp(MIN_COLUMN_COUNT, MAX_COLUMN_COUNT)
        .min(column_cap(width))
}

/// A column count from any number: rounded, then held between one and eight.
pub fn normalize_column_count(count: f64) -> usize {
    let finite = if count.is_finite() {
        count
    } else {
        DEFAULT_COLUMN_COUNT as f64
    };
    finite
        .round()
        .clamp(MIN_COLUMN_COUNT as f64, MAX_COLUMN_COUNT as f64) as usize
}

// Zoom: ⌘ or ctrl with the wheel changes how many columns the board shows.

/// How far the wheel travels, in pixels, for each column added or removed.
pub const ZOOM_WHEEL_THRESHOLD: f64 = 60.0;
const WHEEL_LINE_HEIGHT: f64 = 16.0;
const WHEEL_PAGE_HEIGHT: f64 = 100.0;
/// How a wheel event counts its travel: in pixels, lines or pages.
pub const WHEEL_DELTA_PIXEL: u32 = 0;
pub const WHEEL_DELTA_LINE: u32 = 1;
pub const WHEEL_DELTA_PAGE: u32 = 2;

/// Whether a wheel event zooms the board. Without ⌘ or ctrl the wheel scrolls it.
pub fn is_zoom_wheel(ctrl: bool, meta: bool, delta_y: f64) -> bool {
    (ctrl || meta) && delta_y != 0.0
}

/// A wheel event's travel in pixels, whatever it was counted in.
pub fn wheel_pixel_delta(delta_y: f64, delta_mode: u32) -> f64 {
    delta_y
        * match delta_mode {
            WHEEL_DELTA_LINE => WHEEL_LINE_HEIGHT,
            WHEEL_DELTA_PAGE => WHEEL_PAGE_HEIGHT,
            _ => 1.0,
        }
}

/// The ctrl-wheel travel, in pixels, that Chromium reports for a trackpad pinch: 100 times the
/// log of the pinch's growth, negated so a growing pinch zooms in.
pub fn pinch_wheel_delta(magnification: f64) -> f64 {
    -100.0 * magnification.ln_1p()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomWheel {
    pub column_count: usize,
    /// The travel not yet spent on a step, carried to the next wheel event.
    pub remainder: f64,
    pub stepped: bool,
}

/// Zooming out adds columns; zooming in removes them so each card grows. The wheel must travel
/// a whole threshold before the count changes.
pub fn resolve_zoom_wheel(current: usize, accumulated: f64) -> ZoomWheel {
    let column_count = current.clamp(MIN_COLUMN_COUNT, MAX_COLUMN_COUNT);
    let steps = (accumulated.abs() / ZOOM_WHEEL_THRESHOLD).floor();
    if steps == 0.0 {
        return ZoomWheel {
            column_count,
            remainder: accumulated,
            stepped: false,
        };
    }
    let direction = accumulated.signum();
    ZoomWheel {
        column_count: normalize_column_count(column_count as f64 + direction * steps),
        remainder: accumulated - direction * steps * ZOOM_WHEEL_THRESHOLD,
        stepped: true,
    }
}

/// One wheel event of a zoom gesture over a board `width` wide whose zoom is `zoom`.
/// `pending` is the gesture's travel so far; turning the wheel the other way starts it again.
/// Steps from the count on screen, so a narrow board still answers. Returns the new zoom when
/// the wheel travelled far enough for a step, even one that the limits held in place.
pub fn zoom_wheel(pending: &mut f64, zoom: usize, width: f64, pixel_delta: f64) -> Option<usize> {
    if *pending != 0.0 && pixel_delta.signum() != pending.signum() {
        *pending = 0.0;
    }
    *pending += pixel_delta;
    let cap = column_cap(inner_width(width));
    let resolved = resolve_zoom_wheel(
        zoom.clamp(MIN_COLUMN_COUNT, MAX_COLUMN_COUNT).min(cap),
        *pending,
    );
    *pending = resolved.remainder;
    resolved.stepped.then(|| resolved.column_count.min(cap))
}

/// What the board says while it zooms, in `language`: `1 column`, `6 columns`.
pub fn zoom_label(column_count: usize, language: &str) -> String {
    let message = if column_count == 1 {
        key::BOARD_ZOOM_COLUMN
    } else {
        key::BOARD_ZOOM_COLUMNS
    };
    strings::format(language, message, &[("count", &column_count.to_string())])
}

/// A card's width over its height. Folders and terminals are square, since a folder holds media
/// of every shape. A page is a sheet; anything else is a video frame.
pub fn aspect_ratio(kind: EntryKind) -> f64 {
    match kind {
        EntryKind::Folder | EntryKind::Terminal => 1.0,
        EntryKind::Page => 4.0 / 5.0,
        EntryKind::Image | EntryKind::Video | EntryKind::Audio | EntryKind::File => 16.0 / 9.0,
    }
}

/// The unit every card measurement is a multiple of, so a card reads the same at any size.
/// It is 0.347222% of the card width, clamped to 0.5–3 px.
pub fn card_unit(card_width: f64) -> f64 {
    (card_width * CARD_UNIT_PER_WIDTH).clamp(0.5, 3.0)
}

/// A card's unit for each point of its width (`card_unit`).
const CARD_UNIT_PER_WIDTH: f64 = 0.003_472_22;

/// The width the cards share: the board's, less `GAP` at its left and right edges.
fn inner_width(width: f64) -> f64 {
    (width - 2.0 * GAP).max(0.0)
}

/// How many columns a board `width` wide shows at `zoom`, and how wide each card is.
pub fn columns(width: f64, zoom: usize) -> (usize, f64) {
    let inner = inner_width(width);
    let count = visible_column_count(zoom, inner);
    (count, (inner - GAP * (count - 1) as f64) / count as f64)
}

/// The shortest column so far and where it ends; the leftmost of equals.
fn shortest(bottoms: &[f64]) -> (usize, f64) {
    let top = bottoms.iter().copied().fold(f64::INFINITY, f64::min);
    (
        bottoms
            .iter()
            .position(|bottom| *bottom == top)
            .unwrap_or(0),
        top,
    )
}

/// The columns being packed: each card goes into the shortest so far.
struct Packing {
    card_width: f64,
    bottoms: Vec<f64>,
}

impl Packing {
    fn new(width: f64, zoom: usize) -> Self {
        let (count, card_width) = columns(width, zoom);
        Self {
            card_width,
            bottoms: vec![0.0; count],
        }
    }

    fn place(&mut self, height: f64) -> Frame {
        let (column, top) = shortest(&self.bottoms);
        self.bottoms[column] = top + height + GAP;
        Frame {
            x: GAP + column as f64 * (self.card_width + GAP),
            y: GAP + top,
            width: self.card_width,
            height,
        }
    }

    /// The height of the packed cards, from the board's top edge.
    fn height(&self) -> f64 {
        let tallest = self.bottoms.iter().copied().fold(0.0, f64::max);
        GAP + (tallest - GAP).max(0.0)
    }
}

/// Packs the cards in order, each into the shortest column so far. `width` is the board's
/// whole width; the cards sit `GAP` inside its left, right and top edges. Returns each card's
/// frame and the height of the packed cards.
pub fn layout(kinds: &[EntryKind], width: f64, zoom: usize) -> (Vec<Frame>, f64) {
    layout_shapes(&shapes(kinds), width, zoom)
}

/// Each card's width over its height.
pub fn shapes(kinds: &[EntryKind]) -> Vec<f64> {
    kinds.iter().map(|kind| aspect_ratio(*kind)).collect()
}

/// `layout` for cards given by their shape (width over height). Tiles for outputs still being
/// made take their final shape, so they pack with the folder's cards.
pub fn layout_shapes(shapes: &[f64], width: f64, zoom: usize) -> (Vec<Frame>, f64) {
    let mut packing = Packing::new(width, zoom);
    let card_width = packing.card_width;
    let frames = shapes
        .iter()
        .map(|shape| packing.place(card_width / shape))
        .collect();
    (frames, packing.height())
}

// A page card previews the page's first blocks.

/// Past this the card is full anyway; the rest is not drawn.
pub const MAX_PREVIEW_BLOCKS: usize = 40;

/// What a page card shows of the page in the file named `name` (without `.md`) holding
/// `text`: its first blocks, with their inline segments and list numbers. `None` when the
/// page has nothing to show besides its title, and the card draws a page icon instead.
pub fn page_preview(name: &str, text: &str, language: &str) -> Option<Page> {
    let mut next_id = 0_u32;
    let mut page = Page::from_file(name, text, language, &mut || {
        next_id += 1;
        next_id.to_string()
    });
    // Cards draw no players; a page's `<video>` or `<audio>` shows as text.
    for block in &mut page.blocks {
        if let BlockKind::Player { html } = &block.kind {
            block.kind = BlockKind::Paragraph { text: html.clone() };
        }
    }
    let has_content = page.blocks.iter().any(|block| match &block.kind {
        BlockKind::Divider => false,
        BlockKind::Table { .. } | BlockKind::Media { .. } | BlockKind::Player { .. } => true,
        BlockKind::Paragraph { text }
        | BlockKind::Heading { text, .. }
        | BlockKind::Bullet { text, .. }
        | BlockKind::Numbered { text, .. }
        | BlockKind::Todo { text, .. }
        | BlockKind::Quote { text }
        | BlockKind::Code { text, .. }
        | BlockKind::Html { text } => !text.trim().is_empty(),
    });
    if !has_content {
        return None;
    }
    page.blocks.truncate(MAX_PREVIEW_BLOCKS);
    page.derive();
    Some(page)
}

/// What the card of a text file that is not a page file shows under its header.
pub enum TextCard {
    /// The text as paragraphs, drawn as a page's preview is.
    Page(Page),
    /// The top of a code file, its lines and indentation kept, set in the monospace.
    Code(String),
    /// Nothing to show: the card draws its large glyph instead.
    Empty,
}

/// A code card draws the top of its file: the card is full well before either limit.
const CODE_PREVIEW_MAX_LINES: usize = 60;
/// Counted in UTF-16 code units.
const CODE_PREVIEW_MAX_UNITS: usize = 8_000;

/// The top of a code file as its card shows it; `None` when it is blank.
pub fn code_preview(text: &str) -> Option<String> {
    let mut units = 0;
    let head: String = text
        .chars()
        .take_while(|character| {
            units += character.len_utf16();
            units <= CODE_PREVIEW_MAX_UNITS
        })
        .collect();
    let preview = head
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .take(CODE_PREVIEW_MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    (!preview.trim().is_empty()).then_some(preview)
}

/// Text as the paragraphs of a page titled `title`: a blank line ends a paragraph, single
/// line breaks stay inside it.
pub fn plain_text_page(title: &str, text: &str) -> Page {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    // A break is a line end, any other white space, and a line end: `/\n[^\S\n]*\n/u`.
    let mut paragraphs: Vec<&str> = Vec::new();
    let mut rest = normalized.as_str();
    'split: loop {
        let mut search = 0;
        while let Some(found) = rest[search..].find('\n') {
            let start = search + found;
            let after = &rest[start + 1..];
            let blank = after.len()
                - after
                    .trim_start_matches(|character: char| {
                        character.is_whitespace() && character != '\n'
                    })
                    .len();
            if after[blank..].starts_with('\n') {
                paragraphs.push(&rest[..start]);
                rest = &after[blank + 1..];
                continue 'split;
            }
            search = start + 1;
        }
        paragraphs.push(rest);
        break;
    }
    let blocks = paragraphs
        .into_iter()
        .filter(|paragraph| !paragraph.trim().is_empty())
        .enumerate()
        .map(|(index, paragraph)| crate::page::Block {
            id: (index + 1).to_string(),
            kind: BlockKind::Paragraph {
                text: paragraph.to_owned(),
            },
        })
        .collect();
    let mut page = Page {
        title: title.to_owned(),
        blocks,
        states_title: false,
        segments: Vec::new(),
        numbers: Vec::new(),
        source: crate::page::PageSource::default(),
    };
    page.derive();
    page
}

/// What the card of the text file named `title` holding `text` shows: a code file (`code`,
/// by its extension) its source, any other its text as paragraphs.
pub fn text_card(title: &str, text: &str, code: bool) -> TextCard {
    if code {
        return code_preview(text).map_or(TextCard::Empty, TextCard::Code);
    }
    let mut page = plain_text_page(title, text);
    if page.blocks.is_empty() {
        return TextCard::Empty;
    }
    page.blocks.truncate(MAX_PREVIEW_BLOCKS);
    page.derive();
    TextCard::Page(page)
}

/// What a card shows, chosen from the file's kind and what the drive read of it
/// (`workspace::read_file`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardFace {
    /// Its kind's own face: a folder's or a timeline's art, a terminal's mark, a picture, a
    /// video's first frame, or a page file's page.
    Own,
    /// A page's face over the text of a file that is not a page file, as paragraphs.
    PageText,
    /// A page's face over the top of a code file's source, or of a database's first table.
    PageCode,
    /// The file's glyph (code's for a code file) over its name. Used for a sound, an HTML page
    /// and a page file that cannot be read as one.
    File,
    /// The system thumbnail, covering the card from its top edge, for a PDF, a workbook and
    /// anything not read as text (`FileRead::shows_system_thumbnail`). Shows the glyph and name
    /// while loading and where the system has no thumbnail.
    SystemPicture,
    /// A 3D model's still on the studio grey. Shows the glyph and name until the still is drawn,
    /// or if it cannot be.
    Model,
}

/// The face of a card for a file of `kind`, a code file by its name (`code`), that the drive
/// read as `read` (`None` for anything not read: a folder, a terminal, media).
pub fn card_face(kind: EntryKind, code: bool, read: Option<&FileRead>) -> CardFace {
    match kind {
        EntryKind::Folder | EntryKind::Terminal | EntryKind::Image | EntryKind::Video => {
            CardFace::Own
        }
        // A sound is not read for its card.
        EntryKind::Audio => CardFace::File,
        // A Markdown file is a page when it reads as text; past 1 MB, or not UTF-8, it is a file.
        EntryKind::Page => match read {
            Some(FileRead::Text(_)) => CardFace::Own,
            _ => CardFace::File,
        },
        EntryKind::File => match read {
            Some(FileRead::Text(_)) if code => CardFace::PageCode,
            Some(FileRead::Text(_)) => CardFace::PageText,
            // A database with nothing to show (no table, or not read) is its glyph and name.
            Some(FileRead::Database(top)) if !top.is_empty() => CardFace::PageCode,
            Some(FileRead::Media(Media::Model)) => CardFace::Model,
            Some(read) if read.shows_system_thumbnail() => CardFace::SystemPicture,
            _ => CardFace::File,
        },
    }
}

/// The tallest shape a picture's or a video's card takes, width over height: a phone's upright
/// frame. A taller picture is shown whole inside it (`shown_whole`).
pub const TALLEST_PICTURE: f64 = 9.0 / 16.0;
/// The widest: a cinema's frame. A wider picture is shown whole inside it.
pub const WIDEST_PICTURE: f64 = 21.0 / 9.0;

/// Whether a picture or video `pixels` big sits whole on the card's ground instead of filling
/// it. True when it is taller than `TALLEST_PICTURE` or wider than `WIDEST_PICTURE`; filling
/// would crop a banner to its middle.
pub fn shown_whole(pixels: Option<(u32, u32)>) -> bool {
    pixels
        .filter(|(width, height)| *width > 0 && *height > 0)
        .is_some_and(|(width, height)| {
            let shape = f64::from(width) / f64::from(height);
            !(TALLEST_PICTURE..=WIDEST_PICTURE).contains(&shape)
        })
}

/// A card's width over its height: a sheet for a page's face, a frame of video for a file's
/// glyph, a square for a model's still (drawn square), a picture's or a video's own shape
/// (`pixels`, its width and height, when known) between `TALLEST_PICTURE` and
/// `WIDEST_PICTURE`, and its kind's own shape otherwise.
pub fn card_shape(kind: EntryKind, face: CardFace, pixels: Option<(u32, u32)>) -> f64 {
    match face {
        CardFace::PageText | CardFace::PageCode => aspect_ratio(EntryKind::Page),
        CardFace::File | CardFace::SystemPicture => aspect_ratio(EntryKind::File),
        CardFace::Model => 1.0,
        CardFace::Own => match (kind, pixels) {
            (EntryKind::Image | EntryKind::Video, Some((width, height)))
                if width > 0 && height > 0 =>
            {
                (f64::from(width) / f64::from(height)).clamp(TALLEST_PICTURE, WIDEST_PICTURE)
            }
            _ => aspect_ratio(kind),
        },
    }
}

/// How many evenly spaced moments skimming a video card shows.
pub const SKIM_STEPS: u32 = 12;

/// The skim step for the pointer `across` a card (0 at its left edge, 1 at its right). Snapping
/// to `SKIM_STEPS` lets the frames drawn for one pass serve the next.
pub fn skim_step(across: f64) -> u32 {
    let steps = SKIM_STEPS;
    ((across.clamp(0.0, 1.0) * f64::from(steps)) as u32).min(steps - 1)
}

/// The fraction of the video to show for the pointer `across` a card, at the middle of its
/// `skim_step`.
pub fn skim_fraction(across: f64) -> f64 {
    (f64::from(skim_step(across)) + 0.5) / f64::from(SKIM_STEPS)
}

/// The prefix of the line under a terminal card's name: the agent's label and a dot, or empty
/// when there is no agent or the name already says it. The activity follows.
pub fn terminal_card_lead(agent: Option<crate::terminal::agent::Agent>, title: &str) -> String {
    match agent {
        Some(agent) if agent.label() != title => format!("{} · ", agent.label()),
        _ => String::new(),
    }
}

// A folder's art: three sheets fanned inside it and a flap tipped open, which lift when the
// pointer is over the card and open wider when a drag is.

/// One sheet of a folder's art: its offset in hundredths of its own width and height, and its
/// turn in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetPose {
    pub x: f64,
    pub y: f64,
    pub turn: f64,
}

/// How a folder's art stands: each sheet, right sheet first, and how far the flap is tipped
/// open in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FolderPose {
    pub sheets: [SheetPose; 3],
    pub flap: f64,
}

/// The pose a folder's art moves towards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderTarget {
    Rest,
    /// The pointer is over the folder's card: the sheets lift and the flap tips open.
    Hover,
    /// A drag is over the card: the folder opens wider to take the drop.
    Drop,
}

const fn sheet(x: f64, y: f64, turn: f64) -> SheetPose {
    SheetPose { x, y, turn }
}

impl FolderTarget {
    pub fn pose(self) -> FolderPose {
        match self {
            Self::Rest => FolderPose {
                sheets: [
                    sheet(24.4, -4.7, 10.0),
                    sheet(1.8, -9.3, 2.0),
                    sheet(-24.4, -10.3, -5.0),
                ],
                flap: 15.0,
            },
            Self::Hover => FolderPose {
                sheets: [
                    sheet(24.4, -14.0, 14.0),
                    sheet(1.8, -16.4, -1.0),
                    sheet(-24.4, -20.6, -9.0),
                ],
                flap: 45.0,
            },
            Self::Drop => FolderPose {
                sheets: [
                    sheet(34.0, -30.0, 18.0),
                    sheet(0.0, -34.0, -3.0),
                    sheet(-32.0, -32.0, -14.0),
                ],
                flap: 55.0,
            },
        }
    }
}

/// How long a folder's art takes to reach its pose, the latest sheet's delay included.
pub const FOLDER_MOTION_SECONDS: f64 = 0.68;
const FOLDER_SHEETS_SECONDS: f64 = 0.56;
const FOLDER_FLAP_SECONDS: f64 = 0.52;
/// When the pointer arrives the right and middle sheets start late.
const FOLDER_HOVER_DELAYS: [f64; 3] = [0.12, 0.06, 0.0];

/// A folder's art `elapsed` seconds after it set off from `from` towards `target`: the sheets
/// over 560 ms and the flap over 520 ms, eased to a stop without passing it.
pub fn folder_pose(from: FolderPose, target: FolderTarget, elapsed: f64) -> FolderPose {
    use crate::motion::Curve;
    let to = target.pose();
    let delays = if target == FolderTarget::Hover {
        FOLDER_HOVER_DELAYS
    } else {
        [0.0; 3]
    };
    let between = |a: f64, b: f64, done: f64| a + (b - a) * done;
    let mut pose = to;
    for (index, delay) in delays.into_iter().enumerate() {
        let done = Curve::Standard.at((elapsed - delay) / FOLDER_SHEETS_SECONDS);
        let (a, b) = (from.sheets[index], to.sheets[index]);
        pose.sheets[index] = sheet(
            between(a.x, b.x, done),
            between(a.y, b.y, done),
            between(a.turn, b.turn, done),
        );
    }
    pose.flap = between(
        from.flap,
        to.flap,
        Curve::Standard.at(elapsed / FOLDER_FLAP_SECONDS),
    );
    pose
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinch_zooms_as_a_ctrl_wheel_of_its_logarithm() {
        assert_eq!(pinch_wheel_delta(0.0), 0.0);
        assert!((pinch_wheel_delta(0.5) - -100.0 * 1.5f64.ln()).abs() < 1e-12);
        assert!(
            pinch_wheel_delta(-0.2) > 0.0,
            "a closing pinch travels towards the person and zooms out"
        );
        assert!(is_zoom_wheel(true, false, pinch_wheel_delta(0.1)));
    }

    #[test]
    fn a_terminal_card_names_its_agent_unless_its_name_does() {
        use crate::terminal::agent::Agent;
        assert_eq!(
            terminal_card_lead(Some(Agent::Codex), "Refactor"),
            "Codex · "
        );
        assert_eq!(terminal_card_lead(Some(Agent::Claude), "Claude Code"), "");
        assert_eq!(terminal_card_lead(None, "Build"), "");
    }

    #[test]
    fn a_folders_art_eases_to_its_pose_the_right_sheet_last_on_hover() {
        let rest = FolderTarget::Rest.pose();
        assert_eq!(
            folder_pose(rest, FolderTarget::Hover, 0.0),
            FolderPose {
                sheets: rest.sheets,
                flap: rest.flap
            }
        );
        assert_eq!(
            folder_pose(rest, FolderTarget::Hover, FOLDER_MOTION_SECONDS),
            FolderTarget::Hover.pose()
        );
        assert_eq!(
            folder_pose(rest, FolderTarget::Drop, 0.6),
            FolderTarget::Drop.pose()
        );
        // 60 ms in, only the left sheet has started towards the hover; the flap moves at once.
        let early = folder_pose(rest, FolderTarget::Hover, 0.06);
        assert_eq!(early.sheets[0], rest.sheets[0]);
        assert_eq!(early.sheets[1], rest.sheets[1]);
        assert!(early.sheets[2].y < rest.sheets[2].y && early.flap > rest.flap);
        // It never passes its pose on the way.
        let hover = FolderTarget::Hover.pose();
        for step in 0..=68 {
            let pose = folder_pose(rest, FolderTarget::Hover, f64::from(step) / 100.0);
            assert!(
                pose.flap <= hover.flap,
                "the flap passed its pose at {step}"
            );
        }
    }

    #[test]
    fn plain_text_reads_as_paragraphs_split_at_blank_lines() {
        let texts = |text: &str| -> Vec<String> {
            plain_text_page("t", text)
                .blocks
                .iter()
                .map(|block| match &block.kind {
                    BlockKind::Paragraph { text } => text.clone(),
                    _ => panic!("only paragraphs"),
                })
                .collect()
        };
        assert_eq!(texts("plain text\n"), ["plain text\n"]);
        assert_eq!(texts("one\ntwo\n\nthree"), ["one\ntwo", "three"]);
        assert_eq!(texts("one\r\n \t\r\ntwo"), ["one", "two"]);
        // Three line ends: the first two are the break and the third stays with what follows.
        assert_eq!(texts("one\n\n\ntwo"), ["one", "\ntwo"]);
        assert_eq!(texts("one\n\n\n\ntwo"), ["one", "two"]);
        assert!(texts("  \n\n \n").is_empty());
        assert!(texts("").is_empty());
    }

    #[test]
    fn a_text_card_shows_paragraphs_or_source_or_nothing() {
        assert!(
            matches!(text_card("a.txt", "plain text\n", false), TextCard::Page(page) if page.blocks.len() == 1 && page.segments[0][0].text.starts_with("plain text"))
        );
        assert!(matches!(text_card("a.txt", " \n", false), TextCard::Empty));
        assert!(matches!(text_card("a.py", " \n", true), TextCard::Empty));
        assert!(
            matches!(text_card("a.py", "def f():\r\n\treturn 1\n", true), TextCard::Code(code) if code == "def f():\n\treturn 1\n")
        );
        let long = (0..100)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let Some(shown) = code_preview(&long) else {
            panic!("a preview")
        };
        assert_eq!(shown.split('\n').count(), 60);
        assert_eq!(
            code_preview(&"a".repeat(9_000)).map(|shown| shown.len()),
            Some(8_000)
        );
        let many = (0..60)
            .map(|index| format!("p{index}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        assert!(
            matches!(text_card("a.txt", &many, false), TextCard::Page(page) if page.blocks.len() == MAX_PREVIEW_BLOCKS)
        );
    }

    #[test]
    fn a_text_file_takes_a_pages_shape() {
        assert_eq!(
            card_shape(EntryKind::File, CardFace::PageText, None),
            4.0 / 5.0
        );
        assert_eq!(
            card_shape(EntryKind::File, CardFace::PageCode, None),
            4.0 / 5.0
        );
        assert_eq!(
            card_shape(EntryKind::File, CardFace::File, None),
            16.0 / 9.0
        );
        assert_eq!(
            card_shape(EntryKind::File, CardFace::Model, None),
            1.0,
            "a model's still is square"
        );
        // A file the system draws keeps its shape whether a thumbnail comes or not.
        assert_eq!(
            card_shape(EntryKind::File, CardFace::SystemPicture, None),
            16.0 / 9.0
        );
        assert_eq!(card_shape(EntryKind::Page, CardFace::Own, None), 4.0 / 5.0);
        assert_eq!(
            card_shape(EntryKind::Page, CardFace::File, None),
            16.0 / 9.0
        );
        assert_eq!(
            card_shape(EntryKind::Image, CardFace::Own, None),
            16.0 / 9.0
        );
        assert_eq!(card_shape(EntryKind::Folder, CardFace::Own, None), 1.0);
    }

    #[test]
    fn a_picture_or_a_video_takes_its_own_shape_within_bounds() {
        let shape = |kind, pixels| card_shape(kind, CardFace::Own, Some(pixels));
        assert_eq!(
            shape(EntryKind::Image, (1200, 1600)),
            3.0 / 4.0,
            "an upright photo"
        );
        assert_eq!(
            shape(EntryKind::Video, (1080, 1920)),
            9.0 / 16.0,
            "a phone's video"
        );
        assert_eq!(shape(EntryKind::Image, (1000, 1000)), 1.0);
        assert_eq!(
            shape(EntryKind::Image, (400, 4000)),
            TALLEST_PICTURE,
            "a strip's card stops"
        );
        assert_eq!(
            shape(EntryKind::Image, (640, 40)),
            WIDEST_PICTURE,
            "so does a banner's"
        );
        assert!(shown_whole(Some((640, 40))) && shown_whole(Some((400, 4000))));
        assert!(!shown_whole(Some((1920, 1080))) && !shown_whole(Some((1080, 1920))));
        assert!(
            !shown_whole(Some((2100, 900))),
            "a cinema's frame fills its card"
        );
        assert!(!shown_whole(None) && !shown_whole(Some((0, 0))));
        assert_eq!(
            shape(EntryKind::Image, (0, 0)),
            16.0 / 9.0,
            "a size the file does not say"
        );
        assert_eq!(
            card_shape(
                EntryKind::Image,
                CardFace::SystemPicture,
                Some((1000, 1000))
            ),
            16.0 / 9.0,
            "a face other than its own picture keeps its own shape"
        );
    }

    /// Every kind of file on a board card, written to disk and read as the board reads it. A
    /// PDF, an HTML page or a workbook made only of plain ASCII is never a page of its source
    /// text.
    #[test]
    fn every_kind_of_file_has_its_card() {
        let folder = tempfile::tempdir().expect("folder");
        let face = |relative: &str, bytes: &[u8]| {
            let path = folder.path().join(relative);
            std::fs::write(&path, bytes).expect("write");
            let name = path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .to_string();
            let extension = name
                .rsplit_once('.')
                .map(|(_, extension)| extension.to_ascii_lowercase())
                .unwrap_or_default();
            let kind = match extension.as_str() {
                "md" => EntryKind::Page,
                _ => match crate::workspace::media_of_extension(&extension) {
                    Some(Media::Image) => EntryKind::Image,
                    Some(Media::Video) => EntryKind::Video,
                    Some(Media::Audio) => EntryKind::Audio,
                    Some(Media::Model) | None => EntryKind::File,
                },
            };
            let read = crate::workspace::read_file(&path);
            card_face(
                kind,
                crate::workspace::is_code_extension(&extension),
                Some(&read),
            )
        };
        // file-kinds.py's PDF: every byte of it ASCII, no NUL.
        let pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\nBT /F1 24 Tf 72 700 Td (A page of a PDF) Tj ET\n%%EOF\n";
        assert_eq!(face("Brief.pdf", pdf), CardFace::SystemPicture);
        assert_eq!(face("Brief.PDF", pdf), CardFace::SystemPicture);
        assert_eq!(
            face("Card.html", b"<!doctype html><h1>An HTML file</h1>\n"),
            CardFace::File
        );
        assert_eq!(face("Card.htm", b"<p>old</p>\n"), CardFace::File);
        assert_eq!(
            face("Budget.xlsx", b"PK plain enough"),
            CardFace::SystemPicture
        );
        assert_eq!(face("Budget.xlsm", b"plain"), CardFace::SystemPicture);
        assert_eq!(face("Model.glb", b"glTF"), CardFace::Model);
        assert_eq!(face("tone.wav", b"RIFF"), CardFace::File);
        assert_eq!(face("still.png", b"plain"), CardFace::Own);
        assert_eq!(face("vector.svg", b"<svg/>"), CardFace::Own);
        assert_eq!(face("clip.mp4", b"plain"), CardFace::Own);
        // A still the platform decodes shows itself: an iPhone's HEIC on a Mac, and on Windows
        // with its extensions; the system's thumbnail, if any, where nothing decodes it.
        let heic = if crate::workspace::phone_still_decodes_here("heic") {
            CardFace::Own
        } else {
            CardFace::SystemPicture
        };
        assert_eq!(face("photo.heic", b"ftyp\0heic"), heic);
        assert_eq!(face("scan.tiff", b"II*\0"), CardFace::Own);
        assert_eq!(
            face("types.ts", b"export const a = 1;\n"),
            CardFace::PageCode
        );
        assert_eq!(face("main.rs", b"fn main() {}\n"), CardFace::PageCode);
        assert_eq!(
            face("data.csv", b"Name,Value\nalpha,1\n"),
            CardFace::PageText
        );
        assert_eq!(face("notes.txt", b"plain text\n"), CardFace::PageText);
        assert_eq!(face("README", b"First paragraph.\n"), CardFace::PageText);
        assert_eq!(face("Plan.md", b"# Plan\n"), CardFace::Own);
        assert_eq!(face("Latin.md", &[0xe9, b'a']), CardFace::File);
        assert_eq!(
            face("archive.zip", b"PK\x03\x04\0\0binary"),
            CardFace::SystemPicture
        );
        assert_eq!(
            face("latin.dat", &[0xe9, 0x74, 0xe9]),
            CardFace::SystemPicture
        );
        assert_eq!(card_face(EntryKind::Folder, false, None), CardFace::Own);
        assert_eq!(card_face(EntryKind::Terminal, false, None), CardFace::Own);
        // A database shows the top of its first table; one with nothing to show is its glyph.
        let top = FileRead::Database("name  age\nAda   36".to_owned());
        assert_eq!(
            card_face(EntryKind::File, false, Some(&top)),
            CardFace::PageCode
        );
        assert_eq!(
            card_face(
                EntryKind::File,
                false,
                Some(&FileRead::Database(String::new()))
            ),
            CardFace::File
        );
    }

    #[test]
    fn skimming_shows_one_of_a_few_moments_each_from_the_middle_of_its_step() {
        let steps = SKIM_STEPS;
        assert_eq!(skim_step(0.0), 0);
        assert_eq!(skim_step(-0.5), 0, "left of the card is its first moment");
        assert_eq!(skim_step(1.0), steps - 1, "its right edge is its last");
        assert_eq!(skim_step(2.0), steps - 1);
        assert_eq!(skim_fraction(0.0), 0.5 / f64::from(steps));
        assert_eq!(skim_fraction(1.0), 1.0 - 0.5 / f64::from(steps));
        let across = 0.3;
        assert!(
            (skim_fraction(across) - across).abs() <= 0.5 / f64::from(steps),
            "a moment is never more than half a step from the pointer"
        );
    }

    #[test]
    fn a_narrow_board_shows_fewer_columns() {
        assert_eq!(column_cap(1272.0), 7);
        assert_eq!(visible_column_count(6, 1272.0), 6);
        assert_eq!(visible_column_count(6, 500.0), 3);
        assert_eq!(visible_column_count(0, 100.0), 1);
    }

    #[test]
    fn cards_fill_the_shortest_column_in_order() {
        use EntryKind::{Folder, Page, Video};
        // 3 columns of 200: 8 inset + 2 gaps of 4 + 600.
        let (frames, height) = layout(&[Folder, Page, Video, Video, Folder], 616.0, 3);
        assert_eq!(
            frames[0],
            Frame {
                x: 4.0,
                y: 4.0,
                width: 200.0,
                height: 200.0
            }
        );
        assert_eq!(
            frames[1],
            Frame {
                x: 208.0,
                y: 4.0,
                width: 200.0,
                height: 250.0
            }
        );
        assert_eq!((frames[2].x, frames[2].height), (412.0, 112.5));
        // The video column is the shortest, so the next card goes under it.
        assert_eq!((frames[3].x, frames[3].y), (412.0, 4.0 + 112.5 + 4.0));
        assert_eq!((frames[4].x, frames[4].y), (4.0, 208.0));
        assert_eq!(height, 4.0 + 200.0 + 4.0 + 200.0);
    }

    #[test]
    fn the_zoom_is_a_whole_count_from_one_to_eight() {
        assert_eq!(
            (MIN_COLUMN_COUNT, MAX_COLUMN_COUNT, DEFAULT_COLUMN_COUNT),
            (1, 8, 6)
        );
        assert_eq!(normalize_column_count(0.0), 1);
        assert_eq!(normalize_column_count(4.6), 5);
        assert_eq!(normalize_column_count(99.0), 8);
        assert_eq!(normalize_column_count(f64::NAN), DEFAULT_COLUMN_COUNT);
    }

    const STEP: f64 = ZOOM_WHEEL_THRESHOLD;

    fn wheel(column_count: usize, remainder: f64, stepped: bool) -> ZoomWheel {
        ZoomWheel {
            column_count,
            remainder,
            stepped,
        }
    }

    #[test]
    fn the_wheel_travels_a_threshold_before_the_count_changes() {
        assert_eq!(
            resolve_zoom_wheel(4, STEP - 1.0),
            wheel(4, STEP - 1.0, false)
        );
        assert_eq!(resolve_zoom_wheel(4, STEP), wheel(5, 0.0, true));
        assert_eq!(resolve_zoom_wheel(4, -STEP), wheel(3, 0.0, true));
    }

    #[test]
    fn a_long_wheel_takes_several_steps_and_keeps_what_is_left() {
        assert_eq!(
            resolve_zoom_wheel(4, 2.0 * STEP + 15.0),
            wheel(6, 15.0, true)
        );
        assert_eq!(
            resolve_zoom_wheel(5, -(2.0 * STEP + 15.0)),
            wheel(3, -15.0, true)
        );
    }

    #[test]
    fn the_zoom_stays_within_its_limits_and_still_answers() {
        assert_eq!(
            resolve_zoom_wheel(MIN_COLUMN_COUNT, -STEP),
            wheel(MIN_COLUMN_COUNT, 0.0, true)
        );
        assert_eq!(
            resolve_zoom_wheel(MAX_COLUMN_COUNT, STEP),
            wheel(MAX_COLUMN_COUNT, 0.0, true)
        );
    }

    #[test]
    fn a_plain_wheel_scrolls_the_board() {
        assert!(is_zoom_wheel(true, false, 1.0));
        assert!(is_zoom_wheel(false, true, -1.0));
        assert!(!is_zoom_wheel(false, false, 1.0));
        assert!(!is_zoom_wheel(true, false, 0.0));
    }

    #[test]
    fn line_and_page_wheels_count_in_pixels() {
        assert_eq!(wheel_pixel_delta(2.0, WHEEL_DELTA_PIXEL), 2.0);
        assert_eq!(wheel_pixel_delta(2.0, WHEEL_DELTA_LINE), 32.0);
        assert_eq!(wheel_pixel_delta(2.0, WHEEL_DELTA_PAGE), 200.0);
    }

    #[test]
    fn a_narrower_board_fits_fewer_columns() {
        assert_eq!(column_cap(1400.0), 8);
        assert_eq!(column_cap(540.0), 3);
        assert_eq!(column_cap(330.0), 2);
        assert_eq!(column_cap(100.0), MIN_COLUMN_COUNT);
        assert_eq!(visible_column_count(4, 1400.0), 4);
        assert_eq!(visible_column_count(6, 540.0), 3);
    }

    #[test]
    fn a_gesture_adds_its_travel_up_and_starts_again_when_it_turns() {
        let mut pending = 0.0;
        // Wide enough for eight columns.
        assert_eq!(zoom_wheel(&mut pending, 6, 1400.0, 40.0), None);
        assert_eq!(zoom_wheel(&mut pending, 6, 1400.0, 40.0), Some(7));
        assert_eq!(pending, 20.0);
        // Turning back drops the 20 left over: 59 the other way is not yet a step.
        assert_eq!(zoom_wheel(&mut pending, 7, 1400.0, -59.0), None);
        assert_eq!(zoom_wheel(&mut pending, 7, 1400.0, -1.0), Some(6));
        // A board with room for three steps from the three on screen, not from the zoom's six,
        // and never past what fits.
        assert_eq!(zoom_wheel(&mut pending, 6, 540.0, -60.0), Some(2));
        assert_eq!(zoom_wheel(&mut pending, 6, 540.0, 180.0), Some(3));
        assert_eq!(
            (zoom_label(1, "en"), zoom_label(6, "en")),
            ("1 column".to_owned(), "6 columns".to_owned())
        );
    }

    #[test]
    fn a_page_card_previews_the_pages_first_blocks() {
        use crate::page::inline::Mark;
        let page = page_preview(
            "Plan",
            "# Plan\n\n## Goals\n\nShip the **native** board.\n\n1. One\n2. Two\n\n- [x] Done\n",
            "en",
        )
        .unwrap();
        assert_eq!(page.blocks.len(), 5);
        assert!(
            matches!(&page.blocks[0].kind, BlockKind::Heading { level: 2, text } if text == "Goals")
        );
        // Marks are found, not left in the words.
        let words: Vec<(&str, bool)> = page.segments[1]
            .iter()
            .map(|segment| (segment.text.as_str(), segment.marks.contains(&Mark::Bold)))
            .collect();
        assert_eq!(
            words,
            [("Ship the ", false), ("native", true), (" board.", false)]
        );
        assert_eq!(page.numbers[2..4], [Some(1), Some(2)]);
        assert!(matches!(
            page.blocks[4].kind,
            BlockKind::Todo { checked: true, .. }
        ));
        // Nothing but a title, blank lines or a rule is nothing to preview.
        assert!(page_preview("Empty", "# Empty\n\n---\n", "en").is_none());
        assert!(page_preview("Blank", "", "en").is_none());
        let long: String = (0..60).map(|line| format!("Line {line}\n\n")).collect();
        assert_eq!(
            page_preview("Long", &long, "en").unwrap().blocks.len(),
            MAX_PREVIEW_BLOCKS
        );
    }

    #[test]
    fn a_cards_unit_follows_its_width_within_limits() {
        assert_eq!(card_unit(100.0), 0.5);
        assert!((card_unit(288.0) - 1.0).abs() < 1e-4);
        assert_eq!(card_unit(2000.0), 3.0);
    }
}
