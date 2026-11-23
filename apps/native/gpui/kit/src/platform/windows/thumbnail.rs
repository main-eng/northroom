//! Shell thumbnails (`IShellItemImageFactory`), the same ones Explorer shows. Only content
//! thumbnails are requested (`SIIGBF_THUMBNAILONLY`), never icons, so files without one keep
//! Northroom's glyph.

use super::Pixels;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_THUMBNAILONLY,
};
use windows::core::HSTRING;

/// The shell thumbnail of `path`, at most `pixels` on its long side, as BGRA with straight
/// alpha; none when there is no content thumbnail. Handlers can be slow, so call it off the
/// main thread.
pub fn system_thumbnail(path: &str, pixels: u32) -> Option<Pixels> {
    // SAFETY: initialises COM in a single-threaded apartment, which thumbnail handlers expect,
    // and uninitialises it afterwards if this call started it.
    let started = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let read = shell_thumbnail(path, pixels);
    if started {
        // SAFETY: balances the start above on the same thread.
        unsafe { CoUninitialize() };
    }
    read
}

fn shell_thumbnail(path: &str, pixels: u32) -> Option<Pixels> {
    let side = i32::try_from(pixels).ok()?;
    // SAFETY: the call copies the wide path; we own the returned bitmap and delete it after
    // reading.
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        let bitmap = factory
            .GetImage(SIZE { cx: side, cy: side }, SIIGBF_THUMBNAILONLY)
            .ok()?;
        let read = pixels_of(bitmap);
        let _ = DeleteObject(bitmap.into());
        read
    }
}

/// The 32-bit pixels of `bitmap`, top row first, unpremultiplied.
///
/// # Safety
/// `bitmap` is a live bitmap.
unsafe fn pixels_of(bitmap: HBITMAP) -> Option<Pixels> {
    let mut held = BITMAP::default();
    // SAFETY: per the caller; `held` matches the size passed to the call.
    let described = unsafe {
        GetObjectW(
            bitmap.into(),
            size_of::<BITMAP>() as i32,
            Some((&raw mut held).cast()),
        )
    };
    let (width, height) = (held.bmWidth, held.bmHeight.abs());
    if described == 0 || width <= 0 || height <= 0 {
        return None;
    }
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Rows top first.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let (width, height) = (width as u32, height as u32);
    let mut bgra = vec![0u8; width as usize * height as usize * 4];
    // SAFETY: `bgra` holds `height` rows of `width` 32-bit pixels, matching `info`; the memory
    // context is created and deleted here.
    let copied = unsafe {
        let context = CreateCompatibleDC(None);
        let copied = GetDIBits(
            context,
            bitmap,
            0,
            height,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(context);
        copied
    };
    if copied != height as i32 {
        return None;
    }
    straighten_alpha(&mut bgra);
    Some(Pixels {
        width,
        height,
        bgra,
    })
}

/// Unpremultiplies a shell thumbnail for GPUI. Handlers return premultiplied alpha, or all-zero
/// alpha for images without alpha, which means opaque.
fn straighten_alpha(bgra: &mut [u8]) {
    let pixels = bgra.as_chunks_mut::<4>().0;
    if pixels.iter().all(|pixel| pixel[3] == 0) {
        for pixel in pixels {
            pixel[3] = u8::MAX;
        }
        return;
    }
    for pixel in pixels {
        let alpha = u16::from(pixel[3]);
        if alpha == 0 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thumbnail_with_no_alpha_is_opaque_and_one_with_alpha_has_it_taken_out() {
        let mut opaque = [10, 20, 30, 0, 40, 50, 60, 0];
        straighten_alpha(&mut opaque);
        assert_eq!(opaque, [10, 20, 30, 255, 40, 50, 60, 255]);
        let mut multiplied = [64, 32, 0, 128, 0, 0, 0, 0];
        straighten_alpha(&mut multiplied);
        assert_eq!(multiplied, [128, 64, 0, 128, 0, 0, 0, 0]);
    }
}
