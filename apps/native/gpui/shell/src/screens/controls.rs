//! Player controls, drawn like a browser's `<video controls>` and `<audio controls>`. For video:
//! play and time on the left, volume, full screen and the overflow menu on the right, the scrub
//! bar below, and the overflow menu itself. One addition Chromium lacks is a frame and time
//! preview above the scrub bar. For sound: one light pill with play, time, bar, volume and the
//! overflow button.
//!
//! The core makes every decision: layout, fades, what presses, drags, wheels and keys do, and
//! time formatting (`nr_controls_*`, `player::video_controls::Controls`). This module measures
//! text, draws what the core says and tracks drags. The open file's controls take every press.
//! A page player's controls (made for its `block`) leave presses outside their parts to the
//! page and wake their player through `wake` when the core asks.
//!
//! The panel and the menu each fade as one group (`kit::layer::group`); the buttons inside the
//! panel fade individually.

use super::preview;
use gpui::{
    App, Bounds, Context, Corners, DispatchPhase, FocusHandle, Hitbox, HitboxBehavior, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Render, RenderImage, Rgba, ScrollDelta, ScrollEvent,
    SharedString, Styled, TransformationMatrix, Window, canvas, point, px,
};
use kit::controls::placed;
use kit::core::{c, frame, rect, text};
use kit::draw::{fill, oval, rounded};
use kit::nr::*;
use kit::text::PlatformFace;
use northroom_core::preview::player::video_controls::{GROUND_RGB, MENU_TICK_RGB};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

/// The browser sets video time and menu text in Roboto if installed, else its sans-serif,
/// Helvetica. macOS doesn't ship Roboto. This is the platform face, not the app's.
const VIDEO_FAMILY: &str = "Helvetica";
const VIDEO_TIME: PlatformFace = PlatformFace {
    family: VIDEO_FAMILY,
    size: 14.0,
};
/// A sound's time. The browser uses 14 px Roboto, which matches the system font at 13 px in
/// width.
const SOUND_TIME: PlatformFace = PlatformFace {
    family: ".SystemUIFont",
    size: 13.0,
};
/// The line height for a sound's time.
const SOUND_TIME_LINE: f32 = 18.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Video,
    Sound,
}

/// What a page's player does when the core asks to wake it (`NrControlsPress::Wake`).
pub type Wake = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct Controls {
    style: Style,
    handle: *mut NrControls,
    /// Where they are drawn, in window points.
    frame: Bounds<Pixels>,
    focus: FocusHandle,
    /// A resting page player opens its file when a press or key asks it to.
    pub wake: Option<Wake>,
    /// What a page draws over the controls (its field and `/` menu), in window points. Presses
    /// and wheels there are not theirs.
    pub covered: Vec<Bounds<Pixels>>,
    /// The pointer is over the controls.
    inside: bool,
    /// A press on a bar is tracked until release.
    dragging: bool,
    /// A hash of what the controls showed when last drawn (`shown_now`), so they redraw only
    /// when it changes (`follow`).
    drawn: Rc<Cell<Option<u64>>>,
    /// The playing file, whose frames the scrub preview shows.
    path: Rc<str>,
    /// The scrub preview's decoded frames, and the last one drawn.
    preview: Rc<RefCell<Storyboard>>,
}

