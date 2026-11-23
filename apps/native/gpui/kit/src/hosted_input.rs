//! Pointer and key input for an interactive platform view hosted under the GPUI view (`hosted`),
//! such as a live page's web view or a PDF. AppKit sends every event to GPUI's view, which is in
//! front, so the current event is forwarded to the hosted view as AppKit would have delivered it.
//!
//! A press goes to the view under it, which takes key focus; the drags and release that follow
//! go to the same view. Scrolls and pinches go to the view under the pointer. Mouse moves need no
//! forwarding because tracking areas receive them regardless of what is in front. AppKit focus
//! follows GPUI focus (`follow_focus`).

#[cfg(target_os = "macos")]
pub use mac::{Forward, follow_focus};

/// No AppKit focus to sync outside macOS.
#[cfg(not(target_os = "macos"))]
pub fn follow_focus<T: 'static>(
    _: *mut std::ffi::c_void,
    _: &gpui::FocusHandle,
    _: &mut gpui::Window,
    _: &mut gpui::Context<T>,
) {
}

/// No platform views are hosted outside macOS, so nothing is forwarded.
#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct Forward;

#[cfg(not(target_os = "macos"))]
impl Forward {
    pub fn pass_on(&mut self, _: *mut std::ffi::c_void, _: bool, _: &gpui::Window) -> bool {
        false
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use gpui::{Context, FocusHandle, Window};
    use objc2::runtime::{AnyObject, Bool, Sel};
    use objc2::{class, msg_send, sel};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::c_void;

    type Id = *mut AnyObject;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    struct Point {
        x: f64,
        y: f64,
    }

    // SAFETY: matches Core Graphics' `CGPoint`, two doubles.
    unsafe impl objc2::encode::Encode for Point {
        const ENCODING: objc2::encode::Encoding =
            objc2::encode::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }

    /// The `NSResponder` method for each forwarded `NSEventType`: presses, releases and drags of
    /// each button, the scroll wheel and trackpad pinch.
    fn method(kind: usize) -> Option<(Sel, Phase)> {
        Some(match kind {
            1 => (sel!(mouseDown:), Phase::Press),
            2 => (sel!(mouseUp:), Phase::Release),
            3 => (sel!(rightMouseDown:), Phase::Press),
            4 => (sel!(rightMouseUp:), Phase::Release),
            6 => (sel!(mouseDragged:), Phase::Drag),
            7 => (sel!(rightMouseDragged:), Phase::Drag),
            22 => (sel!(scrollWheel:), Phase::Gesture),
            25 => (sel!(otherMouseDown:), Phase::Press),
            26 => (sel!(otherMouseUp:), Phase::Release),
            27 => (sel!(otherMouseDragged:), Phase::Drag),
            30 => (sel!(magnifyWithEvent:), Phase::Gesture),
            _ => return None,
        })
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Phase {
        Press,
        Drag,
        Release,
        /// A scroll or pinch, sent to the view under the pointer.
        Gesture,
    }

    /// GPUI's view of `window`.
    fn gpui_view(window: &Window) -> Option<Id> {
        match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::AppKit(handle)) => Some(handle.ns_view.as_ptr().cast()),
            _ => None,
        }
    }

    /// Makes `to` the first responder of `window`, unless it or a subview already is.
    fn give_keys(to: Id, window: &Window) {
        let Some(gpui) = gpui_view(window) else {
            return;
        };
        // SAFETY: live views and their window, used on the main thread.
        unsafe {
            let ns_window: Id = msg_send![gpui, window];
            let keys: Id = msg_send![ns_window, firstResponder];
            let is_view: Bool = msg_send![keys, isKindOfClass: class!(NSView)];
            if is_view.as_bool() {
                let inside: Bool = msg_send![keys, isDescendantOf: to];
                if inside.as_bool() {
                    return;
                }
            }
            let _: Bool = msg_send![ns_window, makeFirstResponder: to];
        }
    }

