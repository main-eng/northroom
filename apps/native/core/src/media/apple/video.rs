//! A video stream decoded by VideoToolbox directly, as Chromium's decoder does (ADR-0004).
//! AVFoundation reads the compressed samples as stored, a `VTDecompressionSession` decodes them
//! asynchronously with several frames in flight (on the media engine when the Mac has one for
//! the codec), and frames are put back in presentation order. A decoded frame stays a
//! `CVPixelBuffer` on its IOSurface for the GPU. Matroska and WebM, which AVFoundation doesn't
//! read, go through the core's demuxer to the same decoder (`matroska`).

use super::av::{
    check_status, four_character_code, kCFBooleanTrue,
    kCMFormatDescriptionExtension_BitsPerComponent, kCVPixelBufferHeightKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, number_of, Asset, CFRelease,
    CFTypeRef, CMFormatDescriptionGetExtension, CMFormatDescriptionGetMediaSubType, CMTime,
    CMVideoFormatDescriptionGetDimensions, Cf, Edits, OSStatus, Reader, Sample, Track, TrackKind,
    Transform,
};
use super::format::{bit_depth, codec_name, ColorTags};
use super::matroska::{self, MatroskaVideo};
use crate::media::{frame_rate_of, DecodedFrame, MediaError, MediaResult, VideoInfo, VideoSource};
use crate::Relock;
use northroom_media::pixel_buffer::{CVPixelBufferRef, PixelBuffer};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex, Once, PoisonError};

/// Frames submitted to the decoder and not yet returned.
const MAX_FRAMES_IN_DECODER: usize = 8;
/// Frames submitted and not yet handed to the caller; bounds decoded-frame memory.
const MAX_FRAMES_HELD: usize = 24;

type OutputCallback = unsafe extern "C" fn(
    decoder_context: *mut c_void,
    frame_context: *mut c_void,
    status: OSStatus,
    info_flags: u32,
    image: CVPixelBufferRef,
    presentation: CMTime,
    duration: CMTime,
);

#[repr(C)]
struct OutputCallbackRecord {
    callback: OutputCallback,
    context: *mut c_void,
}

/// `kVTDecodeFrame_EnableAsynchronousDecompression`.
const DECODE_ASYNCHRONOUSLY: u32 = 1;
/// `kCVPixelFormatType_32BGRA` and `kCVPixelFormatType_64RGBAHalf`: the formats AVFoundation's
/// image generator uses for 8-bit and deeper streams.
const PICTURE_8_BIT: &[u8; 4] = b"BGRA";
const PICTURE_HALF_FLOAT: &[u8; 4] = b"RGhA";

/// What the decoder outputs for each frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Output {
    /// The stream's own planes at its own size, for the GPU (`VideoInfo::layout`).
    Planes,
    /// A picture no larger than `max_pixels` on either side, made as AVFoundation's image
    /// generator makes one: 8-bit BGRA, or half-float RGBA when the stream declares more bits.
    Picture { max_pixels: u32, half_float: bool },
}

/// Whether the format description declares samples deeper than 8 bits. The image generator goes
/// by this, so a 10-bit H.264 stream that doesn't declare it is pictured in 8 bits.
///
/// # Safety
/// `format` must be a live format description.
unsafe fn says_deeper_than_8_bits(format: CFTypeRef) -> bool {
    // SAFETY: as the caller guarantees; the extension, when present, is a number.
    unsafe {
        number_of(CMFormatDescriptionGetExtension(
            format,
            kCMFormatDescriptionExtension_BitsPerComponent,
        ))
        .is_some_and(|bits| bits > 8.0)
    }
}

#[allow(non_upper_case_globals)]
#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    static kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder: CFTypeRef;
    fn VTRegisterSupplementalVideoDecoderIfAvailable(codec: u32);
    fn VTDecompressionSessionCreate(
        allocator: CFTypeRef,
        format: CFTypeRef,
        decoder_specification: CFTypeRef,
        image_buffer_attributes: CFTypeRef,
        output_callback: *const OutputCallbackRecord,
        session_out: *mut CFTypeRef,
    ) -> OSStatus;
    fn VTDecompressionSessionDecodeFrame(
        session: CFTypeRef,
        sample: CFTypeRef,
        flags: u32,
        frame_context: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    fn VTDecompressionSessionWaitForAsynchronousFrames(session: CFTypeRef) -> OSStatus;
    fn VTDecompressionSessionFinishDelayedFrames(session: CFTypeRef) -> OSStatus;
    fn VTDecompressionSessionInvalidate(session: CFTypeRef);
}

