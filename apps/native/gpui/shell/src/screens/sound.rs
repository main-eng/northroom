//! Plays a preview's sound on the system output, as the core configures every player
//! (`nr_sound_output`). The output calls a render function, on its own thread, that the core
//! fills with the next interleaved frames. A silent run (`NR_PREVIEW_SILENT`) sends nothing to
//! the output but still pulls samples on its own clock, so a check can read what would play.
//!
//! On a Mac the output is the default output unit (Audio Toolbox). Other platforms have none
//! yet, and starting one reports that in the core's words.

use northroom_core::preview::sound_output::{NrSoundOutput, nr_sound_output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Fills the buffer with the next interleaved frames. Its length is the frames requested times
/// the output's channels. Called from any thread.
pub type Render = Arc<dyn Fn(&mut [f32]) + Send + Sync>;

fn rule() -> &'static NrSoundOutput {
    static RULE: OnceLock<NrSoundOutput> = OnceLock::new();
    RULE.get_or_init(|| nr_sound_output())
}

pub struct SoundOutput {
    render: Render,
    running: Option<Running>,
}

enum Running {
    /// A silent run's clock, pulling the samples it would have played.
    Pull(Arc<AtomicBool>),
    /// The platform output, held while it plays and stopped on drop.
    #[cfg(target_os = "macos")]
    Output { _unit: mac::Output },
}

impl SoundOutput {
    pub fn new(render: Render) -> Self {
        Self {
            render,
            running: None,
        }
    }

    /// Whether samples are being pulled, for a check.
    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    pub fn follow(&mut self, playing: bool) {
        if playing && self.running.is_none() {
            self.start();
        } else if !playing && self.running.is_some() {
            self.stop();
        }
    }

    pub fn stop(&mut self) {
        if let Some(Running::Pull(stopped)) = self.running.take() {
            stopped.store(true, Ordering::SeqCst);
        }
    }

    fn start(&mut self) {
        let rule = rule();
        let render = self.render.clone();
        if rule.silent {
            let stopped = Arc::new(AtomicBool::new(false));
            let seen = stopped.clone();
            let (seconds, frames) = (rule.pull_seconds, rule.pull_frames as usize);
            let mut scratch = vec![0.0; frames * rule.channels as usize];
            std::thread::spawn(move || {
                // Keep a fixed beat like a repeating timer, so a late pull doesn't delay the next.
                let every = Duration::from_secs_f64(seconds);
                let mut due = Instant::now() + every;
                while !seen.load(Ordering::SeqCst) {
                    std::thread::sleep(due.saturating_duration_since(Instant::now()));
                    due += every;
                    if !seen.load(Ordering::SeqCst) {
                        render(&mut scratch);
                    }
                }
            });
            self.running = Some(Running::Pull(stopped));
            return;
        }
        #[cfg(target_os = "macos")]
        match mac::Output::start(render, rule) {
            Ok(output) => self.running = Some(Running::Output { _unit: output }),
            Err(status) => said_no(&format!("OSStatus {status}")),
        }
        #[cfg(not(target_os = "macos"))]
        {
            drop(render);
            said_no("this platform has no sound output in this shell yet");
        }
    }
}

impl Drop for SoundOutput {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Reports, in the core's words, that the output could not start.
fn said_no(detail: &str) {
    let detail = kit::core::c(detail);
    // SAFETY: a NUL-terminated detail.
    let line = kit::core::text(&unsafe {
        northroom_core::page::host::ffi::nr_host_problem(
            northroom_core::page::host::ffi::NR_HOST_SOUND_OUTPUT,
            detail.as_ptr(),
            0,
            0,
        )
    });
    eprintln!("{line}");
}

#[cfg(target_os = "macos")]
mod mac {
    //! The default output unit, fed from its own thread by a render callback.

    use super::Render;
    use northroom_core::preview::sound_output::NrSoundOutput;
    use std::ffi::c_void;

    type OsStatus = i32;
    type AudioUnit = *mut c_void;

    const OUTPUT: u32 = u32::from_be_bytes(*b"auou");
    const DEFAULT_OUTPUT: u32 = u32::from_be_bytes(*b"def ");
    const APPLE: u32 = u32::from_be_bytes(*b"appl");
    const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
    const FLOAT: u32 = 1;
    const PACKED: u32 = 8;
    const STREAM_FORMAT: u32 = 8;
    const SET_RENDER_CALLBACK: u32 = 23;
    const INPUT_SCOPE: u32 = 1;
    /// The system has no output unit to start: the Component Manager's `badComponentType`.
    const NO_COMPONENT: OsStatus = -2005;

    #[repr(C)]
    struct ComponentDescription {
        kind: u32,
        sub_kind: u32,
        maker: u32,
        flags: u32,
        flags_mask: u32,
    }

    #[repr(C)]
    struct StreamDescription {
        rate: f64,
        format: u32,
        flags: u32,
        bytes_per_packet: u32,
        frames_per_packet: u32,
        bytes_per_frame: u32,
        channels: u32,
        bits: u32,
        reserved: u32,
    }

    #[repr(C)]
    struct Buffer {
        channels: u32,
        bytes: u32,
        data: *mut c_void,
    }

    #[repr(C)]
    struct BufferList {
        count: u32,
        buffers: [Buffer; 1],
    }

