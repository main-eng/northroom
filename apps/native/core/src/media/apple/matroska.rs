//! Matroska and WebM on a Mac, which AVFoundation doesn't read. The core's demuxer
//! (`media::matroska`) supplies a track's frames, and this module turns them into what the
//! Mac's decoders take: Core Media sample buffers with a format description built from the
//! track's settings, as AVFoundation builds one from an MP4 sample description, so VideoToolbox
//! decodes them as it does MP4.

use super::av::{
    four_character_code, kCFBooleanFalse, kCFBooleanTrue,
    kCMFormatDescriptionExtension_BitsPerComponent, kCMFormatDescriptionExtension_FullRangeVideo,
    kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms,
    kCVImageBufferChromaLocationTopFieldKey, kCVImageBufferColorPrimariesKey,
    kCVImageBufferTransferFunctionKey, kCVImageBufferYCbCrMatrixKey, video_format, CFTypeRef,
    CMFormatDescriptionGetExtension, CMTime, Cf, FrameKind, OSStatus, Sample,
};
use crate::media::matroska::codec::{
    restore_frame, video_configuration, VideoCodec, VideoConfiguration,
};
use crate::media::matroska::{Colour, Demuxer, Packet, TrackKind};
use crate::media::{MediaError, MediaResult, Probe};
use std::collections::VecDeque;

/// Times are in nanoseconds, Matroska's own unit.
pub(super) const TIMESCALE: i32 = 1_000_000_000;
/// Frames read ahead to find when each may be decoded: at least the deepest reordering H.264
/// and HEVC allow (16 frames).
const LOOKAHEAD: usize = 32;
/// Frames whose spacing gives the rate of a track that doesn't state one.
const FRAMES_FOR_RATE: usize = 60;

#[allow(non_upper_case_globals)]
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    static kCVImageBufferChromaLocationBottomFieldKey: CFTypeRef;
    fn CVColorPrimariesGetStringForIntegerCodePoint(code: i32) -> CFTypeRef;
    fn CVTransferFunctionGetStringForIntegerCodePoint(code: i32) -> CFTypeRef;
    fn CVYCbCrMatrixGetStringForIntegerCodePoint(code: i32) -> CFTypeRef;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(
        allocator: CFTypeRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_unit_header_length: i32,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        allocator: CFTypeRef,
        count: usize,
        pointers: *const *const u8,
        sizes: *const usize,
        nal_unit_header_length: i32,
        extensions: CFTypeRef,
        out: *mut CFTypeRef,
    ) -> OSStatus;
}

/// The properties of a Matroska video track the reader needs.
pub(super) struct Opened {
    pub(super) samples: MatroskaVideo,
    pub(super) format: Cf,
    pub(super) codec: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) frames_per_second: f64,
    pub(super) duration_seconds: f64,
    pub(super) codec_name: &'static str,
}

/// A Matroska video track's frames as Core Media sample buffers, in file order.
pub(super) struct MatroskaVideo {
    demuxer: Demuxer,
    format: Cf,
    codec: VideoCodec,
    lookahead: VecDeque<Packet>,
    last_decode: Option<i64>,
    /// The next frame is the first since a seek.
    starting: bool,
}

// SAFETY: the format description is immutable and Core Foundation's reference counting is
// thread-safe; the demuxer is plain data.
unsafe impl Send for MatroskaVideo {}

