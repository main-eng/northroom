//! The Windows media backend. Media Foundation's Source Reader reads the container (MP4, MOV,
//! M4A, MP3, WAV, FLAC, ADTS, and MKV on Windows 10 and later) and decodes with the system's
//! decoders, in hardware where available. Frames come out as two-plane Y'CbCr at the stream's
//! own depth (NV12, or P010 when the decoder's output is deeper), with the video processor
//! converting any other decoder format. Sound comes out as 32-bit float at the file's own rate.
//! The core makes stills from frames (`media::planar`). WIC reads picture files (`picture`).

mod audio;
pub mod picture;
mod video;

pub use audio::AudioDecoder;
pub use video::VideoReader;

use super::colour::Tags;
use super::planar::{self, Still, Turn};
use super::{MediaError, MediaResult, Probe, VideoSource};
use northroom_media::color::{ChromaSiting, Matrix, Primaries, Transfer};
use std::sync::OnceLock;
use windows::core::{GUID, HSTRING};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::{VT_I8, VT_UI8};

/// Media Foundation's clock: 100 ns per tick.
const TICKS_PER_SECOND: f64 = 10_000_000.0;

const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const FIRST_AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const MEDIA_SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;

/// A Media Foundation failure as the core's error, naming what was being done.
fn failed(doing: &str) -> impl FnOnce(windows::core::Error) -> MediaError + '_ {
    move |error| MediaError(format!("{doing}: {error}"))
}

/// Starts Media Foundation for the process and COM for the calling thread. Source readers are
/// free-threaded, so the thread joins the multithreaded apartment; a thread already in another
/// apartment (a window's) keeps it, which works too.
fn start() -> MediaResult<()> {
    thread_local! {
        static COM: () = {
            // SAFETY: initialising COM on this thread, once; the result only reports whether it
            // already was.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        };
    }
    COM.with(|_| ());
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        // SAFETY: starting Media Foundation has no preconditions; it is never shut down.
        .get_or_init(|| {
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
                .map_err(|error| format!("Media Foundation did not start: {error}"))
        })
        .clone()
        .map_err(MediaError)
}

/// A Source Reader for the file at `path`, with every stream deselected. `process_video` makes
/// the reader convert and scale decoded frames to the requested type.
fn open_reader(path: &str, process_video: bool) -> MediaResult<IMFSourceReader> {
    start()?;
    // SAFETY: Media Foundation calls with live arguments; the attributes outlive the call.
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1).map_err(failed("make the reader's attributes"))?;
        let attributes = attributes.ok_or_else(|| MediaError("no reader attributes".into()))?;
        if process_video {
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
                .map_err(failed("ask for video processing"))?;
        }
        let reader =
            MFCreateSourceReaderFromURL(&HSTRING::from(plain_path(path).as_ref()), &attributes)
                .map_err(failed("open the file"))?;
        reader
            .SetStreamSelection(ALL_STREAMS, false)
            .map_err(failed("deselect the streams"))?;
        Ok(reader)
    }
}

/// `path` without the verbatim prefix of a canonical Windows path (`\\?\D:\...`,
/// `\\?\UNC\server\...`). Media Foundation reads a path as a URL and takes `\\?\` for a server
/// name.
fn plain_path(path: &str) -> std::borrow::Cow<'_, str> {
    if let Some(shared) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{shared}").into();
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).into()
}

/// Selects the first stream of `stream` (a `FIRST_*` index) and returns its stored type; `None`
/// if the file has no such stream.
fn select(reader: &IMFSourceReader, stream: u32) -> Option<IMFMediaType> {
    // SAFETY: a live reader; an absent stream is an error the reader returns.
    unsafe {
        reader.SetStreamSelection(stream, true).ok()?;
        reader.GetNativeMediaType(stream, 0).ok()
    }
}

/// The file's duration, if the source knows it.
fn duration_seconds(reader: &IMFSourceReader) -> Option<f64> {
    // SAFETY: a live reader; a duration is an unsigned 64-bit value, read as its type says.
    unsafe {
        let value = reader
            .GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION)
            .ok()?;
        let value = &value.Anonymous.Anonymous;
        (value.vt == VT_UI8)
            .then(|| value.Anonymous.uhVal as f64 / TICKS_PER_SECOND)
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
    }
}

/// Seeks `reader` to `seconds` on the file's clock.
fn set_position(reader: &IMFSourceReader, seconds: f64) -> MediaResult<()> {
    let mut position = PROPVARIANT::default();
    // SAFETY: a 64-bit integer position, the type a time position is given in; the reader
    // copies it.
    unsafe {
        let value = &mut position.Anonymous.Anonymous;
        value.vt = VT_I8;
        value.Anonymous.hVal = (seconds * TICKS_PER_SECOND).round() as i64;
        reader
            .SetCurrentPosition(&GUID::zeroed(), &position)
            .map_err(failed("seek"))
    }
}

/// One of a type's 32-bit attributes, if set.
fn number(kind: &IMFMediaType, key: &GUID) -> Option<u32> {
    // SAFETY: a live media type; an unset attribute is an error.
    unsafe { kind.GetUINT32(key).ok() }
}

