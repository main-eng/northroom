//! Runs a check's script. The core parses `NR_DO` and runs its own steps (`core/src/script.rs`);
//! steps that need the window are handed to the shell as they come due.
//!
//! `table` maps each step's route (its kind, or its event for events) to a handler. Steps every
//! window handles the same way (synthetic input, snapshots, keys, menu frames, state lines) live
//! in `events`; a screen registers its own steps in its module's `register` and is listed in
//! `table`. A step with no handler ends the run at once with `gpui\tnot supported yet`, so an
//! unsupported check neither hangs nor reads as a failure.

pub mod events;
pub mod probe;

use crate::workspace::Workspace;
use gpui::{App, AppContext, AsyncApp, Modifiers, Pixels, Point, Window, WindowHandle, point, px};
use kit::core::text;
use kit::nr::*;
use std::time::{Duration, Instant};

/// An owned copy of the core's `NrScriptStep`.
#[derive(Clone, Debug)]
pub struct Step {
    pub kind: NrScriptKind,
    pub event: NrScriptEvent,
    pub columns: NrColumnsScript,
    pub sync: bool,
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
    pub modifiers: u32,
    pub key: NrKey,
    pub number: i64,
    pub alone: bool,
    pub text: String,
    pub name: String,
}

impl Step {
    pub fn of(step: &NrScriptStep) -> Self {
        Self {
            kind: step.kind,
            event: step.event,
            columns: step.columns,
            sync: step.sync,
            x: step.x,
            y: step.y,
            dx: step.dx,
            dy: step.dy,
            modifiers: step.modifiers,
            key: step.key,
            number: step.number,
            alone: step.alone,
            text: text(&step.text),
            name: text(&step.name),
        }
    }

    pub fn at(&self) -> Point<Pixels> {
        point(px(self.x as f32), px(self.y as f32))
    }

    /// The modifier keys the step holds (`NR_HELD_*`).
    pub fn held(&self) -> Modifiers {
        Modifiers {
            shift: self.modifiers & NR_HELD_SHIFT != 0,
            alt: self.modifiers & NR_HELD_OPTION != 0,
            control: self.modifiers & NR_HELD_CONTROL != 0,
            platform: self.modifiers & NR_HELD_COMMAND != 0,
            function: false,
        }
    }
}

/// How a step finds its handler: events by event, everything else by kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Kind(NrScriptKind),
    Event(NrScriptEvent),
}

impl Route {
    fn of(step: &Step) -> Self {
        if step.kind == NrScriptKind::Event {
            Self::Event(step.event)
        } else {
            Self::Kind(step.kind)
        }
    }
}

/// The synthetic pointer a check's events move.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pointer {
    pub at: Point<Pixels>,
    /// The left button is held between `down` and `up`, so moves in between are drags.
    pub down: bool,
}

/// What a handler returns to the runner.
pub enum Answer {
    Done,
    /// Save a snapshot to `path`. `whole` includes overlays (menus, tooltips, a drag's chip).
    Picture {
        path: String,
        whole: bool,
    },
    /// Run another step now, such as an `at:` step's event aimed at the row's position.
    Then(Step),
}

/// Performs a step in the window; get the workspace with `workspace(window)`. Dispatched events
/// go through the window as a user's would.
pub type Handler = fn(&Step, &mut Pointer, &mut Window, &mut App) -> Answer;

/// The steps the shell handles, one handler per route.
#[derive(Default)]
pub struct Table {
    routes: Vec<(Route, Handler)>,
}

impl Table {
    /// Registers `handler` for `route`. Panics if the route already has one.
    pub fn answer(&mut self, route: Route, handler: Handler) {
        assert!(self.find(route).is_none(), "two handlers answer {route:?}");
        self.routes.push((route, handler));
    }

    fn find(&self, route: Route) -> Option<Handler> {
        self.routes
            .iter()
            .find(|(answered, _)| *answered == route)
            .map(|(_, handler)| *handler)
    }
}

/// All handlers: the window's, then each screen's.
fn table() -> Table {
    let mut table = Table::default();
    events::register(&mut table);
    crate::screens::columns::register(&mut table);
    crate::screens::board::register(&mut table);
    crate::screens::changes::register(&mut table);
    crate::screens::html::register(&mut table);
    crate::screens::title_bar::register(&mut table);
    crate::menu_bar::register(&mut table);
    crate::screens::preview::register(&mut table);
    crate::screens::page::register(&mut table);
    crate::screens::terminal::register(&mut table);
    table
}

pub fn workspace(window: &Window) -> gpui::Entity<Workspace> {
    window
        .root::<Workspace>()
        .flatten()
        .expect("the window shows the workspace")
}

