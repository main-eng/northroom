//! Tests video stills against the picture AVFoundation's image generator makes (first frame,
//! `appliesPreferredTrackTransform`, a square `maximumSize`): MP4 and MOV byte for byte at
//! every depth, size and turn, and Matroska against the generator's picture of the same stream
//! in an MP4. The FFmpeg CLI makes the files (a test-time tool only), so these tests are
//! ignored by default and run on request:
//!
//! ```sh
//! NR_STILL_FIXTURES=<folder> cargo test --release --lib still_parity -- --ignored --nocapture
//! ```
//!
//! `NR_FFMPEG` points at another `ffmpeg`. The folder keeps the files between runs.

use super::av::{CFDataGetBytePtr, CFDataGetLength, CFRelease, CFTypeRef, Cf};
use super::still::{still, Still};
use crate::media::picture_size;
use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
    fn CGImageGetBitsPerComponent(image: CFTypeRef) -> usize;
    fn CGImageGetBitsPerPixel(image: CFTypeRef) -> usize;
    fn CGImageGetBytesPerRow(image: CFTypeRef) -> usize;
    fn CGImageGetBitmapInfo(image: CFTypeRef) -> u32;
    fn CGImageGetColorSpace(image: CFTypeRef) -> CFTypeRef;
    fn CGImageGetDataProvider(image: CFTypeRef) -> CFTypeRef;
    fn CGDataProviderCopyData(provider: CFTypeRef) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFEqual(left: CFTypeRef, right: CFTypeRef) -> u8;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Size {
    width: f64,
    height: f64,
}

// SAFETY: Core Graphics' `CGSize`, two `CGFloat`s.
unsafe impl Encode for Size {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}

/// A `CGImageRef` as an Objective-C method returns it.
#[repr(transparent)]
struct ImageRef(*const c_void);

// SAFETY: a pointer to the opaque `CGImage` struct.
unsafe impl Encode for ImageRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Time {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: Core Media's `CMTime`.
unsafe impl Encode for Time {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING],
    );
}

// SAFETY: a pointer to a `CMTime`.
unsafe impl RefEncode for Time {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

/// The reference picture: `AVAssetImageGenerator` at time zero, turned per the track, no larger
/// than `max_pixels` on either side.
fn generator_picture(path: &str, max_pixels: u32) -> Option<Still> {
    objc2::rc::autoreleasepool(|_| {
        let path = Cf::string(path).ok()?;
        // SAFETY: each message goes to a live object with the arguments its method takes; the
        // image comes back +1 (a `copy` method) and is owned by the `Still`.
        unsafe {
            let url: *const AnyObject = msg_send![class!(NSURL), fileURLWithPath: path.object()];
            let asset: Option<Retained<AnyObject>> = msg_send![class!(AVURLAsset), URLAssetWithURL: url, options: ptr::null::<AnyObject>()];
            let generator: Option<Retained<AnyObject>> =
                msg_send![class!(AVAssetImageGenerator), assetImageGeneratorWithAsset: &*asset?];
            let generator = generator?;
            let _: () = msg_send![&*generator, setAppliesPreferredTrackTransform: Bool::YES];
            let _: () = msg_send![&*generator, setMaximumSize: Size { width: f64::from(max_pixels), height: f64::from(max_pixels) }];
            let zero = Time {
                value: 0,
                timescale: 1,
                flags: 1,
                epoch: 0,
            };
            let image: ImageRef = msg_send![&*generator, copyCGImageAtTime: zero, actualTime: ptr::null_mut::<Time>(), error: ptr::null_mut::<*mut AnyObject>()];
            (!image.0.is_null()).then(|| Still::from_raw(image.0))
        }
    })
}

/// A picture's width and height.
fn size_of(picture: &Still) -> (usize, usize) {
    // SAFETY: the image is live.
    unsafe {
        (
            CGImageGetWidth(picture.as_ptr()),
            CGImageGetHeight(picture.as_ptr()),
        )
    }
}

/// How two pictures differ, or `None` if they are identical: size, depth, layout, colour space
/// and every byte of every row.
fn difference(ours: &Still, theirs: &Still) -> Option<String> {
    let (a, b) = (ours.as_ptr(), theirs.as_ptr());
    // SAFETY: both images are live; their data is copied and released here.
    unsafe {
        let shape = |image| {
            (
                CGImageGetWidth(image),
                CGImageGetHeight(image),
                CGImageGetBitsPerComponent(image),
                CGImageGetBitsPerPixel(image),
                CGImageGetBitmapInfo(image),
            )
        };
        if shape(a) != shape(b) {
            return Some(format!("shape {:?} against {:?}", shape(a), shape(b)));
        }
        if CFEqual(CGImageGetColorSpace(a), CGImageGetColorSpace(b)) == 0 {
            return Some("colour space".into());
        }
        let (data_a, data_b) = (
            CGDataProviderCopyData(CGImageGetDataProvider(a)),
            CGDataProviderCopyData(CGImageGetDataProvider(b)),
        );
        let bytes = |data| {
            std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize)
        };
        let (rows_a, rows_b) = (CGImageGetBytesPerRow(a), CGImageGetBytesPerRow(b));
        let row_bytes = CGImageGetWidth(a) * CGImageGetBitsPerPixel(a) / 8;
        let differing = (0..CGImageGetHeight(a))
            .map(|row| {
                let left = &bytes(data_a)[row * rows_a..row * rows_a + row_bytes];
                let right = &bytes(data_b)[row * rows_b..row * rows_b + row_bytes];
                left.iter().zip(right).filter(|(x, y)| x != y).count()
            })
            .sum::<usize>();
        CFRelease(data_a);
        CFRelease(data_b);
        (differing > 0).then(|| format!("{differing} bytes differ"))
    }
}

