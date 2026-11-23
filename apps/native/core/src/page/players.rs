//! Media players on the open page: a `<video>` or `<audio>` in it (`page::media`) plays inline.
//! At rest a player shows its poster, or else the file's first frame, under the preview's
//! controls drawn paused where it was left. A press or key on the controls wakes it, and its
//! file becomes the core's preview (the same one the opened-file player uses). Only one player
//! is awake at a time; waking another puts the previous one back to rest where it was. Players
//! are keyed by block id across layouts, so a playing one keeps playing while the page changes.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::page::media::{PlayerElement, PlayerShows, Plays};
use crate::preview::player::video_controls::{fullscreen_fit, Menu, Playback, Rect};

/// The default box a browser gives a `<video>` of unknown size.
const UNKNOWN_SIZE: (f64, f64) = (300.0, 150.0);
/// A player with nothing to play shows its text on a line this tall, 2 pt in from each side of
/// the column, like a missing image.
pub const WORDS_LINE: f64 = 20.8;
const WORDS_INSET: f64 = 2.0;

/// What a player block shows and plays, with the file's stored size (0 when unknown) and
/// duration.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub shows: PlayerShows,
    pub media_width: f64,
    pub media_height: f64,
    pub duration: f64,
}

/// How a player block is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Small muted text, for a player with nothing to play or a sound without controls.
    Words,
    /// A picture (poster or first frame at rest), with controls over it if requested.
    Video,
    /// A sound's pill.
    Sound,
}

impl Shown {
    fn plays_video(&self) -> bool {
        matches!(self.shows.plays, Some((Plays::Video, _)))
    }

    /// A player when it has something to play and asks for controls, or when it is a video, whose
    /// picture shows without them. Anything else shows its text.
    pub fn kind(&self) -> Kind {
        match self.shows.plays {
            None => Kind::Words,
            Some((Plays::Video, _)) => Kind::Video,
            Some((Plays::Sound, _)) if self.shows.controls => Kind::Sound,
            Some((Plays::Sound, _)) => Kind::Words,
        }
    }
}

/// The size a browser gives the element in a `column`-wide column: its written width and
/// height, deriving a missing one from the file's aspect ratio, else the file's size (300 by
/// 150 when unknown). It never exceeds the column, keeps its aspect ratio and rounds to the
/// point. A sound's pill is its written width (default 300) by the pill height, capped at the
/// column.
pub fn player_box(shown: &Shown, column: f64) -> (f64, f64) {
    let shows = &shown.shows;
    if !shown.plays_video() {
        let (width, height) = crate::preview::player::sound_controls::SIZE;
        return (shows.width.unwrap_or(width).min(column), height);
    }
    let natural = if shown.media_width > 0.0 && shown.media_height > 0.0 {
        (shown.media_width, shown.media_height)
    } else {
        UNKNOWN_SIZE
    };
    let mut size = match (shows.width, shows.height) {
        (Some(width), Some(height)) => (width, height),
        (Some(width), None) => (width, width * natural.1 / natural.0),
        (None, Some(height)) => (height * natural.0 / natural.1, height),
        (None, None) => natural,
    };
    if size.0 > column {
        size = (column, size.1 * column / size.0);
    }
    (size.0.round(), size.1.round())
}

/// Where the picture sits in the element's box `frame`: fitted whole and centred
/// (`fullscreen_fit`), or the whole box when its size is unknown or it is a sound.
pub fn picture_in(shown: &Shown, frame: Rect) -> Rect {
    if !(shown.plays_video() && shown.media_width > 0.0 && shown.media_height > 0.0) {
        return frame;
    }
    let fitted = fullscreen_fit(
        shown.media_width,
        shown.media_height,
        frame.width,
        frame.height,
    );
    Rect {
        x: frame.x + fitted.x,
        y: frame.y + fitted.y,
        ..fitted
    }
}

/// One player block as laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Laid {
    pub kind: Kind,
    /// The element's box in the page (its text line, for `Kind::Words`), and the picture's frame
    /// inside it (none for text).
    pub frame: Rect,
    pub picture: Rect,
    /// It draws the preview's controls.
    pub controls: bool,
    /// What it shows changed since the last layout, so it resets to rest with new controls.
    pub changed: bool,
    /// It was awake, and that change put it back to rest.
    pub slept: bool,
}

