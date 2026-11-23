//! The preview's sound. A thread decodes the file's audio through the platform (`media`) one
//! second ahead into a buffer; the shell's audio output requests samples and gets the ones at
//! the preview's clock. The preview clock is the master, never the output's: samples play
//! contiguously while they stay near the clock, and resync to it when they diverge (a seek, a
//! stall, or clock drift).

use crate::media::{AudioChunk, AudioFormat, AudioReader};
use crate::Relock;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// The format sent to the shell's output: interleaved 32-bit float at the shared sound output's
/// rate and channel count (`sound_output`).
pub const OUTPUT: AudioFormat = AudioFormat {
    sample_rate: crate::preview::sound_output::RATE,
    channels: crate::preview::sound_output::CHANNELS,
};
const CHANNELS: usize = OUTPUT.channels as usize;
/// How far decoding runs ahead of playback.
const BUFFER_AHEAD_FRAMES: usize = OUTPUT.sample_rate as usize;
/// How far playback may drift from the clock before it resyncs (60 ms). Below this, playing on
/// sounds better than a jump.
const RESYNC_FRAMES: i64 = OUTPUT.sample_rate as i64 * 60 / 1000;
/// Beyond this drift (5 ms), playback is eased back towards the clock by about one frame in a
/// few hundred, which is inaudible. The output clock and the preview clock always drift a little.
const EASE_FRAMES: i64 = OUTPUT.sample_rate as i64 * 5 / 1000;
/// While easing, one frame is dropped or repeated per this many frames.
const EASE_ONE_IN: i64 = 384;
/// At other speeds the sound keeps its pitch, as in a browser. It is cut into two-hop (20 ms)
/// Hann-windowed grains read `speed` hops apart and written one hop apart, each shifted by up to
/// `GRAIN_SEARCH_FRAMES` to best continue the previous one. This is WSOLA, as Chromium uses.
const GRAIN_HOP: usize = OUTPUT.sample_rate as usize / 100;
const GRAIN_SEARCH_FRAMES: i64 = OUTPUT.sample_rate as i64 * 5 / 1000;
/// How long the decoding thread sleeps when the buffer is full.
const FULL_BUFFER_WAIT: Duration = Duration::from_millis(20);

/// Playback statistics, for checks that inspect output instead of listening.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioStats {
    /// Frames sent while playing, including silence for gaps.
    pub frames: u64,
    /// Of those, frames the buffer did not have yet.
    pub missing_frames: u64,
    /// Number of resyncs to the clock.
    pub resyncs: u32,
    /// Requests in which playback was eased towards the clock by about a frame.
    pub eased_frames: u64,
    /// The largest sample sent, before volume; 0 means only silence.
    pub peak: f32,
    /// Drift of the last samples sent from the clock, in seconds; positive is ahead.
    pub drift_seconds: f64,
    /// The largest drift in either direction since the last resync.
    pub largest_drift_seconds: f64,
    /// The media time of the first sample in the last request.
    pub position_seconds: f64,
}

/// Time-stretch state for playback at another speed.
struct Stretch {
    /// Frames produced and not yet sent.
    ready: VecDeque<f32>,
    /// The falling half of the last grain, which the next grain's rising half overlaps.
    tail: Vec<f32>,
    /// The file frame the next grain should start near.
    next: f64,
    /// The file frame that naturally follows the last grain.
    follows: Option<i64>,
}

impl Stretch {
    fn from(frame: f64) -> Self {
        Self {
            ready: VecDeque::new(),
            tail: vec![0.0; GRAIN_HOP * CHANNELS],
            next: frame,
            follows: None,
        }
    }

    /// The file frame of the next sample to send.
    fn position(&self, rate: f64) -> f64 {
        self.next - (self.ready.len() / CHANNELS) as f64 * rate
    }
}

/// Decoded samples waiting to be sent, and the output's position.
#[derive(Default)]
struct Buffer {
    samples: VecDeque<f32>,
    /// The file frame of the first buffered sample.
    start_frame: i64,
    /// Where the decoder should seek next, when the output requested frames not buffered.
    wanted_frame: Option<i64>,
    /// The next frame to send; `None` until the first request while playing.
    cursor: Option<i64>,
    /// All the file's audio from the buffer's start has been decoded.
    finished: bool,
    /// The playback rate when not 1×, and its time-stretch state.
    rate: Option<f64>,
    stretch: Option<Stretch>,
    stats: AudioStats,
}

impl Buffer {
    fn end_frame(&self) -> i64 {
        self.start_frame + (self.samples.len() / CHANNELS) as i64
    }

