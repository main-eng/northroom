//! Picture files read by the Windows Imaging Component, as Image I/O reads them on a Mac: a
//! picture's displayed size, and its pixels turned by orientation, scaled down and converted to
//! sRGB. WIC always decodes PNG, JPEG, GIF, TIFF and BMP, and WebP, HEIC and AVIF where the
//! system extensions for them are installed (`decodes_extension`).

use super::super::picture_size;
use super::super::planar::{Still, Turn};
use super::start;
use std::sync::OnceLock;
use windows::core::{IUnknown, Interface, GUID, HSTRING, PWSTR};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::Media::MediaFoundation::{
    MFMediaType_Video, MFTEnumEx, MFVideoFormat_AV1, MFVideoFormat_HEVC,
    MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_ENUM_FLAG_SYNCMFT, MFT_REGISTER_TYPE_INFO,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoTaskMemFree, IEnumUnknown, CLSCTX_INPROC_SERVER,
};
use windows::Win32::System::Variant::VT_UI2;

/// WIC's metadata query for a picture's EXIF orientation, in every format that has one.
const ORIENTATION: &str = "System.Photo.Orientation";

/// The EXIF colour space value for sRGB.
const EXIF_SRGB: u32 = 1;

/// Bytes per pixel of 32-bit BGRA.
const BGRA_BYTES: u32 = 4;

/// A picture file opened for reading: its first frame and the factory that made it.
struct Opened {
    factory: IWICImagingFactory,
    frame: IWICBitmapFrameDecode,
}

impl Opened {
    /// The picture at `path`, or `None` if no WIC decoder reads it.
    fn at(path: &str) -> Option<Self> {
        start().ok()?;
        // SAFETY: COM is started on this thread; every argument is live for its call.
        unsafe {
            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let decoder = factory
                .CreateDecoderFromFilename(
                    &HSTRING::from(path),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )
                .ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            Some(Self { factory, frame })
        }
    }

    /// The frame's stored size, or `None` if it has none.
    fn stored_size(&self) -> Option<(u32, u32)> {
        let (mut width, mut height) = (0, 0);
        // SAFETY: a live frame writing two numbers.
        unsafe { self.frame.GetSize(&mut width, &mut height) }.ok()?;
        (width > 0 && height > 0).then_some((width, height))
    }

    /// How the picture is turned for display, from its EXIF orientation; upright if it has none.
    fn turn(&self) -> Turn {
        Turn::of_orientation(self.orientation().unwrap_or(1))
    }

    fn orientation(&self) -> Option<u16> {
        // SAFETY: a live frame; the value is read as an unsigned 16-bit number only when its
        // type says so, and a number holds nothing to free.
        unsafe {
            let reader = self.frame.GetMetadataQueryReader().ok()?;
            let mut value = PROPVARIANT::default();
            reader
                .GetMetadataByName(&HSTRING::from(ORIENTATION), &mut value)
                .ok()?;
            let value = &value.Anonymous.Anonymous;
            (value.vt == VT_UI2).then(|| value.Anonymous.uiVal)
        }
    }

    /// The frame's colour profile, if it names one other than sRGB.
    fn profile(&self) -> Option<IWICColorContext> {
        // SAFETY: a live frame and factory. The count is queried as WIC documents, with no
        // array at all (not windows-rs's dangling pointer for an empty slice), and the contexts
        // passed in are WIC's own, made here.
        unsafe {
            let mut count = 0;
            (Interface::vtable(&self.frame).GetColorContexts)(
                Interface::as_raw(&self.frame),
                0,
                std::ptr::null_mut(),
                &mut count,
            )
            .ok()
            .ok()?;
            if count == 0 {
                return None;
            }
            let mut contexts = (0..count)
                .map(|_| self.factory.CreateColorContext().ok().map(Some))
                .collect::<Option<Vec<_>>>()?;
            self.frame
                .GetColorContexts(&mut contexts, &mut count)
                .ok()?;
            let context = contexts.into_iter().next().flatten()?;
            let srgb = context.GetType().ok() == Some(WICColorContextExifColorSpace)
                && context.GetExifColorSpace().ok() == Some(EXIF_SRGB);
            (!srgb).then_some(context)
        }
    }