impl Controls {
    /// Controls for the file at `path`, `file_bytes` long: the open file's (`block` empty), or a
    /// page player's, which rests while another plays.
    pub fn new(
        style: Style,
        file_bytes: u64,
        block: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Self {
        let (named, file) = (c(block), c(path));
        // SAFETY: NUL-terminated block and path. The controls are freed in `drop`.
        let handle = unsafe {
            nr_controls_new(
                style == Style::Sound,
                file_bytes,
                named.as_ptr(),
                file.as_ptr(),
                kit::motion::now(),
            )
        };
        Self {
            style,
            handle,
            frame: Bounds::default(),
            focus: cx.focus_handle(),
            wake: None,
            covered: Vec::new(),
            inside: false,
            dragging: false,
            drawn: Rc::new(Cell::new(None)),
            path: Rc::from(path),
            preview: Rc::new(RefCell::new(Storyboard::default())),
        }
    }

    /// Tracks the clip on every `NrFrame::Follow`. Redraws when what the core says they show has
    /// changed (a fade, the time or the bar moves), and not otherwise (a paused clip under hidden
    /// controls). Returns true when they redraw.
    pub fn follow(&mut self, cx: &mut Context<Self>) -> bool {
        let now = shown_now(self.style, self.handle, self.frame);
        if self.drawn.get() == Some(now) {
            return false;
        }
        cx.notify();
        true
    }

    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    /// Whether the controls have keyboard focus.
    pub fn has_keys(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    fn width(&self) -> f64 {
        f64::from(self.frame.size.width)
    }

    fn height(&self) -> f64 {
        f64::from(self.frame.size.height)
    }

    /// Converts `at` (window points) to control coordinates.
    fn local(&self, at: gpui::Point<Pixels>) -> (f64, f64) {
        (
            f64::from(at.x - self.frame.origin.x),
            f64::from(at.y - self.frame.origin.y),
        )
    }

    fn moved(&mut self, event: &MouseMoveEvent, over: bool, cx: &mut Context<Self>) {
        if self.dragging {
            if event.pressed_button == Some(MouseButton::Left) {
                let (x, _) = self.local(event.position);
                // SAFETY: live controls.
                unsafe { nr_controls_drag(self.handle, x) };
                cx.notify();
            }
            return;
        }
        let now = kit::motion::now();
        if over {
            self.inside = true;
            let (x, y) = self.local(event.position);
            // SAFETY: live controls.
            unsafe { nr_controls_pointer(self.handle, self.width(), self.height(), x, y, now) };
            cx.notify();
        } else if std::mem::replace(&mut self.inside, false) {
            // SAFETY: live controls.
            unsafe { nr_controls_exit(self.handle, now) };
            cx.notify();
        }
    }

    /// Whether the controls take a press at `at` (a page player leaves the rest to the page).
    fn takes(&self, at: gpui::Point<Pixels>) -> bool {
        if self.is_covered(at) {
            return false;
        }
        let (x, y) = self.local(at);
        // SAFETY: live controls.
        unsafe { nr_controls_takes_press(self.handle, self.width(), self.height(), x, y) }
    }

    /// Handles a press as the core says (`nr_controls_press`). A press on a bar is tracked until
    /// release. The player then keeps keyboard focus.
    fn pressed(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let (x, y) = self.local(event.position);
        let clicks = event.click_count as u32;
        let mut answer = self.press(x, y, clicks);
        if answer == NrControlsPress::Wake {
            if let Some(wake) = self.wake.clone() {
                wake(window, cx);
            }
            answer = self.press(x, y, clicks);
        }
        match answer {
            NrControlsPress::Drag => self.dragging = true,
            NrControlsPress::Download => preview::save_copy(cx),
            NrControlsPress::Fullscreen => preview::follow_fullscreen(window),
            NrControlsPress::Done | NrControlsPress::Wake => {}
        }
        preview::follow_sound();
        cx.notify();
    }

    fn press(&self, x: f64, y: f64, clicks: u32) -> NrControlsPress {
        // SAFETY: live controls.
        let clock = text(&unsafe { nr_controls_clock(self.handle) }.whole);
        let clock = SOUND_TIME.width(&clock);
        // SAFETY: as above.
        unsafe {
            nr_controls_press(
                self.handle,
                self.width(),
                self.height(),
                x,
                y,
                clicks,
                kit::motion::now(),
                f64::from(clock),
            )
        }
    }

    fn released(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if !std::mem::replace(&mut self.dragging, false) {
            return;
        }
        let (x, _) = self.local(event.position);
        // SAFETY: live controls.
        unsafe {
            nr_controls_drag(self.handle, x);
            nr_controls_release(self.handle);
        }
        preview::follow_sound();
        cx.notify();
    }

    fn is_covered(&self, at: gpui::Point<Pixels>) -> bool {
        self.covered
            .iter()
            .any(|over| kit::core::contains(over, at))
    }

    /// A wheel over the open speed list scrolls it; elsewhere it propagates.
    fn wheeled(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) -> bool {
        if self.is_covered(event.position) {
            return false;
        }
        let (x, y) = self.local(event.position);
        let (down, unit) = match event.delta {
            ScrollDelta::Pixels(delta) => (-f64::from(delta.y), NR_WHEEL_POINTS),
            ScrollDelta::Lines(delta) => (-f64::from(delta.y), NR_WHEEL_CLICKS),
        };
        // SAFETY: live controls.
        let took = unsafe {
            nr_controls_wheel(self.handle, self.width(), self.height(), x, y, down, unit)
        };
        if took {
            cx.notify();
        }
        took
    }

    /// Handles the keys the browser's player handles; others propagate.
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let press = kit::keys::press(&event.keystroke);
        let route = |controls: &Self| {
            // SAFETY: live controls.
            unsafe { nr_controls_key(controls.handle, press, controls.width(), controls.height()) }
        };
        let mut answer = route(self);
        if answer == NrControlsKey::Wake {
            if let Some(wake) = self.wake.clone() {
                wake(window, cx);
            }
            answer = route(self);
        }
        match answer {
            NrControlsKey::Menu => {}
            NrControlsKey::Download => preview::save_copy(cx),
            NrControlsKey::Player => {
                preview::follow_sound();
                preview::follow_fullscreen(window);
            }
            NrControlsKey::PassOn | NrControlsKey::Wake => {
                cx.propagate();
                return;
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        // SAFETY: the controls from `new`, not used again.
        unsafe { nr_controls_free(self.handle) };
    }
}

impl Render for Controls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let controls = cx.entity();
        let (style, handle, bounds) = (self.style, self.handle, self.frame);
        let drawn = self.drawn.clone();
        let preview = Preview {
            path: self.path.clone(),
            held: self.preview.clone(),
        };
        placed("player-controls", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|controls, event: &KeyDownEvent, window, cx| {
                controls.key(event, window, cx)
            }))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox: Hitbox, window, cx| {
                        drawn.set(Some(match style {
                            Style::Video => draw_video(window, cx, handle, bounds, &preview),
                            Style::Sound => draw_sound(window, cx, handle, bounds),
                        }));
                        listen(window, &controls, hitbox);
                    },
                )
                .size_full(),
            )
    }
}

