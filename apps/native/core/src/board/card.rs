//! The marks a shell draws a board card with (`Mark`), and a page card's face in them: a header
//! with the page's glyph and name, and the page's first blocks under it, faded out at the card's
//! bottom edge. Every other face is `board::face`'s.
//!
//! Where words wrap is the typesetter's to say (`typeset`): the card asks a `Measure` how many
//! lines a set of words takes at a width, and stacks the blocks by the answers.

use crate::board::Frame;
use crate::glyph::Glyph;
use crate::page::inline::{Mark as InlineMark, Segment};
use crate::page::{BlockKind, Page};
use crate::theme::{Paint, Token};

/// The page glyph's blue, the same in every theme.
pub const PAGE_ACCENT: Paint = Paint::Fixed("#3f93f7");

/// A face at a size. The sans is Inter at `weight`; the monospace has one weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    pub size: f64,
    pub weight: f64,
    pub mono: bool,
    /// The upright face leaned 0.2 of its height forward: the app loads no italic, so the
    /// upright one is slanted.
    pub slanted: bool,
    /// Its digits all one width (`font-variant-numeric: tabular-nums`), so a number keeps its
    /// width as it changes.
    pub tabular: bool,
}

impl Font {
    /// The named weights the app's faces use, as CSS numbers them.
    pub const REGULAR: f64 = 400.0;
    pub const MEDIUM: f64 = 500.0;
    pub const SEMIBOLD: f64 = 600.0;
    pub const BOLD: f64 = 700.0;

    pub const fn sans(size: f64, weight: f64) -> Self {
        Self {
            size,
            weight,
            mono: false,
            slanted: false,
            tabular: false,
        }
    }

    pub const fn mono(size: f64) -> Self {
        Self {
            size,
            weight: Self::REGULAR,
            mono: true,
            slanted: false,
            tabular: false,
        }
    }

    /// The face with its digits all one width.
    pub const fn tabular(self) -> Self {
        Self {
            tabular: true,
            ..self
        }
    }
}

/// A colour at a share of its own opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    pub paint: Paint,
    pub opacity: f64,
}

impl Ink {
    pub fn of(paint: impl Into<Paint>) -> Self {
        Self {
            paint: paint.into(),
            opacity: 1.0,
        }
    }
}

/// How a stretch of words is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub font: Font,
    pub color: Token,
    /// A line through the words, a third of the font's height up.
    pub strike: bool,
    /// Inline code: a rounded ground behind it.
    pub code: bool,
    /// A line under the words, as a link has.
    pub underline: Option<Ink>,
    /// Room added after each letter (CSS `letter-spacing`); 0 none.
    pub tracking: f64,
    /// The share of `color`'s own opacity the words are drawn at.
    pub opacity: f64,
}

/// A stretch of words, or empty room in a line.
#[derive(Clone, Debug, PartialEq)]
pub enum Run {
    Words {
        text: String,
        style: Style,
    },
    /// Room `width` wide that takes its place in the line as words do: the padding either side
    /// of inline code, inside its ground.
    Space {
        width: f64,
        style: Style,
    },
}

/// Words set in lines from (`x`, `top`), each line `line_height` tall with the height of
/// `line_font` centred in it, as a browser sets a block's lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Words {
    pub x: f64,
    pub top: f64,
    pub width: f64,
    /// False keeps each line whole and lets the card cut it (`white-space: pre`); the words'
    /// own line ends still break.
    pub wraps: bool,
    pub line_height: f64,
    pub line_font: Font,
    /// A tab this many spaces of the first run's font wide; 0 Core Text's own stops.
    pub tab_spaces: u32,
    /// Nothing is drawn outside this.
    pub clip: Option<Frame>,
    pub runs: Vec<Run>,
}

