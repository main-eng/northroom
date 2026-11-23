//! The share sheet (`DataTransferManager`), as Explorer's Share opens it. The sheet requests the
//! data as it opens, and the files are resolved to storage items off the UI thread while it
//! waits.

use crate::platform::portable::windows_path_text;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use windows::ApplicationModel::DataTransfer::{
    DataRequest, DataRequestedEventArgs, DataTransferManager,
};
use windows::Foundation::TypedEventHandler;
use windows::Storage::{IStorageItem, StorageFile, StorageFolder};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::IDataTransferManagerInterop;
use windows::core::{HSTRING, Interface, Ref, Result};
use windows_collections::IIterable;

/// Opens the share sheet for `paths` over `window`. Windows positions the sheet, so the anchor
/// is ignored.
pub fn share(paths: &[PathBuf], window: &gpui::Window, _at: gpui::Bounds<gpui::Pixels>) {
    if let Err(error) = show_share_sheet(super::window_handle(window), paths.to_vec()) {
        eprintln!("share\tthe share sheet did not open: {error}");
    }
}

/// Shows `window`'s share sheet and answers its single data request with `paths`.
fn show_share_sheet(window: HWND, paths: Vec<PathBuf>) -> Result<()> {
    let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()?;
    // SAFETY: the app's window, on its own thread.
    let manager: DataTransferManager = unsafe { interop.GetForWindow(window) }?;
    // Remove the handler after it answers, since each sheet has its own files.
    let registration = Arc::new(OnceLock::new());
    let handler = TypedEventHandler::new({
        let registration = Arc::clone(&registration);
        move |sender: Ref<DataTransferManager>, args: Ref<DataRequestedEventArgs>| {
            if let (Some(manager), Some(&token)) = (sender.as_ref(), registration.get()) {
                manager.RemoveDataRequested(token)?;
            }
            offer_files(&args.ok()?.Request()?, paths.clone())
        }
    });
    let token = manager.DataRequested(&handler)?;
    registration
        .set(token)
        .expect("a handler is registered once");
    // SAFETY: as above.
    if let Err(error) = unsafe { interop.ShowShareUIForWindow(window) } {
        manager.RemoveDataRequested(token)?;
        return Err(error);
    }
    Ok(())
}

/// Answers `request` with `paths`: their names as the title right away, and the files once
/// resolved, which the sheet waits for (a deferral).
fn offer_files(request: &DataRequest, paths: Vec<PathBuf>) -> Result<()> {
    let data = request.Data()?;
    data.Properties()?
        .SetTitle(&HSTRING::from(sheet_title(&paths)))?;
    let deferral = request.GetDeferral()?;
    let request = request.clone();
    std::thread::spawn(move || {
        // The lookups run in the multithreaded apartment, which this thread stays in until the
        // sheet is answered.
        let apartment = Apartment::enter();
        let offered = match &apartment {
            Ok(_) => storage_items(&paths)
                .and_then(|items| data.SetStorageItemsReadOnly(&IIterable::from(items))),
            Err(error) => Err(error.clone()),
        };
        if let Err(error) = offered {
            eprintln!("share\tthe files were not offered: {error}");
            if let Err(error) = request.FailWithDisplayText(&error.message().into()) {
                eprintln!("share\tthe sheet was not told: {error}");
            }
        }
        if let Err(error) = deferral.Complete() {
            eprintln!("share\tthe sheet was not answered: {error}");
        }
    });
    Ok(())
}

/// The sheet title: the shared file names.
fn sheet_title(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .filter_map(|path| path.file_name())
        .map(|name| name.to_string_lossy())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `paths` as storage items (files and folders).
fn storage_items(paths: &[PathBuf]) -> Result<Vec<Option<IStorageItem>>> {
    paths
        .iter()
        .map(|path| {
            let name = HSTRING::from(windows_path_text(path));
            let item: IStorageItem = if path.is_dir() {
                StorageFolder::GetFolderFromPathAsync(&name)?
                    .join()?
                    .cast()?
            } else {
                StorageFile::GetFileFromPathAsync(&name)?.join()?.cast()?
            };
            Ok(Some(item))
        })
        .collect()
}

/// Keeps this thread in COM's multithreaded apartment until dropped.
struct Apartment;

impl Apartment {
    fn enter() -> Result<Self> {
        // SAFETY: a thread this module owns; uninitialised on drop.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances the initialisation above.
        unsafe { CoUninitialize() };
    }
}