/// Registers handlers for pointer moves, presses, drags and wheels over the controls.
fn listen(window: &mut Window, controls: &gpui::Entity<Controls>, hitbox: Hitbox) {
    let entity = controls.clone();
    let over = hitbox.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble {
            let inside = over.is_hovered(window);
            entity.update(cx, |controls, cx| controls.moved(event, inside, cx));
        }
    });
    let entity = controls.clone();
    let over = hitbox.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble
            || event.button != MouseButton::Left
            || !over.is_hovered(window)
            || !entity.read(cx).takes(event.position)
        {
            return;
        }
        cx.stop_propagation();
        entity.update(cx, |controls, cx| controls.pressed(event, window, cx));
    });
    let entity = controls.clone();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
            entity.update(cx, |controls, cx| controls.released(event, cx));
        }
    });
    listen_to_wheels(window, controls, hitbox);
}

/// Registers the wheel handler for while the pointer is over the controls.
fn listen_to_wheels(window: &mut Window, controls: &gpui::Entity<Controls>, hitbox: Hitbox) {
    let entity = controls.clone();
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && hitbox.is_hovered(window)
            && entity.update(cx, |controls, cx| controls.wheeled(event, cx))
        {
            cx.stop_propagation();
        }
    });
}

// MARK: Text and glyphs

/// Draws one line of control text in a line box `line_height` tall at `top`, on Chromium's
/// baseline (`nr_controls_baseline`).
#[allow(clippy::too_many_arguments)]
fn controls_line(
    window: &mut Window,
    cx: &mut App,
    words: &str,
    size: f32,
    color: Hsla,
    x: f32,
    top: f32,
    line_height: f32,
) {
    let face = PlatformFace {
        family: VIDEO_FAMILY,
        size,
    };
    let (ascent, descent) = face.metrics(cx);
    let family = c(VIDEO_FAMILY);
    // SAFETY: a NUL-terminated family.
    let baseline = unsafe {
        nr_controls_baseline(
            f64::from(top),
            f64::from(line_height),
            f64::from(ascent),
            f64::from(descent),
            family.as_ptr(),
            f64::from(window.scale_factor()),
        )
    } as f32;
    face.paint(window, cx, words, color, x, baseline);
}

thread_local! {
    static SHAPES: RefCell<HashMap<String, (SharedString, Vec<u8>)>> = RefCell::new(HashMap::new());
}

/// Draws the media glyph `name` (`nr_media_glyph_shapes`) in `bounds`; nothing for no name.
pub fn glyph(window: &mut Window, cx: &App, name: &str, bounds: Bounds<Pixels>, color: Hsla) {
    if name.is_empty() || kit::core::is_empty(&bounds) {
        return;
    }
    let (key, svg) = SHAPES.with(|shapes| {
        shapes
            .borrow_mut()
            .entry(name.to_owned())
            .or_insert_with(|| {
                let named = c(name);
                // SAFETY: a NUL-terminated name.
                let shapes = text(&unsafe { nr_media_glyph_shapes(named.as_ptr()) });
                let side = nr_media_glyph_box();
                let svg = format!(
                    "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {side} {side}\">{shapes}</svg>"
                );
                (SharedString::from(format!("media/{name}.svg")), svg.into_bytes())
            })
            .clone()
    });
    let _ = window.paint_svg(
        bounds,
        key,
        Some(&svg),
        TransformationMatrix::unit(),
        color,
        cx,
    );
}

/// Converts a core 0xRRGGBB sRGB colour at `alpha` to the window's colour space.
fn rgb(value: u32, alpha: f32) -> Hsla {
    kit::display::shown(Rgba {
        r: ((value >> 16) & 0xFF) as f32 / 255.0,
        g: ((value >> 8) & 0xFF) as f32 / 255.0,
        b: (value & 0xFF) as f32 / 255.0,
        a: alpha,
    })
}

fn white(alpha: f32) -> Hsla {
    Rgba {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: alpha,
    }
    .into()
}

fn black(alpha: f32) -> Hsla {
    Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: alpha,
    }
    .into()
}

/// Converts a core rect from control coordinates to window coordinates.
fn at(origin: gpui::Point<Pixels>, rect_of: &NrRect) -> Bounds<Pixels> {
    kit::core::offset(rect(rect_of), f32::from(origin.x), f32::from(origin.y))
}

fn middle(bounds: Bounds<Pixels>, side: f32) -> Bounds<Pixels> {
    frame(
        f32::from(bounds.origin.x) + (f32::from(bounds.size.width) - side) / 2.0,
        f32::from(bounds.origin.y) + (f32::from(bounds.size.height) - side) / 2.0,
        side,
        side,
    )
}

// MARK: Video controls

/// A hash of what the core says the controls in `bounds` show now (`video_print`,
/// `sound_print`).
fn shown_now(style: Style, handle: *mut NrControls, bounds: Bounds<Pixels>) -> u64 {
    match style {
        Style::Video => {
            let (shown, first, second) = video_now(handle, bounds);
            video_print(&shown, &first, &second)
        }
        Style::Sound => {
            let (shown, clock) = sound_now(handle, bounds);
            sound_print(&shown, &clock)
        }
    }
}

