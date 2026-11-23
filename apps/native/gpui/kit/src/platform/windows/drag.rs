//! Dragging files out of the window, as Explorer does. The shell's own data object for the items
//! (`BHID_DataObject`, offering `CF_HDROP` and the other shell formats) goes to the shell's drag
//! loop (`SHDoDragDrop`) with its own drop source, so any app that accepts files gets real
//! files. The drag image is the first file's thumbnail, else the shell's icons. Only copy is
//! offered, so Explorer never moves files out of the drive.

use super::window_handle;
use crate::lists::chip::Carried;
use gpui::RenderImage;
use image::imageops::{FilterType, resize};
use windows::Win32::Foundation::{COLORREF, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IBindCtx, IDataObject};
use windows::Win32::System::Ole::{DROPEFFECT_COPY, IDropSource};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, CLSID_DragDropHelper, IDragSourceHelper, ILCreateFromPathW, ILFree,
    SHCreateShellItemArrayFromIDLists, SHDRAGIMAGE, SHDoDragDrop,
};
use windows::core::HSTRING;

/// Side of the square the drag image fits in, in points. Matches Explorer's large icon.
const PICTURE_POINTS: f32 = 64.0;

/// `CLR_NONE`: no colour key; alpha decides what shows.
const NO_COLOUR_KEY: COLORREF = COLORREF(0xFFFF_FFFF);

/// Starts a shell drag of `files` from `window` after the current event. False when the shell
/// can't make a data object for them.
pub fn drag_files_out(files: &[Carried], window: &mut gpui::Window, cx: &mut gpui::App) -> bool {
    let Some(data) = data_object(files) else {
        return false;
    };
    if let Some(picture) = files.first().and_then(|file| file.picture.as_ref()) {
        show_picture(&data, picture, window.scale_factor());
    }
    let window = window_handle(window);
    // The shell's drag runs its own message loop until the drop. Started inside an event, the
    // window's messages would re-enter an event GPUI is still handling, so wait until it ends.
    cx.foreground_executor()
        .spawn(async move {
            // SAFETY: the window and data object outlive the drag, which runs on their thread;
            // the shell supplies the drop source (`None`). The result is ignored: the drop is
            // the destination's business, and a drop nowhere just ends.
            let _ =
                unsafe { SHDoDragDrop(Some(window), &data, None::<&IDropSource>, DROPEFFECT_COPY) };
        })
        .detach();
    true
}

/// The shell's data object for `files`, as Explorer builds it for a selection.
fn data_object(files: &[Carried]) -> Option<IDataObject> {
    // SAFETY: each path is a NUL-terminated wide string; the item lists are freed here after the
    // shell copies them into its array.
    unsafe {
        let items: Vec<*mut ITEMIDLIST> = files
            .iter()
            .map(|file| ILCreateFromPathW(&HSTRING::from(file.path.as_os_str())))
            .filter(|item| !item.is_null())
            .collect();
        let listed: Vec<*const ITEMIDLIST> = items.iter().map(|item| item.cast_const()).collect();
        let data = (!listed.is_empty())
            .then(|| {
                SHCreateShellItemArrayFromIDLists(&listed).and_then(|array| {
                    array.BindToHandler::<_, IDataObject>(None::<&IBindCtx>, &BHID_DataObject)
                })
            })
            .and_then(Result::ok);
        for item in items {
            ILFree(Some(item.cast_const()));
        }
        data
    }
}

/// Sets `picture` (sRGB, straight alpha) as the drag image, fitted to a `PICTURE_POINTS` square
/// at `scale` px per pt and centred on the pointer. Falls back to the shell's icons if the drag
/// helper refuses it.
fn show_picture(data: &IDataObject, picture: &RenderImage, scale: f32) {
    let Some(bytes) = picture.as_bytes(0) else {
        return;
    };
    let pixels = picture.size(0);
    let (width, height) = (pixels.width.0 as u32, pixels.height.0 as u32);
    let Some(source) = image::RgbaImage::from_raw(width, height, bytes.to_vec()) else {
        return;
    };
    let fit = PICTURE_POINTS * scale / width.max(height) as f32;
    let (width, height) = (
        ((width as f32 * fit).round() as u32).max(1),
        ((height as f32 * fit).round() as u32).max(1),
    );
    // BGRA; resizing treats every channel the same.
    let mut fitted = resize(&source, width, height, FilterType::Triangle).into_raw();
    // The shell expects premultiplied alpha.
    for pixel in fitted.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width as i32,
        // Rows top first.
        biHeight: -(height as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    // SAFETY: a `width` × `height` 32-bit bitmap filled from `fitted`, which has that many
    // pixels. The drag helper owns it if it accepts it; otherwise it is deleted here.
    unsafe {
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return;
        };
        std::ptr::copy_nonoverlapping(fitted.as_ptr(), bits.cast::<u8>(), fitted.len());
        let image = SHDRAGIMAGE {
            sizeDragImage: SIZE {
                cx: width as i32,
                cy: height as i32,
            },
            ptOffset: POINT {
                x: width as i32 / 2,
                y: height as i32 / 2,
            },
            hbmpDragImage: bitmap,
            crColorKey: NO_COLOUR_KEY,
        };
        let shown = CoCreateInstance::<_, IDragSourceHelper>(
            &CLSID_DragDropHelper,
            None,
            CLSCTX_INPROC_SERVER,
        )
        .and_then(|helper| helper.InitializeFromBitmap(&image, data));
        if shown.is_err() {
            delete(bitmap);
        }
    }
}

/// Deletes a bitmap the drag helper didn't take.
fn delete(bitmap: HBITMAP) {
    // SAFETY: a bitmap created here with no other owner.
    let _ = unsafe { DeleteObject(bitmap.into()) };
}
