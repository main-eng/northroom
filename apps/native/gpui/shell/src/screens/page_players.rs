//! The `<video>` and `<audio>` elements in the open page: their layout from the core, their
//! controls (`controls`) and, for the awake video, the layer the preview draws on
//! (`preview::VideoLayer`). The core decides what each shows and plays, where it rests and
//! which one is awake (`nr_page_player_*`, `page/players.rs`). One plays at a time, in the
//! core's preview.
//!
//! The page holds one `PagePlayers` and drives it as it lays out and draws:
//!
//! - Layout: `begin`, then `lay` for each player block from the core's layout callback (it
//!   returns where the block's space ends, its box and whether it draws as a player), then
//!   `end`.
//! - Each frame: `set_place` with the page's top-left in the window (it moves as the page
//!   scrolls) and the visible part of the window.
//! - Drawing: the `PagePlayers` view in an element at the window origin, above the page's
//!   blocks. It draws resting players' posters and text and every player's controls.
//! - `set_state`, once: what a check's report needs (the selected blocks, the block being
//!   edited, what has the keys).
//! - When the page closes it drops the view, which puts the awake player to rest.
//!
//! A press on a player's picture outside its controls goes to the page (it selects the block).
//! A press or key on the controls of a resting player wakes it.

use super::controls::{Controls, Style, Wake};
use super::preview::{self, VideoLayer};
use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, IntoElement, ParentElement, Pixels,
    Point, Render, Styled, Task, WeakEntity, Window, canvas, div,
};
use kit::core::{c, rect, text};
use kit::nr::*;
use kit::theme::Token;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

/// What in a page has keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyHolder {
    /// The text field being edited.
    Field,
    /// The page itself, with its selected blocks.
    Page,
    /// A player's controls, because the last press was on them.
    PlayerControls,
}

impl KeyHolder {
    /// The name a check reads (`ev:observe`, `previewpage`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Field => "PageTextView",
            Self::Page => "PageContentView",
            Self::PlayerControls => "PlayerControlsView",
        }
    }
}

/// What in the page has the keys in `window`: its field, else the page, else a player's
/// controls (checked with `controls` only then). `None` when something else has them.
pub fn key_holder(
    window: &Window,
    field: &FocusHandle,
    page: &FocusHandle,
    controls: impl FnOnce() -> bool,
) -> Option<KeyHolder> {
    if field.is_focused(window) {
        Some(KeyHolder::Field)
    } else if page.is_focused(window) {
        Some(KeyHolder::Page)
    } else if controls() {
        Some(KeyHolder::PlayerControls)
    } else {
        None
    }
}

/// What a check's report needs from the page: the selected blocks (start and end, or none), the
/// block being edited and what has the keys.
pub struct PageState {
    pub chosen: Option<(i64, i64)>,
    pub editing: Option<i64>,
    pub keys: Option<KeyHolder>,
}

/// Reads the page's state for a check's report. Takes the window and a test for whether a
/// player's controls have the keys, or `None` to read the page as last drawn.
pub type ReadPageState = Rc<dyn Fn(Option<(&Window, &dyn Fn() -> bool)>, &App) -> PageState>;

/// A player block as last laid out, in page points.
struct Player {
    block: String,
    kind: NrPlayerKind,
    /// The element's box (its text line), and the picture's place inside it.
    frame: Bounds<Pixels>,
    picture: Bounds<Pixels>,
    words: String,
    file: String,
    poster: String,
    controls: Option<Entity<Controls>>,
}

thread_local! {
    /// The open page's players, for a check's `previewpage`.
    static SHOWN: RefCell<Option<WeakEntity<PagePlayers>>> = const { RefCell::new(None) };
}

pub struct PagePlayers {
    /// Keyed by block id and kept across layouts, so a playing player keeps playing while the
    /// page around it changes.
    players: HashMap<String, Player>,
    seen: HashSet<String>,
    /// The awake video player's layer, with its block id.
    layer: Option<(String, VideoLayer)>,
    /// Tracks the awake player (`nr_page_player_follow`).
    follow: Option<Task<()>>,
    /// The page's top-left in the window, and the visible part of the window.
    origin: Point<Pixels>,
    visible: Bounds<Pixels>,
    /// What the page draws over the players (its field and `/` menu), in window points.
    covered: Vec<Bounds<Pixels>>,
    state: Option<ReadPageState>,
    tracing: bool,
}

