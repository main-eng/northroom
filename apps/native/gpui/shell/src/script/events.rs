//! Steps every window handles the same way. Pointer events are built at window points and
//! dispatched as the platform would deliver them, so hit testing, focus and core rules run as
//! for a user. Also covers keys and typing, snapshots, menu frames and snapshots, state reports
//! (`ev:observe`, `ev:state`, `ev:say`), theme and appearance changes, the core's dialog, and
//! quitting.

use super::{Answer, Handler, Pointer, Route, Step, Table, workspace};
use gpui::{
    App, Bounds, ExternalPaths, FileDropEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PinchEvent, Pixels, PlatformInput, Point, ScrollDelta,
    ScrollEvent, TouchPhase, Window, WindowAppearance, point,
};
use kit::core::c;
use kit::nr::*;

pub fn register(table: &mut Table) {
    use NrScriptEvent as E;
    use NrScriptKind as K;
    let events: [(NrScriptEvent, Handler); 19] = [
        (E::Move, moved),
        (E::Down, down),
        (E::Up, up),
        (E::Click, click),
        (E::DoubleClick, click),
        (E::RightClick, right_click),
        (E::MiddleClick, middle_click),
        (E::Precise, wheel),
        (E::Lines, wheel),
        (E::Swipe, wheel),
        (E::Pinch, pinch),
        (E::Drop, dropped),
        (E::Key, key),
        (E::Text, typed),
        (E::Shot, shot),
        (E::Observe, observe),
        (E::State, state),
        (E::LookUnder, look_under),
        (E::Say, say),
    ];
    for (event, handler) in events {
        table.answer(Route::Event(event), handler);
    }
    let kinds: [(NrScriptKind, Handler); 10] = [
        (K::At, at),
        (K::Window, window_picture),
        (K::Quit, quit),
        (K::ThemeChanged, theme_changed),
        (K::Look, look),
        (K::Dialog, dialog),
        (K::Rename, rename),
        (K::RightClick, report_right_click),
        (K::ReportMenus, report_menus),
        (K::SaveMenus, save_menus),
    ];
    for (kind, handler) in kinds {
        table.answer(Route::Kind(kind), handler);
    }
}

// MARK: Events

/// Dispatches `input` against a freshly drawn frame.
fn dispatch(input: PlatformInput, window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
    window.dispatch_event(input, cx);
}

fn mouse_move(at: Point<Pixels>, pressed: Option<MouseButton>, held: Modifiers) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position: at,
        pressed_button: pressed,
        modifiers: held,
    })
}

fn press(
    at: Point<Pixels>,
    button: MouseButton,
    held: Modifiers,
    clicks: usize,
) -> [PlatformInput; 2] {
    [
        PlatformInput::MouseDown(MouseDownEvent {
            button,
            position: at,
            modifiers: held,
            click_count: clicks,
            first_mouse: false,
        }),
        PlatformInput::MouseUp(MouseUpEvent {
            button,
            position: at,
            modifiers: held,
            click_count: clicks,
        }),
    ]
}

/// Moves the pointer to `at`, notifying the views it crosses as tracking areas would.
fn come_to(at: Point<Pixels>, pointer: &mut Pointer, window: &mut Window, cx: &mut App) {
    pointer.at = at;
    let pressed = pointer.down.then_some(MouseButton::Left);
    dispatch(mouse_move(at, pressed, Modifiers::default()), window, cx);
}

fn moved(step: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    come_to(step.at(), pointer, window, cx);
    Answer::Done
}