/// How a page's player opens its file when it wakes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Waking {
    pub video: bool,
    pub muted: bool,
    /// Where it was left.
    pub start_at: f64,
}

/// What the awake player needs, checked on each poll while it is awake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    /// The page left the window or was hidden, so it goes back to rest.
    Sleep,
    /// A `loop` player reached the end and restarts.
    PlayAgain,
    /// It keeps playing; redraw its controls.
    Draw,
}

#[derive(Clone, Debug, PartialEq)]
struct Player {
    shown: Shown,
    /// Where it was left, to resume from when woken.
    resting_at: f64,
    laid: Laid,
}

/// The open page's players by block id, the rows they were last laid out in, and which one is
/// awake.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PagePlayers {
    players: HashMap<String, Player>,
    rows: BTreeMap<usize, String>,
    /// The rows laid out since the last `forget_except`, which then become `rows`.
    laying: BTreeMap<usize, String>,
    awake: Option<String>,
}

/// State from the preview and the shell for a check's report (`PagePlayers::report`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Observed {
    /// The page's top-left corner relative to the window's.
    pub corner: (f64, f64),
    /// The open file's state while a player is awake, and its menu.
    pub preview: Option<crate::preview::PreviewInfo>,
    pub menu: Menu,
    pub sound_frames: u64,
    pub sound_peak: f32,
    pub output: bool,
    pub chosen: Option<(i64, i64)>,
    pub editing: i64,
    pub keys: String,
    /// How many blocks the page has; -1 when none is open.
    pub blocks: i64,
}

#[derive(Serialize)]
struct RowReport<'a> {
    row: usize,
    element: PlayerElement,
    /// `video`, `sound` or `nothing` (`Plays::name`).
    plays: &'static str,
    words: &'a str,
    controls: bool,
    awake: bool,
    #[serde(rename = "box")]
    frame: [i64; 4],
    picture: [i64; 4],
    #[serde(rename = "restingAt")]
    resting_at: f64,
    autoplay: bool,
    #[serde(rename = "loop")]
    looping: bool,
    muted: bool,
    layer: bool,
}

impl PagePlayers {
    /// Lays out player block `row` with id `block`, its box's top at `top`, in a column starting at
    /// `side` and `column` wide.
    pub fn lay(
        &mut self,
        row: usize,
        block: &str,
        shown: Shown,
        top: f64,
        side: f64,
        column: f64,
    ) -> Laid {
        let previous = self.players.remove(block);
        let changed = previous
            .as_ref()
            .is_some_and(|player| player.shown != shown);
        let slept = changed && self.awake.as_deref() == Some(block);
        if slept {
            self.awake = None;
        }
        // Its media changed, so it resets to rest at the start.
        let resting_at = previous
            .filter(|_| !changed)
            .map_or(0.0, |player| player.resting_at);
        let kind = shown.kind();
        let laid = if kind == Kind::Words {
            let frame = Rect {
                x: side + WORDS_INSET,
                y: top,
                width: column - 2.0 * WORDS_INSET,
                height: WORDS_LINE,
            };
            Laid {
                kind,
                frame,
                picture: Rect::default(),
                controls: false,
                changed,
                slept,
            }
        } else {
            let (width, height) = player_box(&shown, column);
            let frame = Rect {
                x: side,
                y: top,
                width,
                height,
            };
            Laid {
                kind,
                frame,
                picture: picture_in(&shown, frame),
                controls: shown.shows.controls,
                changed,
                slept,
            }
        };
        self.players.insert(
            block.to_owned(),
            Player {
                shown,
                resting_at,
                laid: laid.clone(),
            },
        );
        self.laying.insert(row, block.to_owned());
        laid
    }

    /// Finishes a layout pass: drops players whose blocks were not laid out (`seen` lists those
    /// that were), putting the awake one to rest first. Returns the dropped blocks.
    pub fn forget_except(&mut self, seen: &[String]) -> Vec<String> {
        self.rows = std::mem::take(&mut self.laying);
        let gone: Vec<String> = self
            .players
            .keys()
            .filter(|block| !seen.contains(block))
            .cloned()
            .collect();
        for block in &gone {
            self.players.remove(block);
            if self.awake.as_ref() == Some(block) {
                self.awake = None;
            }
        }
        gone
    }

    /// The block whose player is awake.
    pub fn awake(&self) -> Option<&str> {
        self.awake.as_deref()
    }

