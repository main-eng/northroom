//! Sound decoded by GStreamer to interleaved 32-bit float at the file's own rate
//! (`audioconvert` changes sample format and channels, never the rate). Mono is read as is and
//! copied to each output channel here (`media::spread_one_channel`); more channels are read in
//! the output's count, which `audioconvert` mixes down. The core changes the rate
//! (`media::resample`).

use super::{codec_name, discover, Playback};
use crate::media::{spread_one_channel, AudioDecoding, AudioSourceInfo, MediaError, MediaResult};
use gstreamer as gst;
use gstreamer_pbutils::prelude::*;

/// A file's audio stream as PCM at its own rate.
pub struct AudioDecoder {
    playback: Playback,
    source: AudioSourceInfo,
    /// Channels the sink delivers: the output's count, or one to be spread.
    read_channels: u32,
    channels: u32,
}

const FLOAT_BYTES: usize = 4;

impl AudioDecoding for AudioDecoder {
    fn open(path: &str, channels: u32) -> MediaResult<Option<Self>> {
        let discovered = discover(path)?;
        let Some(stream) = discovered.audio_streams().into_iter().next() else {
            return Ok(None);
        };
        if stream.channels() == 0 {
            return Err(MediaError("the audio stream has no channels".into()));
        }
        let read_channels = if stream.channels() == 1 { 1 } else { channels };
        let caps = gst::Caps::builder("audio/x-raw")
            .field("format", "F32LE")
            .field("layout", "interleaved")
            .field("channels", read_channels as i32)
            .build();
        let playback = Playback::open(path, false, &caps)?;
        let decoded = playback.caps()?;
        let rate = decoded
            .structure(0)
            .and_then(|structure| structure.get::<i32>("rate").ok())
            .filter(|rate| *rate > 0);
        let rate = rate.ok_or_else(|| MediaError("the audio stream has no usable rate".into()))?;
        let length = discovered
            .duration()
            .or_else(|| playback.playbin.query_duration::<gst::ClockTime>());
        let source = AudioSourceInfo {
            codec_name: codec_name(stream.caps().as_deref()),
            sample_rate: rate as u32,
            channels: stream.channels(),
            duration_seconds: length.map_or(0.0, |length| length.nseconds() as f64 / 1e9),
        };
        Ok(Some(Self {
            playback,
            source,
            read_channels,
            channels,
        }))
    }

    fn source(&self) -> &AudioSourceInfo {
        &self.source
    }

    fn read(&mut self) -> MediaResult<Option<(i64, Vec<f32>)>> {
        loop {
            let Some(sample) = self.playback.next()? else {
                return Ok(None);
            };
            let (Some(seconds), Some(buffer)) = (Playback::seconds_of(&sample), sample.buffer())
            else {
                continue;
            };
            let map = buffer
                .map_readable()
                .map_err(|_| MediaError("sound could not be read".into()))?;
            let frame_bytes = FLOAT_BYTES * self.read_channels as usize;
            let whole = map.len() - map.len() % frame_bytes;
            let samples: Vec<f32> = map[..whole]
                .as_chunks::<FLOAT_BYTES>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes))
                .collect();
            if samples.is_empty() {
                continue;
            }
            let start = (seconds * f64::from(self.source.sample_rate)).round() as i64;
            let samples = if self.read_channels == 1 {
                spread_one_channel(samples, self.channels)
            } else {
                samples
            };
            return Ok(Some((start, samples)));
        }
    }

    fn seek(&mut self, frame: u64) -> MediaResult<()> {
        // An accurate seek: the decoder clips its output to start on the sample.
        self.playback.seek(
            frame as f64 / f64::from(self.source.sample_rate),
            gst::SeekFlags::ACCURATE,
        )
    }
}