/// Runs the check's script, if there is one, to the end.
pub fn run(handle: WindowHandle<Workspace>, cx: &mut App) {
    let table = table();
    cx.spawn(async move |cx| {
        // Let the window draw before the first step.
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        let mut pointer = Pointer::default();
        loop {
            let next = nr_script_next(kit::motion::now());
            let (over, delay) = (next.over, next.delay_ms);
            let step = Step::of(&next);
            if !perform(&table, handle, step, &mut pointer, cx).await {
                cx.update(|cx| cx.quit());
                break;
            }
            if over {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(u64::from(delay)))
                .await;
        }
    })
    .detach();
}

/// Performs `step`. Returns false when the run ends: the step is unsupported or the window is
/// gone.
async fn perform(
    table: &Table,
    handle: WindowHandle<Workspace>,
    step: Step,
    pointer: &mut Pointer,
    cx: &mut AsyncApp,
) -> bool {
    let sync = step.sync;
    let mut due = Some(step);
    while let Some(step) = due.take() {
        if step.kind == NrScriptKind::Nothing {
            break;
        }
        let route = Route::of(&step);
        let Some(handler) = table.find(route) else {
            say_unsupported(route, &step);
            return false;
        };
        let Ok(answer) = cx.update_window(handle.into(), |_, window, cx| {
            handler(&step, pointer, window, cx)
        }) else {
            return false;
        };
        match answer {
            Answer::Done => {}
            Answer::Picture { path, whole } => picture(handle, &path, whole, cx).await,
            Answer::Then(next) => due = Some(next),
        }
    }
    if sync {
        let _ = handle.update(cx, |workspace, window, cx| workspace.changed(window, cx));
    }
    true
}

/// Reports that `step` is unsupported, which ends the run.
fn say_unsupported(route: Route, step: &Step) {
    let gap = if step.name.is_empty() || step.text.is_empty() {
        ""
    } else {
        " "
    };
    eprintln!(
        "gpui\tnot supported yet: {route:?} {}{gap}{} (the run ends here)",
        step.name, step.text
    );
}

/// Longest a snapshot waits for hosted views to render at their current size
/// (`kit::hosted::holes_shown`).
const HOSTED_PICTURE_WAIT: Duration = Duration::from_secs(2);

/// Saves a snapshot of the window. Draws the frame here because GPUI only draws on display
/// refresh while the window is uncovered, and a check's window often isn't.
async fn picture(handle: WindowHandle<Workspace>, path: &str, whole: bool, cx: &mut AsyncApp) {
    let _ = handle.update(cx, |workspace, window, cx| {
        workspace.changed(window, cx);
        workspace.picture_whole = whole;
    });
    // The first draw requests its thumbnails. Wait up to 1 s for them, as a user would see
    // them arrive.
    let _ = cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx));
    let deadline = Instant::now() + Duration::from_secs(1);
    while kit::lists::thumbs::reading() > 0 && Instant::now() < deadline {
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
    // Hosted views drawn on their own threads (the core's preview) lag a moment behind a
    // resize, so also wait for them to match their size.
    let deadline = Instant::now() + HOSTED_PICTURE_WAIT;
    while !kit::hosted::holes_shown() && Instant::now() < deadline {
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
    // On Linux the snapshot is the on-screen frame (`platform::save_window_picture`), so redraw
    // and allow a few frames to present.
    #[cfg(target_os = "linux")]
    {
        let _ = cx.update_window(handle.into(), |_, window, _| window.refresh());
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
    }
    let path = path.to_owned();
    let _ = cx.update_window(handle.into(), move |_, window, cx| {
        window.draw(cx).clear(cx);
        save(window, &path);
    });
    let _ = handle.update(cx, |workspace, _, cx| {
        workspace.picture_whole = true;
        cx.notify();
    });
}

#[cfg(target_os = "linux")]
fn save(window: &mut Window, path: &str) {
    if let Err(error) = kit::platform::save_window_picture(window, path) {
        eprintln!("gpui\tthe picture {path} was not saved: {error}");
    }
}

#[cfg(all(feature = "pictures", not(target_os = "linux")))]
fn save(window: &mut Window, path: &str) {
    match window.render_to_image() {
        Ok(mut image) => {
            // Paint native hosted views into their holes.
            kit::hosted::fill_holes(&mut image, window.scale_factor());
            if let Err(error) = kit::display::save(&image, path) {
                eprintln!("gpui\tthe picture {path} was not saved: {error}");
            }
        }
        Err(error) => eprintln!("gpui\tno picture of the window: {error}"),
    }
}

#[cfg(all(not(feature = "pictures"), not(target_os = "linux")))]
fn save(_: &mut Window, path: &str) {
    eprintln!("gpui\tbuilt without pictures: {path} was not saved");
}
