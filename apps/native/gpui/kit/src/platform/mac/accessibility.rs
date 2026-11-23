//! Patches AppKit accessibility so VoiceOver reads the window the way it reads native views,
//! filling gaps AccessKit leaves.
//!
//! Focus. GPUI's view is first responder, but AccessKit's tree hangs from a view beneath it, so
//! VoiceOver would only ever see the window as focused. GPUI's view reports the element focused
//! in AccessKit's tree instead, and falls back to its own answer when nothing there has focus.
//!
//! Hosted views. Native views under GPUI's view (a web view, PDF pages, `kit::hosted`) are
//! otherwise invisible, because AccessKit's root reports only its tree. Each one takes the
//! place of a placeholder element (`hosted_element`): same index among its parent's children,
//! same parent. Hit tests and focus inside it go to the view.
//!
//! Roles AccessKit can't express: scroll areas (`AXScrollArea`) and table columns (`AXColumn`).
//! An element names the role in its role description (`crate::lists::voiceover`) and is read as
//! that role with the system description. Also, no subrole where AccessKit has none, and
//! placeholders read even when the field has text.

use super::appkit::{Id, ns_string};
use objc2::runtime::{AnyClass, Bool, Imp, Sel};
use objc2::{class, msg_send, sel};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

/// Identifier prefix for placeholder elements of hosted views; the view's number follows.
const HOSTED: &str = "northroom.hosted.";

thread_local! {
    /// Hosted views, keyed by their placeholder element's identifier.
    static VIEWS: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
    /// Placeholder text per field identifier. AppKit fields report it even when filled;
    /// AccessKit reports it only while empty.
    static PLACEHOLDERS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Sets the placeholder text for field `field`.
pub fn show_placeholder(field: &str, words: &str) {
    PLACEHOLDERS.with(|placeholders| {
        placeholders
            .borrow_mut()
            .insert(field.to_owned(), words.to_owned())
    });
}

/// The placeholder element identifier for hosted view `number`.
pub fn hosted_element(number: u64) -> String {
    format!("{HOSTED}{number}")
}

/// Puts `view` (an `NSView` under GPUI's view) in the tree in place of element `element`.
pub fn show_hosted(element: &str, view: *mut c_void) {
    VIEWS.with(|views| views.borrow_mut().insert(element.to_owned(), view as usize));
}

/// Removes the hosted view registered for `element`.
pub fn forget_hosted(element: &str) {
    VIEWS.with(|views| views.borrow_mut().remove(element));
}

/// The text of an `NSString`, or none.
fn words_of(string: Id) -> Option<String> {
    if string.is_null() {
        return None;
    }
    // SAFETY: the UTF-8 bytes, when available, live as long as the `NSString`.
    unsafe {
        let bytes: *const c_char = msg_send![string, UTF8String];
        (!bytes.is_null()).then(|| CStr::from_ptr(bytes).to_string_lossy().into_owned())
    }
}

/// The hosted view that `element` is a placeholder for, if any.
fn hosted_for(element: Id) -> Option<Id> {
    if element.is_null() {
        return None;
    }
    // SAFETY: one of the app's accessibility elements, queried on the main thread.
    let identifier = unsafe {
        let answers: Bool = msg_send![element, respondsToSelector: sel!(accessibilityIdentifier)];
        if !answers.as_bool() {
            return None;
        }
        words_of(msg_send![element, accessibilityIdentifier])?
    };
    if !identifier.starts_with(HOSTED) {
        return None;
    }
    VIEWS.with(|views| views.borrow().get(&identifier).map(|&view| view as Id))
}

/// The unignored accessibility elements of hosted view `view` (the web view, the PDF view),
/// each reparented to `parent`.
///
/// # Safety
/// A live view and element, on the main thread.
unsafe fn hosted_elements(view: Id, parent: Id) -> Vec<Id> {
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        fn NSAccessibilityUnignoredChildrenForOnlyChild(child: Id) -> Id;
    }
    // SAFETY: as the caller says.
    unsafe {
        let shown: Id = NSAccessibilityUnignoredChildrenForOnlyChild(view);
        let count: usize = msg_send![shown, count];
        (0..count)
            .map(|index| {
                let element: Id = msg_send![shown, objectAtIndex: index];
                let _: () = msg_send![element, setAccessibilityParent: parent];
                element
            })
            .collect()
    }
}

