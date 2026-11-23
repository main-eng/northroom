//! The AVFoundation, Core Media and Core Foundation calls the Mac backend makes: an asset and
//! its tracks, a reader of one track from a given time, and the sample buffers it returns.
//! Objective-C goes through `objc2` messages; everything else is plain C.

use crate::media::{MediaError, MediaResult};
use objc2::encode::{Encode, Encoding};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use std::ffi::{c_char, c_void, CStr};
use std::ptr;

pub(super) type CFTypeRef = *const c_void;
pub(super) type OSStatus = i32;

#[repr(C)]
struct OpaqueCallBacks {
    _private: [u8; 0],
}

const STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const NUMBER_TYPE_SINT64: isize = 4;
const NUMBER_TYPE_FLOAT64: isize = 6;

#[allow(non_upper_case_globals)]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: OpaqueCallBacks;
    static kCFTypeDictionaryValueCallBacks: OpaqueCallBacks;
    pub(super) static kCFBooleanTrue: CFTypeRef;
    pub(super) static kCFBooleanFalse: CFTypeRef;
    fn CFDictionaryCreateMutable(
        allocator: CFTypeRef,
        capacity: isize,
        keys: *const OpaqueCallBacks,
        values: *const OpaqueCallBacks,
    ) -> CFTypeRef;
    fn CFDictionarySetValue(dictionary: CFTypeRef, key: CFTypeRef, value: CFTypeRef);
    pub(super) fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    pub(super) fn CFArrayGetCount(array: CFTypeRef) -> isize;
    pub(super) fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
    pub(super) fn CFGetTypeID(object: CFTypeRef) -> usize;
    pub(super) fn CFBooleanGetTypeID() -> usize;
    pub(super) fn CFBooleanGetValue(boolean: CFTypeRef) -> u8;
    pub(super) fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: CFTypeRef, number_type: isize, value: *mut c_void) -> u8;
    fn CFNumberCreate(allocator: CFTypeRef, number_type: isize, value: *const c_void) -> CFTypeRef;
    pub(super) fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        text: *const c_char,
        encoding: u32,
    ) -> CFTypeRef;
    fn CFStringGetCString(string: CFTypeRef, buffer: *mut c_char, size: isize, encoding: u32)
        -> u8;
    pub(super) fn CFDataGetTypeID() -> usize;
    fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: isize) -> CFTypeRef;
    pub(super) fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    pub(super) fn CFDataGetLength(data: CFTypeRef) -> isize;
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CFTypeRef,
        path: *const u8,
        length: isize,
        directory: u8,
    ) -> CFTypeRef;
    fn CFRetain(object: CFTypeRef) -> CFTypeRef;
    pub(super) fn CFRelease(object: CFTypeRef);
}

/// A Core Foundation object this code holds one reference to.
pub(super) struct Cf(pub(super) CFTypeRef);

// SAFETY: Core Foundation reference counting is thread-safe, and the objects held here (strings,
// numbers, dictionaries built once, format descriptions, sample buffers) are not mutated after
// they are handed over.
unsafe impl Send for Cf {}

impl Cf {
    pub(super) fn created(object: CFTypeRef, what: &str) -> MediaResult<Self> {
        if object.is_null() {
            Err(MediaError(format!("create {what}")))
        } else {
            Ok(Self(object))
        }
    }

    /// Retains `object`, which may be null.
    ///
    /// # Safety
    /// `object` must be null or a live Core Foundation object.
    pub(super) unsafe fn retained(object: CFTypeRef) -> Option<Self> {
        // SAFETY: as the caller guarantees.
        (!object.is_null()).then(|| Self(unsafe { CFRetain(object) }))
    }

    /// The file URL of `path`.
    pub(super) fn file_url(path: &str) -> MediaResult<Self> {
        // SAFETY: the bytes are a valid path of the given length.
        Self::created(
            unsafe {
                CFURLCreateFromFileSystemRepresentation(
                    ptr::null(),
                    path.as_ptr(),
                    path.len() as isize,
                    0,
                )
            },
            "file URL",
        )
    }

    pub(super) fn string(text: &str) -> MediaResult<Self> {
        let text = std::ffi::CString::new(text)
            .map_err(|_| MediaError("a string holds a NUL byte".into()))?;
        // SAFETY: `text` is a NUL-terminated UTF-8 string.
        Self::created(
            unsafe { CFStringCreateWithCString(ptr::null(), text.as_ptr(), STRING_ENCODING_UTF8) },
            "string",
        )
    }