    /// The picture no larger than `max_pixels` on its long side, as sRGB BGRA with straight
    /// alpha, as stored (not yet turned).
    fn pixels(&self, max_pixels: u32) -> Option<Still> {
        let stored = self.stored_size()?;
        let (width, height) = picture_size(stored.0, stored.1, max_pixels);
        // SAFETY: a live frame and factory; each step reads the one before and all are held to
        // the end, and the buffer is as large as the stride and row count require.
        unsafe {
            let mut source: IWICBitmapSource = self.frame.cast().ok()?;
            if (width, height) != stored {
                let scaler = self.factory.CreateBitmapScaler().ok()?;
                scaler
                    .Initialize(
                        &source,
                        width,
                        height,
                        WICBitmapInterpolationModeHighQualityCubic,
                    )
                    .ok()?;
                source = scaler.cast().ok()?;
            }
            if let Some(converted) = self.in_srgb(&source) {
                source = converted;
            }
            if source.GetPixelFormat().ok()? != GUID_WICPixelFormat32bppBGRA {
                let converter = self.factory.CreateFormatConverter().ok()?;
                converter
                    .Initialize(
                        &source,
                        &GUID_WICPixelFormat32bppBGRA,
                        WICBitmapDitherTypeNone,
                        None::<&IWICPalette>,
                        0.0,
                        WICBitmapPaletteTypeCustom,
                    )
                    .ok()?;
                source = converter.cast().ok()?;
            }
            let stride = width * BGRA_BYTES;
            let mut bgra = vec![0; stride as usize * height as usize];
            source
                .CopyPixels(std::ptr::null(), stride, &mut bgra)
                .ok()?;
            Some(Still {
                width,
                height,
                bgra,
            })
        }
    }

    /// `source` converted from the frame's colour profile to sRGB, as BGRA. `None` if the frame
    /// names no profile, names sRGB, or names one WIC can't convert from; such pictures are
    /// treated as sRGB.
    fn in_srgb(&self, source: &IWICBitmapSource) -> Option<IWICBitmapSource> {
        let profile = self.profile()?;
        // SAFETY: a live factory, source and profile, held by the transform it makes.
        unsafe {
            let srgb = self.factory.CreateColorContext().ok()?;
            srgb.InitializeFromExifColorSpace(EXIF_SRGB).ok()?;
            let transform = self.factory.CreateColorTransformer().ok()?;
            transform
                .Initialize(source, &profile, &srgb, &GUID_WICPixelFormat32bppBGRA)
                .ok()?;
            transform.cast().ok()
        }
    }
}

/// The size in pixels the picture at `path` is shown at, after orientation; `None` if WIC
/// can't read it or it has no size.
pub fn picture_dimensions(path: &str) -> Option<(u32, u32)> {
    let picture = Opened::at(path)?;
    let (width, height) = picture.stored_size()?;
    Some(picture.turn().shown_size(width, height))
}

/// The picture at `path`, turned by orientation, no larger than `max_pixels` on its long side,
/// as sRGB BGRA with straight alpha, top row first. `None` if WIC can't decode it, such as a
/// HEIC without the HEVC decoder installed.
pub fn picture(path: &str, max_pixels: u32) -> Option<Still> {
    let picture = Opened::at(path)?;
    let still = picture.pixels(max_pixels)?;
    Some(picture.turn().apply(still))
}

/// Whether WIC decodes picture files with this extension (lowercase, no dot) on this machine.
/// One of its decoders must name it, and for HEIF (HEIC) or AVIF, Media Foundation must have
/// the HEVC or AV1 video decoder the picture decoder hands its pixels to. Both come from Store
/// extensions, so a file WIC can't decode shows as a plain file instead of a picture that never
/// appears. Checked once per run on its own thread, so the calling thread (a window's, say) is
/// never put in COM's multithreaded apartment.
pub fn decodes_extension(extension: &str) -> bool {
    static DECODED: OnceLock<Vec<String>> = OnceLock::new();
    DECODED
        .get_or_init(|| {
            std::thread::spawn(decoded_extensions)
                .join()
                .unwrap_or_default()
        })
        .iter()
        .any(|decoded| decoded == extension)
}

/// Every extension WIC decodes here, lowercase without the dot.
fn decoded_extensions() -> Vec<String> {
    let mut extensions = named_by_decoders();
    if !media_foundation_decodes(MFVideoFormat_HEVC) {
        extensions.retain(|extension| !matches!(extension.as_str(), "heic" | "heif" | "hif"));
    }
    if !media_foundation_decodes(MFVideoFormat_AV1) {
        extensions.retain(|extension| extension != "avif");
    }
    extensions
}

/// The extensions WIC's installed decoders name.
fn named_by_decoders() -> Vec<String> {
    if start().is_err() {
        return Vec::new();
    }
    // SAFETY: COM is started on this thread.
    unsafe {
        let Ok(factory) = CoCreateInstance::<_, IWICImagingFactory>(
            &CLSID_WICImagingFactory,
            None,
            CLSCTX_INPROC_SERVER,
        ) else {
            return Vec::new();
        };
        let Ok(decoders) = factory
            .CreateComponentEnumerator(WICDecoder.0 as u32, WICComponentEnumerateDefault.0 as u32)
        else {
            return Vec::new();
        };
        extensions_named_by(&decoders)
    }
}