/// What a card draws, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    /// `icon` in `color`.
    Glyph {
        frame: Frame,
        icon: Glyph,
        color: Paint,
    },
    /// One line of words, cut with an ellipsis past `max_width`.
    Line {
        text: String,
        font: Font,
        color: Paint,
        x: f64,
        top: f64,
        line_height: f64,
        max_width: f64,
    },
    Words(Words),
    /// Words set as the open page sets them, both while editing and at rest: Core Text's fixed
    /// line height, with the baseline `ceil(line_height - round(descent))` less
    /// `ceil((line_height - round(ascent) - round(descent)) / 2)` below each line's top, in
    /// points, not snapped to the screen. This puts body type where Chromium puts it, which
    /// `Words` does not.
    PageWords(Words),
    /// A filled rectangle, its corners rounded by `radius` (0 square).
    Fill {
        frame: Frame,
        radius: f64,
        ink: Ink,
    },
    /// A to-do's box standing on the baseline of the line at `line_top`; `frame`'s `y` is how
    /// far its top sits above that baseline. A checked box is filled with `ink`; an unchecked
    /// one is outlined with a `stroke` line, `inset` inside its frame.
    Box {
        frame: Frame,
        line_top: f64,
        line_height: f64,
        line_font: Font,
        radius: f64,
        ink: Ink,
        stroke: f64,
        inset: f64,
    },
    /// Fades what is under it out into `token`, clear at its top and solid at its bottom.
    Fade {
        frame: Frame,
        token: Token,
    },
    /// The oval inside `frame`, filled with `ink`.
    Oval {
        frame: Frame,
        ink: Ink,
    },
    /// A session's dot inside `frame` in the shape `sign`, drawn `sign.growth()` times as
    /// wide about its middle, in `ink`.
    Sign {
        frame: Frame,
        sign: crate::terminal::activity::Sign,
        ink: Ink,
    },
    /// The card's folder art (`board::art`), its 321:270 box in `frame`, in `color`.
    Art {
        frame: Frame,
        color: String,
    },
    /// The card's file as a picture (a video's first frame), at most `pixels` on a side,
    /// covering `frame` about its middle.
    Picture {
        frame: Frame,
        video: bool,
        pixels: u32,
        /// Shown whole inside `frame` rather than covering it (`board::shown_whole`).
        whole: bool,
    },
    /// The video frame `seconds` in, shown while skimming. At most `pixels` on a side, covering
    /// `frame` about its middle.
    VideoMoment {
        frame: Frame,
        seconds: f64,
        pixels: u32,
        /// Shown whole inside `frame` rather than covering it (`board::shown_whole`).
        whole: bool,
    },
    /// A picture the core keeps outside the drive at `path` (`film::poster` or a 3D model's
    /// `model_still`), at most `pixels` on a side, covering `frame` about its middle.
    PictureAt {
        frame: Frame,
        path: String,
        pixels: u32,
    },
    /// The system thumbnail of the card's file (`FileRead::shows_system_thumbnail`), at most
    /// `pixels` on a side, covering `frame` from its top edge so a document shows its first
    /// lines. The card gets an inner border so a white page keeps its edge. When a thumbnail
    /// exists, the marks after this one are skipped; while loading, or without one, they draw.
    SystemPicture {
        frame: Frame,
        pixels: u32,
    },
    /// A terminal program's own logo, by name, in its colours.
    Logo {
        frame: Frame,
        name: &'static str,
    },
    /// The outline of `frame`, its corners rounded by `radius` (0 square), drawn `width` wide
    /// about the frame's edge.
    Stroke {
        frame: Frame,
        radius: f64,
        ink: Ink,
        width: f64,
    },
    /// The picture file `id`, no larger than `pixels` on a side, filling `frame` on a `ground`,
    /// its corners rounded by `radius`.
    Image {
        frame: Frame,
        id: String,
        radius: f64,
        ground: Token,
        pixels: u32,
    },
}

/// How words are set (`typeset::measure`): how many lines `words` take and how wide the widest
/// is.
/// Empty words, or words with no width to go in, take no lines.
pub struct Measured {
    pub lines: usize,
    pub widest: f64,
}

pub type Measure<'a> = dyn FnMut(&Words) -> Measured + 'a;

/// What a page card shows under its header.
pub enum Preview {
    /// The page's first blocks.
    Page(Page),
    /// The top of a code file's source.
    Code(String),
    /// Nothing: the card draws its large glyph.
    Empty,
}

impl Preview {
    /// Whether there is anything to preview: a page with blocks or a source with text.
    fn is_shown(&self) -> bool {
        match self {
            Preview::Page(page) => !page.blocks.is_empty(),
            Preview::Code(source) => !source.is_empty(),
            Preview::Empty => false,
        }
    }
}

/// The marker a list item shows before its words, on a card as on the open page: a bullet
/// that changes with each level in, or a numbered item's number. `None` for anything else (a
/// to-do draws its box).
pub fn list_marker(kind: &BlockKind, number: Option<u32>) -> Option<String> {
    match kind {
        BlockKind::Bullet { indent, .. } => {
            Some(["•", "◦", "▪"][usize::from(*indent) % 3].to_owned())
        }
        BlockKind::Numbered { .. } => Some(format!("{}.", number.unwrap_or(0))),
        _ => None,
    }
}

/// Inline code's padding either side: 0.3 rem, inside its ground.
pub(crate) const CODE_PADDING: f64 = 4.8;

/// The face a stretch of a block's words is drawn in, in the block's type `size` and `weight`,
/// for its marks: code in the monospace at 0.85 of the size, bold at 600, italic slanted. The
/// page draws its words in these faces at rest (`styled`) and while they are typed in
/// (`page::inline_edit::style`).
pub(crate) fn marked_face(size: f64, weight: f64, code: bool, bold: bool, italic: bool) -> Font {
    let upright = if code {
        Font::mono(size * 0.85)
    } else {
        Font::sans(size, if bold { Font::SEMIBOLD } else { weight })
    };
    Font {
        slanted: italic,
        ..upright
    }
}

/// Whitespace as Foundation's `CharacterSet.whitespaces` counts it: spaces and tabs, but not
/// line ends.
fn is_blank(text: &str) -> bool {
    text.chars().all(|character| {
        character.is_whitespace()
            && !matches!(
                character,
                '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}'
            )
    })
}

