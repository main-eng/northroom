//! Native views under the GPUI view, shown through holes GPUI leaves in its frame (ADR-0001).
//! The core's video preview presents to a `CAMetalLayer` from its own thread, and a live HTML
//! page is a web view; GPUI can draw neither. The window background is painted around this
//! frame's holes (`ground`), so GPUI's frame is clear there and Core Animation shows the view
//! below. Anything a screen draws over a hole, such as player controls, sits over the view.
//!
//! Holes are cut during prepaint (`cut`), before anything paints. A hole may also supply what its
//! view shows, for window captures (`fill_holes`), since a capture of GPUI's frame doesn't
//! contain other views' pixels.

use gpui::{Bounds, Hsla, IntoElement, Pixels, Styled, Window, canvas};
use std::cell::RefCell;
use std::rc::Rc;

/// What a hosted view shows, as row-major RGB pixels.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// What a hosted view currently shows, for window captures; `None` if unknown.
pub type Shows = Rc<dyn Fn() -> Option<Picture>>;

struct Hole {
    bounds: Bounds<Pixels>,
    shows: Option<Shows>,
    /// The view under it is interactive and GPUI forwards its input (`hosted_input`).
    used: bool,
}

thread_local! {
    static HOLES: RefCell<Vec<Hole>> = const { RefCell::new(Vec::new()) };
}

/// Leaves `bounds` (window points) clear this frame over a native view showing `shows`. Call
/// during prepaint.
pub fn cut(bounds: Bounds<Pixels>, shows: Option<Shows>) {
    HOLES.with(|holes| {
        holes.borrow_mut().push(Hole {
            bounds,
            shows,
            used: false,
        })
    });
}

/// `cut` over an interactive view (a live page, a PDF) whose input GPUI forwards.
pub fn cut_used(bounds: Bounds<Pixels>, shows: Option<Shows>) {
    HOLES.with(|holes| {
        holes.borrow_mut().push(Hole {
            bounds,
            shows,
            used: true,
        })
    });
}

/// Whether `point` (window points) is in a hole cut this frame over an interactive view.
pub fn over_used(point: gpui::Point<Pixels>) -> bool {
    HOLES.with(|holes| {
        holes
            .borrow()
            .iter()
            .any(|hole| hole.used && hole.bounds.contains(&point))
    })
}

/// `area` minus every hole cut this frame, as rectangles.
fn around(area: Bounds<Pixels>) -> Vec<Bounds<Pixels>> {
    HOLES.with(|holes| {
        holes.borrow().iter().fold(vec![area], |parts, hole| {
            parts
                .into_iter()
                .flat_map(|part| without(part, hole.bounds))
                .collect()
        })
    })
}

/// `part` minus `hole`, as up to four bands.
fn without(part: Bounds<Pixels>, hole: Bounds<Pixels>) -> Vec<Bounds<Pixels>> {
    let cut = part.intersect(&hole);
    if cut.size.width <= gpui::px(0.0) || cut.size.height <= gpui::px(0.0) {
        return vec![part];
    }
    let (left, top) = (part.origin.x, part.origin.y);
    let (right, bottom) = (left + part.size.width, top + part.size.height);
    let (cut_left, cut_top) = (cut.origin.x, cut.origin.y);
    let (cut_right, cut_bottom) = (cut_left + cut.size.width, cut_top + cut.size.height);
    let band = |x0: Pixels, y0: Pixels, x1: Pixels, y1: Pixels| {
        Bounds::from_corners(gpui::point(x0, y0), gpui::point(x1, y1))
    };
    [
        band(left, top, right, cut_top),
        band(left, cut_bottom, right, bottom),
        band(left, cut_top, cut_left, cut_bottom),
        band(cut_right, cut_top, right, cut_bottom),
    ]
    .into_iter()
    .filter(|band| band.size.width > gpui::px(0.0) && band.size.height > gpui::px(0.0))
    .collect()
}

/// Fills `area` with `color` except where a hole was cut this frame.
pub fn paint_around(window: &mut Window, area: Bounds<Pixels>, color: Hsla) {
    for part in around(area) {
        crate::draw::fill(window, part, color);
    }
}

