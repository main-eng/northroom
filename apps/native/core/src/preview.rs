//! The preview: one thread that decodes, composites and presents to a layer the shell owns. The
//! shell never waits on it. Commands change `State` under a lock and wake the thread, which
//! decodes through the platform (`media`), draws the frame into the working texture on the same
//! GPU device, and presents through the grade. On Windows and Linux there is no native layer
//! under the shell's window, so it presents into an image the shell draws in its own frame
//! (`Preview::picture`).

pub(crate) mod audio;
pub mod details;
pub mod player;
pub(crate) mod present;
pub mod sound_output;

use crate::media::{DecodedFrame, MediaError, MediaResult, VideoInfo, VideoReader, VideoSource};
use crate::preview::audio::{AudioStats, AudioTrack};
use crate::preview::player::video_controls::{fullscreen_after, Chosen, FullscreenAsk, Menu};
use crate::preview::present::PresentPass;
#[cfg(target_os = "macos")]
use crate::reconfiguring;
use crate::Relock;
use crate::{drawing, gpu};
use northroom_media::color::{ChromaSiting, ColorDescription};
use northroom_media::gpu::{Gpu, InputTransform, WorkingFormat};
use player::{Anchor, Key, KeyAction};
use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// HDR clips present in half-float extended linear sRGB, so values above 1.0 reach the display.
/// Everything else presents in 8-bit sRGB, at half the memory per drawable. Both are full-chroma
/// RGB.
const HDR_SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const SDR_SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
/// One frame queued behind the one on screen: two drawables, not three.
#[cfg(target_os = "macos")]
const FRAME_LATENCY: u32 = 1;
/// How long to wait before presenting again when the layer gave no drawable.
#[cfg(target_os = "macos")]
const RETRY_PRESENT_AFTER: Duration = Duration::from_millis(8);
/// The longest the thread sleeps before rechecking commands while playing.
const LONGEST_PLAY_WAIT: Duration = Duration::from_millis(50);

/// The working texture matches the presentation format: half floats for an HDR clip on a macOS
/// extended-range layer, and 8-bit sRGB for everything shown in 8-bit sRGB (always the case on
/// Windows and Linux), which halves the memory with identical pixels.
fn working_format(hdr: bool) -> WorkingFormat {
    if hdr && cfg!(target_os = "macos") {
        WorkingFormat::Float
    } else {
        WorkingFormat::Srgb8
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewInfo {
    pub duration_seconds: f64,
    pub position_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub playing: bool,
    pub hdr: bool,
    pub ready: bool,
    /// Playback reached the end and stopped; playing again restarts from the beginning.
    pub ended: bool,
    pub has_audio: bool,
    pub muted: bool,
    pub volume: f32,
    /// Playback rate; 1 is normal speed.
    pub rate: f64,
    /// The picture fills the screen.
    pub fullscreen: bool,
    /// It has played since the file was opened; a played file counts as read.
    pub has_played: bool,
}

impl PreviewInfo {
    /// Whether the sound output should run: while a file with sound plays, even when muted.
    pub fn output_wanted(&self) -> bool {
        self.ready && self.has_audio && self.playing
    }
}

impl Default for PreviewInfo {
    fn default() -> Self {
        Self {
            duration_seconds: 0.0,
            position_seconds: 0.0,
            width: 0,
            height: 0,
            playing: false,
            hdr: false,
            ready: false,
            ended: false,
            has_audio: false,
            muted: false,
            volume: 0.0,
            rate: 1.0,
            fullscreen: false,
            has_played: false,
        }
    }
}

/// A seek requested before the duration is known, applied once it is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pending {
    /// A fraction of the duration, from a press on the scrub bar.
    Share(f64),
    /// A key that moves through the file (Left, Right, Home, End).
    Key(Key),
}

struct State {
    playing: bool,
    seek: Option<f64>,
    /// Seeks waiting for the duration, in request order.
    pending: Vec<Pending>,
    /// Play was requested since the file was opened.
    has_played: bool,
    /// The shell's layer, once provided. The thread prepares everything else beforehand.
    layer: Option<usize>,
    size: (u32, u32),
    resized: bool,
    /// Present the working texture again without a new frame.
    redraw: bool,
    quit: bool,
    info: PreviewInfo,
    error: Option<String>,
    ended: bool,
    /// The playback clock: the start instant, start media time and rate. The thread publishes it
    /// and the audio reads it.
    anchor: Option<Anchor>,
    /// The speed chosen in the menu's "Playback speed".
    rate: f64,
    /// The overflow (three dots) menu.
    menu: Menu,
    /// The picture fills the screen.
    fullscreen: bool,
    muted: bool,
    volume: f32,
    /// A scrub drag paused playback, which resumes when the drag ends.
    resume_after_scrub: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    /// The last presented frame on Windows and Linux, under its own lock, which the shell takes
    /// each time it draws.
    #[cfg(not(target_os = "macos"))]
    picture: Mutex<Option<Arc<Picture>>>,
}

/// A presented frame on Windows and Linux, for the shell to draw where a layer would be: the
/// layer's size in pixels, with the picture fitted on black as the present pass places it, in
/// sRGB BGRA. `number` counts presented frames, so the shell rebuilds its image only for a new
/// one.
#[cfg(not(target_os = "macos"))]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub number: u64,
}

pub struct Preview {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// The file's audio, when it has any and the preview should play it.
    audio: Option<AudioTrack>,
    /// The file being played, for the menu's Download.
    path: String,
}

/// What a preview plays.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Playing {
    /// A video file, played like a browser's player: picture and sound, stopping at the end.
    Video,
    /// A sound file: no picture and no layer.
    Sound,
}

