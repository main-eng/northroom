//! The host on Windows and Linux, under the same names as the Mac's. `engine` runs the system web
//! view (WebView2 on Windows, WebKitGTK on Linux) in a hidden window. `pages` answers the page
//! host's scheme through the core, blocks the network, runs scripts as futures and snapshots
//! what the view drew. A shell shows a live page by drawing those snapshots and forwarding the
//! user's pointer and key events (`Input`). `pictures` encodes them with the `image` crate.

mod files;
mod pages;
mod pictures;

#[cfg(target_os = "windows")]
#[path = "webview2.rs"]
mod engine;
#[cfg(target_os = "linux")]
#[path = "webkitgtk.rs"]
mod engine;

use std::cell::Cell;

pub use pages::{HtmlPage, Said, model_still};
pub use pictures::{decode, png};

thread_local! {
    /// Web views currently open on the main thread.
    static OPEN: Cell<usize> = const { Cell::new(0) };
}

/// Number of open web views. GPUI's loop polls the main context more often while any are open.
#[cfg(target_os = "linux")]
pub fn open_views() -> usize {
    OPEN.with(Cell::get)
}

/// Counts one open web view for as long as it lives.
pub(crate) struct Opened;

impl Opened {
    pub fn new() -> Self {
        OPEN.with(|open| open.set(open.get() + 1));
        Self
    }
}

impl Drop for Opened {
    fn drop(&mut self) {
        let _ = OPEN.try_with(|open| open.set(open.get().saturating_sub(1)));
    }
}

/// An event a web view reports to its owner.
pub(crate) enum WebEvent {
    /// The entry page finished loading.
    Loaded,
    /// The entry page failed to load, with the platform's message.
    Failed(String),
    /// The web content process ended.
    Ended,
    /// A page posted JSON through the bridge.
    Message(String),
}

/// A pointer or key event for a live page, in CSS pixels from the page's top left.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Press {
        x: f64,
        y: f64,
        button: Button,
        clicks: u32,
    },
    Release {
        x: f64,
        y: f64,
        button: Button,
    },
    Move {
        x: f64,
        y: f64,
        /// The button held during a drag.
        held: Option<Button>,
    },
    /// A scroll in pixels. Positive `dy` scrolls down the page and positive `dx` to the right.
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    /// A key press: GPUI's key name ("a", "enter", "backspace", "left" …), the text it types
    /// if any, and the held modifiers.
    Key {
        key: String,
        text: Option<String>,
        modifiers: Modifiers,
    },
    KeyUp {
        key: String,
        modifiers: Modifiers,
    },
    /// Text an input method committed.
    Text(String),
    /// The view gained (`true`) or lost keyboard focus.
    Focus(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub platform: bool,
}
