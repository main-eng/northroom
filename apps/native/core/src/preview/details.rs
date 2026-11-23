//! The preview pane for a file with no view of its own, modelled on Finder's Quick Look.
//!
//! A picture, video or sound stands alone, as large as the pane allows. Any other file shows its
//! system thumbnail or kind glyph over its name, kind and size.

use crate::board::card::Font;
use crate::board::Frame;
use crate::dialog::layout::{Line, Measure};
use crate::geometry::Rect;
use crate::theme::Token;
use crate::workspace::{EntryKind, FileRead};

/// What the pane shows above the file name. Read across the FFI as `ffi::NrDetailsShows`, so
/// keep the order.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shows {
    /// The kind glyph, in a band 128 pt tall.
    Glyph,
    /// The picture, read from the file.
    Picture,
    /// The system thumbnail (`FileRead::shows_system_thumbnail`) in the glyph's place. The glyph
    /// shows while it loads and when there is none.
    SystemPicture,
    /// The video, playing with its controls overlaid.
    Video,
    /// The sound's controls alone.
    Sound,
}

/// What to show for a file of `kind` that the drive reads as `read` (`None` if not read).
pub fn shows(kind: EntryKind, read: Option<&FileRead>) -> Shows {
    match kind {
        EntryKind::Image => Shows::Picture,
        EntryKind::Video => Shows::Video,
        EntryKind::Audio => Shows::Sound,
        EntryKind::File if read.is_some_and(FileRead::shows_system_thumbnail) => {
            Shows::SystemPicture
        }
        EntryKind::File | EntryKind::Folder | EntryKind::Page | EntryKind::Terminal => Shows::Glyph,
    }
}

/// Longest side, in px, a picture is decoded at: enough to fill a tab on any screen.
pub const PICTURE_PIXELS: u32 = 2560;
/// Longest side, in px, of the system thumbnail: its 256 pt maximum at 2x.
pub const SYSTEM_PICTURE_PIXELS: u32 = 512;

/// Longest side, in px, to decode `shows` at.
pub fn picture_pixels(shows: Shows) -> u32 {
    match shows {
        Shows::SystemPicture => SYSTEM_PICTURE_PIXELS,
        Shows::Glyph | Shows::Picture | Shows::Video | Shows::Sound => PICTURE_PIXELS,
    }
}

/// Whether `shows` stands alone, as large as the pane allows, with no text under it. True for a
/// picture, video or sound, both beside the column and filling the tab.
pub fn fills_alone(shows: Shows) -> bool {
    match shows {
        Shows::Picture | Shows::Video | Shows::Sound => true,
        Shows::Glyph | Shows::SystemPicture => false,
    }
}

/// Inset of the pane's contents when it sits beside the column.
const INSET: f64 = 16.0;
/// Margin around a picture that fills the tab.
const FILL_MARGIN: f64 = 32.0;

/// The most `shows` may be scaled up. A video grows to fill the pane as a player does. Pictures
/// and sound controls never grow past their own size.
fn largest_scale(shows: Shows) -> f64 {
    match shows {
        Shows::Video => f64::INFINITY,
        Shows::Picture | Shows::Sound | Shows::SystemPicture | Shows::Glyph => 1.0,
    }
}

/// Centres a picture or video of `size` in a `width` × `height` pane, as large as fits inside
/// `margin` and at most `largest_scale` times its size.
pub fn picture_frame(
    width: f64,
    height: f64,
    size: (f64, f64),
    margin: f64,
    largest_scale: f64,
) -> Frame {
    let (picture_width, picture_height) = size;
    let (room_width, room_height) = (width - 2.0 * margin, height - 2.0 * margin);
    let scale = (room_width / picture_width)
        .min(room_height / picture_height)
        .clamp(0.0, largest_scale);
    let (fitted_width, fitted_height) = (picture_width * scale, picture_height * scale);
    Frame {
        x: width / 2.0 - fitted_width / 2.0,
        y: height / 2.0 - fitted_height / 2.0,
        width: fitted_width,
        height: fitted_height,
    }
}