#[test]
fn a_still_is_sized_as_the_generator_sizes_it() {
    assert_eq!(picture_size(1280, 720, 960), (960, 540));
    assert_eq!(picture_size(1000, 562, 960), (960, 539), "rounded down");
    assert_eq!(picture_size(1000, 562, 96), (96, 53));
    assert_eq!(picture_size(720, 1280, 96), (54, 96));
    assert_eq!(picture_size(640, 360, 960), (640, 360), "never enlarged");
    assert_eq!(picture_size(960, 960, 960), (960, 960));
    assert_eq!(picture_size(4000, 2, 96), (96, 1), "never nothing");
}

#[test]
fn a_file_with_no_picture_has_no_still() {
    let folder = tempfile::tempdir().unwrap();
    let text = folder.path().join("notes.mp4");
    std::fs::write(&text, b"plain text, not a film").unwrap();
    assert!(still(&text.to_string_lossy(), 0.0, 96).is_err());
    assert!(still(&folder.path().join("gone.mp4").to_string_lossy(), 0.0, 96).is_err());
    // Through the boundary the shell calls: null, and nothing to release.
    let path = CString::new(text.to_string_lossy().as_bytes()).unwrap();
    // SAFETY: `path` is NUL-terminated.
    assert!(unsafe { crate::ffi::nr_video_still(path.as_ptr(), 0.0, 96) }.is_null());
}

fn ffmpeg() -> String {
    std::env::var("NR_FFMPEG").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".into())
}

