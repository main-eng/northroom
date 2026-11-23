//! A file's sound in the working format. The platform decodes at the file's own rate; the rate
//! is changed here so every platform sounds the same.

use super::resample::Resampler;
use super::{
    AudioChunk, AudioDecoding, AudioFormat, AudioSourceInfo, MediaError, MediaResult, PlatformAudio,
};

/// How far before a seek target decoding starts. A decoder's first packets after a seek lack
/// the history that overlapping transforms need, and so does the resampler's filter. Opus is
/// slowest: it matches a read from the start to within 1/1000 after about 200 ms, and exactly
/// by 500 ms.
const SEEK_PRE_ROLL_SECONDS: f64 = 0.5;

/// A file's audio stream as PCM in one output format. Mono is copied to every output channel;
/// more channels than the output has are mixed down.
pub struct AudioReader {
    decoder: PlatformAudio,
    output: AudioFormat,
    /// Set when the file's rate differs from the output's.
    resampler: Option<Resampler>,
    /// The source frame the next decoded samples should start at.
    expected_source: u64,
    /// The output frame the next returned samples start at.
    next_frame: i64,
    finished: bool,
}

impl AudioReader {
    /// Opens the file's first audio stream, or `None` if it has none.
    pub fn open(path: &str, output: AudioFormat) -> MediaResult<Option<Self>> {
        if output.sample_rate == 0 || output.channels == 0 {
            return Err(MediaError(
                "audio output needs a sample rate and at least one channel".into(),
            ));
        }
        let Some(decoder) = PlatformAudio::open(path, output.channels)? else {
            return Ok(None);
        };
        let rate = decoder.source().sample_rate;
        let resampler = (rate != output.sample_rate)
            .then(|| Resampler::new(rate, output.sample_rate, output.channels as usize));
        Ok(Some(Self {
            decoder,
            output,
            resampler,
            expected_source: 0,
            next_frame: 0,
            finished: false,
        }))
    }

    pub fn source(&self) -> &AudioSourceInfo {
        self.decoder.source()
    }

    /// The next run of samples, or `None` at the end. Runs follow each other without gaps from
    /// where reading started.
    pub fn read(&mut self) -> MediaResult<Option<AudioChunk>> {
        let channels = self.output.channels as usize;
        while !self.finished {
            let samples = match self.decoder.read()? {
                Some((start, samples)) => {
                    let run = self.placed_run(start, &samples);
                    self.resampled(run)
                }
                None => {
                    self.finished = true;
                    self.resampler_tail()
                }
            };
            if samples.is_empty() {
                continue;
            }
            let start_frame = self.next_frame;
            self.next_frame += (samples.len() / channels) as i64;
            return Ok(Some(AudioChunk {
                samples,
                start_frame,
            }));
        }
        Ok(None)
    }

    /// Places decoded samples starting at source frame `start` right after the last ones. A gap
    /// becomes silence; repeated samples are dropped.
    fn placed_run(&mut self, start: i64, samples: &[f32]) -> Vec<f32> {
        let channels = self.output.channels as usize;
        let start = start.max(0) as u64;
        let mut run = Vec::with_capacity(samples.len());
        if start > self.expected_source {
            run.resize((start - self.expected_source) as usize * channels, 0.0);
        }
        let repeated =
            (self.expected_source.saturating_sub(start) as usize * channels).min(samples.len());
        run.extend_from_slice(&samples[repeated..]);
        self.expected_source += (run.len() / channels) as u64;
        run
    }

    /// Source samples at the output rate, resampled when the rates differ.
    fn resampled(&mut self, run: Vec<f32>) -> Vec<f32> {
        match &mut self.resampler {
            Some(resampler) => resampler.push(&run),
            None => run,
        }
    }

    /// The output still owed once the source has ended.
    fn resampler_tail(&mut self) -> Vec<f32> {
        match &mut self.resampler {
            Some(resampler) => resampler.finish(),
            None => Vec::new(),
        }
    }

    /// Seeks so the next samples lead up to `seconds`. Decoding starts a little earlier, on a
    /// frame where source and output samples line up, so a read after a seek matches a read
    /// from the start.
    pub fn seek(&mut self, seconds: f64) -> MediaResult<()> {
        let output_rate = u64::from(self.output.sample_rate);
        let wanted =
            ((seconds - SEEK_PRE_ROLL_SECONDS).max(0.0) * output_rate as f64).floor() as u64;
        let (output_frame, source_frame) = match &mut self.resampler {
            Some(resampler) => {
                resampler.reset();
                let (up, down) = resampler.grid();
                let steps = wanted / up;
                (steps * up, steps * down)
            }
            None => (wanted, wanted),
        };
        self.decoder.seek(source_frame)?;
        self.expected_source = source_frame;
        self.next_frame = output_frame as i64;
        self.finished = false;
        Ok(())
    }
}