/// A hash of the video controls as drawn: every part's frame, opacity and glyph, and the time
/// text. An open menu changes it every time, because its entrance clock keeps running.
fn video_print(shown: &NrVideoFrame, first: &str, second: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    let rects = [
        &shown.shade,
        &shown.play,
        &shown.track,
        &shown.left_ground.rect,
        &shown.ground.rect,
    ];
    let numbers = [
        shown.panel,
        shown.buttons,
        shown.first_x,
        shown.second_x,
        shown.volume_strength,
        shown.volume_level,
        shown.loaded,
        shown.played,
        shown.left_ground.strength,
        shown.ground.strength,
    ]
    .into_iter()
    .chain(
        rects
            .iter()
            .flat_map(|of| [of.x, of.y, of.width, of.height]),
    );
    for number in numbers {
        number.to_bits().hash(&mut hasher);
    }
    [
        shown.shown,
        shown.first_shown,
        shown.second_shown,
        shown.volume_shown,
        shown.slider_shown,
        shown.knob,
        shown.left_ground.shown,
        shown.ground.shown,
        shown.menu != NrMenuShown::Closed,
        shown.preview_shown,
    ]
    .hash(&mut hasher);
    if shown.preview_shown {
        for number in [
            shown.preview_picture.x,
            shown.preview_picture.y,
            shown.preview_picture.width,
            shown.preview_picture.height,
            shown.preview_seconds,
        ] {
            number.to_bits().hash(&mut hasher);
        }
        text(&shown.preview_time).hash(&mut hasher);
    }
    // The menu's clock keeps running whether or not it is open.
    if shown.menu != NrMenuShown::Closed {
        shown.menu_since_ms.to_bits().hash(&mut hasher);
    }
    [
        first.to_owned(),
        second.to_owned(),
        text(&shown.play_glyph),
        text(&shown.mute_glyph),
        text(&shown.fullscreen_glyph),
        text(&shown.overflow_glyph),
    ]
    .hash(&mut hasher);
    hasher.finish()
}

/// A hash of the sound pill as drawn: its time, bar and glyphs.
fn sound_print(shown: &NrSoundFrame, clock: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    for number in [shown.played, shown.time_x, shown.track.x, shown.track.width] {
        number.to_bits().hash(&mut hasher);
    }
    shown.knob.hash(&mut hasher);
    [
        clock.to_owned(),
        text(&shown.play_glyph),
        text(&shown.mute_glyph),
        text(&shown.more_glyph),
    ]
    .hash(&mut hasher);
    hasher.finish()
}

/// The video controls in `bounds` as the core draws them now, with the two parts of the time.
fn video_now(handle: *mut NrControls, bounds: Bounds<Pixels>) -> (NrVideoFrame, String, String) {
    // SAFETY: live controls (the view holding them is drawn or followed).
    let clock = unsafe { nr_controls_clock(handle) };
    let (first, second) = (text(&clock.first), text(&clock.second));
    let first_width = VIDEO_TIME.width(&first);
    let second_width = VIDEO_TIME.width(&second);
    let (width, height) = (f64::from(bounds.size.width), f64::from(bounds.size.height));
    // SAFETY: as above.
    let shown = unsafe {
        nr_controls_video_frame(
            handle,
            width,
            height,
            kit::motion::now(),
            f64::from(first_width),
            f64::from(second_width),
        )
    };
    (shown, first, second)
}

/// Draws the video controls. The panel and menu each fade as one group, like the browser's
/// layers (`kit::layer::group`). While either animates it redraws every display frame rather
/// than at the follow rate. Returns a hash of what was drawn (`video_print`).
fn draw_video(
    window: &mut Window,
    cx: &mut App,
    handle: *mut NrControls,
    bounds: Bounds<Pixels>,
    preview: &Preview,
) -> u64 {
    let (shown, first, second) = video_now(handle, bounds);
    let print = video_print(&shown, &first, &second);
    if !shown.shown {
        return print;
    }
    let menu_moving = window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        kit::layer::group(window, cx, shown.panel as f32, |window, cx| {
            draw_panel(window, cx, &shown, bounds.origin, (&first, &second));
            draw_preview(window, cx, &shown, bounds.origin, preview);
        });
        shown.menu != NrMenuShown::Closed && draw_menu(window, cx, bounds, shown.menu_since_ms)
    });
    if (shown.panel > 0.0 && shown.panel < 1.0) || menu_moving {
        window.request_animation_frame();
    }
    print
}

/// Draws the panel at full opacity: the shade, buttons, time and bar.
fn draw_panel(
    window: &mut Window,
    cx: &mut App,
    shown: &NrVideoFrame,
    origin: gpui::Point<Pixels>,
    time: (&str, &str),
) {
    draw_shade(window, at(origin, &shown.shade));
    // Buttons fade inside the panel, each on its own.
    let strength = shown.buttons as f32;
    let glyph_side = nr_video_controls_number(NrControlsNumber::Glyph) as f32;
    let button = |window: &mut Window, cx: &mut App, name: &NrText, place: &NrRect| {
        glyph(
            window,
            cx,
            &text(name),
            middle(at(origin, place), glyph_side),
            white(strength),
        );
    };
    let ground = GROUND_RGB;
    // The hovered button gets a round background that fades in; the previous one fades out.
    for worn in [&shown.left_ground, &shown.ground] {
        if worn.shown {
            oval(
                window,
                at(origin, &worn.rect),
                rgb(ground, worn.strength as f32 * strength),
            );
        }
    }
    button(window, cx, &shown.play_glyph, &shown.play);
    let words = at(origin, &shown.words);
    let (top, line) = (f32::from(words.origin.y), f32::from(words.size.height));
    if shown.first_shown {
        controls_line(
            window,
            cx,
            time.0,
            VIDEO_TIME.size,
            white(strength),
            f32::from(origin.x) + shown.first_x as f32,
            top,
            line,
        );
    }
    if shown.second_shown {
        controls_line(
            window,
            cx,
            time.1,
            VIDEO_TIME.size,
            white(strength),
            f32::from(origin.x) + shown.second_x as f32,
            top,
            line,
        );
    }
    if shown.volume_shown {
        let volume = at(origin, &shown.volume_ground);
        rounded(
            window,
            volume,
            rgb(ground, shown.volume_strength as f32 * strength),
            f32::from(volume.size.height) / 2.0,
        );
        // Chromium fills the volume bar with the buffered grey, never white.
        if shown.slider_shown {
            draw_bar(
                window,
                at(origin, &shown.volume_bar),
                shown.volume_level as f32,
                0.0,
                false,
                strength,
            );
        }
    }
    button(window, cx, &shown.mute_glyph, &shown.mute);
    button(window, cx, &shown.fullscreen_glyph, &shown.fullscreen);
    button(window, cx, &shown.overflow_glyph, &shown.overflow);
    draw_bar(
        window,
        at(origin, &shown.track),
        shown.loaded as f32,
        shown.played as f32,
        shown.knob,
        strength,
    );
}