    pub(super) fn data(bytes: &[u8]) -> MediaResult<Self> {
        // SAFETY: Core Foundation copies `bytes.len()` bytes from a valid slice.
        Self::created(
            unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize) },
            "data",
        )
    }

    pub(super) fn integer(value: i64) -> MediaResult<Self> {
        // SAFETY: Core Foundation copies one 64-bit integer from a valid pointer.
        Self::created(
            unsafe { CFNumberCreate(ptr::null(), NUMBER_TYPE_SINT64, (&raw const value).cast()) },
            "number",
        )
    }

    pub(super) fn real(value: f64) -> MediaResult<Self> {
        // SAFETY: Core Foundation copies one 64-bit float from a valid pointer.
        Self::created(
            unsafe { CFNumberCreate(ptr::null(), NUMBER_TYPE_FLOAT64, (&raw const value).cast()) },
            "number",
        )
    }

    /// A dictionary of `entries`, which retains them.
    pub(super) fn dictionary(entries: &[(CFTypeRef, CFTypeRef)]) -> MediaResult<Self> {
        // SAFETY: the standard callback tables make the dictionary retain its keys and values,
        // each a live object the caller holds.
        unsafe {
            let dictionary = Self::created(
                CFDictionaryCreateMutable(
                    ptr::null(),
                    0,
                    &raw const kCFTypeDictionaryKeyCallBacks,
                    &raw const kCFTypeDictionaryValueCallBacks,
                ),
                "dictionary",
            )?;
            for (key, value) in entries {
                CFDictionarySetValue(dictionary.0, *key, *value);
            }
            Ok(dictionary)
        }
    }

    /// The object as an Objective-C object; every Core Foundation type bridged here is one.
    pub(super) fn object(&self) -> *const AnyObject {
        self.0.cast()
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: releases the one reference this value owns.
        unsafe { CFRelease(self.0) };
    }
}

/// The text of a Core Foundation string, or `None` if `object` isn't one.
///
/// # Safety
/// `object` must be null or a live Core Foundation object.
pub(super) unsafe fn string_of(object: CFTypeRef) -> Option<String> {
    // SAFETY: as the caller guarantees; the buffer is writable for its whole length.
    unsafe {
        if object.is_null() || CFGetTypeID(object) != CFStringGetTypeID() {
            return None;
        }
        let mut buffer = [0 as c_char; 256];
        (CFStringGetCString(
            object,
            buffer.as_mut_ptr(),
            buffer.len() as isize,
            STRING_ENCODING_UTF8,
        ) != 0)
            .then(|| {
                CStr::from_ptr(buffer.as_ptr())
                    .to_string_lossy()
                    .into_owned()
            })
    }
}

/// The value of a Core Foundation number or boolean, or `None` if `object` is neither.
///
/// # Safety
/// `object` must be null or a live Core Foundation object.
pub(super) unsafe fn number_of(object: CFTypeRef) -> Option<f64> {
    // SAFETY: as the caller guarantees; the out-pointer is valid for one double.
    unsafe {
        if object.is_null() {
            return None;
        }
        let kind = CFGetTypeID(object);
        if kind == CFBooleanGetTypeID() {
            return Some(f64::from(CFBooleanGetValue(object)));
        }
        if kind != CFNumberGetTypeID() {
            return None;
        }
        let mut value = 0.0_f64;
        (CFNumberGetValue(object, NUMBER_TYPE_FLOAT64, (&raw mut value).cast()) != 0)
            .then_some(value)
    }
}

/// The bytes of a Core Foundation data object, or `None` if `object` isn't one.
///
/// # Safety
/// `object` must be null or a live Core Foundation object, alive while the slice is used.
pub(super) unsafe fn bytes_of<'a>(object: CFTypeRef) -> Option<&'a [u8]> {
    // SAFETY: as the caller guarantees; Core Foundation keeps the bytes for the object's life.
    unsafe {
        if object.is_null() || CFGetTypeID(object) != CFDataGetTypeID() {
            return None;
        }
        Some(std::slice::from_raw_parts(
            CFDataGetBytePtr(object),
            CFDataGetLength(object).max(0) as usize,
        ))
    }
}

/// Core Media's rational time.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CMTime {
    pub(super) value: i64,
    pub(super) timescale: i32,
    pub(super) flags: u32,
    pub(super) epoch: i64,
}

// SAFETY: the layout above is Core Media's `CMTime`, an anonymous struct of these four fields.
unsafe impl Encode for CMTime {
    const ENCODING: Encoding = Encoding::Struct(
        "?",
        &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING],
    );
}

/// `kCMTimeFlags_Valid`.
const TIME_VALID: u32 = 1;
/// `kCMTimeFlags_PositiveInfinity`.
const TIME_POSITIVE_INFINITY: u32 = 4;

impl CMTime {
    pub(super) const POSITIVE_INFINITY: Self = Self {
        value: 0,
        timescale: 0,
        flags: TIME_VALID | TIME_POSITIVE_INFINITY,
        epoch: 0,
    };