fn fixtures() -> PathBuf {
    let folder = PathBuf::from(
        std::env::var("NR_STILL_FIXTURES")
            .expect("NR_STILL_FIXTURES names the folder for the test files"),
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

fn picture(size: &str, rate: u32) -> String {
    format!("testsrc2=size={size}:rate={rate}:duration=2")
}

/// The sizes the shell requests: board cards and list rows.
const SIZES: [u32; 2] = [960, 96];

#[test]
#[ignore = "makes its files with the FFmpeg command line; run with NR_STILL_FIXTURES set"]
fn still_parity_with_the_image_generator() {
    let folder = fixtures();
    let lavfi = |source: String| vec!["-f".to_owned(), "lavfi".to_owned(), "-i".to_owned(), source];
    let mut files: Vec<(&str, String)> = Vec::new();
    let mut add = |name: &'static str, inputs: Vec<String>, encode: &[&str]| {
        let arguments: Vec<&str> = inputs
            .iter()
            .map(String::as_str)
            .chain(encode.iter().copied())
            .collect();
        files.push((name, make(&folder, name, &arguments)));
    };
    add(
        "h264.mp4",
        lavfi(picture("1280x720", 30)),
        &["-c:v", "libx264", "-bf", "3", "-pix_fmt", "yuv420p"],
    );
    add(
        "odd.mp4",
        lavfi(picture("1000x562", 25)),
        &["-c:v", "libx264", "-pix_fmt", "yuv420p"],
    );
    add(
        "portrait.m4v",
        lavfi(picture("720x1280", 30)),
        &["-c:v", "libx264", "-pix_fmt", "yuv420p"],
    );
    add(
        "h264-10.mp4",
        lavfi(picture("640x360", 30)),
        &["-c:v", "libx264", "-pix_fmt", "yuv420p10le"],
    );
    add(
        "hevc8.mp4",
        lavfi(picture("640x360", 30)),
        &[
            "-c:v",
            "libx265",
            "-pix_fmt",
            "yuv420p",
            "-tag:v",
            "hvc1",
            "-x265-params",
            "log-level=error",
        ],
    );
    add(
        "hevc10.mov",
        lavfi(picture("1920x1080", 24)),
        &[
            "-c:v",
            "libx265",
            "-pix_fmt",
            "yuv420p10le",
            "-tag:v",
            "hvc1",
            "-x265-params",
            "log-level=error",
        ],
    );
    add(
        "prores.mov",
        lavfi(picture("640x360", 30)),
        &["-c:v", "prores_ks", "-profile:v", "2"],
    );
    add(
        "prores4444.mov",
        lavfi(picture("640x360", 30)),
        &[
            "-c:v",
            "prores_ks",
            "-profile:v",
            "4",
            "-pix_fmt",
            "yuva444p10le",
        ],
    );
    add(
        "av1.mp4",
        lavfi(picture("640x360", 30)),
        &["-c:v", "libsvtav1", "-svtav1-params", "loglevel=0"],
    );
    let h264 = files[0].1.clone();
    let hevc10 = files[5].1.clone();
    // Phone portrait clips: stored sideways, turned by the track.
    for (name, turn) in [
        ("turned-90.mp4", "90"),
        ("turned-180.mp4", "180"),
        ("turned-270.mp4", "-90"),
    ] {
        files.push((
            name,
            make(
                &folder,
                name,
                &["-display_rotation", turn, "-i", &h264, "-c", "copy"],
            ),
        ));
    }
    files.push((
        "turned-10bit.mov",
        make(
            &folder,
            "turned-10bit.mov",
            &["-display_rotation", "90", "-i", &hevc10, "-c", "copy"],
        ),
    ));
    // Mirrored, as a front camera may store it, and mirrored plus turned.
    files.push((
        "mirrored.mp4",
        make(
            &folder,
            "mirrored.mp4",
            &["-display_hflip", "-i", &h264, "-c", "copy"],
        ),
    ));
    files.push((
        "mirrored-90.mp4",
        make(
            &folder,
            "mirrored-90.mp4",
            &[
                "-display_rotation",
                "90",
                "-display_hflip",
                "-i",
                &h264,
                "-c",
                "copy",
            ],
        ),
    ));
    files.push((
        "mirrored-vertically.mp4",
        make(
            &folder,
            "mirrored-vertically.mp4",
            &["-display_vflip", "-i", &h264, "-c", "copy"],
        ),
    ));
    let mut failures = Vec::new();
    for (name, path) in &files {
        for max in SIZES {
            let theirs = generator_picture(path, max)
                .unwrap_or_else(|| panic!("the generator pictures {name}"));
            let ours = still(path, 0.0, max).unwrap_or_else(|error| panic!("{name}: {error}"));
            match difference(&ours, &theirs) {
                None => println!("{name} at {max}: {:?} identical", size_of(&ours)),
                Some(difference) => failures.push(format!("{name} at {max}: {difference}")),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "stills differ from the generator's: {failures:#?}"
    );
}

/// Matroska stills against the generator's stills of the same stream copied into an MP4. Where
/// the two format descriptions agree, the pictures are byte-identical. One case differs: the
/// reader declares a 10-bit VP9's depth where AVFoundation doesn't (so it is pictured in half
/// floats, like 10-bit HEVC), and only its size is checked against the twin. Both read an
/// HEVC's chroma siting from its parameter sets.
#[test]
#[ignore = "makes its files with the FFmpeg command line; run with NR_STILL_FIXTURES set"]
fn still_parity_matroska_against_the_same_stream_in_mp4() {
    let folder = fixtures();
    let lavfi = |source: String| vec!["-f".to_owned(), "lavfi".to_owned(), "-i".to_owned(), source];
    let mut failures = Vec::new();
    const HD: &str = "1280x720";
    // Standard definition, untagged (BT.601 assumed) and tagged as BT.601 and as BT.470BG.
    const SD: &str = "640x480";
    const PAL: &str = "720x576";
    let pairs: [(&str, &str, Described, &str, Vec<&str>); 9] = [
        (
            "h264.mkv",
            "h264-twin.mp4",
            Described::Alike,
            HD,
            vec!["-c:v", "libx264", "-bf", "3", "-pix_fmt", "yuv420p"],
        ),
        (
            "hevc10.mkv",
            "hevc10-twin.mp4",
            Described::Alike,
            HD,
            vec![
                "-c:v",
                "libx265",
                "-pix_fmt",
                "yuv420p10le",
                "-x265-params",
                "log-level=error",
            ],
        ),
        (
            "vp9.webm",
            "vp9-twin.mp4",
            Described::Alike,
            HD,
            vec![
                "-c:v",
                "libvpx-vp9",
                "-b:v",
                "1M",
                "-deadline",
                "realtime",
                "-pix_fmt",
                "yuv420p",
            ],
        ),
        (
            "vp9-10.webm",
            "vp9-10-twin.mp4",
            Described::Otherwise,
            HD,
            vec![
                "-c:v",
                "libvpx-vp9",
                "-b:v",
                "1M",
                "-deadline",
                "realtime",
                "-pix_fmt",
                "yuv420p10le",
            ],
        ),
        (
            "av1.webm",
            "av1-twin.mp4",
            Described::Alike,
            HD,
            vec!["-c:v", "libsvtav1", "-svtav1-params", "loglevel=0"],
        ),
        (
            "h264-sd.mkv",
            "h264-sd-twin.mp4",
            Described::Alike,
            SD,
            vec!["-c:v", "libx264", "-pix_fmt", "yuv420p"],
        ),
        (
            "h264-sd-170m.mkv",
            "h264-sd-170m-twin.mp4",
            Described::Alike,
            SD,
            vec![
                "-vf",
                "setparams=color_primaries=smpte170m:color_trc=smpte170m:colorspace=smpte170m",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ],
        ),
        (
            "h264-pal-470bg.mkv",
            "h264-pal-470bg-twin.mp4",
            Described::Alike,
            PAL,
            vec![
                "-vf",
                "setparams=color_primaries=bt470bg:color_trc=bt470bg:colorspace=bt470bg",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ],
        ),
        (
            "vp9-sd.webm",
            "vp9-sd-twin.mp4",
            Described::Alike,
            SD,
            vec![
                "-c:v",
                "libvpx-vp9",
                "-b:v",
                "1M",
                "-deadline",
                "realtime",
                "-pix_fmt",
                "yuv420p",
            ],
        ),
    ];
    let mut files: Vec<(&str, &str, Described, String, String)> = pairs
        .into_iter()
        .map(|(matroska, twin, described, size, encode)| {
            let arguments: Vec<String> = lavfi(picture(size, 30))
                .into_iter()
                .chain(encode.iter().map(|part| (*part).to_owned()))
                .collect();
            let (matroska_path, twin_path) = make_with_twin(&folder, matroska, twin, &arguments);
            (matroska, twin, described, matroska_path, twin_path)
        })
        .collect();
    // The stream says BT.470BG and neither container does: the decoder tags its pictures with
    // the code point Core Video has no name for. Both are restamped copies of the untagged pair
    // (the filter leaves the container's colour unset).
    let restamp = |from: &str, out: &str| {
        let from = folder.join(from).to_string_lossy().into_owned();
        make(
            &folder,
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
    let restamped = (
        restamp("h264-sd.mkv", "h264-sd-vui-470bg.mkv"),
        restamp("h264-sd-twin.mp4", "h264-sd-vui-470bg-twin.mp4"),
    );
    files.push((
        "h264-sd-vui-470bg.mkv",
        "h264-sd-vui-470bg-twin.mp4",
        Described::Alike,
        restamped.0,
        restamped.1,
    ));
    for (matroska, twin, described, matroska_path, twin_path) in files {
        for max in SIZES {
            let ours = still(&matroska_path, 0.0, max)
                .unwrap_or_else(|error| panic!("{matroska}: {error}"));
            let theirs = generator_picture(&twin_path, max)
                .unwrap_or_else(|| panic!("the generator pictures {twin}"));
            let pair = StillPair {
                matroska,
                twin,
                max,
            };
            failures.extend(compare_twin_stills(&pair, described, &ours, &theirs));
        }
    }
    assert!(
        failures.is_empty(),
        "Matroska stills differ from their twins': {failures:#?}"
    );
}

/// Whether a Matroska file's format description matches its MP4 twin's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Described {
    Alike,
    Otherwise,
}

/// Makes a Matroska file with `arguments`, then its twin: the same stream copied into an MP4.
fn make_with_twin(
    folder: &Path,
    matroska: &str,
    twin: &str,
    arguments: &[String],
) -> (String, String) {
    let matroska_path = make(
        folder,
        matroska,
        &arguments.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let mut copy = vec!["-copyts", "-i", matroska_path.as_str(), "-c", "copy"];
    if twin.starts_with("hevc") {
        copy.extend(["-tag:v", "hvc1"]);
    }
    let twin_path = make(folder, twin, &copy);
    (matroska_path, twin_path)
}

/// A Matroska file and its twin, pictured at most `max` pixels.
struct StillPair<'a> {
    matroska: &'a str,
    twin: &'a str,
    max: u32,
}

/// What is wrong with a Matroska still compared with its twin's: described alike, they must be
/// the same bytes; described differently, the same size.
fn compare_twin_stills(
    pair: &StillPair,
    described: Described,
    ours: &Still,
    theirs: &Still,
) -> Option<String> {
    let StillPair {
        matroska,
        twin,
        max,
    } = pair;
    match described {
        Described::Otherwise => {
            println!(
                "{matroska} at {max}: {:?} as {twin}, described otherwise: {:?}",
                size_of(ours),
                difference(ours, theirs)
            );
            (size_of(ours) != size_of(theirs)).then(|| {
                format!(
                    "{matroska} at {max}: {:?} against {:?}",
                    size_of(ours),
                    size_of(theirs)
                )
            })
        }
        Described::Alike => {
            let difference = difference(ours, theirs);
            if difference.is_none() {
                println!(
                    "{matroska} at {max}: {:?} identical to the generator's picture of {twin}",
                    size_of(ours)
                );
            }
            difference.map(|difference| format!("{matroska} at {max}: {difference}"))
        }
    }
}
