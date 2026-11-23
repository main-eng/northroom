//! The core's video and sound preview. The video draws on a layer the core presents to from its
//! own thread (`VideoLayer`, a platform view under the window's, seen through a hole via
//! `kit::hosted`), with the controls on top (`controls`). Sound goes to the system output while
//! the core wants it (`sound::SoundOutput`). The core decodes, keeps the clock and owns all state
//! (`nr_preview_*`).
//!
//! The details pane plays its file in a `Player` placed where its layout puts the media. A
//! page's players (`page_players`) share the layer, controls and sound.
//!
//! The core runs the check steps (`NrScriptKind::Preview`: `previewplay`, `previewstats`,
//! `previewpage`, …). The sound output then catches up, and `previewpage` reports what the
//! open page's players show (`page_players::report`).

use super::controls::{Controls, Style};
use super::sound::SoundOutput;
use crate::script::{Answer, Pointer, Route, Step, Table};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Pixels, Render, Styled,
    Task, Window, canvas,
};
use kit::controls::placed;
use kit::core::{c, text, zeroed};
use kit::hosted::HostedView;
#[cfg(target_os = "macos")]
use kit::hosted::Picture;
use kit::nr::*;
use std::cell::{Cell, RefCell};
#[cfg(target_os = "macos")]
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub fn register(table: &mut Table) {
    table.answer(Route::Kind(NrScriptKind::Preview), step);
}

fn step(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    if step.name == "previewpage" {
        super::page_players::report(window, cx);
    }
    follow_sound();
    Answer::Done
}

// MARK: Sound

thread_local! {
    /// The preview's sound output, while the core wants it.
    static SOUND: RefCell<Option<SoundOutput>> = const { RefCell::new(None) };
    /// The video's full-screen state.
    static FULLSCREEN: Cell<Fullscreen> = const { Cell::new(Fullscreen::Off) };
    /// The last drawn playing player, which fills the screen when the video does.
    static SHOWN: RefCell<Option<gpui::WeakEntity<Player>>> = const { RefCell::new(None) };
    /// The player was dropped while it had the window in full screen; the window leaves full
    /// screen on its next frame (`give_back_window`).
    static WINDOW_OWED: Cell<bool> = const { Cell::new(false) };
}

/// The video's full-screen state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fullscreen {
    Off,
    /// Requested. The window is going full screen and already shows the video alone, so the
    /// system transition starts from it.
    Entering,
    /// The window is full screen. Leaving it any other way (the green button, ⌃⌘F) also ends
    /// the video's full screen. `took_window` means the video put the window into full screen
    /// and takes it out on leaving; a window that was already full screen stays so.
    On {
        took_window: bool,
    },
}

impl Fullscreen {
    /// Whether the video put, or is putting, the window into full screen.
    fn took_window(self) -> bool {
        matches!(self, Self::Entering | Self::On { took_window: true })
    }
}

fn fullscreen() -> Fullscreen {
    FULLSCREEN.with(Cell::get)
}

fn set_fullscreen(now: Fullscreen) {
    FULLSCREEN.with(|fullscreen| fullscreen.set(now));
}

/// The player whose video fills the screen, if any. The window then shows it alone, because
/// anything else drawn would cover the video, which sits under the window (`kit::hosted`).
pub fn filling_screen(_: &App) -> Option<gpui::Entity<Player>> {
    if fullscreen() == Fullscreen::Off {
        return None;
    }
    SHOWN
        .with(|shown| shown.borrow().clone())
        .and_then(|player| player.upgrade())
}

/// Tracks the window's full screen. Once in, the state is `On`; if the window leaves while `On`
/// (through the system), the video leaves full screen too.
fn follow_window(window: &mut Window) {
    match (fullscreen(), window.is_fullscreen()) {
        (Fullscreen::Entering, true) => set_fullscreen(Fullscreen::On { took_window: true }),
        (Fullscreen::On { .. }, false) => {
            nr_preview_leave_fullscreen();
            set_fullscreen(Fullscreen::Off);
            window.refresh();
        }
        (Fullscreen::Off, _) | (Fullscreen::Entering, false) | (Fullscreen::On { .. }, true) => {}
    }
}

/// Starts the output when the core wants it (`output_wanted`) and drops it otherwise.
pub fn follow_sound() {
    let wanted = nr_preview_info().output_wanted;
    SOUND.with(|sound| {
        let mut sound = sound.borrow_mut();
        if wanted && sound.is_none() {
            let mut output = SoundOutput::new(Arc::new(|out: &mut [f32]| {
                let frames = out.len() / channels();
                // SAFETY: `out` holds `frames` frames of the output's channels.
                unsafe { nr_preview_audio_render(out.as_mut_ptr(), frames as u32) };
            }));
            output.follow(true);
            *sound = Some(output);
        } else if !wanted {
            *sound = None;
        }
    });
}

