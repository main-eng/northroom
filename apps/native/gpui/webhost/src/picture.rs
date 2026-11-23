//! The page host's picture format on every platform: premultiplied sRGB RGBA, top row first,
//! byte-identical for the same picture. Each platform builds it from a web view snapshot and
//! converts it to and from PNG (`crate::host`).

use crate::core::{self, HostError};
use northroom_core::page::host::ffi::NR_HOST_TRANSPARENT_FAILED;

#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// `width` × `height` × 4 bytes.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Picture({}×{})", self.width, self.height)
    }
}

impl Picture {
    /// Recovers the transparent picture from renders over black and over white
    /// (`host::unground`). Public web view APIs only draw a page over an opaque background.
    pub fn unground(black: &Self, white: &Self) -> Result<Self, HostError> {
        if (black.width, black.height) != (white.width, white.height) {
            return Err(HostError::of(NR_HOST_TRANSPARENT_FAILED, ""));
        }
        Ok(Self {
            width: black.width,
            height: black.height,
            rgba: core::unground(&black.rgba, &white.rgba),
        })
    }

    /// Encodes the picture as PNG.
    pub fn png(&self) -> Option<Vec<u8>> {
        crate::host::png(self)
    }

    /// Decodes a PNG, or any image format the platform reads, into the host's format.
    pub fn from_file(bytes: &[u8]) -> Option<Self> {
        crate::host::decode(bytes)
    }
}
