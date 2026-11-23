//! Native media for Northroom.
//!
//! `color` interprets a stream's colour tags. `pixel_buffer` is a decoded frame: an
//! IOSurface-backed `CVPixelBuffer` on a Mac, the decoder's planes in memory elsewhere. `gpu`
//! wraps a frame as wgpu textures (zero-copy on a Mac), converts it into the working space, and
//! writes the working texture into an encoder's input planes.
//!
//! The app's core reads and writes media files (`apps/native/core/src/media`).

pub mod color;
pub mod gpu;
pub mod pixel_buffer;

use std::fmt;

#[derive(Debug)]
pub struct MediaError(pub String);

impl fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MediaError {}

pub type MediaResult<T> = Result<T, MediaError>;

/// A four-character code (`'avc1'`) in the numeric form Core Media, Core Video and Core Audio use.
pub(crate) const fn four_character_code(code: &[u8; 4]) -> u32 {
    ((code[0] as u32) << 24) | ((code[1] as u32) << 16) | ((code[2] as u32) << 8) | code[3] as u32
}

pub(crate) fn media_error<T>(message: impl Into<String>) -> MediaResult<T> {
    Err(MediaError(message.into()))
}
