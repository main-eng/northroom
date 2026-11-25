//! A PDF, drawn by PDFKit on the panel colour with its pages in their own colours. Scrolling and
//! text selection work as in Preview, with no bounce at the ends. A trackpad pinch zooms about
//! the pointer within the core's limits (`pdf_zoom`). PDFKit's magnification scales the drawn
//! pages during the pinch and redraws them sharp when it ends; setting the scale on every step
//! would redraw every page each time and show them half drawn. The PDF fills its pane, beside
//! its column or filling the tab. Pages show at true size, narrower only when the pane is
//! (the core's `nr_pdf_scale`).
//!
//! PDFKit's view sits under the window's GPUI view (`kit::hosted`), which leaves a hole for it.
//! Presses, drags, scrolls and pinches GPUI receives over the hole are forwarded
//! (`kit::hosted_input`). Off macOS the PDF opens in the system web view's own viewer
//! (WebView2, WebKitGTK), shown like a live page (`html::drawn`): this view draws its snapshots,
//! forwards input and shows, in the core's words, why it failed to open.

#[cfg(not(target_os = "macos"))]
use super::html::{drawn, say_in_middle};
#[cfg(target_os = "macos")]
use super::html::{hosted, pass_pointer_on};
#[cfg(target_os = "macos")]
use super::on_bubble_over;
use super::{Ask, Screen};
use crate::script::probe::Probe;
#[cfg(target_os = "macos")]
use gpui::PinchEvent;
use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Hitbox, HitboxBehavior, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, Styled, Window, canvas,
};
use kit::controls::placed;
#[cfg(target_os = "macos")]
use kit::hosted_input::Forward;
use kit::nr::*;
use std::rc::Rc;

pub struct Pdf {
    frame: Bounds<Pixels>,
    focus: FocusHandle,
    /// The file shown.
    path: String,
    #[cfg(target_os = "macos")]
    pages: mac::Pages,
    #[cfg(target_os = "macos")]
    hosted: kit::hosted::HostedView,
    /// The frame the pages were last scaled for.
    #[cfg(target_os = "macos")]
    scaled_for: Option<Bounds<Pixels>>,
    #[cfg(target_os = "macos")]
    forward: Forward,
    /// The pages in the system web view, once a file is open.
    #[cfg(not(target_os = "macos"))]
    drawn: Option<drawn::Drawn>,
    /// Why the file failed to open, shown in place of its pages.
    #[cfg(not(target_os = "macos"))]
    failed: Rc<std::cell::RefCell<Option<String>>>,
}

impl Pdf {
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        #[cfg(target_os = "macos")]
        let pages = mac::Pages::new();
        let focus = cx.focus_handle();
        #[cfg(target_os = "macos")]
        kit::hosted_input::follow_focus(pages.view(), &focus, window, cx);
        let mut pdf = Self {
            frame: Bounds::default(),
            focus,
            path: String::new(),
            #[cfg(target_os = "macos")]
            hosted: hosted(pages.view(), window),
            #[cfg(target_os = "macos")]
            pages,
            #[cfg(target_os = "macos")]
            scaled_for: None,
            #[cfg(target_os = "macos")]
            forward: Forward::default(),
            #[cfg(not(target_os = "macos"))]
            drawn: None,
            #[cfg(not(target_os = "macos"))]
            failed: Rc::default(),
        };
        pdf.reload(window, cx);
        pdf
    }

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            self.hosted.place(self.frame, window);
            // The background follows the theme, which may have changed since the last frame.
            self.pages
                .set_ground(kit::theme::stated(kit::theme::Token::Panel));
            if self.scaled_for != Some(self.frame) && self.frame.size.width > gpui::px(0.0) {
                self.scaled_for = Some(self.frame);
                self.pages.scale();
            }
        }
        #[cfg(not(target_os = "macos"))]
        match (self.failed.borrow().as_ref(), &self.drawn) {
            (Some(failure), _) => say_in_middle(failure, self.frame, window, cx),
            (None, Some(drawn)) => drawn.paint(self.frame, window),
            (None, None) => {}
        }
        self.listen(hitbox, window, cx);
    }

    /// Forwards presses, drags, releases and scrolls over the pages to the web view. A press
    /// gives the view the keys.
    #[cfg(not(target_os = "macos"))]
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, _: &mut Context<Self>) {
        if let Some(drawn) = &self.drawn {
            let focus = self.focus.clone();
            drawn::listen(&drawn.shown, hitbox, window, move |window, cx| {
                window.focus(&focus, cx)
            });
        }
    }

    /// Forwards presses, drags, releases, scrolls and pinches over the pages.
    #[cfg(target_os = "macos")]
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        pass_pointer_on(window, hitbox, move |hitbox, window, cx| {
            let over = hitbox.is_hovered(window);
            entity.update(cx, |pdf, cx| {
                let view = pdf.pages.view();
                if pdf.forward.pass_on(view, over, window) && over {
                    window.focus(&pdf.focus, cx);
                }
            });
        });
        let pdf = cx.entity();
        on_bubble_over(window, hitbox, move |_: &PinchEvent, window, cx| {
            pdf.update(cx, |pdf, _| {
                let view = pdf.pages.view();
                pdf.forward.pass_on(view, true, window);
            });
        });
    }
}