impl Preview {
    /// Opens `path` paused at `start_at` seconds (a page player resuming where it was left), shown
    /// as soon as the file is read.
    pub fn start_as(path: String, what: Playing, start_at: f64) -> Self {
        let audio = AudioTrack::open(&path);
        let sound_seconds = audio.as_ref().map(AudioTrack::duration_seconds);
        // `NR_AUTOPLAY` starts playing without a key press, for unattended checks.
        let playing = std::env::var_os("NR_AUTOPLAY").is_some();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                playing,
                seek: (start_at > 0.0).then_some(start_at),
                pending: Vec::new(),
                has_played: playing,
                layer: None,
                size: (1, 1),
                resized: false,
                redraw: false,
                quit: false,
                info: PreviewInfo {
                    has_audio: audio.is_some(),
                    volume: 1.0,
                    ..PreviewInfo::default()
                },
                error: None,
                ended: false,
                anchor: None,
                rate: 1.0,
                menu: Menu::Closed,
                fullscreen: false,
                muted: false,
                volume: 1.0,
                resume_after_scrub: false,
            }),
            wake: Condvar::new(),
            #[cfg(not(target_os = "macos"))]
            picture: Mutex::new(None),
        });
        let thread_shared = Arc::clone(&shared);
        let file = path.clone();
        let thread = std::thread::Builder::new()
            .name("northroom-preview".into())
            .spawn(move || {
                let ran = if what == Playing::Sound {
                    run_sound(sound_seconds, &thread_shared);
                    Ok(())
                } else {
                    run(&path, &thread_shared)
                };
                if let Err(error) = ran {
                    eprintln!("preview stopped: {error}");
                    thread_shared.state.relock().error = Some(error.to_string());
                }
            })
            .expect("start the preview thread");
        Self {
            shared,
            thread: Some(thread),
            audio,
            path: file,
        }
    }

    /// The file being played.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Plays at `rate` times normal speed from now on. The clock continues from its current
    /// position and the audio follows at that speed.
    pub fn set_rate(&self, rate: f64) {
        let rate = player::playback_rate(rate);
        let mut state = self.shared.state.relock();
        state.rate = rate;
        state.info.rate = rate;
        if let Some(anchor) = state.anchor {
            state.anchor = Some(anchor.at_rate(Instant::now(), rate));
        }
        if let Some(audio) = &self.audio {
            audio.set_rate(rate);
        }
        self.shared.wake.notify_all();
    }

    /// The overflow menu's current state.
    pub fn menu(&self) -> Menu {
        self.shared.state.relock().menu
    }

    /// Handles a press on the three dots: opens the menu, or closes whatever is open.
    pub fn toggle_menu(&self) {
        let mut state = self.shared.state.relock();
        state.menu = state.menu.toggled();
    }

    /// Applies the chosen menu row and returns what is left for the shell (a copy to save). A
    /// chosen speed takes effect.
    fn follow(&self, menu: Menu, chosen: Chosen) -> Chosen {
        self.shared.state.relock().menu = menu;
        if let Chosen::Rate(rate) = chosen {
            self.set_rate(rate);
        }
        chosen
    }

    /// Handles a press at (`x`, `y`) on a `width` by `height` picture while the menu is open.
    pub fn press_menu(&self, x: f64, y: f64, width: f64, height: f64) -> Chosen {
        let (menu, rate) = {
            let state = self.shared.state.relock();
            (state.menu, state.rate)
        };
        let (menu, chosen) = menu.pressed(x, y, rate, width, height);
        self.follow(menu, chosen)
    }

    /// Handles a key while the menu is open. `None` when the menu is closed or ignores the key.
    pub fn menu_key(&self, key: Key, width: f64, height: f64) -> Option<Chosen> {
        let (menu, rate) = {
            let state = self.shared.state.relock();
            (state.menu, state.rate)
        };
        let (menu, chosen) = menu.key(key, rate, width, height)?;
        Some(self.follow(menu, chosen))
    }

    /// Handles the wheel over the open menu.
    pub fn scroll_menu(&self, down: f64, width: f64, height: f64) {
        let mut state = self.shared.state.relock();
        state.menu = state.menu.scrolled(down, width, height);
    }

    /// The full screen button, a double click or Escape. Returns whether the picture is full screen
    /// from now on; the shell does the resizing.
    pub fn ask_fullscreen(&self, ask: FullscreenAsk) -> bool {
        let mut state = self.shared.state.relock();
        state.fullscreen = fullscreen_after(state.fullscreen, ask);
        state.info.fullscreen = state.fullscreen;
        state.fullscreen
    }

    /// The last presented frame on Windows and Linux (`Picture`).
    #[cfg(not(target_os = "macos"))]
    pub fn picture(&self) -> Option<Arc<Picture>> {
        self.shared.picture.relock().clone()
    }

    /// Presents to `layer`, a `CAMetalLayer` the shell keeps alive until `stop`. On Windows and
    /// Linux `layer` is null and frames go into a `width` × `height` image (`picture`).
    pub fn attach(&self, layer: *mut c_void, width: u32, height: u32) {
        let mut state = self.shared.state.relock();
        state.layer = Some(layer as usize);
        state.size = (width.max(1), height.max(1));
        state.resized = true;
        self.shared.wake.notify_all();
    }

    pub fn set_playing(&self, playing: bool) {
        let mut state = self.shared.state.relock();
        if state.playing != playing {
            state.playing = playing;
            state.has_played |= playing;
            // Playing again from the end starts from the beginning.
            if playing && std::mem::take(&mut state.ended) {
                state.seek = Some(0.0);
            }
            self.shared.wake.notify_all();
        }
    }

    pub fn toggle_playing(&self) {
        let playing = self.shared.state.relock().playing;
        self.set_playing(!playing);
    }

    pub fn set_muted(&self, muted: bool) {
        let mut state = self.shared.state.relock();
        state.muted = muted;
        state.info.muted = muted;
    }

    pub fn set_volume(&self, volume: f32) {
        let mut state = self.shared.state.relock();
        state.volume = volume.clamp(0.0, 1.0);
        state.info.volume = state.volume;
    }

    /// Starts a scrub drag, pausing playback until `end_scrub`.
    pub fn begin_scrub(&self) {
        let mut state = self.shared.state.relock();
        state.resume_after_scrub = state.playing;
        if state.playing {
            state.playing = false;
            self.shared.wake.notify_all();
        }
    }

    pub fn end_scrub(&self) {
        let resume = std::mem::take(&mut self.shared.state.relock().resume_after_scrub);
        if resume {
            self.set_playing(true);
        }
    }

    /// Handles `key` as a browser's player would. Returns whether the player used it.
    pub fn key(&self, key: Key) -> bool {
        if key == Key::Escape {
            // Escape closes an open menu first, then leaves full screen; otherwise the player
            // ignores it.
            let mut state = self.shared.state.relock();
            if state.menu.is_open() {
                state.menu = Menu::Closed;
                return true;
            }
            let filling = std::mem::take(&mut state.fullscreen);
            state.info.fullscreen = false;
            return filling;
        }
        let info = self.info();
        if !info.ready && matches!(key, Key::Left | Key::Right | Key::Home | Key::End) {
            // The target is known once the duration is.
            self.shared.state.relock().pending.push(Pending::Key(key));
            return true;
        }
        match player::key_action(
            key,
            info.position_seconds,
            info.duration_seconds,
            info.volume,
        ) {
            Some(KeyAction::TogglePlaying) => self.toggle_playing(),
            Some(KeyAction::Seek(seconds)) => self.seek(seconds),
            Some(KeyAction::Volume(volume)) => {
                self.set_volume(volume);
                // Raising the volume of a muted player unmutes it, as a browser's does.
                self.set_muted(volume == 0.0);
            }
            None => return false,
        }
        true
    }

    /// Fills `out` (interleaved stereo at 48 kHz) with audio at the preview's clock, or silence
    /// when paused, silent or muted. Called on the output's thread.
    pub fn render_audio(&self, out: &mut [f32]) {
        let Some(audio) = &self.audio else {
            out.fill(0.0);
            return;
        };
        let (clock, gain) = {
            let state = self.shared.state.relock();
            let clock = state
                .anchor
                .filter(|_| state.playing && state.seek.is_none())
                .map(|anchor| anchor.position(Instant::now()));
            (clock, if state.muted { 0.0 } else { state.volume })
        };
        audio.render(clock, gain, out);
    }

    pub fn audio_stats(&self) -> AudioStats {
        self.audio
            .as_ref()
            .map(AudioTrack::stats)
            .unwrap_or_default()
    }

    /// Shows the frame at `seconds`. A newer request replaces one not yet started.
    pub fn seek(&self, seconds: f64) {
        let mut state = self.shared.state.relock();
        state.ended = false;
        state.info.ended = false;
        state.seek = Some(seconds);
        state.pending.clear();
        self.shared.wake.notify_all();
    }

    /// Seeks to fraction `share` of the file, immediately if the duration is known, else as soon as
    /// it is. Does nothing if the file has no duration.
    pub fn seek_share(&self, share: f64) {
        let mut state = self.shared.state.relock();
        if state.info.ready {
            let duration = state.info.duration_seconds;
            drop(state);
            if let Some(seconds) = player::seconds_at_share(share, duration) {
                self.seek(seconds);
            }
            return;
        }
        state.pending.push(Pending::Share(share));
    }

    pub fn resize(&self, width: u32, height: u32) {
        let mut state = self.shared.state.relock();
        let size = (width.max(1), height.max(1));
        if state.size != size {
            state.size = size;
            state.resized = true;
            self.shared.wake.notify_all();
        }
    }

    /// The state the shell displays. `playing` and `ended` reflect the last request, not what the
    /// thread last processed, so a pause shows immediately.
    pub fn info(&self) -> PreviewInfo {
        let state = self.shared.state.relock();
        PreviewInfo {
            playing: state.playing,
            ended: state.ended,
            has_played: state.has_played,
            ..state.info
        }
    }

    pub fn stop(mut self) {
        self.shared.state.relock().quit = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Copies `source` to `target`, as the menu's Download does. The copy is written beside the
/// target under a hidden name and renamed over it only when complete, so an existing file is
/// replaced atomically and a failed copy leaves nothing behind. A target equal to the source is
/// left alone.
pub fn copy_replacing(source: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    let same = |path: &std::path::Path| std::fs::canonicalize(path).ok();
    if same(source).is_some() && same(source) == same(target) {
        return Ok(());
    }
    let folder = target.parent().unwrap_or(std::path::Path::new("."));
    let whole = folder.join(format!(".{}.download", uuid::Uuid::new_v4()));
    let copied = std::fs::copy(source, &whole).and_then(|_| std::fs::rename(&whole, target));
    if copied.is_err() {
        let _ = std::fs::remove_file(&whole);
    }
    copied
}

/// Applies seeks requested before the duration was known, now that it is `duration`.
fn settle_pending(state: &mut State, duration: f64) {
    for pending in std::mem::take(&mut state.pending) {
        let at = state.seek.unwrap_or(0.0);
        let seconds = match pending {
            Pending::Share(share) => player::seconds_at_share(share, duration),
            Pending::Key(key) => match player::key_action(key, at, duration, state.volume) {
                Some(KeyAction::Seek(seconds)) => Some(seconds),
                _ => None,
            },
        };
        if let Some(seconds) = seconds {
            state.seek = Some(seconds);
        }
    }
}

/// How often a playing sound file's position is published.
const SOUND_POSITION_EVERY: Duration = Duration::from_millis(50);

/// Advances a sound file's clock for one check of the player state: follows a seek, starts or
/// stops with `playing`, and stops at the end. Returns the position to publish.
fn step_sound(state: &mut State, position: f64, duration: f64, now: Instant) -> f64 {
    let mut position = state.anchor.map_or(position, |anchor| anchor.position(now));
    if let Some(seconds) = state.seek.take() {
        position = seconds.clamp(0.0, duration);
        state.anchor = None;
    }
    if state.playing && position >= duration {
        position = duration;
        state.playing = false;
        state.ended = true;
    }
    if state.playing {
        let rate = state.rate;
        state.anchor.get_or_insert(Anchor {
            at: now,
            from: position,
            rate,
        });
    } else {
        state.anchor = None;
    }
    state.info.position_seconds = position;
    state.info.playing = state.playing;
    state.info.ended = state.ended;
    position
}

/// A sound file has no picture, so its thread only keeps the clock that drives the audio.
fn run_sound(duration_seconds: Option<f64>, shared: &Shared) {
    let duration = duration_seconds
        .filter(|seconds| seconds.is_finite())
        .unwrap_or(0.0);
    let mut state = shared.state.relock();
    state.info.duration_seconds = duration;
    state.info.ready = true;
    settle_pending(&mut state, duration);
    let mut position = 0.0;
    loop {
        if state.quit {
            return;
        }
        position = step_sound(&mut state, position, duration, Instant::now());
        state = if state.playing {
            shared
                .wake
                .wait_timeout(state, SOUND_POSITION_EVERY)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0
        } else {
            shared
                .wake
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        };
    }
}

/// Which presented frame `NR_SNAPSHOT_DIR` saves, far enough in to be past start-up.
const SNAPSHOT_FRAME: u32 = 20;

/// Draws the working texture through the present pass into an 8-bit sRGB texture the size of
/// the layer and writes it as a PPM, so a run can be checked without a screen capture. The file
/// holds exactly what the layer receives.
fn write_snapshot(
    gpu: &Gpu,
    working_view: &wgpu::TextureView,
    size: (u32, u32),
    source_size: (u32, u32),
    path: &str,
) -> MediaResult<()> {
    let pass = PresentPass::new(&gpu.device, SDR_SURFACE_FORMAT);
    let bind = pass.bind(&gpu.device, working_view);
    let bgra = read_presented(gpu, &pass, &bind, size, source_size)?;
    let mut ppm = format!("P6\n{} {}\n255\n", size.0, size.1).into_bytes();
    for &[blue, green, red, _] in bgra.as_chunks::<4>().0 {
        ppm.extend_from_slice(&[red, green, blue]);
    }
    std::fs::write(path, ppm).map_err(|cause| MediaError(format!("write {path}: {cause}")))
}

/// What `pass` (drawing in `SDR_SURFACE_FORMAT`) presents of the working texture `bind` reads,
/// for a layer of `size` pixels: the pixels in row order, sRGB BGRA.
fn read_presented(
    gpu: &Gpu,
    pass: &PresentPass,
    bind: &wgpu::BindGroup,
    size: (u32, u32),
    source_size: (u32, u32),
) -> MediaResult<Vec<u8>> {
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("presented"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SDR_SURFACE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row_bytes = (size.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("presented"),
        size: u64::from(row_bytes) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut commands = gpu.device.create_command_encoder(&Default::default());
    pass.draw(
        &mut commands,
        bind,
        &target.create_view(&Default::default()),
        size,
        source_size,
    );
    commands.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: None,
            },
        },
        extent,
    );
    let submission = gpu.queue.submit([commands.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    gpu.wait_for(Some(submission))?;
    gpu.wait_for(None)?;
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|cause| MediaError(format!("read the presented frame back: {cause}")))?;
    let mut bgra = Vec::with_capacity((size.0 * size.1 * 4) as usize);
    for row in mapped.chunks(row_bytes as usize) {
        bgra.extend_from_slice(&row[..(size.0 * 4) as usize]);
    }
    Ok(bgra)
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => fraction * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + fraction / 1024.0) * 2f32.powi(exponent - 15),
    }
}