    /// Fills `out` with the frames at `clock_seconds`, or silence when nothing is playing.
    /// Returns whether the decoder has work to do.
    fn hand_out(&mut self, clock_seconds: Option<f64>, gain: f32, out: &mut [f32]) -> bool {
        out.fill(0.0);
        let Some(clock_seconds) = clock_seconds else {
            // Paused: the next samples resync to the clock.
            self.cursor = None;
            self.stretch = None;
            return false;
        };
        if let Some(rate) = self.rate {
            return self.hand_out_stretched(clock_seconds, rate, gain, out);
        }
        let frames = (out.len() / CHANNELS) as i64;
        let target = (clock_seconds * f64::from(OUTPUT.sample_rate)).round() as i64;
        let (cursor, held) = self.place_cursor(target, frames);
        self.note_position(cursor as f64, target as f64);
        // Everything buffered before the cursor has been played or skipped.
        self.drop_before(cursor);
        let wake = self.send_decoder_to(cursor);
        let wanted = frames - held;
        let copied = self.copy_out(cursor, held, wanted, gain, out);
        self.stats.frames += frames as u64;
        self.count_missing((wanted - copied) as u64);
        self.cursor = Some(cursor + wanted);
        wake || self.samples.len() / CHANNELS < BUFFER_AHEAD_FRAMES
    }

    /// Where a request of `frames` frames starts for the clock at `target`: continuing from the
    /// last request, eased towards `target`, while near it, else at `target`. Also returns how many
    /// leading frames repeat the next sample instead of advancing, which is how playback that ran
    /// ahead of the clock is held back.
    fn place_cursor(&mut self, target: i64, frames: i64) -> (i64, i64) {
        match self.cursor {
            Some(cursor) if (cursor - target).abs() <= RESYNC_FRAMES => {
                let apart = cursor - target;
                let ease = if apart.abs() > EASE_FRAMES {
                    (frames / EASE_ONE_IN).max(1).min(frames)
                } else {
                    0
                };
                self.stats.eased_frames += ease as u64;
                if apart > 0 {
                    (cursor, ease)
                } else {
                    // Behind the clock: a frame is skipped.
                    (cursor + ease, 0)
                }
            }
            previous => {
                self.note_resync(previous);
                (target, 0)
            }
        }
    }

    /// Restarts playback at the clock, counting a resync when there was a `previous` position.
    fn note_resync<T>(&mut self, previous: Option<T>) {
        if previous.is_some() {
            self.stats.resyncs += 1;
        }
        self.stats.largest_drift_seconds = 0.0;
    }

    /// Records that file frame `position` is sent next, for the clock at frame `target`.
    fn note_position(&mut self, position: f64, target: f64) {
        let drift = (position - target) / f64::from(OUTPUT.sample_rate);
        self.stats.drift_seconds = drift;
        if drift.abs() > self.stats.largest_drift_seconds.abs() {
            self.stats.largest_drift_seconds = drift;
        }
        self.stats.position_seconds = position / f64::from(OUTPUT.sample_rate);
    }

    /// Drops buffered samples before file frame `frame`.
    fn drop_before(&mut self, frame: i64) {
        let stale = (frame - self.start_frame).clamp(0, (self.samples.len() / CHANNELS) as i64);
        self.samples.drain(..stale as usize * CHANNELS);
        self.start_frame += stale;
    }

    /// When `cursor` is before the buffer or far past it, sends the decoder there, unless it is
    /// past the end of a fully decoded file. Returns whether it did.
    fn send_decoder_to(&mut self, cursor: i64) -> bool {
        let outside =
            cursor < self.start_frame || cursor > self.end_frame() + BUFFER_AHEAD_FRAMES as i64;
        if !outside || (self.finished && cursor >= self.end_frame()) {
            return false;
        }
        self.wanted_frame = Some(cursor);
        true
    }

    /// Fills `out` from a `cursor` at the buffer's start: `held` frames repeating its first frame,
    /// then up to `wanted` advancing frames. Returns how many advancing frames were written.
    fn copy_out(&mut self, cursor: i64, held: i64, wanted: i64, gain: f32, out: &mut [f32]) -> i64 {
        if cursor != self.start_frame || self.samples.is_empty() {
            return 0;
        }
        // `place_cursor` holds back between none and all of the requested frames, so neither
        // count is negative.
        let held = usize::try_from(held).expect("frames held are never negative");
        let wanted = usize::try_from(wanted).expect("frames wanted are never negative");
        for slot in out[..held * CHANNELS].chunks_mut(CHANNELS) {
            for (channel, value) in slot.iter_mut().enumerate() {
                *value = self.samples[channel] * gain;
            }
        }
        let copied = wanted.min(self.samples.len() / CHANNELS);
        write_gained(
            &mut out[held * CHANNELS..],
            self.samples.drain(..copied * CHANNELS),
            gain,
            &mut self.stats.peak,
        );
        let copied = i64::try_from(copied).expect("a buffer's frames fit an i64");
        self.start_frame += copied;
        copied
    }