/// `children` (an `NSArray` of `parent`'s) with each placeholder replaced by its hosted view's
/// elements.
///
/// # Safety
/// A live array and element, on the main thread.
unsafe fn with_hosted(children: Id, parent: Id) -> Id {
    if children.is_null() {
        return children;
    }
    // SAFETY: as the caller says.
    unsafe {
        let count: usize = msg_send![children, count];
        let elements: Vec<Id> = (0..count)
            .map(|index| -> Id { msg_send![children, objectAtIndex: index] })
            .collect();
        if !elements
            .iter()
            .any(|&element| hosted_for(element).is_some())
        {
            return children;
        }
        let shown: Id = msg_send![class!(NSMutableArray), arrayWithCapacity: count];
        for element in elements {
            match hosted_for(element) {
                Some(view) => {
                    for own in hosted_elements(view, parent) {
                        let _: () = msg_send![shown, addObject: own];
                    }
                }
                None => {
                    let _: () = msg_send![shown, addObject: element];
                }
            }
        }
        shown
    }
}

/// The hit test result at `point` (screen points) in `element`'s hosted view, or `element`.
///
/// # Safety
/// A live element, on the main thread.
unsafe fn hit_hosted(element: Id, point: Point) -> Id {
    // SAFETY: as the caller says.
    unsafe {
        let Some(view) = hosted_for(element) else {
            return element;
        };
        let parent: Id = msg_send![element, accessibilityParent];
        hosted_elements(view, parent)
            .into_iter()
            .find(|&own| holds(own, point))
            .map_or(element, |own| {
                let hit: Id = msg_send![own, accessibilityHitTest: point];
                if hit.is_null() { own } else { hit }
            })
    }
}

/// The focused element in `element`'s hosted view, or `element`.
///
/// # Safety
/// A live element, on the main thread.
unsafe fn focus_hosted(element: Id) -> Id {
    // SAFETY: as the caller says.
    unsafe {
        match hosted_for(element) {
            Some(view) => {
                let focused: Id = msg_send![view, accessibilityFocusedUIElement];
                if focused.is_null() { element } else { focused }
            }
            None => element,
        }
    }
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

// MARK: Swizzled methods

/// The original implementations, one slot per swizzled method.
#[derive(Default)]
struct Own {
    view_focus: OnceLock<Imp>,
    view_hit: OnceLock<Imp>,
    root_children: OnceLock<Imp>,
    root_hit: OnceLock<Imp>,
    root_focus: OnceLock<Imp>,
    node_children: OnceLock<Imp>,
    node_ordered: OnceLock<Imp>,
    node_role: OnceLock<Imp>,
    node_subrole: OnceLock<Imp>,
    node_description: OnceLock<Imp>,
    node_placeholder: OnceLock<Imp>,
}

static OWN: OnceLock<Own> = OnceLock::new();

fn own() -> &'static Own {
    OWN.get_or_init(Own::default)
}

type Answer = unsafe extern "C-unwind" fn(Id, Sel) -> Id;
type HitAnswer = unsafe extern "C-unwind" fn(Id, Sel, Point) -> Id;

/// The original implementation in `slot`, as an object-returning method.
fn kept(slot: &OnceLock<Imp>) -> Answer {
    // SAFETY: every slot holds an implementation of a method of types `@@:`.
    unsafe { std::mem::transmute(*slot.get().expect("the replaced implementation is kept")) }
}