/// For an HDR clip, draws what the layer receives into a half-float texture and prints its
/// largest value. Above 1.0 means highlights brighter than SDR white reach the layer.
fn report_hdr_range(
    gpu: &Gpu,
    working_view: &wgpu::TextureView,
    size: (u32, u32),
) -> MediaResult<()> {
    let pass = PresentPass::new(&gpu.device, HDR_SURFACE_FORMAT);
    let bind = pass.bind(&gpu.device, working_view);
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hdr range"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_SURFACE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row_bytes = (size.0 * 8).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hdr range"),
        size: u64::from(row_bytes) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut commands = gpu.device.create_command_encoder(&Default::default());
    pass.draw(
        &mut commands,
        &bind,
        &target.create_view(&Default::default()),
        size,
        size,
    );
    commands.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: None,
            },
        },
        extent,
    );
    let submission = gpu.queue.submit([commands.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    gpu.wait_for(Some(submission))?;
    gpu.wait_for(None)?;
    let mapped = buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|cause| MediaError(format!("read the HDR range back: {cause}")))?;
    let mut largest = 0.0_f32;
    let mut above_white = 0_u64;
    for row in mapped.chunks(row_bytes as usize) {
        for pixel in row[..(size.0 * 8) as usize].chunks(8) {
            let green = half_to_f32(u16::from_le_bytes([pixel[2], pixel[3]]));
            largest = largest.max(green);
            above_white += u64::from(green > 1.0);
        }
    }
    let share = above_white as f64 / (f64::from(size.0) * f64::from(size.1)) * 100.0;
    eprintln!(
        "hdr range: largest value {largest:.2} x SDR white ({:.0} nits); {share:.0}% of pixels above SDR white",
        f64::from(largest) * 203.0
    );
    Ok(())
}