#[cfg(not(target_os = "macos"))]
impl Pdf {
    /// Opens the PDF at `path` in a new web view, replacing the current one.
    fn open(&mut self, path: &str, cx: &mut Context<Self>) {
        let page = Rc::new(webhost::HtmlPage::new(std::path::Path::new(path)));
        *self.failed.borrow_mut() = None;
        let (said, woken, async_cx) = (self.failed.clone(), cx.weak_entity(), cx.to_async());
        page.on_loaded(move |failure| {
            if let Some(failure) = &failure {
                eprintln!("pdf\tthe file did not open: {failure}");
            }
            *said.borrow_mut() = failure.as_ref().map(ToString::to_string);
            let _ = woken.update(&mut async_cx.clone(), |_, cx| cx.notify());
        });
        let starting = page.clone();
        webhost::run::spawn(async move { starting.start().await });
        self.drawn = Some(drawn::Drawn::new(page, cx));
    }
}

impl EventEmitter<Ask> for Pdf {}

impl Screen for Pdf {
    /// Reads the open file from the core again; a different file replaces this one.
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let opened = kit::core::text(&nr_opened_path());
        if opened.is_empty() || opened == self.path {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            self.pages.open(&opened);
            self.scaled_for = None;
        }
        #[cfg(not(target_os = "macos"))]
        self.open(&opened, cx);
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
            probe.keys = "PdfFileView".to_owned();
        }
    }
}

impl Render for Pdf {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pdf = cx.entity();
        #[cfg(target_os = "macos")]
        let shows: kit::hosted::Shows = {
            let view = self.pages.view();
            Rc::new(move || mac::picture(view))
        };
        let placed = placed("pdf", cx.entity_id(), self.frame).track_focus(&self.focus);
        // VoiceOver reads PDFKit's view where this element stands.
        #[cfg(target_os = "macos")]
        let placed = {
            use gpui::StatefulInteractiveElement;
            placed
                .role(gpui::Role::Group)
                .accessibility_id(self.hosted.read_in_place())
        };
        // Off macOS, keys the view receives go to the web view showing the pages.
        #[cfg(not(target_os = "macos"))]
        let placed = match &self.drawn {
            Some(drawn) => {
                let (down, up) = (drawn.shown.clone(), drawn.shown.clone());
                placed
                    .on_key_down(move |event, _, _| down.key_down(event))
                    .on_key_up(move |event, _, _| up.key_up(event))
            }
            None => placed,
        };
        placed.child(
            canvas(
                move |bounds, window, _| {
                    // The pages show through the window here, and in screenshots of it.
                    #[cfg(target_os = "macos")]
                    kit::hosted::cut_used(bounds, Some(shows.clone()));
                    window.insert_hitbox(bounds, HitboxBehavior::Normal)
                },
                move |_, hitbox, window, cx| {
                    pdf.update(cx, |pdf, cx| pdf.paint(&hitbox, window, cx))
                },
            )
            .size_full(),
        )
    }
}

/// PDFKit's view, through plain `msg_send!`s.
#[cfg(target_os = "macos")]
mod mac {
    use gpui::Rgba;
    use kit::nr::{nr_pdf_scale, pdf_zoom};
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    use std::cell::Cell;
    use std::ffi::{CString, c_void};

    type Id = *mut AnyObject;

    #[link(name = "PDFKit", kind = "framework")]
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

    /// `PDFDisplaySinglePageContinuous`: one page wide, scrolling through all pages.
    const SINGLE_PAGE_CONTINUOUS: isize = 1;
    /// `NSScrollElasticityNone`: no bounce past the scroll view's ends.
    const NO_ELASTICITY: isize = 1;

    /// A `PDFView` showing pages stacked vertically with breaks between them.
    pub struct Pages {
        view: Id,
        ground: Cell<Option<Rgba>>,
    }

    impl Pages {
        pub fn new() -> Self {
            // SAFETY: plain AppKit and PDFKit calls on the main thread. The view is released in
            // `drop`.
            unsafe {
                let view: Id = msg_send![class!(PDFView), alloc];
                let view: Id = msg_send![view, initWithFrame: Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 }];
                let _: () = msg_send![view, setDisplayMode: SINGLE_PAGE_CONTINUOUS];
                let _: () = msg_send![view, setDisplaysPageBreaks: Bool::YES];
                Self {
                    view,
                    ground: Cell::new(None),
                }
            }
        }

        /// The view, for the window that hosts it.
        pub fn view(&self) -> *mut c_void {
            self.view.cast()
        }

