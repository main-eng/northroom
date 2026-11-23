//! The Linux media backend: GStreamer, linked as a system library and never bundled. Its
//! discoverer reads a file's details. `playbin` reads the file and decodes it with whatever
//! decoders are installed (hardware ones through VA-API where present), and an app sink takes
//! frames as two-plane Y'CbCr at the stream's own depth (NV12, or P010 for deeper streams) and
//! sound as 32-bit float at the file's own rate. The core makes stills from frames
//! (`media::planar`).

mod audio;
mod video;

pub use audio::AudioDecoder;
pub use video::VideoReader;

use super::planar::{self, Still, Turn};
use super::{MediaError, MediaResult, Probe, VideoSource};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_pbutils::prelude::*;
use gstreamer_pbutils::{Discoverer, DiscovererInfo};
use std::sync::OnceLock;

/// How long the discoverer or a pipeline may take to read a file's start.
const READ_WAIT: gst::ClockTime = gst::ClockTime::from_seconds(20);
/// Decoded frames or sound runs the app sink holds before the decoder waits for the reader.
const HELD: u32 = 4;
/// How often a reader waiting for the next sample checks the pipeline for errors. An app
/// sink's wait ends at a sample or at end of stream, never at an error, and an error stops the
/// stream with neither.
const SAMPLE_LOOK: gst::ClockTime = gst::ClockTime::from_mseconds(100);

/// Starts GStreamer for the process.
fn start() -> MediaResult<()> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| gst::init().map_err(|error| format!("GStreamer did not start: {error}")))
        .clone()
        .map_err(MediaError)
}

/// A file's URI for GStreamer.
fn uri_of(path: &str) -> MediaResult<String> {
    let absolute =
        std::path::absolute(path).map_err(|error| MediaError(format!("{path}: {error}")))?;
    gst::glib::filename_to_uri(absolute, None)
        .map(Into::into)
        .map_err(|error| MediaError(format!("{path}: {error}")))
}

/// The details of the file at `path`.
fn discover(path: &str) -> MediaResult<DiscovererInfo> {
    start()?;
    let discoverer = Discoverer::new(READ_WAIT)
        .map_err(|error| MediaError(format!("no discoverer: {error}")))?;
    discoverer
        .discover_uri(&uri_of(path)?)
        .map_err(|error| MediaError(format!("{path}: {error}")))
}

/// FFmpeg's short codec name for compressed `caps`, matching the Mac's names.
fn codec_name(caps: Option<&gst::CapsRef>) -> String {
    let Some(structure) = caps.and_then(|caps| caps.structure(0)) else {
        return String::new();
    };
    let version = |field| structure.get::<i32>(field).ok();
    match structure.name().as_str() {
        "video/x-h264" => "h264",
        "video/x-h265" => "hevc",
        "video/x-vp9" => "vp9",
        "video/x-vp8" => "vp8",
        "video/x-av1" => "av1",
        "video/x-prores" => "prores",
        "image/jpeg" => "mjpeg",
        "video/mpeg" if version("mpegversion") == Some(4) => "mpeg4",
        "audio/mpeg" if version("mpegversion") == Some(1) => "mp3",
        "audio/mpeg" => "aac",
        "audio/x-flac" => "flac",
        "audio/x-opus" => "opus",
        "audio/x-vorbis" => "vorbis",
        "audio/x-alac" => "alac",
        "audio/x-raw" => "pcm",
        "audio/x-ac3" => "ac3",
        "audio/x-eac3" => "eac3",
        other => {
            return other
                .rsplit('/')
                .next()
                .unwrap_or(other)
                .trim_start_matches("x-")
                .to_owned()
        }
    }
    .to_owned()
}

/// The turn for GStreamer's `image-orientation` tag. `rotate-90` means turn a quarter clockwise
/// for display; `flip-rotate-90` means mirror, then turn.
fn turn_of_orientation(orientation: &str) -> MediaResult<Turn> {
    let (mirrored, degrees) = match orientation.strip_prefix("flip-rotate-") {
        Some(degrees) => (true, degrees),
        None => (
            false,
            orientation.strip_prefix("rotate-").unwrap_or(orientation),
        ),
    };
    let unknown = || {
        MediaError(format!(
            "the video is turned as {orientation:?}, not by quarter turns"
        ))
    };
    let degrees: u32 = degrees.parse().map_err(|_| unknown())?;
    // Mirroring then turning equals turning back as far, then mirroring.
    let degrees = if mirrored {
        (360 - degrees % 360) % 360
    } else {
        degrees
    };
    Turn::clockwise(degrees, mirrored).ok_or_else(unknown)
}