/// VP9 uses a decoder the system loads only on request.
fn register_supplemental_decoders() {
    static ONCE: Once = Once::new();
    // SAFETY: registering a codec the system knows has no preconditions.
    ONCE.call_once(|| unsafe {
        VTRegisterSupplementalVideoDecoderIfAvailable(four_character_code(b"vp09"))
    });
}

/// A frame given to the decoder; `buffer` arrives from the decoder's callback.
struct Slot {
    buffer: Option<PixelBuffer>,
}

#[derive(Default)]
struct Decoded {
    /// Submitted frames not yet handed out, keyed by presentation time in the track's ticks.
    slots: BTreeMap<i64, Slot>,
    in_decoder: usize,
    failure: Option<OSStatus>,
}

struct Shared {
    decoded: Mutex<Decoded>,
    changed: Condvar,
}

/// Runs on a VideoToolbox thread for every submitted frame.
unsafe extern "C" fn receive_decoded_frame(
    decoder_context: *mut c_void,
    frame_context: *mut c_void,
    status: OSStatus,
    _info_flags: u32,
    image: CVPixelBufferRef,
    _presentation: CMTime,
    _duration: CMTime,
) {
    // SAFETY: `decoder_context` is the `Shared` the reader keeps alive until it has invalidated
    // the session and waited for its callbacks.
    let shared = unsafe { &*(decoder_context as *const Shared) };
    let timestamp = frame_context as isize as i64;
    // SAFETY: VideoToolbox passes null or a live pixel buffer for the duration of the callback.
    let buffer = unsafe { PixelBuffer::retain(image) };
    let mut decoded = shared.decoded.relock();
    decoded.in_decoder = decoded.in_decoder.saturating_sub(1);
    match buffer {
        Some(buffer) if status == 0 => {
            if let Some(slot) = decoded.slots.get_mut(&timestamp) {
                slot.buffer = Some(buffer);
            }
        }
        _ => {
            decoded.slots.remove(&timestamp);
            decoded.failure = Some(status);
        }
    }
    drop(decoded);
    shared.changed.notify_all();
}

/// Where a stream's compressed samples come from.
enum Samples {
    /// AVFoundation's reader for a file it supports, as stored, from the last seek.
    Asset {
        asset: Asset,
        track: Track,
        reader: Reader,
    },
    Matroska(Box<MatroskaVideo>),
}

impl Samples {
    /// The next sample with a picture, skipping the reader's markers.
    fn next(&mut self) -> MediaResult<Option<Sample>> {
        match self {
            Self::Asset { reader, .. } => loop {
                match reader.next()? {
                    Some(sample) if sample.count() == 0 => continue,
                    other => return Ok(other),
                }
            },
            Self::Matroska(matroska) => matroska.next(),
        }
    }
}

/// One video stream decoded by VideoToolbox with several frames in flight.
pub struct VideoReader {
    samples: Samples,
    /// How the stream's times map onto the file's clock (none for Matroska).
    edits: Edits,
    info: VideoInfo,
    session: Session,
    shared: Arc<Shared>,
    /// The timescale every presentation time is keyed in.
    timescale: i32,
    end_of_stream: bool,
    last_timestamp: Option<i64>,
    /// Presentation time of the key frame decoding started from, once its sample is read.
    entry_timestamp: Option<i64>,
    /// The decode time of the last submitted sample; no sample still to come is shown before it.
    decoded_up_to: Option<i64>,
}

// SAFETY: the session is used only through `&mut self` or in `Drop`, the callback state is
// behind a mutex, and the reader is moved between threads only as a whole.
unsafe impl Send for VideoReader {}

