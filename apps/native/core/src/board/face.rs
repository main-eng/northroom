//! Card faces other than a page's (`board::card::page_card`), as marks a shell draws: folder
//! art, terminal tiles, pictures, video frames, 3D model stills, system thumbnails and file
//! glyphs. Lengths are in card `unit`s (`board::card_unit`), so a card reads the same at any
//! size.

use crate::board::card::{plain, Font, Ink, Mark, Measure, Words};
use crate::board::{CardFace, Frame};
use crate::glyph::Glyph;
use crate::terminal::activity::Look;
use crate::theme::{Paint, Token};
use crate::workspace::{EntryKind, FileFamily};

/// The longest side of a card's picture, in pixels.
pub const PICTURE_PIXELS: u32 = 960;
/// The longest side of a video frame drawn while skimming, in pixels. Smaller than the resting
/// picture so frames arrive quickly.
pub const SKIM_PIXELS: u32 = 480;
/// The studio grey a model's still stands on.
pub const MODEL_BACKDROP: Paint = Paint::Fixed("#d4d4d4");

/// Which face a card shows: the core's choice (`board::card_face`), or the kind's own face for
/// `Own`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    /// A page's face (`board::card::page_card`).
    Page,
    Terminal,
    Picture,
    Video,
    File,
    /// The system's thumbnail of the file (`CardFace::SystemPicture`).
    SystemPicture,
    Model,
}

pub fn kind_of(entry: EntryKind, face: CardFace) -> Kind {
    match face {
        CardFace::PageText | CardFace::PageCode => Kind::Page,
        CardFace::File => Kind::File,
        CardFace::SystemPicture => Kind::SystemPicture,
        CardFace::Model => Kind::Model,
        CardFace::Own => match entry {
            EntryKind::Folder => Kind::Folder,
            EntryKind::Page => Kind::Page,
            EntryKind::Terminal => Kind::Terminal,
            EntryKind::Image => Kind::Picture,
            EntryKind::Video => Kind::Video,
            EntryKind::Audio | EntryKind::File => Kind::File,
        },
    }
}

