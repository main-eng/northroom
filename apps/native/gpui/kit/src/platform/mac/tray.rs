//! The menu bar icon: a template image at both scales, a tooltip, and a menu rebuilt from the
//! core each time it opens. The chosen row is reported by index.

use super::super::tray::{TrayMenu, TrayRow};
use super::appkit::{Id, Size, ns_string};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, msg_send, sel};

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "NRStatusMenu"]
    #[ivars = TrayMenu]
    struct Target;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        /// Rebuilds the rows as the menu opens (`NSMenuDelegate`).
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: *mut AnyObject) {
            let rows = (self.ivars().rows)();
            // SAFETY: this target's own menu, on the main thread.
            unsafe {
                let _: () = msg_send![menu, removeAllItems];
                for (place, row) in rows.iter().enumerate() {
                    let _: () = msg_send![menu, addItem: self.item(place, row)];
                }
            }
        }

        #[unsafe(method(choose:))]
        fn choose(&self, item: *mut AnyObject) {
            // SAFETY: the menu item that sent it.
            let place: isize = unsafe { msg_send![item, tag] };
            (self.ivars().chosen)(place as usize);
        }
    }
);

impl Target {
    /// The menu item for `row` at index `place`.
    fn item(&self, place: usize, row: &TrayRow) -> Id {
        // SAFETY: items are created on the main thread and retained by the menu.
        unsafe {
            if row.separator {
                return msg_send![class!(NSMenuItem), separatorItem];
            }
            let item: Id = msg_send![class!(NSMenuItem), alloc];
            let item: Id = msg_send![
                item,
                initWithTitle: ns_string(&row.title),
                action: sel!(choose:),
                keyEquivalent: ns_string(&row.key)
            ];
            if row.key.is_empty() {
                // No shortcut, so no modifier.
                let _: () = msg_send![item, setKeyEquivalentModifierMask: 0usize];
            }
            let _: () = msg_send![item, setTarget: self];
            let _: () = msg_send![item, setTag: place as isize];
            let _: () = msg_send![item, setEnabled: row.enabled];
            let _: () = msg_send![item, setState: isize::from(row.ticked)];
            let _: Id = msg_send![item, autorelease];
            item
        }
    }
}

/// The menu bar icon; dropping it removes the icon.
pub struct Tray {
    item: Retained<AnyObject>,
    _target: Retained<Target>,
}

/// Adds the icon to the menu bar, with `pictures` (PNG at 1x and 2x) as a template image,
/// `tooltip`, and `menu` on click.
pub fn show_tray(pictures: [&[u8]; 2], tooltip: &str, menu: TrayMenu) -> Tray {
    let main_thread = MainThreadMarker::new().expect("the tray is made on the main thread");
    let target = Target::alloc(main_thread).set_ivars(menu);
    // SAFETY: NSObject's designated initialiser.
    let target: Retained<Target> = unsafe { msg_send![super(target), init] };
    // SAFETY: status bar, images and menus are created and set on the main thread.
    unsafe {
        let bar: Id = msg_send![class!(NSStatusBar), systemStatusBar];
        // `NSVariableStatusItemLength`.
        let item: Retained<AnyObject> = msg_send![bar, statusItemWithLength: -1.0f64];
        let point = Size {
            width: 16.0,
            height: 16.0,
        };
        let image: Id = msg_send![class!(NSImage), alloc];
        let image: Id = msg_send![image, initWithSize: point];
        for picture in pictures {
            let data: Id = msg_send![
                class!(NSData),
                dataWithBytes: picture.as_ptr(),
                length: picture.len()
            ];
            let representation: Id = msg_send![class!(NSBitmapImageRep), imageRepWithData: data];
            let _: () = msg_send![representation, setSize: point];
            let _: () = msg_send![image, addRepresentation: representation];
        }
        let _: () = msg_send![image, setTemplate: true];
        let button: Id = msg_send![&*item, button];
        let _: () = msg_send![button, setImage: image];
        let _: () = msg_send![image, release];
        let _: () = msg_send![button, setToolTip: ns_string(tooltip)];
        let shown: Id = msg_send![class!(NSMenu), alloc];
        let shown: Id = msg_send![shown, initWithTitle: ns_string("")];
        let _: () = msg_send![shown, setDelegate: &*target];
        let _: () = msg_send![&*item, setMenu: shown];
        let _: () = msg_send![shown, release];
        Tray {
            item,
            _target: target,
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        // SAFETY: removes the status bar's own item on the main thread.
        unsafe {
            let bar: Id = msg_send![class!(NSStatusBar), systemStatusBar];
            let _: () = msg_send![bar, removeStatusItem: &*self.item];
        }
    }
}