/// Two 32-bit values packed in one of a type's attributes (a frame size or rate), if set.
fn pair(kind: &IMFMediaType, key: &GUID) -> Option<(u32, u32)> {
    // SAFETY: a live media type; an unset attribute is an error.
    let packed = unsafe { kind.GetUINT64(key).ok()? };
    Some(((packed >> 32) as u32, packed as u32))
}

/// Where a video type's picture sits in its buffers: the visible area's offset and size, and
/// the rows the buffers hold.
#[derive(Clone, Copy, Debug)]
struct Area {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    /// Rows of the luma plane in a buffer, visible ones plus any padding below.
    stored_height: u32,
}

impl Area {
    /// The visible area of `kind`'s pictures: its minimum display aperture if it has one (a
    /// 1080-line H.264 stream is coded 1088 lines tall), else the whole frame.
    fn of(kind: &IMFMediaType) -> MediaResult<Self> {
        let (stored_width, stored_height) = pair(kind, &MF_MT_FRAME_SIZE)
            .ok_or_else(|| MediaError("the video stream has no frame size".into()))?;
        let mut aperture = [0u8; std::mem::size_of::<MFVideoArea>()];
        // SAFETY: a live media type; the aperture is an `MFVideoArea` blob.
        let has_aperture = unsafe {
            kind.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut aperture, None)
                .is_ok()
        };
        let area = if has_aperture {
            // SAFETY: the blob holds one `MFVideoArea`, a plain C struct.
            let area: MFVideoArea = unsafe { std::ptr::read_unaligned(aperture.as_ptr().cast()) };
            Self {
                left: area.OffsetX.value.max(0) as u32,
                top: area.OffsetY.value.max(0) as u32,
                width: area.Area.cx.max(0) as u32,
                height: area.Area.cy.max(0) as u32,
                stored_height,
            }
        } else {
            Self {
                left: 0,
                top: 0,
                width: stored_width,
                height: stored_height,
                stored_height,
            }
        };
        if area.width == 0
            || area.height == 0
            || area.left + area.width > stored_width
            || area.top + area.height > stored_height
        {
            return Err(MediaError(format!("the video's picture area {area:?} is not inside its {stored_width} x {stored_height} frame")));
        }
        Ok(area)
    }
}

/// How a video type's pictures are turned for display. Media Foundation reports the stored
/// rotation counter-clockwise, so the picture is shown turned that much clockwise.
fn turn_of(kind: &IMFMediaType) -> MediaResult<Turn> {
    let degrees = number(kind, &MF_MT_VIDEO_ROTATION).unwrap_or(0);
    Turn::clockwise(degrees, false).ok_or_else(|| {
        MediaError(format!(
            "the video is turned by {degrees} degrees, not by quarter turns"
        ))
    })
}

/// The colour a video type's attributes describe.
// Matched by Media Foundation's own names for its values.
#[allow(non_upper_case_globals)]
fn tags_of(kind: &IMFMediaType) -> MediaResult<Tags> {
    let matrix =
        match number(kind, &MF_MT_YUV_MATRIX).map(|value| MFVideoTransferMatrix(value as i32)) {
            None | Some(MFVideoTransferMatrix_Unknown) => None,
            Some(MFVideoTransferMatrix_BT709) => Some(Matrix::Bt709),
            Some(MFVideoTransferMatrix_BT601) => Some(Matrix::Bt601),
            Some(MFVideoTransferMatrix_BT2020_10 | MFVideoTransferMatrix_BT2020_12) => {
                Some(Matrix::Bt2020)
            }
            Some(other) => {
                return Err(MediaError(format!(
                    "unsupported colour matrix: {}",
                    other.0
                )))
            }
        };
    let primaries = match number(kind, &MF_MT_VIDEO_PRIMARIES)
        .map(|value| MFVideoPrimaries(value as i32))
    {
        None | Some(MFVideoPrimaries_Unknown) => None,
        Some(MFVideoPrimaries_BT709) => Some(Primaries::Bt709),
        Some(MFVideoPrimaries_BT470_2_SysBG | MFVideoPrimaries_EBU3213) => {
            Some(Primaries::Bt601Pal)
        }
        Some(MFVideoPrimaries_SMPTE170M | MFVideoPrimaries_SMPTE_C) => Some(Primaries::Bt601Ntsc),
        Some(MFVideoPrimaries_BT2020) => Some(Primaries::Bt2020),
        Some(other) => {
            return Err(MediaError(format!(
                "unsupported colour primaries: {}",
                other.0
            )))
        }
    };
    let transfer = match number(kind, &MF_MT_TRANSFER_FUNCTION)
        .map(|value| MFVideoTransferFunction(value as i32))
    {
        None | Some(MFVideoTransFunc_Unknown) => None,
        // BT.601, BT.470BG and SDR BT.2020 share BT.709's curve, as on a Mac.
        Some(
            MFVideoTransFunc_709
            | MFVideoTransFunc_709_sym
            | MFVideoTransFunc_2020
            | MFVideoTransFunc_2020_const
            | MFVideoTransFunc_28,
        ) => Some(Transfer::Bt709),
        Some(MFVideoTransFunc_sRGB) => Some(Transfer::Srgb),
        Some(MFVideoTransFunc_2084) => Some(Transfer::Pq),
        Some(MFVideoTransFunc_HLG) => Some(Transfer::Hlg),
        Some(other) => {
            return Err(MediaError(format!(
                "unsupported transfer function: {}",
                other.0
            )))
        }
    };
    let full_range =
        match number(kind, &MF_MT_VIDEO_NOMINAL_RANGE).map(|value| MFNominalRange(value as i32)) {
            Some(MFNominalRange_0_255) => Some(true),
            Some(MFNominalRange_16_235) => Some(false),
            _ => None,
        };
    let chroma_siting = number(kind, &MF_MT_VIDEO_CHROMA_SITING)
        .filter(|siting| *siting != MFVideoChromaSubsampling_Unknown.0 as u32)
        .map(|siting| {
            let cosited = |flag: MFVideoChromaSubsampling| siting & flag.0 as u32 != 0;
            match (
                cosited(MFVideoChromaSubsampling_Horizontally_Cosited),
                cosited(MFVideoChromaSubsampling_Vertically_Cosited),
            ) {
                (true, true) => ChromaSiting::TopLeft,
                (true, false) => ChromaSiting::Left,
                _ => ChromaSiting::Center,
            }
        });
    Ok(Tags {
        matrix,
        primaries,
        transfer,
        full_range,
        chroma_siting,
    })
}