/// Replaces `selector` in `class` with `with`, saving the original in `slot`. Runs once per
/// class.
///
/// # Safety
/// `with` matches the selector's type encoding `types`, and the class implements the selector.
unsafe fn replace(
    class: *const AnyClass,
    selector: Sel,
    with: Imp,
    types: &CStr,
    slot: &OnceLock<Imp>,
) {
    // SAFETY: as the caller says.
    unsafe {
        let method = (*class)
            .instance_method(selector)
            .expect("the class answers the selector");
        if slot.set(method.implementation()).is_ok() {
            objc2::ffi::class_replaceMethod(class.cast_mut(), selector, with, types.as_ptr());
        }
    }
}

/// `-[GPUIView accessibilityFocusedUIElement]`: AccessKit's focused element, else the
/// original answer.
extern "C-unwind" fn view_focus(this: Id, selector: Sel) -> Id {
    // SAFETY: GPUI's view is in the window while AppKit asks it.
    unsafe {
        let under: Id = msg_send![this, superview];
        let focused: Id = msg_send![under, accessibilityFocusedUIElement];
        if !focused.is_null() {
            return focused;
        }
        kept(&own().view_focus)(this, selector)
    }
}

/// `-[GPUIView accessibilityHitTest:]`: AccessKit's hit test at `point`. GPUI's view sits in
/// front of AccessKit's view and every hosted view, so without this a hit test stops at GPUI's
/// view, which AppKit ignores, and finds nothing.
extern "C-unwind" fn view_hit(this: Id, _: Sel, point: Point) -> Id {
    // SAFETY: GPUI's view is in the window while AppKit asks it.
    unsafe {
        let under: Id = msg_send![this, superview];
        msg_send![under, accessibilityHitTest: point]
    }
}

/// AccessKit root's children, with hosted views in place.
extern "C-unwind" fn root_children(this: Id, selector: Sel) -> Id {
    // SAFETY: AccessKit's own answer, on the main thread.
    unsafe {
        let children = kept(&own().root_children)(this, selector);
        install_on_nodes(children);
        with_hosted(children, this)
    }
}

/// AccessKit's hit test at `point` (screen points), with hosted views resolved. Columns overlap
/// their rows as in `NSTableView`, so a point finds the row and cell beneath.
extern "C-unwind" fn root_hit(this: Id, selector: Sel, point: Point) -> Id {
    // SAFETY: AccessKit's own answer, on the main thread.
    unsafe {
        let own: HitAnswer = std::mem::transmute(*own().root_hit.get().expect("kept"));
        let hit = own(this, selector, point);
        let hit = if is_column(hit) {
            let table: Id = msg_send![hit, accessibilityParent];
            deepest_at(table, point).unwrap_or(hit)
        } else {
            hit
        };
        hit_hosted(hit, point)
    }
}

/// Whether `element` is an AccessKit table column.
///
/// # Safety
/// A live element, on the main thread.
unsafe fn is_column(element: Id) -> bool {
    let Some(nodes) = AnyClass::get(c"AccessKitNode") else {
        return false;
    };
    // SAFETY: as the caller says.
    unsafe {
        let ours: Bool = msg_send![element, isKindOfClass: nodes];
        ours.as_bool() && named_role(element).and_then(words_of).as_deref() == Some("AXColumn")
    }
}

/// Whether `element`'s frame contains `point` (screen points).
///
/// # Safety
/// A live element, on the main thread.
unsafe fn holds(element: Id, point: Point) -> bool {
    // SAFETY: as the caller says.
    let frame: Rect = unsafe { msg_send![element, accessibilityFrame] };
    point.x >= frame.x
        && point.x < frame.x + frame.width
        && point.y >= frame.y
        && point.y < frame.y + frame.height
}

