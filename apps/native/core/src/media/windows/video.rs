//! A video stream decoded by Media Foundation. The Source Reader decodes each frame with the
//! system's decoder, and its video processor converts the output to two-plane Y'CbCr at the
//! stream's own depth. Only the visible area is copied from the reader's buffers into the
//! core's `PixelBuffer`.

use super::{
    codec_name, duration_seconds, failed, number, open_reader, pair, select, set_position, tags_of,
    turn_of, Area, FIRST_VIDEO, TICKS_PER_SECOND,
};
use crate::media::colour::Tags;
use crate::media::planar::Turn;
use crate::media::{DecodedFrame, MediaError, MediaResult, VideoInfo, VideoSource};
use northroom_media::pixel_buffer::{PixelBuffer, PixelLayout, PlaneSource};
use windows::core::{Interface, GUID};
use windows::Win32::Media::MediaFoundation::*;

/// One video stream decoded in presentation order.
pub struct VideoReader {
    reader: IMFSourceReader,
    info: VideoInfo,
    layout: PixelLayout,
    /// Where frames sit in the reader's buffers, per its current type.
    area: Area,
    /// Bytes between luma rows when a buffer doesn't say.
    default_stride: usize,
    turn: Turn,
}

// SAFETY: a Source Reader is free-threaded (it lives in the multithreaded apartment, or the
// opening thread's, and marshals itself), and this reader is used from one thread at a time.
unsafe impl Send for VideoReader {}

/// Deeper decoder output formats; frames from these are read as 10-bit P010.
const DEEP_OUTPUTS: [GUID; 2] = [MFVideoFormat_P010, MFVideoFormat_P016];

impl VideoReader {
    /// How the stream's pictures are turned for display.
    pub(super) fn turn(&self) -> Turn {
        self.turn
    }