    /// Counts `frames` requested but missing, unless the whole file has been played.
    fn count_missing(&mut self, frames: u64) {
        if !(self.finished && self.samples.is_empty()) {
            self.stats.missing_frames += frames;
        }
    }

    /// The buffered sample of `channel` at file frame `frame`; silence outside the buffer.
    fn sample(&self, frame: i64, channel: usize) -> f32 {
        // A frame before the buffer's start is outside it.
        let Ok(offset) = usize::try_from(frame - self.start_frame) else {
            return 0.0;
        };
        self.samples
            .get(offset * CHANNELS + channel)
            .copied()
            .unwrap_or(0.0)
    }

    /// The sum of both channels at `frame`, for waveform comparison.
    fn mixed(&self, frame: i64) -> f32 {
        (0..CHANNELS)
            .map(|channel| self.sample(frame, channel))
            .sum()
    }

    /// Where near `ideal` the next grain starts: the point most similar to what naturally follows
    /// the last grain, so the two overlap without a break.
    fn grain_start(&self, ideal: i64, follows: Option<i64>) -> i64 {
        let Some(follows) = follows.filter(|follows| *follows != ideal) else {
            return ideal;
        };
        let wanted: Vec<f32> = (0..GRAIN_HOP as i64)
            .map(|frame| self.mixed(follows + frame))
            .collect();
        let first = ideal - GRAIN_SEARCH_FRAMES;
        let near: Vec<f32> = (0..2 * GRAIN_SEARCH_FRAMES + GRAIN_HOP as i64)
            .map(|frame| self.mixed(first + frame))
            .collect();
        let mut best = (f32::NEG_INFINITY, ideal);
        for shift in 0..=(2 * GRAIN_SEARCH_FRAMES) as usize {
            let candidate = &near[shift..shift + GRAIN_HOP];
            let (alike, energy) = candidate
                .iter()
                .zip(&wanted)
                .fold((0.0_f32, 0.0_f32), |(alike, energy), (have, want)| {
                    (alike + have * want, energy + have * have)
                });
            let score = alike / (energy.sqrt() + 1e-9);
            if score > best.0 {
                best = (score, first + shift as i64);
            }
        }
        best.1
    }

    /// `hand_out` at another speed: the frames at the clock, played `rate` times as fast with pitch
    /// preserved. Resync and easing use the same limits as at 1×.
    fn hand_out_stretched(
        &mut self,
        clock_seconds: f64,
        rate: f64,
        gain: f32,
        out: &mut [f32],
    ) -> bool {
        let frames = out.len() / CHANNELS;
        let target = clock_seconds * f64::from(OUTPUT.sample_rate);
        self.cursor = None;
        let mut stretch = self.place_stretch(target, rate, frames);
        self.note_position(stretch.position(rate), target);
        let wake = self.lay_down(&mut stretch, rate, frames);
        let copied = frames.min(stretch.ready.len() / CHANNELS);
        write_gained(
            out,
            stretch.ready.drain(..copied * CHANNELS),
            gain,
            &mut self.stats.peak,
        );
        self.stats.frames += frames as u64;
        self.count_missing((frames - copied) as u64);
        // Drop what no later grain can read.
        let keep_from = stretch
            .follows
            .unwrap_or(i64::MAX)
            .min(stretch.next.round() as i64)
            - 2 * GRAIN_SEARCH_FRAMES;
        self.drop_before(keep_from);
        self.stretch = Some(stretch);
        wake || self.samples.len() / CHANNELS < BUFFER_AHEAD_FRAMES
    }

    /// The stretch state for `rate` at clock frame `target`: the last request's, with its next
    /// grain eased towards the clock, while near it; else a fresh one at the clock.
    fn place_stretch(&mut self, target: f64, rate: f64, frames: usize) -> Stretch {
        match self.stretch.take() {
            Some(mut stretch)
                if (stretch.position(rate) - target).abs() <= RESYNC_FRAMES as f64 =>
            {
                let apart = stretch.position(rate) - target;
                if apart.abs() > EASE_FRAMES as f64 {
                    // Take the next grain about a frame nearer the clock; the overlap hides the
                    // shift.
                    let ease = (frames as i64 / EASE_ONE_IN).max(1);
                    stretch.next -= apart.signum() * ease as f64;
                    self.stats.eased_frames += ease as u64;
                }
                stretch
            }
            previous => {
                self.note_resync(previous);
                Stretch::from(target)
            }
        }
    }