/// A block's words styled by their marks: bold at 600, italic slanted, strike through, code in
/// the monospace at 0.85 of the size padded 0.3 rem either side, links underlined at 40% of
/// the text colour. Line ends become spaces on a card (`Lines::Collapsed`); the open page keeps
/// them (`Lines::Kept`).
pub(crate) fn styled(
    segments: &[Segment],
    size: f64,
    weight: f64,
    color: Token,
    struck: bool,
    lines: Lines,
) -> Vec<Run> {
    let mut runs = Vec::new();
    for segment in segments {
        let code = segment.marks.contains(&InlineMark::Code);
        let font = marked_face(
            size,
            weight,
            code,
            segment.marks.contains(&InlineMark::Bold),
            segment.marks.contains(&InlineMark::Italic),
        );
        let style = Style {
            font,
            color,
            strike: segment.marks.contains(&InlineMark::Strike) || struck,
            code,
            underline: segment
                .link
                .as_ref()
                .filter(|link| !link.href.is_empty())
                .map(|_| Ink {
                    paint: Token::Foreground.into(),
                    opacity: 0.4,
                }),
            tracking: 0.0,
            opacity: 1.0,
        };
        // The padding is room, not words: nothing strikes or underlines it.
        let padding = Run::Space {
            width: CODE_PADDING,
            style: Style {
                strike: false,
                underline: None,
                ..style
            },
        };
        if code {
            runs.push(padding.clone());
        }
        let text = match lines {
            Lines::Collapsed => segment.text.replace('\n', " "),
            Lines::Kept => segment.text.clone(),
        };
        runs.push(Run::Words { text, style });
        if code {
            runs.push(padding);
        }
    }
    runs
}

/// What becomes of the line ends inside a block's words (`styled`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lines {
    /// Each is a space, as a browser collapses them in a card's preview.
    Collapsed,
    /// Each ends its line, as the open page shows them.
    Kept,
}

/// Words in one face and colour, their line ends kept.
pub(crate) fn plain(text: &str, font: Font, color: Token) -> Vec<Run> {
    let style = Style {
        font,
        color,
        strike: false,
        code: false,
        underline: None,
        tracking: 0.0,
        opacity: 1.0,
    };
    vec![Run::Words {
        text: text.to_owned(),
        style,
    }]
}

/// Lays words out, and the height their lines take.
struct Setter<'a, 'b> {
    marks: Vec<Mark>,
    measure: &'a mut Measure<'b>,
    /// The interface language's code, which the card's own words are in.
    language: &'a str,
}

impl Setter<'_, '_> {
    /// Sets `runs` wrapped `width` wide from (`x`, `top`) and returns the height they take.
    fn words(
        &mut self,
        runs: Vec<Run>,
        x: f64,
        top: f64,
        width: f64,
        line_height: f64,
        line_font: Font,
    ) -> f64 {
        let words = Words {
            x,
            top,
            width,
            wraps: true,
            line_height,
            line_font,
            tab_spaces: 0,
            clip: None,
            runs,
        };
        let lines = (self.measure)(&words).lines;
        self.marks.push(Mark::Words(words));
        lines as f64 * line_height
    }