/// What the thread does next, decided under the lock.
struct Work {
    seek: Option<f64>,
    playing: bool,
    resized: Option<(u32, u32)>,
    rate: f64,
}

#[cfg(target_os = "macos")]
fn surface_error(action: &str, cause: impl std::fmt::Display) -> MediaError {
    MediaError(format!("{action}: {cause}"))
}

/// The size a `source` picture is shown at inside a `layer`-pixel layer: fitted whole, as the
/// present pass places it.
fn fitted_size(source: (u32, u32), layer: (u32, u32)) -> (u32, u32) {
    let scale = (f64::from(layer.0) / f64::from(source.0.max(1)))
        .min(f64::from(layer.1) / f64::from(source.1.max(1)));
    let side = |length: u32| ((f64::from(length) * scale).round() as u32).max(1);
    (side(source.0), side(source.1))
}

#[cfg(target_os = "macos")]
fn surface_format(hdr: bool) -> wgpu::TextureFormat {
    if hdr {
        HDR_SURFACE_FORMAT
    } else {
        SDR_SURFACE_FORMAT
    }
}

#[cfg(target_os = "macos")]
fn configure(
    surface: &wgpu::Surface<'_>,
    gpu: &Gpu,
    size: (u32, u32),
    hdr: bool,
) -> MediaResult<()> {
    let mut config = surface
        .get_default_config(&gpu.adapter, size.0, size.1)
        .ok_or_else(|| MediaError("the preview layer is not usable on this GPU".into()))?;
    config.format = surface_format(hdr);
    config.color_space = if hdr {
        wgpu::SurfaceColorSpace::ExtendedSrgbLinear
    } else {
        wgpu::SurfaceColorSpace::Srgb
    };
    config.desired_maximum_frame_latency = FRAME_LATENCY;
    config.present_mode = wgpu::PresentMode::Fifo;
    config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
    let _alone = reconfiguring();
    surface.configure(&gpu.device, &config);
    Ok(())
}

/// The surface on the shell's `layer`, configured for `size` pixels. On Windows and Linux frames
/// go into an image the shell draws in its own frame (`Preview::picture`).
#[cfg(target_os = "macos")]
fn open_screen(
    gpu: &Gpu,
    layer: *mut c_void,
    size: (u32, u32),
    hdr: bool,
) -> MediaResult<wgpu::Surface<'static>> {
    // SAFETY: the shell passes a live CAMetalLayer and keeps it alive until `Preview::stop`
    // has joined this thread.
    let surface = unsafe { crate::preview::present::layer_surface(&gpu.instance, layer) }
        .map_err(|cause| surface_error("make the preview surface", cause))?;
    configure(&surface, gpu, size, hdr)?;
    Ok(surface)
}