/// The deepest descendant of `element` containing `point` (screen points), searching the last
/// child first and skipping columns.
///
/// # Safety
/// A live element of AccessKit's, on the main thread.
unsafe fn deepest_at(element: Id, point: Point) -> Option<Id> {
    // SAFETY: as the caller says.
    unsafe {
        let children: Id = msg_send![element, accessibilityChildren];
        if children.is_null() {
            return None;
        }
        let count: usize = msg_send![children, count];
        (0..count).rev().find_map(|index| {
            let child: Id = msg_send![children, objectAtIndex: index];
            (holds(child, point) && !is_column(child))
                .then(|| deepest_at(child, point).unwrap_or(child))
        })
    }
}

extern "C-unwind" fn root_focus(this: Id, selector: Sel) -> Id {
    // SAFETY: AccessKit's own answer, on the main thread.
    unsafe { focus_hosted(kept(&own().root_focus)(this, selector)) }
}

extern "C-unwind" fn node_children(this: Id, selector: Sel) -> Id {
    // SAFETY: AccessKit's own answer, on the main thread.
    unsafe { with_hosted(kept(&own().node_children)(this, selector), this) }
}

extern "C-unwind" fn node_ordered(this: Id, selector: Sel) -> Id {
    // SAFETY: AccessKit's own answer, on the main thread.
    unsafe { with_hosted(kept(&own().node_ordered)(this, selector), this) }
}

/// The AppKit role named in an element's role description, for roles AccessKit lacks.
///
/// # Safety
/// A live element of AccessKit's, on the main thread.
unsafe fn named_role(this: Id) -> Option<Id> {
    // A hit test can arrive before the root's children are requested, which is what installs
    // the swizzles; until then AccessKit's role stands.
    if own().node_role.get().is_none() || own().node_description.get().is_none() {
        return None;
    }
    // SAFETY: as the caller says.
    unsafe {
        let role = kept(&own().node_role)(this, sel!(accessibilityRole));
        if words_of(role).as_deref() != Some("AXUnknown") {
            return None;
        }
        let named = kept(&own().node_description)(this, sel!(accessibilityRoleDescription));
        words_of(named)
            .filter(|named| named.starts_with("AX"))
            .map(|named| ns_string(&named))
    }
}

extern "C-unwind" fn node_role(this: Id, selector: Sel) -> Id {
    // SAFETY: AccessKit's own answers, on the main thread.
    unsafe { named_role(this).unwrap_or_else(|| kept(&own().node_role)(this, selector)) }
}

/// AccessKit's subrole, or nil instead of `AXUnknown`, as AppKit's tables, cells, text fields,
/// scroll areas and columns have none.
extern "C-unwind" fn node_subrole(this: Id, selector: Sel) -> Id {
    // SAFETY: AccessKit's own answers, on the main thread.
    unsafe {
        if named_role(this).is_some() {
            return std::ptr::null_mut();
        }
        let subrole = kept(&own().node_subrole)(this, selector);
        if words_of(subrole).as_deref() == Some("AXUnknown") {
            return std::ptr::null_mut();
        }
        subrole
    }
}

extern "C-unwind" fn node_description(this: Id, selector: Sel) -> Id {
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        fn NSAccessibilityRoleDescription(role: Id, subrole: Id) -> Id;
    }
    // SAFETY: AccessKit and AppKit calls on the main thread.
    unsafe {
        match named_role(this) {
            Some(role) => NSAccessibilityRoleDescription(role, std::ptr::null_mut()),
            None => kept(&own().node_description)(this, selector),
        }
    }
}

/// The field's placeholder, reported even when filled, as AppKit fields do.
extern "C-unwind" fn node_placeholder(this: Id, selector: Sel) -> Id {
    // SAFETY: AccessKit's own answers, on the main thread.
    unsafe {
        let own = kept(&own().node_placeholder)(this, selector);
        if !own.is_null() {
            return own;
        }
        let identifier: Id = msg_send![this, accessibilityIdentifier];
        words_of(identifier)
            .and_then(|identifier| {
                PLACEHOLDERS.with(|placeholders| placeholders.borrow().get(&identifier).cloned())
            })
            .map_or(std::ptr::null_mut(), |words| ns_string(&words))
    }
}

