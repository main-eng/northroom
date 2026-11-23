//! The Mac media backend. AVFoundation reads containers (MP4, MOV, M4A, MP3, WAV, AIFF, FLAC,
//! CAF, Ogg) and decodes sound; VideoToolbox decodes video (H.264, HEVC, ProRes, and VP9 and
//! AV1 where the Mac has a decoder). Matroska and WebM go through the core's demuxer
//! (`media::matroska`), with video decoded by VideoToolbox and sound by Core Audio converters.
//! A still is the same decoder's frame, scaled and converted on output.

mod audio;
mod av;
mod converter;
mod format;
mod matroska;
#[cfg(test)]
mod parity_tests;
mod picture;
mod still;
#[cfg(test)]
mod still_tests;
mod video;

pub use audio::AudioDecoder;
pub use picture::picture_dimensions;
pub use still::still;
pub use video::VideoReader;

use super::Probe;
use av::{Asset, CMVideoFormatDescriptionGetDimensions, TrackKind};

/// A decoded picture: a `CVPixelBuffer` on its IOSurface.
pub type Frame = northroom_media::pixel_buffer::PixelBuffer;

/// A video's displayed size (after rotation) and duration, or a sound's duration.
/// `Probe::default()` if the Mac can't read the file as media.
pub fn probe(path: &str) -> Probe {
    if crate::media::matroska::is_matroska(path) {
        return matroska::probe(path);
    }
    let Ok(asset) = Asset::open(path) else {
        return Probe::default();
    };
    let length = asset
        .duration_seconds()
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0);
    if let Some(track) = asset.first_track(TrackKind::Video) {
        if let Some(format) = track.format_description() {
            // SAFETY: `format` is a live video format description.
            let dimensions = unsafe { CMVideoFormatDescriptionGetDimensions(format.0) };
            if dimensions.width > 0 && dimensions.height > 0 {
                let shown = track
                    .preferred_transform()
                    .shown_size(dimensions.width as u32, dimensions.height as u32);
                return Probe {
                    dimensions: Some(shown),
                    duration_seconds: length,
                };
            }
        }
    }
    let has_sound = asset.first_track(TrackKind::Audio).is_some();
    Probe {
        dimensions: None,
        duration_seconds: length.filter(|_| has_sound),
    }
}