/// A video is laid out as 16 by 9 until its own shape is known.
const VIDEO_SHAPE: (f64, f64) = (16.0, 9.0);
/// Size of the kind glyph when there is no picture.
const GLYPH: f64 = 64.0;
/// Longest side the system thumbnail is drawn at.
const THUMBNAIL_BOX: f64 = 256.0;
/// The name sits 16 pt below the preview in 14 px medium on a 20 px line. Kind and size sit 4 pt
/// below that in 12 px on a 16 px line.
const SPACE: f64 = 16.0;
const NAME_FONT: Font = Font::sans(14.0, Font::MEDIUM);
const NAME_LINE: f64 = 20.0;
const SUMMARY_GAP: f64 = 4.0;
const SUMMARY_FONT: Font = Font::sans(12.0, Font::REGULAR);
const SUMMARY_LINE: f64 = 16.0;

/// What the pane shows of the file and where.
pub struct Shown<'a> {
    pub kind: EntryKind,
    pub shows: Shows,
    pub title: &'a str,
    /// The kind and size line under the name (`player::summary_line`).
    pub summary: &'a str,
    /// The interface language code.
    pub language: &'a str,
    /// Pixel size of a picture or video from its header, after rotation, so its space is held
    /// before it loads. `None` for other files or when the size is unknown.
    pub media_pixel_size: Option<(u32, u32)>,
}

/// Where the pane's parts sit.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The file fills the tab, alone on black.
    pub fills: bool,
    /// Frame of the picture, video or sound controls. `None` while a picture loads and for
    /// files without one.
    pub media: Option<Rect>,
    /// The kind glyph, when there is no picture.
    pub glyph: Option<Rect>,
    /// The name, with kind and size below it.
    pub words: Vec<Line>,
}

/// What is known of the picture or video the pane shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Media {
    /// Not loaded or not playing yet.
    Unknown,
    /// Its pixel size, once read or playing.
    Sized(f64, f64),
    /// The picture failed to load, so the pane falls back to glyph, name, kind and size.
    Unreadable,
}

fn rect(frame: Frame) -> Rect {
    Rect::new(frame.x, frame.y, frame.width, frame.height)
}

/// Lays out a `width` × `height` pane for `shown`. `fills` means the file fills the tab.
///
/// A picture, video or sound stands alone, as large as fits (`fills_alone`). When the header
/// gives a size, the layout uses it from the first frame so nothing moves once `media` loads.
/// Other files, and pictures that fail to load, centre a thumbnail or glyph over the name and
/// the kind and size line.
pub fn layout(
    width: f64,
    height: f64,
    shown: &Shown,
    media: Media,
    fills: bool,
    measure: &mut Measure,
) -> Layout {
    let known = match media {
        Media::Sized(width, height) if width > 0.0 && height > 0.0 => Some((width, height)),
        _ => None,
    };
    let from_file = shown
        .media_pixel_size
        .map(|(width, height)| (f64::from(width), f64::from(height)));
    let shape = match shown.shows {
        Shows::Video => Some(known.or(from_file).unwrap_or(VIDEO_SHAPE)),
        Shows::Sound => Some(crate::preview::player::sound_controls::SIZE),
        Shows::Picture => from_file.or(known),
        Shows::SystemPicture => known,
        Shows::Glyph => None,
    };
    if media == Media::Unreadable {
        return summary_layout(width, height, shown, None, measure);
    }
    if fills_alone(shown.shows) {
        let margin = if fills { FILL_MARGIN } else { INSET };
        return Layout {
            fills,
            media: shape.map(|shape| {
                rect(picture_frame(
                    width,
                    height,
                    shape,
                    margin,
                    largest_scale(shown.shows),
                ))
            }),
            glyph: None,
            words: Vec::new(),
        };
    }
    summary_layout(width, height, shown, shape, measure)
}