impl MatroskaVideo {
    pub(super) fn open(path: &str) -> MediaResult<Opened> {
        let mut demuxer = Demuxer::open(path, TrackKind::Video)?
            .ok_or_else(|| MediaError("the file has no video stream".into()))?;
        let track = demuxer.track().clone();
        let settings = track.video.clone().unwrap_or_default();
        // Read the first frames: VP9 keeps its settings in a key frame's header, and a track
        // that doesn't state its rate reveals it in the frame spacing.
        let mut first = Vec::new();
        while first.len() < FRAMES_FOR_RATE {
            let Some(packet) = demuxer.next_packet()? else {
                break;
            };
            first.push(packet);
        }
        let keyframe = first
            .iter()
            .find(|packet| packet.keyframe)
            .map(|packet| packet.data.as_slice());
        let configuration = video_configuration(&track, keyframe)?;
        let vp9_full_range = (configuration.codec == VideoCodec::Vp9)
            .then(|| {
                keyframe
                    .and_then(crate::media::matroska::codec::vp9_keyframe_header)
                    .map(|header| header.full_range)
            })
            .flatten();
        let extensions = extensions(&configuration, &settings.colour, vp9_full_range)?;
        let codec = four_character_code(&match configuration.codec {
            VideoCodec::H264 => *b"avc1",
            VideoCodec::Hevc => *b"hvc1",
            VideoCodec::Vp9 => *b"vp09",
            VideoCodec::Av1 => *b"av01",
            VideoCodec::ProRes => configuration.prores_type.unwrap_or(*b"apcn"),
        });
        if settings.width == 0 || settings.height == 0 {
            return Err(MediaError("the video stream has no size".into()));
        }
        let format = video_format(codec, settings.width, settings.height, &extensions)?;
        let frames_per_second = match track.default_duration_ns {
            Some(duration) => 1e9 / duration as f64,
            None => rate_of_spacing(&first),
        };
        let duration_seconds = demuxer.segment().duration_seconds.unwrap_or(0.0);
        demuxer.seek(0, true)?;
        let samples = Self {
            demuxer,
            format: retain(&format),
            codec: configuration.codec,
            lookahead: VecDeque::new(),
            last_decode: None,
            starting: true,
        };
        Ok(Opened {
            samples,
            format,
            codec,
            width: settings.width,
            height: settings.height,
            frames_per_second,
            duration_seconds,
            codec_name: configuration.name,
        })
    }

    /// The next frame in file order, with the time from which it may be decoded: the earliest
    /// presentation time of any frame still to come, so no later frame is shown before it.
    pub(super) fn next(&mut self) -> MediaResult<Option<Sample>> {
        while self.lookahead.len() < LOOKAHEAD {
            match self.demuxer.next_packet()? {
                Some(packet) => self.lookahead.push_back(packet),
                None => break,
            }
        }
        let Some(packet) = self.lookahead.pop_front() else {
            return Ok(None);
        };
        let earliest = self
            .lookahead
            .iter()
            .map(|later| later.timestamp_ns)
            .fold(packet.timestamp_ns, i64::min);
        let decode = self.last_decode.map_or(earliest, |last| last.max(earliest));
        self.last_decode = Some(decode);
        let duration = packet
            .duration_ns
            .map(|duration| CMTime::new(duration, TIMESCALE));
        let kind = FrameKind {
            sync: packet.keyframe,
            resets_decoder: std::mem::take(&mut self.starting),
        };
        let data = restore_frame(self.codec, packet.data);
        let (presentation, decode) = (
            CMTime::new(packet.timestamp_ns, TIMESCALE),
            CMTime::new(decode, TIMESCALE),
        );
        Sample::compressed(
            &self.format,
            &data,
            kind,
            presentation,
            Some(decode),
            duration,
        )
        .map(Some)
    }

    /// Seeks to the key frame at or before `seconds`.
    pub(super) fn seek(&mut self, seconds: f64) -> MediaResult<()> {
        self.lookahead.clear();
        self.last_decode = None;
        self.starting = true;
        self.demuxer.seek((seconds * 1e9).round() as i64, true)
    }

    /// Seeks through the clusters even when the file has cues.
    #[cfg(test)]
    pub(super) fn ignore_cues(&mut self) {
        self.demuxer.ignore_cues();
    }
}

fn retain(format: &Cf) -> Cf {
    // SAFETY: `format` is live.
    unsafe { Cf::retained(format.0) }.unwrap_or_else(|| unreachable!("a live object is not null"))
}

/// The frame rate of the frames in `first`: the median of their spacings.
fn rate_of_spacing(first: &[Packet]) -> f64 {
    let mut times: Vec<i64> = first.iter().map(|packet| packet.timestamp_ns).collect();
    times.sort_unstable();
    let mut spacings: Vec<i64> = times
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|spacing| *spacing > 0)
        .collect();
    spacings.sort_unstable();
    spacings
        .get(spacings.len() / 2)
        .map_or(0.0, |spacing| 1e9 / *spacing as f64)
}

