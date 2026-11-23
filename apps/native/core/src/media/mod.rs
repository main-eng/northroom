//! Media decoding through each platform's own frameworks (ADR-0004). No FFmpeg.
//!
//! The contract is the same on every platform: `VideoSource` gives frames in order or at a
//! time, `AudioDecoding` gives sound at the file's own rate, and `probe` and `still` read a
//! file's details and a thumbnail frame sized by `picture_size`.
//!
//! - `apple`: AVFoundation reads containers and decodes sound, VideoToolbox decodes video.
//!   Frames stay `CVPixelBuffer`s on their IOSurfaces and reach the GPU without a copy.
//! - `windows`: Media Foundation's Source Reader.
//! - `linux`: GStreamer.
//!
//! Off a Mac a frame is the decoder's two planes copied into memory and uploaded when drawn;
//! `planar` makes stills from them. Shared code lives here: sample-rate conversion
//! (`resample`) and its seek rules (`AudioReader`), so a film sounds the same on every
//! platform; colour tag defaults (`colour`); and a Matroska/WebM demuxer (`matroska`), which
//! the Mac needs because AVFoundation doesn't read Matroska.

use crate::Relock;
pub use northroom_media::color::ColorDescription;
use northroom_media::pixel_buffer::PixelLayout;
pub use northroom_media::{MediaError, MediaResult};

mod audio;
mod colour;
// Off a Mac only the headers are read (`matroska_holds_video`). On a Mac the demuxer feeds
// the system decoders.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod matroska;
// Built on a Mac for its tests.
#[cfg(any(not(target_os = "macos"), test))]
mod planar;
mod resample;

pub use audio::AudioReader;
#[cfg(not(target_os = "macos"))]
pub use planar::Still;

#[cfg(target_os = "macos")]
mod apple;
#[cfg(target_os = "macos")]
use apple::picture_dimensions;
#[cfg(target_os = "macos")]
use apple::AudioDecoder as PlatformAudio;
#[cfg(target_os = "macos")]
pub use apple::{probe, still, Frame, VideoReader};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use self::windows::AudioDecoder as PlatformAudio;
#[cfg(target_os = "windows")]
pub use self::windows::{probe, still, VideoReader};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::AudioDecoder as PlatformAudio;
#[cfg(target_os = "linux")]
pub use linux::{probe, still, VideoReader};

/// A decoded picture: a `CVPixelBuffer` on a Mac, its planes in memory elsewhere.
#[cfg(not(target_os = "macos"))]
pub type Frame = northroom_media::pixel_buffer::PixelBuffer;

// Picture size as shown: Image I/O on a Mac, WIC on Windows, and on Linux the reader the
// shell draws pictures with.
#[cfg(target_os = "windows")]
use self::windows::picture::picture_dimensions;
#[cfg(target_os = "windows")]
pub use self::windows::picture::{
    decodes_extension as wic_decodes_extension, picture as picture_thumbnail,
};
#[cfg(target_os = "linux")]
mod picture;
#[cfg(target_os = "linux")]
use picture::picture_dimensions;

/// Copies mono samples to each of `channels`, matching how the Mac decoder maps a single
/// channel (`AudioDecoding::open`). Other platforms call this for mono files so every platform
/// maps them alike; more channels are mixed down by the platform's own converter.
#[cfg(any(not(target_os = "macos"), test))]
fn spread_one_channel(samples: Vec<f32>, channels: u32) -> Vec<f32> {
    if channels == 1 {
        return samples;
    }
    samples
        .into_iter()
        .flat_map(|sample| std::iter::repeat_n(sample, channels as usize))
        .collect()
}

/// The still size for a video, no larger than `max` on either side. Keeps the stored size if
/// it fits; otherwise scales the long side to `max` and rounds down, as AVFoundation's image
/// generator does with `maximumSize`.
pub fn picture_size(width: u32, height: u32, max: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max {
        return (width, height);
    }
    let scaled = |side: u32| (u64::from(side) * u64::from(max) / u64::from(long)).max(1) as u32;
    (scaled(width), scaled(height))
}

/// A stream's nominal frame rate as numerator and denominator. NTSC's 1000/1001 rates come out
/// exact, whole rates as themselves, anything else to a thousandth. A Mac track states its
/// rate; Linux measures it between the first two frames (`linux::video`); Windows reads a ratio.
#[cfg(any(not(target_os = "windows"), test))]
fn frame_rate_of(nominal: f32) -> (i32, i32) {
    let rate = f64::from(nominal);
    if !(rate.is_finite() && rate > 0.0) {
        return (0, 1);
    }
    let whole = rate.round();
    if (rate - whole).abs() < 0.001 {
        return (whole as i32, 1);
    }
    let ntsc = (rate * 1.001).round();
    if (rate - ntsc / 1.001).abs() < 0.001 {
        return ((ntsc * 1000.0) as i32, 1001);
    }
    ((rate * 1000.0).round() as i32, 1000)
}