/// Whether a press at `at` lands on a native hosted view (a live page or PDF). Such presses are
/// posted as an `NSEvent` on the app's queue so GPUI forwards them like a user's.
#[cfg(target_os = "macos")]
fn for_hosted(at: Point<Pixels>, window: &mut Window, cx: &mut App) -> bool {
    window.draw(cx).clear(cx);
    kit::hosted::over_used(at)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Left {
    Down,
    Up,
}

/// Presses or releases the left button at `at` with click count `clicks`.
fn left(
    button: Left,
    at: Point<Pixels>,
    held: Modifiers,
    clicks: usize,
    window: &mut Window,
    cx: &mut App,
) {
    #[cfg(target_os = "macos")]
    if for_hosted(at, window, cx) {
        let made = match button {
            Left::Down => kit::platform::Button::Down,
            Left::Up => kit::platform::Button::Up,
        };
        let (x, y) = (f64::from(at.x), f64::from(at.y));
        kit::platform::post_press(window, made, x, y, clicks as isize, kit::keys::held(&held));
        return;
    }
    let [down, up] = press(at, MouseButton::Left, held, clicks);
    dispatch(if button == Left::Down { down } else { up }, window, cx);
}

fn down(step: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    pointer.at = step.at();
    pointer.down = true;
    left(Left::Down, step.at(), step.held(), 1, window, cx);
    Answer::Done
}

fn up(step: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    pointer.at = step.at();
    pointer.down = false;
    left(Left::Up, step.at(), step.held(), 1, window, cx);
    Answer::Done
}

/// A click, or two clicks with the second counted as a double click.
fn click(step: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    come_to(step.at(), pointer, window, cx);
    let clicks = if step.event == NrScriptEvent::DoubleClick {
        2
    } else {
        1
    };
    for count in 1..=clicks {
        for button in [Left::Down, Left::Up] {
            left(button, step.at(), step.held(), count, window, cx);
        }
    }
    Answer::Done
}

fn right_click(step: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    come_to(step.at(), pointer, window, cx);
    for input in press(step.at(), MouseButton::Right, Modifiers::default(), 1) {
        dispatch(input, window, cx);
    }
    Answer::Done
}

fn middle_click(step: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    come_to(step.at(), pointer, window, cx);
    for input in press(step.at(), MouseButton::Middle, Modifiers::default(), 1) {
        dispatch(input, window, cx);
    }
    Answer::Done
}

/// Scrolls at the step's point by `dy` down and `dx` across. Precise is one trackpad event in
/// points; Lines is mouse wheel clicks; Swipe is a full two-finger gesture of the same travel,
/// followed by momentum that adds another third.
fn wheel(step: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    pointer.at = step.at();
    let turn = |across: f64, down: f64, lines: bool, phase: TouchPhase| {
        let travel = point(-across as f32, -down as f32);
        PlatformInput::Scroll(ScrollEvent {
            position: step.at(),
            delta: if lines {
                ScrollDelta::Lines(travel)
            } else {
                ScrollDelta::Pixels(point(travel.x.into(), travel.y.into()))
            },
            modifiers: step.held(),
            touch_phase: phase,
            source: Default::default(),
        })
    };
    let (across, down) = (step.dx, step.dy);
    let turns = match step.event {
        NrScriptEvent::Lines => vec![turn(across, down, true, TouchPhase::Moved)],
        NrScriptEvent::Swipe => {
            let mut turns = vec![turn(0.0, 0.0, false, TouchPhase::Started)];
            turns.extend((0..3).map(|_| turn(across / 3.0, down / 3.0, false, TouchPhase::Moved)));
            turns.push(turn(0.0, 0.0, false, TouchPhase::Ended));
            turns.extend((0..3).map(|_| turn(across / 9.0, down / 9.0, false, TouchPhase::Moved)));
            turns
        }
        _ => vec![
            turn(across, down, false, TouchPhase::Started),
            turn(0.0, 0.0, false, TouchPhase::Ended),
        ],
    };
    for input in turns {
        dispatch(input, window, cx);
    }
    Answer::Done
}

/// Drops a file from another app at the step's point: the drag enters the window, then
/// releases.
fn dropped(step: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    pointer.at = step.at();
    let paths = ExternalPaths(vec![std::path::PathBuf::from(&step.text)].into());
    let drop = [
        FileDropEvent::Entered {
            position: step.at(),
            paths,
        },
        FileDropEvent::Submit {
            position: step.at(),
        },
    ];
    for event in drop {
        dispatch(PlatformInput::FileDrop(event), window, cx);
    }
    Answer::Done
}

/// Moves between the start and end of a scripted pinch.
const PINCH_MOVES: u32 = 5;

/// Pinches at the step's point, opening by `dy` (closing when negative): a start, equal moves,
/// and an end.
fn pinch(step: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    pointer.at = step.at();
    let made = |delta: f64, phase: TouchPhase| {
        PlatformInput::Pinch(PinchEvent {
            position: step.at(),
            delta: delta as f32,
            modifiers: step.held(),
            phase,
        })
    };
    let share = step.dy / f64::from(PINCH_MOVES);
    let mut events = vec![made(0.0, TouchPhase::Started)];
    events.extend((0..PINCH_MOVES).map(|_| made(share, TouchPhase::Moved)));
    events.push(made(0.0, TouchPhase::Ended));
    for input in events {
        dispatch(input, window, cx);
    }
    Answer::Done
}

/// GPUI's keystroke for a core key, using `characters` when the key has no name.
fn keystroke(key: NrKey, characters: &str, held: Modifiers) -> Keystroke {
    let typed =
        (!held.platform && !held.control && !characters.is_empty()).then(|| characters.to_owned());
    match kit::keys::gpui_name(key) {
        Some(name) => Keystroke {
            modifiers: held,
            key: name,
            key_char: None,
        },
        None => Keystroke {
            modifiers: held,
            key: characters.to_lowercase(),
            key_char: typed,
        },
    }
}

/// Presses and releases a key. While a native view has focus (a live page's web view), the key
/// is posted as an `NSEvent` so it takes AppKit's route: key equivalents, the focused view, then
/// the menu bar. ⌘ or Control keys no GPUI view takes go the same way, as GPUI forwards a
/// user's, because AppKit owns menu items like Quit and Hide.
fn press_key(key: NrKey, characters: &str, held: Modifiers, window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
    #[cfg(target_os = "macos")]
    let posted = |window: &mut Window| {
        kit::platform::post_key(window, key, characters, kit::keys::held(&held))
    };
    #[cfg(target_os = "macos")]
    if page_has_keys(window, cx) && posted(window) {
        return;
    }
    if !window.dispatch_keystroke(keystroke(key, characters, held), cx)
        && (held.platform || held.control)
    {
        #[cfg(target_os = "macos")]
        posted(window);
    }
}

/// Whether the open file is a live page or PDF whose screen has GPUI focus and whose view has
/// AppKit focus. Native focus follows GPUI focus only on draw, and a check's window may not have
/// drawn since focus moved.
#[cfg(target_os = "macos")]
fn page_has_keys(window: &Window, cx: &App) -> bool {
    let hosting = workspace(window)
        .read(cx)
        .opened
        .as_ref()
        .is_some_and(|opened| {
            matches!(opened.kind, NrOpenedViewKind::Html | NrOpenedViewKind::Pdf)
                && opened
                    .screen
                    .keys(cx)
                    .is_some_and(|keys| keys.is_focused(window))
        });
    hosting && kit::platform::hosted_has_keys(window)
}

fn key(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    press_key(step.key, &step.text, step.held(), window, cx);
    Answer::Done
}

/// Types each character in turn, holding ⇧ for capitals.
fn typed(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    for character in step.text.chars() {
        let held = Modifiers {
            shift: character.is_uppercase(),
            ..Modifiers::default()
        };
        press_key(NrKey::Other, &character.to_string(), held, window, cx);
    }
    Answer::Done
}

/// Resolves an `at:` step to its event at the centre of the named row or card. Draws first, as
/// `dispatch` does, because a screen from the core's last change has no layout until drawn.
fn at(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    window.draw(cx).clear(cx);
    let found = workspace(window).read(cx).point_of(&step.name, cx);
    let made = nr_script_at(
        found.is_some(),
        found.map_or(0.0, |at| f64::from(at.x)),
        found.map_or(0.0, |at| f64::from(at.y)),
    );
    Answer::Then(Step::of(&made))
}

// MARK: Pictures

/// `ev:shot`: the window with its overlays, or without them for `alone`.
fn shot(step: &Step, _: &mut Pointer, _: &mut Window, _: &mut App) -> Answer {
    Answer::Picture {
        path: step.text.clone(),
        whole: !step.alone,
    }
}

/// The run's window snapshot (`NR_SNAPSHOT_DIR`), without menus and tooltips.
fn window_picture(step: &Step, _: &mut Pointer, _: &mut Window, _: &mut App) -> Answer {
    Answer::Picture {
        path: step.text.clone(),
        whole: false,
    }
}

// MARK: Menus

/// Right-clicks at the step's point and reports the frames of the menus it opened.
fn report_right_click(
    step: &Step,
    pointer: &mut Pointer,
    window: &mut Window,
    cx: &mut App,
) -> Answer {
    right_click(step, pointer, window, cx);
    say_menus(&step.text, &step.name, window, cx);
    Answer::Done
}

fn report_menus(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    say_menus(&step.text, &step.name, window, cx);
    Answer::Done
}

fn save_menus(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let frames = workspace(window).read(cx).overlay.menu_frames(cx);
    menu_pictures(&frames, &step.text, window, cx);
    Answer::Done
}

/// Reports each open menu's frame (x, y from the window's top-left, width, height) tagged
/// `tag`, and saves snapshots as `<stem><n>.png` unless `stem` is empty.
fn say_menus(tag: &str, stem: &str, window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
    let frames = workspace(window).read(cx).overlay.menu_frames(cx);
    let placed: Vec<String> = frames
        .iter()
        .map(|frame| {
            format!(
                "{},{},{},{}",
                f32::from(frame.origin.x),
                f32::from(frame.origin.y),
                f32::from(frame.size.width),
                f32::from(frame.size.height)
            )
        })
        .collect();
    eprintln!("menus\t{tag}\t{}", placed.join("\t"));
    if !stem.is_empty() {
        menu_pictures(&frames, stem, window, cx);
    }
}

/// Crops each menu from the current frame and saves it as `<stem><n>.png`.
#[cfg(all(feature = "pictures", not(target_os = "linux")))]
fn menu_pictures(frames: &[Bounds<Pixels>], stem: &str, window: &mut Window, cx: &mut App) {
    window.draw(cx).clear(cx);
    let scale = window.scale_factor();
    let image = match window.render_to_image() {
        Ok(image) => image,
        Err(error) => {
            eprintln!("gpui\tno picture of the window: {error}");
            return;
        }
    };
    for (index, frame) in frames.iter().enumerate() {
        let pixel = |value: Pixels| (f32::from(value) * scale).round().max(0.0) as u32;
        let (x, y) = (pixel(frame.origin.x), pixel(frame.origin.y));
        let width = pixel(frame.size.width).min(image.width().saturating_sub(x));
        let height = pixel(frame.size.height).min(image.height().saturating_sub(y));
        let part = image::imageops::crop_imm(&image, x, y, width, height).to_image();
        let path = format!("{stem}{index}.png");
        if let Err(error) = kit::display::save(&part, &path) {
            eprintln!("gpui\tthe picture {path} was not saved: {error}");
        }
    }
}

#[cfg(not(all(feature = "pictures", not(target_os = "linux"))))]
fn menu_pictures(frames: &[Bounds<Pixels>], stem: &str, _: &mut Window, _: &mut App) {
    if !frames.is_empty() {
        eprintln!("gpui\tno pictures of menus here: {stem}* were not saved");
    }
}

// MARK: Reports

fn observe(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let probe = c(&workspace(window).read(cx).probe(window, cx).json());
    // SAFETY: a NUL-terminated probe.
    unsafe { nr_script_observe(probe.as_ptr()) };
    Answer::Done
}

fn state(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let probe = c(&workspace(window).read(cx).probe(window, cx).json());
    // SAFETY: a NUL-terminated probe.
    unsafe { nr_script_state(probe.as_ptr()) };
    Answer::Done
}

/// Reports the scroll offset under the last point, for the core to log once it settles.
fn look_under(_: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    window.draw(cx).clear(cx);
    let (offset, shown, height) = workspace(window)
        .read(cx)
        .scrolled_under(pointer.at, window, cx);
    nr_script_looked(f64::from(offset), f64::from(shown), f64::from(height), true);
    Answer::Done
}

/// Reports the view under the last point, the focused view, and the app's windows on screen
/// (just the main window, since overlays draw inside it), each as `name:x,y,widthxheight`.
fn say(_: &Step, pointer: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let workspace = workspace(window).read(cx);
    let size = window.viewport_size();
    eprintln!(
        "ev\tpoint {},{}\twindow Workspace main\thit {}\tkeys Workspace {}\twindows WorkspaceWindow:0,0,{}x{}",
        f32::from(pointer.at.x) as i64,
        f32::from(pointer.at.y) as i64,
        workspace.hit(pointer.at, window, cx),
        workspace.probe(window, cx).keys,
        f32::from(size.width) as i64,
        f32::from(size.height) as i64,
    );
    Answer::Done
}

// MARK: App

fn quit(_: &Step, _: &mut Pointer, _: &mut Window, cx: &mut App) -> Answer {
    cx.quit();
    Answer::Done
}

fn theme_changed(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    workspace(window).update(cx, |workspace, cx| workspace.theme_changed(window, cx));
    Answer::Done
}

/// Simulates a system appearance change to light (0), dark (1) or the real system value. The
/// theme follows it when the user's appearance follows the system.
fn look(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let dark = match step.number {
        0 => false,
        1 => true,
        _ => matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    if nr_appearance_system(dark) {
        workspace(window).update(cx, |workspace, cx| workspace.theme_changed(window, cx));
    }
    Answer::Done
}

/// Shows the dialog the core describes (`nr_prompt`).
fn dialog(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    workspace(window).update(cx, |workspace, cx| workspace.show_dialog(window, cx));
    Answer::Done
}

/// Starts an in-place rename of the step's file, as after New Folder.
fn rename(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let id = step.text.clone();
    workspace(window).update(cx, |workspace, cx| workspace.rename(&id, window, cx));
    Answer::Done
}