    /// `value / timescale` seconds.
    pub(super) const fn new(value: i64, timescale: i32) -> Self {
        Self {
            value,
            timescale,
            flags: TIME_VALID,
            epoch: 0,
        }
    }

    /// A finite time, as close as a timescale of one billion can represent it.
    pub(super) fn from_seconds(seconds: f64) -> Self {
        Self::new((seconds * 1e9).round() as i64, 1_000_000_000)
    }

    /// A valid, finite time.
    pub(super) fn is_numeric(self) -> bool {
        self.flags & 0x1d == TIME_VALID && self.timescale > 0
    }

    pub(super) fn seconds(self) -> Option<f64> {
        self.is_numeric()
            .then(|| self.value as f64 / f64::from(self.timescale))
    }

    /// The time in units of `timescale`, rounded to nearest, for keys that must compare exactly
    /// when every time shares one timescale.
    pub(super) fn ticks(self, timescale: i32) -> i64 {
        if self.timescale == timescale {
            return self.value;
        }
        let scaled = i128::from(self.value) * i128::from(timescale);
        let denominator = i128::from(self.timescale);
        ((scaled + denominator / 2).div_euclid(denominator)) as i64
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CMTimeRange {
    pub(super) start: CMTime,
    pub(super) duration: CMTime,
}

// SAFETY: Core Media's `CMTimeRange`, an anonymous struct of two `CMTime`s.
unsafe impl Encode for CMTimeRange {
    const ENCODING: Encoding = Encoding::Struct("?", &[CMTime::ENCODING, CMTime::ENCODING]);
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CMTimeMapping {
    source: CMTimeRange,
    target: CMTimeRange,
}

// SAFETY: Core Media's `CMTimeMapping`, an anonymous struct of two `CMTimeRange`s.
unsafe impl Encode for CMTimeMapping {
    const ENCODING: Encoding =
        Encoding::Struct("?", &[CMTimeRange::ENCODING, CMTimeRange::ENCODING]);
}

/// `CGAffineTransform`: how a track's pictures are turned and placed for display, in the
/// picture's own top-left coordinate space.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Transform {
    pub(super) a: f64,
    pub(super) b: f64,
    pub(super) c: f64,
    pub(super) d: f64,
    // Where the turned picture is placed. Unused, since each picture is turned into its own
    // buffer.
    pub(super) _tx: f64,
    pub(super) _ty: f64,
}

// SAFETY: Core Graphics' `CGAffineTransform`, a struct of six `CGFloat`s (doubles on a Mac).
unsafe impl Encode for Transform {
    const ENCODING: Encoding = Encoding::Struct("CGAffineTransform", &[f64::ENCODING; 6]);
}

impl Transform {
    pub(super) const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        _tx: 0.0,
        _ty: 0.0,
    };

    /// The size of a `width` by `height` picture once turned, as `naturalSize` reports it with
    /// the transform applied.
    pub(super) fn shown_size(&self, width: u32, height: u32) -> (u32, u32) {
        let (width, height) = (f64::from(width), f64::from(height));
        let shown_width = (self.a * width + self.c * height).abs().round();
        let shown_height = (self.b * width + self.d * height).abs().round();
        (shown_width as u32, shown_height as u32)
    }
}

/// `CMVideoDimensions`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Dimensions {
    pub(super) width: i32,
    pub(super) height: i32,
}

/// The fields of `AudioStreamBasicDescription` read here, laid out as the full struct.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct StreamDescription {
    pub(super) sample_rate: f64,
    pub(super) format_id: u32,
    pub(super) format_flags: u32,
    pub(super) bytes_per_packet: u32,
    pub(super) frames_per_packet: u32,
    pub(super) bytes_per_frame: u32,
    pub(super) channels_per_frame: u32,
    pub(super) bits_per_channel: u32,
    pub(super) reserved: u32,
}

#[allow(non_upper_case_globals)]
#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    pub(super) static kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: CFTypeRef;
    pub(super) static kCMFormatDescriptionExtension_BitsPerComponent: CFTypeRef;
    pub(super) static kCMFormatDescriptionExtension_FullRangeVideo: CFTypeRef;
    pub(super) fn CMFormatDescriptionGetMediaSubType(format: CFTypeRef) -> u32;
    pub(super) fn CMFormatDescriptionGetExtension(format: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    pub(super) fn CMVideoFormatDescriptionGetDimensions(format: CFTypeRef) -> Dimensions;
    pub(super) fn CMAudioFormatDescriptionGetStreamBasicDescription(
        format: CFTypeRef,
    ) -> *const StreamDescription;
    fn CMSampleBufferGetPresentationTimeStamp(sample: CFTypeRef) -> CMTime;
    fn CMSampleBufferGetDecodeTimeStamp(sample: CFTypeRef) -> CMTime;
    fn CMSampleBufferGetNumSamples(sample: CFTypeRef) -> isize;
    fn CMSampleBufferGetDataBuffer(sample: CFTypeRef) -> CFTypeRef;
    fn CMBlockBufferGetDataLength(block: CFTypeRef) -> usize;
    fn CMBlockBufferCopyDataBytes(
        block: CFTypeRef,
        offset: usize,
        length: usize,
        destination: *mut c_void,
    ) -> OSStatus;
}

