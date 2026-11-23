//! Tests Matroska and WebM against the same streams in containers AVFoundation reads: every
//! frame's planes and time, seeks with and without cues, details, and sound lag and level,
//! laced and unlaced. The FFmpeg CLI makes the files (a test-time tool only; nothing here links
//! it), so these tests are ignored by default and run on request:
//!
//! ```sh
//! NR_MATROSKA_FIXTURES=<folder> cargo test --release --lib matroska_parity -- --ignored --nocapture
//! ```
//!
//! `NR_FFMPEG` points at another `ffmpeg`. The folder keeps the files between runs.

use super::{probe, AudioDecoder, VideoReader};
use crate::media::matroska::tests::{laced_copy, Lacing};
use crate::media::{
    AudioDecoding, AudioFormat, AudioReader, AudioSourceInfo, Probe, VideoInfo, VideoSource,
};
use northroom_media::pixel_buffer::{CVPixelBufferRef, PixelBuffer};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferLockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferGetPlaneCount(buffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetBaseAddressOfPlane(buffer: CVPixelBufferRef, plane: usize) -> *const c_void;
    fn CVPixelBufferGetBytesPerRowOfPlane(buffer: CVPixelBufferRef, plane: usize) -> usize;
    fn CVPixelBufferGetWidthOfPlane(buffer: CVPixelBufferRef, plane: usize) -> usize;
    fn CVPixelBufferGetHeightOfPlane(buffer: CVPixelBufferRef, plane: usize) -> usize;
}

const PICTURE: &str = "testsrc2=size=320x180:rate=30:duration=3";
const PICTURE_NTSC: &str = "testsrc2=size=320x180:rate=30000/1001:duration=3";
const PICTURE_SD: &str = "testsrc2=size=320x240:rate=30:duration=3";
const PICTURE_PAL: &str = "testsrc2=size=720x576:rate=25:duration=3";
const TWO_TONES: &str = "aevalsrc=0.4*sin(2*PI*(220+200*t)*t)|0.3*sin(2*PI*330*t+1):s=48000:d=4";

fn ffmpeg() -> String {
    std::env::var("NR_FFMPEG").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".into())
}