/// What the thread does after checking the clock.
enum Step {
    /// Present a new frame, or the current one again.
    Present(Option<DecodedFrame>),
    /// Nothing is due yet, or the file ended; check again.
    LookAgain,
}

impl State {
    /// Something needs drawing or decoding.
    fn has_work(&self) -> bool {
        self.seek.is_some() || self.playing || self.resized || self.redraw
    }

    /// Still playing, with no new requests since the last check.
    fn playing_undisturbed(&self) -> bool {
        !self.quit && self.seek.is_none() && !self.resized && !self.redraw && self.playing
    }
}

/// A video file's thread: decodes frames and presents each when the clock reaches it.
fn run(path: &str, shared: &Shared) -> MediaResult<()> {
    let Some(mut presenter) = Presenter::open(path, shared)? else {
        return Ok(());
    };
    while let Some(work) = presenter.wait_for_work() {
        if let Some(size) = work.resized {
            presenter.resize(size)?;
        }
        if let Step::Present(frame) = presenter.step(&work)? {
            presenter.present(frame)?;
            presenter.publish_position();
        }
    }
    Ok(())
}

/// A video thread's state between frames.
struct Presenter<'a> {
    shared: &'a Shared,
    gpu: Arc<Gpu>,
    reader: VideoReader,
    duration_seconds: f64,
    frame_seconds: f64,
    /// The latest seek target: a frame or two before the end, so it lands on a picture.
    last_frame_seconds: f64,
    source_size: (u32, u32),
    hdr: bool,
    transform: InputTransform,
    present: PresentPass,
    /// The working texture, sized to match the layer, and what presents it.
    working_size: (u32, u32),
    working_view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    /// The current frame, kept to redraw when the layer is resized.
    shown: DecodedFrame,
    position: f64,
    /// The layer's size in pixels.
    size: (u32, u32),
    #[cfg(target_os = "macos")]
    screen: wgpu::Surface<'static>,
    /// Wall-clock instant and media time playback started from.
    clock: Option<Anchor>,
    /// The next decoded frame, held until it is due.
    pending: Option<DecodedFrame>,
    snapshot_path: Option<String>,
    presented_frames: u32,
    /// The layer size of the last snapshot. It is rewritten on resize, since a paused file presents
    /// nothing more on its own.
    snapshot_size: Option<(u32, u32)>,
}

impl<'a> Presenter<'a> {
    /// Opens the file, draws its first frame, then waits for the shell's layer. `None` if the
    /// preview is stopped first.
    fn open(path: &str, shared: &'a Shared) -> MediaResult<Option<Self>> {
        let gpu = gpu()?;
        let mut reader = VideoReader::open(path)?;
        let info = reader.info().clone();
        let frame_seconds = 1.0 / info.frames_per_second();
        let hdr = info.color.is_hdr();
        // Show the file as a browser's player does: each layer pixel samples the stored planes
        // at that pixel, so scaling happens on stored values, not linear light, and chroma is
        // sampled at the luma position whatever siting the stream declares (Chromium samples
        // every plane at the same place).
        let shown_color = ColorDescription {
            chroma_siting: ChromaSiting::Center,
            ..info.color
        };
        let working_view = gpu
            .create_working_texture(info.width, info.height, working_format(hdr))
            .create_view(&Default::default());
        let transform = gpu.create_input_transform(&shown_color, info.layout())?;
        #[cfg(target_os = "macos")]
        let present = PresentPass::new(&gpu.device, surface_format(hdr));
        // On Windows and Linux the frame goes into an 8-bit sRGB image, whatever the clip.
        #[cfg(not(target_os = "macos"))]
        let present = PresentPass::new(&gpu.device, SDR_SURFACE_FORMAT);
        let bind = present.bind(&gpu.device, &working_view);
        // Draw the first frame into the working texture before the shell provides a layer.
        let shown = reader.frame_at(0.0)?;
        {
            let _drawing = drawing();
            draw_video(&gpu, &shown, &transform, &working_view)?;
        }
        #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
        let Some((layer, size)) = wait_for_layer(shared, &info, hdr) else {
            return Ok(None);
        };
        #[cfg(target_os = "macos")]
        let screen = open_screen(&gpu, layer, size, hdr)?;
        Ok(Some(Self {
            shared,
            reader,
            duration_seconds: info.duration_seconds,
            frame_seconds,
            last_frame_seconds: (info.duration_seconds - 2.0 * frame_seconds).max(0.0),
            source_size: (info.width, info.height),
            hdr,
            transform,
            present,
            working_size: (info.width, info.height),
            working_view,
            bind,
            position: shown.seconds,
            shown,
            size,
            #[cfg(target_os = "macos")]
            screen,
            clock: None,
            pending: None,
            snapshot_path: std::env::var("NR_SNAPSHOT_DIR")
                .ok()
                .map(|directory| format!("{directory}/preview.ppm")),
            presented_frames: 0,
            snapshot_size: None,
            gpu,
        }))
    }