/// Per-file results cached until the file's size or modification time changes, so listing a
/// folder again reads no file twice.
struct PerFile<T> {
    known: std::sync::Mutex<
        std::collections::HashMap<std::path::PathBuf, (u64, std::time::SystemTime, T)>,
    >,
}

/// Entries a cache holds before it clears.
const FILES_KEPT: usize = 4096;

impl<T: Clone> PerFile<T> {
    fn new() -> Self {
        Self {
            known: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// `read` applied to `path`, once per version of the file; `None` if it can't be read.
    fn get(&self, path: &std::path::Path, read: impl FnOnce() -> T) -> Option<T> {
        let stat = std::fs::metadata(path).ok()?;
        let stamp = (stat.len(), stat.modified().ok()?);
        let lock = || self.known.relock();
        if let Some((length, modified, value)) = lock().get(path) {
            if (*length, *modified) == stamp {
                return Some(value.clone());
            }
        }
        // Read outside the lock so one slow file doesn't block others.
        let value = read();
        let mut known = lock();
        if known.len() >= FILES_KEPT {
            known.clear();
        }
        known.insert(path.to_path_buf(), (stamp.0, stamp.1, value.clone()));
        Some(value)
    }
}

/// Whether the Matroska or WebM file at `path` has an enabled video track. `None` if it isn't
/// Matroska or its headers can't be read. Reads only the headers, once per version of the file.
pub fn matroska_holds_video(path: &std::path::Path) -> Option<bool> {
    static KNOWN: std::sync::LazyLock<PerFile<Option<bool>>> =
        std::sync::LazyLock::new(PerFile::new);
    KNOWN
        .get(path, || matroska::holds_video(&path.to_string_lossy()))
        .flatten()
}

/// `probe`, once per version of the file.
pub fn probe_once(path: &std::path::Path) -> Probe {
    static KNOWN: std::sync::LazyLock<PerFile<Probe>> = std::sync::LazyLock::new(PerFile::new);
    KNOWN
        .get(path, || probe(&path.to_string_lossy()))
        .unwrap_or_default()
}

/// The size in pixels the picture at `path` is shown at, after orientation, read once per
/// version of the file. `None` if the platform can't read it as a picture.
pub fn picture_size_once(path: &std::path::Path) -> Option<(u32, u32)> {
    static KNOWN: std::sync::LazyLock<PerFile<Option<(u32, u32)>>> =
        std::sync::LazyLock::new(PerFile::new);
    KNOWN
        .get(path, || picture_dimensions(&path.to_string_lossy()))
        .flatten()
}

/// A video stream's properties.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// Numerator and denominator.
    pub frame_rate: (i32, i32),
    /// Duration of the file.
    pub duration_seconds: f64,
    /// FFmpeg's short codec name (`h264`, `hevc`, `prores`, `vp9`, `av1`), for messages.
    pub codec_name: String,
    /// Bits per luma sample in the coded stream.
    pub bit_depth: u32,
    pub color: ColorDescription,
}

impl VideoInfo {
    pub fn frames_per_second(&self) -> f64 {
        f64::from(self.frame_rate.0) / f64::from(self.frame_rate.1)
    }

    /// Sample layout of decoded frames: two planes of 8 bits, or 10 for deeper streams, in the
    /// stream's own range.
    pub fn layout(&self) -> PixelLayout {
        let full_range = self.color.full_range;
        if self.bit_depth > 8 {
            PixelLayout::P010 { full_range }
        } else {
            PixelLayout::Nv12 { full_range }
        }
    }
}

/// A decoded picture and its presentation time, on the file's clock.
pub struct DecodedFrame {
    pub buffer: Frame,
    pub seconds: f64,
}

/// A file's main video stream, decoded in presentation order.
pub trait VideoSource: Sized {
    /// Opens the file's first video stream; errors if there is none or it can't be decoded.
    fn open(path: &str) -> MediaResult<Self>;

    fn info(&self) -> &VideoInfo;

    /// The next frame in presentation order, or `None` at the end.
    fn next_frame(&mut self) -> MediaResult<Option<DecodedFrame>>;

    /// Seeks to the key frame at or before `seconds`; the frames decoded next lead up to it.
    fn seek(&mut self, seconds: f64) -> MediaResult<()>;

    /// The frame shown at `seconds`. Seeks, then decodes forward to the first frame within half
    /// a frame of it, so a time between two frames gets the nearer one.
    fn frame_at(&mut self, seconds: f64) -> MediaResult<DecodedFrame> {
        self.seek(seconds)?;
        let half_frame = 0.5 / self.info().frames_per_second();
        let mut latest = None;
        while let Some(frame) = self.next_frame()? {
            let reached = frame.seconds + half_frame >= seconds;
            latest = Some(frame);
            if reached {
                break;
            }
        }
        latest.ok_or_else(|| MediaError(format!("no frame at {seconds} s")))
    }
}

/// Interleaved 32-bit float audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSourceInfo {
    /// FFmpeg's short codec name (`aac`, `mp3`, `flac`, `opus`, `pcm_s16le`), for messages.
    pub codec_name: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub duration_seconds: f64,
}