fn channels() -> usize {
    northroom_core::preview::sound_output::nr_sound_output().channels as usize
}

pub fn stop_sound() {
    SOUND.with(|sound| *sound.borrow_mut() = None);
}

/// Whether the output is running, for a check.
pub fn sound_running() -> bool {
    SOUND.with(|sound| sound.borrow().as_ref().is_some_and(SoundOutput::running))
}

// MARK: Platform actions

/// Saves a copy of the open file where the person chooses (the menu's Download). Never runs
/// unattended, since nobody can answer the panel (`nr_preview_download_asks`).
pub fn save_copy(cx: &mut App) {
    let path = text(&nr_preview_path());
    if path.is_empty() || !nr_preview_download_asks() {
        return;
    }
    let file = std::path::Path::new(&path);
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let folder = file
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let answer = cx.prompt_for_new_path(&folder, name.as_deref());
    cx.spawn(async move |_| {
        let Ok(Ok(Some(target))) = answer.await else {
            return;
        };
        let target = c(&target.to_string_lossy());
        // SAFETY: a NUL-terminated path.
        let failed = text(&unsafe { nr_preview_save_copy(target.as_ptr()) });
        if !failed.is_empty() {
            eprintln!("the copy was not saved: {failed}");
        }
    })
    .detach();
}

/// Enters or leaves full-screen video to match the core (`nr_preview_info().fullscreen`), after
/// the full-screen button, a double click or Escape. The window goes full screen and the player
/// covers it. An unattended run never covers the screen and only reports what it would do.
pub fn follow_fullscreen(window: &mut Window) {
    let wanted = nr_preview_info().fullscreen;
    match (wanted, fullscreen()) {
        (true, Fullscreen::Off) => enter_fullscreen(window),
        (false, Fullscreen::Entering | Fullscreen::On { .. }) => leave_fullscreen(window),
        (true, Fullscreen::Entering | Fullscreen::On { .. }) | (false, Fullscreen::Off) => {}
    }
}

/// Draws the video alone first so the system transition starts from that frame and nothing
/// else shows as the window grows. A window already in full screen is left alone and stays so
/// when the video leaves.
fn enter_fullscreen(window: &mut Window) {
    if nr_launch().unattended {
        eprintln!("video: full screen asked for; an unattended run does not cover the screen");
        return;
    }
    if window.is_fullscreen() {
        set_fullscreen(Fullscreen::On { took_window: false });
        window.refresh();
        return;
    }
    set_fullscreen(Fullscreen::Entering);
    window.refresh();
    window.on_next_frame(|window, _| {
        // Left before this frame (an immediate Escape); keep the window as it is.
        if fullscreen() == Fullscreen::Entering && !window.is_fullscreen() {
            window.toggle_fullscreen();
        }
    });
}

fn leave_fullscreen(window: &mut Window) {
    let took_window = fullscreen().took_window();
    set_fullscreen(Fullscreen::Off);
    if took_window && window.is_fullscreen() {
        window.toggle_fullscreen();
    }
    window.refresh();
}

/// Takes the window out of full screen if a player that put it there was dropped while full
/// screen (`WINDOW_OWED`). Called every window frame.
pub fn give_back_window(window: &mut Window) {
    if WINDOW_OWED.with(|owed| owed.replace(false)) && window.is_fullscreen() {
        window.toggle_fullscreen();
    }
}

// MARK: Video

/// The frame the core last wrote for a check (`nr_script_preview_picture`), for a window
/// screenshot. The core draws video straight to its layer, which GPUI's frame doesn't include.
/// `None` until a frame at `pixels`, the layer's size, exists; a different size is a stale
/// frame (`script::picture` waits for it).
#[cfg(target_os = "macos")]
fn written_frame(pixels: (u32, u32)) -> Option<Picture> {
    let mut written: NrScriptPicture = zeroed();
    // SAFETY: storage for one picture.
    if !unsafe { nr_script_preview_picture(&mut written) }
        || (written.width as u32, written.height as u32) != pixels
    {
        return None;
    }
    let length = written.width * written.height * 3;
    // SAFETY: the core lends `width × height` RGB pixels until its next call.
    let rgb = unsafe { std::slice::from_raw_parts(written.bytes, length) }.to_vec();
    Some(Picture {
        width: written.width as u32,
        height: written.height as u32,
        rgb,
    })
}