    /// Sleeps until there is work and returns it; `None` once the preview is stopped.
    fn wait_for_work(&mut self) -> Option<Work> {
        let shared = self.shared;
        let mut state = shared.state.relock();
        loop {
            if state.quit {
                return None;
            }
            if state.has_work() {
                break;
            }
            self.clock = None;
            state.anchor = None;
            state = shared
                .wake
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        let resized = std::mem::take(&mut state.resized).then_some(state.size);
        state.redraw = false;
        // A new rate: the clock continues from its current position at that rate.
        if let Some(running) = self.clock.filter(|running| running.rate != state.rate) {
            self.clock = Some(running.at_rate(Instant::now(), state.rate));
            state.anchor = self.clock;
        }
        Some(Work {
            seek: state.seek.take(),
            playing: state.playing,
            resized,
            rate: state.rate,
        })
    }

    fn resize(&mut self, size: (u32, u32)) -> MediaResult<()> {
        self.size = size;
        #[cfg(target_os = "macos")]
        configure(&self.screen, &self.gpu, size, self.hdr)?;
        Ok(())
    }

    /// Picks the frame to show, if a new one is due.
    fn step(&mut self, work: &Work) -> MediaResult<Step> {
        if let Some(seconds) = work.seek {
            self.clock = None;
            self.pending = None;
            let frame = self
                .reader
                .frame_at(seconds.clamp(0.0, self.last_frame_seconds))?;
            return Ok(Step::Present(Some(frame)));
        }
        if !work.playing {
            return Ok(Step::Present(None));
        }
        self.step_playing(work.rate)
    }

    /// While playing at `rate`: the frame the clock has reached, or a wait until the next is due.
    fn step_playing(&mut self, rate: f64) -> MediaResult<Step> {
        let now = Instant::now();
        let running = *self.clock.get_or_insert(Anchor {
            at: now,
            from: self.position,
            rate,
        });
        let target = running.position(now);
        if self.pending.is_none() {
            self.pending = self.reader.next_frame()?;
        }
        let Some(due) = self.pending.as_ref().map(|frame| frame.seconds) else {
            self.stop_at_end()?;
            return Ok(Step::LookAgain);
        };
        if due > target {
            self.wait_until_due(running.until(due, now).min(LONGEST_PLAY_WAIT));
            return Ok(Step::LookAgain);
        }
        Ok(Step::Present(self.catch_up(target)?))
    }

    /// End of the file: stop there, as a browser's player does.
    fn stop_at_end(&mut self) -> MediaResult<()> {
        self.position = self.duration_seconds;
        self.clock = None;
        {
            let mut state = self.shared.state.relock();
            state.playing = false;
            state.ended = true;
            state.anchor = None;
            state.info.position_seconds = self.position;
            state.info.playing = false;
            state.info.ended = true;
        }
        // The snapshot shows the frame the file stopped on.
        self.write_snapshot()
    }

    /// Sleeps up to `wait`, waking early on a new request.
    fn wait_until_due(&self, wait: Duration) {
        let mut state = self.shared.state.relock();
        state.anchor = self.clock;
        if state.playing_undisturbed() {
            let _ = self.shared.wake.wait_timeout(state, wait);
        }
    }

    /// The pending frame, or a later one when more than a frame behind, skipping stale frames.
    fn catch_up(&mut self, target: f64) -> MediaResult<Option<DecodedFrame>> {
        let mut next = self.pending.take();
        while next
            .as_ref()
            .is_some_and(|frame| frame.seconds < target - self.frame_seconds)
        {
            match self.reader.next_frame()? {
                Some(newer) => next = Some(newer),
                None => break,
            }
        }
        Ok(next)
    }

    /// Draws `frame`, or redraws the current one after a resize, and presents it.
    fn present(&mut self, frame: Option<DecodedFrame>) -> MediaResult<()> {
        let gate = drawing();
        let commands = self.gpu.device.create_command_encoder(&Default::default());
        let new_frame = frame.is_some();
        let remade = self.fit_working_texture();
        if let Some(frame) = frame {
            self.position = frame.seconds;
            self.shown = frame;
        }
        if new_frame || remade {
            // Submit now, since the snapshot below reads the working texture.
            draw_video(&self.gpu, &self.shown, &self.transform, &self.working_view)?;
        }
        // Write the snapshot at the current size before the layer takes the frame: a layer that
        // is resizing or off screen may take it late or never, and a snapshot at an earlier
        // size shows a frame that is gone.
        if self.snapshot_size != Some(self.size) {
            self.snapshot_size = Some(self.size);
            self.write_snapshot()?;
        }
        if self.show(commands)? {
            self.count_presented()?;
        }
        drop(gate);
        Ok(())
    }

    /// Resizes the working texture to match the layer. Returns whether it was recreated.
    fn fit_working_texture(&mut self) -> bool {
        let wanted_size = fitted_size(self.source_size, self.size);
        if wanted_size == self.working_size {
            return false;
        }
        self.working_size = wanted_size;
        self.working_view = self
            .gpu
            .create_working_texture(wanted_size.0, wanted_size.1, working_format(self.hdr))
            .create_view(&Default::default());
        self.bind = self.present.bind(&self.gpu.device, &self.working_view);
        true
    }

    /// Presents to the layer; `false` when the layer could not accept a frame now.
    #[cfg(target_os = "macos")]
    fn show(&self, mut commands: wgpu::CommandEncoder) -> MediaResult<bool> {
        let texture = match self.screen.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Some(texture),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.shared.state.relock().resized = true;
                None
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                // The layer is not on screen yet, or is covered. The frame is in the working
                // texture (and the snapshot already written), so retry shortly instead of
                // dropping it.
                std::thread::sleep(RETRY_PRESENT_AFTER);
                self.shared.state.relock().redraw = true;
                None
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(MediaError("the preview surface was rejected".into()));
            }
        };
        let Some(texture) = texture else {
            self.gpu.queue.submit([commands.finish()]);
            return Ok(false);
        };
        let view = texture.texture.create_view(&Default::default());
        self.present.draw(
            &mut commands,
            &self.bind,
            &view,
            self.size,
            self.working_size,
        );
        self.gpu.queue.submit([commands.finish()]);
        self.gpu.queue.present(texture);
        Ok(true)
    }

    /// On Windows and Linux the frame becomes the image the shell draws next.
    #[cfg(not(target_os = "macos"))]
    fn show(&self, commands: wgpu::CommandEncoder) -> MediaResult<bool> {
        self.gpu.queue.submit([commands.finish()]);
        let bgra = read_presented(
            &self.gpu,
            &self.present,
            &self.bind,
            self.size,
            self.working_size,
        )?;
        let mut picture = self.shared.picture.relock();
        let number = picture.as_ref().map_or(1, |shown| shown.number + 1);
        *picture = Some(Arc::new(Picture {
            width: self.size.0,
            height: self.size.1,
            bgra,
            number,
        }));
        Ok(true)
    }

    /// Counts a presented frame. The first frame is snapshotted as well as the `SNAPSHOT_FRAME`th,
    /// since a paused file never presents a twentieth and the snapshot needs something to show (a
    /// playing file's later frame replaces it).
    fn count_presented(&mut self) -> MediaResult<()> {
        self.presented_frames += 1;
        if self.presented_frames != 1 && self.presented_frames != SNAPSHOT_FRAME {
            return Ok(());
        }
        let Some(path) = &self.snapshot_path else {
            return Ok(());
        };
        write_snapshot(
            &self.gpu,
            &self.working_view,
            self.size,
            self.working_size,
            path,
        )?;
        if self.hdr {
            report_hdr_range(&self.gpu, &self.working_view, self.working_size)?;
        }
        Ok(())
    }

    /// Writes a snapshot of what the layer receives, when the run requested snapshots.
    fn write_snapshot(&self) -> MediaResult<()> {
        let Some(path) = &self.snapshot_path else {
            return Ok(());
        };
        write_snapshot(
            &self.gpu,
            &self.working_view,
            self.size,
            self.working_size,
            path,
        )
    }

    fn publish_position(&self) {
        let mut state = self.shared.state.relock();
        state.info.position_seconds = self.position;
        state.info.playing = state.playing;
        state.info.ended = state.ended;
        state.anchor = self.clock.filter(|_| state.playing);
    }
}