/// The file the scrub bar previews, and its frames decoded so far.
struct Preview {
    path: Rc<str>,
    held: Rc<RefCell<Storyboard>>,
}

/// Frames for the scrub preview, one per `PreviewSteps` step along the clip, decoded at `pixels`
/// on the long side. Each is kept for the life of the controls so revisited moments show at
/// once.
#[derive(Default)]
struct Storyboard {
    pixels: u32,
    /// Keyed by time in milliseconds.
    frames: HashMap<u64, Arc<RenderImage>>,
    /// The last drawn frame, shown while the frame under the pointer decodes.
    last: Option<Arc<RenderImage>>,
}

impl Storyboard {
    /// The frame at `seconds`. Decodes it and any missing frames within `PreviewAhead` steps on
    /// either side, nearest first, so the next one is ready. `None` until the first frame
    /// decodes.
    fn frame(
        &mut self,
        path: &str,
        pixels: u32,
        seconds: f64,
        step: f64,
    ) -> Option<Arc<RenderImage>> {
        if self.pixels != pixels {
            // A new draw size (the window resized), so decode again at that size.
            self.pixels = pixels;
            self.let_go_all();
        }
        let steps = nr_video_controls_number(NrControlsNumber::PreviewSteps);
        let ahead = nr_video_controls_number(NrControlsNumber::PreviewAhead) as i32;
        let last_moment = step * (steps - 1.0);
        let near = std::iter::once(0).chain((1..=ahead).flat_map(|k| [k, -k]));
        for offset in near {
            let moment = seconds + f64::from(offset) * step;
            if !(0.0..=last_moment + step / 2.0).contains(&moment) {
                continue;
            }
            let key = (moment * 1000.0).round() as u64;
            if self.frames.contains_key(&key) {
                continue;
            }
            if let Some(image) = kit::lists::thumbs::video_moment_at(path, pixels, moment) {
                self.frames.insert(key, image);
            }
        }
        if let Some(image) = self.frames.get(&((seconds * 1000.0).round() as u64)) {
            self.last = Some(image.clone());
        }
        self.last.clone()
    }

    /// Returns every kept frame to the thumbs cache, freeing its atlas space. `last` is always
    /// one of them, so it goes too.
    fn let_go_all(&mut self) {
        self.last = None;
        kit::lists::thumbs::let_go(self.frames.drain().map(|(_, image)| image));
    }
}

impl Drop for Storyboard {
    fn drop(&mut self) {
        self.let_go_all();
    }
}

/// Draws the scrub preview at the buttons' opacity: the frame under the pointer, on a
/// background until the first decodes (the last frame stays while the next decodes,
/// `Storyboard`), with the pointer time below in a pill of the same background.
fn draw_preview(
    window: &mut Window,
    cx: &mut App,
    shown: &NrVideoFrame,
    origin: gpui::Point<Pixels>,
    preview: &Preview,
) {
    if !shown.preview_shown {
        preview.held.borrow_mut().last = None;
        return;
    }
    let strength = shown.buttons as f32;
    let picture = at(origin, &shown.preview_picture);
    let corners = Corners::all(px(
        nr_video_controls_number(NrControlsNumber::PreviewCorner) as f32,
    ));
    let long_side = f32::from(picture.size.width).max(f32::from(picture.size.height));
    let pixels = (long_side * window.scale_factor()).ceil() as u32;
    let held = preview.held.borrow_mut().frame(
        &preview.path,
        pixels,
        shown.preview_seconds,
        shown.preview_step_seconds,
    );
    let ground = rgb(
        GROUND_RGB,
        nr_video_controls_number(NrControlsNumber::GroundAlpha) as f32 * strength,
    );
    match held {
        Some(image) => {
            let _ = window.paint_image(picture, picture, corners, image, 0, false);
        }
        None => rounded(window, picture, ground, f32::from(corners.top_left)),
    }
    let words = text(&shown.preview_time);
    let line = at(origin, &shown.preview_line);
    let (left, top, tall) = (
        f32::from(line.origin.x),
        f32::from(line.origin.y),
        f32::from(line.size.height),
    );
    let wide = VIDEO_TIME.width(&words);
    let pad = nr_video_controls_number(NrControlsNumber::PreviewPad) as f32;
    let pill = wide + 2.0 * pad;
    let pill_left = left + (f32::from(line.size.width) - pill) / 2.0;
    rounded(
        window,
        frame(pill_left, top, pill, tall),
        ground,
        tall / 2.0,
    );
    controls_line(
        window,
        cx,
        &words,
        VIDEO_TIME.size,
        white(strength),
        pill_left + pad,
        top,
        tall,
    );
}