    /// Adds grains to `stretch` at `rate` until it holds `frames` frames. Returns whether it
    /// stopped early to wait for the decoder.
    fn lay_down(&mut self, stretch: &mut Stretch, rate: f64, frames: usize) -> bool {
        while stretch.ready.len() < frames * CHANNELS {
            let ideal = stretch.next.round() as i64;
            if self.grain_unready(ideal, stretch.follows) {
                return true;
            }
            let start = self.grain_start(ideal, stretch.follows);
            self.add_grain(stretch, start);
            stretch.follows = Some(start + GRAIN_HOP as i64);
            stretch.next += GRAIN_HOP as f64 * rate;
        }
        false
    }

    /// Whether a grain near `ideal` must wait for the decoder because what it could read is
    /// outside the buffer (the decoder is sent there) or not decoded yet.
    fn grain_unready(&mut self, ideal: i64, follows: Option<i64>) -> bool {
        // Every frame a grain here could read: itself and, after a previous grain, the search
        // window either side and that grain's natural continuation. The first grain is not shifted.
        let (first, last) = match follows {
            Some(follows) => (
                follows.min(ideal - GRAIN_SEARCH_FRAMES).max(0),
                (follows + GRAIN_HOP as i64)
                    .max(ideal + GRAIN_SEARCH_FRAMES + 2 * GRAIN_HOP as i64),
            ),
            None => (ideal.max(0), ideal + 2 * GRAIN_HOP as i64),
        };
        if first < self.start_frame || first > self.end_frame() + BUFFER_AHEAD_FRAMES as i64 {
            // Before the buffer, or far past it: send the decoder there.
            if self.finished && first >= self.start_frame {
                return false;
            }
            self.wanted_frame = Some(first);
            return true;
        }
        // Not decoded that far yet.
        last > self.end_frame() && !self.finished
    }

    /// Adds the grain starting at file frame `start`: its rising half overlaps the last grain's
    /// falling half and is ready to send; its falling half is kept for the next grain.
    fn add_grain(&self, stretch: &mut Stretch, start: i64) {
        for frame in 0..GRAIN_HOP {
            // A two-hop Hann window, whose rising and falling halves sum to one.
            let rise = 0.5 - 0.5 * (std::f32::consts::PI * frame as f32 / GRAIN_HOP as f32).cos();
            for channel in 0..CHANNELS {
                let made = stretch.tail[frame * CHANNELS + channel]
                    + rise * self.sample(start + frame as i64, channel);
                stretch.ready.push_back(made);
                stretch.tail[frame * CHANNELS + channel] =
                    (1.0 - rise) * self.sample(start + (GRAIN_HOP + frame) as i64, channel);
            }
        }
    }
}

/// Writes `samples` scaled by `gain` into `out`, tracking the loudest sample in `peak`.
#[inline]
fn write_gained(out: &mut [f32], samples: impl Iterator<Item = f32>, gain: f32, peak: &mut f32) {
    for (slot, sample) in out.iter_mut().zip(samples) {
        *peak = peak.max(sample.abs());
        *slot = sample * gain;
    }
}

struct Shared {
    buffer: Mutex<Buffer>,
    wake: Condvar,
    quit: Mutex<bool>,
}

impl Shared {
    fn quitting(&self) -> bool {
        *self.quit.relock()
    }
}

/// A file's audio, decoded ahead on its own thread.
pub struct AudioTrack {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    duration_seconds: f64,
}

impl AudioTrack {
    /// Opens the audio in `path`. `None` when the file has no audio or it cannot be decoded. The
    /// reader lives on the decoding thread from the start because it cannot move between threads;
    /// this waits only for that thread to report the duration.
    pub fn open(path: &str) -> Option<Self> {
        let shared = Arc::new(Shared {
            buffer: Mutex::new(Buffer::default()),
            wake: Condvar::new(),
            quit: Mutex::new(false),
        });
        let thread_shared = Arc::clone(&shared);
        let path = path.to_owned();
        let (opened, duration) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("northroom-preview-sound".into())
            .spawn(move || {
                let reader = match AudioReader::open(&path, OUTPUT) {
                    Ok(reader) => reader,
                    Err(error) => {
                        eprintln!("preview sound did not open: {error}");
                        None
                    }
                };
                let _ = opened.send(
                    reader
                        .as_ref()
                        .map(|reader| reader.source().duration_seconds),
                );
                if let Some(reader) = reader {
                    decode(reader, &thread_shared);
                }
            })
            .ok()?;
        let Some(duration_seconds) = duration.recv().ok().flatten() else {
            let _ = thread.join();
            return None;
        };
        Some(Self {
            shared,
            thread: Some(thread),
            duration_seconds,
        })
    }

    pub fn duration_seconds(&self) -> f64 {
        self.duration_seconds
    }

    /// Fills `out` (interleaved, `OUTPUT`) with the samples at `clock_seconds` scaled by `gain`,
    /// or with silence when the clock is stopped. Called on the output's thread.
    pub fn render(&self, clock_seconds: Option<f64>, gain: f32, out: &mut [f32]) {
        let wake = self
            .shared
            .buffer
            .relock()
            .hand_out(clock_seconds, gain, out);
        if wake {
            self.shared.wake.notify_all();
        }
    }