#[allow(non_upper_case_globals)]
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    pub(super) static kCVPixelBufferPixelFormatTypeKey: CFTypeRef;
    pub(super) static kCVPixelBufferIOSurfacePropertiesKey: CFTypeRef;
    pub(super) static kCVPixelBufferMetalCompatibilityKey: CFTypeRef;
    pub(super) static kCVPixelBufferWidthKey: CFTypeRef;
    pub(super) static kCVPixelBufferHeightKey: CFTypeRef;
    pub(super) static kCVImageBufferColorPrimariesKey: CFTypeRef;
    pub(super) static kCVImageBufferTransferFunctionKey: CFTypeRef;
    pub(super) static kCVImageBufferYCbCrMatrixKey: CFTypeRef;
    pub(super) static kCVImageBufferChromaLocationTopFieldKey: CFTypeRef;
    /// `CVBufferCopyAttachment`: the attachment by key, +1, or null.
    pub(super) fn CVBufferCopyAttachment(
        buffer: CFTypeRef,
        key: CFTypeRef,
        mode: *mut u32,
    ) -> CFTypeRef;
}

#[allow(non_upper_case_globals)]
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeVideo: *const AnyObject;
    static AVMediaTypeAudio: *const AnyObject;
    static AVURLAssetPreferPreciseDurationAndTimingKey: CFTypeRef;
    pub(super) static AVFormatIDKey: CFTypeRef;
    pub(super) static AVSampleRateKey: CFTypeRef;
    pub(super) static AVNumberOfChannelsKey: CFTypeRef;
    pub(super) static AVLinearPCMBitDepthKey: CFTypeRef;
    pub(super) static AVLinearPCMIsFloatKey: CFTypeRef;
    pub(super) static AVLinearPCMIsNonInterleaved: CFTypeRef;
    pub(super) static AVLinearPCMIsBigEndianKey: CFTypeRef;
}

/// An Objective-C string's text.
///
/// # Safety
/// `string` must be null or a live `NSString`.
unsafe fn text_of(string: *const AnyObject) -> String {
    if string.is_null() {
        return String::new();
    }
    // SAFETY: as the caller guarantees; the returned C string lives at least as long as `string`.
    unsafe {
        let text: *const c_char = msg_send![string, UTF8String];
        if text.is_null() {
            String::new()
        } else {
            CStr::from_ptr(text).to_string_lossy().into_owned()
        }
    }
}

/// An `NSError` as text, with its domain's code.
///
/// # Safety
/// `error` must be null or a live `NSError`.
unsafe fn error_text(error: *const AnyObject) -> String {
    if error.is_null() {
        return "no reason given".into();
    }
    // SAFETY: as the caller guarantees.
    unsafe {
        let description: *const AnyObject = msg_send![error, localizedDescription];
        let code: isize = msg_send![error, code];
        let underlying: *const AnyObject = msg_send![error, userInfo];
        let reason = if underlying.is_null() {
            String::new()
        } else {
            let key = Cf::string("NSUnderlyingError").ok();
            let inner = key.map_or(ptr::null(), |key| {
                CFDictionaryGetValue(underlying.cast(), key.0)
            });
            if inner.is_null() {
                String::new()
            } else {
                let inner_code: isize = msg_send![inner.cast::<AnyObject>(), code];
                format!(", status {inner_code}")
            }
        };
        format!("{} (code {code}{reason})", text_of(description))
    }
}

/// A track's media kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TrackKind {
    Video,
    Audio,
}

/// A file opened as an `AVURLAsset` with precise timing, so seeks land on samples.
pub(super) struct Asset(Retained<AnyObject>);

// SAFETY: an AVAsset is immutable once created and documented as safe to read from any thread.
unsafe impl Send for Asset {}

impl Asset {
    pub(super) fn open(path: &str) -> MediaResult<Self> {
        objc2::rc::autoreleasepool(|_| {
            let url = Cf::file_url(path)?;
            // SAFETY: the key is a framework constant, the value a boolean.
            let options = Cf::dictionary(&[(
                unsafe { AVURLAssetPreferPreciseDurationAndTimingKey },
                unsafe { kCFBooleanTrue },
            )])?;
            // SAFETY: `url` and `options` are live; `initWithURL:options:` returns an owned
            // asset or nil.
            let asset: Option<Retained<AnyObject>> = unsafe {
                let allocated: Allocated<AnyObject> = msg_send![class!(AVURLAsset), alloc];
                msg_send![allocated, initWithURL: url.object(), options: options.object()]
            };
            asset
                .map(Self)
                .ok_or_else(|| MediaError(format!("open {path}")))
        })
    }