/// How a stream with `tags` is turned for display.
fn turn_of(tags: Option<&gst::TagList>) -> MediaResult<Turn> {
    match tags.and_then(|tags| tags.get::<gst::tags::ImageOrientation>()) {
        Some(orientation) => turn_of_orientation(orientation.get()),
        None => Ok(Turn::default()),
    }
}

/// A file read by `playbin` into an app sink: its video (`video`) or its sound, through
/// `convert` into what `caps` asks for.
struct Playback {
    playbin: gst::Element,
    sink: AppSink,
}

impl Playback {
    fn open(path: &str, video: bool, caps: &gst::Caps) -> MediaResult<Self> {
        start()?;
        let built = |error: gst::glib::BoolError| {
            MediaError(format!("the pipeline could not be built: {error}"))
        };
        let make = |name: &str| {
            gst::ElementFactory::make(name)
                .build()
                .map_err(|error| MediaError(format!("no {name}: {error}")))
        };
        let playbin = make("playbin")?;
        playbin.set_property("uri", uri_of(path)?);
        // Only one kind of stream, through this sink alone, with none of playbin's own conversion.
        playbin.set_property_from_str(
            "flags",
            if video {
                "video+native-video"
            } else {
                "audio+native-audio"
            },
        );
        let convert = make(if video {
            "videoconvert"
        } else {
            "audioconvert"
        })?;
        if video {
            // Repack planes only. Deeper streams pass through 16 bits per sample, and the
            // converter would dither back down to 10 bits, changing samples.
            convert.set_property_from_str("dither", "none");
        }
        let sink = AppSink::builder()
            .caps(caps)
            .sync(false)
            .max_buffers(HELD)
            .build();
        let bin = gst::Bin::new();
        bin.add_many([&convert, sink.upcast_ref()]).map_err(built)?;
        convert.link(&sink).map_err(built)?;
        let input = convert
            .static_pad("sink")
            .ok_or_else(|| MediaError("the converter has no input".into()))?;
        bin.add_pad(&gst::GhostPad::with_target(&input).map_err(built)?)
            .map_err(built)?;
        playbin.set_property(if video { "video-sink" } else { "audio-sink" }, &bin);
        // Send the other kind of stream nowhere. Left unset, playbin tries the system's sinks
        // while plugging decoders, and one that waits on a desktop main loop (GTK's Wayland
        // sink) never answers: the file never starts and stopping it hangs.
        playbin.set_property(
            if video { "audio-sink" } else { "video-sink" },
            &make("fakesink")?,
        );
        let playback = Self { playbin, sink };
        playback
            .playbin
            .set_state(gst::State::Playing)
            .map_err(|_| playback.failure("the file could not be played"))?;
        let bus = playback.bus()?;
        match bus.timed_pop_filtered(
            READ_WAIT,
            &[gst::MessageType::AsyncDone, gst::MessageType::Error],
        ) {
            Some(message) if message.type_() == gst::MessageType::AsyncDone => Ok(playback),
            Some(message) => Err(Self::error_of(&message)),
            None => Err(MediaError(format!(
                "{path} did not start playing in {READ_WAIT}"
            ))),
        }
    }

    fn bus(&self) -> MediaResult<gst::Bus> {
        self.playbin
            .bus()
            .ok_or_else(|| MediaError("the pipeline has no bus".into()))
    }

    fn error_of(message: &gst::Message) -> MediaError {
        match message.view() {
            gst::MessageView::Error(error) => MediaError(format!(
                "{} ({})",
                error.error(),
                error.debug().unwrap_or_default()
            )),
            _ => MediaError("the pipeline stopped".into()),
        }
    }

    /// The pipeline's own error if it posted one, else a failure of `doing`.
    fn failure(&self, doing: &str) -> MediaError {
        self.playbin
            .bus()
            .and_then(|bus| bus.pop_filtered(&[gst::MessageType::Error]))
            .map_or_else(
                || MediaError(doing.to_owned()),
                |message| Self::error_of(&message),
            )
    }