    /// Requests frames in `subtype` at the stream's visible size. The video processor converts
    /// when the decoder outputs another format, and crops when it codes a taller picture than
    /// it shows.
    fn ask_for(reader: &IMFSourceReader, subtype: &GUID, shown: Area) -> MediaResult<()> {
        // SAFETY: Media Foundation calls with live arguments.
        unsafe {
            let wanted = MFCreateMediaType().map_err(failed("make a media type"))?;
            let describe = || -> windows::core::Result<()> {
                wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                wanted.SetGUID(&MF_MT_SUBTYPE, subtype)?;
                wanted.SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    (u64::from(shown.width) << 32) | u64::from(shown.height),
                )
            };
            describe().map_err(failed("describe the frames"))?;
            reader
                .SetCurrentMediaType(FIRST_VIDEO, None, &wanted)
                .map_err(failed("set up the video decoder"))
        }
    }

    /// Whether the chosen decoder outputs more than 8 bits: its own first output choice, before
    /// the video processor converts it.
    fn decoder_is_deep(reader: &IMFSourceReader) -> bool {
        // SAFETY: Media Foundation calls with live arguments; the first transform of a stream
        // is its decoder.
        unsafe {
            let Ok(extended) = reader.cast::<IMFSourceReaderEx>() else {
                return false;
            };
            let mut decoder = None;
            if extended
                .GetTransformForStream(FIRST_VIDEO, 0, None, &mut decoder)
                .is_err()
            {
                return false;
            }
            decoder
                .and_then(|decoder| decoder.GetOutputAvailableType(0, 0).ok())
                .and_then(|kind| kind.GetGUID(&MF_MT_SUBTYPE).ok())
                .is_some_and(|subtype| DEEP_OUTPUTS.contains(&subtype))
        }
    }

    /// The colour the decoder's output reports once it has decoded a picture, then the reader's.
    fn decoder_tags(reader: &IMFSourceReader) -> MediaResult<Tags> {
        // SAFETY: Media Foundation calls with live arguments; the first transform of a stream
        // is its decoder.
        let decoded = unsafe {
            let mut decoder = None;
            reader
                .cast::<IMFSourceReaderEx>()
                .and_then(|extended| {
                    extended.GetTransformForStream(FIRST_VIDEO, 0, None, &mut decoder)
                })
                .ok()
                .and(decoder)
                .and_then(|decoder| decoder.GetOutputCurrentType(0).ok())
        };
        // SAFETY: a live reader.
        let current = unsafe { reader.GetCurrentMediaType(FIRST_VIDEO) }
            .map_err(failed("read the frames' type"))?;
        let decoded = decoded
            .map(|kind| tags_of(&kind))
            .transpose()?
            .unwrap_or_default();
        Ok(decoded.or(tags_of(&current)?))
    }

    /// Reads where frames sit from the reader's current type.
    fn read_area(&mut self) -> MediaResult<()> {
        // SAFETY: a live reader.
        let current = unsafe { self.reader.GetCurrentMediaType(FIRST_VIDEO) }
            .map_err(failed("read the frames' type"))?;
        self.area = Area::of(&current)?;
        self.default_stride = default_stride(&current, self.layout, self.area)?;
        if (self.area.width, self.area.height) != (self.info.width, self.info.height) {
            return Err(MediaError(format!(
                "the video's frames changed size from {} x {} to {} x {}",
                self.info.width, self.info.height, self.area.width, self.area.height
            )));
        }
        Ok(())
    }

    /// The visible area of `sample`'s picture copied out of its buffer.
    fn copy(&self, sample: &IMFSample) -> MediaResult<PixelBuffer> {
        // SAFETY: the buffer is locked while its bytes are read and unlocked after; the slices
        // cover only memory the lock returned.
        unsafe {
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(failed("read a frame"))?;
            if let Ok(two_dimensional) = buffer.cast::<IMF2DBuffer2>() {
                let (mut scanline, mut pitch, mut start, mut length) =
                    (std::ptr::null_mut(), 0, std::ptr::null_mut(), 0);
                two_dimensional
                    .Lock2DSize(
                        MF2DBuffer_LockFlags_Read,
                        &mut scanline,
                        &mut pitch,
                        &mut start,
                        &mut length,
                    )
                    .map_err(failed("lock a frame"))?;
                let bytes = std::slice::from_raw_parts(start, length as usize);
                let copied = usize::try_from(pitch)
                    .map_err(|_| {
                        MediaError(format!("a frame's rows run upwards ({pitch} bytes apart)"))
                    })
                    .and_then(|stride| {
                        self.planes(&bytes[scanline.offset_from(start) as usize..], stride)
                    });
                two_dimensional
                    .Unlock2D()
                    .map_err(failed("unlock a frame"))?;
                return copied;
            }
            let (mut start, mut length) = (std::ptr::null_mut(), 0);
            buffer
                .Lock(&mut start, None, Some(&mut length))
                .map_err(failed("lock a frame"))?;
            let copied = self.planes(
                std::slice::from_raw_parts(start, length as usize),
                self.default_stride,
            );
            buffer.Unlock().map_err(failed("unlock a frame"))?;
            copied
        }
    }

    /// The visible area of a frame whose luma rows start at `bytes`, `stride` apart, with the
    /// chroma plane after the luma plane's stored rows.
    fn planes(&self, bytes: &[u8], stride: usize) -> MediaResult<PixelBuffer> {
        let Area {
            left,
            top,
            width,
            height,
            stored_height,
        } = self.area;
        let sample_bytes = if self.layout.bit_depth() > 8 { 2 } else { 1 };
        let luma_start = top as usize * stride + left as usize * sample_bytes;
        // Chroma rows are as long as luma rows: half as many sample pairs.
        let chroma_start = stored_height as usize * stride
            + (top / 2) as usize * stride
            + (left / 2 * 2) as usize * sample_bytes;
        let plane = |start: usize| {
            bytes.get(start..).ok_or_else(|| {
                MediaError(format!(
                    "a frame of {} bytes has no plane at {start}",
                    bytes.len()
                ))
            })
        };
        PixelBuffer::copy_from(
            self.layout,
            width,
            height,
            PlaneSource {
                bytes: plane(luma_start)?,
                stride,
            },
            PlaneSource {
                bytes: plane(chroma_start)?,
                stride,
            },
        )
    }
}

/// Bytes between luma rows in `kind`'s buffers: its default stride if set, else its stored
/// width in samples.
fn default_stride(kind: &IMFMediaType, layout: PixelLayout, area: Area) -> MediaResult<usize> {
    if let Some(stride) = number(kind, &MF_MT_DEFAULT_STRIDE) {
        return usize::try_from(stride as i32).map_err(|_| {
            MediaError(format!(
                "the video's rows run upwards ({} bytes apart)",
                stride as i32
            ))
        });
    }
    let (stored_width, _) = pair(kind, &MF_MT_FRAME_SIZE)
        .ok_or_else(|| MediaError("the video stream has no frame size".into()))?;
    let sample_bytes = if layout.bit_depth() > 8 { 2 } else { 1 };
    Ok(stored_width.max(area.left + area.width) as usize * sample_bytes)
}