    /// The asset's duration: its own, or where its longest track ends if later (a movie header
    /// can omit an AAC track's priming).
    pub(super) fn duration_seconds(&self) -> Option<f64> {
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: the asset is live; `duration` and each track's `timeRange` are returned by
            // value, and the tracks array lives until the pool drains.
            unsafe {
                let duration: CMTime = msg_send![&*self.0, duration];
                let tracks: *const AnyObject = msg_send![&*self.0, tracks];
                let count = if tracks.is_null() {
                    0
                } else {
                    CFArrayGetCount(tracks.cast())
                };
                let ends = (0..count).filter_map(|index| {
                    let track = CFArrayGetValueAtIndex(tracks.cast(), index).cast::<AnyObject>();
                    let range: CMTimeRange = msg_send![track, timeRange];
                    Some(range.start.seconds()? + range.duration.seconds()?)
                });
                ends.chain(duration.seconds()).reduce(f64::max)
            }
        })
    }

    /// The asset's first track of `kind`, or `None` if it has none.
    pub(super) fn first_track(&self, kind: TrackKind) -> Option<Track> {
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: the asset and the framework constants are live; `tracks` is an array of
            // tracks the asset keeps, each retained here before the pool drains.
            unsafe {
                let wanted = match kind {
                    TrackKind::Video => AVMediaTypeVideo,
                    TrackKind::Audio => AVMediaTypeAudio,
                };
                let tracks: *const AnyObject = msg_send![&*self.0, tracks];
                if tracks.is_null() {
                    return None;
                }
                (0..CFArrayGetCount(tracks.cast())).find_map(|index| {
                    let track = CFArrayGetValueAtIndex(tracks.cast(), index).cast::<AnyObject>();
                    let media_type: *const AnyObject = msg_send![track, mediaType];
                    let same: Bool = msg_send![media_type, isEqualToString: wanted];
                    same.as_bool()
                        .then(|| Retained::retain(track.cast_mut()))
                        .flatten()
                        .map(Track)
                })
            }
        })
    }
}

/// One track of an asset.
pub(super) struct Track(Retained<AnyObject>);

// SAFETY: like `Asset`, a track is an immutable part of its asset.
unsafe impl Send for Track {}

impl Track {
    /// The track's first format description, retained.
    pub(super) fn format_description(&self) -> Option<Cf> {
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: the track is live; the array and its descriptions live until the pool
            // drains, and the one kept is retained first.
            unsafe {
                let formats: *const AnyObject = msg_send![&*self.0, formatDescriptions];
                if formats.is_null() || CFArrayGetCount(formats.cast()) == 0 {
                    return None;
                }
                Cf::retained(CFArrayGetValueAtIndex(formats.cast(), 0))
            }
        })
    }

    /// The timescale of the track's sample times.
    pub(super) fn media_timescale(&self) -> i32 {
        // SAFETY: the track is live; the property is a `CMTimeScale`.
        let timescale: i32 = unsafe { msg_send![&*self.0, naturalTimeScale] };
        timescale.max(1)
    }

    pub(super) fn nominal_frame_rate(&self) -> f32 {
        // SAFETY: the track is live; the property is a float.
        unsafe { msg_send![&*self.0, nominalFrameRate] }
    }

    /// How the track's pictures are turned for display (a phone's portrait clip is stored
    /// sideways and turned by this).
    pub(super) fn preferred_transform(&self) -> Transform {
        // SAFETY: the track is live; the property is a `CGAffineTransform`.
        unsafe { msg_send![&*self.0, preferredTransform] }
    }

    /// How the track's media times map to the asset's: the track's non-empty edits, in order.
    pub(super) fn edits(&self) -> Edits {
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: the track is live; each segment is read while the pool holds the array.
            unsafe {
                let segments: *const AnyObject = msg_send![&*self.0, segments];
                let count = if segments.is_null() {
                    0
                } else {
                    CFArrayGetCount(segments.cast())
                };
                let mut edits = Vec::new();
                for index in 0..count {
                    let segment =
                        CFArrayGetValueAtIndex(segments.cast(), index).cast::<AnyObject>();
                    let empty: Bool = msg_send![segment, isEmpty];
                    let mapping: CMTimeMapping = msg_send![segment, timeMapping];
                    if let (false, Some(media), Some(target), Some(length)) = (
                        empty.as_bool(),
                        mapping.source.start.seconds(),
                        mapping.target.start.seconds(),
                        mapping.source.duration.seconds(),
                    ) {
                        edits.push(Edit {
                            media,
                            target,
                            length,
                        });
                    }
                }
                Edits(edits)
            }
        })
    }
}