    /// The next decoded sample, or `None` at end of stream. Returns the pipeline's error if it
    /// stopped on one, or a timeout error after `READ_WAIT` with no sample.
    fn next(&self) -> MediaResult<Option<gst::Sample>> {
        let bus = self.bus()?;
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_nanos(READ_WAIT.nseconds());
        loop {
            if let Some(sample) = self.sink.try_pull_sample(SAMPLE_LOOK) {
                return Ok(Some(sample));
            }
            if self.sink.is_eos() {
                return Ok(None);
            }
            if let Some(message) = bus.pop_filtered(&[gst::MessageType::Error]) {
                return Err(Self::error_of(&message));
            }
            if std::time::Instant::now() >= deadline {
                return Err(MediaError(format!(
                    "decoding stalled: nothing came in {READ_WAIT}"
                )));
            }
        }
    }

    /// The caps the sink negotiated.
    fn caps(&self) -> MediaResult<gst::Caps> {
        let input = self
            .sink
            .static_pad("sink")
            .ok_or_else(|| MediaError("the sink has no input".into()))?;
        input
            .current_caps()
            .ok_or_else(|| MediaError("the stream was not set up".into()))
    }

    /// Where `sample` starts on the file's clock.
    fn seconds_of(sample: &gst::Sample) -> Option<f64> {
        let start = sample.buffer()?.pts()?;
        let segment = sample.segment()?.downcast_ref::<gst::ClockTime>()?;
        let stream = segment.to_stream_time(start)?;
        Some(stream.nseconds() as f64 / 1e9)
    }

    /// Seeks to `seconds`: to the key frame at or before it with `KEY_UNIT`, exactly with
    /// `ACCURATE`.
    fn seek(&self, seconds: f64, flags: gst::SeekFlags) -> MediaResult<()> {
        let position = gst::ClockTime::from_nseconds((seconds.max(0.0) * 1e9) as u64);
        self.playbin
            .seek_simple(gst::SeekFlags::FLUSH | flags, position)
            .map_err(|_| self.failure("the file could not be seeked"))
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        // Stopping frees the pipeline's threads; nothing can be done if it fails.
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

/// A video's displayed size (after rotation) and duration, or a sound's duration.
/// `Probe::default()` if GStreamer can't read the file as media.
pub fn probe(path: &str) -> Probe {
    let Ok(info) = discover(path) else {
        return Probe::default();
    };
    let length = info
        .duration()
        .map(|length| length.nseconds() as f64 / 1e9)
        .filter(|seconds| *seconds > 0.0);
    if let Some(video) = info
        .video_streams()
        .into_iter()
        .find(|video| !video.is_image())
    {
        if let Ok(turn) = turn_of(video.tags().as_ref()) {
            if video.width() > 0 && video.height() > 0 {
                return Probe {
                    dimensions: Some(turn.shown_size(video.width(), video.height())),
                    duration_seconds: length,
                };
            }
        }
    }
    let has_sound = !info.audio_streams().is_empty();
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
    fn orientations_turn_as_gstreamer_names_them() {
        let turn = |orientation| turn_of_orientation(orientation).expect(orientation);
        assert_eq!(turn("rotate-0"), Turn::default());
        assert_eq!(
            turn("rotate-90"),
            Turn {
                quarter_turns: 1,
                mirrored: false
            }
        );
        assert_eq!(
            turn("rotate-270"),
            Turn {
                quarter_turns: 3,
                mirrored: false
            }
        );
        assert_eq!(
            turn("flip-rotate-0"),
            Turn {
                quarter_turns: 0,
                mirrored: true
            }
        );
        assert_eq!(
            turn("flip-rotate-90"),
            Turn {
                quarter_turns: 3,
                mirrored: true
            }
        );
        assert_eq!(
            turn("flip-rotate-180"),
            Turn {
                quarter_turns: 2,
                mirrored: true
            }
        );
        assert!(turn_of_orientation("rotate-45").is_err());
        assert!(turn_of_orientation("sideways").is_err());
    }

    #[test]
    fn codecs_are_named_as_on_a_mac() {
        start().expect("GStreamer");
        let name = |caps: &str| codec_name(Some(&caps.parse::<gst::Caps>().expect(caps)));
        assert_eq!(name("video/x-h264"), "h264");
        assert_eq!(name("video/x-h265"), "hevc");
        assert_eq!(name("audio/mpeg, mpegversion=(int)4"), "aac");
        assert_eq!(name("audio/mpeg, mpegversion=(int)1, layer=(int)3"), "mp3");
        assert_eq!(name("video/x-theora"), "theora");
    }
}