impl VideoSource for VideoReader {
    fn open(path: &str) -> MediaResult<Self> {
        let reader = open_reader(path, true)?;
        let stored = select(&reader, FIRST_VIDEO)
            .ok_or_else(|| MediaError("the file has no video stream".into()))?;
        let shown = Area::of(&stored)?;
        let (numerator, denominator) = pair(&stored, &MF_MT_FRAME_RATE).unwrap_or((0, 1));
        if numerator == 0 || denominator == 0 {
            return Err(MediaError("video stream has no frame rate".into()));
        }
        // SAFETY: a live media type.
        let codec =
            unsafe { stored.GetGUID(&MF_MT_SUBTYPE) }.map_err(failed("read the video's codec"))?;
        let turn = turn_of(&stored)?;
        Self::ask_for(&reader, &MFVideoFormat_NV12, shown)?;
        let deep = Self::decoder_is_deep(&reader);
        if deep {
            Self::ask_for(&reader, &MFVideoFormat_P010, shown)?;
        }
        // SAFETY: a live reader.
        let current = unsafe { reader.GetCurrentMediaType(FIRST_VIDEO) }
            .map_err(failed("read the frames' type"))?;
        // Container tags first, then what the decoder reports about the stream.
        let tags = tags_of(&stored)?.or(tags_of(&current)?);
        let full_range = tags.describe(shown.height).full_range;
        let layout = if deep {
            PixelLayout::P010 { full_range }
        } else {
            PixelLayout::Nv12 { full_range }
        };
        let info = VideoInfo {
            width: shown.width,
            height: shown.height,
            frame_rate: (numerator as i32, denominator as i32),
            duration_seconds: duration_seconds(&reader).unwrap_or(0.0),
            codec_name: codec_name(&codec),
            bit_depth: layout.bit_depth(),
            color: tags.describe(shown.height),
        };
        let complete = tags.matrix.is_some() && tags.primaries.is_some() && tags.transfer.is_some();
        let mut video = Self {
            reader,
            info,
            layout,
            area: shown,
            default_stride: 0,
            turn,
        };
        video.read_area()?;
        if !complete {
            // The decoder fills in what the container leaves out once it has read the first
            // picture, as on a Mac.
            video
                .next_frame()?
                .ok_or_else(|| MediaError("the video stream has no frames".into()))?;
            let tags = tags.or(Self::decoder_tags(&video.reader)?);
            let full_range = tags.describe(shown.height).full_range;
            video.layout = if deep {
                PixelLayout::P010 { full_range }
            } else {
                PixelLayout::Nv12 { full_range }
            };
            video.info.color = tags.describe(shown.height);
            video.seek(0.0)?;
        }
        Ok(video)
    }

    fn info(&self) -> &VideoInfo {
        &self.info
    }

    fn next_frame(&mut self) -> MediaResult<Option<DecodedFrame>> {
        super::start()?;
        loop {
            let (mut flags, mut time, mut sample) = (0u32, 0i64, None);
            // SAFETY: a live reader; it fills what it is given.
            unsafe {
                self.reader.ReadSample(
                    FIRST_VIDEO,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
            }
            .map_err(failed("decode a frame"))?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(MediaError("the video decoder failed".into()));
            }
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                self.read_area()?;
            }
            if let Some(sample) = sample {
                return Ok(Some(DecodedFrame {
                    buffer: self.copy(&sample)?,
                    seconds: time as f64 / TICKS_PER_SECOND,
                }));
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok(None);
            }
            // A gap in the stream (a tick with no sample): keep reading.
        }
    }

    fn seek(&mut self, seconds: f64) -> MediaResult<()> {
        super::start()?;
        // The reader starts at the key frame at or before `seconds`. Past the end, it starts at
        // the one before the last frame, so the last frame is still reachable.
        let last_frame =
            (self.info.duration_seconds - 1.0 / self.info.frames_per_second()).max(0.0);
        set_position(&self.reader, seconds.clamp(0.0, last_frame))
    }
}
