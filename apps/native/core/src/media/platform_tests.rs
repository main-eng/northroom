//! Tests the Windows and Linux backends against the FFmpeg CLI's decode of the same files:
//! frame planes, times and seeks, turned and scaled stills, details, and sound read whole,
//! after a seek and from mono. FFmpeg makes the files (a test-time tool only; nothing here
//! links it), so these tests are ignored by default and run on request, as CI does:
//!
//! ```sh
//! NR_MEDIA_FIXTURES=<folder> cargo test --release --lib media::platform_tests -- --ignored
//! ```
//!
//! `NR_FFMPEG` points at another `ffmpeg`. The folder keeps the files between runs.

use super::{probe, still, AudioFormat, AudioReader, VideoReader, VideoSource};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const PICTURE: &str = "testsrc2=size=320x180:rate=30:duration=2";
const TONE: &str = "aevalsrc=0.4*sin(2*PI*(220+200*t)*t)|0.3*sin(2*PI*330*t+1):s=48000:d=3";
const ONE_TONE: &str = "aevalsrc=0.5*sin(2*PI*440*t):s=44100:d=2";

fn ffmpeg() -> String {
    std::env::var("NR_FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

/// The file `name` in the fixtures folder, made by FFmpeg from `arguments` if missing. Tests
/// run in parallel and share the folder, so FFmpeg writes to a name unique to this call
/// (keeping the extension, which picks the format) and the finished file is moved into place.
/// A test never reads a file another is still writing.
fn make(name: &str, arguments: &[&str]) -> String {
    static MAKING: AtomicU64 = AtomicU64::new(0);
    let folder = PathBuf::from(
        std::env::var("NR_MEDIA_FIXTURES")
            .expect("NR_MEDIA_FIXTURES names the folder for the test files"),
    );
    std::fs::create_dir_all(&folder).expect("fixtures folder");
    let path = folder.join(name);
    if !path.exists() {
        let making = folder.join(format!(
            "making-{}-{}-{name}",
            std::process::id(),
            MAKING.fetch_add(1, Ordering::Relaxed)
        ));
        let status = Command::new(ffmpeg())
            .args(["-v", "error", "-y"])
            .args(arguments)
            .arg(&making)
            .status()
            .expect("run ffmpeg");
        assert!(
            status.success(),
            "ffmpeg failed making {name}: {arguments:?}"
        );
        // Another test may have moved its copy in first, and on Windows a file being read
        // can't be replaced. Either way the whole file is there.
        if std::fs::rename(&making, &path).is_err() {
            assert!(
                path.exists(),
                "{name} was made but could not be moved into place"
            );
            std::fs::remove_file(&making).expect("remove the spare copy");
        }
    }
    path.to_string_lossy().into_owned()
}

/// FFmpeg's decode of `path` with output options `arguments`, as raw bytes.
fn decoded(path: &str, arguments: &[&str]) -> Vec<u8> {
    let output = Command::new(ffmpeg())
        .args(["-v", "error", "-i", path])
        .args(arguments)
        .arg("-")
        .output()
        .expect("run ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg failed reading {path}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect()
}

/// The mean difference of two equal-length byte runs.
fn mean_difference(ours: &[u8], theirs: &[u8]) -> f64 {
    assert_eq!(ours.len(), theirs.len(), "the pictures differ in size");
    ours.iter()
        .zip(theirs)
        .map(|(a, b)| f64::from(a.abs_diff(*b)))
        .sum::<f64>()
        / ours.len() as f64
}

fn h264() -> String {
    // Tagged, so every reader takes colour from the stream, not its own defaults.
    make(
        "picture.mp4",
        &[
            "-f", "lavfi", "-i", PICTURE, "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "15",
            "-bf", "2",
        ]
        .into_iter()
        .chain([
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_range",
            "tv",
        ])
        .collect::<Vec<_>>(),
    )
}

#[test]
#[ignore = "makes its files with the FFmpeg command line"]
fn h264_frames_match_ffmpegs_in_order_and_at_a_time() {
    let path = h264();
    let mut reader = VideoReader::open(&path).expect("open");
    let info = reader.info().clone();
    assert_eq!(
        (info.width, info.height, info.frame_rate, info.bit_depth),
        (320, 180, (30, 1), 8)
    );
    assert_eq!(info.codec_name, "h264");
    assert!(
        (info.duration_seconds - 2.0).abs() < 0.05,
        "{}",
        info.duration_seconds
    );
    // Every frame, in presentation order, at its time.
    let mut times = Vec::new();
    let first = reader.next_frame().expect("frame").expect("a first frame");
    times.push(first.seconds);
    while let Some(frame) = reader.next_frame().expect("frame") {
        times.push(frame.seconds);
    }
    assert_eq!(times.len(), 60);
    for (index, seconds) in times.iter().enumerate() {
        assert!(
            (seconds - index as f64 / 30.0).abs() < 0.002,
            "frame {index} at {seconds}"
        );
    }
    // The first frame's planes match FFmpeg's.
    let theirs = decoded(
        &path,
        &["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "nv12"],
    );
    let ours = [first.buffer.plane(0), first.buffer.plane(1)].concat();
    assert!(
        mean_difference(&ours, &theirs) < 0.05,
        "first frame differs by {}",
        mean_difference(&ours, &theirs)
    );
    // A frame at a time is the frame shown then, the same after any seek.
    let at_one = reader.frame_at(1.0).expect("frame at 1 s");
    assert!(
        (at_one.seconds - 1.0).abs() < 0.5 / 30.0,
        "{}",
        at_one.seconds
    );
    reader.frame_at(1.9).expect("frame at 1.9 s");
    let again = reader.frame_at(1.0).expect("frame at 1 s again");
    assert_eq!(
        (again.seconds, again.buffer.plane(0)),
        (at_one.seconds, at_one.buffer.plane(0))
    );
    let past = reader.frame_at(5.0).expect("frame past the end");
    assert!(
        (past.seconds - 59.0 / 30.0).abs() < 0.002,
        "{}",
        past.seconds
    );
}

#[test]
#[ignore = "makes its files with the FFmpeg command line"]
fn a_still_is_ffmpegs_frame_shrunk_and_turned() {
    let path = h264();
    let small = still(&path, 0.0, 64).expect("still");
    assert_eq!((small.width, small.height), (64, 36));
    let theirs = decoded(
        &path,
        &[
            "-frames:v",
            "1",
            "-vf",
            "scale=64:36:flags=area",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgra",
        ],
    );
    // FFmpeg keeps the stored curve where the Mac (and so the still) reads BT.709 as a power of
    // 1.961, a few levels apart on mid greys.
    let colour = VideoReader::open(&path).expect("open").info().color.name();
    assert!(
        mean_difference(&small.bgra, &theirs) < 10.0,
        "{} in {colour}",
        mean_difference(&small.bgra, &theirs)
    );
    // A file whose stream is shown turned a quarter.
    let turned = make(
        "turned.mp4",
        &["-display_rotation", "90", "-i", &path, "-c", "copy"],
    );
    assert_eq!(probe(&turned).dimensions, Some((180, 320)));
    let ours = still(&turned, 0.0, 1000).expect("turned still");
    assert_eq!((ours.width, ours.height), (180, 320));
    let theirs = decoded(
        &turned,
        &["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "bgra"],
    );
    assert!(
        mean_difference(&ours.bgra, &theirs) < 10.0,
        "turned the wrong way: {}",
        mean_difference(&ours.bgra, &theirs)
    );
}

#[test]
#[ignore = "makes its files with the FFmpeg command line"]
fn details_say_a_videos_size_and_a_sounds_length() {
    let video = probe(&h264());
    assert_eq!(video.dimensions, Some((320, 180)));
    assert!(
        video
            .duration_seconds
            .is_some_and(|seconds| (seconds - 2.0).abs() < 0.05),
        "{video:?}"
    );
    let sound = probe(&make(
        "tone.wav",
        &["-f", "lavfi", "-i", TONE, "-c:a", "pcm_s16le"],
    ));
    assert_eq!(sound.dimensions, None);
    assert!(
        sound
            .duration_seconds
            .is_some_and(|seconds| (seconds - 3.0).abs() < 0.01),
        "{sound:?}"
    );
    let folder = std::env::var("NR_MEDIA_FIXTURES").expect("NR_MEDIA_FIXTURES");
    let text = PathBuf::from(folder).join("notes.txt");
    std::fs::write(&text, "not media").expect("text");
    assert_eq!(probe(&text.to_string_lossy()), super::Probe::default());
}

/// All of `path`'s sound at 48 kHz stereo from `seconds` on (`None` for the start), and the
/// output frame it starts at.
fn read_all(path: &str, seconds: Option<f64>) -> (i64, Vec<f32>) {
    let mut reader = AudioReader::open(
        path,
        AudioFormat {
            sample_rate: 48_000,
            channels: 2,
        },
    )
    .expect("open")
    .expect("sound");
    if let Some(seconds) = seconds {
        reader.seek(seconds).expect("seek");
    }
    let mut start = None;
    let mut samples = Vec::new();
    while let Some(chunk) = reader.read().expect("read") {
        start.get_or_insert(chunk.start_frame);
        samples.extend(chunk.samples);
    }
    (start.expect("some sound"), samples)
}

#[test]
#[ignore = "makes its files with the FFmpeg command line"]
fn sound_is_ffmpegs_whole_and_after_a_seek() {
    let wav = make(
        "tone.wav",
        &["-f", "lavfi", "-i", TONE, "-c:a", "pcm_s16le"],
    );
    let theirs = floats(&decoded(&wav, &["-f", "f32le", "-ac", "2"]));
    let (start, ours) = read_all(&wav, None);
    assert_eq!((start, ours.len()), (0, theirs.len()));
    let worst = ours
        .iter()
        .zip(&theirs)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(worst < 1e-6, "{worst}");
    // A compressed, resampled file: from the seek target on, a read after a seek matches a read
    // from the start. Before it is the reader's pre-roll (`AudioReader::seek`), whose first
    // samples a decoder makes without the earlier packets.
    let aac = make(
        "tone.m4a",
        &[
            "-f", "lavfi", "-i", ONE_TONE, "-ac", "2", "-c:a", "aac", "-b:a", "192k",
        ],
    );
    let (_, whole) = read_all(&aac, None);
    assert!(
        (whole.len() as f64 / 2.0 / 48_000.0 - 2.0).abs() < 0.05,
        "{} frames",
        whole.len() / 2
    );
    let (start, seeked) = read_all(&aac, Some(1.2));
    let sought = (1.2 * 48_000.0) as usize;
    let skipped = (sought - start as usize) * 2;
    let pairs = || {
        seeked[skipped..]
            .iter()
            .zip(&whole[sought * 2..])
            .take(24_000 * 2)
    };
    let (at, worst) =
        pairs()
            .map(|(a, b)| (a - b).abs())
            .enumerate()
            .fold(
                (0, 0.0),
                |most, (at, gap)| if gap > most.1 { (at, gap) } else { most },
            );
    // How far the seeked read is offset from the whole one, in output frames, if at all.
    let lag = (-2048..=2048_i64)
        .min_by(|&a, &b| {
            let mismatch = |lag: i64| -> f32 {
                (0..4800)
                    .map(|frame| {
                        (seeked[skipped + frame * 2]
                            - whole[((sought + frame) as i64 + lag) as usize * 2])
                            .abs()
                    })
                    .sum()
            };
            mismatch(a).total_cmp(&mismatch(b))
        })
        .expect("a lag");
    assert!(worst < 1e-3, "after a seek (from frame {start}), {worst} from the read from the start at frame {}; best aligned {lag} frames on", sought + at / 2);
}

#[test]
#[ignore = "makes its files with the FFmpeg command line"]
fn one_channel_is_heard_in_both() {
    let mono = make(
        "mono.wav",
        &[
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0.5*sin(2*PI*440*t):s=48000:d=1",
            "-c:a",
            "pcm_s16le",
        ],
    );
    let theirs = floats(&decoded(&mono, &["-f", "f32le", "-ac", "1"]));
    let (_, ours) = read_all(&mono, None);
    assert_eq!(ours.len(), theirs.len() * 2);
    for (pair, sample) in ours.chunks(2).zip(&theirs) {
        assert_eq!(pair[0], pair[1], "the same in both");
        assert!(
            (pair[0] - sample).abs() < 1e-6,
            "{} against {sample}",
            pair[0]
        );
    }
}

/// Uses the system's GStreamer decoders (Windows Server has no VP9 decoder without the Store
/// extension): a 10-bit WebM stream comes out as 10-bit planes.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "makes its files with the FFmpeg command line"]
fn a_deeper_stream_keeps_its_depth() {
    let path = make(
        "deep.webm",
        &[
            "-f",
            "lavfi",
            "-i",
            PICTURE,
            "-c:v",
            "libvpx-vp9",
            "-pix_fmt",
            "yuv420p10le",
            "-b:v",
            "1M",
            "-deadline",
            "realtime",
        ],
    );
    let mut reader = VideoReader::open(&path).expect("open");
    assert_eq!(
        (reader.info().bit_depth, reader.info().codec_name.as_str()),
        (10, "vp9")
    );
    let frame = reader.next_frame().expect("frame").expect("a first frame");
    assert_eq!(
        frame.buffer.layout(),
        Some(northroom_media::pixel_buffer::PixelLayout::P010 { full_range: false })
    );
    // The same samples as FFmpeg's, in the high bits of each word.
    let theirs = decoded(
        &path,
        &["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "p010le"],
    );
    let ours = [frame.buffer.plane(0), frame.buffer.plane(1)].concat();
    assert!(
        mean_difference(&ours, &theirs) < 0.05,
        "{}",
        mean_difference(&ours, &theirs)
    );
}

#[test]
#[ignore = "makes its files with the FFmpeg command line"]
fn the_preview_presents_into_a_picture_the_shell_draws() {
    use std::time::{Duration, Instant};
    let path = h264();
    let preview =
        crate::preview::Preview::start_as(path.clone(), crate::preview::Playing::Video, 0.0);
    // No layer off a Mac: the frame is presented into a picture of the layer's size.
    preview.attach(std::ptr::null_mut(), 320, 180);
    let asked = Instant::now();
    let picture = loop {
        if let Some(picture) = preview.picture() {
            break picture;
        }
        assert!(
            asked.elapsed() < Duration::from_secs(30),
            "no picture presented"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    preview.stop();
    assert_eq!(
        (picture.width, picture.height, picture.number),
        (320, 180, 1)
    );
    // The first frame as FFmpeg reads it (the still test explains the difference).
    let theirs = decoded(
        &path,
        &["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "bgra"],
    );
    assert!(
        mean_difference(&picture.bgra, &theirs) < 10.0,
        "{}",
        mean_difference(&picture.bgra, &theirs)
    );
}