    /// The id of the file `block`'s player plays; `None` when it plays nothing.
    pub fn file_of(&self, block: &str) -> Option<&str> {
        let (_, file) = self.players.get(block)?.shown.shows.plays.as_ref()?;
        Some(file)
    }

    /// Wakes `block`'s player once the previously awake one is at rest (`sleep`), if its file
    /// still exists (`file_there`). `None` when there is nothing to open.
    pub fn wake(&mut self, block: &str, file_there: bool) -> Option<Waking> {
        let player = self.players.get(block)?;
        if self.awake.is_some() || !file_there || player.shown.shows.plays.is_none() {
            return None;
        }
        self.awake = Some(block.to_owned());
        Some(Waking {
            video: player.shown.plays_video(),
            muted: player.shown.shows.muted,
            start_at: player.resting_at,
        })
    }

    /// Puts the awake player to rest at the preview's position (`position_seconds`), or at the
    /// start when it played to the end. Returns its block.
    pub fn sleep(&mut self, position_seconds: f64, ended: bool) -> Option<String> {
        let block = self.awake.take()?;
        if let Some(player) = self.players.get_mut(&block) {
            player.resting_at = if ended { 0.0 } else { position_seconds };
        }
        Some(block)
    }

    /// What the awake player needs, given whether the page is `visible` and the file has `ended`.
    pub fn follow(&self, visible: bool, ended: bool) -> Option<Follow> {
        let player = self.players.get(self.awake.as_ref()?)?;
        Some(if !visible {
            Follow::Sleep
        } else if player.shown.shows.looping && ended {
            Follow::PlayAgain
        } else {
            Follow::Draw
        })
    }

    /// What `block`'s controls show at rest: paused where it was left, with the speaker drawn
    /// (whether the file has sound is unknown until it opens, and most do). `None` while it is
    /// awake, or when the page has no such player.
    pub fn resting(&self, block: &str) -> Option<Playback> {
        if self.awake.as_deref() == Some(block) {
            return None;
        }
        let player = self.players.get(block)?;
        Some(Playback {
            playing: false,
            position_seconds: player.resting_at,
            duration_seconds: player.shown.duration,
            has_audio: true,
            muted: player.shown.shows.muted,
            volume: 1.0,
            fullscreen: false,
            has_played: false,
            menu: Menu::Closed,
            resting: true,
        })
    }