/// A `VTDecompressionSession`, invalidated when dropped.
struct Session(CFTypeRef);

impl Session {
    /// A hardware decoder where the Mac has one for the codec, delivering frames to `shared`.
    fn create(
        format: &Cf,
        decoder_specification: &Cf,
        image_attributes: &Cf,
        shared: &Arc<Shared>,
        codec: &str,
    ) -> MediaResult<Self> {
        let callback = OutputCallbackRecord {
            callback: receive_decoded_frame,
            context: Arc::as_ptr(shared) as *mut c_void,
        };
        let mut session = ptr::null();
        // SAFETY: every argument is a live object or valid pointer; the callback context is kept
        // alive by the reader, which drops its session first (it is declared before `shared`).
        let status = unsafe {
            VTDecompressionSessionCreate(
                ptr::null(),
                format.0,
                decoder_specification.0,
                image_attributes.0,
                &callback,
                &mut session,
            )
        };
        check_status(status, &format!("create a {codec} decoder"))?;
        Ok(Self(session))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the session is live and owned here. Waiting, then invalidating, guarantees no
        // callback runs after this returns.
        unsafe {
            VTDecompressionSessionWaitForAsynchronousFrames(self.0);
            VTDecompressionSessionInvalidate(self.0);
            CFRelease(self.0);
        }
    }
}

impl VideoReader {
    /// A reader whose frames are pictures no larger than `max_pixels` on either side, for a
    /// file's still (`apple::still`).
    pub(super) fn open_picture(path: &str, max_pixels: u32) -> MediaResult<Self> {
        Self::open_as(path, |half_float| Output::Picture {
            max_pixels,
            half_float,
        })
    }

    /// How the stream's pictures are turned for display; Matroska's are shown as stored.
    pub(super) fn transform(&self) -> Transform {
        match &self.samples {
            Samples::Asset { track, .. } => track.preferred_transform(),
            Samples::Matroska(_) => Transform::IDENTITY,
        }
    }

    fn passthrough(asset: &Asset, track: &Track, start: f64) -> MediaResult<Reader> {
        Reader::start_at(
            asset,
            track,
            None,
            (start > 0.0).then(|| CMTime::from_seconds(start)),
        )
    }

    /// Seeks through a Matroska file's clusters even when it has cues.
    #[cfg(test)]
    pub(super) fn ignore_cues(&mut self) {
        if let Samples::Matroska(matroska) = &mut self.samples {
            matroska.ignore_cues();
        }
    }

    /// Reads one sample and submits it to the decoder, or finishes the stream at the end.
    fn submit_next_sample(&mut self) -> MediaResult<()> {
        let Some(sample) = self.next_sample()? else {
            self.end_of_stream = true;
            // SAFETY: the session is live.
            return check_status(
                unsafe { VTDecompressionSessionFinishDelayedFrames(self.session.0) },
                "finish decoding",
            );
        };
        let presentation = sample.presentation();
        if !presentation.is_numeric() {
            return Ok(());
        }
        let timestamp = presentation.ticks(self.timescale);
        // Frames shown before the entry key frame but stored after it depend on the previous
        // GOP, which wasn't decoded; nothing later depends on them.
        let entry_timestamp = *self.entry_timestamp.get_or_insert(timestamp);
        let decode = sample.decode();
        let decode = if decode.is_numeric() {
            decode.ticks(self.timescale)
        } else {
            timestamp
        };
        self.decoded_up_to = Some(self.decoded_up_to.map_or(decode, |up_to| up_to.max(decode)));
        if timestamp < entry_timestamp {
            return Ok(());
        }
        {
            // The slot exists before the decoder can call back for it.
            let mut decoded = self.shared.decoded.relock();
            decoded.slots.insert(timestamp, Slot { buffer: None });
            decoded.in_decoder += 1;
        }
        // SAFETY: session and sample are live; the frame context carries the timestamp by value.
        let status = unsafe {
            VTDecompressionSessionDecodeFrame(
                self.session.0,
                sample.as_ptr(),
                DECODE_ASYNCHRONOUSLY,
                timestamp as isize as *mut c_void,
                ptr::null_mut(),
            )
        };
        if status != 0 {
            let mut decoded = self.shared.decoded.relock();
            if decoded.slots.remove(&timestamp).is_some() {
                decoded.in_decoder = decoded.in_decoder.saturating_sub(1);
            }
        }
        check_status(status, "decode frame")
    }

