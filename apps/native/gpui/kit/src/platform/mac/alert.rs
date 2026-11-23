//! An app-modal alert (`NSAlert`) with buttons in the given order and chosen Return and Escape
//! buttons. GPUI's prompt is a window sheet that always gives Return to the first button.

use super::appkit::{Id, ns_string};
use objc2::{class, msg_send};

/// Maps to `NSAlertStyle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertStyle {
    Informational,
    Critical,
}

pub struct Alert<'a> {
    pub style: AlertStyle,
    pub message: &'a str,
    pub detail: &'a str,
    /// In order; empty shows AppKit's OK.
    pub buttons: &'a [String],
    /// The button Return presses; with none, the first button keeps it.
    pub return_button: Option<usize>,
    /// The button Escape presses.
    pub escape_button: Option<usize>,
}

/// Shows the alert and returns the index of the pressed button. It runs the modal loop, so call
/// it from a task, never inside a GPUI update.
pub fn run_alert(alert: &Alert) -> usize {
    // `NSAlertFirstButtonReturn`: the first button returns 1000, the next 1001.
    const FIRST: isize = 1000;
    // SAFETY: the alert is created, run and released on the main thread.
    unsafe {
        let shown: Id = msg_send![class!(NSAlert), new];
        let style: usize = match alert.style {
            AlertStyle::Informational => 1,
            AlertStyle::Critical => 2,
        };
        let _: () = msg_send![shown, setAlertStyle: style];
        let _: () = msg_send![shown, setMessageText: ns_string(alert.message)];
        let _: () = msg_send![shown, setInformativeText: ns_string(alert.detail)];
        for (place, title) in alert.buttons.iter().enumerate() {
            let button: Id = msg_send![shown, addButtonWithTitle: ns_string(title)];
            let key = if Some(place) == alert.return_button {
                Some("\r")
            } else if Some(place) == alert.escape_button {
                Some("\u{1b}")
            } else if place == 0 && alert.return_button.is_some() {
                // Return belongs to another button, so take it off the first.
                Some("")
            } else {
                None
            };
            if let Some(key) = key {
                let _: () = msg_send![button, setKeyEquivalent: ns_string(key)];
            }
        }
        let answer: isize = msg_send![shown, runModal];
        let _: () = msg_send![shown, release];
        (answer - FIRST) as usize
    }
}
