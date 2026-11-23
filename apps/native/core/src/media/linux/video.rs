//! A video stream decoded by GStreamer. `playbin` decodes with the system's decoder,
//! `videoconvert` hands frames over as two-plane Y'CbCr at the stream's own depth (NV12 for
//! 8-bit, P010 for deeper, repacking planes only), and each frame is copied into the core's
//! `PixelBuffer`.

use super::{codec_name, discover, turn_of, Playback};
use crate::media::colour::Tags;
use crate::media::planar::Turn;
use crate::media::{frame_rate_of, DecodedFrame, MediaError, MediaResult, VideoInfo, VideoSource};
use gstreamer as gst;
use gstreamer_pbutils::prelude::*;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;
use northroom_media::color::{ChromaSiting, Matrix, Primaries, Transfer};
use northroom_media::pixel_buffer::{PixelBuffer, PixelLayout, PlaneSource};

/// The plane format frames are requested in; either, depending on the stream's depth.
const FORMATS: [&str; 2] = ["NV12", "P010_10LE"];

/// One video stream decoded in presentation order.
pub struct VideoReader {
    playback: Playback,
    /// Frames read ahead to measure the rate, returned before any others.
    ahead: std::collections::VecDeque<(gst::Sample, f64)>,
    info: VideoInfo,
    layout: PixelLayout,
    turn: Turn,
}

impl VideoReader {
    /// How the stream's pictures are turned for display.
    pub(super) fn turn(&self) -> Turn {
        self.turn
    }

    /// `sample`'s picture copied out of its buffer.
    fn copy(&self, sample: &gst::Sample) -> MediaResult<PixelBuffer> {
        let caps = sample
            .caps()
            .ok_or_else(|| MediaError("a frame has no format".into()))?;
        let described = gst_video::VideoInfo::from_caps(caps)
            .map_err(|error| MediaError(format!("a frame's format: {error}")))?;
        let buffer = sample
            .buffer()
            .ok_or_else(|| MediaError("a frame has no picture".into()))?;
        let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &described)
            .map_err(|_| MediaError("a frame could not be read".into()))?;
        let plane = |index: u32| -> MediaResult<PlaneSource<'_>> {
            let bytes = frame
                .plane_data(index)
                .map_err(|error| MediaError(format!("a frame's plane {index}: {error}")))?;
            let stride = usize::try_from(frame.plane_stride()[index as usize])
                .map_err(|_| MediaError("a frame's rows run upwards".into()))?;
            Ok(PlaneSource { bytes, stride })
        };
        PixelBuffer::copy_from(
            self.layout,
            described.width(),
            described.height(),
            plane(0)?,
            plane(1)?,
        )
    }
}

/// The next decoded frame and its presentation time, or `None` at end of stream. Frames outside
/// the segment (before a seek's key frame) have no clock time and are skipped.
fn timed_sample(playback: &Playback) -> MediaResult<Option<(gst::Sample, f64)>> {
    loop {
        let Some(sample) = playback.next()? else {
            return Ok(None);
        };
        if let Some(seconds) = Playback::seconds_of(&sample) {
            return Ok(Some((sample, seconds)));
        }
    }
}