/// Draws the panel shade in `shade`: clear to black top to bottom with the stops from
/// Chromium's style sheet, one pixel row at a time.
fn draw_shade(window: &mut Window, shade: Bounds<Pixels>) {
    let stops: Vec<(f32, f32)> = (0..nr_video_controls_number(NrControlsNumber::ShadeStops)
        as usize)
        .map(|index| {
            (
                nr_video_controls_shade(index, false) as f32,
                nr_video_controls_shade(index, true) as f32,
            )
        })
        .collect();
    let scale = window.scale_factor();
    let rows = (f32::from(shade.size.height) * scale).round().max(0.0) as usize;
    for row in 0..rows {
        let place = (row as f32 + 0.5) / rows as f32;
        let strength = shade_at(&stops, place);
        if strength <= 0.0 {
            continue;
        }
        fill(
            window,
            frame(
                f32::from(shade.origin.x),
                f32::from(shade.origin.y) + row as f32 / scale,
                f32::from(shade.size.width),
                1.0 / scale,
            ),
            black(strength),
        );
    }
}

/// The shade's opacity at fraction `place` down, interpolated between its stops.
fn shade_at(stops: &[(f32, f32)], place: f32) -> f32 {
    let Some(first) = stops.first() else {
        return 0.0;
    };
    if place <= first.0 {
        return first.1;
    }
    for pair in stops.windows(2) {
        let ((from, a), (to, b)) = (pair[0], pair[1]);
        if place <= to {
            let share = if to > from {
                (place - from) / (to - from)
            } else {
                1.0
            };
            return a + (b - a) * share;
        }
    }
    stops.last().map_or(0.0, |last| last.1)
}

/// Draws a control bar: the track, the buffered part over it, the played part in white, and
/// the knob on hover. The track has round ends (half its thickness); the fills are cut square
/// inside. The browser draws these as one group, so each stretch is drawn once in their
/// combined white at the group's `strength`.
fn draw_bar(
    window: &mut Window,
    bar: Bounds<Pixels>,
    loaded: f32,
    played: f32,
    knob: bool,
    strength: f32,
) {
    let radius = f32::from(bar.size.height) / 2.0;
    let width = f32::from(bar.size.width);
    let parts = [
        (
            1.0,
            nr_video_controls_number(NrControlsNumber::TrackAlpha) as f32,
        ),
        (
            loaded.clamp(0.0, 1.0),
            nr_video_controls_number(NrControlsNumber::LoadedAlpha) as f32,
        ),
        (played.clamp(0.0, 1.0), 1.0),
    ];
    let mut edges: Vec<f32> = parts.iter().map(|(share, _)| *share).chain([0.0]).collect();
    edges.sort_by(f32::total_cmp);
    edges.dedup();
    for stretch in edges.windows(2) {
        let (from, to) = (stretch[0], stretch[1]);
        // The layers covering this stretch, composited.
        let clear: f32 = parts
            .iter()
            .filter(|(share, _)| *share >= to)
            .map(|(_, alpha)| 1.0 - alpha)
            .product();
        let (left, right) = (width * from, width * to);
        let round_left = if from == 0.0 {
            radius.min((right - left) / 2.0)
        } else {
            0.0
        };
        let round_right = if to >= 1.0 { radius } else { 0.0 };
        window.paint_quad(
            gpui::fill(
                frame(
                    f32::from(bar.origin.x) + left,
                    f32::from(bar.origin.y),
                    right - left,
                    f32::from(bar.size.height),
                ),
                white((1.0 - clear) * strength),
            )
            .corner_radii(Corners {
                top_left: px(round_left),
                bottom_left: px(round_left),
                top_right: px(round_right),
                bottom_right: px(round_right),
            }),
        );
    }
    if knob {
        let side = nr_video_controls_number(NrControlsNumber::Knob) as f32;
        oval(
            window,
            frame(
                f32::from(bar.origin.x) + width * played - side / 2.0,
                f32::from(bar.origin.y) + radius - side / 2.0,
                side,
                side,
            ),
            white(strength),
        );
    }
}

/// Draws the overflow menu like Chromium's: a white sheet with a shadow, the focused row on
/// grey. The core decides layout and text (`player::video_controls::Menu`). It grows from its
/// bottom-right corner and its rows follow. Returns whether the entrance is still animating.
fn draw_menu(window: &mut Window, cx: &mut App, bounds: Bounds<Pixels>, since: f64) -> bool {
    let (width, height) = (f64::from(bounds.size.width), f64::from(bounds.size.height));
    let menu = nr_video_menu(width, height);
    let entrance = menu_entrance(-1, since);
    let (grown, strength) = (entrance[0] as f32, entrance[1] as f32);
    // Rows enter bottom first, so the top row is last.
    let last_row = if menu.kind == NrMenuShown::Main {
        menu_entrance(menu.rows as i32 - 1, since)
    } else {
        [1.0, 0.0]
    };
    let moving = grown < 1.0 || strength < 1.0 || last_row != [1.0, 0.0];
    if grown <= 0.0 || strength <= 0.0 {
        return moving;
    }
    let posed = PosedMenu::new(menu, bounds, grown, since);
    let sheet = posed.posed(
        posed.sheet_left(),
        posed.top + posed.menu.y as f32,
        posed.menu.width as f32,
        posed.menu.height as f32,
    );
    // The sheet and its shadow fade together.
    kit::layer::group(window, cx, strength, |window, cx| {
        let corner = menu_number(NrMenuNumber::Corner) * grown;
        window.paint_drop_shadows(
            sheet,
            Corners::all(px(corner)),
            &[gpui::BoxShadow {
                color: black(menu_number(NrMenuNumber::ShadowAlpha)),
                offset: point(px(0.0), px(menu_number(NrMenuNumber::ShadowDrop) * grown)),
                blur_radius: px(menu_number(NrMenuNumber::ShadowBlur) * grown),
                spread_radius: px(0.0),
                inset: false,
            }],
        );
        rounded(window, sheet, white(1.0), corner);
        let look = MenuLook::read();
        window.with_content_mask(Some(gpui::ContentMask { bounds: sheet }), |window| {
            for index in 0..posed.menu.rows {
                draw_menu_row(window, cx, &posed, &look, index);
            }
        });
    });
    moving
}