    /// Draws one block of the preview with its top-left at (`left`, `top`) and returns its
    /// height. Sizes are in ems of the preview's type `size`.
    #[allow(clippy::too_many_arguments)]
    fn block(
        &mut self,
        page: &Page,
        index: usize,
        first: bool,
        left: f64,
        top: f64,
        width: f64,
        size: f64,
    ) -> f64 {
        let line_height = size * 1.6;
        let block = &page.blocks[index];
        let segments = &page.segments[index];
        match &block.kind {
            BlockKind::Heading { level, .. } => {
                // Semibold, larger by its level, with air above it unless it opens the card.
                let own = size * [1.35, 1.2, 1.08][usize::from((*level).clamp(1, 3)) - 1];
                let above = if first { 0.0 } else { own * 0.4 };
                let runs = styled(
                    segments,
                    own,
                    600.0,
                    Token::Foreground,
                    false,
                    Lines::Collapsed,
                );
                above
                    + self.words(
                        runs,
                        left,
                        top + above,
                        width,
                        own * 1.3,
                        Font::sans(own, Font::REGULAR),
                    )
            }
            BlockKind::Bullet { indent, .. }
            | BlockKind::Numbered { indent, .. }
            | BlockKind::Todo { indent, .. } => {
                // Each level in is 1.25 em.
                let x = left + f64::from(*indent) * size * 1.25;
                self.list_item(page, index, x, top, left + width, size)
            }
            BlockKind::Quote { .. } => {
                // A 2-point rule down its left, the words 0.75 em in.
                let runs = styled(
                    segments,
                    size,
                    400.0,
                    Token::Foreground,
                    false,
                    Lines::Collapsed,
                );
                let inset = 2.0 + size * 0.75;
                let height = self.words(
                    runs,
                    left + inset,
                    top,
                    width - inset,
                    line_height,
                    Font::sans(size, Font::REGULAR),
                );
                self.marks.push(Mark::Fill {
                    frame: Frame {
                        x: left,
                        y: top,
                        width: 2.0,
                        height,
                    },
                    radius: 0.0,
                    ink: Ink::of(Token::MutedForeground),
                });
                height
            }
            BlockKind::Code { text, .. } => {
                // The monospace at 0.85 em on a muted ground, its line breaks kept.
                let own = size * 0.85;
                let (pad_y, pad_x) = (own * 0.5, own * 0.75);
                let font = Font::mono(own);
                let words = Words {
                    x: left + pad_x,
                    top: top + pad_y,
                    width: width - pad_x * 2.0,
                    wraps: true,
                    line_height: own * 1.6,
                    line_font: font,
                    tab_spaces: 0,
                    clip: None,
                    runs: plain(text, font, Token::Foreground),
                };
                let text_height = (self.measure)(&words).lines as f64 * own * 1.6;
                self.marks.push(Mark::Fill {
                    frame: Frame {
                        x: left,
                        y: top,
                        width,
                        height: text_height + pad_y * 2.0,
                    },
                    radius: 6.0,
                    ink: Ink::of(Token::Muted),
                });
                self.marks.push(Mark::Words(words));
                text_height + pad_y * 2.0
            }
            BlockKind::Html { .. } => {
                let font = Font::sans(size, Font::REGULAR);
                self.words(
                    plain(
                        crate::strings::text(
                            self.language,
                            crate::strings::key::BOARD_HTML_ARTIFACT,
                        ),
                        font,
                        Token::MutedForeground,
                    ),
                    left,
                    top,
                    width,
                    line_height,
                    font,
                )
            }
            BlockKind::Divider => {
                // A rule with 0.2 em above and below it.
                self.marks.push(Mark::Fill {
                    frame: Frame {
                        x: left,
                        y: top + size * 0.2,
                        width,
                        height: 1.0,
                    },
                    radius: 0.0,
                    ink: Ink::of(Token::Border),
                });
                size * 0.4 + 1.0
            }
            BlockKind::Media { caption, .. } => {
                let font = Font {
                    slanted: true,
                    ..Font::sans(size, Font::REGULAR)
                };
                self.words(
                    plain(caption, font, Token::MutedForeground),
                    left,
                    top,
                    width,
                    line_height,
                    font,
                )
            }
            BlockKind::Paragraph { .. } | BlockKind::Table { .. } | BlockKind::Player { .. } => {
                let runs = styled(
                    segments,
                    size,
                    400.0,
                    Token::Foreground,
                    false,
                    Lines::Collapsed,
                );
                self.words(
                    runs,
                    left,
                    top,
                    width,
                    line_height,
                    Font::sans(size, Font::REGULAR),
                )
            }
        }
    }

    /// Draws the list item `page.blocks[index]` from (`x`, `top`) to `right` and returns its
    /// height: a quiet marker at least 1 em wide, half an em, then the words.
    fn list_item(
        &mut self,
        page: &Page,
        index: usize,
        x: f64,
        top: f64,
        right: f64,
        size: f64,
    ) -> f64 {
        let line_height = size * 1.6;
        let kind = &page.blocks[index].kind;
        let (marker_width, checked) = if let BlockKind::Todo { checked, .. } = *kind {
            let look = if checked { CHECKED_BOX } else { OPEN_BOX };
            self.marks.push(todo_box(x, top, size, look));
            (size, checked)
        } else {
            let marker_width = self.marker(kind, page.numbers[index], x, top, size);
            (marker_width, false)
        };
        let color = if checked {
            Token::MutedForeground
        } else {
            Token::Foreground
        };
        let runs = styled(
            &page.segments[index],
            size,
            400.0,
            color,
            checked,
            Lines::Collapsed,
        );
        let text_x = x + marker_width + size * 0.5;
        line_height.max(self.words(
            runs,
            text_x,
            top,
            right - text_x,
            line_height,
            Font::sans(size, Font::REGULAR),
        ))
    }

    /// Sets a bullet's or a number's marker at (`x`, `top`) and returns how wide it is: its
    /// words' width, 1 em at the least.
    fn marker(
        &mut self,
        kind: &BlockKind,
        number: Option<u32>,
        x: f64,
        top: f64,
        size: f64,
    ) -> f64 {
        let line_height = size * 1.6;
        let marker = list_marker(kind, number).unwrap_or_default();
        let font = Font::sans(size, Font::REGULAR);
        let alone = Words {
            x,
            top,
            width: f64::INFINITY,
            wraps: false,
            line_height,
            line_font: font,
            tab_spaces: 0,
            clip: None,
            runs: plain(&marker, font, Token::MutedForeground),
        };
        let marker_width = size.max((self.measure)(&alone).widest);
        self.marks.push(Mark::Line {
            text: marker,
            font,
            color: Token::MutedForeground.into(),
            x,
            top,
            line_height,
            max_width: marker_width,
        });
        marker_width
    }

