//! Menu bar access beyond GPUI. Reads the items AppKit adds to the Edit menu (Writing Tools,
//! AutoFill, Start Dictation, Emoji & Symbols), which only the native menu knows; hides items
//! the core hides while keeping their key equivalents; and enables paste items only when the
//! clipboard holds text or files.

use super::appkit::{Id, application};
use objc2::runtime::Bool;
use objc2::{msg_send, sel};

/// The text of an `NSString`.
fn words_of(string: Id) -> String {
    if string.is_null() {
        return String::new();
    }
    // SAFETY: the UTF-8 bytes, when available, live as long as the `NSString`.
    unsafe {
        let bytes: *const std::ffi::c_char = msg_send![string, UTF8String];
        if bytes.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(bytes)
            .to_string_lossy()
            .into_owned()
    }
}

/// The items of menu `index` after its first `ours` visible items, in the menu outline format:
/// `-` for a separator, `✓` before a checked item and ` (off)` after one its target disables.
pub fn added_items(index: usize, ours: usize) -> Vec<String> {
    let Some(menu) = bar_menu(index) else {
        return Vec::new();
    };
    // SAFETY: the menu and its items are AppKit's, read on the main thread.
    unsafe {
        let items: isize = msg_send![menu, numberOfItems];
        (0..items)
            .map(|place| -> Id { msg_send![menu, itemAtIndex: place] })
            .filter(|&item| {
                let hidden: Bool = msg_send![item, isHidden];
                !hidden.as_bool()
            })
            .skip(ours)
            .map(outlined)
            .collect()
    }
}

fn bar_menu(index: usize) -> Option<Id> {
    // SAFETY: the main menu and its items are AppKit's, read on the main thread.
    unsafe {
        let main: Id = msg_send![application(), mainMenu];
        if main.is_null() {
            return None;
        }
        let count: isize = msg_send![main, numberOfItems];
        if index as isize >= count {
            return None;
        }
        let top: Id = msg_send![main, itemAtIndex: index as isize];
        let menu: Id = msg_send![top, submenu];
        (!menu.is_null()).then_some(menu)
    }
}

/// `item` in the menu outline format.
fn outlined(item: Id) -> String {
    // SAFETY: an AppKit menu item, read on the main thread.
    unsafe {
        let separator: Bool = msg_send![item, isSeparatorItem];
        if separator.as_bool() {
            return "-".to_owned();
        }
        let title = words_of(msg_send![item, title]);
        // Read after validation, which may check it.
        let usable = can_choose(item);
        let state: isize = msg_send![item, state];
        format!(
            "{}{title}{}",
            if state == 1 { "✓" } else { "" },
            if usable { "" } else { " (off)" }
        )
    }
}

/// Whether `item` is enabled: it opens a submenu, or its target allows its action.
fn can_choose(item: Id) -> bool {
    // SAFETY: an AppKit menu item and its target, read on the main thread.
    unsafe {
        let submenu: Id = msg_send![item, submenu];
        let action: Option<objc2::runtime::Sel> = msg_send![item, action];
        let target: Id = msg_send![item, target];
        let checks = !target.is_null() && {
            let answers: Bool = msg_send![target, respondsToSelector: sel!(validateMenuItem:)];
            answers.as_bool()
        };
        !submenu.is_null()
            || (action.is_some()
                && (!checks || {
                    let valid: Bool = msg_send![target, validateMenuItem: item];
                    valid.as_bool()
                }))
    }
}

/// Hides the items at `places` (index paths through the menu bar and its submenus) and keeps
/// their key equivalents working. AppKit runs a hidden item's key equivalent once the first
/// responder passes the key on.
pub fn hide_items(places: &[Vec<usize>]) {
    // SAFETY: changed on the main thread; every place is an item the caller set (`set_menus`),
    // ahead of any AppKit adds.
    unsafe {
        let main: Id = msg_send![application(), mainMenu];
        if main.is_null() {
            return;
        }
        for place in places {
            let item = item_at(main, place);
            let _: () = msg_send![item, setHidden: Bool::YES];
            let _: () = msg_send![item, setAllowsKeyEquivalentWhenHidden: Bool::YES];
        }
    }
}

thread_local! {
    /// Paste items, enabled only when the clipboard holds text or files.
    static PASTING: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The application delegate's own `validateMenuItem:`.
static OWN_VALIDATE: std::sync::OnceLock<objc2::runtime::Imp> = std::sync::OnceLock::new();

/// The item at index path `place`.
///
/// # Safety
/// The main menu has an item there, and this runs on the main thread.
unsafe fn item_at(main: Id, place: &[usize]) -> Id {
    let (mut menu, mut item): (Id, Id) = (main, std::ptr::null_mut());
    for &index in place {
        // SAFETY: per the caller.
        unsafe {
            item = msg_send![menu, itemAtIndex: index as isize];
            menu = msg_send![item, submenu];
        }
    }
    item
}

/// Replacement `validateMenuItem:` for the application delegate. It keeps the delegate's answer
/// and enables paste items only with text or files on the clipboard (fields paste text, as
/// AppKit text views do; folders paste files).
extern "C-unwind" fn validate(this: Id, selector: objc2::runtime::Sel, item: Id) -> Bool {
    type Own = unsafe extern "C-unwind" fn(Id, objc2::runtime::Sel, Id) -> Bool;
    // SAFETY: the original implementation has the selector's types (`c@:@`).
    let own: Own = unsafe {
        std::mem::transmute(
            *OWN_VALIDATE
                .get()
                .expect("the delegate's own answer is kept"),
        )
    };
    // SAFETY: AppKit asks on the main thread about a live item.
    let valid = unsafe { own(this, selector, item) };
    let pastes = PASTING.with(|pasting| pasting.borrow().contains(&(item as usize)));
    Bool::new(valid.as_bool() && (!pastes || crate::platform::clipboard::holds_paste()))
}

/// Marks the items at `places` as paste items, enabled only with text or files on the clipboard.
/// Replaces the delegate's `validateMenuItem:` on first call.
pub fn paste_items(places: &[Vec<usize>]) {
    // SAFETY: on the main thread; every place is an item the caller set, and the replacement
    // has the selector's types.
    unsafe {
        let main: Id = msg_send![application(), mainMenu];
        if main.is_null() {
            return;
        }
        let items: Vec<usize> = places
            .iter()
            .map(|place| item_at(main, place) as usize)
            .collect();
        PASTING.with(|pasting| *pasting.borrow_mut() = items);
        let delegate: Id = msg_send![application(), delegate];
        let class: *const objc2::runtime::AnyClass = msg_send![delegate, class];
        let selector = sel!(validateMenuItem:);
        let method = (*class)
            .instance_method(selector)
            .expect("GPUI's delegate validates the menu bar's items");
        if OWN_VALIDATE.set(method.implementation()).is_ok() {
            let with: objc2::runtime::Imp = std::mem::transmute(
                validate as extern "C-unwind" fn(Id, objc2::runtime::Sel, Id) -> Bool,
            );
            objc2::ffi::class_replaceMethod(class.cast_mut(), selector, with, c"c@:@".as_ptr());
        }
    }
}