    pub fn stats(&self) -> AudioStats {
        self.shared.buffer.relock().stats
    }

    /// Plays at `rate` times normal speed from now on. The caller of `render` must advance its
    /// clock at that speed.
    pub fn set_rate(&self, rate: f64) {
        let mut buffer = self.shared.buffer.relock();
        buffer.rate = (rate != 1.0).then_some(rate);
        // The next samples resync to the clock without counting as a resync.
        buffer.cursor = None;
        buffer.stretch = None;
    }
}

impl Drop for AudioTrack {
    fn drop(&mut self) {
        *self.shared.quit.relock() = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Keeps the buffer one second ahead of the output, seeking where the output asks.
fn decode(mut reader: AudioReader, shared: &Shared) {
    loop {
        match next_work(shared) {
            Work::Quit => return,
            Work::Seek(frame) => {
                if let Err(error) = reader.seek(frame as f64 / f64::from(OUTPUT.sample_rate)) {
                    eprintln!("preview sound did not seek: {error}");
                    shared.buffer.relock().finished = true;
                    continue;
                }
            }
            Work::Read => {}
        }
        let chunk = match reader.read() {
            Ok(chunk) => chunk,
            Err(error) => {
                eprintln!("preview sound stopped: {error}");
                None
            }
        };
        keep_chunk(shared, chunk);
    }
}

/// What the decoding thread does next.
enum Work {
    Quit,
    /// The output requested frames from here; the buffer now starts here, empty.
    Seek(i64),
    /// Keep reading; the buffer is not a second ahead yet.
    Read,
}

/// Waits until there is decoding to do or the track is closing.
fn next_work(shared: &Shared) -> Work {
    let mut buffer = shared.buffer.relock();
    loop {
        if shared.quitting() {
            return Work::Quit;
        }
        if let Some(frame) = buffer.wanted_frame.take() {
            buffer.samples.clear();
            buffer.start_frame = frame;
            buffer.finished = false;
            return Work::Seek(frame);
        }
        if !buffer.finished && buffer.samples.len() / CHANNELS < BUFFER_AHEAD_FRAMES {
            return Work::Read;
        }
        buffer = shared
            .wake
            .wait_timeout(buffer, FULL_BUFFER_WAIT)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
}

/// Buffers a decoded chunk, or marks the file finished when there was none.
fn keep_chunk(shared: &Shared, chunk: Option<AudioChunk>) {
    let mut buffer = shared.buffer.relock();
    if buffer.wanted_frame.is_some() {
        // The output moved during decoding, so this chunk is for the old position.
        return;
    }
    let Some(chunk) = chunk else {
        buffer.finished = true;
        return;
    };
    append(&mut buffer, &chunk.samples, chunk.start_frame);
}

/// Adds decoded samples starting at `start_frame`, keeping the buffer contiguous: samples before
/// the buffer's start are dropped (the decoder's lead-in after a seek), and gaps are filled with
/// silence.
fn append(buffer: &mut Buffer, samples: &[f32], start_frame: i64) {
    let end = buffer.end_frame();
    let skip = (end - start_frame).max(0) as usize * CHANNELS;
    let gap = (start_frame - end).max(0) as usize * CHANNELS;
    buffer.samples.extend(std::iter::repeat_n(0.0, gap));
    buffer.samples.extend(samples.iter().skip(skip));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const RATE: f64 = OUTPUT.sample_rate as f64;

    /// A buffer of `frames` frames from `start` where each sample equals its frame number, so the
    /// output shows where it came from.
    fn buffer_from(start: i64, frames: i64) -> Buffer {
        let mut buffer = Buffer {
            start_frame: start,
            ..Buffer::default()
        };
        let samples: Vec<f32> = (start..start + frames)
            .flat_map(|frame| [frame as f32, frame as f32])
            .collect();
        append(&mut buffer, &samples, start);
        buffer
    }

    /// Writes `seconds` of a 480 Hz tone as a 16-bit mono WAV at the output rate, so every 100
    /// frames is one cycle and a sample's value identifies its position.
    pub(crate) fn write_tone(path: &std::path::Path, seconds: u32) {
        let frames = OUTPUT.sample_rate * seconds;
        let mut bytes = Vec::with_capacity(44 + frames as usize * 2);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + frames * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&OUTPUT.sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(OUTPUT.sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(frames * 2).to_le_bytes());
        for frame in 0..frames {
            let value = (f64::from(frame % 100) / 100.0 * std::f64::consts::TAU).sin() * 0.5;
            bytes.extend_from_slice(&((value * 32767.0).round() as i16).to_le_bytes());
        }
        std::fs::write(path, bytes).expect("write the tone");
    }

    /// The tone's value at `frame`.
    fn tone_at(frame: i64) -> f32 {
        ((frame.rem_euclid(100) as f64 / 100.0 * std::f64::consts::TAU).sin() * 0.5) as f32
    }

    /// Requests `out` at `clock` until the decoder has those frames, as an output keeps asking.
    /// Each try is a fresh request at the same moment.
    fn render_when_decoded(track: &AudioTrack, clock: f64, out: &mut [f32]) {
        for _ in 0..400 {
            track.shared.buffer.relock().cursor = None;
            track.render(Some(clock), 1.0, out);
            let buffer = track.shared.buffer.relock();
            if buffer.stats.peak > 0.0 && out.iter().any(|sample| *sample != 0.0) {
                return;
            }
            drop(buffer);
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the decoder never reached {clock}");
    }

    #[test]
    fn a_real_file_hands_out_the_samples_that_belong_to_the_clocks_moment() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("tone.wav");
        write_tone(&path, 2);
        let track = AudioTrack::open(path.to_str().expect("path")).expect("a file with sound");
        assert!((track.duration_seconds() - 2.0).abs() < 0.01);
        let mut out = [0.0_f32; 512];
        // From the start, after a forward seek and after a backward one, the sample sent is the
        // tone's value at that frame, on both channels.
        for clock_frame in [0_i64, 60_025, 12_010] {
            render_when_decoded(&track, clock_frame as f64 / RATE, &mut out);
            for frame in [0_usize, 1, 37, 255] {
                let expected = tone_at(clock_frame + frame as i64);
                assert!(
                    (out[frame * 2] - expected).abs() < 2e-3,
                    "frame {clock_frame}+{frame}: {} vs {expected}",
                    out[frame * 2]
                );
                assert_eq!(out[frame * 2], out[frame * 2 + 1]);
            }
        }
        // The next request continues where the last one ended.
        track.render(Some((12_010.0 + 256.0) / RATE), 1.0, &mut out);
        assert!((out[0] - tone_at(12_010 + 256)).abs() < 2e-3);
        assert!((track.stats().peak - 0.5).abs() < 0.01);
        assert!(
            AudioTrack::open(directory.path().join("missing.wav").to_str().expect("path"))
                .is_none()
        );
    }

    /// A fully decoded buffer of `frames` frames of the tone from the file's start.
    fn tone_buffer(frames: i64) -> Buffer {
        let mut buffer = Buffer {
            finished: true,
            ..Buffer::default()
        };
        let samples: Vec<f32> = (0..frames)
            .flat_map(|frame| [tone_at(frame), tone_at(frame)])
            .collect();
        append(&mut buffer, &samples, 0);
        buffer
    }

    /// Requests 10 ms of audio every 10 ms for `seconds` while the clock runs at `rate`, and
    /// returns one channel of the output.
    fn play_at(buffer: &mut Buffer, rate: f64, seconds: f64) -> Vec<f32> {
        buffer.rate = (rate != 1.0).then_some(rate);
        let mut heard = Vec::new();
        let mut out = [0.0_f32; 960];
        for ask in 0..(seconds * 100.0) as i64 {
            buffer.hand_out(Some(ask as f64 * 0.01 * rate), 1.0, &mut out);
            heard.extend(out.chunks(CHANNELS).map(|frame| frame[0]));
        }
        heard
    }

    /// How many times `heard` rises through zero.
    fn cycles(heard: &[f32]) -> usize {
        heard
            .windows(2)
            .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
            .count()
    }

    #[test]
    fn at_twice_the_speed_the_sound_keeps_up_with_the_clock_and_keeps_its_pitch() {
        let mut buffer = tone_buffer(3 * 48_000);
        let heard = play_at(&mut buffer, 2.0, 1.0);
        // One second of output covers two of the file, and the last position sent matches the
        // clock.
        assert_eq!(buffer.stats.resyncs, 0);
        assert_eq!(buffer.stats.missing_frames, 0);
        assert!(
            buffer.stats.largest_drift_seconds.abs() <= 0.005,
            "drift {}",
            buffer.stats.largest_drift_seconds
        );
        assert!(
            (buffer.stats.position_seconds - 1.98).abs() < 0.005,
            "at {}",
            buffer.stats.position_seconds
        );
        // The 480 Hz tone stays 480 Hz (unstretched at 2× it would be 960 Hz) and as loud.
        let steady = &heard[4800..];
        assert!(
            (cycles(steady) as i64 - 432).abs() <= 2,
            "{} cycles in 0.9 s",
            cycles(steady)
        );
        let loudest = steady
            .iter()
            .fold(0.0_f32, |most, sample| most.max(sample.abs()));
        assert!((0.45..=0.55).contains(&loudest), "loudest {loudest}");
    }

    #[test]
    fn at_half_the_speed_the_sound_keeps_up_with_the_clock_and_keeps_its_pitch() {
        let mut buffer = tone_buffer(2 * 48_000);
        let heard = play_at(&mut buffer, 0.5, 2.0);
        assert_eq!(buffer.stats.resyncs, 0);
        assert_eq!(buffer.stats.missing_frames, 0);
        assert!(
            buffer.stats.largest_drift_seconds.abs() <= 0.005,
            "drift {}",
            buffer.stats.largest_drift_seconds
        );
        assert!(
            (buffer.stats.position_seconds - 0.995).abs() < 0.005,
            "at {}",
            buffer.stats.position_seconds
        );
        let steady = &heard[4800..];
        assert!(
            (cycles(steady) as i64 - 912).abs() <= 3,
            "{} cycles in 1.9 s",
            cycles(steady)
        );
        let loudest = steady
            .iter()
            .fold(0.0_f32, |most, sample| most.max(sample.abs()));
        assert!((0.45..=0.55).contains(&loudest), "loudest {loudest}");
    }

    #[test]
    fn a_change_of_speed_takes_the_place_from_the_clock_and_a_seek_at_speed_resyncs() {
        let mut buffer = buffer_from(0, 96_000);
        buffer.finished = true;
        let mut out = [0.0_f32; 960];
        // At 1×, then 1.5×: the grains start at the clock.
        buffer.hand_out(Some(0.0), 1.0, &mut out);
        buffer.rate = Some(1.5);
        buffer.cursor = None;
        buffer.hand_out(Some(0.01), 1.0, &mut out);
        assert_eq!(buffer.stats.resyncs, 0);
        assert!((buffer.stats.position_seconds - 0.01).abs() < 1e-9);
        // Each sample is its frame number, so once the first grain has faded in, the output
        // shows its source frame, which is within the search window of the clock.
        buffer.hand_out(Some(0.025), 1.0, &mut out);
        assert!(
            (f64::from(out[958]) - (480.0 + 720.0 + 479.0 * 1.5)).abs() <= 800.0,
            "{}",
            out[958]
        );
        assert!(buffer.stats.drift_seconds.abs() < 1e-9);
        // The clock jumps (a seek): the grains restart there, and a resync is counted.
        buffer.hand_out(Some(1.0), 1.0, &mut out);
        assert_eq!(buffer.stats.resyncs, 1);
        assert!((buffer.stats.position_seconds - 1.0).abs() < 1e-9);
        // A little behind the clock, inside the limit: eased, no jump.
        buffer.hand_out(Some(1.015 + 0.02), 1.0, &mut out);
        assert_eq!((buffer.stats.resyncs, buffer.stats.eased_frames), (1, 1));
        // Pausing drops the grains; back at 1× the samples are contiguous again.
        buffer.hand_out(None, 1.0, &mut out);
        assert!(buffer.stretch.is_none());
        buffer.rate = None;
        buffer.hand_out(Some(1.5), 1.0, &mut out);
        assert_eq!((out[0], out[958]), (72_000.0, 72_479.0));
    }

    #[test]
    fn a_stopped_clock_hands_out_silence() {
        let mut buffer = buffer_from(0, 1000);
        let mut out = [1.0_f32; 64];
        buffer.hand_out(None, 1.0, &mut out);
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert_eq!(buffer.stats.frames, 0);
        assert_eq!(buffer.samples.len(), 2000);
    }

    #[test]
    fn samples_follow_one_another_while_they_stay_near_the_clock() {
        let mut buffer = buffer_from(0, 4800);
        let mut out = [0.0_f32; 200];
        buffer.hand_out(Some(0.0), 1.0, &mut out);
        assert_eq!((out[0], out[198]), (0.0, 99.0));
        // The clock is 110 frames in and the output asked 10 frames late. No jump.
        buffer.hand_out(Some(110.0 / RATE), 1.0, &mut out);
        assert_eq!((out[0], out[198]), (100.0, 199.0));
        assert_eq!(buffer.stats.resyncs, 0);
        assert!((buffer.stats.drift_seconds - -10.0 / RATE).abs() < 1e-9);
        assert_eq!(buffer.stats.missing_frames, 0);
        assert_eq!(buffer.stats.peak, 199.0);
    }

    #[test]
    fn samples_take_their_place_from_the_clock_once_they_have_come_apart() {
        let mut buffer = buffer_from(0, 48_000);
        let mut out = [0.0_f32; 200];
        buffer.hand_out(Some(0.0), 1.0, &mut out);
        // Just inside the limit it continues, eased one frame towards the clock. Just outside,
        // it jumps to the clock.
        buffer.hand_out(Some((100 + RESYNC_FRAMES) as f64 / RATE), 1.0, &mut out);
        assert_eq!(out[0], 101.0);
        assert_eq!((buffer.stats.resyncs, buffer.stats.eased_frames), (0, 1));
        buffer.hand_out(Some((201 + RESYNC_FRAMES + 1) as f64 / RATE), 1.0, &mut out);
        assert_eq!(out[0], (201 + RESYNC_FRAMES + 1) as f32);
        assert_eq!(buffer.stats.resyncs, 1);
        assert_eq!(buffer.stats.drift_seconds, 0.0);
    }

    #[test]
    fn samples_ahead_of_the_clock_are_held_back_a_frame_at_a_time() {
        let mut buffer = buffer_from(0, 48_000);
        let mut out = [0.0_f32; 200];
        buffer.hand_out(Some(0.0), 1.0, &mut out);
        // Within 5 ms nothing changes.
        buffer.hand_out(Some((100 - EASE_FRAMES) as f64 / RATE), 1.0, &mut out);
        assert_eq!((out[0], buffer.stats.eased_frames), (100.0, 0));
        // Further ahead, the next request repeats its first frame once and so advances one
        // frame less. Behind the clock, one frame is skipped (the test above).
        buffer.hand_out(Some((200 - EASE_FRAMES - 1) as f64 / RATE), 1.0, &mut out);
        assert_eq!(buffer.stats.eased_frames, 1);
        assert_eq!((out[0], out[2], out[198]), (200.0, 200.0, 298.0));
        assert_eq!(buffer.stats.missing_frames, 0);
        assert_eq!(buffer.cursor, Some(299));
    }

    #[test]
    fn a_seek_outside_the_buffer_sends_the_decoder_there_and_is_silent_meanwhile() {
        let mut buffer = buffer_from(0, 4800);
        let mut out = [9.0_f32; 200];
        let wake = buffer.hand_out(Some(10.0), 1.0, &mut out);
        assert!(wake);
        assert_eq!(buffer.wanted_frame, Some(480_000));
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert_eq!(buffer.stats.missing_frames, 100);
        // The decoder returns a lead-in before the requested position, which is dropped.
        buffer.wanted_frame = None;
        buffer.samples.clear();
        buffer.start_frame = 480_000;
        let lead_in: Vec<f32> = (479_000..481_000)
            .flat_map(|frame| [frame as f32, frame as f32])
            .collect();
        append(&mut buffer, &lead_in, 479_000);
        assert_eq!(buffer.start_frame, 480_000);
        buffer.hand_out(Some(480_100.0 / RATE), 1.0, &mut out);
        assert_eq!(out[0], 480_100.0);
    }

    #[test]
    fn a_seek_backwards_asks_for_the_earlier_frames() {
        let mut buffer = buffer_from(48_000, 4800);
        let mut out = [0.0_f32; 200];
        buffer.hand_out(Some(0.5), 1.0, &mut out);
        assert_eq!(buffer.wanted_frame, Some(24_000));
    }

    #[test]
    fn volume_scales_what_is_handed_out_but_not_the_peak_read() {
        let mut buffer = buffer_from(100, 200);
        let mut out = [0.0_f32; 20];
        buffer.hand_out(Some(100.0 / RATE), 0.5, &mut out);
        assert_eq!(out[0], 50.0);
        assert_eq!(buffer.stats.peak, 109.0);
        buffer.hand_out(Some(110.0 / RATE), 0.0, &mut out);
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert_eq!(buffer.stats.peak, 119.0);
    }

    #[test]
    fn the_end_of_the_file_is_silence_without_counting_as_missing() {
        let mut buffer = buffer_from(0, 50);
        buffer.finished = true;
        let mut out = [0.0_f32; 200];
        let wake = buffer.hand_out(Some(0.0), 1.0, &mut out);
        assert_eq!(out[98], 49.0);
        assert_eq!(out[100], 0.0);
        assert!(!wake || buffer.wanted_frame.is_none());
        buffer.hand_out(Some(100.0 / RATE), 1.0, &mut out);
        assert_eq!(buffer.wanted_frame, None);
        assert_eq!(buffer.stats.missing_frames, 0);
    }

    #[test]
    fn a_gap_between_decoded_runs_is_filled_with_silence() {
        let mut buffer = buffer_from(0, 10);
        append(&mut buffer, &[7.0, 7.0], 12);
        assert_eq!(buffer.samples.len(), 26);
        assert_eq!(buffer.samples[20], 0.0);
        assert_eq!(buffer.samples[24], 7.0);
        // A chunk that overlaps the buffer adds only its new frames.
        append(&mut buffer, &[1.0, 1.0, 2.0, 2.0], 12);
        assert_eq!(buffer.samples.len(), 28);
        assert_eq!(buffer.samples[26], 2.0);
    }
}