/// One edit of a track: `length` seconds of media from `media`, shown from `target` on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Edit {
    media: f64,
    target: f64,
    length: f64,
}

/// A track's edits, which place its media on the asset's clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Edits(Vec<Edit>);

impl Edits {
    /// A media time on the asset's clock, through the edit that contains it or the nearest one.
    /// A track with no edits shows its media unchanged.
    pub(super) fn asset_seconds(&self, media: f64) -> f64 {
        let edit = self
            .0
            .iter()
            .find(|edit| media >= edit.media && media < edit.media + edit.length)
            .or_else(|| {
                self.0
                    .iter()
                    .min_by(|a, b| distance(a, media).total_cmp(&distance(b, media)))
            });
        edit.map_or(media, |edit| edit.target + (media - edit.media))
    }
}

fn distance(edit: &Edit, media: f64) -> f64 {
    if media < edit.media {
        edit.media - media
    } else {
        (media - (edit.media + edit.length)).max(0.0)
    }
}

/// `AVAssetReaderStatus`.
const READER_FAILED: isize = 3;

/// An `AVAssetReader` with one output, reading one track from a given time to the end.
pub(super) struct Reader {
    reader: Retained<AnyObject>,
    output: Retained<AnyObject>,
}

// SAFETY: an asset reader is used from one thread at a time, which `&mut self` ensures; the
// framework allows that thread to change.
unsafe impl Send for Reader {}

impl Reader {
    /// Reads `track` of `asset` from `start` on the asset's clock, or from the beginning.
    /// `settings` are the output's decode settings, or `None` for the samples as stored.
    pub(super) fn start_at(
        asset: &Asset,
        track: &Track,
        settings: Option<&Cf>,
        start: Option<CMTime>,
    ) -> MediaResult<Self> {
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: asset, track and settings are live objects; every init returns an owned
            // object or nil; the error out-pointer is valid and its object autoreleased.
            unsafe {
                let mut error: *mut AnyObject = ptr::null_mut();
                let allocated: Allocated<AnyObject> = msg_send![class!(AVAssetReader), alloc];
                let reader: Option<Retained<AnyObject>> =
                    msg_send![allocated, initWithAsset: &*asset.0, error: &mut error];
                let reader = reader
                    .ok_or_else(|| MediaError(format!("read the file: {}", error_text(error))))?;
                let allocated: Allocated<AnyObject> =
                    msg_send![class!(AVAssetReaderTrackOutput), alloc];
                let settings = settings.map_or(ptr::null(), Cf::object);
                let output: Option<Retained<AnyObject>> =
                    msg_send![allocated, initWithTrack: &*track.0, outputSettings: settings];
                let output = output.ok_or_else(|| MediaError("make the track's reader".into()))?;
                // The samples are passed on unchanged, so the reader needn't copy them.
                let _: () = msg_send![&*output, setAlwaysCopiesSampleData: Bool::NO];
                let can_add: Bool = msg_send![&*reader, canAddOutput: &*output];
                if !can_add.as_bool() {
                    return Err(MediaError("the track cannot be read that way".into()));
                }
                let _: () = msg_send![&*reader, addOutput: &*output];
                if let Some(start) = start {
                    let range = CMTimeRange {
                        start,
                        duration: CMTime::POSITIVE_INFINITY,
                    };
                    let _: () = msg_send![&*reader, setTimeRange: range];
                }
                let started: Bool = msg_send![&*reader, startReading];
                if !started.as_bool() {
                    let error: *const AnyObject = msg_send![&*reader, error];
                    return Err(MediaError(format!("start reading: {}", error_text(error))));
                }
                Ok(Self { reader, output })
            }
        })
    }

    /// The next sample buffer, or `None` at the end.
    pub(super) fn next(&mut self) -> MediaResult<Option<Sample>> {
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: the output is live and attached to a reader that is reading;
            // `copyNextSampleBuffer` returns an owned buffer or null.
            unsafe {
                let sample: SampleRef = msg_send![&*self.output, copyNextSampleBuffer];
                if !sample.0.is_null() {
                    return Ok(Some(Sample(Cf(sample.0))));
                }
                let status: isize = msg_send![&*self.reader, status];
                if status == READER_FAILED {
                    let error: *const AnyObject = msg_send![&*self.reader, error];
                    return Err(MediaError(format!(
                        "the file could not be read: {}",
                        error_text(error)
                    )));
                }
                Ok(None)
            }
        })
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: the reader is live; cancelling a reader that has finished does nothing.
        unsafe {
            let _: () = msg_send![&*self.reader, cancelReading];
        }
    }
}