    /// One line of JSON for a check (`previewpage`): each player's block row, what it plays, its
    /// box and picture in window coordinates and whether it is awake; the preview's state and the
    /// sound it sent to the output; the selected blocks and what has keyboard focus. Read-only.
    pub(crate) fn report(&self, observed: &Observed) -> String {
        let (dx, dy) = observed.corner;
        let in_window = |rect: Rect| {
            [
                (rect.x + dx).round() as i64,
                (rect.y + dy).round() as i64,
                rect.width.round() as i64,
                rect.height.round() as i64,
            ]
        };
        let rows: Vec<RowReport> = self
            .rows
            .iter()
            .filter_map(|(row, block)| {
                let player = self.players.get(block)?;
                let shows = &player.shown.shows;
                let awake = self.awake.as_ref() == Some(block);
                Some(RowReport {
                    row: *row,
                    element: shows.element,
                    plays: shows
                        .plays
                        .as_ref()
                        .map_or("nothing", |(plays, _)| plays.name()),
                    words: &shows.words,
                    controls: player.laid.controls,
                    awake,
                    frame: in_window(player.laid.frame),
                    picture: in_window(player.laid.picture),
                    resting_at: player.resting_at,
                    autoplay: shows.autoplay,
                    looping: shows.looping,
                    muted: shows.muted,
                    layer: awake && player.laid.kind == Kind::Video,
                })
            })
            .collect();
        let awake_row = self
            .rows
            .iter()
            .find(|(_, block)| self.awake.as_ref() == Some(*block))
            .map_or(-1, |(row, _)| *row as i64);
        let preview = match (&self.awake, observed.preview) {
            (Some(_), Some(info)) => serde_json::json!({
                "playing": info.playing,
                "position": info.position_seconds,
                "duration": info.duration_seconds,
                "ended": info.ended,
                "muted": info.muted,
                "width": info.width,
                "height": info.height,
                "ready": info.ready,
                "volume": info.volume,
                "rate": info.rate,
                "menu": observed.menu.kind() as u32,
                "menuScroll": observed.menu.scroll(),
            }),
            _ => serde_json::json!({}),
        };
        serde_json::json!({
            "players": rows,
            "awake": awake_row,
            "preview": preview,
            "sound": { "frames": observed.sound_frames, "peak": observed.sound_peak, "output": observed.output },
            "chosen": observed.chosen.map_or(Vec::new(), |(start, end)| vec![start, end]),
            "editing": observed.editing,
            "keys": observed.keys,
            "blocks": observed.blocks,
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `PlayerShows` for what a check's report says a player plays.
    fn shows(plays: &'static str) -> PlayerShows {
        PlayerShows {
            element: if plays == "sound" {
                PlayerElement::Audio
            } else {
                PlayerElement::Video
            },
            plays: match plays {
                "video" => Some((Plays::Video, "file-1".to_owned())),
                "sound" => Some((Plays::Sound, "file-1".to_owned())),
                _ => None,
            },
            poster: String::new(),
            words: "clip.mp4".to_owned(),
            controls: true,
            looping: false,
            muted: false,
            autoplay: false,
            width: None,
            height: None,
        }
    }

    fn shown(plays: &'static str) -> Shown {
        Shown {
            shows: shows(plays),
            media_width: 1280.0,
            media_height: 720.0,
            duration: 4.0,
        }
    }

    #[test]
    fn a_player_is_as_large_as_a_browser_makes_it() {
        // The file's own size, scaled to the column with its aspect ratio.
        assert_eq!(player_box(&shown("video"), 384.0), (384.0, 216.0));
        assert_eq!(player_box(&shown("video"), 2000.0), (1280.0, 720.0));
        // One written side derives the other from the file's aspect ratio; both written are kept.
        let wide = Shown {
            shows: PlayerShows {
                width: Some(320.0),
                ..shows("video")
            },
            ..shown("video")
        };
        assert_eq!(player_box(&wide, 1000.0), (320.0, 180.0));
        let tall = Shown {
            shows: PlayerShows {
                height: Some(90.0),
                ..shows("video")
            },
            ..shown("video")
        };
        assert_eq!(player_box(&tall, 1000.0), (160.0, 90.0));
        let both = Shown {
            shows: PlayerShows {
                width: Some(240.0),
                height: Some(180.0),
                ..shows("video")
            },
            ..shown("video")
        };
        assert_eq!(player_box(&both, 1000.0), (240.0, 180.0));
        // A file of unknown size gets 300 by 150, rounded to the point.
        let unknown = Shown {
            media_width: 0.0,
            media_height: 0.0,
            ..shown("video")
        };
        assert_eq!(player_box(&unknown, 1000.0), (300.0, 150.0));
        assert_eq!(player_box(&unknown, 211.0), (211.0, 106.0));
        // A sound's pill: 300 by 54, or as wide as written, never wider than the column.
        assert_eq!(player_box(&shown("sound"), 1000.0), (300.0, 54.0));
        let written = Shown {
            shows: PlayerShows {
                width: Some(420.0),
                ..shows("sound")
            },
            ..shown("sound")
        };
        assert_eq!(player_box(&written, 384.5), (384.5, 54.0));
    }

    #[test]
    fn the_picture_sits_whole_in_the_middle_of_its_box() {
        let both = Shown {
            shows: PlayerShows {
                width: Some(240.0),
                height: Some(180.0),
                ..shows("video")
            },
            ..shown("video")
        };
        let frame = Rect {
            x: 77.0,
            y: 100.0,
            width: 240.0,
            height: 180.0,
        };
        assert_eq!(
            picture_in(&both, frame),
            Rect {
                x: 77.0,
                y: 122.5,
                width: 240.0,
                height: 135.0
            }
        );
        let unknown = Shown {
            media_width: 0.0,
            ..shown("video")
        };
        assert_eq!(picture_in(&unknown, frame), frame);
        assert_eq!(picture_in(&shown("sound"), frame), frame);
    }

    #[test]
    fn what_draws_as_a_player_and_what_as_its_words() {
        assert_eq!(shown("video").kind(), Kind::Video);
        let bare = Shown {
            shows: PlayerShows {
                controls: false,
                ..shows("video")
            },
            ..shown("video")
        };
        assert_eq!(bare.kind(), Kind::Video);
        assert_eq!(shown("sound").kind(), Kind::Sound);
        let quiet = Shown {
            shows: PlayerShows {
                controls: false,
                ..shows("sound")
            },
            ..shown("sound")
        };
        assert_eq!(quiet.kind(), Kind::Words);
        assert_eq!(shown("nothing").kind(), Kind::Words);
        let mut players = PagePlayers::default();
        let words = players.lay(5, "b5", shown("nothing"), 600.0, 75.0, 400.0);
        assert_eq!(
            words.frame,
            Rect {
                x: 77.0,
                y: 600.0,
                width: 396.0,
                height: WORDS_LINE
            }
        );
        assert!(!words.controls);
        assert!(!players.lay(1, "b1", bare, 0.0, 75.0, 400.0).controls);
    }

    #[test]
    fn the_report_names_the_element_and_what_it_plays() {
        let mut players = PagePlayers::default();
        players.lay(1, "clip", shown("video"), 100.0, 77.0, 384.0);
        players.lay(2, "tone", shown("sound"), 330.0, 77.0, 384.0);
        players.lay(3, "gone", shown("nothing"), 400.0, 77.0, 384.0);
        players.forget_except(&["clip".to_owned(), "tone".to_owned(), "gone".to_owned()]);
        let report: serde_json::Value =
            serde_json::from_str(&players.report(&Observed::default())).expect("json");
        let said: Vec<(&str, &str)> = report["players"]
            .as_array()
            .expect("players")
            .iter()
            .map(|row| {
                (
                    row["element"].as_str().expect("element"),
                    row["plays"].as_str().expect("plays"),
                )
            })
            .collect();
        assert_eq!(
            said,
            [("video", "video"), ("audio", "sound"), ("video", "nothing")]
        );
    }

    #[test]
    fn one_player_is_awake_and_the_one_before_rests_where_it_was() {
        let mut players = PagePlayers::default();
        players.lay(1, "clip", shown("video"), 100.0, 77.0, 384.0);
        players.lay(2, "tone", shown("sound"), 330.0, 77.0, 384.0);
        players.forget_except(&["clip".to_owned(), "tone".to_owned()]);
        assert!(players
            .resting("clip")
            .is_some_and(|rest| rest.resting && rest.position_seconds == 0.0 && rest.has_audio));
        let waking = players.wake("clip", true).expect("opens");
        assert_eq!(
            waking,
            Waking {
                video: true,
                muted: false,
                start_at: 0.0
            }
        );
        assert_eq!(players.file_of("clip"), Some("file-1"));
        assert_eq!(players.resting("clip"), None);
        assert_eq!(players.awake(), Some("clip"));
        // Another wakes only once this one is at rest; it rests where it was.
        assert_eq!(players.wake("tone", true), None);
        assert_eq!(players.sleep(3.02, false), Some("clip".to_owned()));
        assert_eq!(
            players.resting("clip").map(|rest| rest.position_seconds),
            Some(3.02)
        );
        assert_eq!(
            players.wake("tone", true).map(|waking| waking.video),
            Some(false)
        );
        // Played to its end, it rests at the start.
        assert_eq!(players.sleep(4.0, true), Some("tone".to_owned()));
        assert_eq!(
            players.resting("tone").map(|rest| rest.position_seconds),
            Some(0.0)
        );
        assert_eq!(players.sleep(1.0, false), None);
        // Woken again, it opens where it was left; a missing file opens nothing.
        assert_eq!(
            players.wake("clip", true).map(|waking| waking.start_at),
            Some(3.02)
        );
        players.sleep(3.02, false);
        assert_eq!(players.wake("clip", false), None);
        assert_eq!(players.awake(), None);
    }

    #[test]
    fn a_player_whose_file_changes_starts_again_at_rest() {
        let mut players = PagePlayers::default();
        players.lay(1, "clip", shown("video"), 100.0, 77.0, 384.0);
        players.wake("clip", true);
        // Laid out again unchanged, it stays awake.
        let same = players.lay(1, "clip", shown("video"), 120.0, 77.0, 384.0);
        assert!(!same.changed && !same.slept);
        assert_eq!(players.awake(), Some("clip"));
        players.sleep(2.0, false);
        players.wake("clip", true);
        let muted = Shown {
            shows: PlayerShows {
                muted: true,
                ..shows("video")
            },
            ..shown("video")
        };
        let changed = players.lay(1, "clip", muted, 120.0, 77.0, 384.0);
        assert!(changed.changed && changed.slept);
        assert_eq!(players.awake(), None);
        assert!(players
            .resting("clip")
            .is_some_and(|rest| rest.position_seconds == 0.0 && rest.muted));
    }

    #[test]
    fn blocks_gone_from_the_page_let_their_players_go() {
        let mut players = PagePlayers::default();
        players.lay(1, "clip", shown("video"), 100.0, 77.0, 384.0);
        players.lay(2, "tone", shown("sound"), 330.0, 77.0, 384.0);
        players.forget_except(&["clip".to_owned(), "tone".to_owned()]);
        players.wake("tone", true);
        players.lay(1, "clip", shown("video"), 100.0, 77.0, 384.0);
        assert_eq!(
            players.forget_except(&["clip".to_owned()]),
            vec!["tone".to_owned()]
        );
        assert_eq!(players.awake(), None);
        assert_eq!(players.resting("tone"), None);
        // A page that cannot be typed in has no players at all.
        assert_eq!(players.forget_except(&[]), vec!["clip".to_owned()]);
        assert!(players
            .report(&Observed::default())
            .contains("\"players\":[]"));
    }

    #[test]
    fn the_player_awake_rests_when_the_page_goes_and_a_loop_plays_again() {
        let mut players = PagePlayers::default();
        assert_eq!(players.follow(true, false), None);
        let looping = Shown {
            shows: PlayerShows {
                looping: true,
                ..shows("video")
            },
            ..shown("video")
        };
        players.lay(1, "clip", looping, 100.0, 77.0, 384.0);
        players.wake("clip", true);
        assert_eq!(players.follow(true, false), Some(Follow::Draw));
        assert_eq!(players.follow(true, true), Some(Follow::PlayAgain));
        assert_eq!(players.follow(false, false), Some(Follow::Sleep));
    }

    #[test]
    fn the_report_reads_the_players_as_laid_out_in_the_window() {
        let mut players = PagePlayers::default();
        players.lay(1, "clip", shown("video"), 105.0, 77.0, 384.0);
        players.lay(5, "gone", shown("nothing"), 700.0, 75.0, 400.0);
        players.forget_except(&["clip".to_owned(), "gone".to_owned()]);
        players.wake("clip", true);
        let info = crate::preview::PreviewInfo {
            playing: true,
            position_seconds: 1.5,
            duration_seconds: 4.0,
            width: 1280,
            height: 720,
            ready: true,
            volume: 0.25,
            rate: 1.5,
            ..Default::default()
        };
        let menu = Menu::Speeds {
            focus: 6,
            scroll: 140.0,
        };
        let observed = Observed {
            corner: (0.0, 64.0),
            preview: Some(info),
            menu,
            output: true,
            chosen: Some((1, 1)),
            editing: -1,
            keys: "PlayerControlsView".into(),
            blocks: 9,
            ..Default::default()
        };
        let said: serde_json::Value =
            serde_json::from_str(&players.report(&observed)).expect("json");
        assert_eq!(said["awake"], 1);
        assert_eq!(
            said["players"][0]["box"],
            serde_json::json!([77, 169, 384, 216])
        );
        assert_eq!(said["players"][0]["layer"], true);
        assert_eq!(said["players"][1]["row"], 5);
        assert_eq!(
            said["players"][1]["box"],
            serde_json::json!([77, 764, 396, 21])
        );
        assert_eq!(said["preview"]["position"], 1.5);
        assert_eq!(
            (
                said["preview"]["volume"].as_f64(),
                said["preview"]["rate"].as_f64()
            ),
            (Some(0.25), Some(1.5))
        );
        assert_eq!(
            (
                said["preview"]["menu"].as_u64(),
                said["preview"]["menuScroll"].as_f64()
            ),
            (Some(2), Some(140.0))
        );
        assert_eq!(said["chosen"], serde_json::json!([1, 1]));
        assert_eq!(
            (said["blocks"].as_u64(), said["sound"]["output"].as_bool()),
            (Some(9), Some(true))
        );
        players.sleep(1.5, false);
        let resting: serde_json::Value =
            serde_json::from_str(&players.report(&observed)).expect("json");
        assert_eq!(
            (
                resting["awake"].as_i64(),
                resting["preview"].as_object().map(|it| it.len())
            ),
            (Some(-1), Some(0))
        );
        assert_eq!(resting["players"][0]["restingAt"], 1.5);
    }
}