impl PagePlayers {
    pub fn new(cx: &mut Context<Self>) -> Self {
        SHOWN.with(|shown| *shown.borrow_mut() = Some(cx.entity().downgrade()));
        Self {
            players: HashMap::new(),
            seen: HashSet::new(),
            layer: None,
            follow: None,
            origin: Point::default(),
            visible: Bounds::default(),
            covered: Vec::new(),
            state: None,
            tracing: nr_page_tracing(),
        }
    }

    pub fn set_state(&mut self, state: ReadPageState) {
        self.state = Some(state);
    }

    /// Whether a player's controls have the keys, because the last press was on them.
    pub fn controls_have_keys(&self, window: &Window, cx: &App) -> bool {
        self.players
            .values()
            .filter_map(|player| player.controls.as_ref())
            .any(|controls| controls.read(cx).has_keys(window))
    }

    /// Sets the page's top-left in the window and the visible part of the window.
    pub fn set_place(
        &mut self,
        origin: Point<Pixels>,
        visible: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.origin != origin || self.visible != visible {
            self.origin = origin;
            self.visible = visible;
            self.place_controls(cx);
            cx.notify();
        }
    }

    /// Sets what the page draws over the players; their controls ignore presses there.
    pub fn set_covered(&mut self, covered: Vec<Bounds<Pixels>>, cx: &mut Context<Self>) {
        if self.covered != covered {
            self.covered = covered;
            self.place_controls(cx);
        }
    }

    /// Starts a new layout pass; players are laid out again as their blocks arrive.
    pub fn begin(&mut self) {
        self.seen.clear();
    }

    /// Lays out player block `row` with its top at `top` in a column `column` wide starting at
    /// `side` (`nr_page_player_lay`). Returns where the block's space ends, its box and whether
    /// it draws as a player. A player with nothing to play (an address outside the drive, a
    /// missing file) shows its text small and muted, like a missing image. A block whose element
    /// can't be parsed gets no player, only its space.
    pub fn lay(
        &mut self,
        row: usize,
        top: f64,
        side: f64,
        column: f64,
        cx: &mut Context<Self>,
    ) -> NrPagePlayerPlaced {
        let laid = nr_page_player_lay(row, top, side, column);
        if laid.kind == NrPlayerKind::None {
            return NrPagePlayerPlaced {
                end: top + 24.0,
                frame: NrRect {
                    x: side,
                    y: top,
                    width: 0.0,
                    height: 24.0,
                },
                drawn: false,
            };
        }
        let block = text(&laid.block);
        if laid.slept {
            self.rested(cx);
        }
        let mut player = self.players.remove(&block).unwrap_or_else(|| Player {
            block: block.clone(),
            kind: laid.kind,
            frame: Bounds::default(),
            picture: Bounds::default(),
            words: String::new(),
            file: String::new(),
            poster: String::new(),
            controls: None,
        });
        if laid.changed {
            // Its source changed, so it restarts at rest with new controls.
            player.controls = None;
        }
        player.kind = laid.kind;
        player.frame = rect(&laid.frame);
        player.picture = rect(&laid.picture);
        player.words = text(&laid.words);
        player.file = text(&laid.file);
        player.poster = text(&laid.poster);
        if laid.controls && player.controls.is_none() {
            let style = if laid.kind == NrPlayerKind::Video {
                Style::Video
            } else {
                Style::Sound
            };
            let file = c(&player.file);
            // SAFETY: a NUL-terminated id.
            let bytes = unsafe { nr_entry_size(file.as_ptr()) };
            // SAFETY: as above.
            let path = text(&unsafe { nr_entry_path(file.as_ptr()) });
            let players = cx.entity().downgrade();
            let waking = block.clone();
            let wake: Wake = Rc::new(move |window, cx| {
                let _ = players.update(cx, |players, cx| players.wake(&waking, window, cx));
            });
            player.controls = Some(cx.new(|cx| {
                let mut controls = Controls::new(style, bytes, &block, &path, cx);
                controls.wake = Some(wake);
                controls
            }));
        } else if !laid.controls {
            player.controls = None;
        }
        let placed = NrPagePlayerPlaced {
            end: f64::from(player.frame.origin.y + player.frame.size.height),
            frame: kit::core::nr_rect(player.frame),
            drawn: player.kind != NrPlayerKind::Words,
        };
        self.seen.insert(block.clone());
        self.players.insert(block, player);
        self.place_controls(cx);
        cx.notify();
        placed
    }