    fn next_sample(&mut self) -> MediaResult<Option<Sample>> {
        self.samples.next()
    }

    fn seconds_at(&self, timestamp: i64) -> f64 {
        self.edits
            .asset_seconds(timestamp as f64 / f64::from(self.timescale))
    }

    /// A decoder of `format` for the stream in `samples`, reading its first frames when the
    /// container omits its colour.
    fn start(
        samples: Samples,
        edits: Edits,
        timescale: i32,
        format: &Cf,
        info: VideoInfo,
        container_tags: &ColorTags,
        output: Output,
    ) -> MediaResult<Self> {
        if info.frame_rate.0 <= 0 {
            return Err(MediaError("video stream has no frame rate".into()));
        }
        let surface_properties = Cf::dictionary(&[])?;
        // SAFETY: the framework constants are live for the life of the process.
        let (decoder_specification, image_attributes) = unsafe {
            let image_attributes = match output {
                Output::Planes => {
                    // Left alone, the decoder emits a compressed surface format Metal can't
                    // sample per plane, so ask for plain two-plane Y'CbCr at the stream's own bit
                    // depth and range. IOSurface-backed, Metal-compatible output lets the GPU
                    // share the frame.
                    let surface_format = Cf::integer(i64::from(info.layout().format_type()))?;
                    Cf::dictionary(&[
                        (kCVPixelBufferPixelFormatTypeKey, surface_format.0),
                        (kCVPixelBufferIOSurfacePropertiesKey, surface_properties.0),
                        (kCVPixelBufferMetalCompatibilityKey, kCFBooleanTrue),
                    ])?
                }
                Output::Picture {
                    max_pixels,
                    half_float,
                } => {
                    // The decoder scales and converts on output, as the image generator has it
                    // do, so the same request gives the same pixels.
                    let (width, height) =
                        crate::media::picture_size(info.width, info.height, max_pixels);
                    let surface_format =
                        Cf::integer(i64::from(four_character_code(if half_float {
                            PICTURE_HALF_FLOAT
                        } else {
                            PICTURE_8_BIT
                        })))?;
                    let (width, height) = (
                        Cf::integer(i64::from(width))?,
                        Cf::integer(i64::from(height))?,
                    );
                    Cf::dictionary(&[
                        (kCVPixelBufferPixelFormatTypeKey, surface_format.0),
                        (kCVPixelBufferWidthKey, width.0),
                        (kCVPixelBufferHeightKey, height.0),
                        (kCVPixelBufferIOSurfacePropertiesKey, surface_properties.0),
                    ])?
                }
            };
            (
                Cf::dictionary(&[(
                    kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder,
                    kCFBooleanTrue,
                )])?,
                image_attributes,
            )
        };
        let shared = Arc::new(Shared {
            decoded: Mutex::new(Decoded::default()),
            changed: Condvar::new(),
        });
        let session = Session::create(
            format,
            &decoder_specification,
            &image_attributes,
            &shared,
            &info.codec_name,
        )?;
        let mut video = Self {
            samples,
            edits,
            timescale,
            info,
            session,
            shared,
            end_of_stream: false,
            last_timestamp: None,
            entry_timestamp: None,
            decoded_up_to: None,
        };
        // A picture is drawn in its buffer's tagged colour, so only planes need it settled.
        if output == Output::Planes && !container_tags.is_complete() {
            // The decoder tags pictures with the stream's colour where the container is silent.
            let frame = video
                .next_frame()?
                .ok_or_else(|| MediaError("the video stream has no frames".into()))?;
            // SAFETY: the frame's pixel buffer is live.
            let stream_tags = unsafe { ColorTags::of_image(frame.buffer.as_ptr() as CFTypeRef) };
            video.info.color = container_tags
                .clone()
                .or(stream_tags)
                .describe(video.info.height)?;
            drop(frame);
            video.seek(0.0)?;
        }
        Ok(video)
    }

