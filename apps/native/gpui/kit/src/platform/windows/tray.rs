//! The notification area icon: the template image tinted for the taskbar (black on light, white
//! on dark), a tooltip, and a menu rebuilt each time it opens. A hidden message window receives
//! the icon's clicks.

use super::super::tray::{TrayMenu, TrayRow};
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon,
    DestroyMenu, DestroyWindow, GetCursorPos, HICON, HWND_MESSAGE, ICONINFO, MENU_ITEM_FLAGS,
    MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, PostMessageW, RegisterClassW,
    SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

/// The message the icon sends its window on click.
const CLICKED: u32 = WM_APP + 1;

thread_local! {
    /// The icon's menu, read by the window procedure on the UI thread.
    static MENU: RefCell<Option<TrayMenu>> = const { RefCell::new(None) };
}

/// The notification area icon; dropping it removes the icon.
pub struct Tray {
    window: HWND,
    icon: HICON,
}

/// `words` as a NUL-terminated wide string.
fn wide(words: &str) -> Vec<u16> {
    words.encode_utf16().chain(Some(0)).collect()
}

/// Whether the taskbar is light (`SystemUsesLightTheme`), which sets the icon colour.
fn taskbar_is_light() -> bool {
    let mut light = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: a DWORD value read into a DWORD.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut light).cast()),
            Some(&mut size),
        )
    };
    read.is_ok() && light == 1
}

/// The template image (PNG) as an icon, tinted for the taskbar.
fn icon_of(picture: &[u8]) -> HICON {
    let shape = image::load_from_memory(picture)
        .expect("the tray's picture is a PNG")
        .to_rgba8();
    let ink = if taskbar_is_light() { 0 } else { 255 };
    // BGRA, as a 32-bit colour bitmap stores it.
    let colour: Vec<u8> = shape
        .pixels()
        .flat_map(|pixel| [ink, ink, ink, pixel[3]])
        .collect();
    let (width, height) = (shape.width() as i32, shape.height() as i32);
    let mask = vec![0u8; (width as usize).div_ceil(16) * 2 * height as usize];
    // SAFETY: the bitmaps match the pixel buffer and are released after the icon copies them.
    unsafe {
        let colour = CreateBitmap(width, height, 1, 32, Some(colour.as_ptr().cast()));
        let mask = CreateBitmap(width, height, 1, 1, Some(mask.as_ptr().cast()));
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: colour,
            ..Default::default()
        })
        .expect("Windows makes the tray's icon");
        let _ = DeleteObject(colour.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

/// Shows the menu at the pointer and passes the chosen row to the owner.
fn show_menu(window: HWND) {
    let rows = MENU.with(|menu| menu.borrow().as_ref().map(|menu| (menu.rows)()));
    let Some(rows) = rows else {
        return;
    };
    // SAFETY: the menu is created, shown and destroyed here on the window's thread.
    let chosen = unsafe {
        let Ok(popup) = CreatePopupMenu() else {
            return;
        };
        for (place, row) in rows.iter().enumerate() {
            add_row(popup, place, row);
        }
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // The menu dismisses on an outside click only while its window is foreground.
        let _ = SetForegroundWindow(window);
        let chosen = TrackPopupMenu(
            popup,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            at.x,
            at.y,
            None,
            window,
            None,
        );
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(popup);
        chosen.0
    };
    // Command IDs are the row index plus 1; 0 means nothing was chosen.
    if let Ok(place) = usize::try_from(chosen - 1) {
        MENU.with(|menu| {
            if let Some(menu) = menu.borrow().as_ref() {
                (menu.chosen)(place);
            }
        });
    }
}

/// Adds `row` at index `place` to `popup`.
///
/// # Safety
/// `popup` is a live menu of this thread's.
unsafe fn add_row(
    popup: windows::Win32::UI::WindowsAndMessaging::HMENU,
    place: usize,
    row: &TrayRow,
) {
    if row.separator {
        // SAFETY: per the caller.
        let _ = unsafe { AppendMenuW(popup, MF_SEPARATOR, 0, PCWSTR::null()) };
        return;
    }
    let mut flags: MENU_ITEM_FLAGS = MF_STRING;
    if !row.enabled {
        flags |= MF_GRAYED;
    }
    if row.ticked {
        flags |= MF_CHECKED;
    }
    let title = wide(&row.title);
    // SAFETY: per the caller; the title outlives the call, which copies it.
    let _ = unsafe { AppendMenuW(popup, flags, place + 1, PCWSTR(title.as_ptr())) };
}

/// The hidden window's procedure: a click on the icon with either button opens the menu.
extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == CLICKED {
        let clicked = (lparam.0 & 0xffff) as u32;
        if clicked == WM_LBUTTONUP || clicked == WM_RBUTTONUP {
            show_menu(window);
        }
        return LRESULT(0);
    }
    // SAFETY: the default procedure handles every other message.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Adds the icon to the notification area with `pictures` (PNG at 1x and 2x; Windows scales the
/// larger), `tooltip`, and `menu` on click.
pub fn show_tray(pictures: [&[u8]; 2], tooltip: &str, menu: TrayMenu) -> Tray {
    MENU.with(|shown| *shown.borrow_mut() = Some(menu));
    // SAFETY: a window class and message-only window on this thread, and an icon Windows copies
    // into the notification area.
    unsafe {
        let module = GetModuleHandleW(None).expect("the app has a module");
        let class = w!("NorthroomTray");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        });
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Northroom"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(module.into()),
            None,
        )
        .expect("Windows makes the tray's window");
        let icon = icon_of(pictures[1]);
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: CLICKED,
            hIcon: icon,
            ..Default::default()
        };
        for (to, from) in data.szTip.iter_mut().zip(tooltip.encode_utf16().take(127)) {
            *to = from;
        }
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
        Tray { window, icon }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.window,
            uID: 1,
            ..Default::default()
        };
        // SAFETY: removes the icon and window from `show_tray` on their own thread.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            let _ = DestroyIcon(self.icon);
            let _ = DestroyWindow(self.window);
        }
        MENU.with(|shown| *shown.borrow_mut() = None);
    }
}