/// The window background in `token`, drawn first. During prepaint it clears last frame's holes
/// and releases retired views whose frames are off screen (`mac::let_go_retired`). It keeps the
/// window's backing the same colour (`mac::colour_backing`) and paints everywhere outside the
/// holes.
pub fn ground(token: crate::theme::Token) -> impl IntoElement {
    canvas(
        move |_, _window, _| {
            HOLES.with(|holes| holes.borrow_mut().clear());
            #[cfg(target_os = "macos")]
            {
                mac::let_go_retired(_window);
                mac::colour_backing(_window, crate::theme::stated(token));
            }
        },
        move |bounds, _, window, _| paint_around(window, bounds, crate::theme::color(token)),
    )
    .absolute()
    .size_full()
}

/// Whether every hole with a `Shows` can supply its picture now. A view drawn on its own thread,
/// like the core's preview, may not have a frame at its current size yet.
pub fn holes_shown() -> bool {
    HOLES.with(|holes| {
        holes
            .borrow()
            .iter()
            .filter_map(|hole| hole.shows.as_ref())
            .all(|shows| shows().is_some())
    })
}

/// Composites what the hosted views show into a capture of GPUI's frame (`image`, at `scale`
/// pixels per point), under what GPUI drew over them. GPUI's colour is premultiplied and clear
/// over a hole, so each view shows through in proportion to GPUI's transparency.
pub fn fill_holes(image: &mut image::RgbaImage, scale: f32) {
    HOLES.with(|holes| {
        for hole in holes.borrow().iter() {
            let Some(picture) = hole.shows.as_ref().and_then(|shows| shows()) else {
                continue;
            };
            lay_under(image, scale, hole.bounds, &picture);
        }
    });
}

/// Draws `picture` (sRGB) into `at` (window points) of a GPUI frame capture at `scale` pixels per
/// point, under what GPUI drew there. Compositing happens in the window's colour space
/// (`crate::display`), so a player's shade drawn over the view darkens it.
pub fn lay_under(image: &mut image::RgbaImage, scale: f32, at: Bounds<Pixels>, picture: &Picture) {
    let pixel = |value: Pixels| (f32::from(value) * scale).round() as i64;
    let (x0, y0) = (pixel(at.origin.x), pixel(at.origin.y));
    let (width, height) = (pixel(at.size.width), pixel(at.size.height));
    if width <= 0 || height <= 0 || picture.width == 0 || picture.height == 0 {
        return;
    }
    let mut shown = picture.rgb.clone();
    crate::display::convert(&mut shown, picture.width, picture.height, 3, [0, 1, 2]);
    let (image_width, image_height) = (i64::from(image.width()), i64::from(image.height()));
    for y in y0.max(0)..(y0 + height).min(image_height) {
        // Nearest-neighbour sample at each hole pixel's centre.
        let from_y = (((y - y0) as f64 + 0.5) * f64::from(picture.height) / height as f64) as u32;
        let from_y = from_y.min(picture.height - 1);
        for x in x0.max(0)..(x0 + width).min(image_width) {
            let over = image.get_pixel_mut(x as u32, y as u32);
            let clear = 1.0 - f32::from(over.0[3]) / 255.0;
            if clear <= 0.0 {
                continue;
            }
            let from_x = (((x - x0) as f64 + 0.5) * f64::from(picture.width) / width as f64) as u32;
            let from_x = from_x.min(picture.width - 1);
            let at = ((from_y * picture.width + from_x) * 3) as usize;
            mix_under(over, &shown[at..at + 3], clear);
        }
    }
}

/// Composites RGB `under` beneath GPUI's premultiplied `over`, weighted by GPUI's transparency
/// (`clear`), giving an opaque pixel.
#[inline]
fn mix_under(over: &mut image::Rgba<u8>, under: &[u8], clear: f32) {
    for (stored, &laid) in over.0[..3].iter_mut().zip(under) {
        let mixed = f32::from(*stored) + f32::from(laid) * clear;
        *stored = mixed.round().min(255.0) as u8;
    }
    over.0[3] = 255;
}

#[cfg(target_os = "macos")]
/// Makes GPUI's frame clear where nothing is drawn, so holes show what is under them. GPUI clears
/// an opaque window to black and a transparent one to nothing. Beneath every view the window gets
/// a backing in the background colour (`mac::back`). GPUI's frame reaches the screen a frame or
/// two after AppKit moves a hosted view, so while a view moves, part of the previous hole is
/// uncovered and shows the backing instead of the desktop. Runs once per window.
fn clear_window(window: &mut Window) {
    if mac::back(window) {
        window.set_background_appearance(gpui::WindowBackgroundAppearance::Transparent);
    }
}

