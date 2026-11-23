//! Sample-rate conversion, shared by every platform: a Kaiser-windowed sinc over 32 taps at the
//! lower rate, cut off at 97% of the lower Nyquist, matching libswresample's default filter.
//! Output frame `n` is the source at exactly `n × in / out` with no delay, so a conversion
//! started where both rates' samples line up matches one started from the beginning, once past
//! the filter's reach.

/// Taps at the lower of the two rates.
const FILTER_SIZE: usize = 32;
/// Where the pass band ends, as a share of the lower rate's Nyquist frequency.
const CUTOFF: f64 = 0.97;
/// The Kaiser window's shape.
const KAISER_BETA: f64 = 9.0;
/// The most phases whose taps are tabulated; rarer rate pairs compute taps per frame.
const MOST_TABULATED_PHASES: usize = 4096;

fn greatest_common_divisor(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The modified Bessel function of the first kind, order zero, by its series.
fn bessel_i0(x: f64) -> f64 {
    let quarter_square = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..64 {
        term *= quarter_square / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Whether the source continues or has ended (silence after the end).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceState {
    Flowing,
    Ended,
}

/// Converts interleaved audio of `channels` channels from one rate to another.
pub struct Resampler {
    /// Output frames per `down` source frames: the rates divided by their GCD.
    up: u64,
    down: u64,
    channels: usize,
    taps: usize,
    /// The cutoff as a share of the source rate's Nyquist.
    cutoff: f64,
    /// `up` phases of `taps` weights each, when tabulated.
    table: Option<Vec<f32>>,
    /// Held source frames; the first is source frame `held_from` of this run.
    held: Vec<f32>,
    held_from: u64,
    /// Source frames pushed so far in this run.
    pushed: u64,
    /// The next output frame of this run.
    next: u64,
}

impl Resampler {
    pub fn new(source_rate: u32, output_rate: u32, channels: usize) -> Self {
        let common = greatest_common_divisor(u64::from(source_rate), u64::from(output_rate)).max(1);
        let (up, down) = (
            u64::from(output_rate) / common,
            u64::from(source_rate) / common,
        );
        let ratio = (up as f64 / down as f64).min(1.0);
        // An even number of taps, the same reach on either side.
        let taps = ((FILTER_SIZE as f64 / ratio).ceil() as usize)
            .div_ceil(2)
            .max(1)
            * 2;
        let mut resampler = Self {
            up,
            down,
            channels: channels.max(1),
            taps,
            cutoff: CUTOFF * ratio,
            table: None,
            held: Vec::new(),
            held_from: 0,
            pushed: 0,
            next: 0,
        };
        if up as usize <= MOST_TABULATED_PHASES {
            let table = (0..up).flat_map(|phase| resampler.weights(phase)).collect();
            resampler.table = Some(table);
        }
        resampler
    }

    /// The `taps` weights for an output frame `phase / up` of the way past a source frame,
    /// covering source frames from `taps / 2 - 1` before it to `taps / 2` after. They sum to 1.
    fn weights(&self, phase: u64) -> Vec<f32> {
        let half = (self.taps / 2) as f64;
        let fraction = phase as f64 / self.up as f64;
        let window_scale = bessel_i0(KAISER_BETA);
        let raw: Vec<f64> = (0..self.taps)
            .map(|tap| {
                let distance = tap as f64 - (half - 1.0) - fraction;
                let x = std::f64::consts::PI * self.cutoff * distance;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                let reach = (distance / half).clamp(-1.0, 1.0);
                sinc * bessel_i0(KAISER_BETA * (1.0 - reach * reach).sqrt()) / window_scale
            })
            .collect();
        let sum: f64 = raw.iter().sum();
        raw.iter().map(|weight| (weight / sum) as f32).collect()
    }

    /// Output frames for `frames` source frames.
    fn outputs_for(&self, frames: u64) -> u64 {
        (frames * self.up).div_ceil(self.down)
    }

    /// Adds source frames and returns the output frames they complete.
    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        self.held.extend_from_slice(samples);
        self.pushed += (samples.len() / self.channels) as u64;
        self.produce(SourceState::Flowing)
    }

    /// The output frames still owed once the source has ended, treating the rest as silence.
    pub fn finish(&mut self) -> Vec<f32> {
        self.produce(SourceState::Ended)
    }

    fn produce(&mut self, source: SourceState) -> Vec<f32> {
        let half = (self.taps / 2) as u64;
        let owed = self.outputs_for(self.pushed);
        let mut out = Vec::new();
        let mut computed;
        while self.next_is_ready(source, owed, half) {
            let position = self.next * self.down;
            let (index, phase) = (position / self.up, position % self.up);
            let weights = match &self.table {
                Some(table) => &table[phase as usize * self.taps..(phase as usize + 1) * self.taps],
                None => {
                    computed = self.weights(phase);
                    &computed[..]
                }
            };
            let first = index as i64 - (half as i64 - 1);
            self.push_frame(weights, first, &mut out);
            self.next += 1;
        }
        self.release_read_frames(half);
        out
    }

    /// Whether the next output frame can be made: after the end, while frames are owed; before
    /// it, once the last source frame it reads is held.
    fn next_is_ready(&self, source: SourceState, owed: u64, half: u64) -> bool {
        match source {
            SourceState::Ended => self.next < owed,
            SourceState::Flowing => {
                let index = self.next * self.down / self.up;
                index + half < self.pushed
            }
        }
    }

    /// Pushes one output frame: per channel, the held source frames from `first` on weighted by
    /// `weights`. Frames already dropped or not yet held count as silence.
    #[inline]
    fn push_frame(&self, weights: &[f32], first: i64, out: &mut Vec<f32>) {
        let channels = self.channels;
        for channel in 0..channels {
            let mut sum = 0.0_f32;
            for (tap, weight) in weights.iter().enumerate() {
                let frame = first + tap as i64;
                if frame < self.held_from as i64 {
                    continue;
                }
                let at = (frame as u64 - self.held_from) as usize * channels + channel;
                if let Some(sample) = self.held.get(at) {
                    sum += sample * weight;
                }
            }
            out.push(sum);
        }
    }

    /// Drops source frames no later output reads.
    fn release_read_frames(&mut self, half: u64) {
        let channels = self.channels;
        let needed_from = ((self.next * self.down) / self.up).saturating_sub(half);
        if needed_from > self.held_from {
            let drop = ((needed_from - self.held_from) as usize).min(self.held.len() / channels);
            self.held.drain(..drop * channels);
            self.held_from += drop as u64;
        }
    }

    /// Output frames per source frame as `(up, down)`: output frame `k * up` falls on source
    /// frame `k * down`.
    pub fn grid(&self) -> (u64, u64) {
        (self.up, self.down)
    }

    /// Resets with nothing held, so the next source frame pushed lines up with the next output
    /// frame.
    pub fn reset(&mut self) {
        self.held.clear();
        self.held_from = 0;
        self.pushed = 0;
        self.next = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, frames: usize, frequency: f64) -> Vec<f32> {
        (0..frames)
            .map(|frame| {
                (std::f64::consts::TAU * frequency * frame as f64 / f64::from(rate)).sin() as f32
                    * 0.5
            })
            .collect()
    }

    #[test]
    fn a_tone_keeps_its_place_and_level_through_a_change_of_rate() {
        for (from, to) in [
            (44_100, 48_000),
            (22_050, 48_000),
            (96_000, 48_000),
            (8_000, 48_000),
        ] {
            let source = tone(from, from as usize, 440.0);
            let mut resampler = Resampler::new(from, to, 1);
            let mut out = Vec::new();
            for piece in source.chunks(1000) {
                out.extend(resampler.push(piece));
            }
            out.extend(resampler.finish());
            assert_eq!(out.len(), to as usize, "{from} -> {to}");
            let expected = tone(to, to as usize, 440.0);
            // Away from the ends, where the source stops, the output is the tone at the new rate.
            let worst = out[200..out.len() - 200]
                .iter()
                .zip(&expected[200..])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(worst < 2e-4, "{from} -> {to}: {worst}");
        }
    }

    #[test]
    fn a_run_started_on_the_shared_grid_matches_a_run_from_the_start() {
        let source = tone(44_100, 44_100, 1000.0);
        let mut whole = Resampler::new(44_100, 48_000, 1);
        let mut all = whole.push(&source);
        all.extend(whole.finish());
        let (up, down) = whole.grid();
        assert_eq!((up, down), (160, 147));
        // From source frame 147 × 100 = output frame 160 × 100.
        let mut part = Resampler::new(44_100, 48_000, 1);
        let late = part.push(&source[147 * 100..]);
        // Past the filter's reach the two agree to the bit.
        assert_eq!(&late[64..4000], &all[16_000 + 64..16_000 + 4000]);
    }

    #[test]
    fn channels_stay_apart() {
        let left = tone(44_100, 4410, 440.0);
        let interleaved: Vec<f32> = left.iter().flat_map(|sample| [*sample, -*sample]).collect();
        let mut resampler = Resampler::new(44_100, 48_000, 2);
        let mut out = resampler.push(&interleaved);
        out.extend(resampler.finish());
        assert_eq!(out.len(), 4800 * 2);
        assert!(out.chunks(2).all(|frame| frame[0] == -frame[1]));
    }
}