/// The format description's extensions: the codec configuration record as AVFoundation stores
/// an MP4's, and the track's colour in Core Video's strings.
fn extensions(
    configuration: &VideoConfiguration,
    colour: &Colour,
    vp9_full_range: Option<bool>,
) -> MediaResult<Cf> {
    let mut extensions = Extensions::default();
    if let Some((atom, record)) = &configuration.atom {
        extensions.add_atom(atom, record)?;
    }
    extensions.add_colour_names(colour);
    extensions.add_chroma_siting(colour)?;
    extensions.add_full_range(colour, vp9_full_range);
    // An H.264 or HEVC stream's parameter sets (their VUI) may fill in what `Colour` omits;
    // Core Media reads them as AVFoundation does for an MP4 without `colr`.
    if let Some(stream) = parameter_set_format(configuration) {
        extensions.add_unsaid_by(stream);
    }
    if let Some(bits) = configuration
        .bit_depth
        .or(colour.bits_per_channel.map(|bits| bits as u32))
        .filter(|_| configuration.codec != VideoCodec::H264)
    {
        extensions.add_bits_per_component(bits)?;
    }
    extensions.dictionary()
}

/// A format description's extensions being collected: each key and value, plus the objects
/// that keep the values alive until the dictionary retains them.
#[derive(Default)]
struct Extensions {
    entries: Vec<(CFTypeRef, CFTypeRef)>,
    held: Vec<Cf>,
}

impl Extensions {
    /// The codec configuration record, keyed by its atom name.
    fn add_atom(&mut self, atom: &str, record: &[u8]) -> MediaResult<()> {
        let (key, data) = (Cf::string(atom)?, Cf::data(record)?);
        let atoms = Cf::dictionary(&[(key.0, data.0)])?;
        // SAFETY: the key is a framework constant; the dictionary is held below.
        let atoms_key = unsafe { kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms };
        self.entries.push((atoms_key, atoms.0));
        self.held.extend([key, data, atoms]);
        Ok(())
    }

    /// The colour primaries, transfer function and matrix the track states.
    fn add_colour_names(&mut self, colour: &Colour) {
        // SAFETY: the keys are framework constants; the names are constant strings Core Video
        // returns.
        unsafe {
            self.entries.extend(
                colour_name(
                    colour.primaries,
                    CVColorPrimariesGetStringForIntegerCodePoint,
                )
                .map(|name| (kCVImageBufferColorPrimariesKey, name)),
            );
            self.entries.extend(
                colour_name(
                    colour.transfer,
                    CVTransferFunctionGetStringForIntegerCodePoint,
                )
                .map(|name| (kCVImageBufferTransferFunctionKey, name)),
            );
            self.entries.extend(
                colour_name(colour.matrix, CVYCbCrMatrixGetStringForIntegerCodePoint)
                    .map(|name| (kCVImageBufferYCbCrMatrixKey, name)),
            );
        }
    }

    /// Where the chroma samples sit, for both fields, if the track says.
    fn add_chroma_siting(&mut self, colour: &Colour) -> MediaResult<()> {
        // Siting: horizontally 1 is co-sited with the left sample, 2 half-way; vertically 1 is
        // co-sited with the top sample, 2 half-way.
        let siting = match (
            colour.chroma_siting_horizontal,
            colour.chroma_siting_vertical,
        ) {
            (Some(1), Some(2)) => "Left",
            (Some(2), Some(2)) => "Center",
            (Some(1), Some(1)) => "TopLeft",
            (Some(2), Some(1)) => "Top",
            _ => return Ok(()),
        };
        let text = Cf::string(siting)?;
        // SAFETY: the keys are framework constants; the string is held below.
        unsafe {
            self.entries
                .push((kCVImageBufferChromaLocationTopFieldKey, text.0));
            self.entries
                .push((kCVImageBufferChromaLocationBottomFieldKey, text.0));
        }
        self.held.push(text);
        Ok(())
    }