/// Swizzles AccessKit's element class once it exists, which is the first time AccessKit returns
/// children; `children` holds some of its instances.
///
/// # Safety
/// A live array of AccessKit elements, or null, on the main thread.
unsafe fn install_on_nodes(children: Id) {
    if children.is_null() || own().node_children.get().is_some() {
        return;
    }
    // SAFETY: per the caller; each replacement matches its selector's types.
    unsafe {
        let count: usize = msg_send![children, count];
        if count == 0 {
            return;
        }
        let first: Id = msg_send![children, objectAtIndex: 0_usize];
        let class: *const AnyClass = objc2::ffi::object_getClass(first.cast());
        let answer =
            |with: extern "C-unwind" fn(Id, Sel) -> Id| -> Imp { std::mem::transmute(with) };
        let slots = own();
        for (selector, with, slot) in [
            (
                sel!(accessibilityChildren),
                node_children as extern "C-unwind" fn(Id, Sel) -> Id,
                &slots.node_children,
            ),
            (
                sel!(accessibilityChildrenInNavigationOrder),
                node_ordered,
                &slots.node_ordered,
            ),
            (sel!(accessibilityRole), node_role, &slots.node_role),
            (
                sel!(accessibilitySubrole),
                node_subrole,
                &slots.node_subrole,
            ),
            (
                sel!(accessibilityRoleDescription),
                node_description,
                &slots.node_description,
            ),
            (
                sel!(accessibilityPlaceholderValue),
                node_placeholder,
                &slots.node_placeholder,
            ),
        ] {
            replace(class, selector, answer(with), c"@@:", slot);
        }
    }
}

/// Installs this module's swizzles for `window`'s view classes: focus and hit testing on GPUI's
/// view, plus hosted views and extra roles on AccessKit's root and elements. Runs once, since
/// every window shares the classes.
pub fn report_accessibility_focus(window: &gpui::Window) {
    let view: Id = match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::AppKit(handle)) => handle.ns_view.as_ptr().cast(),
        _ => panic!("a GPUI window on a Mac has an AppKit view"),
    };
    let answer =
        |with: extern "C-unwind" fn(Id, Sel) -> Id| -> Imp { unsafe { std::mem::transmute(with) } };
    let slots = own();
    // SAFETY: the implementations match their selectors' types; GPUI's and AccessKit's view
    // classes live for the app's lifetime, and NSView implements every replaced selector.
    unsafe {
        let class: *const AnyClass = msg_send![view, class];
        replace(
            class,
            sel!(accessibilityFocusedUIElement),
            answer(view_focus),
            c"@@:",
            &slots.view_focus,
        );
        let hit: Imp = std::mem::transmute(view_hit as extern "C-unwind" fn(Id, Sel, Point) -> Id);
        replace(
            class,
            sel!(accessibilityHitTest:),
            hit,
            c"@@:{CGPoint=dd}",
            &slots.view_hit,
        );
        let under: Id = msg_send![view, superview];
        let root: *const AnyClass = objc2::ffi::object_getClass(under.cast());
        replace(
            root,
            sel!(accessibilityChildren),
            answer(root_children),
            c"@@:",
            &slots.root_children,
        );
        replace(
            root,
            sel!(accessibilityFocusedUIElement),
            answer(root_focus),
            c"@@:",
            &slots.root_focus,
        );
        let hit: Imp = std::mem::transmute(root_hit as extern "C-unwind" fn(Id, Sel, Point) -> Id);
        replace(
            root,
            sel!(accessibilityHitTest:),
            hit,
            c"@@:{CGPoint=dd}",
            &slots.root_hit,
        );
    }
}