/// FFmpeg's short codec name for a Media Foundation subtype, matching the Mac's names; an
/// unknown subtype is named by its four characters.
fn codec_name(subtype: &GUID) -> String {
    let known: &[(GUID, &str)] = &[
        (MFVideoFormat_H264, "h264"),
        (MFVideoFormat_HEVC, "hevc"),
        (MFVideoFormat_VP90, "vp9"),
        (MFVideoFormat_VP80, "vp8"),
        (MFVideoFormat_AV1, "av1"),
        (MFVideoFormat_MJPG, "mjpeg"),
        (MFVideoFormat_MP4V, "mpeg4"),
        (MFAudioFormat_AAC, "aac"),
        (MFAudioFormat_MP3, "mp3"),
        (MFAudioFormat_FLAC, "flac"),
        (MFAudioFormat_Opus, "opus"),
        (MFAudioFormat_ALAC, "alac"),
        (MFAudioFormat_PCM, "pcm"),
        (MFAudioFormat_Float, "pcm"),
        (MFAudioFormat_Dolby_AC3, "ac3"),
        (MFAudioFormat_Dolby_DDPlus, "eac3"),
    ];
    known
        .iter()
        .find(|(known, _)| known == subtype)
        .map_or_else(
            || {
                String::from_utf8_lossy(&subtype.data1.to_le_bytes())
                    .trim_matches(|c: char| c.is_control() || c == ' ')
                    .to_owned()
            },
            |(_, name)| (*name).to_owned(),
        )
}

/// A video's displayed size (after rotation) and duration, or a sound's duration.
/// `Probe::default()` if Media Foundation can't read the file as media.
pub fn probe(path: &str) -> Probe {
    let Ok(reader) = open_reader(path, false) else {
        return Probe::default();
    };
    let length = duration_seconds(&reader);
    if let Some(video) = select(&reader, FIRST_VIDEO) {
        if let (Ok(area), Ok(turn)) = (Area::of(&video), turn_of(&video)) {
            return Probe {
                dimensions: Some(turn.shown_size(area.width, area.height)),
                duration_seconds: length,
            };
        }
    }
    let has_sound = select(&reader, FIRST_AUDIO).is_some();
    Probe {
        dimensions: None,
        duration_seconds: length.filter(|_| has_sound),
    }
}

/// The frame shown at `seconds` of the video at `path`, no larger than `max_pixels` on either
/// side and turned for display. Errors if the file has no decodable video.
pub fn still(path: &str, seconds: f64, max_pixels: u32) -> MediaResult<Still> {
    let mut reader = VideoReader::open(path)?;
    let turn = reader.turn();
    planar::still_of(&mut reader, turn, seconds, max_pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_paths_are_read_as_plain_ones() {
        assert_eq!(plain_path(r"\\?\D:\a\clip.mp4"), r"D:\a\clip.mp4");
        assert_eq!(
            plain_path(r"\\?\UNC\server\share\clip.mp4"),
            r"\\server\share\clip.mp4"
        );
        assert_eq!(plain_path(r"C:\clip.mp4"), r"C:\clip.mp4");
    }

    #[test]
    fn subtypes_are_named_as_on_a_mac() {
        assert_eq!(codec_name(&MFVideoFormat_H264), "h264");
        assert_eq!(codec_name(&MFAudioFormat_AAC), "aac");
        // A subtype with four characters it doesn't know.
        let mut odd = MFVideoFormat_H264;
        odd.data1 = u32::from_le_bytes(*b"wxyz");
        assert_eq!(codec_name(&odd), "wxyz");
    }
}