/// What a card that is not a page's shows.
pub enum Face<'a> {
    /// A folder named `title`, its art in the muted text colour.
    Folder { title: &'a str },
    /// A terminal: the `look` of its activity, the word for it (`doing`), the prefix before that
    /// word (`board::terminal_card_lead`) and its program's logo.
    Terminal {
        title: &'a str,
        look: Look,
        doing: &'static str,
        lead: &'a str,
        logo: Option<&'static str>,
    },
    /// A picture, covering the card, or shown `whole` on it (`board::shown_whole`).
    Picture { video: bool, whole: bool },
    /// A video: its first frame, or the frame `at` seconds in while skimming, shown `whole`
    /// (`board::shown_whole`) or covering the card. Its length sits bottom right. While skimming
    /// it also shows a hairline at the pointer and a play button (`play_frame`) in the middle.
    Plays {
        at: Option<f64>,
        whole: bool,
        /// Its length as a clock says it ("0:42"); none while it is not known.
        length: Option<&'a str>,
        /// How far across the card the pointer is, 0 to 1, while it skims the card.
        skim: Option<f64>,
    },
    /// Any other file, marked as its family is.
    File { title: &'a str, family: FileFamily },
    /// A file the system draws a thumbnail of; its glyph and name until it is read, and where
    /// there is none.
    SystemPicture { title: &'a str, family: FileFamily },
    /// A 3D model: its still, a PNG the core keeps (`model_still`), once it is drawn.
    Model {
        title: &'a str,
        family: FileFamily,
        still: Option<&'a str>,
    },
}

/// How wide `text` is set on one line in `font`, as the platform measures it.
fn width_of(text: &str, font: Font, measure: &mut Measure) -> f64 {
    let words = Words {
        x: 0.0,
        top: 0.0,
        width: f64::INFINITY,
        wraps: false,
        line_height: 0.0,
        line_font: font,
        tab_spaces: 0,
        clip: None,
        runs: plain(text, font, Token::Foreground),
    };
    measure(&words).widest
}

/// One line of `text` centred across `frame` inside `inset` either side, cut there.
#[allow(clippy::too_many_arguments)]
fn centred(
    text: &str,
    font: Font,
    color: Token,
    frame: Frame,
    top: f64,
    line_height: f64,
    inset: f64,
    measure: &mut Measure,
) -> Mark {
    let available = frame.width - inset * 2.0;
    let width = width_of(text, font, measure).min(available);
    Mark::Line {
        text: text.to_owned(),
        font,
        color: color.into(),
        x: frame.x + inset + (available - width) / 2.0,
        top,
        line_height,
        max_width: available,
    }
}

/// `frame` grown by `by` on every side.
fn outset(frame: Frame, by: f64) -> Frame {
    Frame {
        x: frame.x - by,
        y: frame.y - by,
        width: frame.width + by * 2.0,
        height: frame.height + by * 2.0,
    }
}

/// The ground under marks drawn over a picture: black at 60%, so white reads on any video frame
/// in every theme.
const OVER_PICTURE_GROUND: Ink = Ink {
    paint: Paint::Fixed("#000000"),
    opacity: 0.6,
};
/// What is drawn on that ground, and the play button's own ground.
const OVER_PICTURE: Paint = Paint::Fixed("#ffffff");
/// The play button's glyph, on its white ground.
const PLAY_GLYPH: Paint = Paint::Fixed("#111111");

/// The play button's hit area over a picture filling `frame`: a circle 40 units across (at
/// least 32 pt) in the middle. It shows while the pointer is over the card; a double click
/// anywhere on the card also plays.
pub fn play_frame(frame: Frame, unit: f64) -> Frame {
    let side = (unit * 40.0).max(32.0);
    square(
        frame.x + frame.width / 2.0,
        frame.y + frame.height / 2.0,
        side,
    )
}

/// The marks drawn over the picture of a card that plays, filling `frame`: the skim's hairline
/// and the play button while the pointer skims it, and its length in the bottom right corner.
fn playing_marks(
    frame: Frame,
    unit: f64,
    length: Option<&str>,
    skim: Option<f64>,
    measure: &mut Measure,
) -> Vec<Mark> {
    let mut marks = Vec::new();
    if let Some(across) = skim {
        let width = unit.max(1.0) * 1.5;
        marks.push(Mark::Fill {
            frame: Frame {
                x: frame.x + (frame.width - width) * across.clamp(0.0, 1.0),
                y: frame.y,
                width,
                height: frame.height,
            },
            radius: 0.0,
            ink: Ink {
                paint: OVER_PICTURE,
                opacity: 0.9,
            },
        });
        let button = play_frame(frame, unit);
        marks.push(Mark::Oval {
            frame: button,
            ink: Ink {
                paint: OVER_PICTURE,
                opacity: 0.92,
            },
        });
        // A triangle's weight sits left of its box's middle: nudged right to look centred.
        marks.push(Mark::Glyph {
            frame: square(
                button.x + button.width * 0.53,
                button.y + button.height / 2.0,
                button.width * 0.42,
            ),
            icon: Glyph::PlayFill,
            color: PLAY_GLYPH,
        });
    }
    if let Some(length) = length {
        let unit = length_unit(unit);
        let font = Font {
            tabular: true,
            ..Font::sans(unit * 11.0, Font::MEDIUM)
        };
        let (height, pad, inset) = (unit * 18.0, unit * 6.0, unit * 8.0);
        let width = width_of(length, font, measure) + pad * 2.0;
        let chip = Frame {
            x: frame.x + frame.width - inset - width,
            y: frame.y + frame.height - inset - height,
            width,
            height,
        };
        marks.push(Mark::Fill {
            frame: chip,
            radius: unit * 5.0,
            ink: OVER_PICTURE_GROUND,
        });
        marks.push(Mark::Line {
            text: length.to_owned(),
            font,
            color: OVER_PICTURE,
            x: chip.x + pad,
            top: chip.y,
            line_height: height,
            max_width: width - pad,
        });
    }
    marks
}

fn square(x: f64, y: f64, side: f64) -> Frame {
    Frame {
        x: x - side / 2.0,
        y: y - side / 2.0,
        width: side,
        height: side,
    }
}

/// The unit for a video's length label: the card unit, but at least 1 pt so it stays readable
/// on the smallest cards.
fn length_unit(unit: f64) -> f64 {
    unit.max(1.0)
}

/// The marks for a card filling `frame` and showing `face`.
pub fn face_marks(frame: Frame, unit: f64, face: &Face, measure: &mut Measure) -> Vec<Mark> {
    let (mid_x, mid_y) = (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
    let name = Font::sans(unit * 13.0, Font::MEDIUM);
    match face {
        Face::Folder { title } => {
            // Its art over its name, centred, 6 units apart, 12 units in.
            let (width, height) = (unit * 104.0, unit * 104.0 * 270.0 / 321.0);
            let (gap, line) = (unit * 6.0, unit * 18.0);
            let top = mid_y - (height + gap + line) / 2.0;
            vec![
                Mark::Art {
                    frame: Frame {
                        x: mid_x - width / 2.0,
                        y: top,
                        width,
                        height,
                    },
                    color: Token::MutedForeground.name().to_owned(),
                },
                centred(
                    title,
                    name,
                    Token::Foreground,
                    frame,
                    top + height + gap,
                    line,
                    unit * 12.0,
                    measure,
                ),
            ]
        }
        Face::Terminal {
            title,
            look,
            doing,
            lead,
            logo,
        } => {
            // Its program's mark on an ink tile over its name, where a folder's art and name
            // sit, ringed in the colour of what it is doing.
            let side = unit * 88.0;
            let (gap, line) = (unit * 6.0, unit * 18.0);
            let top = mid_y - (side + gap + line) / 2.0;
            let tile = Frame {
                x: mid_x - side / 2.0,
                y: top,
                width: side,
                height: side,
            };
            let (tile_x, tile_y) = (tile.x + tile.width / 2.0, tile.y + tile.height / 2.0);
            let ring = unit * 1.5;
            let mut marks = vec![
                Mark::Fill {
                    frame: outset(tile, ring),
                    radius: unit * 21.0 + ring,
                    ink: Ink {
                        paint: look.ring.into(),
                        opacity: look.ring_opacity,
                    },
                },
                Mark::Fill {
                    frame: tile,
                    radius: unit * 21.0,
                    ink: Ink::of(Token::Ink),
                },
                match logo {
                    Some(name) => Mark::Logo {
                        frame: square(tile_x, tile_y, unit * 48.0),
                        name,
                    },
                    None => Mark::Glyph {
                        frame: square(tile_x, tile_y, unit * 42.0),
                        icon: Glyph::TerminalWindow,
                        color: Paint::Theme(Token::Paper),
                    },
                },
            ];
            if let Some(dot) = look.dot {
                let at = Frame {
                    x: tile.x + tile.width - unit * 11.0,
                    y: tile.y + tile.height - unit * 11.0,
                    width: unit * 14.0,
                    height: unit * 14.0,
                };
                let grown = at.width * (look.sign.growth() - 1.0) / 2.0;
                marks.push(Mark::Oval {
                    frame: outset(at, grown + unit * 3.0),
                    ink: Ink::of(Token::Ink),
                });
                marks.push(Mark::Sign {
                    frame: at,
                    sign: look.sign,
                    ink: Ink {
                        paint: dot.into(),
                        opacity: look.dot_fade,
                    },
                });
            }
            // Under the name: the harness, unless the name already says it, then what it is
            // doing, centred as one line.
            let meta = Font::sans(unit * 10.0, Font::REGULAR);
            let meta_top = top + side + gap + line;
            let whole = width_of(&format!("{lead}{doing}"), meta, measure);
            let mut x = mid_x - whole / 2.0;
            if !lead.is_empty() {
                marks.push(Mark::Line {
                    text: (*lead).to_owned(),
                    font: meta,
                    color: Token::MutedForeground.into(),
                    x,
                    top: meta_top,
                    line_height: unit * 14.0,
                    max_width: whole,
                });
                x += width_of(lead, meta, measure);
            }
            marks.push(Mark::Line {
                text: (*doing).to_owned(),
                font: meta,
                color: look.label.into(),
                x,
                top: meta_top,
                line_height: unit * 14.0,
                max_width: whole,
            });
            marks.push(centred(
                title,
                name,
                Token::Foreground,
                frame,
                top + side + gap,
                line,
                unit * 12.0,
                measure,
            ));
            marks
        }
        Face::Picture { video, whole } => vec![Mark::Picture {
            frame,
            video: *video,
            pixels: PICTURE_PIXELS,
            whole: *whole,
        }],
        Face::Plays {
            at,
            whole,
            length,
            skim,
        } => {
            let mut marks: Vec<Mark> = [Mark::Picture {
                frame,
                video: true,
                pixels: PICTURE_PIXELS,
                whole: *whole,
            }]
            .into_iter()
            .chain(at.map(|seconds| Mark::VideoMoment {
                frame,
                seconds,
                pixels: SKIM_PIXELS,
                whole: *whole,
            }))
            .collect();
            marks.extend(playing_marks(frame, unit, *length, *skim, measure));
            marks
        }
        Face::File { title, family } => file_marks(frame, unit, title, *family, measure),
        Face::SystemPicture { title, family } => {
            let mut marks = vec![Mark::SystemPicture {
                frame,
                pixels: PICTURE_PIXELS,
            }];
            marks.extend(file_marks(frame, unit, title, *family, measure));
            marks
        }
        Face::Model {
            title,
            family,
            still,
        } => match still {
            // Its still whole on the studio grey, which fills the card.
            Some(path) => vec![
                Mark::Fill {
                    frame,
                    radius: 0.0,
                    ink: Ink::of(MODEL_BACKDROP),
                },
                Mark::PictureAt {
                    frame: square(mid_x, mid_y, frame.width.min(frame.height)),
                    path: (*path).to_owned(),
                    pixels: PICTURE_PIXELS,
                },
            ],
            // The file's glyph and name while it is drawn, and when it cannot be.
            None => file_marks(frame, unit, title, *family, measure),
        },
    }
}

/// Any other file: its glyph over its name, both quiet.
fn file_marks(
    frame: Frame,
    unit: f64,
    title: &str,
    family: FileFamily,
    measure: &mut Measure,
) -> Vec<Mark> {
    let (icon, gap, line) = (unit * 20.0, unit * 8.0, unit * 14.0);
    let mid_x = frame.x + frame.width / 2.0;
    let top = frame.y + frame.height / 2.0 - (icon + gap + line) / 2.0;
    vec![
        Mark::Glyph {
            frame: Frame {
                x: mid_x - icon / 2.0,
                y: top,
                width: icon,
                height: icon,
            },
            icon: match family {
                FileFamily::Code => Glyph::FileCode,
                FileFamily::Archive => Glyph::FileZip,
                FileFamily::Other => Glyph::FileText,
            },
            color: Paint::Theme(Token::MutedForeground),
        },
        // Its name 12 points in at every size.
        centred(
            title,
            Font::sans(unit * 10.0, Font::REGULAR),
            Token::MutedForeground,
            frame,
            top + icon + gap,
            line,
            12.0,
            measure,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::card::Measured;
    use crate::terminal::activity::{look, Activity};

    /// Each character half the font's size wide, on one line.
    fn fixed(words: &Words) -> Measured {
        let widest = words
            .runs
            .iter()
            .map(|run| match run {
                crate::board::card::Run::Words { text, style } => {
                    text.chars().count() as f64 * style.font.size / 2.0
                }
                crate::board::card::Run::Space { width, .. } => *width,
            })
            .sum();
        Measured { lines: 1, widest }
    }

    const CARD: Frame = Frame {
        x: 4.0,
        y: 4.0,
        width: 200.0,
        height: 250.0,
    };

    fn line(mark: &Mark) -> (&str, f64, f64, f64, f64, Paint) {
        let Mark::Line {
            text,
            x,
            top,
            line_height,
            max_width,
            color,
            ..
        } = mark
        else {
            panic!("a line, not {mark:?}")
        };
        (text, *x, *top, *line_height, *max_width, *color)
    }

    #[test]
    fn each_kind_shows_its_own_face_unless_the_file_says_otherwise() {
        assert_eq!(kind_of(EntryKind::Folder, CardFace::Own), Kind::Folder);
        assert_eq!(kind_of(EntryKind::Page, CardFace::Own), Kind::Page);
        assert_eq!(kind_of(EntryKind::Image, CardFace::Own), Kind::Picture);
        assert_eq!(kind_of(EntryKind::Video, CardFace::Own), Kind::Video);
        assert_eq!(kind_of(EntryKind::Audio, CardFace::Own), Kind::File);
        assert_eq!(kind_of(EntryKind::File, CardFace::PageCode), Kind::Page);
        assert_eq!(kind_of(EntryKind::Page, CardFace::File), Kind::File);
        assert_eq!(kind_of(EntryKind::File, CardFace::Model), Kind::Model);
    }

    #[test]
    fn a_folder_centres_its_art_over_its_name() {
        let face = Face::Folder { title: "Assets" };
        let marks = face_marks(CARD, 1.0, &face, &mut fixed);
        let art_height = 104.0 * 270.0 / 321.0;
        let top = 129.0 - (art_height + 6.0 + 18.0) / 2.0;
        assert_eq!(
            marks[0],
            Mark::Art {
                frame: Frame {
                    x: 52.0,
                    y: top,
                    width: 104.0,
                    height: art_height
                },
                color: Token::MutedForeground.name().to_owned()
            }
        );
        // Six characters of 6.5 points, centred in the 176 points inside the insets.
        assert_eq!(
            line(&marks[1]),
            (
                "Assets",
                16.0 + (176.0 - 39.0) / 2.0,
                top + art_height + 6.0,
                18.0,
                176.0,
                Paint::Theme(Token::Foreground)
            )
        );
    }

    #[test]
    fn a_name_wider_than_the_card_starts_at_the_inset_and_is_cut() {
        let face = Face::File {
            title: &"a".repeat(100),
            family: FileFamily::Code,
        };
        let marks = face_marks(CARD, 1.0, &face, &mut fixed);
        assert!(matches!(
            marks[0],
            Mark::Glyph {
                icon: Glyph::FileCode,
                color: Paint::Theme(Token::MutedForeground),
                ..
            }
        ));
        let (_, x, top, line_height, max_width, color) = line(&marks[1]);
        assert_eq!(
            (x, max_width, color),
            (16.0, 176.0, Paint::Theme(Token::MutedForeground))
        );
        assert_eq!((top, line_height), (129.0 - 21.0 + 28.0, 14.0));
    }

    #[test]
    fn a_terminal_rings_its_tile_and_says_what_it_is_doing() {
        let face = Face::Terminal {
            title: "Shell",
            look: look(Some(Activity::Working)),
            doing: "Working",
            lead: "Codex · ",
            logo: Some("codex"),
        };
        let marks = face_marks(CARD, 1.0, &face, &mut fixed);
        let top = 129.0 - (88.0 + 24.0) / 2.0;
        let tile = Frame {
            x: 60.0,
            y: top,
            width: 88.0,
            height: 88.0,
        };
        assert_eq!(
            marks[0],
            Mark::Fill {
                frame: outset(tile, 1.5),
                radius: 22.5,
                ink: Ink {
                    paint: Token::StatusInfo.into(),
                    opacity: 0.5
                }
            }
        );
        assert_eq!(
            marks[2],
            Mark::Logo {
                frame: square(104.0, top + 44.0, 48.0),
                name: "codex"
            }
        );
        // The dot in the tile's corner, a turning comet at work, on an ink ring 3 wide past
        // the comet's reach.
        assert!(
            matches!(&marks[4], Mark::Sign { frame, sign: crate::terminal::activity::Sign::Comet, ink } if frame.x == 137.0 && ink.paint == Paint::Theme(Token::StatusInfo))
        );
        assert!(
            matches!(&marks[3], Mark::Oval { frame, .. } if frame.x == 137.0 - 3.5 - 3.0 && frame.width == 14.0 + 7.0 + 6.0)
        );
        // "Codex · Working" is 15 characters of 5 points, centred; "Working" follows the lead.
        let lead = line(&marks[5]);
        let doing = line(&marks[6]);
        assert_eq!((lead.0, lead.1, lead.4), ("Codex · ", 104.0 - 37.5, 75.0));
        assert_eq!(
            (doing.0, doing.1, doing.5),
            (
                "Working",
                104.0 - 37.5 + 40.0,
                Paint::Theme(Token::Foreground)
            )
        );
        assert_eq!(line(&marks[7]).2, top + 94.0);
    }

    #[test]
    fn a_terminal_without_a_session_shows_its_window_glyph_and_no_dot() {
        let face = Face::Terminal {
            title: "Shell",
            look: look(None),
            doing: "",
            lead: "",
            logo: None,
        };
        let marks = face_marks(CARD, 1.0, &face, &mut fixed);
        assert!(matches!(
            marks[2],
            Mark::Glyph {
                icon: Glyph::TerminalWindow,
                color: Paint::Theme(Token::Paper),
                ..
            }
        ));
        assert!(!marks
            .iter()
            .any(|mark| matches!(mark, Mark::Oval { .. } | Mark::Sign { .. })));
        assert_eq!(
            line(&marks[3]).1,
            104.0,
            "nothing leads; the empty word is in the middle"
        );
    }

    #[test]
    fn a_model_shows_its_still_on_the_grey_or_its_glyph_until_it_comes() {
        let drawn = Face::Model {
            title: "Chair.glb",
            family: FileFamily::Other,
            still: Some("/caches/Model stills/chair.png"),
        };
        assert_eq!(
            face_marks(CARD, 1.0, &drawn, &mut fixed),
            [
                Mark::Fill {
                    frame: CARD,
                    radius: 0.0,
                    ink: Ink::of(MODEL_BACKDROP)
                },
                Mark::PictureAt {
                    frame: Frame {
                        x: 4.0,
                        y: 29.0,
                        width: 200.0,
                        height: 200.0
                    },
                    path: "/caches/Model stills/chair.png".to_owned(),
                    pixels: PICTURE_PIXELS,
                }
            ]
        );
        let coming = Face::Model {
            title: "Chair.glb",
            family: FileFamily::Other,
            still: None,
        };
        let marks = face_marks(CARD, 1.0, &coming, &mut fixed);
        assert_eq!(marks.len(), 2);
        assert!(matches!(
            marks[0],
            Mark::Glyph {
                icon: Glyph::FileText,
                ..
            }
        ));
    }

    /// What each mark is, in drawing order.
    fn kinds(marks: &[Mark]) -> Vec<&'static str> {
        marks
            .iter()
            .map(|mark| match mark {
                Mark::Picture { .. } => "picture",
                Mark::VideoMoment { .. } => "moment",
                Mark::PictureAt { .. } => "file",
                Mark::Fill { .. } => "fill",
                Mark::Oval { .. } => "oval",
                Mark::Glyph { .. } => "glyph",
                Mark::Line { .. } => "line",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn a_video_at_rest_shows_its_first_frame_and_its_length_in_the_corner() {
        let face = Face::Plays {
            at: None,
            whole: false,
            length: Some("0:42"),
            skim: None,
        };
        let marks = face_marks(CARD, 1.0, &face, &mut fixed);
        assert_eq!(kinds(&marks), ["picture", "fill", "line"]);
        let Mark::Fill { frame: chip, .. } = marks[1] else {
            panic!("the length's ground")
        };
        assert_eq!(
            (chip.x + chip.width, chip.y + chip.height),
            (CARD.x + CARD.width - 8.0, CARD.y + CARD.height - 8.0),
            "8 in from the bottom right corner"
        );
        let (text, _, _, _, _, color) = line(&marks[2]);
        assert_eq!((text, color), ("0:42", OVER_PICTURE), "white on any frame");
    }

    #[test]
    fn a_skimmed_card_shows_the_moment_a_hairline_and_the_play_button() {
        let face = Face::Plays {
            at: Some(12.5),
            whole: false,
            length: None,
            skim: Some(0.25),
        };
        let marks = face_marks(CARD, 1.0, &face, &mut fixed);
        assert_eq!(
            kinds(&marks),
            ["picture", "moment", "fill", "oval", "glyph"],
            "the first frame stands under the moment while it is read"
        );
        let Mark::Fill {
            frame: hairline, ..
        } = marks[2]
        else {
            panic!("the hairline")
        };
        assert_eq!(hairline.x, CARD.x + (CARD.width - 1.5) * 0.25);
        assert_eq!(hairline.height, CARD.height);
        let Mark::Oval { frame: button, .. } = marks[3] else {
            panic!("the play button")
        };
        assert_eq!(button, play_frame(CARD, 1.0));
        assert_eq!(
            (
                button.x + button.width / 2.0,
                button.y + button.height / 2.0
            ),
            (CARD.x + CARD.width / 2.0, CARD.y + CARD.height / 2.0),
            "in the card's middle"
        );
        assert_eq!(button.width, 40.0);
        assert_eq!(play_frame(CARD, 0.5).width, 32.0, "never under 32 points");
    }

    #[test]
    fn a_picture_covers_the_card_or_shows_whole_on_it() {
        for whole in [false, true] {
            assert_eq!(
                face_marks(CARD, 1.0, &Face::Picture { video: true, whole }, &mut fixed),
                [Mark::Picture {
                    frame: CARD,
                    video: true,
                    pixels: PICTURE_PIXELS,
                    whole,
                }]
            );
        }
    }

    #[test]
    fn a_file_the_system_draws_asks_for_its_thumbnail_over_its_glyph_and_name() {
        let face = Face::SystemPicture {
            title: "Brief.pdf",
            family: FileFamily::Other,
        };
        let marks = face_marks(CARD, 1.0, &face, &mut fixed);
        assert_eq!(
            marks[0],
            Mark::SystemPicture {
                frame: CARD,
                pixels: PICTURE_PIXELS
            }
        );
        // What is drawn while it is read, and where the system has none: a file's own face.
        let plain = Face::File {
            title: "Brief.pdf",
            family: FileFamily::Other,
        };
        assert_eq!(marks[1..], face_marks(CARD, 1.0, &plain, &mut fixed));
        assert_eq!(
            kind_of(EntryKind::File, CardFace::SystemPicture),
            Kind::SystemPicture
        );
    }
}
