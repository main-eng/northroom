//! Frame log for measuring the app by hand on a Mac (ADR "One GPUI shell for Mac, Windows and
//! Linux", production measurements). Enabled by `NR_FRAME_TIMES` and never used in checks; when
//! off, a frame costs one atomic load. When on, each draw logs
//! `frame\t<seconds>\t<reason>` on the steady clock. Add `ZED_MEASUREMENTS=1` and GPUI also logs
//! draw and present time after each frame (`frame duration:`). On a Mac the window floats above
//! other apps while logging, because a covered window gets no frames.
//!
//! `NR_FRAME_BUSY=<from>+<seconds>` redraws on every display frame for that span, so a phase
//! measures what the window holds rather than what happens to change.
//! `NR_FRAME_INPUT` sends one input per frame, with phases separated by `;`:
//! `drag:<x0>:<x1>:<y>@<from>+<seconds>` presses at x0, sweeps between x0 and x1 (1 s each way)
//! and releases; `wheel:<x>:<y>:<points>@<from>+<seconds>` scrolls that many points per frame at
//! the given spot, reversing every 2 s.

use crate::workspace::Workspace;
use gpui::{
    App, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    PlatformInput, Point, ScrollDelta, ScrollEvent, TouchPhase, Window, WindowHandle, point, px,
};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static ON: AtomicBool = AtomicBool::new(false);

/// A span of the run, in seconds on the steady clock.
#[derive(Clone, Copy, Debug)]
struct While {
    from: f64,
    until: f64,
}

impl While {
    /// `<from>+<seconds>`.
    fn parse(text: &str) -> Option<Self> {
        let (from, seconds) = text.split_once('+')?;
        let from: f64 = from.parse().ok()?;
        Some(Self {
            from,
            until: from + seconds.parse::<f64>().ok()?,
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum Input {
    Drag { from: f32, to: f32, y: f32 },
    Wheel { x: f32, y: f32, points: f32 },
}

impl Input {
    /// `drag:<x0>:<x1>:<y>` or `wheel:<x>:<y>:<points>`.
    fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split(':').collect();
        let numbers: Vec<f32> = parts[1..]
            .iter()
            .filter_map(|part| part.parse().ok())
            .collect();
        match (parts[0], numbers.as_slice()) {
            ("drag", &[from, to, y]) => Some(Self::Drag { from, to, y }),
            ("wheel", &[x, y, points]) => Some(Self::Wheel { x, y, points }),
            _ => None,
        }
    }

    /// The event for the frame `into` seconds after the span began.
    fn event(self, into: f64, moment: Moment) -> PlatformInput {
        match self {
            Self::Drag { from, to, y } => {
                // Triangle wave: 1 s from `from` to `to`, 1 s back.
                let leg = into % 2.0;
                let share = if leg < 1.0 { leg } else { 2.0 - leg } as f32;
                drag_event(point(px(from + (to - from) * share), px(y)), moment)
            }
            Self::Wheel { x, y, points } => {
                let down = if (into / 2.0) as i64 % 2 == 0 {
                    points
                } else {
                    -points
                };
                PlatformInput::Scroll(ScrollEvent {
                    position: point(px(x), px(y)),
                    delta: ScrollDelta::Pixels(point(px(0.0), px(-down))),
                    modifiers: Modifiers::default(),
                    touch_phase: moment.touch_phase(),
                    source: Default::default(),
                })
            }
        }
    }
}

/// Where a frame falls in its span.
#[derive(Clone, Copy, Debug)]
enum Moment {
    First,
    Between,
    Last,
}

impl Moment {
    /// A frame that is both first and last counts as first.
    fn of(first: bool, last: bool) -> Self {
        match (first, last) {
            (true, _) => Self::First,
            (false, true) => Self::Last,
            (false, false) => Self::Between,
        }
    }

    fn touch_phase(self) -> TouchPhase {
        match self {
            Self::First => TouchPhase::Started,
            Self::Between => TouchPhase::Moved,
            Self::Last => TouchPhase::Ended,
        }
    }
}

/// A drag event at `at`: left button down on the first frame, up on the last, held between.
fn drag_event(at: Point<Pixels>, moment: Moment) -> PlatformInput {
    match moment {
        Moment::First => PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
        Moment::Last => PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
        Moment::Between => PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        }),
    }
}

/// The per-frame work of a phase: redraw, or send an input.
#[derive(Clone, Copy, Debug)]
enum Each {
    Draw,
    Input(Input),
}

/// Whether frame logging is on (`NR_FRAME_TIMES`). The window then stays on screen even in a
/// scripted run, because an off-screen window gets no frames.
pub fn asked() -> bool {
    std::env::var_os("NR_FRAME_TIMES").is_some_and(|value| !value.is_empty())
}

/// Starts the log and any per-frame phases, if the run asks for them.
pub fn start(handle: WindowHandle<Workspace>, cx: &mut App) {
    if !asked() {
        return;
    }
    ON.store(true, Ordering::Relaxed);
    // Float above other apps so the display requests every frame.
    #[cfg(target_os = "macos")]
    let _ = handle.update(cx, |_, window, _| kit::platform::float_over_others(window));
    let busy = std::env::var("NR_FRAME_BUSY").ok();
    let inputs = std::env::var("NR_FRAME_INPUT").unwrap_or_default();
    let mut phases: Vec<(While, Each, String)> = Vec::new();
    if let Some(busy) = busy.as_deref() {
        let span = While::parse(busy).expect("NR_FRAME_BUSY is <from>+<seconds>");
        phases.push((span, Each::Draw, "busy".to_owned()));
    }
    for phase in inputs.split(';').filter(|phase| !phase.is_empty()) {
        let (input, span) = phase
            .split_once('@')
            .expect("an NR_FRAME_INPUT phase is <input>@<from>+<seconds>");
        let input = Input::parse(input).expect("an input is drag:x0:x1:y or wheel:x:y:points");
        let span = While::parse(span).expect("a phase's while is <from>+<seconds>");
        phases.push((span, Each::Input(input), phase.to_owned()));
    }
    for (span, each, name) in phases {
        cx.spawn(async move |cx| {
            let wait = (span.from - kit::motion::now()).max(0.0);
            cx.background_executor()
                .timer(Duration::from_secs_f64(wait))
                .await;
            let _ = handle.update(cx, |_, window, _| {
                eprintln!("frames\tbegin {name}\t{:.4}", kit::motion::now());
                every_frame(window, Rc::new((span, each, name)), true);
            });
        })
        .detach();
    }
}

/// Does `phase`'s work on the next display frame, and keeps rescheduling until its span ends.
fn every_frame(window: &mut Window, phase: Rc<(While, Each, String)>, first: bool) {
    window.on_next_frame(move |window, cx| {
        let (span, each, name) = &*phase;
        let now = kit::motion::now();
        let last = now >= span.until;
        match each {
            Each::Draw => window.refresh(),
            Each::Input(input) => {
                window.dispatch_event(input.event(now - span.from, Moment::of(first, last)), cx);
            }
        }
        if last {
            eprintln!("frames\tend {name}\t{now:.4}");
        } else {
            every_frame(window, phase.clone(), false);
        }
    });
}

/// Called on every window draw; logs the frame when logging is on.
pub fn drawn() {
    if ON.load(Ordering::Relaxed) {
        let taking = match kit::layer::taking() {
            kit::layer::Taking::Nothing => "shown",
            kit::layer::Taking::Under => "under",
        };
        eprintln!("frame\t{:.4}\t{taking}", kit::motion::now());
    }
}