/// A `CMSampleBufferRef` as an Objective-C method returns it.
#[repr(transparent)]
struct SampleRef(CFTypeRef);

// SAFETY: a pointer to Core Media's opaque sample buffer, as `copyNextSampleBuffer` declares it.
unsafe impl Encode for SampleRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("opaqueCMSampleBuffer", &[]));
}

/// One sample buffer: a compressed sample, or a run of decoded sound.
pub(super) struct Sample(Cf);

/// `CMSampleTimingInfo`.
#[repr(C)]
struct SampleTiming {
    duration: CMTime,
    presentation: CMTime,
    decode: CMTime,
}

/// `kCMAttachmentMode_ShouldNotPropagate`.
const ATTACHMENT_DOES_NOT_PROPAGATE: u32 = 0;

/// How the decoder should treat a compressed frame.
#[derive(Clone, Copy, Debug)]
pub(super) struct FrameKind {
    /// Decoding can start from it.
    pub(super) sync: bool,
    /// The first frame after a seek: the decoder flushes the frames it holds, as it does for
    /// AVFoundation's first sample from a given time.
    pub(super) resets_decoder: bool,
}

/// `kCMBlockBufferAssureMemoryNowFlag`: allocates the block's memory when it is created.
const BLOCK_BUFFER_ASSURE_MEMORY_NOW: u32 = 1;