#[cfg(not(target_os = "macos"))]
pub use elsewhere::HostedView;
#[cfg(target_os = "macos")]
pub use mac::HostedView;

#[cfg(not(target_os = "macos"))]
mod elsewhere {
    //! No native views are hosted on this platform; the core's preview and the page host are
    //! macOS only. Holes are still cut, over the window background.

    use gpui::{Bounds, Pixels, Window};
    use std::ffi::c_void;

    pub struct HostedView;

    impl HostedView {
        pub fn metal(_: &mut Window, _: [f64; 3]) -> Self {
            Self
        }

        pub fn plain(_: &mut Window, _: [f64; 3]) -> Self {
            Self
        }

        pub fn hold(&self, _: *mut c_void, _: Option<*mut c_void>) {}

        pub fn let_go(_: *mut c_void) {}

        /// Null: there is no layer to present to.
        pub fn layer(&self) -> *mut c_void {
            std::ptr::null_mut()
        }

        pub fn place(&mut self, _: Bounds<Pixels>, _: &Window) {}
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::clear_window;
    use gpui::{Bounds, Pixels, Window};
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::c_void;

    type Id = *mut AnyObject;

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    struct Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    // SAFETY: matches Core Graphics' `CGRect`, a `CGPoint` then a `CGSize`, each two doubles.
    unsafe impl objc2::encode::Encode for Rect {
        const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
            "CGRect",
            &[
                objc2::encode::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]),
                objc2::encode::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]),
            ],
        );
    }

    type CFTypeRef = *const c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGColorCreateSRGB(red: f64, green: f64, blue: f64, alpha: f64) -> CFTypeRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: CFTypeRef);
    }

    #[link(name = "QuartzCore", kind = "framework")]
    unsafe extern "C" {}

    /// GPUI's view in `window`, whose superview is the content view.
    fn gpui_view(window: &Window) -> Id {
        match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::AppKit(handle)) => handle.ns_view.as_ptr().cast(),
            _ => panic!("a GPUI window on a Mac has an AppKit view"),
        }
    }

    /// A window's backing: a view beneath all others, filling the content view. It is retained
    /// here as well as by its window, so it stays alive while listed.
    struct Backing {
        view: Id,
        /// The last colour set, in sRGB.
        colour: Option<gpui::Rgba>,
    }

    thread_local! {
        static BACKINGS: std::cell::RefCell<Vec<Backing>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// The index in `backings` of `ns_window`'s backing, after dropping backings whose windows
    /// have closed.
    fn backing_of(backings: &mut Vec<Backing>, ns_window: Id) -> Option<usize> {
        // SAFETY: each backing's view is retained here, so it is alive; AppKit calls on the main
        // thread.
        unsafe {
            backings.retain(|backing| {
                let window: Id = msg_send![backing.view, window];
                if window.is_null() {
                    let _: () = msg_send![backing.view, removeFromSuperview];
                    let _: () = msg_send![backing.view, release];
                }
                !window.is_null()
            });
            backings.iter().position(|backing| {
                let window: Id = msg_send![backing.view, window];
                window == ns_window
            })
        }
    }

    /// The `NSWindow` for `window`.
    fn ns_window_of(window: &Window) -> Id {
        // SAFETY: a live view, on the main thread.
        unsafe { msg_send![gpui_view(window), window] }
    }

    /// Adds a backing beneath every view of `window` if it has none. Returns whether one was
    /// added.
    pub(super) fn back(window: &Window) -> bool {
        let gpui = gpui_view(window);
        let ns_window = ns_window_of(window);
        let backed = BACKINGS.with(|backings| backing_of(&mut backings.borrow_mut(), ns_window));
        if backed.is_some() {
            return false;
        }
        // SAFETY: AppKit calls on the main thread; the view is retained here (`Backing`) and by
        // the content view.
        let view = unsafe {
            let content: Id = msg_send![gpui, superview];
            if content.is_null() {
                return false;
            }
            let bounds: Rect = msg_send![content, bounds];
            let view: Id = msg_send![class!(NSView), alloc];
            let view: Id = msg_send![view, initWithFrame: bounds];
            let _: () = msg_send![view, setWantsLayer: Bool::YES];
            // NSViewWidthSizable | NSViewHeightSizable: resizes with the window.
            let _: () = msg_send![view, setAutoresizingMask: 18_usize];
            // NSWindowBelow relative to nil: beneath every other view.
            let _: () = msg_send![content, addSubview: view, positioned: -1_isize, relativeTo: std::ptr::null_mut::<AnyObject>()];
            view
        };
        BACKINGS.with(|backings| backings.borrow_mut().push(Backing { view, colour: None }));
        true
    }

    /// Sets `window`'s backing to `colour` (sRGB) if it has a backing of a different colour.
    pub(super) fn colour_backing(window: &Window, colour: gpui::Rgba) {
        let ns_window = ns_window_of(window);
        BACKINGS.with(|backings| {
            let mut backings = backings.borrow_mut();
            let Some(index) = backing_of(&mut backings, ns_window) else {
                return;
            };
            let backing = &mut backings[index];
            if backing.colour == Some(colour) {
                return;
            }
            backing.colour = Some(colour);
            let view = backing.view;
            at_once(|| {
                // SAFETY: a view retained here, on the main thread; the colour is released after
                // it is set.
                unsafe {
                    let layer: Id = msg_send![view, layer];
                    let ground = CGColorCreateSRGB(
                        f64::from(colour.r),
                        f64::from(colour.g),
                        f64::from(colour.b),
                        1.0,
                    );
                    let _: () = msg_send![layer, setBackgroundColor: ground];
                    CFRelease(ground);
                }
            });
        });
    }

    /// Runs `change` with Core Animation's implicit animations disabled.
    fn at_once(change: impl FnOnce()) {
        // SAFETY: Core Animation calls on the main thread.
        unsafe {
            let _: () = msg_send![class!(CATransaction), begin];
            let _: () = msg_send![class!(CATransaction), setDisableActions: Bool::YES];
            change();
            let _: () = msg_send![class!(CATransaction), commit];
        }
    }

    /// A native view under the GPUI view, placed where a hole is cut.
    pub struct HostedView {
        view: Id,
        layer: Id,
        /// The last frame set, in window points from the top left.
        shown: Option<Bounds<Pixels>>,
        /// Unique number, used to name the accessibility element that stands in for the view.
        number: u64,
    }

    static NUMBERED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn numbered() -> u64 {
        NUMBERED.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    impl HostedView {
        /// A view hosting a `CAMetalLayer` for the core to present to, showing `rgb` (sRGB)
        /// until the first frame.
        pub fn metal(window: &mut Window, rgb: [f64; 3]) -> Self {
            clear_window(window);
            let above = gpui_view(window);
            // SAFETY: AppKit and Core Animation calls on the main thread; the view and layer are
            // retained here and released in `let_go_retired`.
            unsafe {
                let layer: Id = msg_send![class!(CAMetalLayer), new];
                let _: () = msg_send![layer, setOpaque: Bool::YES];
                let ground = CGColorCreateSRGB(rgb[0], rgb[1], rgb[2], 1.0);
                let _: () = msg_send![layer, setBackgroundColor: ground];
                CFRelease(ground);
                let _: () = msg_send![layer, setContentsScale: f64::from(window.scale_factor())];
                let view: Id = msg_send![class!(NSView), alloc];
                let view: Id = msg_send![view, initWithFrame: Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 }];
                // Layer-hosting: set the layer before `wantsLayer`.
                let _: () = msg_send![view, setLayer: layer];
                let _: () = msg_send![view, setWantsLayer: Bool::YES];
                // The view now owns the layer.
                let _: () = msg_send![layer, release];
                Self::put_below(view, above);
                Self {
                    view,
                    layer,
                    shown: None,
                    number: numbered(),
                }
            }
        }

        /// A plain view filled with `rgb` (sRGB) that holds other native views (`hold`), such as
        /// a live page's web view.
        pub fn plain(window: &mut Window, rgb: [f64; 3]) -> Self {
            clear_window(window);
            let above = gpui_view(window);
            // SAFETY: AppKit and Core Animation calls on the main thread; the view is retained
            // here and released in `let_go_retired`.
            unsafe {
                let view: Id = msg_send![class!(NSView), alloc];
                let view: Id = msg_send![view, initWithFrame: Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 }];
                let _: () = msg_send![view, setWantsLayer: Bool::YES];
                let layer: Id = msg_send![view, layer];
                let ground = CGColorCreateSRGB(rgb[0], rgb[1], rgb[2], 1.0);
                let _: () = msg_send![layer, setBackgroundColor: ground];
                CFRelease(ground);
                Self::put_below(view, above);
                Self {
                    view,
                    layer,
                    shown: None,
                    number: numbered(),
                }
            }
        }

        /// Adds `child` (an `NSView`) to this view, resizing with it, on top or just below
        /// `below`.
        pub fn hold(&self, child: *mut c_void, below: Option<*mut c_void>) {
            let child: Id = child.cast();
            // SAFETY: live views, on the main thread.
            unsafe {
                let bounds: Rect = msg_send![self.view, bounds];
                let _: () = msg_send![child, setFrame: bounds];
                // NSViewWidthSizable | NSViewHeightSizable.
                let _: () = msg_send![child, setAutoresizingMask: 18_usize];
                match below {
                    Some(sibling) => {
                        let sibling: Id = sibling.cast();
                        let _: () = msg_send![self.view, addSubview: child, positioned: -1_isize, relativeTo: sibling];
                    }
                    None => {
                        let _: () = msg_send![self.view, addSubview: child];
                    }
                }
            }
        }

        /// Removes `child` from its superview.
        pub fn let_go(child: *mut c_void) {
            // SAFETY: a live view, on the main thread.
            let _: () = unsafe { msg_send![child.cast::<AnyObject>(), removeFromSuperview] };
        }

        /// # Safety
        /// Both views must be live, and this must run on the main thread.
        unsafe fn put_below(view: Id, above: Id) {
            // SAFETY: upheld by the caller.
            unsafe {
                let content: Id = msg_send![above, superview];
                // NSWindowBelow: beneath GPUI's view.
                let _: () =
                    msg_send![content, addSubview: view, positioned: -1_isize, relativeTo: above];
            }
        }

        /// The identifier of the accessibility element VoiceOver reads in place of this view.
        /// The screen showing the view gives that element this id with `accessibility_id`.
        pub fn read_in_place(&self) -> String {
            let element = crate::platform::hosted_element(self.number);
            crate::platform::show_hosted(&element, self.view.cast());
            element
        }

        /// The view's layer (a `CAMetalLayer` for `metal`).
        pub fn layer(&self) -> *mut c_void {
            self.layer.cast()
        }

        /// Moves the view to `frame` (window points from the top left) without animating.
        pub fn place(&mut self, frame: Bounds<Pixels>, window: &Window) {
            if self.shown == Some(frame) {
                return;
            }
            self.shown = Some(frame);
            let content_height = f64::from(window.viewport_size().height);
            let flipped = Rect {
                x: f64::from(frame.origin.x),
                y: content_height - f64::from(frame.origin.y) - f64::from(frame.size.height),
                width: f64::from(frame.size.width),
                height: f64::from(frame.size.height),
            };
            let (view, layer, scale) = (self.view, self.layer, f64::from(window.scale_factor()));
            at_once(|| {
                // SAFETY: the view and layer are live.
                unsafe {
                    let _: () = msg_send![view, setFrame: flipped];
                    let _: () = msg_send![layer, setContentsScale: scale];
                }
            });
        }
    }

    /// How many frames a dropped view stays under the window. GPUI presents separately from
    /// AppKit's views and lands a frame or two later, so the last frame with the view's hole is
    /// still on screen. Removing the view at once would let the hole show through.
    const FRAMES_RETIRED: u8 = 3;

    /// A dropped view, kept under the GPUI view until the frames with its hole are off screen.
    struct Retired {
        view: Id,
        frames: u8,
    }

    thread_local! {
        static RETIRED: std::cell::RefCell<Vec<Retired>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// During prepaint, removes views retired `FRAMES_RETIRED` frames ago and requests frames
    /// for the rest.
    pub(super) fn let_go_retired(window: &mut Window) {
        RETIRED.with(|retired| {
            let mut retired = retired.borrow_mut();
            retired.retain_mut(|view| {
                view.frames += 1;
                if view.frames < FRAMES_RETIRED {
                    return true;
                }
                // SAFETY: releases the view retained in `metal` or `plain`, on the main thread.
                unsafe {
                    let _: () = msg_send![view.view, removeFromSuperview];
                    let _: () = msg_send![view.view, release];
                }
                false
            });
            if !retired.is_empty() {
                window.request_animation_frame();
            }
        });
    }

    impl Drop for HostedView {
        /// The view stays under GPUI's view until the frames that showed it are off screen
        /// (`let_go_retired`), because a screen is dropped before the frame without it is drawn.
        fn drop(&mut self) {
            crate::platform::forget_hosted(&crate::platform::hosted_element(self.number));
            RETIRED.with(|retired| {
                retired.borrow_mut().push(Retired {
                    view: self.view,
                    frames: 0,
                })
            });
        }
    }
}