    /// Draws `page`'s blocks from (`left`, `top`), `width` wide, 0.6 em apart, until one would
    /// start at `bottom` or below.
    fn page_blocks(
        &mut self,
        page: &Page,
        left: f64,
        mut top: f64,
        width: f64,
        size: f64,
        bottom: f64,
    ) {
        let mut first = true;
        for (index, block) in page.blocks.iter().enumerate() {
            // A table is not drawn on a card yet, nor a media block without a caption.
            match &block.kind {
                BlockKind::Table { .. } => continue,
                BlockKind::Media { caption, .. } if is_blank(caption) => continue,
                _ => {}
            }
            if !first {
                top += size * 0.6;
            }
            if top >= bottom {
                break;
            }
            top += self.block(page, index, first, left, top, width, size);
            first = false;
        }
    }
}

/// How a to-do's box looks: filled when it is checked, a 1.5-point ring when it is not, its
/// corners drawn `inset` in.
#[derive(Clone, Copy)]
struct BoxLook {
    color: Token,
    stroke: f64,
    inset: f64,
}

const CHECKED_BOX: BoxLook = BoxLook {
    color: Token::Foreground,
    stroke: 0.0,
    inset: 0.0,
};

const OPEN_BOX: BoxLook = BoxLook {
    color: Token::MutedForeground,
    stroke: 1.5,
    inset: 0.75,
};

/// A to-do's box at `x` on the line from `top`, in the preview's type `size`. The box is 0.85 em
/// and stands on the line's baseline. Its 6-point corners are all of so small a box.
fn todo_box(x: f64, top: f64, size: f64, look: BoxLook) -> Mark {
    let side = size * 0.85;
    Mark::Box {
        frame: Frame {
            x,
            y: side,
            width: side,
            height: side,
        },
        line_top: top,
        line_height: size * 1.6,
        line_font: Font::sans(size, Font::REGULAR),
        radius: (side / 2.0).min(6.0) - look.inset,
        ink: Ink::of(look.color),
        stroke: look.stroke,
        inset: look.inset,
    }
}

/// The marks for a page card filling `frame`, in `unit`s (`board::card_unit`), titled `title`
/// and showing `preview`. A code file (`code`) gets the code glyph. Floors keep the text
/// legible when the board is zoomed out.
pub fn page_card(
    frame: Frame,
    unit: f64,
    title: &str,
    code: bool,
    preview: &Preview,
    language: &str,
    measure: &mut Measure,
) -> Vec<Mark> {
    let header = header_height(unit);
    let mut setter = Setter {
        marks: Vec::new(),
        measure,
        language,
    };
    let glyph = if code {
        Glyph::FileCode
    } else {
        Glyph::FileText
    };
    setter
        .marks
        .extend(card_header(frame, unit, title, glyph, PAGE_ACCENT));

    let body = Frame {
        x: frame.x,
        y: frame.y + header,
        width: frame.width,
        height: frame.height - header,
    };
    let bottom = body.y + body.height;
    if !preview.is_shown() {
        setter.marks.push(large_glyph(body, unit, glyph));
        return setter.marks;
    }
    // At the card's type size: blocks 0.6 em apart, lines 1.6 em.
    let size = 10.0_f64.max(unit * 9.0);
    let (pad_y, pad_x) = (10.0_f64.max(unit * 22.0), 10.0_f64.max(unit * 21.0));
    let (left, width) = (body.x + pad_x, body.width - pad_x * 2.0);
    let top = body.y + pad_y;
    match preview {
        Preview::Code(source) => {
            // A code file's source: the monospace at the card's type size, lines kept whole and
            // cut by the card, a tab two characters wide.
            let font = Font::mono(size);
            setter.marks.push(Mark::Words(Words {
                x: left,
                top,
                width,
                wraps: false,
                line_height: size * 1.6,
                line_font: font,
                tab_spaces: 2,
                clip: Some(Frame {
                    x: left,
                    y: body.y,
                    width,
                    height: body.height,
                }),
                runs: plain(source, font, Token::Foreground),
            }));
        }
        Preview::Page(page) => setter.page_blocks(page, left, top, width, size, bottom),
        Preview::Empty => {}
    }
    // The preview runs past the card's bottom edge; it fades out rather than cutting a line.
    let fade = 24.0_f64.max(unit * 40.0);
    setter.marks.push(Mark::Fade {
        frame: Frame {
            x: body.x,
            y: bottom - fade,
            width: body.width,
            height: fade,
        },
        token: Token::Card,
    });
    setter.marks
}

/// How tall a card's header is (`card_header`): 56 units, never under 36 points.
pub fn header_height(unit: f64) -> f64 {
    36.0_f64.max(unit * 56.0)
}

