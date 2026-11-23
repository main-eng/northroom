//! A video's still: one frame as a picture, for card and row thumbnails. It is made the way
//! AVFoundation's image generator makes one (`appliesPreferredTrackTransform`, a square
//! `maximumSize`): the decoder scales and converts the frame on output, then VideoToolbox turns
//! it per the track. MP4 and MOV stills match the generator's byte for byte, and Matroska
//! stills, which the generator can't read, are made the same way.

use super::av::{
    kCFBooleanTrue, kCVPixelBufferIOSurfacePropertiesKey, CFRelease, CFTypeRef, Cf, Transform,
};
use super::video::VideoReader;
use crate::media::{MediaError, MediaResult, VideoSource};
use northroom_media::pixel_buffer::{CVPixelBufferRef, PixelBuffer};
use std::ptr;

#[allow(non_upper_case_globals)]
#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    static kVTPixelRotationPropertyKey_Rotation: CFTypeRef;
    static kVTPixelRotationPropertyKey_FlipHorizontalOrientation: CFTypeRef;
    static kVTRotation_CW90: CFTypeRef;
    static kVTRotation_180: CFTypeRef;
    static kVTRotation_CCW90: CFTypeRef;
    fn VTCreateCGImageFromCVPixelBuffer(
        buffer: CFTypeRef,
        options: CFTypeRef,
        image_out: *mut CFTypeRef,
    ) -> i32;
    fn VTPixelRotationSessionCreate(allocator: CFTypeRef, session_out: *mut CFTypeRef) -> i32;
    fn VTPixelRotationSessionRotateImage(
        session: CFTypeRef,
        source: CFTypeRef,
        destination: CFTypeRef,
    ) -> i32;
    fn VTPixelRotationSessionInvalidate(session: CFTypeRef);
    fn VTSessionSetProperty(session: CFTypeRef, key: CFTypeRef, value: CFTypeRef) -> i32;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferCreate(
        allocator: CFTypeRef,
        width: usize,
        height: usize,
        format: u32,
        attributes: CFTypeRef,
        out: *mut CVPixelBufferRef,
    ) -> i32;
    fn CVBufferPropagateAttachments(source: CFTypeRef, destination: CFTypeRef);
}

/// A `CGImage`, released when dropped.
pub struct Still(CFTypeRef);

// SAFETY: a `CGImage` is immutable and Core Foundation's reference counting is thread-safe.
unsafe impl Send for Still {}

impl Still {
    /// Hands the image to the caller, who releases it.
    pub fn into_raw(self) -> CFTypeRef {
        let image = self.0;
        std::mem::forget(self);
        image
    }

    #[cfg(test)]
    pub(super) fn as_ptr(&self) -> CFTypeRef {
        self.0
    }

    /// Takes ownership of `image`, a +1 `CGImage`.
    #[cfg(test)]
    pub(super) fn from_raw(image: CFTypeRef) -> Self {
        Self(image)
    }
}

impl Drop for Still {
    fn drop(&mut self) {
        // SAFETY: the image is owned here.
        unsafe { CFRelease(self.0) };
    }
}

/// The frame shown at `seconds` of the video at `path`, no larger than `max_pixels` on either
/// side and turned for display. Errors if the file has no decodable video.
pub fn still(path: &str, seconds: f64, max_pixels: u32) -> MediaResult<Still> {
    let mut reader = VideoReader::open_picture(path, max_pixels.max(1))?;
    let frame = reader.frame_at(seconds)?;
    let picture = match Turn::of(reader.transform())? {
        Some(turn) => turn.apply(&frame.buffer)?,
        None => frame.buffer,
    };
    let mut image: CFTypeRef = ptr::null();
    // SAFETY: the pixel buffer is live; the image comes back +1 and keeps what it needs of it.
    let status = unsafe {
        VTCreateCGImageFromCVPixelBuffer(picture.as_ptr() as CFTypeRef, ptr::null(), &mut image)
    };
    if status != 0 || image.is_null() {
        return Err(MediaError(format!(
            "the frame could not be made a picture: VideoToolbox status {status}"
        )));
    }
    Ok(Still(image))
}

/// What a track's transform does to its pictures, as VideoToolbox's rotation session applies
/// it: a quarter-turn rotation, optionally mirrored.
struct Turn {
    /// `kVTRotation_*`, or `None` for no rotation.
    rotation: Option<CFTypeRef>,
    /// Mirrored horizontally after the rotation.
    mirrored: bool,
}

