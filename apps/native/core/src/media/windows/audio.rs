//! Sound decoded by Media Foundation to interleaved 32-bit float at the file's own rate. Mono
//! is read as is and copied to each output channel here (`media::spread_one_channel`); more
//! channels are read in the output's count, which the reader's converter mixes down. The core
//! changes the rate (`media::resample`).

use super::{
    codec_name, duration_seconds, failed, number, open_reader, select, set_position, FIRST_AUDIO,
    TICKS_PER_SECOND,
};
use crate::media::{spread_one_channel, AudioDecoding, AudioSourceInfo, MediaError, MediaResult};
use windows::Win32::Media::MediaFoundation::*;

/// A file's audio stream as PCM at its own rate.
pub struct AudioDecoder {
    reader: IMFSourceReader,
    source: AudioSourceInfo,
    /// Channels the reader delivers: the output's count, or one to be spread.
    read_channels: u32,
    channels: u32,
}

// SAFETY: a Source Reader is free-threaded, and this decoder is used from one thread at a time.
unsafe impl Send for AudioDecoder {}

const FLOAT_BYTES: u32 = 4;

impl AudioDecoder {
    /// The reader's next audio sample, with its flags and its time in ticks.
    fn read_sample(&self) -> MediaResult<(u32, i64, Option<IMFSample>)> {
        let (mut flags, mut time, mut sample) = (0u32, 0i64, None);
        // SAFETY: a live reader; it fills what it is given.
        unsafe {
            self.reader.ReadSample(
                FIRST_AUDIO,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )
        }
        .map_err(failed("decode sound"))?;
        Ok((flags, time, sample))
    }

    /// A sample's whole frames and the frame they start on, from its `time` in ticks, with mono
    /// copied to each output channel. `None` if it holds no whole frame.
    fn placed_samples(
        &self,
        sample: &IMFSample,
        time: i64,
    ) -> MediaResult<Option<(i64, Vec<f32>)>> {
        let mut samples = floats(sample)?;
        let whole = samples.len() - samples.len() % self.read_channels as usize;
        samples.truncate(whole);
        if samples.is_empty() {
            return Ok(None);
        }
        let start =
            (time as f64 * f64::from(self.source.sample_rate) / TICKS_PER_SECOND).round() as i64;
        let samples = if self.read_channels == 1 {
            spread_one_channel(samples, self.channels)
        } else {
            samples
        };
        Ok(Some((start, samples)))
    }
}

impl AudioDecoding for AudioDecoder {
    fn open(path: &str, channels: u32) -> MediaResult<Option<Self>> {
        let reader = open_reader(path, false)?;
        let Some(stored) = select(&reader, FIRST_AUDIO) else {
            return Ok(None);
        };
        let rate = number(&stored, &MF_MT_AUDIO_SAMPLES_PER_SECOND)
            .filter(|rate| *rate > 0 && *rate <= i32::MAX as u32);
        let rate = rate.ok_or_else(|| MediaError("the audio stream has no usable rate".into()))?;
        let stored_channels = number(&stored, &MF_MT_AUDIO_NUM_CHANNELS).filter(|count| *count > 0);
        let stored_channels =
            stored_channels.ok_or_else(|| MediaError("the audio stream has no channels".into()))?;
        let read_channels = if stored_channels == 1 { 1 } else { channels };
        // SAFETY: Media Foundation calls with live arguments.
        let codec = unsafe {
            let wanted = MFCreateMediaType().map_err(failed("make a media type"))?;
            let block = read_channels * FLOAT_BYTES;
            let describe = || -> windows::core::Result<()> {
                wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
                wanted.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, read_channels)?;
                wanted.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
                wanted.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, FLOAT_BYTES * 8)?;
                wanted.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block)?;
                wanted.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, block * rate)
            };
            describe().map_err(failed("describe the sound"))?;
            reader
                .SetCurrentMediaType(FIRST_AUDIO, None, &wanted)
                .map_err(failed("set up the audio decoder"))?;
            stored
                .GetGUID(&MF_MT_SUBTYPE)
                .map_err(failed("read the sound's codec"))?
        };
        let source = AudioSourceInfo {
            codec_name: codec_name(&codec),
            sample_rate: rate,
            channels: stored_channels,
            duration_seconds: duration_seconds(&reader).unwrap_or(0.0),
        };
        Ok(Some(Self {
            reader,
            source,
            read_channels,
            channels,
        }))
    }

    fn source(&self) -> &AudioSourceInfo {
        &self.source
    }

    fn read(&mut self) -> MediaResult<Option<(i64, Vec<f32>)>> {
        super::start()?;
        loop {
            let (flags, time, sample) = self.read_sample()?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(MediaError("the audio decoder failed".into()));
            }
            match sample {
                Some(sample) => {
                    if let Some(run) = self.placed_samples(&sample, time)? {
                        return Ok(Some(run));
                    }
                }
                None if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 => return Ok(None),
                None => {}
            }
        }
    }

    fn seek(&mut self, frame: u64) -> MediaResult<()> {
        super::start()?;
        // The reader starts at the packet holding `frame` or earlier; `AudioReader` drops the
        // repeated samples before it.
        set_position(
            &self.reader,
            frame as f64 / f64::from(self.source.sample_rate),
        )
    }
}

/// A sample's 32-bit floats.
fn floats(sample: &IMFSample) -> MediaResult<Vec<f32>> {
    // SAFETY: the buffer is locked while its bytes are read and unlocked after.
    unsafe {
        let buffer = sample
            .ConvertToContiguousBuffer()
            .map_err(failed("read sound"))?;
        let (mut start, mut length) = (std::ptr::null_mut(), 0);
        buffer
            .Lock(&mut start, None, Some(&mut length))
            .map_err(failed("lock sound"))?;
        let bytes = std::slice::from_raw_parts(start, length as usize);
        let samples = bytes
            .as_chunks::<{ FLOAT_BYTES as usize }>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        buffer.Unlock().map_err(failed("unlock sound"))?;
        Ok(samples)
    }
}