/// A core menu metric (`nr_video_menu_number`), in points.
fn menu_number(which: NrMenuNumber) -> f32 {
    nr_video_menu_number(which) as f32
}

/// The menu's entrance `since` ms after opening. With `row` -1, the sheet's scale and opacity;
/// otherwise a main-menu row counted from the bottom, with its opacity and offset below its
/// place.
fn menu_entrance(row: i32, since: f64) -> [f64; 2] {
    let mut entrance = [0.0; 2];
    // SAFETY: storage for two numbers.
    unsafe { nr_video_menu_entrance(row, since, entrance.as_mut_ptr()) };
    entrance
}

/// The menu as drawn now: the core's menu over the video in `bounds`, scaled by `grown` about
/// its bottom-right corner.
struct PosedMenu {
    menu: NrVideoMenu,
    /// The video's size, which the core lays the rows out over.
    picture: (f64, f64),
    since: f64,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    grown: f32,
}

impl PosedMenu {
    fn new(menu: NrVideoMenu, bounds: Bounds<Pixels>, grown: f32, since: f64) -> Self {
        let (left, top) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let (right, bottom) = (
            left + (menu.x + menu.width) as f32,
            top + (menu.y + menu.height) as f32,
        );
        Self {
            picture: (f64::from(bounds.size.width), f64::from(bounds.size.height)),
            menu,
            since,
            left,
            top,
            right,
            bottom,
            grown,
        }
    }

    /// The sheet's left edge before scaling.
    fn sheet_left(&self) -> f32 {
        self.left + self.menu.x as f32
    }

    /// A menu point, scaled about the bottom-right corner.
    fn pose(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.right + (x - self.right) * self.grown,
            self.bottom + (y - self.bottom) * self.grown,
        )
    }

    /// A menu frame, scaled about the bottom-right corner.
    fn posed(&self, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        let (x, y) = self.pose(x, y);
        frame(x, y, w * self.grown, h * self.grown)
    }
}

/// The menu's text and icon sizes from the core.
struct MenuLook {
    font_size: f32,
    ink: Hsla,
    line: f32,
    picture: f32,
}

impl MenuLook {
    fn read() -> Self {
        Self {
            font_size: menu_number(NrMenuNumber::Font),
            ink: black(menu_number(NrMenuNumber::InkAlpha)),
            line: menu_number(NrMenuNumber::Line),
            picture: menu_number(NrMenuNumber::Icon),
        }
    }
}

/// Draws menu row `index` once it has entered: grey when focused, with its icon and text.
fn draw_menu_row(
    window: &mut Window,
    cx: &mut App,
    posed: &PosedMenu,
    look: &MenuLook,
    index: u32,
) {
    let menu = &posed.menu;
    let mut span = [0.0; 2];
    let (width, height) = posed.picture;
    // SAFETY: storage for two numbers.
    if !unsafe { nr_video_menu_row(index, width, height, span.as_mut_ptr()) } {
        return;
    }
    let main = menu.kind == NrMenuShown::Main;
    // Main menu rows enter one by one, bottom first, rising into place.
    let rising = if main {
        menu_entrance((menu.rows - 1 - index) as i32, posed.since)
    } else {
        [1.0, 0.0]
    };
    if rising[0] <= 0.0 {
        return;
    }
    let row = MenuRow {
        posed,
        look,
        strength: rising[0] as f32,
        top: posed.top + span[0] as f32 + rising[1] as f32,
        height: span[1] as f32,
    };
    let row_frame = posed.posed(posed.sheet_left(), row.top, menu.width as f32, row.height);
    if index == menu.focus {
        let grey = menu_number(NrMenuNumber::FocusGrey) / 255.0;
        fill(
            window,
            row_frame,
            Rgba {
                r: grey,
                g: grey,
                b: grey,
                a: row.strength,
            }
            .into(),
        );
    }
    let words = text(&nr_video_menu_label(index, false));
    let under = text(&nr_video_menu_label(index, true));
    glyph(
        window,
        cx,
        &text(&nr_video_menu_glyph(index)),
        posed.posed(
            posed.sheet_left() + menu_number(NrMenuNumber::IconX),
            row.middle() - look.picture / 2.0,
            look.picture,
            look.picture,
        ),
        row.faded(black(1.0)),
    );
    if main {
        row.draw_main_words(window, cx, &words, &under);
    } else {
        row.draw_speed(window, cx, &words, index);
    }
}

/// A menu row during its entrance: its opacity and its place before the menu is scaled.
struct MenuRow<'a> {
    posed: &'a PosedMenu,
    look: &'a MenuLook,
    strength: f32,
    top: f32,
    height: f32,
}