/// The extensions named by every decoder `decoders` lists.
fn extensions_named_by(decoders: &IEnumUnknown) -> Vec<String> {
    let mut extensions = Vec::new();
    loop {
        let mut next: [Option<IUnknown>; 1] = [None];
        let mut fetched = 0;
        // SAFETY: COM is started on this thread, as `decoders` was made on it; one decoder is
        // fetched into room for one.
        if unsafe { decoders.Next(&mut next, Some(&mut fetched)) }.is_err() || fetched == 0 {
            break;
        }
        let Some(info) = next[0]
            .take()
            .and_then(|decoder| decoder.cast::<IWICBitmapDecoderInfo>().ok())
        else {
            continue;
        };
        if let Some(named) = file_extensions_of(&info) {
            extensions.extend(parse_extensions(&named));
        }
    }
    extensions
}

/// The extensions a decoder's `info` names (".heic,.heif,.avif"), if any.
fn file_extensions_of(info: &IWICBitmapDecoderInfo) -> Option<String> {
    let codec: &IWICBitmapCodecInfo = info;
    let mut length = 0;
    // SAFETY: COM is started on this thread, as `info` was made on it. The length is queried as
    // WIC documents, with no buffer at all (windows-rs passes an empty slice as a dangling
    // pointer, which WIC treats as too small and refuses); the list is then read into a buffer
    // of that length.
    unsafe {
        (Interface::vtable(codec).GetFileExtensions)(
            Interface::as_raw(codec),
            0,
            PWSTR::null(),
            &mut length,
        )
        .ok()
        .ok()?;
        if length == 0 {
            return None;
        }
        let mut named = vec![0_u16; length as usize];
        codec.GetFileExtensions(&mut named, &mut length).ok()?;
        // ".heic,.heif,.avif", ending in a NUL.
        Some(PWSTR(named.as_mut_ptr()).to_string().unwrap_or_default())
    }
}

/// A decoder's extension list (".jpeg,.jpe,.jpg") as the drive names them.
fn parse_extensions(named: &str) -> impl Iterator<Item = String> + '_ {
    named
        .split(',')
        .map(|extension| {
            extension
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase()
        })
        .filter(|extension| !extension.is_empty())
}

/// Whether Media Foundation has a video decoder for `subtype` on this machine.
fn media_foundation_decodes(subtype: GUID) -> bool {
    if start().is_err() {
        return false;
    }
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype,
    };
    let mut found = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: Media Foundation hands back `count` activation objects in memory COM allocated;
    // each is released, then the memory freed.
    unsafe {
        let enumerated = MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT
                | MFT_ENUM_FLAG_ASYNCMFT
                | MFT_ENUM_FLAG_HARDWARE
                | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            None,
            &mut found,
            &mut count,
        );
        if found.is_null() {
            return false;
        }
        for index in 0..count as usize {
            drop(found.add(index).read());
        }
        CoTaskMemFree(Some(found.cast_const().cast()));
        enumerated.is_ok() && count > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decoders_extensions_are_named_as_the_drive_names_them() {
        assert_eq!(
            parse_extensions(".heic, .HEIF,.avif,").collect::<Vec<_>>(),
            ["heic", "heif", "avif"]
        );
    }

    #[test]
    fn the_formats_every_windows_decodes_are_decoded() {
        for extension in ["png", "jpg", "jpeg", "gif", "tif", "tiff", "bmp"] {
            assert!(decodes_extension(extension), "{extension}");
        }
        assert!(!decodes_extension("txt"));
    }

    #[test]
    fn a_picture_is_turned_scaled_and_kept_with_its_alpha() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("half.png");
        // 40 by 20: left half opaque red, right half half-transparent blue.
        let drawn = image::RgbaImage::from_fn(40, 20, |x, _| {
            if x < 20 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 128])
            }
        });
        drawn.save(&path).expect("picture");
        let path = path.to_string_lossy();
        assert_eq!(picture_dimensions(&path), Some((40, 20)));
        let still = picture(&path, 20).expect("decoded");
        assert_eq!((still.width, still.height), (20, 10));
        // Far from the seam, so the scaler blends neither corner with the other half.
        let near = |pixel: &[u8], expected: [u8; 4]| {
            pixel
                .iter()
                .zip(expected)
                .all(|(&byte, expected)| byte.abs_diff(expected) <= 1)
        };
        assert!(
            near(&still.bgra[..4], [0, 0, 255, 255]),
            "red, as blue-green-red-alpha"
        );
        let last = &still.bgra[still.bgra.len() - 4..];
        assert!(
            near(last, [255, 0, 0, 128]),
            "alpha not multiplied in: {last:?}"
        );
        assert_eq!(
            picture_dimensions(&folder.path().join("gone.png").to_string_lossy()),
            None
        );
    }
}