    /// Opens the file's first video stream, with frames made per `output` depending on whether
    /// the stream declares more than 8 bits.
    fn open_as(path: &str, output: impl Fn(bool) -> Output) -> MediaResult<Self> {
        register_supplemental_decoders();
        if crate::media::matroska::is_matroska(path) {
            let opened = MatroskaVideo::open(path)?;
            // SAFETY: `format` is a live video format description.
            let (depth, container_tags, deeper) = unsafe {
                (
                    bit_depth(opened.format.0, opened.codec),
                    ColorTags::of_format(opened.format.0),
                    says_deeper_than_8_bits(opened.format.0),
                )
            };
            let info = VideoInfo {
                width: opened.width,
                height: opened.height,
                frame_rate: frame_rate_of(opened.frames_per_second as f32),
                duration_seconds: opened.duration_seconds,
                codec_name: opened.codec_name.to_owned(),
                bit_depth: depth,
                color: container_tags.describe(opened.height)?,
            };
            let samples = Samples::Matroska(Box::new(opened.samples));
            return Self::start(
                samples,
                Edits::default(),
                matroska::TIMESCALE,
                &opened.format,
                info,
                &container_tags,
                output(deeper),
            );
        }
        let asset = Asset::open(path)?;
        let track = asset
            .first_track(TrackKind::Video)
            .ok_or_else(|| MediaError("the file has no video stream".into()))?;
        let format = track
            .format_description()
            .ok_or_else(|| MediaError("the video stream has no format".into()))?;
        // SAFETY: `format` is a live video format description.
        let (codec, dimensions, depth, container_tags, deeper) = unsafe {
            let codec = CMFormatDescriptionGetMediaSubType(format.0);
            (
                codec,
                CMVideoFormatDescriptionGetDimensions(format.0),
                bit_depth(format.0, codec),
                ColorTags::of_format(format.0),
                says_deeper_than_8_bits(format.0),
            )
        };
        let info = VideoInfo {
            width: dimensions.width.max(0) as u32,
            height: dimensions.height.max(0) as u32,
            frame_rate: frame_rate_of(track.nominal_frame_rate()),
            duration_seconds: asset.duration_seconds().unwrap_or(0.0),
            codec_name: codec_name(codec),
            bit_depth: depth,
            // Settled below, once the decoder reports what the stream itself says.
            color: container_tags.describe(dimensions.height.max(0) as u32)?,
        };
        let reader = Self::passthrough(&asset, &track, 0.0)?;
        let (edits, timescale) = (track.edits(), track.media_timescale());
        Self::start(
            Samples::Asset {
                asset,
                track,
                reader,
            },
            edits,
            timescale,
            &format,
            info,
            &container_tags,
            output(deeper),
        )
    }
}

/// The result of one step of reading frames.
enum DecodeTurn {
    /// A frame, or the end of the stream.
    Done(Option<DecodedFrame>),
    /// Nothing yet: a sample went to the decoder, or the decoder was waited on.
    Again,
}

impl VideoReader {
    /// One step of reading frames. Returns the earliest frame once it may leave, or reports the
    /// end of the stream; otherwise submits the next sample, or waits on the decoder while its
    /// queue is full.
    fn decode_turn(&mut self) -> MediaResult<DecodeTurn> {
        let mut decoded = self.shared.decoded.relock();
        if let Some(status) = decoded.failure.take() {
            return Err(MediaError(format!(
                "hardware decode failed: VideoToolbox status {status}"
            )));
        }
        let can_submit = !self.end_of_stream
            && decoded.in_decoder < MAX_FRAMES_IN_DECODER
            && decoded.slots.len() < MAX_FRAMES_HELD;
        // The earliest frame may leave once no sample still to come can be shown before it:
        // every later sample is decoded after the last one submitted and shown no earlier than
        // it is decoded. A full queue also lets it leave.
        let earliest_ready = decoded
            .slots
            .first_key_value()
            .is_some_and(|(timestamp, slot)| {
                slot.buffer.is_some()
                    && (self.end_of_stream
                        || !can_submit
                        || self.decoded_up_to.is_some_and(|up_to| up_to >= *timestamp))
            });
        if earliest_ready {
            let (timestamp, slot) = decoded
                .slots
                .pop_first()
                .ok_or_else(|| MediaError("decoded frame queue emptied".into()))?;
            drop(decoded);
            let frame = self.hand_out(timestamp, slot)?;
            return Ok(DecodeTurn::Done(Some(frame)));
        }
        if self.end_of_stream && decoded.slots.is_empty() {
            return Ok(DecodeTurn::Done(None));
        }
        if can_submit {
            drop(decoded);
            self.submit_next_sample()?;
            return Ok(DecodeTurn::Again);
        }
        if decoded.in_decoder == 0 {
            return Err(MediaError("decoder holds frames it cannot release".into()));
        }
        drop(
            self.shared
                .changed
                .wait(decoded)
                .unwrap_or_else(PoisonError::into_inner),
        );
        Ok(DecodeTurn::Again)
    }