impl MenuRow<'_> {
    fn middle(&self) -> f32 {
        self.top + self.height / 2.0
    }

    /// `color` at the row's opacity.
    fn faded(&self, color: Hsla) -> Hsla {
        color.alpha(color.a * self.strength)
    }

    /// Draws a line of the row's text at `x` in the line box `line` (top, height), scaled with
    /// the menu.
    fn write(
        &self,
        window: &mut Window,
        cx: &mut App,
        said: &str,
        color: Hsla,
        x: f32,
        line: (f32, f32),
    ) {
        let (line_top, line_height) = line;
        let grown = self.posed.grown;
        let (x, line_top) = self.posed.pose(x, line_top);
        controls_line(
            window,
            cx,
            said,
            self.look.font_size * grown,
            self.faded(color),
            x,
            line_top,
            line_height * grown,
        );
    }

    /// Draws a main menu row's text: one line, or two stacked, centred vertically.
    fn draw_main_words(&self, window: &mut Window, cx: &mut App, words: &str, under: &str) {
        let x = self.posed.sheet_left() + menu_number(NrMenuNumber::WordsX);
        let (ink, line) = (self.look.ink, self.look.line);
        if under.is_empty() {
            self.write(window, cx, words, ink, x, (self.top, self.height));
        } else {
            let middle = self.middle();
            self.write(window, cx, words, ink, x, (middle - line, line));
            self.write(
                window,
                cx,
                under,
                black(menu_number(NrMenuNumber::QuietAlpha)),
                x,
                (middle, line),
            );
        }
    }

    /// Draws a speed row: its text, and a check mark on the right for the current speed.
    fn draw_speed(&self, window: &mut Window, cx: &mut App, words: &str, index: u32) {
        let posed = self.posed;
        let x = posed.sheet_left() + menu_number(NrMenuNumber::ChoiceX);
        self.write(window, cx, words, self.look.ink, x, (self.top, self.height));
        if index != posed.menu.ticked {
            return;
        }
        let tick = menu_number(NrMenuNumber::Tick);
        glyph(
            window,
            cx,
            "mediaTick",
            posed.posed(
                posed.sheet_left() + posed.menu.width as f32
                    - menu_number(NrMenuNumber::TickRight)
                    - tick,
                self.middle() - tick / 2.0,
                tick,
                tick,
            ),
            self.faded(rgb(MENU_TICK_RGB, 1.0)),
        );
    }
}

// MARK: Sound controls

/// The sound pill in `bounds` as the core draws it now, with its time text.
fn sound_now(handle: *mut NrControls, bounds: Bounds<Pixels>) -> (NrSoundFrame, String) {
    // SAFETY: live controls (the view holding them is drawn or followed).
    let clock = text(&unsafe { nr_controls_clock(handle) }.whole);
    let clock_width = SOUND_TIME.width(&clock);
    // SAFETY: as above.
    let shown = unsafe {
        nr_controls_sound_frame(
            handle,
            f64::from(bounds.size.width),
            f64::from(bounds.size.height),
            f64::from(clock_width),
        )
    };
    (shown, clock)
}

/// Draws the sound pill like the browser's `<audio controls>`, light regardless of page
/// colours. Returns a hash of what was drawn (`sound_print`).
fn draw_sound(
    window: &mut Window,
    cx: &mut App,
    handle: *mut NrControls,
    bounds: Bounds<Pixels>,
) -> u64 {
    let (shown, clock) = sound_now(handle, bounds);
    let colour = |of: &NrRgba| -> Hsla {
        kit::display::shown(Rgba {
            r: of.red as f32,
            g: of.green as f32,
            b: of.blue as f32,
            a: of.alpha as f32,
        })
    };
    let origin = bounds.origin;
    rounded(
        window,
        bounds,
        colour(&shown.ground),
        f32::from(bounds.size.height) / 2.0,
    );
    let ink = colour(&shown.ink);
    glyph(
        window,
        cx,
        &text(&shown.play_glyph),
        at(origin, &shown.play),
        ink,
    );
    let (ascent, descent) = SOUND_TIME.metrics(cx);
    let top = f32::from(origin.y) + shown.time_y as f32;
    let baseline =
        kit::text::browser_baseline(top, SOUND_TIME_LINE, ascent, descent, window.scale_factor());
    SOUND_TIME.paint(
        window,
        cx,
        &clock,
        ink,
        f32::from(origin.x) + shown.time_x as f32,
        baseline,
    );
    // The bar: the played part solid on a fainter track, with a knob at the current time.
    let track = at(origin, &shown.track);
    rounded(window, track, colour(&shown.track_ink), 2.0);
    let mut filled = track;
    filled.size.width = track.size.width * shown.played as f32;
    rounded(window, filled, ink, 2.0);
    if shown.knob {
        let right = f32::from(filled.origin.x + filled.size.width);
        let middle_y = f32::from(track.origin.y) + f32::from(track.size.height) / 2.0;
        oval(window, frame(right - 6.0, middle_y - 6.0, 12.0, 12.0), ink);
    }
    glyph(
        window,
        cx,
        &text(&shown.more_glyph),
        at(origin, &shown.more),
        ink,
    );
    glyph(
        window,
        cx,
        &text(&shown.mute_glyph),
        at(origin, &shown.mute),
        ink,
    );
    sound_print(&shown, &clock)
}
