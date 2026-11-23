//! A file the app has no view for, shown read-only by Quick Look: Word and Pages documents,
//! PowerPoint and Keynote decks, fonts, raw photos, 3D scenes and the other kinds
//! `workspace::is_system_preview_file` names. It fills its pane, beside its column or filling
//! the tab.
//!
//! Quick Look's view sits under the window's GPUI view (`kit::hosted`), which leaves a hole for
//! it. Presses, drags and scrolls GPUI receives over the hole are forwarded
//! (`kit::hosted_input`). Only macOS has this view; elsewhere the core shows the file's details
//! and it opens in its own application.
//!
//! Quick Look's PowerPoint importer logs `NSMapGet: map table argument is NULL` once per deck.
//! A bare `QLPreviewView` in a standalone program logs the same, so it is harmless.

use super::html::{hosted, pass_pointer_on};
use super::{Ask, Screen};
use crate::script::probe::Probe;
use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Hitbox, HitboxBehavior, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, Styled, Window, canvas,
};
use kit::controls::placed;
use kit::hosted_input::Forward;
use kit::nr::*;
use std::rc::Rc;

pub struct SystemPreview {
    frame: Bounds<Pixels>,
    focus: FocusHandle,
    /// The file shown.
    path: String,
    shown: mac::QuickLook,
    hosted: kit::hosted::HostedView,
    forward: Forward,
}

impl SystemPreview {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shown = mac::QuickLook::new();
        let focus = cx.focus_handle();
        kit::hosted_input::follow_focus(shown.view(), &focus, window, cx);
        let mut preview = Self {
            frame: Bounds::default(),
            focus,
            path: String::new(),
            hosted: hosted(shown.view(), window),
            shown,
            forward: Forward::default(),
        };
        preview.reload(window, cx);
        preview
    }

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        self.hosted.place(self.frame, window);
        self.listen(hitbox, window, cx);
    }

    /// Forwards presses, drags, releases and scrolls over the preview. A press gives it the
    /// keys.
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        pass_pointer_on(window, hitbox, move |hitbox, window, cx| {
            let over = hitbox.is_hovered(window);
            entity.update(cx, |preview, cx| {
                let view = preview.shown.view();
                if preview.forward.pass_on(view, over, window) && over {
                    window.focus(&preview.focus, cx);
                }
            });
        });
    }
}

impl EventEmitter<Ask> for SystemPreview {}

impl Screen for SystemPreview {
    /// Reads the open file from the core again; a different file replaces this one.
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let opened = kit::core::text(&nr_opened_path());
        if opened.is_empty() || opened == self.path {
            return;
        }
        self.shown.open(&opened);
        self.path = opened;
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, _: &App) {
        if self.focus.is_focused(window) {
            probe.keys = "SystemPreviewFileView".to_owned();
        }
    }
}

impl Render for SystemPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui::StatefulInteractiveElement;
        let preview = cx.entity();
        let shows: kit::hosted::Shows = {
            let view = self.shown.view();
            Rc::new(move || super::pdf::picture_of_view(view))
        };
        // VoiceOver reads Quick Look's view where this element stands.
        let placed = placed("preview", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .role(gpui::Role::Group)
            .accessibility_id(self.hosted.read_in_place());
        placed.child(
            canvas(
                move |bounds, window, _| {
                    // The preview shows through the window here, and in screenshots of it.
                    kit::hosted::cut_used(bounds, Some(shows.clone()));
                    window.insert_hitbox(bounds, HitboxBehavior::Normal)
                },
                move |_, hitbox, window, cx| {
                    preview.update(cx, |preview, cx| preview.paint(&hitbox, window, cx))
                },
            )
            .size_full(),
        )
    }
}

/// Quick Look's preview view, through plain `msg_send!`s.
mod mac {
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    use std::ffi::{CString, c_void};

    type Id = *mut AnyObject;

    // `QLPreviewView` is Quick Look's UI, part of the Quartz umbrella framework.
    #[link(name = "Quartz", kind = "framework")]
    unsafe extern "C" {}

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    // SAFETY: matches Core Graphics' `CGRect`: a `CGPoint` then a `CGSize`, each two doubles.
    unsafe impl objc2::encode::Encode for Rect {
        const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
            "CGRect",
            &[
                objc2::encode::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]),
                objc2::encode::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]),
            ],
        );
    }

    /// `QLPreviewViewStyleNormal`: the preview alone, without Quick Look's controls.
    const STYLE_NORMAL: usize = 0;

    /// A `QLPreviewView` showing one file.
    pub struct QuickLook {
        view: Id,
    }

    impl QuickLook {
        pub fn new() -> Self {
            // SAFETY: plain AppKit and Quick Look calls on the main thread. The view is released
            // in `drop`.
            unsafe {
                let view: Id = msg_send![class!(QLPreviewView), alloc];
                let view: Id = msg_send![
                    view,
                    initWithFrame: Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 },
                    style: STYLE_NORMAL
                ];
                // The view outlives any window it leaves; `drop` closes it.
                let _: () = msg_send![view, setShouldCloseWithWindow: Bool::NO];
                Self { view }
            }
        }

        /// The view, for the window that hosts it.
        pub fn view(&self) -> *mut c_void {
            self.view.cast()
        }

        /// Shows the file at `path`.
        pub fn open(&self, path: &str) {
            let path = CString::new(path).expect("a path has no NUL");
            // SAFETY: plain Foundation and Quick Look calls on the main thread; the URL is
            // autoreleased and held by the view.
            unsafe {
                let path: Id = msg_send![class!(NSString), stringWithUTF8String: path.as_ptr()];
                let url: Id = msg_send![class!(NSURL), fileURLWithPath: path];
                let _: () = msg_send![self.view, setPreviewItem: url];
            }
        }
    }

    impl Drop for QuickLook {
        fn drop(&mut self) {
            // SAFETY: the view from `new`. Closing it ends the preview before release, and the
            // window's reference belongs to the hosted view.
            unsafe {
                let _: () = msg_send![self.view, close];
                let _: () = msg_send![self.view, release];
            }
        }
    }
}
