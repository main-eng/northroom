//! Synthetic clicks and keys for native views under the GPUI view (a web view, PDF pages). Each
//! is a real `NSEvent` posted to the application's own queue, so it goes through key
//! equivalents, the menu bar and the first responder like real input. No event is posted to
//! the system.

use super::appkit::{Id, application, ns_string};
use crate::nr::{NR_HELD_COMMAND, NR_HELD_CONTROL, NR_HELD_OPTION, NR_HELD_SHIFT, NrKey};
use objc2::runtime::Bool;
use objc2::{class, msg_send};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Keys without a character of their own: AppKit's key code and the characters it reports.
const NAMED: [(NrKey, u16, &str); 27] = [
    (NrKey::Return, 36, "\r"),
    (NrKey::Tab, 48, "\t"),
    (NrKey::Space, 49, " "),
    (NrKey::Escape, 53, "\u{1b}"),
    (NrKey::Backspace, 51, "\u{7f}"),
    (NrKey::Delete, 117, "\u{F728}"),
    (NrKey::Up, 126, "\u{F700}"),
    (NrKey::Down, 125, "\u{F701}"),
    (NrKey::Left, 123, "\u{F702}"),
    (NrKey::Right, 124, "\u{F703}"),
    (NrKey::Home, 115, "\u{F729}"),
    (NrKey::End, 119, "\u{F72B}"),
    (NrKey::PageUp, 116, "\u{F72C}"),
    (NrKey::PageDown, 121, "\u{F72D}"),
    (NrKey::Insert, 114, "\u{F746}"),
    (NrKey::F1, 122, "\u{F704}"),
    (NrKey::F2, 120, "\u{F705}"),
    (NrKey::F3, 99, "\u{F706}"),
    (NrKey::F4, 118, "\u{F707}"),
    (NrKey::F5, 96, "\u{F708}"),
    (NrKey::F6, 97, "\u{F709}"),
    (NrKey::F7, 98, "\u{F70A}"),
    (NrKey::F8, 100, "\u{F70B}"),
    (NrKey::F9, 101, "\u{F70C}"),
    (NrKey::F10, 109, "\u{F70D}"),
    (NrKey::F11, 103, "\u{F70E}"),
    (NrKey::F12, 111, "\u{F70F}"),
];

/// The ANSI key code for each unshifted character.
const ANSI: [(char, u16); 48] = [
    ('a', 0),
    ('s', 1),
    ('d', 2),
    ('f', 3),
    ('h', 4),
    ('g', 5),
    ('z', 6),
    ('x', 7),
    ('c', 8),
    ('v', 9),
    ('b', 11),
    ('q', 12),
    ('w', 13),
    ('e', 14),
    ('r', 15),
    ('y', 16),
    ('t', 17),
    ('1', 18),
    ('2', 19),
    ('3', 20),
    ('4', 21),
    ('6', 22),
    ('5', 23),
    ('=', 24),
    ('9', 25),
    ('7', 26),
    ('-', 27),
    ('8', 28),
    ('0', 29),
    (']', 30),
    ('o', 31),
    ('u', 32),
    ('[', 33),
    ('i', 34),
    ('p', 35),
    ('l', 37),
    ('j', 38),
    ('\'', 39),
    ('k', 40),
    (';', 41),
    ('\\', 42),
    (',', 43),
    ('/', 44),
    ('n', 45),
    ('m', 46),
    ('.', 47),
    ('`', 50),
    (' ', 49),
];

/// AppKit's key code for `character` on the ANSI keyboard.
fn ansi(character: char) -> Option<u16> {
    ANSI.iter()
        .find(|(typed, _)| *typed == character)
        .map(|(_, code)| *code)
}

/// AppKit's modifier flags for the keys held (`NR_HELD_*`).
fn flags(held: u32) -> usize {
    [
        (NR_HELD_SHIFT, 1 << 17),
        (NR_HELD_CONTROL, 1 << 18),
        (NR_HELD_OPTION, 1 << 19),
        (NR_HELD_COMMAND, 1 << 20),
    ]
    .iter()
    .filter(|(bit, _)| held & bit != 0)
    .fold(0, |flags, (_, flag)| flags | flag)
}