/// The `header_height`-tall header of a page or film card filling `frame`: `glyph` in
/// `accent`, `title`, and a faint rule underneath.
pub fn card_header(frame: Frame, unit: f64, title: &str, glyph: Glyph, accent: Paint) -> [Mark; 3] {
    let header = header_height(unit);
    let icon = 14.0_f64.max(unit * 19.0);
    let inset = 10.0_f64.max(unit * 19.0);
    let title_size = 12.0_f64.max(unit * 16.0);
    let title_x = frame.x + inset + icon + 8.0_f64.max(unit * 12.0);
    let title_line = (title_size * 1.3).round();
    [
        Mark::Glyph {
            frame: Frame {
                x: frame.x + inset,
                y: frame.y + (header - icon) / 2.0,
                width: icon,
                height: icon,
            },
            icon: glyph,
            color: accent,
        },
        Mark::Line {
            text: title.to_owned(),
            font: Font::sans(title_size, Font::MEDIUM),
            color: Token::Foreground.into(),
            x: title_x,
            top: frame.y + (header - title_line) / 2.0,
            line_height: title_line,
            max_width: frame.x + frame.width - inset - title_x,
        },
        Mark::Fill {
            frame: Frame {
                x: frame.x,
                y: frame.y + header - 1.0,
                width: frame.width,
                height: 1.0,
            },
            radius: 0.0,
            ink: Ink {
                paint: Token::Border.into(),
                opacity: 0.6,
            },
        },
    ]
}