    /// Whether the picture is full range: as the track says (1 broadcast, 2 full), else as its
    /// first VP9 key frame says.
    fn add_full_range(&mut self, colour: &Colour, vp9_full_range: Option<bool>) {
        let full_range = match colour.range {
            Some(1) => Some(false),
            Some(2) => Some(true),
            _ => vp9_full_range,
        };
        let Some(full_range) = full_range else {
            return;
        };
        // SAFETY: the key and the booleans are framework constants.
        unsafe {
            let value = if full_range {
                kCFBooleanTrue
            } else {
                kCFBooleanFalse
            };
            self.entries
                .push((kCMFormatDescriptionExtension_FullRangeVideo, value));
        }
    }

    /// Colour from a format description built from the stream's parameter sets, for whatever
    /// the collected extensions still lack.
    fn add_unsaid_by(&mut self, stream: Cf) {
        // SAFETY: the keys are framework constants; the description is live, and held below
        // with the values it owns.
        unsafe {
            let keys = [
                kCVImageBufferColorPrimariesKey,
                kCVImageBufferTransferFunctionKey,
                kCVImageBufferYCbCrMatrixKey,
                kCVImageBufferChromaLocationTopFieldKey,
                kCVImageBufferChromaLocationBottomFieldKey,
                kCMFormatDescriptionExtension_FullRangeVideo,
            ];
            for key in keys {
                let value = CMFormatDescriptionGetExtension(stream.0, key);
                if !value.is_null() && !self.entries.iter().any(|(said, _)| *said == key) {
                    self.entries.push((key, value));
                }
            }
        }
        self.held.push(stream);
    }

    fn add_bits_per_component(&mut self, bits: u32) -> MediaResult<()> {
        let bits = Cf::integer(i64::from(bits))?;
        // SAFETY: the key is a framework constant; the number is held below.
        let key = unsafe { kCMFormatDescriptionExtension_BitsPerComponent };
        self.entries.push((key, bits.0));
        self.held.push(bits);
        Ok(())
    }

    /// The extensions as a dictionary, which retains every key and value before the objects
    /// held for them are released.
    fn dictionary(self) -> MediaResult<Cf> {
        Cf::dictionary(&self.entries)
    }
}

/// Core Video's name for an ISO/IEC 23091-4 code point the track states (2 is "unspecified"),
/// if it has one.
fn colour_name(
    code: Option<u64>,
    name: unsafe extern "C" fn(i32) -> CFTypeRef,
) -> Option<CFTypeRef> {
    code.filter(|code| *code != 2 && *code < 256)
        // SAFETY: Core Video's lookups take any code point and return a constant string or null.
        .map(|code| unsafe { name(code as i32) })
        .filter(|text| !text.is_null())
}

/// The parameter sets an `avcC` or `hvcC` record holds, in order, and the length of the size
/// prefix before each frame's NAL units. `None` if the record is shorter than it claims.
fn parameter_sets<'a>(atom: &str, record: &'a [u8]) -> Option<(Vec<&'a [u8]>, i32)> {
    let mut sets = Vec::new();
    let mut take = |record: &'a [u8], at: &mut usize| -> Option<()> {
        let length = usize::from(u16::from_be_bytes([
            *record.get(*at)?,
            *record.get(*at + 1)?,
        ]));
        sets.push(record.get(*at + 2..*at + 2 + length)?);
        *at += 2 + length;
        Some(())
    };
    match atom {
        // Version, profile, compatibility, level, size-prefix length minus one, then an SPS
        // count (low five bits) with each SPS sized, then a PPS count with each sized.
        "avcC" => {
            let nal_length = i32::from(record.get(4)? & 0x03) + 1;
            let mut at = 6;
            for _ in 0..(record.get(5)? & 0x1f) {
                take(record, &mut at)?;
            }
            let pps = *record.get(at)?;
            at += 1;
            for _ in 0..pps {
                take(record, &mut at)?;
            }
            Some((sets, nal_length))
        }
        // 22 bytes of settings (size-prefix length minus one in the last), then arrays of NAL
        // units by type: a type byte, a 16-bit count, each unit sized.
        "hvcC" => {
            let nal_length = i32::from(record.get(21)? & 0x03) + 1;
            let mut at = 23;
            for _ in 0..*record.get(22)? {
                let count = u16::from_be_bytes([*record.get(at + 1)?, *record.get(at + 2)?]);
                at += 3;
                for _ in 0..count {
                    take(record, &mut at)?;
                }
            }
            Some((sets, nal_length))
        }
        _ => None,
    }
}