/// Publishes the file's details, applies seeks requested before the duration was known, and
/// waits for the shell's layer. Returns the layer and its size, or `None` if stopped first.
fn wait_for_layer(
    shared: &Shared,
    info: &VideoInfo,
    hdr: bool,
) -> Option<(*mut c_void, (u32, u32))> {
    let mut state = shared.state.relock();
    state.info = PreviewInfo {
        duration_seconds: info.duration_seconds,
        position_seconds: 0.0,
        width: info.width,
        height: info.height,
        playing: false,
        hdr,
        ready: true,
        ..state.info
    };
    state.info.rate = state.rate;
    settle_pending(&mut state, info.duration_seconds);
    loop {
        if state.quit {
            return None;
        }
        if let Some(layer) = state.layer {
            return Some((layer as *mut c_void, state.size));
        }
        state = shared
            .wake
            .wait(state)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

/// Draws `frame` into the working texture `target` and submits it to the GPU.
fn draw_video(
    gpu: &Gpu,
    frame: &DecodedFrame,
    transform: &InputTransform,
    target: &wgpu::TextureView,
) -> MediaResult<()> {
    let textures = gpu.wrap_video_frame(&frame.buffer)?;
    let mut commands = gpu.device.create_command_encoder(&Default::default());
    gpu.draw_video_frame(&mut commands, &textures, transform, target)?;
    gpu.queue.submit([commands.finish()]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            playing: false,
            seek: None,
            pending: Vec::new(),
            has_played: false,
            layer: None,
            size: (1, 1),
            resized: false,
            redraw: false,
            quit: false,
            info: PreviewInfo::default(),
            error: None,
            ended: false,
            anchor: None,
            rate: 1.0,
            menu: Menu::Closed,
            fullscreen: false,
            muted: false,
            volume: 1.0,
            resume_after_scrub: false,
        }
    }

    #[test]
    fn a_watched_picture_is_made_at_the_size_its_layer_shows_it() {
        // A 1280 by 720 clip in a 1280 by 800 window at 2 px per point fills the layer.
        assert_eq!(fitted_size((1280, 720), (2432, 1368)), (2432, 1368));
        // A layer of another shape: the whole picture, with bars left to the layer.
        assert_eq!(fitted_size((1280, 720), (1000, 1000)), (1000, 563));
        assert_eq!(fitted_size((720, 1280), (1000, 1000)), (563, 1000));
        // Larger files are scaled down, and nothing is ever zero pixels wide.
        assert_eq!(fitted_size((3840, 2160), (896, 504)), (896, 504));
        assert_eq!(fitted_size((1280, 720), (1, 1)), (1, 1));
        assert_eq!(fitted_size((0, 0), (10, 10)), (1, 1));
    }

    #[test]
    fn a_sound_files_clock_runs_only_while_playing() {
        let mut state = state();
        let start = Instant::now();
        assert_eq!(step_sound(&mut state, 0.0, 5.0, start), 0.0);
        assert!(state.anchor.is_none());
        state.playing = true;
        assert_eq!(step_sound(&mut state, 0.0, 5.0, start), 0.0);
        let position = step_sound(&mut state, 0.0, 5.0, start + Duration::from_millis(1500));
        assert!((position - 1.5).abs() < 1e-9);
        assert!(state.info.playing);
        // Paused: the position is the moment of the pause, and stays there.
        state.playing = false;
        let paused = step_sound(
            &mut state,
            position,
            5.0,
            start + Duration::from_millis(1700),
        );
        assert!((paused - 1.7).abs() < 1e-9);
        assert!(state.anchor.is_none());
        assert_eq!(
            step_sound(&mut state, paused, 5.0, start + Duration::from_secs(3)),
            paused
        );
    }

    #[test]
    fn a_seek_moves_a_sound_files_clock_and_playing_carries_on_from_there() {
        let mut state = state();
        let start = Instant::now();
        state.playing = true;
        step_sound(&mut state, 0.0, 5.0, start);
        state.seek = Some(4.0);
        let at = start + Duration::from_secs(1);
        assert_eq!(step_sound(&mut state, 1.0, 5.0, at), 4.0);
        let position = step_sound(&mut state, 4.0, 5.0, at + Duration::from_millis(500));
        assert!((position - 4.5).abs() < 1e-9);
        state.seek = Some(99.0);
        state.playing = false;
        assert_eq!(step_sound(&mut state, position, 5.0, at), 5.0);
    }

    /// The test tone's length, in seconds.
    const TONE_SECONDS: u32 = 10;

    /// A sound file played through the whole player, read at the output: the samples sent match the
    /// player's clock and stay in step while it plays.
    #[test]
    fn a_playing_sound_file_hands_the_output_the_samples_at_its_clock() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("tone.wav");
        // Long enough that a slow machine's sleeps never play it to its end mid-test: a 10 ms
        // sleep on a hosted Mac can take 50 ms, and an ended file's clock counts nothing.
        crate::preview::audio::tests::write_tone(&path, TONE_SECONDS);
        let preview = Preview::start_as(path.to_string_lossy().into_owned(), Playing::Sound, 0.0);
        let mut out = [0.0_f32; 960];
        // Paused: silence, and nothing counted as sent.
        preview.render_audio(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert_eq!(preview.audio_stats().frames, 0);
        for _ in 0..200 {
            if preview.info().ready {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let info = preview.info();
        assert!(
            info.ready
                && info.has_audio
                && (info.duration_seconds - f64::from(TONE_SECONDS)).abs() < 0.01
        );
        preview.seek(0.5);
        preview.set_playing(true);
        // An output asking for 10 ms every 10 ms, for a third of a second.
        let mut heard = false;
        let started = Instant::now();
        for _ in 0..33 {
            preview.render_audio(&mut out);
            heard |= out.iter().any(|sample| sample.abs() > 0.2);
            std::thread::sleep(Duration::from_millis(10));
        }
        let stats = preview.audio_stats();
        let position = preview.info().position_seconds;
        // A busy machine's sleeps run long: the clock has moved on by the time that passed.
        let latest = 0.5 + started.elapsed().as_secs_f64() + 0.2;
        assert!(heard, "the tone reached the output");
        // The first requests may arrive before the clock starts; those are silent and uncounted.
        assert!(stats.frames >= 25 * 480, "frames {}", stats.frames);
        assert!((stats.peak - 0.5).abs() < 0.01, "peak {}", stats.peak);
        // The last samples sent are within the resync limit of the clock, which has moved on
        // from the seek by about the time played.
        assert!(
            stats.drift_seconds.abs() <= 0.06,
            "drift {}",
            stats.drift_seconds
        );
        assert!(
            stats.position_seconds > 0.7 && stats.position_seconds < latest,
            "at {} of {latest}",
            stats.position_seconds
        );
        assert!(
            position > 0.7 && position < latest,
            "position {position} of {latest}"
        );
        // Muted: the samples are still taken at the clock, but silence goes out.
        assert!(preview.info().playing, "still playing at {position}");
        preview.set_muted(true);
        preview.render_audio(&mut out);
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert!(preview.audio_stats().frames > stats.frames);
        // Space pauses; the output gets silence and the clock stops.
        assert!(preview.key(Key::Space));
        std::thread::sleep(Duration::from_millis(60));
        preview.render_audio(&mut out);
        let paused = preview.info();
        assert!(!paused.playing);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(preview.info().position_seconds, paused.position_seconds);
        // End goes to the end; playing from there stops at once and playing again restarts.
        assert!(preview.key(Key::End));
        preview.set_playing(true);
        std::thread::sleep(Duration::from_millis(120));
        let ended = preview.info();
        assert!(
            ended.ended
                && !ended.playing
                && (ended.position_seconds - ended.duration_seconds).abs() < 1e-9
        );
        preview.set_playing(true);
        std::thread::sleep(Duration::from_millis(120));
        let again = preview.info();
        assert!(
            again.playing && !again.ended && again.position_seconds < 0.5,
            "{}",
            again.position_seconds
        );
        preview.stop();
    }

    #[test]
    fn a_copy_takes_the_targets_place_only_once_it_is_whole() {
        let directory = tempfile::tempdir().expect("directory");
        let source = directory.path().join("clip.mp4");
        std::fs::write(&source, b"picture").expect("source");
        let target = directory.path().join("saved.mp4");
        copy_replacing(&source, &target).expect("copied");
        assert_eq!(std::fs::read(&target).expect("read"), b"picture");
        // A file already there is replaced.
        std::fs::write(&source, b"another picture").expect("source");
        copy_replacing(&source, &target).expect("replaced");
        assert_eq!(std::fs::read(&target).expect("read"), b"another picture");
        // The source itself is left alone; a copy that fails leaves nothing behind.
        copy_replacing(&source, &source).expect("left alone");
        assert_eq!(std::fs::read(&source).expect("read"), b"another picture");
        assert!(copy_replacing(&directory.path().join("gone.mp4"), &target).is_err());
        assert!(copy_replacing(
            &source,
            &directory.path().join("no folder").join("saved.mp4")
        )
        .is_err());
        let names: Vec<String> = std::fs::read_dir(directory.path())
            .expect("list")
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert_eq!(std::fs::read(&target).expect("read"), b"another picture");
    }

    #[test]
    fn seeks_asked_before_the_length_is_known_are_made_once_it_is() {
        let mut state = state();
        // Woken where it was left, then Right pressed before the file was read.
        state.seek = Some(1.0);
        state.pending = vec![Pending::Key(Key::Right)];
        settle_pending(&mut state, 200.0);
        assert_eq!(state.seek, Some(3.0));
        // A press three quarters along the bar, then Left: from where the press went.
        state.pending = vec![Pending::Share(0.75), Pending::Key(Key::Left)];
        settle_pending(&mut state, 4.0);
        assert!(
            state
                .seek
                .is_some_and(|seconds| (seconds - 2.96).abs() < 1e-9),
            "{:?}",
            state.seek
        );
        assert!(state.pending.is_empty());
        // A file with no length moves nowhere.
        state.seek = None;
        state.pending = vec![Pending::Share(0.5), Pending::Key(Key::End)];
        settle_pending(&mut state, 0.0);
        assert_eq!(state.seek, None);
    }

    /// A page player resumed where it was left opens there, and has not played until asked to.
    #[test]
    fn a_sound_file_opened_part_way_starts_there() {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("tone.wav");
        crate::preview::audio::tests::write_tone(&path, 2);
        let preview = Preview::start_as(path.to_string_lossy().into_owned(), Playing::Sound, 1.5);
        // Pressed at a quarter of the bar before the file was read, or after: the same moment.
        preview.seek_share(0.25);
        for _ in 0..200 {
            if preview.info().ready && (preview.info().position_seconds - 0.5).abs() < 1e-9 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let info = preview.info();
        assert!(
            (info.position_seconds - 0.5).abs() < 1e-9,
            "at {}",
            info.position_seconds
        );
        assert!(!info.has_played && !info.output_wanted());
        preview.set_playing(true);
        let playing = preview.info();
        assert!(playing.has_played && playing.output_wanted());
        preview.set_muted(true);
        assert!(
            preview.info().output_wanted(),
            "muted, the output still runs"
        );
        preview.set_playing(false);
        assert!(preview.info().has_played && !preview.info().output_wanted());
        preview.stop();
    }

    #[test]
    fn a_sound_file_stops_at_its_end() {
        let mut state = state();
        let start = Instant::now();
        state.playing = true;
        step_sound(&mut state, 4.0, 5.0, start);
        let position = step_sound(&mut state, 4.0, 5.0, start + Duration::from_secs(2));
        assert_eq!(position, 5.0);
        assert!(!state.playing && state.ended && state.info.ended && !state.info.playing);
        assert!(state.anchor.is_none());
    }
}
