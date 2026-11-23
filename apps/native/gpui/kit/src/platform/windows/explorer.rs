//! Explorer integration: files on the clipboard (`CF_HDROP` plus paths as text, as Explorer's
//! Copy writes them) and revealing files in their folder.

use crate::platform::portable::{drop_files_block, group_by_folder, windows_path_text};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT, CLIPBOARD_FORMAT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};
use windows::core::{Error, HSTRING, Owned, Result};

/// Puts `paths` on the clipboard as files, plus the paths as text (one per line) for terminals
/// and text fields.
pub fn write_files(paths: &[PathBuf]) {
    let names: Vec<String> = paths.iter().map(|path| windows_path_text(path)).collect();
    let files = drop_files_block(&names);
    let text: Vec<u8> = names
        .join("\r\n")
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    if let Err(error) = replace_clipboard(&[(CF_HDROP, &files), (CF_UNICODETEXT, &text)]) {
        eprintln!("clipboard\tthe files were not copied: {error}");
    }
}

/// Replaces the clipboard contents with every format in one session, so a reader sees all of
/// them or none.
fn replace_clipboard(formats: &[(CLIPBOARD_FORMAT, &[u8])]) -> Result<()> {
    // SAFETY: no owner window, as GPUI's clipboard does; closed below.
    unsafe { OpenClipboard(None) }?;
    let placed = (|| {
        // SAFETY: the clipboard is open in this session.
        unsafe { EmptyClipboard() }?;
        formats
            .iter()
            .try_for_each(|&(format, bytes)| place_on_clipboard(format, bytes))
    })();
    // SAFETY: the session opened above.
    let closed = unsafe { CloseClipboard() };
    placed.and(closed)
}

/// Gives the clipboard a global memory block holding `bytes` as `format`.
fn place_on_clipboard(format: CLIPBOARD_FORMAT, bytes: &[u8]) -> Result<()> {
    // SAFETY: the block is `bytes.len()` long and locked while written; once the clipboard
    // takes it, the clipboard frees it.
    unsafe {
        let block = Owned::new(GlobalAlloc(GMEM_MOVEABLE, bytes.len())?);
        let at = GlobalLock(*block);
        if at.is_null() {
            return Err(Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at.cast::<u8>(), bytes.len());
        // Releasing the last lock returns an error whose code is success; ignore it.
        let _ = GlobalUnlock(*block);
        SetClipboardData(u32::from(format.0), Some(HANDLE(block.0)))?;
        std::mem::forget(block);
    }
    Ok(())
}

/// Selects `paths` in Explorer, one window per parent folder.
pub fn reveal(paths: &[PathBuf]) {
    for (folder, items) in group_by_folder(paths) {
        if let Err(error) = select_in_folder(folder, &items) {
            eprintln!("reveal\t{} was not shown: {error}", folder.display());
        }
    }
}

/// Opens `folder` in Explorer with `items` selected.
fn select_in_folder(folder: &Path, items: &[&Path]) -> Result<()> {
    let folder = ItemIdList::of(folder)?;
    let items = items
        .iter()
        .map(|item| ItemIdList::of(item))
        .collect::<Result<Vec<_>>>()?;
    let selected: Vec<*const ITEMIDLIST> = items.iter().map(|item| item.0.cast_const()).collect();
    // SAFETY: the lists outlive the call; GPUI has initialised COM on this thread.
    unsafe { SHOpenFolderAndSelectItems(folder.0, Some(&selected), 0) }
}

/// A shell item ID list for a path, freed on drop.
struct ItemIdList(*mut ITEMIDLIST);

impl ItemIdList {
    fn of(path: &Path) -> Result<Self> {
        let path = HSTRING::from(windows_path_text(path));
        // SAFETY: a NUL-terminated path that outlives the call.
        let list = unsafe { ILCreateFromPathW(&path) };
        if list.is_null() {
            Err(Error::from_thread())
        } else {
            Ok(Self(list))
        }
    }
}

impl Drop for ItemIdList {
    fn drop(&mut self) {
        // SAFETY: this value is the list's only owner.
        unsafe { ILFree(Some(self.0)) };
    }
}