/// The format description Core Media builds from an H.264 or HEVC stream's parameter sets
/// alone, with their VUI colour. `None` for other codecs or sets it can't read.
fn parameter_set_format(configuration: &VideoConfiguration) -> Option<Cf> {
    let (atom, record) = configuration.atom.as_ref()?;
    let (sets, nal_length) = parameter_sets(atom, record)?;
    if sets.is_empty() {
        return None;
    }
    let (pointers, sizes): (Vec<*const u8>, Vec<usize>) =
        sets.iter().map(|set| (set.as_ptr(), set.len())).unzip();
    let mut format: CFTypeRef = std::ptr::null();
    // SAFETY: the pointers and sizes describe `sets`, live for the call; the description is
    // returned +1.
    let status = unsafe {
        match configuration.codec {
            VideoCodec::H264 => CMVideoFormatDescriptionCreateFromH264ParameterSets(
                std::ptr::null(),
                sets.len(),
                pointers.as_ptr(),
                sizes.as_ptr(),
                nal_length,
                &mut format,
            ),
            VideoCodec::Hevc => CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                std::ptr::null(),
                sets.len(),
                pointers.as_ptr(),
                sizes.as_ptr(),
                nal_length,
                std::ptr::null(),
                &mut format,
            ),
            _ => return None,
        }
    };
    (status == 0 && !format.is_null()).then(|| Cf(format))
}

/// A Matroska file's details: the first video track's stored size and the file's duration,
/// or a sound's duration.
pub(super) fn probe(path: &str) -> Probe {
    if let Ok(Some(video)) = Demuxer::open(path, TrackKind::Video) {
        let size = video
            .track()
            .video
            .as_ref()
            .map(|video| (video.width, video.height))
            .filter(|(width, height)| *width > 0 && *height > 0);
        if size.is_some() {
            return Probe {
                dimensions: size,
                duration_seconds: video.segment().duration_seconds,
            };
        }
    }
    match Demuxer::open(path, TrackKind::Audio) {
        Ok(Some(audio)) => Probe {
            dimensions: None,
            duration_seconds: Some(super::converter::played_length(
                audio.segment().duration_seconds,
                audio.track().codec_delay_ns,
            ))
            .filter(|seconds| *seconds > 0.0),
        },
        _ => Probe::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::parameter_sets;

    #[test]
    fn configuration_records_give_their_parameter_sets() {
        // One 3-byte SPS and one 2-byte PPS, with 4-byte size prefixes.
        let avc = [
            1, 100, 0, 13, 0xff, 0xe1, 0, 3, 0x67, 1, 2, 1, 0, 2, 0x68, 3,
        ];
        let (sets, nal_length) = parameter_sets("avcC", &avc).expect("avcC");
        assert_eq!(
            (sets, nal_length),
            (vec![&[0x67, 1, 2][..], &[0x68, 3][..]], 4)
        );
        // 22 bytes of settings (4-byte size prefixes), then a VPS array of one and an SPS
        // array of two.
        let mut hevc = vec![1; 21];
        hevc.extend([
            0xff, 2, 0x20, 0, 1, 0, 2, 0x40, 1, 0x21, 0, 2, 0, 1, 0x42, 0, 1, 0x43,
        ]);
        let (sets, nal_length) = parameter_sets("hvcC", &hevc).expect("hvcC");
        assert_eq!(
            (sets, nal_length),
            (vec![&[0x40, 1][..], &[0x42][..], &[0x43][..]], 4)
        );
        // A record shorter than it claims, or of another codec, gives none.
        assert!(parameter_sets("avcC", &avc[..avc.len() - 1]).is_none());
        assert!(parameter_sets("hvcC", &hevc[..hevc.len() - 1]).is_none());
        assert!(parameter_sets("vpcC", &avc).is_none());
    }
}
