//! Sound decoded by AVFoundation (Core Audio's decoders) to interleaved 32-bit float at the
//! file's own rate, with mono copied to each output channel and more channels mixed down. The
//! core changes the rate (`media::resample`), so every platform converts alike and seeks land
//! on the sample. Matroska and WebM, which AVFoundation doesn't read, go through the core's
//! demuxer and Core Audio converters (`converter`).

use super::av::{
    four_character_code, kCFBooleanFalse, kCFBooleanTrue, AVFormatIDKey, AVLinearPCMBitDepthKey,
    AVLinearPCMIsBigEndianKey, AVLinearPCMIsFloatKey, AVLinearPCMIsNonInterleaved,
    AVNumberOfChannelsKey, AVSampleRateKey, Asset,
    CMAudioFormatDescriptionGetStreamBasicDescription, CMTime, Cf, Reader, Track, TrackKind,
};
use super::converter::MatroskaAudio;
use super::format::codec_name;
use crate::media::{AudioDecoding, AudioSourceInfo, MediaError, MediaResult};

/// A file's audio stream as PCM at its own rate.
pub struct AudioDecoder(Decoder);

enum Decoder {
    Asset(AssetAudio),
    Matroska(Box<MatroskaAudio>),
}

impl AudioDecoding for AudioDecoder {
    fn open(path: &str, channels: u32) -> MediaResult<Option<Self>> {
        if crate::media::matroska::is_matroska(path) {
            return Ok(MatroskaAudio::open(path, channels)?
                .map(|audio| Self(Decoder::Matroska(Box::new(audio)))));
        }
        Ok(AssetAudio::open(path, channels)?.map(|audio| Self(Decoder::Asset(audio))))
    }

    fn source(&self) -> &AudioSourceInfo {
        match &self.0 {
            Decoder::Asset(audio) => audio.source(),
            Decoder::Matroska(audio) => audio.source(),
        }
    }

    fn read(&mut self) -> MediaResult<Option<(i64, Vec<f32>)>> {
        match &mut self.0 {
            Decoder::Asset(audio) => audio.read(),
            Decoder::Matroska(audio) => audio.read(),
        }
    }

    fn seek(&mut self, frame: u64) -> MediaResult<()> {
        match &mut self.0 {
            Decoder::Asset(audio) => audio.seek(frame),
            Decoder::Matroska(audio) => audio.seek(frame),
        }
    }
}

/// A file AVFoundation reads.
struct AssetAudio {
    asset: Asset,
    track: Track,
    settings: Cf,
    reader: Reader,
    channels: usize,
    source: AudioSourceInfo,
}

// SAFETY: the decoder's parts are each safe to move between threads (see `Asset`, `Track`,
// `Reader` and `Cf`), and it is used from one thread at a time.
unsafe impl Send for AssetAudio {}

impl AssetAudio {
    /// Seeks to source frame `frame` with a time range that starts exactly on the sample.
    fn reader_from(&self, frame: u64) -> MediaResult<Reader> {
        let start = CMTime::new(frame as i64, self.source.sample_rate as i32);
        Reader::start_at(
            &self.asset,
            &self.track,
            Some(&self.settings),
            (frame > 0).then_some(start),
        )
    }
}

impl AudioDecoding for AssetAudio {
    fn open(path: &str, channels: u32) -> MediaResult<Option<Self>> {
        let asset = Asset::open(path)?;
        let Some(track) = asset.first_track(TrackKind::Audio) else {
            return Ok(None);
        };
        let format = track
            .format_description()
            .ok_or_else(|| MediaError("the audio stream has no format".into()))?;
        // SAFETY: `format` is a live audio format description; the stream description it points
        // to is owned by it.
        let stream = unsafe {
            CMAudioFormatDescriptionGetStreamBasicDescription(format.0)
                .as_ref()
                .copied()
        }
        .ok_or_else(|| MediaError("the audio stream describes no samples".into()))?;
        let sample_rate = stream.sample_rate.round();
        if !(sample_rate >= 1.0 && sample_rate <= f64::from(i32::MAX)) {
            return Err(MediaError(format!(
                "the audio stream has no usable rate ({})",
                stream.sample_rate
            )));
        }
        let (pcm, bits, rate, count) = (
            Cf::integer(i64::from(four_character_code(b"lpcm")))?,
            Cf::integer(32)?,
            Cf::real(sample_rate)?,
            Cf::integer(i64::from(channels))?,
        );
        // SAFETY: the keys are framework constants; the values are live.
        let settings = unsafe {
            Cf::dictionary(&[
                (AVFormatIDKey, pcm.0),
                (AVLinearPCMBitDepthKey, bits.0),
                (AVLinearPCMIsFloatKey, kCFBooleanTrue),
                (AVLinearPCMIsNonInterleaved, kCFBooleanFalse),
                (AVLinearPCMIsBigEndianKey, kCFBooleanFalse),
                (AVSampleRateKey, rate.0),
                (AVNumberOfChannelsKey, count.0),
            ])?
        };
        let reader = Reader::start_at(&asset, &track, Some(&settings), None)?;
        let source = AudioSourceInfo {
            codec_name: codec_name(stream.format_id),
            sample_rate: sample_rate as u32,
            channels: stream.channels_per_frame,
            duration_seconds: asset.duration_seconds().unwrap_or(0.0),
        };
        Ok(Some(Self {
            asset,
            track,
            settings,
            reader,
            channels: channels as usize,
            source,
        }))
    }

    fn source(&self) -> &AudioSourceInfo {
        &self.source
    }

    fn read(&mut self) -> MediaResult<Option<(i64, Vec<f32>)>> {
        loop {
            let Some(sample) = self.reader.next()? else {
                return Ok(None);
            };
            let mut samples = sample.floats()?;
            samples.truncate(sample.count() * self.channels);
            if samples.is_empty() {
                continue;
            }
            let start = sample.presentation().ticks(self.source.sample_rate as i32);
            return Ok(Some((start, samples)));
        }
    }

    fn seek(&mut self, frame: u64) -> MediaResult<()> {
        self.reader = self.reader_from(frame)?;
        Ok(())
    }
}