    type Callback =
        extern "C" fn(*mut c_void, *mut u32, *const c_void, u32, u32, *mut BufferList) -> OsStatus;

    #[repr(C)]
    struct CallbackStruct {
        callback: Callback,
        context: *mut c_void,
    }

    #[link(name = "AudioToolbox", kind = "framework")]
    unsafe extern "C" {
        fn AudioComponentFindNext(
            after: *mut c_void,
            description: *const ComponentDescription,
        ) -> *mut c_void;
        fn AudioComponentInstanceNew(component: *mut c_void, unit: *mut AudioUnit) -> OsStatus;
        fn AudioComponentInstanceDispose(unit: AudioUnit) -> OsStatus;
        fn AudioUnitSetProperty(
            unit: AudioUnit,
            property: u32,
            scope: u32,
            element: u32,
            data: *const c_void,
            size: u32,
        ) -> OsStatus;
        fn AudioUnitInitialize(unit: AudioUnit) -> OsStatus;
        fn AudioUnitUninitialize(unit: AudioUnit) -> OsStatus;
        fn AudioOutputUnitStart(unit: AudioUnit) -> OsStatus;
        fn AudioOutputUnitStop(unit: AudioUnit) -> OsStatus;
    }

    /// What the callback reads: the render function, the largest request size and the output's
    /// channels.
    struct Feed {
        render: Render,
        longest: usize,
        channels: usize,
        scratch: Vec<f32>,
    }

    pub struct Output {
        unit: AudioUnit,
        feed: *mut Feed,
    }

    extern "C" fn fill(
        context: *mut c_void,
        _: *mut u32,
        _: *const c_void,
        _: u32,
        frames: u32,
        list: *mut BufferList,
    ) -> OsStatus {
        // SAFETY: the feed is the one `start` gave the unit and lives until the unit is disposed.
        // The list is the unit's single interleaved buffer for this call.
        let (feed, list) = unsafe { (&mut *context.cast::<Feed>(), &mut *list) };
        let frames = frames as usize;
        let given = frames.min(feed.longest);
        let wanted = given * feed.channels;
        (feed.render)(&mut feed.scratch[..wanted]);
        let buffer = &mut list.buffers[0];
        let length = buffer.bytes as usize / std::mem::size_of::<f32>();
        // SAFETY: the unit's buffer holds `bytes` bytes of floats.
        let out = unsafe { std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), length) };
        let copied = wanted.min(length);
        out[..copied].copy_from_slice(&feed.scratch[..copied]);
        out[copied..].fill(0.0);
        0
    }

    impl Output {
        pub fn start(render: Render, rule: &NrSoundOutput) -> Result<Self, OsStatus> {
            let channels = rule.channels as usize;
            let longest = rule.longest as usize;
            let feed = Box::into_raw(Box::new(Feed {
                render,
                longest,
                channels,
                scratch: vec![0.0; longest * channels],
            }));
            let description = ComponentDescription {
                kind: OUTPUT,
                sub_kind: DEFAULT_OUTPUT,
                maker: APPLE,
                flags: 0,
                flags_mask: 0,
            };
            let mut unit: AudioUnit = std::ptr::null_mut();
            // SAFETY: plain Audio Toolbox calls with values that outlive each call. The feed lives
            // until `drop` has disposed of the unit.
            unsafe {
                let component = AudioComponentFindNext(std::ptr::null_mut(), &description);
                if component.is_null() {
                    drop(Box::from_raw(feed));
                    return Err(NO_COMPONENT);
                }
                let status = AudioComponentInstanceNew(component, &mut unit);
                if status != 0 {
                    drop(Box::from_raw(feed));
                    return Err(status);
                }
                let output = Self { unit, feed };
                let frame_bytes = (channels * std::mem::size_of::<f32>()) as u32;
                let format = StreamDescription {
                    rate: f64::from(rule.rate),
                    format: LINEAR_PCM,
                    flags: FLOAT | PACKED,
                    bytes_per_packet: frame_bytes,
                    frames_per_packet: 1,
                    bytes_per_frame: frame_bytes,
                    channels: rule.channels,
                    bits: 32,
                    reserved: 0,
                };
                let callback = CallbackStruct {
                    callback: fill,
                    context: feed.cast(),
                };
                for status in [
                    AudioUnitSetProperty(
                        unit,
                        STREAM_FORMAT,
                        INPUT_SCOPE,
                        0,
                        (&raw const format).cast(),
                        std::mem::size_of::<StreamDescription>() as u32,
                    ),
                    AudioUnitSetProperty(
                        unit,
                        SET_RENDER_CALLBACK,
                        INPUT_SCOPE,
                        0,
                        (&raw const callback).cast(),
                        std::mem::size_of::<CallbackStruct>() as u32,
                    ),
                    AudioUnitInitialize(unit),
                    AudioOutputUnitStart(unit),
                ] {
                    if status != 0 {
                        return Err(status);
                    }
                }
                Ok(output)
            }
        }
    }

    impl Drop for Output {
        fn drop(&mut self) {
            // SAFETY: the unit and feed come from `start`. The unit is stopped and disposed of
            // before the feed its callback reads is dropped.
            unsafe {
                AudioOutputUnitStop(self.unit);
                AudioUnitUninitialize(self.unit);
                AudioComponentInstanceDispose(self.unit);
                drop(Box::from_raw(self.feed));
            }
        }
    }
}