/// A run of samples in the reader's output format.
pub struct AudioChunk {
    /// Interleaved samples: `channels` values per frame.
    pub samples: Vec<f32>,
    /// Start of the run, in output frames from the start of the file.
    pub start_frame: i64,
}

/// A platform's audio decoder: the file's first audio stream at its own rate, as interleaved
/// 32-bit float in a chosen channel count (mono copied to each channel, more mixed down).
/// `AudioReader` changes the rate.
pub trait AudioDecoding: Sized {
    /// Opens the file's first audio stream, or `None` if it has none.
    fn open(path: &str, channels: u32) -> MediaResult<Option<Self>>;

    fn source(&self) -> &AudioSourceInfo;

    /// The next run of samples and the source frame it starts at, or `None` at the end.
    fn read(&mut self) -> MediaResult<Option<(i64, Vec<f32>)>>;

    /// Seeks to exactly source frame `frame`.
    fn seek(&mut self, frame: u64) -> MediaResult<()>;
}

/// A media file's details: a video's displayed size and the file's duration. Anything that
/// can't be read is `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Probe {
    pub dimensions: Option<(u32, u32)>,
    pub duration_seconds: Option<f64>,
}

#[cfg(all(test, not(target_os = "macos")))]
mod platform_tests;

#[cfg(test)]
mod shared_tests {
    use super::frame_rate_of;

    #[test]
    fn nominal_rates_become_the_rationals_they_stand_for() {
        assert_eq!(frame_rate_of(30.0), (30, 1));
        assert_eq!(frame_rate_of(29.970_03), (30_000, 1001));
        assert_eq!(frame_rate_of(23.976), (24_000, 1001));
        assert_eq!(frame_rate_of(59.94), (60_000, 1001));
        assert_eq!(frame_rate_of(12.5), (12_500, 1000));
        assert_eq!(frame_rate_of(0.0), (0, 1));
    }

    #[test]
    fn one_channel_is_given_whole_to_each() {
        assert_eq!(
            super::spread_one_channel(vec![0.5, -0.25], 2),
            [0.5, 0.5, -0.25, -0.25]
        );
        assert_eq!(super::spread_one_channel(vec![0.5, -0.25], 1), [0.5, -0.25]);
    }
}

// Contract tests, run against each platform's reader.
#[cfg(test)]
mod picture_tests {
    use super::{picture_dimensions, picture_size_once};

    /// An uncompressed grey TIFF, `width` by `height`, with orientation tag `orientation`.
    fn tiff(width: u16, height: u16, orientation: u16) -> Vec<u8> {
        let pixels = usize::from(width) * usize::from(height);
        let entries: [(u16, u16, u32, u32); 10] = [
            (256, 3, 1, u32::from(width)),  // ImageWidth
            (257, 3, 1, u32::from(height)), // ImageLength
            (258, 3, 1, 8),                 // BitsPerSample
            (259, 3, 1, 1),                 // Compression: none
            (262, 3, 1, 1),                 // PhotometricInterpretation: black is zero
            (273, 4, 1, 0),                 // StripOffsets, set below
            (274, 3, 1, u32::from(orientation)),
            (277, 3, 1, 1),                 // SamplesPerPixel
            (278, 3, 1, u32::from(height)), // RowsPerStrip
            (279, 4, 1, pixels as u32),     // StripByteCounts
        ];
        let directory = 8;
        let data = directory + 2 + entries.len() * 12 + 4;
        let mut bytes = b"II*\0".to_vec();
        bytes.extend_from_slice(&(directory as u32).to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in entries {
            let value = if tag == 273 { data as u32 } else { value };
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(&count.to_le_bytes());
            if kind == 3 {
                bytes.extend_from_slice(&(value as u16).to_le_bytes());
                bytes.extend_from_slice(&[0, 0]);
            } else {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.resize(data + pixels, 128);
        bytes
    }

    #[test]
    fn a_picture_is_its_stored_size_turned_as_its_orientation_says() {
        let folder = tempfile::tempdir().expect("folder");
        let path = |name: &str| folder.path().join(name).to_string_lossy().into_owned();
        for (orientation, shown) in [
            (1, (30, 20)),
            (3, (30, 20)),
            (5, (20, 30)),
            (6, (20, 30)),
            (8, (20, 30)),
        ] {
            let name = format!("o{orientation}.tif");
            std::fs::write(path(&name), tiff(30, 20, orientation)).expect("picture");
            assert_eq!(
                picture_dimensions(&path(&name)),
                Some(shown),
                "orientation {orientation}"
            );
        }
        std::fs::write(path("not.png"), b"x").expect("file");
        assert_eq!(picture_dimensions(&path("not.png")), None);
        assert_eq!(picture_dimensions(&path("gone.png")), None);
    }

    #[test]
    fn a_picture_replaced_by_another_is_read_again() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("shot.tif");
        std::fs::write(&path, tiff(30, 20, 1)).expect("picture");
        assert_eq!(picture_size_once(&path), Some((30, 20)));
        std::fs::write(&path, tiff(12, 40, 1)).expect("picture");
        assert_eq!(picture_size_once(&path), Some((12, 40)));
    }
}