fn gpui_view(window: &gpui::Window) -> Id {
    match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::AppKit(handle)) => handle.ns_view.as_ptr().cast(),
        _ => panic!("a GPUI window on a Mac has an AppKit view"),
    }
}

/// Seconds since boot, for an event's timestamp.
fn now() -> f64 {
    // SAFETY: reads this process's info.
    unsafe {
        let information: Id = msg_send![class!(NSProcessInfo), processInfo];
        msg_send![information, systemUptime]
    }
}

/// Whether a native view under GPUI's view is `window`'s first responder, so keys go through
/// AppKit instead of GPUI focus.
pub fn hosted_has_keys(window: &gpui::Window) -> bool {
    let view = gpui_view(window);
    // SAFETY: reads GPUI's view and window on the main thread.
    unsafe {
        let native: Id = msg_send![view, window];
        let keys: Id = msg_send![native, firstResponder];
        if keys.is_null() || keys == view || keys == native {
            return false;
        }
        let is_view: Bool = msg_send![keys, isKindOfClass: class!(NSView)];
        is_view.as_bool()
    }
}

/// Posts key down and up for `key` (for `Other`, the key that types `characters`) with `held`
/// modifiers to `window`. False when no key types `characters`.
pub fn post_key(window: &gpui::Window, key: NrKey, characters: &str, held: u32) -> bool {
    let (code, characters) = match NAMED.iter().find(|(named, _, _)| *named == key) {
        Some((_, code, characters)) => (*code, (*characters).to_owned()),
        None => {
            let mut typed = characters.chars();
            let (Some(character), None) = (typed.next(), typed.next()) else {
                return false;
            };
            let Some(code) = ansi(character.to_ascii_lowercase()) else {
                return false;
            };
            (code, character.to_string())
        }
    };
    let unmodified = characters.to_lowercase();
    let view = gpui_view(window);
    // SAFETY: on the main thread; the events are autoreleased and the queue retains them.
    unsafe {
        let native: Id = msg_send![view, window];
        let number: isize = msg_send![native, windowNumber];
        for kind in [10_usize, 11] {
            let event: Id = msg_send![
                class!(NSEvent),
                keyEventWithType: kind,
                location: Point::default(),
                modifierFlags: flags(held),
                timestamp: now(),
                windowNumber: number,
                context: std::ptr::null_mut::<objc2::runtime::AnyObject>(),
                characters: ns_string(&characters),
                charactersIgnoringModifiers: ns_string(&unmodified),
                isARepeat: Bool::NO,
                keyCode: code
            ];
            let _: () = msg_send![application(), postEvent: event, atStart: Bool::NO];
        }
    }
    true
}

/// Left button down or up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Down,
    Up,
}

/// Posts a left button event at `x`, `y` (window points from the top left) with click count
/// `clicks` and `held` modifiers to `window`.
pub fn post_press(window: &gpui::Window, button: Button, x: f64, y: f64, clicks: isize, held: u32) {
    let view = gpui_view(window);
    // SAFETY: on the main thread; the event is autoreleased and the queue retains it.
    unsafe {
        let native: Id = msg_send![view, window];
        let number: isize = msg_send![native, windowNumber];
        let content: Id = msg_send![view, superview];
        let bounds: Rect = msg_send![content, bounds];
        let (kind, pressure) = match button {
            Button::Down => (1_usize, 1.0_f32),
            Button::Up => (2, 0.0),
        };
        let event: Id = msg_send![
            class!(NSEvent),
            mouseEventWithType: kind,
            location: Point { x, y: bounds.height - y },
            modifierFlags: flags(held),
            timestamp: now(),
            windowNumber: number,
            context: std::ptr::null_mut::<objc2::runtime::AnyObject>(),
            eventNumber: 0_isize,
            clickCount: clicks,
            pressure: pressure
        ];
        let _: () = msg_send![application(), postEvent: event, atStart: Bool::NO];
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Point {
    x: f64,
    y: f64,
}

// SAFETY: Core Graphics' `CGPoint`, two doubles.
unsafe impl objc2::encode::Encode for Point {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

// SAFETY: Core Graphics' `CGRect`, a `CGPoint` then a `CGSize`, each two doubles.
unsafe impl objc2::encode::Encode for Rect {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CGRect",
        &[
            objc2::encode::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]),
            objc2::encode::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]),
        ],
    );
}
