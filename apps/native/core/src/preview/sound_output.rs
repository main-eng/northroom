//! How a shell plays preview sound: the rate and channel count to open the system output with,
//! the most frames it takes at once, and whether this run is silent (`launch::Launch::quiet`).
//! The system output requests samples on its own thread. A silent run sends nothing to it: the
//! shell pulls `pull_frames` every `pull_seconds` on its own clock and drops them, so a check
//! can read what would have played.

/// 48 kHz.
pub const RATE: u32 = 48_000;
/// Two channels, interleaved.
pub const CHANNELS: u32 = 2;
/// The most frames the system's output is handed at once.
const LONGEST: u32 = 8192;
/// How often a silent run pulls samples, and how much it pulls each time.
const SILENT_PULL_SECONDS: f64 = 0.01;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NrSoundOutput {
    pub rate: u32,
    pub channels: u32,
    pub longest: u32,
    pub silent: bool,
    pub pull_seconds: f64,
    pub pull_frames: u32,
}

/// The output settings for a run, `silent` or not.
pub fn output(silent: bool) -> NrSoundOutput {
    NrSoundOutput {
        rate: RATE,
        channels: CHANNELS,
        longest: LONGEST,
        silent,
        pull_seconds: SILENT_PULL_SECONDS,
        pull_frames: (f64::from(RATE) * SILENT_PULL_SECONDS).round() as u32,
    }
}

/// This run's output.
#[no_mangle]
pub extern "C" fn nr_sound_output() -> NrSoundOutput {
    output(crate::launch::get().quiet())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_run_pulls_ten_milliseconds_at_a_time() {
        let silent = output(true);
        assert_eq!(
            (silent.rate, silent.channels, silent.silent),
            (48_000, 2, true)
        );
        assert_eq!(silent.pull_frames, 480);
        assert!(!output(false).silent);
    }
}