    /// Hands out the earliest frame, shown at `timestamp` (in the track's ticks).
    fn hand_out(&mut self, timestamp: i64, slot: Slot) -> MediaResult<DecodedFrame> {
        if self.last_timestamp.is_some_and(|last| last >= timestamp) {
            return Err(MediaError(
                "frames left the decoder out of presentation order".into(),
            ));
        }
        self.last_timestamp = Some(timestamp);
        let buffer = slot
            .buffer
            .ok_or_else(|| MediaError("ready frame has no pixel buffer".into()))?;
        Ok(DecodedFrame {
            buffer,
            seconds: self.seconds_at(timestamp),
        })
    }
}

impl VideoSource for VideoReader {
    fn open(path: &str) -> MediaResult<Self> {
        Self::open_as(path, |_| Output::Planes)
    }

    fn info(&self) -> &VideoInfo {
        &self.info
    }

    fn next_frame(&mut self) -> MediaResult<Option<DecodedFrame>> {
        loop {
            if let DecodeTurn::Done(frame) = self.decode_turn()? {
                return Ok(frame);
            }
        }
    }

    fn seek(&mut self, seconds: f64) -> MediaResult<()> {
        // The reader starts at the key frame at or before `seconds`. Past the end, it starts at
        // the one before the last frame, so the last frame is still reachable.
        let last_frame =
            (self.info.duration_seconds - 1.0 / self.info.frames_per_second()).max(0.0);
        let wanted = seconds.clamp(0.0, last_frame);
        match &mut self.samples {
            Samples::Matroska(matroska) => matroska.seek(wanted)?,
            Samples::Asset {
                asset,
                track,
                reader,
            } => {
                *reader = Self::passthrough(asset, track, wanted)?;
                if wanted > 0.0 {
                    // Reading from a time, AVFoundation starts at the key frame before it but
                    // drops frames shown before the time that it judges unneeded. For AV1 it
                    // judges wrongly (later frames fail to decode, status -12909). Reading again
                    // from that key frame's own time keeps every frame.
                    if let Some(key) = self.samples.next()? {
                        let ticks = (self.edits.asset_seconds(
                            key.presentation().ticks(self.timescale) as f64
                                / f64::from(self.timescale),
                        ) * f64::from(self.timescale))
                        .round() as i64;
                        let start = (ticks > 0).then(|| CMTime::new(ticks, self.timescale));
                        if let Samples::Asset {
                            asset,
                            track,
                            reader,
                        } = &mut self.samples
                        {
                            *reader = Reader::start_at(asset, track, None, start)?;
                        }
                    }
                }
            }
        }
        // SAFETY: the session is live. After this returns no callback is pending.
        let status = unsafe { VTDecompressionSessionWaitForAsynchronousFrames(self.session.0) };
        check_status(status, "drain decoder")?;
        let mut decoded = self.shared.decoded.relock();
        decoded.slots.clear();
        decoded.in_decoder = 0;
        decoded.failure = None;
        self.end_of_stream = false;
        self.last_timestamp = None;
        self.entry_timestamp = None;
        self.decoded_up_to = None;
        Ok(())
    }
}