impl Turn {
    /// The turn `transform` makes, `None` for none. Errors for anything VideoToolbox can't do
    /// (anything but quarter turns, mirrored or not).
    fn of(transform: Transform) -> MediaResult<Option<Self>> {
        let near = |value: f64, wanted: f64| (value - wanted).abs() < 1e-6;
        // The transform maps (x, y) to (a x + c y, b x + d y). It mirrors when it flips the
        // picture over. The session rotates first and mirrors after, so the rotation is what
        // remains once the mirror is undone at the end (negating the result's x: a and c).
        let mirrored = transform.a * transform.d - transform.b * transform.c < 0.0;
        let (a, b) = if mirrored {
            (-transform.a, transform.b)
        } else {
            (transform.a, transform.b)
        };
        // SAFETY: the rotation constants are live for the life of the process.
        let rotation = unsafe {
            match (a, b) {
                (a, b) if near(a, 1.0) && near(b, 0.0) => None,
                // The y axis points down, so x turning towards +y is clockwise.
                (a, b) if near(a, 0.0) && near(b, 1.0) => Some(kVTRotation_CW90),
                (a, b) if near(a, -1.0) && near(b, 0.0) => Some(kVTRotation_180),
                (a, b) if near(a, 0.0) && near(b, -1.0) => Some(kVTRotation_CCW90),
                _ => {
                    return Err(MediaError(format!(
                        "the track turns its pictures by {transform:?}, not by quarter turns"
                    )))
                }
            }
        };
        Ok((rotation.is_some() || mirrored).then_some(Self { rotation, mirrored }))
    }

    /// `buffer` turned, into a new buffer of the same kind with its colour attachments.
    fn apply(&self, buffer: &PixelBuffer) -> MediaResult<PixelBuffer> {
        let (width, height) = buffer.size();
        let quarter = self.rotation.is_some_and(|rotation| {
            // SAFETY: the rotation constants are live for the life of the process.
            unsafe { rotation == kVTRotation_CW90 || rotation == kVTRotation_CCW90 }
        });
        let (width, height) = if quarter {
            (height, width)
        } else {
            (width, height)
        };
        let surface = Cf::dictionary(&[])?;
        // SAFETY: the framework constants are live; every object made here is released here or
        // owned by the buffer returned.
        unsafe {
            let attributes = Cf::dictionary(&[(kCVPixelBufferIOSurfacePropertiesKey, surface.0)])?;
            let mut turned: CVPixelBufferRef = ptr::null_mut();
            let status = CVPixelBufferCreate(
                ptr::null(),
                width as usize,
                height as usize,
                buffer.format_type(),
                attributes.0,
                &mut turned,
            );
            let turned = PixelBuffer::from_owned(turned)
                .filter(|_| status == 0)
                .ok_or_else(|| {
                    MediaError(format!(
                        "no buffer to turn the picture into: status {status}"
                    ))
                })?;
            let mut session: CFTypeRef = ptr::null();
            let status = VTPixelRotationSessionCreate(ptr::null(), &mut session);
            if status != 0 || session.is_null() {
                return Err(MediaError(format!(
                    "no rotation session: VideoToolbox status {status}"
                )));
            }
            if let Some(rotation) = self.rotation {
                VTSessionSetProperty(session, kVTPixelRotationPropertyKey_Rotation, rotation);
            }
            if self.mirrored {
                VTSessionSetProperty(
                    session,
                    kVTPixelRotationPropertyKey_FlipHorizontalOrientation,
                    kCFBooleanTrue,
                );
            }
            let status = VTPixelRotationSessionRotateImage(
                session,
                buffer.as_ptr() as CFTypeRef,
                turned.as_ptr() as CFTypeRef,
            );
            VTPixelRotationSessionInvalidate(session);
            CFRelease(session);
            if status != 0 {
                return Err(MediaError(format!(
                    "the picture could not be turned: VideoToolbox status {status}"
                )));
            }
            // The picture's colour space travels with it.
            CVBufferPropagateAttachments(
                buffer.as_ptr() as CFTypeRef,
                turned.as_ptr() as CFTypeRef,
            );
            Ok(turned)
        }
    }
}