/// The colour a decoded stream's caps describe.
fn tags_of(
    colorimetry: gst_video::VideoColorimetry,
    chroma: gst_video::VideoChromaSite,
) -> MediaResult<Tags> {
    use gst_video::{
        VideoColorMatrix, VideoColorPrimaries, VideoColorRange, VideoTransferFunction,
    };
    let matrix = match colorimetry.matrix() {
        VideoColorMatrix::Unknown => None,
        VideoColorMatrix::Bt709 => Some(Matrix::Bt709),
        VideoColorMatrix::Bt601 => Some(Matrix::Bt601),
        VideoColorMatrix::Bt2020 => Some(Matrix::Bt2020),
        other => return Err(MediaError(format!("unsupported colour matrix: {other:?}"))),
    };
    let primaries = match colorimetry.primaries() {
        VideoColorPrimaries::Unknown => None,
        VideoColorPrimaries::Bt709 => Some(Primaries::Bt709),
        VideoColorPrimaries::Bt470bg | VideoColorPrimaries::Ebu3213 => Some(Primaries::Bt601Pal),
        VideoColorPrimaries::Smpte170m | VideoColorPrimaries::Smpte240m => {
            Some(Primaries::Bt601Ntsc)
        }
        VideoColorPrimaries::Bt2020 => Some(Primaries::Bt2020),
        VideoColorPrimaries::Smpteeg432 => Some(Primaries::DisplayP3),
        other => {
            return Err(MediaError(format!(
                "unsupported colour primaries: {other:?}"
            )))
        }
    };
    let transfer = match colorimetry.transfer() {
        VideoTransferFunction::Unknown => None,
        // BT.601, BT.470BG and SDR BT.2020 share BT.709's curve, as on a Mac.
        VideoTransferFunction::Bt709
        | VideoTransferFunction::Bt601
        | VideoTransferFunction::Bt202010
        | VideoTransferFunction::Bt202012
        | VideoTransferFunction::Gamma28 => Some(Transfer::Bt709),
        VideoTransferFunction::Srgb => Some(Transfer::Srgb),
        VideoTransferFunction::Smpte2084 => Some(Transfer::Pq),
        VideoTransferFunction::AribStdB67 => Some(Transfer::Hlg),
        other => {
            return Err(MediaError(format!(
                "unsupported transfer function: {other:?}"
            )))
        }
    };
    let full_range = match colorimetry.range() {
        VideoColorRange::Range0_255 => Some(true),
        VideoColorRange::Range16_235 => Some(false),
        _ => None,
    };
    let chroma_siting = (!chroma.is_empty()).then(|| {
        match (
            chroma.contains(gst_video::VideoChromaSite::H_COSITED),
            chroma.contains(gst_video::VideoChromaSite::V_COSITED),
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

impl VideoSource for VideoReader {
    fn open(path: &str) -> MediaResult<Self> {
        let discovered = discover(path)?;
        let stream = discovered
            .video_streams()
            .into_iter()
            .find(|stream| !stream.is_image())
            .ok_or_else(|| MediaError("the file has no video stream".into()))?;
        let turn = turn_of(stream.tags().as_ref())?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", gst::List::new(FORMATS))
            .build();
        let playback = Playback::open(path, true, &caps)?;
        let decoded = gst_video::VideoInfo::from_caps(playback.caps()?.as_ref())
            .map_err(|error| MediaError(format!("the frames' format: {error}")))?;
        let tags = tags_of(decoded.colorimetry(), decoded.chroma_site())?;
        let full_range = tags.describe(decoded.height()).full_range;
        let layout = match decoded.format() {
            gst_video::VideoFormat::Nv12 => PixelLayout::Nv12 { full_range },
            gst_video::VideoFormat::P01010le => PixelLayout::P010 { full_range },
            other => return Err(MediaError(format!("frames came as {other:?}"))),
        };
        // Measure the rate from the first two frames, as a Mac track gives its nominal rate.
        // A demuxer's caps can give an average (an MP4 with B-frames reads 59/2 for 30).
        let mut ahead = std::collections::VecDeque::new();
        while ahead.len() < 2 {
            match timed_sample(&playback)? {
                Some(timed) => ahead.push_back(timed),
                None => break,
            }
        }
        let measured = match (ahead.front(), ahead.back()) {
            (Some((_, first)), Some((_, second))) if second > first => {
                frame_rate_of((1.0 / (second - first)) as f32)
            }
            _ => (0, 1),
        };
        let stated = Some(decoded.fps())
            .filter(|rate| rate.numer() > 0)
            .unwrap_or(stream.framerate());
        let rate = if measured.0 > 0 {
            measured
        } else {
            (stated.numer(), stated.denom())
        };
        if rate.0 <= 0 || rate.1 <= 0 {
            return Err(MediaError("video stream has no frame rate".into()));
        }
        let length = discovered
            .duration()
            .or_else(|| playback.playbin.query_duration::<gst::ClockTime>());
        let info = VideoInfo {
            width: decoded.width(),
            height: decoded.height(),
            frame_rate: rate,
            duration_seconds: length.map_or(0.0, |length| length.nseconds() as f64 / 1e9),
            codec_name: codec_name(stream.caps().as_deref()),
            bit_depth: layout.bit_depth(),
            color: tags.describe(decoded.height()),
        };
        Ok(Self {
            playback,
            ahead,
            info,
            layout,
            turn,
        })
    }

    fn info(&self) -> &VideoInfo {
        &self.info
    }

    fn next_frame(&mut self) -> MediaResult<Option<DecodedFrame>> {
        let next = match self.ahead.pop_front() {
            Some(timed) => Some(timed),
            None => timed_sample(&self.playback)?,
        };
        next.map(|(sample, seconds)| {
            Ok(DecodedFrame {
                buffer: self.copy(&sample)?,
                seconds,
            })
        })
        .transpose()
    }

    fn seek(&mut self, seconds: f64) -> MediaResult<()> {
        // The key frame at or before `seconds`. Past the end, the one before the last frame,
        // so the last frame is still reachable.
        let last_frame =
            (self.info.duration_seconds - 1.0 / self.info.frames_per_second()).max(0.0);
        self.ahead.clear();
        self.playback.seek(
            seconds.clamp(0.0, last_frame),
            gst::SeekFlags::KEY_UNIT | gst::SeekFlags::SNAP_BEFORE,
        )
    }
}