/// The layer the core presents video to, under the window's view at the video's place. The core
/// gets its size in pixels (`nr_preview_attach`, then `nr_preview_resize`). Off macOS no
/// platform view is hosted (`kit::hosted`); the core renders into an image of that size, which
/// this draws where the layer would be (`paint`).
pub struct VideoLayer {
    hosted: HostedView,
    attached: Option<(u32, u32)>,
    /// Where the video was last placed, in window points.
    #[cfg(not(target_os = "macos"))]
    placed: Bounds<Pixels>,
    /// The number of the core's last drawn frame.
    #[cfg(not(target_os = "macos"))]
    drawn: Option<(u64, Arc<gpui::RenderImage>)>,
}

impl VideoLayer {
    pub fn new(window: &mut Window) -> Self {
        Self {
            hosted: HostedView::metal(window, [0.0, 0.0, 0.0]),
            attached: None,
            #[cfg(not(target_os = "macos"))]
            placed: Bounds::default(),
            #[cfg(not(target_os = "macos"))]
            drawn: None,
        }
    }

    /// Shows the video at `frame` (window points) this frame: cuts the hole, places the layer
    /// under it and tells the core its size. Called during prepaint.
    pub fn place(&mut self, frame: Bounds<Pixels>, window: &Window) {
        let scale = window.scale_factor();
        let (width, height) = (
            f32::from(frame.size.width) * scale,
            f32::from(frame.size.height) * scale,
        );
        let pixels = ((width as u32).max(1), (height as u32).max(1));
        #[cfg(target_os = "macos")]
        kit::hosted::cut(frame, Some(Rc::new(move || written_frame(pixels))));
        #[cfg(not(target_os = "macos"))]
        {
            self.placed = frame;
        }
        self.hosted.place(frame, window);
        // On macOS there is no layer until the view exists.
        if width <= 1.0
            || height <= 1.0
            || (cfg!(target_os = "macos") && self.hosted.layer().is_null())
        {
            return;
        }
        match self.attached {
            None => {
                // SAFETY: a live `CAMetalLayer`, kept until after `nr_preview_stop` (the owner
                // stops the preview before dropping this).
                unsafe { nr_preview_attach(self.hosted.layer(), pixels.0, pixels.1) };
                self.attached = Some(pixels);
            }
            Some(attached) if attached != pixels => {
                nr_preview_resize(pixels.0, pixels.1);
                self.attached = Some(pixels);
            }
            Some(_) => {}
        }
    }

    /// Draws the core's last frame at the video's place, on black until one exists. Called
    /// during paint; on macOS the layer shows it instead.
    #[cfg_attr(target_os = "macos", allow(unused_variables))]
    pub fn paint(&mut self, window: &mut Window) {
        #[cfg(not(target_os = "macos"))]
        {
            kit::draw::fill(window, self.placed, gpui::black());
            let mut lent: NrPreviewPicture = zeroed();
            // SAFETY: storage for one picture.
            if unsafe { nr_preview_picture(&mut lent) }
                && self
                    .drawn
                    .as_ref()
                    .is_none_or(|(number, _)| *number != lent.number)
            {
                let length = lent.width as usize * lent.height as usize * 4;
                // SAFETY: the core lends `width × height` BGRA pixels until its next call.
                let bgra = unsafe { std::slice::from_raw_parts(lent.bgra, length) }.to_vec();
                if let Some(image) = kit::display::image(lent.width, lent.height, bgra)
                    && let Some((_, old)) = self.drawn.replace((lent.number, image))
                {
                    let _ = window.drop_image(old);
                }
            }
            if let Some((_, image)) = &self.drawn {
                let frame = self.placed;
                let _ =
                    window.paint_image(frame, frame, Default::default(), image.clone(), 0, false);
            }
        }
    }
}

/// A file playing in the details pane: a video on its layer with controls on top, or a sound
/// with controls alone. Opening it replaces whatever the core's preview was playing; dropping it
/// stops the preview.
pub struct Player {
    controls: Entity<Controls>,
    layer: Option<VideoLayer>,
    /// Where the pane puts the media, in window points.
    frame: Bounds<Pixels>,
    /// The video's own size once it opens.
    video_size: Option<(f64, f64)>,
    /// The preview's number for the open file, used to close it (`nr_preview_close`).
    number: u64,
    _follow: Task<()>,
}