#[allow(non_upper_case_globals)]
#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    static kCMSampleBufferAttachmentKey_ResetDecoderBeforeDecoding: CFTypeRef;
    static kCMSampleAttachmentKey_NotSync: CFTypeRef;
    fn CMSetAttachment(target: CFTypeRef, key: CFTypeRef, value: CFTypeRef, mode: u32);
    fn CMSampleBufferGetSampleAttachmentsArray(sample: CFTypeRef, create: u8) -> CFTypeRef;
    fn CMVideoFormatDescriptionCreate(
        allocator: CFTypeRef,
        codec: u32,
        width: i32,
        height: i32,
        extensions: CFTypeRef,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMBlockBufferCreateWithMemoryBlock(
        structure_allocator: CFTypeRef,
        memory: *mut c_void,
        length: usize,
        block_allocator: CFTypeRef,
        custom_source: *const c_void,
        offset: usize,
        data_length: usize,
        flags: u32,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMBlockBufferReplaceDataBytes(
        source: *const c_void,
        destination: CFTypeRef,
        offset: usize,
        length: usize,
    ) -> OSStatus;
    fn CMSampleBufferCreateReady(
        allocator: CFTypeRef,
        data: CFTypeRef,
        format: CFTypeRef,
        samples: isize,
        timing_entries: isize,
        timing: *const SampleTiming,
        size_entries: isize,
        sizes: *const usize,
        out: *mut CFTypeRef,
    ) -> OSStatus;
}

/// A video format description for `codec` and size, with `extensions` (a dictionary).
pub(super) fn video_format(
    codec: u32,
    width: u32,
    height: u32,
    extensions: &Cf,
) -> MediaResult<Cf> {
    let mut format = ptr::null();
    // SAFETY: the extensions are a live dictionary; the description is returned +1.
    let status = unsafe {
        CMVideoFormatDescriptionCreate(
            ptr::null(),
            codec,
            width as i32,
            height as i32,
            extensions.0,
            &mut format,
        )
    };
    check_status(status, "describe the video stream")?;
    Ok(Cf(format))
}

impl Sample {
    /// One compressed frame of `format`, copied into a block buffer Core Media owns. `decode`
    /// may be invalid (`None`), as for a stream decoded in presentation order.
    pub(super) fn compressed(
        format: &Cf,
        data: &[u8],
        kind: FrameKind,
        presentation: CMTime,
        decode: Option<CMTime>,
        duration: Option<CMTime>,
    ) -> MediaResult<Self> {
        const INVALID: CMTime = CMTime {
            value: 0,
            timescale: 0,
            flags: 0,
            epoch: 0,
        };
        let mut block = ptr::null();
        // SAFETY: Core Media allocates `data.len()` bytes itself (null memory, default allocator)
        // and copies the frame in; every object made is released by `Cf`.
        unsafe {
            let status = CMBlockBufferCreateWithMemoryBlock(
                ptr::null(),
                ptr::null_mut(),
                data.len(),
                ptr::null(),
                ptr::null(),
                0,
                data.len(),
                BLOCK_BUFFER_ASSURE_MEMORY_NOW,
                &mut block,
            );
            check_status(status, "make a block buffer")?;
            let block = Cf(block);
            check_status(
                CMBlockBufferReplaceDataBytes(data.as_ptr().cast(), block.0, 0, data.len()),
                "fill a block buffer",
            )?;
            let timing = SampleTiming {
                duration: duration.unwrap_or(INVALID),
                presentation,
                decode: decode.unwrap_or(INVALID),
            };
            let size = data.len();
            let mut sample = ptr::null();
            let status = CMSampleBufferCreateReady(
                ptr::null(),
                block.0,
                format.0,
                1,
                1,
                &timing,
                1,
                &size,
                &mut sample,
            );
            check_status(status, "make a sample buffer")?;
            let sample = Self(Cf(sample));
            if !kind.sync {
                let attachments = CMSampleBufferGetSampleAttachmentsArray(sample.as_ptr(), 1);
                if !attachments.is_null() && CFArrayGetCount(attachments) > 0 {
                    CFDictionarySetValue(
                        CFArrayGetValueAtIndex(attachments, 0),
                        kCMSampleAttachmentKey_NotSync,
                        kCFBooleanTrue,
                    );
                }
            }
            if kind.resets_decoder {
                CMSetAttachment(
                    sample.as_ptr(),
                    kCMSampleBufferAttachmentKey_ResetDecoderBeforeDecoding,
                    kCFBooleanTrue,
                    ATTACHMENT_DOES_NOT_PROPAGATE,
                );
            }
            Ok(sample)
        }
    }

    pub(super) fn as_ptr(&self) -> CFTypeRef {
        self.0 .0
    }

    pub(super) fn presentation(&self) -> CMTime {
        // SAFETY: the buffer is live.
        unsafe { CMSampleBufferGetPresentationTimeStamp(self.as_ptr()) }
    }

    pub(super) fn decode(&self) -> CMTime {
        // SAFETY: the buffer is live.
        unsafe { CMSampleBufferGetDecodeTimeStamp(self.as_ptr()) }
    }

    /// The number of samples: 0 for a marker (the reader's note of an edit boundary).
    pub(super) fn count(&self) -> usize {
        // SAFETY: the buffer is live.
        unsafe { CMSampleBufferGetNumSamples(self.as_ptr()).max(0) as usize }
    }

    /// The data as 32-bit floats, for decoded sound.
    pub(super) fn floats(&self) -> MediaResult<Vec<f32>> {
        // SAFETY: the buffer is live; the block buffer is owned by it, and the copy writes at
        // most the vector's length in bytes.
        unsafe {
            let block = CMSampleBufferGetDataBuffer(self.as_ptr());
            if block.is_null() {
                return Ok(Vec::new());
            }
            let length = CMBlockBufferGetDataLength(block);
            let mut samples = vec![0.0_f32; length / 4];
            let status = CMBlockBufferCopyDataBytes(
                block,
                0,
                samples.len() * 4,
                samples.as_mut_ptr().cast(),
            );
            if status != 0 {
                return Err(MediaError(format!("copy decoded sound: status {status}")));
            }
            Ok(samples)
        }
    }
}

pub(super) const fn four_character_code(code: &[u8; 4]) -> u32 {
    ((code[0] as u32) << 24) | ((code[1] as u32) << 16) | ((code[2] as u32) << 8) | code[3] as u32
}

pub(super) fn check_status(status: OSStatus, action: &str) -> MediaResult<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(MediaError(format!("{action}: status {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_media_time_is_placed_through_the_edit_that_holds_it() {
        // An MP4 whose B-frames start its media 2 frames in, shown from zero.
        let edits = Edits(vec![Edit {
            media: 0.0667,
            target: 0.0,
            length: 4.0,
        }]);
        assert!((edits.asset_seconds(0.0667) - 0.0).abs() < 1e-12);
        assert!((edits.asset_seconds(1.1667) - 1.1).abs() < 1e-12);
        // Before the edit (a leading frame of an open GOP): maps through the nearest edit.
        assert!((edits.asset_seconds(0.0333) - -0.0334).abs() < 1e-12);
        // A track that starts late, shown from half a second, and one with no edits.
        let late = Edits(vec![Edit {
            media: 0.0,
            target: 0.5,
            length: 2.0,
        }]);
        assert!((late.asset_seconds(1.0) - 1.5).abs() < 1e-12);
        assert_eq!(Edits::default().asset_seconds(3.25), 3.25);
    }

    #[test]
    fn times_turn_into_ticks_of_another_scale() {
        assert_eq!(CMTime::new(1001, 30_000).ticks(30_000), 1001);
        assert_eq!(CMTime::new(1, 30).ticks(600), 20);
        assert_eq!(CMTime::new(-1, 30).ticks(600), -20);
        assert_eq!(CMTime::from_seconds(1.55).seconds(), Some(1.55));
        assert_eq!(
            CMTime {
                value: 1,
                timescale: 0,
                flags: 0,
                epoch: 0
            }
            .seconds(),
            None
        );
        assert_eq!(CMTime::POSITIVE_INFINITY.seconds(), None);
    }
}