/// The page's `glyph` large in the middle of a card's `body`, drawn when there is nothing to
/// preview.
fn large_glyph(body: Frame, unit: f64, glyph: Glyph) -> Mark {
    let large = 22.0_f64.max(unit * 30.0);
    Mark::Glyph {
        frame: Frame {
            x: body.x + body.width * 0.5 - large / 2.0,
            y: body.y + body.height * 0.5 - large / 2.0,
            width: large,
            height: large,
        },
        icon: glyph,
        color: PAGE_ACCENT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each character half the font's size wide; words break between characters.
    fn fixed(words: &Words) -> Measured {
        let mut lines = 0;
        let mut widest: f64 = 0.0;
        for line in text_lines(words) {
            lines += 1;
            widest = widest.max(line);
        }
        Measured { lines, widest }
    }

    fn text_lines(words: &Words) -> Vec<f64> {
        let width_of = |run: &Run| match run {
            Run::Words { text, style } => text.chars().count() as f64 * style.font.size / 2.0,
            Run::Space { width, .. } => *width,
        };
        let total: f64 = words.runs.iter().map(width_of).sum();
        let empty = words
            .runs
            .iter()
            .all(|run| matches!(run, Run::Words { text, .. } if text.is_empty()));
        if empty || words.width <= 0.0 {
            return Vec::new();
        }
        if !words.wraps {
            return vec![total];
        }
        let mut lines = Vec::new();
        let mut left = total;
        while left > words.width {
            lines.push(words.width);
            left -= words.width;
        }
        lines.push(left);
        lines
    }

    fn page(text: &str) -> Page {
        crate::board::page_preview("Card", text, "en").expect("a page with something to show")
    }

    fn card(preview: &Preview) -> Vec<Mark> {
        page_card(
            Frame {
                x: 4.0,
                y: 4.0,
                width: 200.0,
                height: 250.0,
            },
            1.0,
            "Card",
            false,
            preview,
            "en",
            &mut fixed,
        )
    }

    fn words_of(marks: &[Mark]) -> Vec<&Words> {
        marks
            .iter()
            .filter_map(|mark| {
                if let Mark::Words(words) = mark {
                    Some(words)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn a_list_marker_changes_with_each_level_and_counts_numbers() {
        let bullet = |indent| BlockKind::Bullet {
            indent,
            text: String::new(),
        };
        assert_eq!(list_marker(&bullet(0), None).as_deref(), Some("•"));
        assert_eq!(list_marker(&bullet(1), None).as_deref(), Some("◦"));
        assert_eq!(list_marker(&bullet(5), None).as_deref(), Some("▪"));
        assert_eq!(
            list_marker(
                &BlockKind::Numbered {
                    indent: 0,
                    text: String::new()
                },
                Some(3)
            )
            .as_deref(),
            Some("3.")
        );
        assert_eq!(
            list_marker(
                &BlockKind::Todo {
                    indent: 0,
                    checked: false,
                    text: String::new()
                },
                None
            ),
            None
        );
    }

    #[test]
    fn the_header_floors_keep_a_small_card_legible() {
        let marks = page_card(
            Frame {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 125.0,
            },
            0.5,
            "Tiny",
            true,
            &Preview::Empty,
            "en",
            &mut fixed,
        );
        assert_eq!(
            marks[0],
            Mark::Glyph {
                frame: Frame {
                    x: 10.0,
                    y: 11.0,
                    width: 14.0,
                    height: 14.0
                },
                icon: Glyph::FileCode,
                color: PAGE_ACCENT
            }
        );
        let Mark::Line {
            x,
            top,
            line_height,
            max_width,
            font,
            ..
        } = &marks[1]
        else {
            panic!("the title")
        };
        // 12-point title on a 16-point line (15.6 rounded), 8 points after the glyph.
        assert_eq!(
            (*x, *top, *line_height, *max_width, font.size, font.weight),
            (32.0, 10.0, 16.0, 58.0, 12.0, 500.0)
        );
        assert_eq!(
            marks[2],
            Mark::Fill {
                frame: Frame {
                    x: 0.0,
                    y: 35.0,
                    width: 100.0,
                    height: 1.0
                },
                radius: 0.0,
                ink: Ink {
                    paint: Token::Border.into(),
                    opacity: 0.6
                }
            }
        );
        // Nothing to preview: the large glyph in the middle of the body, and no fade.
        assert_eq!(
            marks[3],
            Mark::Glyph {
                frame: Frame {
                    x: 39.0,
                    y: 69.5,
                    width: 22.0,
                    height: 22.0
                },
                icon: Glyph::FileCode,
                color: PAGE_ACCENT
            }
        );
        assert_eq!(marks.len(), 4);
    }

    #[test]
    fn blocks_stack_six_tenths_of_an_em_apart_from_the_body_padding() {
        let marks = card(&Preview::Page(page(
            "# Title\n\nFirst words\n\nSecond words\n",
        )));
        let words = words_of(&marks);
        // The header is 56, the body padded 22 down and 21 in; the type is 10 points (unit 1
        // floors at 10).
        assert_eq!(
            (words[0].x, words[0].top, words[0].width),
            (25.0, 82.0, 158.0)
        );
        // The page's own title opens the page and is not a block; the paragraphs follow at
        // 16-point lines, 6 apart.
        assert_eq!(words[1].top, 82.0 + 16.0 + 6.0);
        assert!(
            matches!(marks.last(), Some(Mark::Fade { frame, token: Token::Card }) if *frame == Frame { x: 4.0, y: 214.0, width: 200.0, height: 40.0 })
        );
    }

    #[test]
    fn a_heading_scales_by_its_level_with_air_above_unless_first() {
        let marks = card(&Preview::Page(page("Intro\n\n## Second\n\n### Third\n")));
        let words = words_of(&marks);
        let Run::Words { style, .. } = &words[1].runs[0] else {
            panic!("words")
        };
        let own = 10.0 * 1.2;
        assert_eq!(
            (style.font.size, style.font.weight, words[1].line_height),
            (own, 600.0, own * 1.3)
        );
        assert_eq!(words[1].top, 82.0 + 16.0 + 6.0 + own * 0.4);
        let Run::Words { style, .. } = &words[2].runs[0] else {
            panic!("words")
        };
        assert_eq!(style.font.size, 10.0 * 1.08);
    }

    #[test]
    fn inline_marks_become_styles_and_code_is_padded() {
        let marks = card(&Preview::Page(page(
            "Some **bold** *lean* `code` ~~gone~~ [link](https://example.com)\nend\n",
        )));
        let runs = &words_of(&marks)[0].runs;
        let style = |index: usize| match &runs[index] {
            Run::Words { style, .. } | Run::Space { style, .. } => *style,
        };
        let text = |index: usize| match &runs[index] {
            Run::Words { text, .. } => text.clone(),
            Run::Space { width, .. } => format!("<{width}>"),
        };
        let all: Vec<String> = (0..runs.len()).map(text).collect();
        assert_eq!(
            all,
            [
                "Some ", "bold", " ", "lean", " ", "<4.8>", "code", "<4.8>", " ", "gone", " ",
                "link", " end"
            ]
        );
        assert_eq!(style(1).font.weight, 600.0);
        assert!(style(3).font.slanted);
        assert_eq!(
            (style(6).font, style(6).code),
            (Font::mono(10.0 * 0.85), true)
        );
        assert!(
            !style(5).strike && style(5).code,
            "the padding is room in the code's face"
        );
        assert!(style(9).strike);
        assert_eq!(
            style(11).underline,
            Some(Ink {
                paint: Token::Foreground.into(),
                opacity: 0.4
            })
        );
        assert_eq!(style(12).underline, None);
    }

    #[test]
    fn list_items_indent_and_put_their_words_after_the_marker() {
        let marks = card(&Preview::Page(page(
            "- one\n  - two\n1. first\n- [x] done\n",
        )));
        let lines: Vec<(&str, f64, f64)> = marks
            .iter()
            .filter_map(|mark| match mark {
                Mark::Line {
                    text, x, max_width, ..
                } if text != "Card" => Some((text.as_str(), *x, *max_width)),
                _ => None,
            })
            .collect();
        assert_eq!(
            lines,
            [("•", 25.0, 10.0), ("◦", 37.5, 10.0), ("1.", 25.0, 10.0)]
        );
        let words = words_of(&marks);
        assert_eq!(
            (words[0].x, words[0].width),
            (25.0 + 10.0 + 5.0, 158.0 - 15.0)
        );
        let Some(Mark::Box {
            frame, ink, stroke, ..
        }) = marks.iter().find(|mark| matches!(mark, Mark::Box { .. }))
        else {
            panic!("the to-do's box")
        };
        let side = 10.0 * 0.85;
        assert_eq!(
            (*frame, *ink, *stroke),
            (
                Frame {
                    x: 25.0,
                    y: side,
                    width: side,
                    height: side
                },
                Ink::of(Token::Foreground),
                0.0
            )
        );
        let Run::Words { style, .. } = &words[3].runs[0] else {
            panic!("words")
        };
        assert_eq!(
            (style.color, style.strike),
            (Token::MutedForeground, true),
            "a done to-do is struck and quiet"
        );
    }

    #[test]
    fn quotes_code_and_dividers_draw_their_grounds_and_rules() {
        let marks = card(&Preview::Page(page(
            "> said\n\n```\nlet a = 1;\n```\n\n---\n\nafter\n",
        )));
        let fills: Vec<(Frame, f64, Paint)> = marks
            .iter()
            .filter_map(|mark| match mark {
                Mark::Fill { frame, radius, ink } if ink.opacity == 1.0 => {
                    Some((*frame, *radius, ink.paint))
                }
                _ => None,
            })
            .collect();
        // The quote's rule as tall as its one line; code's ground over its line and padding
        // (8.5-point code: 4.25 above and below a 13.6 line); a divider 2 points down.
        assert_eq!(
            fills[0],
            (
                Frame {
                    x: 25.0,
                    y: 82.0,
                    width: 2.0,
                    height: 16.0
                },
                0.0,
                Paint::Theme(Token::MutedForeground)
            )
        );
        let code_top = 82.0 + 16.0 + 6.0;
        let own = 10.0 * 0.85;
        let ground = 1.0 * own * 1.6 + own * 0.5 * 2.0;
        assert_eq!(
            fills[1],
            (
                Frame {
                    x: 25.0,
                    y: code_top,
                    width: 158.0,
                    height: ground
                },
                6.0,
                Paint::Theme(Token::Muted)
            )
        );
        let divider_top = code_top + ground + 6.0;
        assert_eq!(
            fills[2],
            (
                Frame {
                    x: 25.0,
                    y: divider_top + 2.0,
                    width: 158.0,
                    height: 1.0
                },
                0.0,
                Paint::Theme(Token::Border)
            )
        );
        let words = words_of(&marks);
        assert_eq!((words[0].x, words[0].width), (25.0 + 9.5, 158.0 - 9.5));
        assert_eq!(words[2].top, divider_top + 5.0 + 6.0);
    }

    #[test]
    fn an_html_artifact_shows_as_its_name_in_the_interface_language() {
        let marks = card(&Preview::Page(page("```northroom-html\n<p>hi</p>\n```\n")));
        let Run::Words { text, .. } = &words_of(&marks)[0].runs[0] else {
            panic!("the artifact's name")
        };
        assert_eq!(text, "HTML artifact");
    }

    #[test]
    fn tables_and_bare_media_are_skipped_and_a_full_card_stops() {
        let marks = card(&Preview::Page(page(
            "| a |\n| --- |\n| 1 |\n\n![](asset:x)\n\n![A shot](asset:y)\n\nwords\n",
        )));
        let words = words_of(&marks);
        assert_eq!(words.len(), 2);
        assert_eq!(
            words[0].top, 82.0,
            "the first block drawn has no space above it"
        );
        let Run::Words { text, style } = &words[0].runs[0] else {
            panic!("the caption")
        };
        assert_eq!(
            (text.as_str(), style.color, style.font.slanted),
            ("A shot", Token::MutedForeground, true)
        );
        assert_eq!(
            words[0].line_font, style.font,
            "a caption's line is centred for its own face"
        );
        let long = (0..40)
            .map(|index| format!("Paragraph {index}\n\n"))
            .collect::<String>();
        let marks = card(&Preview::Page(page(&long)));
        // Body 4+56 to 254, padded 22: each paragraph is a 16-point line and 6 apart.
        assert_eq!(words_of(&marks).len(), 8);
    }

    #[test]
    fn a_code_file_keeps_its_lines_whole_inside_the_card() {
        let marks = card(&Preview::Code("fn main() {\n\tgo();\n}".to_owned()));
        let words = words_of(&marks);
        assert_eq!(words.len(), 1);
        assert!(!words[0].wraps);
        assert_eq!(words[0].tab_spaces, 2);
        assert_eq!(
            words[0].clip,
            Some(Frame {
                x: 25.0,
                y: 60.0,
                width: 158.0,
                height: 194.0
            })
        );
        assert_eq!(words[0].line_font, Font::mono(10.0));
    }

    #[test]
    fn marks_choose_the_face_code_in_the_monospace_bold_at_600_italic_slanted() {
        assert_eq!(
            marked_face(15.0, 400.0, false, false, false),
            Font::sans(15.0, Font::REGULAR)
        );
        assert_eq!(
            marked_face(15.0, 400.0, false, true, false),
            Font::sans(15.0, Font::SEMIBOLD)
        );
        // Code keeps its one weight, at 0.85 of the size, and leans when italic.
        assert_eq!(
            marked_face(20.0, 600.0, true, true, true),
            Font {
                slanted: true,
                ..Font::mono(17.0)
            }
        );
    }

    #[test]
    fn a_blank_caption_is_spaces_and_tabs_only() {
        assert!(is_blank(" \t\u{a0}"));
        assert!(!is_blank("\n"));
        assert!(!is_blank("a"));
    }
}