    /// Ends the layout pass and drops players whose blocks are gone (`nr_page_players_forget`).
    pub fn end(&mut self, cx: &mut Context<Self>) {
        let awake = text(&nr_page_player_awake());
        let seen: Vec<&str> = self.seen.iter().map(String::as_str).collect();
        let seen = c(&seen.join("\n"));
        // SAFETY: a NUL-terminated list.
        let gone = text(&unsafe { nr_page_players_forget(seen.as_ptr()) });
        for block in gone.lines() {
            if self.players.remove(block).is_some() && block == awake {
                self.rested(cx);
            }
        }
        cx.notify();
    }

    fn place_controls(&mut self, cx: &mut Context<Self>) {
        let origin = self.origin;
        for player in self.players.values() {
            if let Some(controls) = &player.controls {
                let frame =
                    kit::core::offset(player.frame, f32::from(origin.x), f32::from(origin.y));
                let covered = self.covered.clone();
                controls.update(cx, |controls, cx| {
                    controls.place(frame, cx);
                    controls.covered = covered;
                });
            }
        }
    }

    /// Makes `block`'s file the preview's. The previously awake player rests where it was
    /// (`nr_page_player_wake`).
    fn wake(&mut self, block: &str, window: &mut Window, cx: &mut Context<Self>) {
        if text(&nr_page_player_awake()) == block || !self.players.contains_key(block) {
            return;
        }
        self.sleep_awake(cx);
        let named = c(block);
        // SAFETY: a NUL-terminated block.
        let woke = unsafe { nr_page_player_wake(named.as_ptr()) };
        if !woke.opened {
            return;
        }
        if woke.video {
            self.layer = Some((block.to_owned(), VideoLayer::new(window)));
        }
        self.follow = Some(cx.spawn(async move |players, cx| {
            let every = Duration::from_secs_f64(nr_frame_seconds(NrFrame::Follow));
            loop {
                cx.background_executor().timer(every).await;
                if players
                    .update(cx, |players, cx| players.follow_awake(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        self.trace("wake", cx);
        cx.notify();
    }

    /// While a player is awake, its sound runs only while it plays (`nr_page_player_follow`).
    fn follow_awake(&mut self, cx: &mut Context<Self>) {
        match nr_page_player_follow(true) {
            NrPlayerFollow::Sleep => self.sleep_awake(cx),
            NrPlayerFollow::Draw => {
                preview::follow_sound();
                cx.notify();
            }
            NrPlayerFollow::Nothing => {}
        }
    }

    /// Puts the awake player back to rest where it was and closes the preview.
    pub fn sleep_awake(&mut self, cx: &mut Context<Self>) {
        let block = text(&nr_page_player_sleep());
        if self.players.contains_key(&block) {
            self.rested(cx);
        }
    }

    /// Releases what a player held once it rests: its follow task, the sound output and its
    /// layer.
    fn rested(&mut self, cx: &mut Context<Self>) {
        self.follow = None;
        preview::stop_sound();
        self.layer = None;
        // Redraw the window and the controls (one may be the pressed one).
        self.trace("rest", cx);
        cx.notify();
    }

    fn trace(&self, step: &str, cx: &App) {
        if self.tracing {
            eprintln!("player\t{step}\t{}", self.said(None, cx));
        }
    }

    /// What the page's players show and do, as the core reports them
    /// (`nr_page_players_report`). Focus comes from `window` when given, else from the last
    /// draw. Read-only.
    fn said(&self, window: Option<&Window>, cx: &App) -> String {
        let state = match &self.state {
            Some(state) => match window {
                Some(window) => state(Some((window, &|| self.controls_have_keys(window, cx))), cx),
                None => state(None, cx),
            },
            None => PageState {
                chosen: None,
                editing: None,
                keys: None,
            },
        };
        let keys = c(state.keys.map_or("none", KeyHolder::name));
        let (start, end) = state.chosen.unwrap_or((-1, -1));
        // SAFETY: a NUL-terminated name.
        text(&unsafe {
            nr_page_players_report(
                f64::from(self.origin.x),
                f64::from(self.origin.y),
                preview::sound_running(),
                start,
                end,
                // -1: no block is being edited.
                state.editing.unwrap_or(-1),
                keys.as_ptr(),
            )
        })
    }

    fn paint(&mut self, window: &mut Window, cx: &mut App) {
        let (dx, dy) = (f32::from(self.origin.x), f32::from(self.origin.y));
        let awake = self.layer.as_ref().map(|(block, _)| block.clone());
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: self.visible,
            }),
            |window| {
                for player in self.players.values() {
                    let frame = kit::core::offset(player.frame, dx, dy);
                    match player.kind {
                        NrPlayerKind::Words => kit::text::draw_line(
                            window,
                            cx,
                            &player.words,
                            kit::theme::Face::sans(13.0, 400.0),
                            kit::theme::color(Token::MutedForeground),
                            f32::from(frame.origin.x),
                            f32::from(frame.origin.y),
                            f32::from(frame.size.height),
                            f32::from(frame.size.width),
                            None,
                        ),
                        NrPlayerKind::Video if awake.as_deref() != Some(&player.block) => {
                            let picture = kit::core::offset(player.picture, dx, dy);
                            paint_video_at_rest(window, player, picture);
                        }
                        _ => {}
                    }
                }
            },
        );
        // The awake player, where the core renders into an image (off macOS).
        if let Some((_, layer)) = self.layer.as_mut() {
            layer.paint(window);
        }
    }

    fn place_layer(&mut self, window: &Window) {
        let (dx, dy) = (f32::from(self.origin.x), f32::from(self.origin.y));
        let Some((block, layer)) = self.layer.as_mut() else {
            return;
        };
        if let Some(player) = self.players.get(block.as_str()) {
            let picture = kit::core::offset(player.picture, dx, dy).intersect(&self.visible);
            layer.place(picture, window);
        }
    }
}

/// Paints a resting video in `picture`: the poster, else the first frame, on black until
/// loaded.
fn paint_video_at_rest(window: &mut Window, player: &Player, picture: Bounds<Pixels>) {
    kit::draw::fill(window, picture, gpui::black());
    let still = if player.poster.is_empty() {
        kit::lists::thumbs::picture(&player.file, 1280, true)
    } else {
        kit::lists::thumbs::picture(&player.poster, 1280, false)
    };
    if let Some(still) = still {
        let _ = window.paint_image(picture, picture, Default::default(), still, 0, false);
    }
}

impl Drop for PagePlayers {
    fn drop(&mut self) {
        // Leaving the page stops its playback. The sound output follows whatever the preview
        // holds now, which may be a file opened since (`nr_preview_close`).
        if !text(&nr_page_player_sleep()).is_empty() {
            preview::follow_sound();
        }
    }
}

impl Render for PagePlayers {
    /// Draws in window points, in an element the page puts at the window origin.
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let players = cx.entity();
        let painted = cx.entity();
        let controls: Vec<Entity<Controls>> = self
            .players
            .values()
            .filter_map(|player| player.controls.clone())
            .collect();
        div()
            .absolute()
            .child(
                canvas(
                    move |_, window, cx| {
                        players.update(cx, |players, _| players.place_layer(window))
                    },
                    move |_, _, window, cx| {
                        painted.update(cx, |players, cx| players.paint(window, cx))
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(controls)
    }
}

/// Reports what the open page's players show and do as one line (`players\t<JSON>`), or `{}`
/// with no page open. Draws a frame first, like an `at:` step, because the page lays out its
/// players only while drawing and a covered check window may not have drawn since it opened.
pub fn report(window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
    let shown = SHOWN.with(|shown| shown.borrow().as_ref().and_then(WeakEntity::upgrade));
    let said = shown.map_or_else(
        || "{}".to_owned(),
        |players| players.read(cx).said(Some(window), cx),
    );
    eprintln!("players\t{said}");
}