/// Centres the thumbnail (`shape` once read) or glyph over the name and the kind and size line.
fn summary_layout(
    width: f64,
    height: f64,
    shown: &Shown,
    shape: Option<(f64, f64)>,
    measure: &mut Measure,
) -> Layout {
    let inner = width - 2.0 * INSET;
    let above = shape.map_or((GLYPH, GLYPH), |(picture_width, picture_height)| {
        let scale = (THUMBNAIL_BOX.min(inner) / picture_width)
            .min(THUMBNAIL_BOX / picture_height)
            .min(1.0);
        (picture_width * scale, picture_height * scale)
    });
    let stack = above.1 + SPACE + NAME_LINE + SUMMARY_GAP + SUMMARY_LINE;
    let top = ((height - stack) / 2.0).max(INSET);
    let place = Rect::new((width - above.0) / 2.0, top, above.0, above.1);
    let name_top = top + above.1 + SPACE;
    let mut centred = |text: &str, top: f64, line: f64, font: Font, color: Token| {
        let wide = measure(text, font).min(inner);
        Line::new(
            text,
            (width - wide) / 2.0,
            top,
            wide + 1.0,
            line,
            font,
            color,
        )
    };
    let words = vec![
        centred(
            shown.title,
            name_top,
            NAME_LINE,
            NAME_FONT,
            Token::Foreground,
        ),
        centred(
            shown.summary,
            name_top + NAME_LINE + SUMMARY_GAP,
            SUMMARY_LINE,
            SUMMARY_FONT,
            Token::MutedForeground,
        ),
    ];
    let (media, glyph) = match shape {
        Some(_) => (Some(place), None),
        None => (None, Some(place)),
    };
    Layout {
        fills: false,
        media,
        glyph,
        words,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::typeset::test_measure::sevens;

    const SUMMARY: &str = "ZIP archive · 327 KB";

    fn shown(shows: Shows, media_pixel_size: Option<(u32, u32)>) -> Shown<'static> {
        Shown {
            kind: EntryKind::File,
            shows,
            title: "Press kit.zip",
            summary: SUMMARY,
            language: "en",
            media_pixel_size,
        }
    }

    #[test]
    fn a_file_with_no_picture_stacks_its_glyph_name_kind_and_size_in_the_middle() {
        let laid = layout(
            600.0,
            800.0,
            &shown(Shows::Glyph, None),
            Media::Unknown,
            true,
            &mut sevens,
        );
        assert!(
            !laid.fills && laid.media.is_none(),
            "a glyph never fills the tab"
        );
        // 64 + 16 + 20 + 4 + 16 = 120 tall, so it starts 340 down a pane 800 tall.
        assert_eq!(laid.glyph, Some(Rect::new(268.0, 340.0, 64.0, 64.0)));
        assert_eq!(laid.words.len(), 2, "no dates: the name and one line");
        assert_eq!(laid.words[0].text, "Press kit.zip");
        assert_eq!(laid.words[0].frame, Rect::new(254.5, 420.0, 92.0, 20.0));
        assert_eq!(laid.words[1].text, "ZIP archive · 327 KB");
        assert_eq!(laid.words[1].frame.y, 444.0);
        assert_eq!(laid.words[1].color, Token::MutedForeground);
    }

    #[test]
    fn the_systems_thumbnail_takes_the_glyphs_place_at_most_256_on_its_long_side() {
        let system = shown(Shows::SystemPicture, None);
        let reading = layout(600.0, 800.0, &system, Media::Unknown, false, &mut sevens);
        assert!(reading.glyph.is_some() && reading.media.is_none());
        let read = layout(
            600.0,
            800.0,
            &system,
            Media::Sized(512.0, 1024.0),
            false,
            &mut sevens,
        );
        assert_eq!(read.glyph, None);
        // 256 + 16 + 20 + 4 + 16 = 312 tall, so it starts 244 down.
        assert_eq!(read.media, Some(Rect::new(236.0, 244.0, 128.0, 256.0)));
    }

    #[test]
    fn a_picture_video_or_sound_stands_alone_as_large_as_the_pane_lets_it() {
        let picture = shown(Shows::Picture, Some((1600, 900)));
        let beside = layout(1000.0, 800.0, &picture, Media::Unknown, false, &mut sevens);
        assert!(!beside.fills && beside.words.is_empty() && beside.glyph.is_none());
        // 968 wide inside 16 each side, so 968 × 544.5, in the middle.
        assert_eq!(beside.media, Some(Rect::new(16.0, 127.75, 968.0, 544.5)));
        let filled = layout(1000.0, 800.0, &picture, Media::Unknown, true, &mut sevens);
        assert!(filled.fills);
        assert_eq!(filled.media.map(|frame| frame.width), Some(936.0));

        // Never larger than it is.
        let small = shown(Shows::Picture, Some((200, 100)));
        let laid = layout(1000.0, 800.0, &small, Media::Unknown, false, &mut sevens);
        assert_eq!(laid.media, Some(Rect::new(400.0, 350.0, 200.0, 100.0)));

        // A video is 16 by 9 until known; a sound is its controls, in the middle.
        let video = layout(
            1000.0,
            800.0,
            &shown(Shows::Video, None),
            Media::Unknown,
            false,
            &mut sevens,
        );
        assert_eq!(video.media.map(|frame| frame.width), Some(968.0));
        let sound = layout(
            1000.0,
            800.0,
            &shown(Shows::Sound, None),
            Media::Unknown,
            false,
            &mut sevens,
        );
        let controls = crate::preview::player::sound_controls::SIZE;
        assert_eq!(
            sound.media,
            Some(Rect::new(
                500.0 - controls.0 / 2.0,
                400.0 - controls.1 / 2.0,
                controls.0,
                controls.1
            ))
        );
    }

    #[test]
    fn a_picture_sized_by_its_header_keeps_its_place_while_it_is_read() {
        let picture = shown(Shows::Picture, Some((800, 600)));
        let reading = layout(1000.0, 800.0, &picture, Media::Unknown, false, &mut sevens);
        let read = layout(
            1000.0,
            800.0,
            &picture,
            Media::Sized(400.0, 300.0),
            false,
            &mut sevens,
        );
        assert_eq!(reading.media, read.media);
        // A picture whose header says no size (an SVG) is placed once read.
        let vector = shown(Shows::Picture, None);
        assert_eq!(
            layout(1000.0, 800.0, &vector, Media::Unknown, false, &mut sevens).media,
            None
        );
        assert!(layout(
            1000.0,
            800.0,
            &vector,
            Media::Sized(2560.0, 2240.0),
            false,
            &mut sevens
        )
        .media
        .is_some());
    }

    #[test]
    fn a_picture_that_cannot_be_read_shows_its_glyph_and_name_instead() {
        for header in [Some((800, 600)), None] {
            let picture = shown(Shows::Picture, header);
            let laid = layout(
                1000.0,
                800.0,
                &picture,
                Media::Unreadable,
                false,
                &mut sevens,
            );
            assert!(laid.media.is_none() && laid.glyph.is_some());
            assert_eq!(laid.words.len(), 2);
        }
    }

    #[test]
    fn a_picture_video_or_sound_plays_or_shows_and_the_rest_show_their_glyph() {
        assert_eq!(shows(EntryKind::Image, None), Shows::Picture);
        assert_eq!(shows(EntryKind::Video, None), Shows::Video);
        assert_eq!(shows(EntryKind::Audio, None), Shows::Sound);
        assert_eq!(
            shows(EntryKind::File, Some(&FileRead::External)),
            Shows::SystemPicture
        );
        assert_eq!(shows(EntryKind::Folder, None), Shows::Glyph);
        assert!(fills_alone(Shows::Sound) && !fills_alone(Shows::Glyph));
        assert!(!fills_alone(Shows::SystemPicture));
    }
}