fn fixtures() -> PathBuf {
    let folder = PathBuf::from(
        std::env::var("NR_MATROSKA_FIXTURES")
            .expect("NR_MATROSKA_FIXTURES names the folder for the test files"),
    );
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

/// Runs FFmpeg to make `out` from `arguments`, unless it exists.
fn make(folder: &Path, out: &str, arguments: &[&str]) -> String {
    let path = folder.join(out);
    if !path.exists() {
        let status = Command::new(ffmpeg())
            .args(["-v", "error", "-y"])
            .args(arguments)
            .arg(&path)
            .status()
            .expect("run ffmpeg");
        assert!(
            status.success(),
            "ffmpeg failed making {out}: {arguments:?}"
        );
    }
    path.to_string_lossy().into_owned()
}

/// A Matroska file and its twin: the same streams in a container AVFoundation reads.
struct Pair {
    name: &'static str,
    matroska: String,
    twin: String,
}

fn video_pairs(folder: &Path) -> Vec<Pair> {
    let lavfi = |source: &'static str| ["-f", "lavfi", "-i", source];
    let mut pairs = Vec::new();
    let mut pair = |name: &'static str, file: &str, twin: &str, encode: &[&str], copy: &[&str]| {
        let matroska = make(folder, file, encode);
        // Times as stored: without `-copyts` FFmpeg moves the video by the sound's codec delay.
        let twin = make(
            folder,
            twin,
            &[
                &[
                    "-copyts",
                    "-i",
                    matroska.as_str(),
                    "-map",
                    "0:v",
                    "-c",
                    "copy",
                ][..],
                copy,
            ]
            .concat(),
        );
        pairs.push(Pair {
            name,
            matroska,
            twin,
        });
    };
    let h264 = [
        &lavfi(PICTURE)[..],
        &lavfi(TWO_TONES),
        &[
            "-c:v",
            "libx264",
            "-bf",
            "3",
            "-g",
            "30",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
    ]
    .concat();
    pair("H.264 + AAC (B-frames)", "h264.mkv", "h264.mp4", &h264, &[]);
    let ntsc = [
        &lavfi(PICTURE_NTSC)[..],
        &[
            "-c:v",
            "libx264",
            "-x264-params",
            "open_gop=1:keyint=24:bframes=3",
            "-pix_fmt",
            "yuv420p",
        ],
    ]
    .concat();
    pair(
        "H.264 open GOP 29.97",
        "h264-open-gop.mkv",
        "h264-open-gop.mp4",
        &ntsc,
        &[],
    );
    let full = [
        &lavfi(PICTURE)[..],
        &[
            "-vf",
            "setparams=range=pc:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuvj420p",
        ],
    ]
    .concat();
    pair(
        "H.264 full range, tagged",
        "h264-full.mkv",
        "h264-full.mp4",
        &full,
        &[],
    );
    let hevc = [
        &lavfi(PICTURE)[..],
        &[
            "-vf",
            "setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc",
            "-c:v",
            "libx265",
            "-pix_fmt",
            "yuv420p10le",
            "-x265-params",
            "keyint=30:log-level=error",
        ],
    ]
    .concat();
    pair(
        "HEVC 10-bit PQ",
        "hevc10.mkv",
        "hevc10.mp4",
        &hevc,
        &["-tag:v", "hvc1"],
    );
    let vp9 = [
        &lavfi(PICTURE)[..],
        &lavfi(TWO_TONES),
        &[
            "-c:v",
            "libvpx-vp9",
            "-g",
            "30",
            "-b:v",
            "500k",
            "-c:a",
            "libopus",
            "-shortest",
        ],
    ]
    .concat();
    pair("VP9 + Opus (WebM)", "vp9.webm", "vp9.mp4", &vp9, &[]);
    let vp9_10 = [
        &lavfi(PICTURE)[..],
        &[
            "-c:v",
            "libvpx-vp9",
            "-pix_fmt",
            "yuv420p10le",
            "-g",
            "30",
            "-b:v",
            "500k",
        ],
    ]
    .concat();
    pair(
        "VP9 profile 2 (10-bit)",
        "vp9-10.webm",
        "vp9-10.mp4",
        &vp9_10,
        &[],
    );
    let av1 = [
        &lavfi(PICTURE)[..],
        &["-c:v", "libsvtav1", "-g", "30", "-preset", "10"],
    ]
    .concat();
    pair("AV1 (WebM)", "av1.webm", "av1.mp4", &av1, &[]);
    let prores = [
        &lavfi(PICTURE)[..],
        &[
            "-c:v",
            "prores_ks",
            "-profile:v",
            "3",
            "-pix_fmt",
            "yuv422p10le",
        ],
    ]
    .concat();
    pair("ProRes 422 HQ", "prores.mkv", "prores.mov", &prores, &[]);
    // Standard definition, untagged (BT.601 assumed) and tagged each possible way.
    let sd = [
        &lavfi(PICTURE_SD)[..],
        &["-c:v", "libx264", "-pix_fmt", "yuv420p"],
    ]
    .concat();
    pair("H.264 SD, untagged", "h264-sd.mkv", "h264-sd.mp4", &sd, &[]);
    let smpte170m = [
        &lavfi(PICTURE_SD)[..],
        &[
            "-vf",
            "setparams=color_primaries=smpte170m:color_trc=smpte170m:colorspace=smpte170m",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
    ]
    .concat();
    pair(
        "H.264 SD, SMPTE 170M",
        "h264-sd-170m.mkv",
        "h264-sd-170m.mp4",
        &smpte170m,
        &[],
    );
    let bt470bg = [
        &lavfi(PICTURE_SD)[..],
        &[
            "-vf",
            "setparams=color_primaries=bt470bg:color_trc=bt470bg:colorspace=bt470bg",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
    ]
    .concat();
    pair(
        "H.264 SD, BT.470BG",
        "h264-sd-470bg.mkv",
        "h264-sd-470bg.mp4",
        &bt470bg,
        &[],
    );
    let pal = [
        &lavfi(PICTURE_PAL)[..],
        &[
            "-vf",
            "setparams=colorspace=bt470bg",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ],
    ]
    .concat();
    pair(
        "H.264 576-line, BT.470BG matrix only",
        "h264-pal.mkv",
        "h264-pal.mp4",
        &pal,
        &[],
    );
    // Made with FFmpeg's lavfi sources and left untagged. FFmpeg's colour bars are BT.470BG, so
    // `Colour` holds only that matrix.
    let bars = [
        &["-f", "lavfi", "-i", "smptebars=s=480x200:r=25:d=3"][..],
        &["-c:v", "libx264", "-pix_fmt", "yuv420p"],
    ]
    .concat();
    pair(
        "H.264 SD colour bars (BT.470BG matrix, by FFmpeg)",
        "h264-bars.mkv",
        "h264-bars.mp4",
        &bars,
        &[],
    );
    let vp9_sd = [
        &lavfi(PICTURE_SD)[..],
        &["-c:v", "libvpx-vp9", "-g", "30", "-b:v", "500k"],
    ]
    .concat();
    pair(
        "VP9 SD, untagged (WebM)",
        "vp9-sd.webm",
        "vp9-sd.mp4",
        &vp9_sd,
        &[],
    );
    // The stream says BT.470BG and neither container does, as when another tool made the
    // stream: the decoder tags its pictures with the code point Core Video has no name for.
    // Both files are restamped copies of the untagged pair (the filter leaves the container's
    // colour unset).
    let restamp = |from: &str, out: &str| {
        let from = folder.join(from).to_string_lossy().into_owned();
        make(
            folder,
            out,
            &[
                "-copyts",
                "-i",
                from.as_str(),
                "-c",
                "copy",
                "-bsf:v",
                "h264_metadata=matrix_coefficients=5:colour_primaries=5:transfer_characteristics=6",
            ],
        )
    };
    let (matroska, twin) = (
        restamp("h264-sd.mkv", "h264-sd-vui-470bg.mkv"),
        restamp("h264-sd.mp4", "h264-sd-vui-470bg.mp4"),
    );
    pairs.push(Pair {
        name: "H.264 SD, BT.470BG in the stream alone",
        matroska,
        twin,
    });
    // Written to a pipe: no cues, no duration, a segment of unknown size.
    let h264_file = folder.join("h264.mkv").to_string_lossy().into_owned();
    let live = folder.join("h264-live.mkv");
    if !live.exists() {
        let out = std::fs::File::create(&live).unwrap();
        let status = Command::new(ffmpeg())
            .args([
                "-v",
                "error",
                "-copyts",
                "-i",
                &h264_file,
                "-avoid_negative_ts",
                "disabled",
                "-c",
                "copy",
                "-f",
                "matroska",
                "pipe:1",
            ])
            .stdout(out)
            .status()
            .unwrap();
        assert!(status.success());
    }
    pairs.push(Pair {
        name: "H.264 written live (no cues, no duration)",
        matroska: live.to_string_lossy().into_owned(),
        twin: folder.join("h264.mp4").to_string_lossy().into_owned(),
    });
    let vp9_file = folder.join("vp9.webm").to_string_lossy().into_owned();
    let live = folder.join("vp9-live.webm");
    if !live.exists() {
        let out = std::fs::File::create(&live).unwrap();
        let status = Command::new(ffmpeg())
            .args([
                "-v",
                "error",
                "-copyts",
                "-i",
                &vp9_file,
                "-avoid_negative_ts",
                "disabled",
                "-c",
                "copy",
                "-f",
                "webm",
                "-live",
                "1",
                "pipe:1",
            ])
            .stdout(out)
            .status()
            .unwrap();
        assert!(status.success());
    }
    pairs.push(Pair {
        name: "VP9 written live (WebM)",
        matroska: live.to_string_lossy().into_owned(),
        twin: folder.join("vp9.mp4").to_string_lossy().into_owned(),
    });
    pairs
}

fn audio_pairs(folder: &Path) -> Vec<Pair> {
    let mut pairs = Vec::new();
    // The same encoder run twice on the same sound, into Matroska and into a container
    // AVFoundation reads, gives the same packets (checked below), each muxer placing them its way.
    let mut pair = |name: &'static str, file: &str, twin: &str, encode: &[&str]| {
        let source = [&["-f", "lavfi", "-i", TWO_TONES][..], encode].concat();
        let matroska = make(folder, file, &source);
        let twin = make(folder, twin, &source);
        pairs.push(Pair {
            name,
            matroska,
            twin,
        });
    };
    pair(
        "AAC stereo",
        "aac.mka",
        "aac.m4a",
        &["-c:a", "aac", "-b:a", "128k"],
    );
    pair(
        "AAC 5.1",
        "aac51.mka",
        "aac51.m4a",
        &[
            "-af",
            "pan=5.1|FL=c0|FR=c1|FC=0.5*c0|LFE=0.1*c1|BL=0.7*c0|BR=0.7*c1",
            "-c:a",
            "aac",
            "-b:a",
            "256k",
        ],
    );
    pair(
        "AAC mono 44.1 kHz",
        "aac-mono.mka",
        "aac-mono.m4a",
        &["-ac", "1", "-ar", "44100", "-c:a", "aac"],
    );
    pair(
        "Opus",
        "opus.webm",
        "opus.mp4",
        &["-c:a", "libopus", "-b:a", "96k"],
    );
    pair(
        "Vorbis",
        "vorbis.webm",
        "vorbis.ogg",
        &["-c:a", "vorbis", "-strict", "-2", "-ac", "2"],
    );
    pair("FLAC", "flac.mka", "flac.flac", &["-c:a", "flac"]);
    pair(
        "PCM 16-bit",
        "pcm16.mka",
        "pcm16.wav",
        &["-c:a", "pcm_s16le"],
    );
    pair(
        "PCM 24-bit",
        "pcm24.mka",
        "pcm24.wav",
        &["-c:a", "pcm_s24le"],
    );
    pair(
        "PCM float",
        "pcmf32.mka",
        "pcmf32.wav",
        &["-c:a", "pcm_f32le"],
    );
    pair(
        "MP3",
        "mp3.mka",
        "mp3.mp3",
        &["-c:a", "libmp3lame", "-b:a", "160k"],
    );
    pair("AC-3", "ac3.mka", "ac3.mp4", &["-c:a", "ac3"]);
    pair("E-AC-3", "eac3.mka", "eac3.mp4", &["-c:a", "eac3"]);
    pair("ALAC", "alac.mka", "alac.m4a", &["-c:a", "alac"]);
    pairs
}

/// FFmpeg's packet hashes for a file's first audio stream, to check two files hold the same
/// packets (the sixth field; side data such as skip counts follows it).
fn packet_hashes(path: &str) -> Vec<String> {
    let output = Command::new(ffmpeg())
        .args([
            "-v", "error", "-i", path, "-map", "0:a:0", "-c", "copy", "-f", "framemd5", "-",
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split(',').nth(5).map(|hash| hash.trim().to_owned()))
        .collect()
}

/// A frame's planes, each row without its padding.
fn planes(frame: &PixelBuffer) -> Vec<Vec<u8>> {
    let buffer = frame.as_ptr();
    // SAFETY: the buffer is live and locked for reading while its planes are copied.
    unsafe {
        assert_eq!(CVPixelBufferLockBaseAddress(buffer, 1), 0);
        let bytes_per_sample = if frame.layout().is_some_and(|layout| layout.bit_depth() > 8) {
            2
        } else {
            1
        };
        let planes = (0..CVPixelBufferGetPlaneCount(buffer))
            .map(|plane| {
                let base = CVPixelBufferGetBaseAddressOfPlane(buffer, plane).cast::<u8>();
                let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, plane);
                let width = CVPixelBufferGetWidthOfPlane(buffer, plane)
                    * bytes_per_sample
                    * if plane == 0 { 1 } else { 2 };
                (0..CVPixelBufferGetHeightOfPlane(buffer, plane))
                    .flat_map(|row| {
                        std::slice::from_raw_parts(base.add(row * stride), width).to_vec()
                    })
                    .collect()
            })
            .collect();
        CVPixelBufferUnlockBaseAddress(buffer, 1);
        planes
    }
}

#[test]
#[ignore = "makes its files with the FFmpeg command line; run with NR_MATROSKA_FIXTURES set"]
fn matroska_parity_video_matches_the_platform_path() {
    let folder = fixtures();
    let mut failures = Vec::new();
    println!("| file | codec | frames | identical planes | max time Δ ms | seeks (cues / clusters) | duration s (mkv / twin) | probe |");
    println!("|---|---|---|---|---|---|---|---|");
    let only = std::env::var("NR_MATROSKA_ONLY").unwrap_or_default();
    for pair in video_pairs(&folder)
        .into_iter()
        .filter(|pair| pair.name.contains(only.as_str()))
    {
        let mut problems = Vec::new();
        compare_video_pair(&pair, &mut problems);
        failures.extend(
            problems
                .into_iter()
                .map(|what| format!("{}: {what}", pair.name)),
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The times every video pair is seeked to, the last one past the end.
const SEEK_SECONDS: [f64; 8] = [0.0, 0.5, 1.0, 1.37, 2.0, 2.5, 2.97, 99.0];

/// How a Matroska seek finds its key frame: from the file's cues, or by walking its clusters.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SeekBy {
    Cues,
    Clusters,
}

/// Compares one Matroska file against its twin (details, every frame, seeks both ways, probe
/// and duration), adds what differs to `problems` and prints the pair's row.
fn compare_video_pair(pair: &Pair, problems: &mut Vec<String>) {
    let mut matroska = match VideoReader::open(&pair.matroska) {
        Ok(reader) => reader,
        Err(error) => {
            problems.push(format!("does not open: {}", error.0));
            return;
        }
    };
    let mut twin = VideoReader::open(&pair.twin)
        .unwrap_or_else(|error| panic!("{} twin: {}", pair.name, error.0));
    let (a, b) = (matroska.info().clone(), twin.info().clone());
    problems.extend(video_info_difference(&a, &b));
    let walk = compare_every_frame(&mut matroska, &mut twin, problems);
    if walk.identical != walk.frames || walk.worst_ms > 0.6 {
        problems.push(format!(
            "{} of {} frames identical, times up to {:.3} ms apart",
            walk.identical, walk.frames, walk.worst_ms
        ));
    }
    let seeks = [SeekBy::Cues, SeekBy::Clusters]
        .map(|way| count_matching_seeks(pair, &mut twin, way, problems));
    let probed = probe(&pair.matroska);
    problems.extend(probe_difference(&probed, &probe(&pair.twin)));
    if (a.duration_seconds - b.duration_seconds).abs() > 0.05 {
        problems.push(format!(
            "duration {} against {}",
            a.duration_seconds, b.duration_seconds
        ));
    }
    println!(
        "| {} | {} {}-bit | {} | {} | {:.3} | {}/{} , {}/{} | {:.3} / {:.3} | {:?} |",
        pair.name,
        a.codec_name,
        a.bit_depth,
        walk.frames,
        walk.identical,
        walk.worst_ms,
        seeks[0],
        SEEK_SECONDS.len(),
        seeks[1],
        SEEK_SECONDS.len(),
        a.duration_seconds,
        b.duration_seconds,
        probed.dimensions
    );
}

/// What differs between two readers' details, if anything.
fn video_info_difference(a: &VideoInfo, b: &VideoInfo) -> Option<String> {
    // Rates to 0.2%: a twin made from Matroska keeps its millisecond times, from which
    // AVFoundation's nominal rate comes out slightly off (29.966 for 29.97).
    let rates_agree = (a.frames_per_second() / b.frames_per_second() - 1.0).abs() < 0.002;
    let same = (a.width, a.height, &a.codec_name, a.bit_depth, a.color)
        == (b.width, b.height, &b.codec_name, b.bit_depth, b.color);
    (!same || !rates_agree).then(|| format!("info differs: {a:?} against {b:?}"))
}

/// How two readers' frames compared, read side by side from the start.
struct FrameWalk {
    frames: usize,
    identical: usize,
    worst_ms: f64,
}

/// Reads both files to the end side by side, counting frames with identical planes and the
/// largest time difference; leftover frames in either file are a problem.
fn compare_every_frame(
    matroska: &mut VideoReader,
    twin: &mut VideoReader,
    problems: &mut Vec<String>,
) -> FrameWalk {
    let mut walk = FrameWalk {
        frames: 0,
        identical: 0,
        worst_ms: 0.0,
    };
    loop {
        match (matroska.next_frame().unwrap(), twin.next_frame().unwrap()) {
            (Some(x), Some(y)) => {
                walk.frames += 1;
                walk.worst_ms = walk.worst_ms.max((x.seconds - y.seconds).abs() * 1000.0);
                walk.identical += usize::from(planes(&x.buffer) == planes(&y.buffer));
            }
            (None, None) => return walk,
            (x, _) => {
                let longer = if x.is_some() {
                    "the Matroska file"
                } else {
                    "the twin"
                };
                problems.push(format!("{longer} has more frames after {}", walk.frames));
                return walk;
            }
        }
    }
}

/// Seeks fresh readers of the Matroska file and its twin to every time in `SEEK_SECONDS`,
/// counting the seeks that land on the same frame.
fn count_matching_seeks(
    pair: &Pair,
    twin: &mut VideoReader,
    way: SeekBy,
    problems: &mut Vec<String>,
) -> usize {
    let ignore_cues = way == SeekBy::Clusters;
    let mut matroska = VideoReader::open(&pair.matroska).unwrap();
    if ignore_cues {
        matroska.ignore_cues();
    }
    let mut matching = 0;
    for seconds in SEEK_SECONDS {
        let x = matroska.frame_at(seconds).unwrap_or_else(|error| {
            panic!(
                "{} at {seconds} s (cues ignored: {ignore_cues}): {}",
                pair.name, error.0
            )
        });
        let y = twin
            .frame_at(seconds)
            .unwrap_or_else(|error| panic!("{} twin at {seconds} s: {}", pair.name, error.0));
        if (x.seconds - y.seconds).abs() < 0.0006 && planes(&x.buffer) == planes(&y.buffer) {
            matching += 1;
        } else {
            problems.push(format!(
                "seek to {seconds} s (cues ignored: {ignore_cues}) gave {} s against {} s",
                x.seconds, y.seconds
            ));
        }
    }
    matching
}

/// What differs between two probes' dimensions and durations, if anything.
fn probe_difference(probed: &Probe, twin: &Probe) -> Option<String> {
    let same = probed.dimensions == twin.dimensions
        && probed
            .duration_seconds
            .zip(twin.duration_seconds)
            .is_some_and(|(x, y)| (x - y).abs() < 0.05);
    (!same).then(|| format!("probe {probed:?} against {twin:?}"))
}

/// The test sound as FFmpeg makes it, `frames` long.
fn two_tones(frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let t = frame as f64 / 48_000.0;
            let pi = std::f64::consts::PI;
            [
                (0.4 * (2.0 * pi * (220.0 + 200.0 * t) * t).sin()) as f32,
                (0.3 * (2.0 * pi * 330.0 * t + 1.0).sin()) as f32,
            ]
        })
        .collect()
}

/// A whole stream at its own rate in two channels.
fn whole_sound(path: &str) -> (Vec<f32>, u32, AudioReader) {
    let rate = AudioDecoder::open(path, 2)
        .unwrap()
        .unwrap()
        .source()
        .sample_rate;
    let mut reader = AudioReader::open(
        path,
        AudioFormat {
            sample_rate: rate,
            channels: 2,
        },
    )
    .unwrap()
    .unwrap();
    let mut samples = Vec::new();
    while let Some(chunk) = reader.read().unwrap() {
        assert_eq!(chunk.start_frame as usize, samples.len() / 2);
        samples.extend(chunk.samples);
    }
    (samples, rate, reader)
}

/// The lag of `b` behind `a` in frames that best aligns them, and their difference in dB below
/// `a`, away from the ends.
fn lag_and_difference(a: &[f32], b: &[f32], rate: u32) -> (i64, f64) {
    let frames = (a.len().min(b.len()) / 2) as i64;
    let middle = frames / 2;
    let window = i64::from(rate / 2).min(frames / 4);
    let correlation = |lag: i64| -> f64 {
        (middle - window..middle + window)
            .filter(|frame| (0..frames).contains(&(frame + lag)))
            .map(|frame| {
                f64::from(a[frame as usize * 2]) * f64::from(b[(frame + lag) as usize * 2])
            })
            .sum()
    };
    let lag = (-3000..=3000)
        .max_by(|x, y| correlation(*x).total_cmp(&correlation(*y)))
        .unwrap_or(0);
    let margin = i64::from(rate / 10);
    let (mut signal, mut noise) = (0.0, 0.0);
    for frame in margin..frames - margin {
        let other = frame + lag;
        if !(0..frames).contains(&other) {
            continue;
        }
        for channel in 0..2 {
            let x = f64::from(a[frame as usize * 2 + channel]);
            let y = f64::from(b[other as usize * 2 + channel]);
            signal += x * x;
            noise += (x - y) * (x - y);
        }
    }
    (lag, decibels_below(signal, noise))
}

/// A difference's level in dB below the signal; no difference is minus infinity.
fn decibels_below(signal: f64, noise: f64) -> f64 {
    if noise == 0.0 {
        f64::NEG_INFINITY
    } else {
        10.0 * (noise / signal).log10()
    }
}

/// The largest difference between a read from a seek and the whole stream, over half a second
/// after the seek's target, in dB below the stream.
fn seek_difference(path: &str, whole: &[f32], rate: u32, seconds: f64) -> f64 {
    let mut reader = AudioReader::open(
        path,
        AudioFormat {
            sample_rate: rate,
            channels: 2,
        },
    )
    .unwrap()
    .unwrap();
    reader.seek(seconds).unwrap();
    let target = (seconds * f64::from(rate)) as i64;
    let (mut signal, mut noise) = (0.0, 0.0);
    while let Some(chunk) = reader.read().unwrap() {
        for (index, sample) in chunk.samples.iter().enumerate() {
            let frame = chunk.start_frame + (index / 2) as i64;
            if frame < target
                || frame >= target + i64::from(rate / 2)
                || frame as usize * 2 + 1 >= whole.len()
            {
                continue;
            }
            let reference = f64::from(whole[frame as usize * 2 + index % 2]);
            signal += reference * reference;
            noise += (reference - f64::from(*sample)).powi(2);
        }
        if chunk.start_frame > target + i64::from(rate) {
            break;
        }
    }
    decibels_below(signal, noise)
}

#[test]
#[ignore = "makes its files with the FFmpeg command line; run with NR_MATROSKA_FIXTURES set"]
fn matroska_parity_audio_matches_the_platform_path() {
    let folder = fixtures();
    let mut pairs = audio_pairs(&folder);
    add_laced_copies(&folder, &mut pairs);
    // The H.264 file's sound, in a file with video and cues for the video only. Its twin is the
    // same encode into MP4 (a copy out of Matroska would lose the priming FFmpeg reads as the
    // codec delay).
    let h264_twin = make(
        &folder,
        "h264-av.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            PICTURE,
            "-f",
            "lavfi",
            "-i",
            TWO_TONES,
            "-c:v",
            "libx264",
            "-bf",
            "3",
            "-g",
            "30",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
    );
    pairs.push(Pair {
        name: "AAC beside H.264",
        matroska: folder.join("h264.mkv").to_string_lossy().into_owned(),
        twin: h264_twin,
    });
    pairs.push(Pair {
        name: "Opus written live",
        matroska: make_opus_written_live(&folder),
        twin: folder.join("opus.mp4").to_string_lossy().into_owned(),
    });
    let mut failures = Vec::new();
    println!("| file | codec | rate / channels | same packets | lag (frames) | from the source (mkv / twin) | difference dB | seeks dB (worst of 3) | duration s (mkv / twin) |");
    println!("|---|---|---|---|---|---|---|---|---|");
    let only = std::env::var("NR_MATROSKA_ONLY").unwrap_or_default();
    for pair in pairs
        .iter()
        .filter(|pair| pair.name.contains(only.as_str()))
    {
        let mut problems = Vec::new();
        compare_audio_pair(pair, &mut problems);
        failures.extend(
            problems
                .into_iter()
                .map(|what| format!("{}: {what}", pair.name)),
        );
    }
    // Laced and unlaced copies decode to exactly the same samples.
    for (laced, plain) in [
        ("aac-laced.mka", "aac.mka"),
        ("vorbis-laced.mka", "vorbis.webm"),
        ("pcm16-laced.mka", "pcm16.mka"),
    ] {
        let (x, _, _) = whole_sound(&folder.join(laced).to_string_lossy());
        let (y, _, _) = whole_sound(&folder.join(plain).to_string_lossy());
        if x != y {
            failures.push(format!("{laced} decodes differently from {plain}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Adds laced copies of three files: the same frames stored with each of Matroska's three
/// lacing modes, each checked against its source's twin.
fn add_laced_copies(folder: &Path, pairs: &mut Vec<Pair>) {
    for (name, source, lacing, out) in [
        ("AAC, EBML-laced", "aac.mka", Lacing::Ebml, "aac-laced.mka"),
        (
            "Vorbis, Xiph-laced",
            "vorbis.webm",
            Lacing::Xiph,
            "vorbis-laced.mka",
        ),
        (
            "PCM, fixed-laced",
            "pcm16.mka",
            Lacing::Fixed,
            "pcm16-laced.mka",
        ),
    ] {
        let source = folder.join(source).to_string_lossy().into_owned();
        let out_path = folder.join(out).to_string_lossy().into_owned();
        laced_copy(&source, &out_path, lacing, 8);
        let twin = pairs
            .iter()
            .find(|pair| pair.matroska == source)
            .map(|pair| pair.twin.clone())
            .unwrap();
        pairs.push(Pair {
            name,
            matroska: out_path,
            twin,
        });
    }
}

/// The Opus sound as written live (no cues, no duration, unknown sizes), made once.
fn make_opus_written_live(folder: &Path) -> String {
    let live = folder.join("opus-live.webm");
    if !live.exists() {
        let out = std::fs::File::create(&live).unwrap();
        let source = folder.join("opus.webm").to_string_lossy().into_owned();
        let arguments = [
            "-v",
            "error",
            "-copyts",
            "-i",
            &source,
            "-avoid_negative_ts",
            "disabled",
            "-c",
            "copy",
            "-f",
            "webm",
            "-live",
            "1",
            "pipe:1",
        ];
        assert!(Command::new(ffmpeg())
            .args(arguments)
            .stdout(out)
            .status()
            .unwrap()
            .success());
    }
    live.to_string_lossy().into_owned()
}

/// Compares one Matroska sound against its twin (placement, level, length, seeks and
/// duration), adds what differs to `problems` and prints the pair's row.
fn compare_audio_pair(pair: &Pair, problems: &mut Vec<String>) {
    let info = match AudioDecoder::open(&pair.matroska, 2) {
        Ok(Some(decoder)) => decoder.source().clone(),
        Ok(None) => {
            problems.push("no sound found".into());
            return;
        }
        Err(error) => {
            problems.push(format!("does not open: {}", error.0));
            return;
        }
    };
    let same_packets = packet_hashes(&pair.matroska) == packet_hashes(&pair.twin);
    let (matroska, rate, _) = whole_sound(&pair.matroska);
    let (twin, twin_rate, twin_reader) = whole_sound(&pair.twin);
    assert_eq!(rate, twin_rate, "{}", pair.name);
    let (lag, difference) = lag_and_difference(&twin, &matroska, rate);
    let truth = lags_from_the_source(&info, rate, &matroska, &twin);
    if !placed_on_time(&info, truth, lag) || difference > -80.0 {
        problems.push(format!(
            "lag {lag} (from the source {truth:?}), difference {difference:.1} dB"
        ));
    }
    let length = |samples: &[f32]| samples.len() as f64 / 2.0 / f64::from(rate);
    let (mkv_length, twin_length) = (length(&matroska), length(&twin));
    if (mkv_length - twin_length).abs() > 0.03 {
        problems.push(format!(
            "decoded {mkv_length:.4} s against {twin_length:.4} s"
        ));
    }
    let seeks =
        [0.73, 1.5, 2.25].map(|seconds| seek_difference(&pair.matroska, &matroska, rate, seconds));
    let worst_seek = seeks.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if worst_seek > -80.0 {
        problems.push(format!(
            "a seeked read differs from the whole one: {seeks:?} dB"
        ));
    }
    let durations = (info.duration_seconds, twin_reader.source().duration_seconds);
    if (durations.0 - durations.1).abs() > 0.05 {
        problems.push(format!("duration {} against {}", durations.0, durations.1));
    }
    println!(
        "| {} | {} | {} / {} | {} | {lag} | {} | {difference:.1} | {worst_seek:.1} | {:.3} / {:.3} |",
        pair.name,
        info.codec_name,
        info.sample_rate,
        info.channels,
        if same_packets { "yes" } else { "no" },
        truth.map_or_else(|| "–".to_owned(), |(mkv, twin)| format!("{mkv} / {twin}")),
        durations.0,
        durations.1
    );
}

/// For the 48 kHz stereo files, where the Matroska read and the twin's each land relative to
/// the source sound, in frames.
fn lags_from_the_source(
    info: &AudioSourceInfo,
    rate: u32,
    matroska: &[f32],
    twin: &[f32],
) -> Option<(i64, i64)> {
    (info.channels == 2 && rate == 48_000).then(|| {
        let source = two_tones(matroska.len().max(twin.len()) / 2);
        (
            lag_and_difference(&source, matroska, rate).0,
            lag_and_difference(&source, twin, rate).0,
        )
    })
}

/// Whether the Matroska sound lands where it should: checked against the source sound where it
/// can be made here, else against the twin.
fn placed_on_time(info: &AudioSourceInfo, truth: Option<(i64, i64)>, lag: i64) -> bool {
    // Core Audio's AC-3 decoder outputs sound 32 samples (0.67 ms) later than the 256-sample
    // delay FFmpeg's encoder states, and AVFoundation reads the same packets from MP4 256
    // samples early, so the source sound is the reference for those.
    let allowed = if info.codec_name.contains("ac3") {
        32
    } else {
        0
    };
    match truth {
        Some((from_source, _)) => from_source.abs() <= allowed,
        None => lag == 0,
    }
}