impl Player {
    /// Opens `path`, `file_bytes` long, as a video, or as a sound when `video` is false.
    pub fn open(
        path: &str,
        video: bool,
        file_bytes: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let named = c(path);
        // SAFETY: a NUL-terminated path.
        let number = unsafe {
            if video {
                nr_preview_open(named.as_ptr())
            } else {
                nr_preview_open_sound(named.as_ptr())
            }
        };
        let style = if video { Style::Video } else { Style::Sound };
        let controls = cx.new(|cx| Controls::new(style, file_bytes, "", path, cx));
        // The clip's size is known once it opens. The controls track playback, and the sound
        // output runs only while it plays.
        let follow = cx.spawn(async move |player, cx| {
            let every = Duration::from_secs_f64(nr_frame_seconds(NrFrame::Follow));
            loop {
                cx.background_executor().timer(every).await;
                if player.update(cx, |player, cx| player.follow(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            controls,
            layer: video.then(|| VideoLayer::new(window)),
            frame: Bounds::default(),
            video_size: None,
            number,
            _follow: follow,
        }
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let info = nr_preview_info();
        let mut changed = false;
        if info.ready && info.width > 0 && info.height > 0 {
            let size = Some((f64::from(info.width), f64::from(info.height)));
            changed = self.video_size != size;
            self.video_size = size;
        }
        follow_sound();
        // Redraw only when the controls or clip size change, so a paused clip under hidden
        // controls draws nothing. Off macOS the window draws the video itself
        // (`VideoLayer::paint`) and redraws on every follow.
        changed |= self.controls.update(cx, |controls, cx| controls.follow(cx));
        if changed || cfg!(not(target_os = "macos")) {
            cx.notify();
        }
    }

    /// The video's own size once it opens.
    pub fn video_size(&self) -> Option<(f64, f64)> {
        self.video_size
    }

    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    /// Where the video and controls go now: the pane's place, or the whole window in full screen,
    /// with the video fitted on black.
    fn shown_at(&self, window: &Window) -> Bounds<Pixels> {
        if fullscreen() == Fullscreen::Off {
            return self.frame;
        }
        let info = nr_preview_info();
        let view = window.viewport_size();
        let mut fitted = [0.0; 4];
        // SAFETY: storage for four numbers.
        unsafe {
            nr_video_fullscreen_fit(
                f64::from(info.width),
                f64::from(info.height),
                f64::from(view.width),
                f64::from(view.height),
                fitted.as_mut_ptr(),
            )
        };
        kit::core::frame(
            fitted[0] as f32,
            fitted[1] as f32,
            fitted[2] as f32,
            fitted[3] as f32,
        )
    }
}

impl Drop for Player {
    /// The preview releases the layer before the layer drops (fields drop after this). A player
    /// is dropped after the next one opened its file (another video selected), and that one's
    /// preview and sound keep playing.
    fn drop(&mut self) {
        if nr_preview_close(self.number) {
            stop_sound();
            if fullscreen().took_window() {
                WINDOW_OWED.with(|owed| owed.set(true));
            }
            set_fullscreen(Fullscreen::Off);
        }
    }
}

impl Render for Player {
    /// Draws in window points; the caller puts the player in an element at the window origin.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.shown_at(window);
        let filling = shown != self.frame;
        // Only the playing player has a layer; a page's resting players have none.
        if self.layer.is_some() {
            SHOWN.with(|player| *player.borrow_mut() = Some(cx.entity().downgrade()));
        }
        follow_window(window);
        self.controls
            .update(cx, |controls, cx| controls.place(shown, cx));
        let player = cx.entity();
        let painted = cx.entity();
        let picture = placed("player", cx.entity_id(), shown).child(
            canvas(
                move |_, window, cx| {
                    player.update(cx, |player, _| {
                        if let Some(layer) = player.layer.as_mut() {
                            layer.place(shown, window);
                        }
                    })
                },
                move |_, _, window, cx| {
                    painted.update(cx, |player, _| {
                        if let Some(layer) = player.layer.as_mut() {
                            layer.paint(window);
                        }
                    })
                },
            )
            .size_full(),
        );
        let viewport = window.viewport_size();
        // In full screen, black around the video covers the window.
        let cover = filling.then(|| {
            canvas(
                |_, _, _| (),
                |bounds, _, window, _| kit::hosted::paint_around(window, bounds, gpui::black()),
            )
            .absolute()
            .w(viewport.width)
            .h(viewport.height)
        });
        // In full screen the window draws this player alone (`filling_screen`), so nothing is
        // under it.
        gpui::div()
            .absolute()
            .children(cover)
            .child(picture)
            .child(self.controls.clone())
            .into_any_element()
    }
}