    /// Keeps AppKit focus on `view` (an `NSView` under GPUI's view) while `focus`, the screen
    /// hosting it, has GPUI focus, so every key reaches the view first (`crate::platform`'s key
    /// handling defers to it). When GPUI focus moves elsewhere, such as to the palette, GPUI's
    /// view takes AppKit focus back; it returns when GPUI focus does.
    pub fn follow_focus<T: 'static>(
        view: *mut c_void,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let view: Id = view.cast();
        cx.on_focus(focus, window, move |_, window, _| give_keys(view, window))
            .detach();
        let handle = focus.clone();
        cx.on_blur(focus, window, move |_, window, _| {
            // GPUI also reports focus lost when the window is deactivated; ignore that case.
            if !handle.is_focused(window)
                && let Some(gpui) = gpui_view(window)
            {
                give_keys(gpui, window);
            }
        })
        .detach();
    }

    /// Forwards events to one hosted view, remembering which subview took the press so it gets
    /// the drags and release.
    #[derive(Default)]
    pub struct Forward {
        pressed: Option<usize>,
    }

    impl Forward {
        /// Forwards the current event to `view` (an `NSView` under the GPUI view) if it is a
        /// press, drag, release, scroll or pinch. Presses, scrolls and pinches are only forwarded
        /// when `over` (the pointer is over the view with no GPUI content in front). Returns
        /// whether the event was forwarded.
        pub fn pass_on(&mut self, view: *mut c_void, over: bool, window: &Window) -> bool {
            let view: Id = view.cast();
            let gpui: Id =
                match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
                    Ok(RawWindowHandle::AppKit(handle)) => handle.ns_view.as_ptr().cast(),
                    _ => return false,
                };
            let Some(handed) = Handed::now(gpui) else {
                return false;
            };
            // SAFETY: `handed.event` is live.
            let kind: usize = unsafe { msg_send![handed.event, type] };
            let Some((method, phase)) = method(kind) else {
                return false;
            };
            let target = match phase {
                Phase::Press | Phase::Gesture => over.then(|| self.hit(view, gpui, &handed, phase)),
                Phase::Drag | Phase::Release => self.held(phase),
            };
            let Some(target) = target else {
                return false;
            };
            // SAFETY: the event belongs to the application and lives while it is dispatched;
            // the target view is live.
            let _: Id =
                unsafe { msg_send![target, performSelector: method, withObject: handed.event] };
            true
        }

        /// The subview of `view` under a press, scroll or pinch (or `view` itself). A press
        /// remembers the hit view for its drags and release, and gives it key focus as AppKit
        /// would.
        fn hit(&mut self, view: Id, gpui: Id, handed: &Handed, phase: Phase) -> Id {
            // SAFETY: AppKit calls on the main thread with live views and a live event.
            unsafe {
                let in_window: Point = msg_send![handed.event, locationInWindow];
                let content: Id = msg_send![gpui, superview];
                let at: Point = msg_send![content, convertPoint: in_window, fromView: std::ptr::null_mut::<AnyObject>()];
                let hit: Id = msg_send![view, hitTest: at];
                let hit = if hit.is_null() { view } else { hit };
                if phase == Phase::Press {
                    self.pressed = Some(hit as usize);
                    // The pressed view takes key focus, as in AppKit.
                    let _: Bool = msg_send![handed.window, makeFirstResponder: hit];
                }
                hit
            }
        }

        /// The view the press went to, for its drags and release.
        fn held(&mut self, phase: Phase) -> Option<Id> {
            let pressed = self.pressed?;
            if phase == Phase::Release {
                self.pressed = None;
            }
            Some(pressed as Id)
        }
    }

    /// The event AppKit is dispatching and its window.
    struct Handed {
        event: Id,
        window: Id,
    }

    impl Handed {
        /// The current event, if it belongs to the window of `gpui` (the GPUI view).
        fn now(gpui: Id) -> Option<Self> {
            // SAFETY: AppKit calls on the main thread with a live view; the event belongs to the
            // application and lives while it is dispatched.
            unsafe {
                let application: Id = msg_send![class!(NSApplication), sharedApplication];
                let event: Id = msg_send![application, currentEvent];
                if event.is_null() {
                    return None;
                }
                let window: Id = msg_send![gpui, window];
                let from: Id = msg_send![event, window];
                (from == window).then_some(Self { event, window })
            }
        }
    }
}