        /// Shows the PDF at `path`.
        pub fn open(&self, path: &str) {
            let path = CString::new(path).expect("a path has no NUL");
            // SAFETY: plain Foundation and PDFKit calls on the main thread. The view retains the
            // document.
            unsafe {
                let path: Id = msg_send![class!(NSString), stringWithUTF8String: path.as_ptr()];
                let url: Id = msg_send![class!(NSURL), fileURLWithPath: path];
                let document: Id = msg_send![class!(PDFDocument), alloc];
                let document: Id = msg_send![document, initWithURL: url];
                let _: () = msg_send![self.view, setDocument: document];
                if !document.is_null() {
                    let _: () = msg_send![document, release];
                }
                // PDFKit creates its scroll view with the document; turn off its bounce.
                let pages: Id = msg_send![self.view, documentView];
                if !pages.is_null() {
                    let scroller: Id = msg_send![pages, enclosingScrollView];
                    if !scroller.is_null() {
                        let _: () = msg_send![scroller, setVerticalScrollElasticity: NO_ELASTICITY];
                        let _: () =
                            msg_send![scroller, setHorizontalScrollElasticity: NO_ELASTICITY];
                    }
                }
            }
        }

        /// Sets pages to true size, narrower only when the pane is (`nr_pdf_scale`), and sets the
        /// pinch limits PDFKit's magnification respects (`pdf_zoom`).
        pub fn scale(&self) {
            // SAFETY: plain PDFKit calls on the main thread.
            unsafe {
                let document: Id = msg_send![self.view, document];
                if document.is_null() {
                    return;
                }
                let fit: f64 = msg_send![self.view, scaleFactorForSizeToFit];
                let scale = nr_pdf_scale(fit);
                let zoom = pdf_zoom(scale);
                // Set the limits first, since PDFKit clamps the scale to them.
                let _: () = msg_send![self.view, setMinScaleFactor: zoom.least];
                let _: () = msg_send![self.view, setMaxScaleFactor: zoom.most];
                let _: () = msg_send![self.view, setScaleFactor: scale];
            }
        }

        /// Sets the background around the pages to the theme's sRGB colour.
        pub fn set_ground(&self, color: Rgba) {
            if self.ground.replace(Some(color)) == Some(color) {
                return;
            }
            // SAFETY: plain AppKit calls on the main thread; the colour is autoreleased.
            unsafe {
                let ns: Id = msg_send![
                    class!(NSColor),
                    colorWithSRGBRed: f64::from(color.r),
                    green: f64::from(color.g),
                    blue: f64::from(color.b),
                    alpha: f64::from(color.a)
                ];
                let _: () = msg_send![self.view, setBackgroundColor: ns];
            }
        }
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGImageGetWidth(image: *const c_void) -> usize;
        fn CGImageGetHeight(image: *const c_void) -> usize;
        fn CGColorSpaceCreateWithName(name: *const c_void) -> *const c_void;
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits: usize,
            row: usize,
            space: *const c_void,
            info: u32,
        ) -> *const c_void;
        fn CGContextDrawImage(context: *const c_void, rect: Rect, image: *const c_void);
        fn CFRelease(value: *const c_void);
        static kCGColorSpaceSRGB: *const c_void;
    }

    /// The pages' current contents, drawn the way AppKit caches a view, for a window
    /// screenshot.
    pub fn picture(view: *mut c_void) -> Option<kit::hosted::Picture> {
        let view: Id = view.cast();
        // SAFETY: plain AppKit and Core Graphics calls on the main thread on a live view. The
        // context draws into `rgba`, which outlives it.
        unsafe {
            let bounds: Rect = msg_send![view, bounds];
            let rep: Id = msg_send![view, bitmapImageRepForCachingDisplayInRect: bounds];
            if rep.is_null() {
                return None;
            }
            let _: () = msg_send![view, cacheDisplayInRect: bounds, toBitmapImageRep: rep];
            let image: *const c_void = msg_send![rep, CGImage];
            if image.is_null() {
                return None;
            }
            let (width, height) = (CGImageGetWidth(image), CGImageGetHeight(image));
            let mut rgba = vec![0_u8; width * height * 4];
            if width == 0 || height == 0 {
                return None;
            }
            let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            if space.is_null() {
                return None;
            }
            // Premultiplied RGBA, alpha last (`kCGImageAlphaPremultipliedLast`).
            let context = CGBitmapContextCreate(
                rgba.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                space,
                1,
            );
            CFRelease(space);
            if context.is_null() {
                return None;
            }
            let all = Rect {
                x: 0.0,
                y: 0.0,
                width: width as f64,
                height: height as f64,
            };
            CGContextDrawImage(context, all, image);
            CFRelease(context);
            let rgb = rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[red, green, blue, _]| [red, green, blue])
                .collect();
            Some(kit::hosted::Picture {
                width: width as u32,
                height: height as u32,
                rgb,
            })
        }
    }

    impl Drop for Pages {
        fn drop(&mut self) {
            // SAFETY: the view from `new`. The window's reference belongs to the hosted view.
            unsafe {
                let _: () = msg_send![self.view, release];
            }
        }
    }
}
