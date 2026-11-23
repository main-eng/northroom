//! Matroska codec settings converted to what MP4-style decoders (such as Apple's) expect.
//! H.264, HEVC and AV1 store their MP4 configuration records as `CodecPrivate`. VP9's `vpcC`
//! record is built from the first key frame's header. AAC's `AudioSpecificConfig` is wrapped in
//! an `esds` descriptor and FLAC's metadata in a `dfLa` box. Opus and Vorbis keep theirs
//! (`OpusHead`, the three Xiph-laced Vorbis headers).

use super::{Colour, Track};
use crate::media::{MediaError, MediaResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VideoCodec {
    H264,
    Hevc,
    Vp9,
    Av1,
    ProRes,
}

/// A video track's codec, ready for a decoder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct VideoConfiguration {
    pub(crate) codec: VideoCodec,
    /// The MP4 sample-description atom that configures the decoder, and its bytes.
    pub(crate) atom: Option<(&'static str, Vec<u8>)>,
    /// ProRes's four-character code (`apcn`, `ap4h`).
    pub(crate) prores_type: Option<[u8; 4]>,
    /// Bits per luma sample, if the stream says.
    pub(crate) bit_depth: Option<u32>,
    /// FFmpeg's short codec name, for messages and details.
    pub(crate) name: &'static str,
}

/// A video track's codec, given its first key frame (VP9 keeps its settings there).
pub(crate) fn video_configuration(
    track: &Track,
    first_keyframe: Option<&[u8]>,
) -> MediaResult<VideoConfiguration> {
    let private = &track.codec_private;
    let needs_private = |name: &str| {
        if private.is_empty() {
            Err(MediaError(format!(
                "the {name} track has no codec configuration"
            )))
        } else {
            Ok(())
        }
    };
    let configuration =
        |codec, atom: Option<(&'static str, Vec<u8>)>, bit_depth, name| VideoConfiguration {
            codec,
            atom,
            prores_type: None,
            bit_depth,
            name,
        };
    match track.codec_id.as_str() {
        "V_MPEG4/ISO/AVC" => {
            needs_private("H.264")?;
            Ok(configuration(
                VideoCodec::H264,
                Some(("avcC", private.clone())),
                None,
                "h264",
            ))
        }
        "V_MPEGH/ISO/HEVC" => {
            needs_private("HEVC")?;
            let depth = private.get(17).map(|byte| u32::from(byte & 0x07) + 8);
            Ok(configuration(
                VideoCodec::Hevc,
                Some(("hvcC", private.clone())),
                depth,
                "hevc",
            ))
        }
        "V_AV1" => {
            needs_private("AV1")?;
            Ok(configuration(
                VideoCodec::Av1,
                Some(("av1C", private.clone())),
                av1_bit_depth(private),
                "av1",
            ))
        }
        "V_VP9" => {
            let header = first_keyframe.and_then(vp9_keyframe_header);
            let record = vpcc(
                private,
                header.as_ref(),
                &track.video.clone().unwrap_or_default().colour,
            );
            Ok(configuration(
                VideoCodec::Vp9,
                Some(("vpcC", record)),
                header.map(|header| header.bit_depth),
                "vp9",
            ))
        }
        "V_PRORES" => {
            let code = private
                .get(..4)
                .and_then(|code| <[u8; 4]>::try_from(code).ok())
                .unwrap_or(*b"apcn");
            // AVFoundation describes every ProRes stream as 12 bits per component, which its
            // decoder fills.
            Ok(VideoConfiguration {
                codec: VideoCodec::ProRes,
                atom: None,
                prores_type: Some(code),
                bit_depth: Some(12),
                name: "prores",
            })
        }
        "V_VP8" => Err(MediaError("VP8 video has no decoder on this Mac".into())),
        other => Err(MediaError(format!("{other} video is not supported"))),
    }
}

/// A frame as the decoder expects it; Matroska stores ProRes frames without their 8-byte header.
pub(crate) fn restore_frame(codec: VideoCodec, frame: Vec<u8>) -> Vec<u8> {
    if codec != VideoCodec::ProRes || frame.get(4..8) == Some(b"icpf") {
        return frame;
    }
    let mut whole = Vec::with_capacity(frame.len() + 8);
    whole.extend_from_slice(&((frame.len() + 8) as u32).to_be_bytes());
    whole.extend_from_slice(b"icpf");
    whole.extend_from_slice(&frame);
    whole
}

fn av1_bit_depth(record: &[u8]) -> Option<u32> {
    let byte = *record.get(2)?;
    Some(match (byte & 0x40 != 0, byte & 0x20 != 0) {
        (false, _) => 8,
        (true, false) => 10,
        (true, true) => 12,
    })
}

/// Stream properties from a VP9 key frame's uncompressed header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Vp9Header {
    pub(crate) profile: u8,
    pub(crate) bit_depth: u32,
    pub(crate) full_range: bool,
    /// `vpcC`'s chroma subsampling: 0 or 1 for 4:2:0, 2 for 4:2:2, 3 for 4:4:4.
    pub(crate) subsampling: u8,
}

/// The header of a VP9 key frame (VP9 bitstream specification §6.2), or `None` for any other
/// frame.
pub(crate) fn vp9_keyframe_header(frame: &[u8]) -> Option<Vp9Header> {
    let mut bits = Bits {
        bytes: frame,
        at: 0,
    };
    let profile = vp9_key_frame_profile(&mut bits)?;
    let bit_depth = vp9_bit_depth(&mut bits, profile)?;
    let (full_range, subsampling) = vp9_colour(&mut bits, profile)?;
    Some(Vp9Header {
        profile,
        bit_depth,
        full_range,
        subsampling,
    })
}

/// Reads a VP9 frame's start through its sync code; returns its profile, or `None` if it isn't
/// a key frame.
fn vp9_key_frame_profile(bits: &mut Bits) -> Option<u8> {
    if bits.read(2)? != 2 {
        return None;
    }
    let low = bits.read(1)?;
    let high = bits.read(1)?;
    let profile = (high << 1 | low) as u8;
    if profile == 3 {
        bits.read(1)?;
    }
    // show_existing_frame, then frame_type (0 is a key frame), show_frame, error_resilient_mode.
    if bits.read(1)? == 1 || bits.read(1)? != 0 {
        return None;
    }
    bits.read(2)?;
    if bits.read(24)? != 0x49_8342 {
        return None;
    }
    Some(profile)
}

/// A key frame's bit depth: 8 in profiles 0 and 1, else 10 or 12 per its flag bit.
fn vp9_bit_depth(bits: &mut Bits, profile: u8) -> Option<u32> {
    if profile < 2 {
        return Some(8);
    }
    Some(if bits.read(1)? == 1 { 12 } else { 10 })
}

/// A key frame's colour range and its `vpcC` chroma subsampling.
fn vp9_colour(bits: &mut Bits, profile: u8) -> Option<(bool, u8)> {
    const RGB: u32 = 7;
    if bits.read(3)? == RGB {
        return Some((true, 3));
    }
    let full = bits.read(1)? == 1;
    if profile != 1 && profile != 3 {
        return Some((full, 0));
    }
    let subsampling = match (bits.read(1)?, bits.read(1)?) {
        (1, 1) => 1,
        (1, 0) => 2,
        _ => 3,
    };
    Some((full, subsampling))
}

/// A version 1 `vpcC` record (VP Codec ISO Media File Format Binding §2.3): profile, level, bit
/// depth and colour, from Matroska's VP9 `CodecPrivate` features, the first key frame and the
/// track's colour.
pub(crate) fn vpcc(private: &[u8], header: Option<&Vp9Header>, colour: &Colour) -> Vec<u8> {
    // CodecPrivate for VP9 is a list of (ID, length, value): 1 profile, 2 level, 3 bit depth,
    // 4 chroma subsampling.
    let mut features = [None::<u8>; 5];
    let mut at = 0;
    while at + 2 <= private.len() {
        let (id, length) = (usize::from(private[at]), usize::from(private[at + 1]));
        if let (Some(slot), Some(&value)) = (features.get_mut(id), private.get(at + 2)) {
            if length == 1 {
                *slot = Some(value);
            }
        }
        at += 2 + length;
    }
    let profile = header
        .map(|header| header.profile)
        .or(features[1])
        .unwrap_or(0);
    let level = features[2].unwrap_or(0);
    let bit_depth = header
        .map(|header| header.bit_depth as u8)
        .or(features[3])
        .unwrap_or(8);
    let chroma = match (
        colour.chroma_siting_horizontal,
        colour.chroma_siting_vertical,
        header,
    ) {
        // 4:2:0 sited with the top-left luma sample is "collocated"; otherwise "vertical".
        (_, _, Some(header)) if header.subsampling >= 2 => header.subsampling,
        (Some(1), Some(1), _) => 1,
        _ => features[4].unwrap_or(0),
    };
    let full_range = header
        .map(|header| header.full_range)
        .unwrap_or(colour.range == Some(2));
    const UNSPECIFIED: u64 = 2;
    let code =
        |value: Option<u64>| value.filter(|value| *value <= 255).unwrap_or(UNSPECIFIED) as u8;
    vec![
        1,
        0,
        0,
        0, // version 1, no flags
        profile,
        level,
        (bit_depth << 4) | (chroma << 1) | u8::from(full_range),
        code(colour.primaries),
        code(colour.transfer),
        code(colour.matrix),
        0,
        0, // no codec initialization data
    ]
}

struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn read(&mut self, count: usize) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            let byte = *self.bytes.get(self.at / 8)?;
            value = (value << 1) | u32::from((byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Some(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AudioCodec {
    Aac,
    Opus,
    Vorbis,
    Flac,
    Mp3,
    Mp2,
    Mp1,
    Ac3,
    Eac3,
    Alac,
    /// Linear PCM of `bits` per sample.
    Pcm {
        bits: u32,
        float: bool,
        big_endian: bool,
    },
}

/// An audio track's codec, ready for a decoder.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AudioConfiguration {
    pub(crate) codec: AudioCodec,
    /// The decoder configuration Apple's decoders take (a "magic cookie").
    pub(crate) cookie: Vec<u8>,
    /// The decoded sample rate.
    pub(crate) sample_rate: u32,
    pub(crate) channels: u32,
    /// Frames per packet, when every packet holds the same number.
    pub(crate) frames_per_packet: Option<u32>,
    /// Every packet starts a whole number of these frames after the first, so a timestamp
    /// rounded to the file's precision can be snapped back to the sample.
    pub(crate) grid: Option<u32>,
    /// Samples decoded before the first played one.
    pub(crate) delay: u64,
    pub(crate) name: String,
}

/// An audio track's codec; errors if no decoder here supports it.
pub(crate) fn audio_configuration(track: &Track) -> MediaResult<AudioConfiguration> {
    let audio = track
        .audio
        .as_ref()
        .ok_or_else(|| MediaError("the audio track has no settings".into()))?;
    let channels = audio.channels.max(1);
    let id = track.codec_id.as_str();
    let private = &track.codec_private;
    // Opus always decodes at 48 kHz.
    let rate = if id == "A_OPUS" {
        48_000
    } else {
        audio.sample_rate.round().max(1.0) as u32
    };
    let packets = packet_codec(id, private, rate, channels, audio.bit_depth)?;
    let delay_ns = codec_delay_ns(track, packets.codec);
    let delay = (delay_ns as f64 * f64::from(rate) / 1e9).round() as u64;
    Ok(AudioConfiguration {
        codec: packets.codec,
        cookie: packets.cookie,
        sample_rate: rate,
        channels,
        frames_per_packet: packets.frames_per_packet,
        grid: packets.grid,
        delay,
        name: packets.name,
    })
}

/// How a codec's packets decode: what the decoder receives, and how packets map to samples.
struct PacketCodec {
    codec: AudioCodec,
    cookie: Vec<u8>,
    frames_per_packet: Option<u32>,
    grid: Option<u32>,
    name: String,
}

impl PacketCodec {
    /// A codec with no cookie and exactly `frames` frames per packet.
    fn fixed(codec: AudioCodec, frames: u32, name: &str) -> Self {
        Self {
            codec,
            cookie: Vec::new(),
            frames_per_packet: Some(frames),
            grid: Some(frames),
            name: name.into(),
        }
    }
}

/// The codec for Matroska codec ID `id`, with settings `private`, at `rate` in `channels`.
/// Errors if no decoder here supports it.
fn packet_codec(
    id: &str,
    private: &[u8],
    rate: u32,
    channels: u32,
    bit_depth: Option<u32>,
) -> MediaResult<PacketCodec> {
    match id {
        "A_AAC" | "A_AAC/MPEG4/LC" | "A_AAC/MPEG4/MAIN" | "A_AAC/MPEG4/LC/SBR"
        | "A_AAC/MPEG2/LC" | "A_AAC/MPEG2/MAIN" | "A_AAC/MPEG2/LC/SBR" => {
            Ok(aac_codec(id, private, rate, channels))
        }
        "A_OPUS" => opus_codec(private),
        "A_VORBIS" => {
            let short_block = vorbis_short_block(private)
                .ok_or_else(|| MediaError("the Vorbis track's headers are malformed".into()))?;
            Ok(PacketCodec {
                codec: AudioCodec::Vorbis,
                cookie: private.to_vec(),
                frames_per_packet: None,
                grid: Some(short_block / 4),
                name: "vorbis".into(),
            })
        }
        "A_FLAC" => {
            let (cookie, block_size) = dfla(private)
                .ok_or_else(|| MediaError("the FLAC track has no STREAMINFO".into()))?;
            Ok(PacketCodec {
                codec: AudioCodec::Flac,
                cookie,
                frames_per_packet: block_size,
                grid: block_size,
                name: "flac".into(),
            })
        }
        "A_MPEG/L3" => {
            let frames = if rate >= 32_000 { 1152 } else { 576 };
            Ok(PacketCodec::fixed(AudioCodec::Mp3, frames, "mp3"))
        }
        "A_MPEG/L2" => Ok(PacketCodec::fixed(AudioCodec::Mp2, 1152, "mp2")),
        "A_MPEG/L1" => Ok(PacketCodec::fixed(AudioCodec::Mp1, 384, "mp1")),
        "A_AC3" => Ok(PacketCodec::fixed(AudioCodec::Ac3, 1536, "ac3")),
        "A_EAC3" => Ok(PacketCodec {
            frames_per_packet: None,
            ..PacketCodec::fixed(AudioCodec::Eac3, 256, "eac3")
        }),
        "A_ALAC" => {
            let frames = alac_frames_per_packet(private);
            Ok(PacketCodec {
                codec: AudioCodec::Alac,
                cookie: private.to_vec(),
                frames_per_packet: frames,
                grid: frames,
                name: "alac".into(),
            })
        }
        "A_PCM/INT/LIT" | "A_PCM/INT/BIG" | "A_PCM/FLOAT/IEEE" => pcm_codec(id, bit_depth),
        "A_DTS" | "A_DTS/EXPRESS" | "A_DTS/LOSSLESS" => {
            Err(MediaError("DTS audio has no decoder on this Mac".into()))
        }
        "A_TRUEHD" | "A_MLP" => Err(MediaError(
            "Dolby TrueHD audio has no decoder on this Mac".into(),
        )),
        other => Err(MediaError(format!("{other} audio is not supported"))),
    }
}

/// AAC, building its `AudioSpecificConfig` from the codec ID when the track has none.
fn aac_codec(id: &str, private: &[u8], rate: u32, channels: u32) -> PacketCodec {
    let config = if private.is_empty() {
        audio_specific_config(id, rate, channels)
    } else {
        private.to_vec()
    };
    PacketCodec {
        cookie: esds(&config),
        ..PacketCodec::fixed(AudioCodec::Aac, 1024, "aac")
    }
}

/// Opus, whose frames are multiples of 2.5 ms; errors without its `OpusHead`.
fn opus_codec(private: &[u8]) -> MediaResult<PacketCodec> {
    if private.len() < 19 || &private[..8] != b"OpusHead" {
        return Err(MediaError("the Opus track has no OpusHead".into()));
    }
    Ok(PacketCodec {
        codec: AudioCodec::Opus,
        cookie: private.to_vec(),
        frames_per_packet: None,
        grid: Some(120),
        name: "opus".into(),
    })
}

/// ALAC's frames per packet, the first field of its `ALACSpecificConfig`, bare or inside its
/// 'alac' atom.
fn alac_frames_per_packet(private: &[u8]) -> Option<u32> {
    let config = if private.get(4..8) == Some(b"alac") {
        private.get(12..)
    } else {
        Some(private)
    };
    config
        .and_then(|config| config.get(..4))
        .map(|bytes| u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Linear PCM per the codec ID and bit depth; errors for a depth the decoder doesn't support.
fn pcm_codec(id: &str, bit_depth: Option<u32>) -> MediaResult<PacketCodec> {
    let float = id == "A_PCM/FLOAT/IEEE";
    let big_endian = id == "A_PCM/INT/BIG";
    let bits = bit_depth.unwrap_or(if float { 32 } else { 16 });
    if !matches!((float, bits), (false, 8 | 16 | 24 | 32) | (true, 32 | 64)) {
        return Err(MediaError(format!("{bits}-bit {id} is not supported")));
    }
    let name = match (float, bits, big_endian) {
        (true, bits, _) => format!("pcm_f{bits}le"),
        (false, 8, _) => "pcm_u8".into(),
        (false, bits, true) => format!("pcm_s{bits}be"),
        (false, bits, false) => format!("pcm_s{bits}le"),
    };
    Ok(PacketCodec {
        codec: AudioCodec::Pcm {
            bits,
            float,
            big_endian,
        },
        cookie: Vec::new(),
        frames_per_packet: Some(1),
        grid: None,
        name,
    })
}

/// Samples the decoder outputs before the first played one, in nanoseconds: the track's
/// `CodecDelay`, or Opus's own pre-skip without it.
fn codec_delay_ns(track: &Track, codec: AudioCodec) -> u64 {
    if track.codec_delay_ns > 0 {
        track.codec_delay_ns
    } else if codec == AudioCodec::Opus {
        let private = &track.codec_private;
        u64::from(u16::from_le_bytes([private[10], private[11]])) * 1_000_000_000 / 48_000
    } else {
        0
    }
}

/// MPEG-4 `AudioSpecificConfig` for legacy codec IDs that name the profile and carry no config.
fn audio_specific_config(id: &str, rate: u32, channels: u32) -> Vec<u8> {
    const RATES: [u32; 13] = [
        96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
        8_000, 7_350,
    ];
    let object = if id.ends_with("MAIN") { 1 } else { 2 };
    let index = RATES.iter().position(|known| *known == rate).unwrap_or(4) as u16;
    let value = (object << 11) | (index << 7) | ((channels.min(7) as u16) << 3);
    value.to_be_bytes().to_vec()
}

/// An `esds` elementary stream descriptor around an `AudioSpecificConfig`, the form Core
/// Audio's AAC decoder takes.
pub(crate) fn esds(config: &[u8]) -> Vec<u8> {
    fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
        let length = body.len() as u32;
        let mut bytes = vec![
            tag,
            0x80 | (length >> 21) as u8 & 0x7f,
            0x80 | (length >> 14) as u8 & 0x7f,
            0x80 | (length >> 7) as u8 & 0x7f,
            length as u8 & 0x7f,
        ];
        bytes.extend_from_slice(body);
        bytes
    }
    // Object type 0x40 (MPEG-4 audio), stream type 5 (audio) with the upstream bit clear and the
    // reserved bit set, then buffer size and bit rates left at zero.
    let mut decoder_config = vec![0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    decoder_config.extend(descriptor(0x05, config));
    let mut stream = vec![0, 0, 0];
    stream.extend(descriptor(0x04, &decoder_config));
    stream.extend(descriptor(0x06, &[0x02]));
    descriptor(0x03, &stream)
}

/// A `dfLa` box (FLAC in ISO media) holding the STREAMINFO from Matroska's `CodecPrivate`
/// ("fLaC" then the metadata blocks), and the block size when fixed.
pub(crate) fn dfla(private: &[u8]) -> Option<(Vec<u8>, Option<u32>)> {
    let blocks = private.strip_prefix(b"fLaC").unwrap_or(private);
    // STREAMINFO is always first: a header (type 0) then 34 bytes.
    let (header, info) = (blocks.get(..4)?, blocks.get(4..38)?);
    if header[0] & 0x7f != 0 {
        return None;
    }
    let mut cookie = Vec::with_capacity(50);
    cookie.extend_from_slice(&50_u32.to_be_bytes());
    cookie.extend_from_slice(b"dfLa");
    cookie.extend_from_slice(&[0, 0, 0, 0]);
    cookie.extend_from_slice(&[0x80, 0, 0, 34]);
    cookie.extend_from_slice(info);
    let (smallest, largest) = (
        u16::from_be_bytes([info[0], info[1]]),
        u16::from_be_bytes([info[2], info[3]]),
    );
    Some((cookie, (smallest == largest).then_some(u32::from(smallest))))
}

/// The shorter of a Vorbis stream's two block sizes, from its identification header (the first
/// Xiph-laced header).
fn vorbis_short_block(private: &[u8]) -> Option<u32> {
    let count = usize::from(*private.first()?) + 1;
    let mut at = 1;
    // Step over the lace sizes of all but the last header.
    for _ in 1..count {
        while *private.get(at)? == 255 {
            at += 1;
        }
        at += 1;
    }
    let identification = private.get(at..at + 30)?;
    if &identification[..7] != b"\x01vorbis" {
        return None;
    }
    Some(1 << (identification[28] & 0x0f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_audio_specific_config_is_wrapped_as_core_audio_writes_esds() {
        // Core Audio's own cookie for 48 kHz stereo AAC from an FFmpeg-written M4A, minus the
        // bit rates and buffer size, which it fills in and the decoder doesn't need.
        let config = [0x11, 0x90, 0x56, 0xe5, 0x00];
        let cookie = esds(&config);
        let expected_head = [
            0x03, 0x80, 0x80, 0x80, 0x25, 0x00, 0x00, 0x00, 0x04, 0x80, 0x80, 0x80, 0x17, 0x40,
            0x15,
        ];
        assert_eq!(cookie[..15], expected_head);
        let tail = [
            0x05, 0x80, 0x80, 0x80, 0x05, 0x11, 0x90, 0x56, 0xe5, 0x00, 0x06, 0x80, 0x80, 0x80,
            0x01, 0x02,
        ];
        assert_eq!(cookie[cookie.len() - tail.len()..], tail);
        assert_eq!(cookie.len(), 42);
    }

    #[test]
    fn flac_metadata_becomes_a_dfla_box_with_streaminfo_alone() {
        let mut private = b"fLaC".to_vec();
        private.extend([0x00, 0, 0, 34]);
        let mut info = vec![0x12, 0x00, 0x12, 0x00];
        info.resize(34, 7);
        private.extend(&info);
        private.extend([0x84, 0, 0, 2, 9, 9]);
        let (cookie, block) = dfla(&private).unwrap();
        assert_eq!(
            &cookie[..16],
            &[0, 0, 0, 0x32, b'd', b'f', b'L', b'a', 0, 0, 0, 0, 0x80, 0, 0, 0x22]
        );
        assert_eq!(&cookie[16..], &info[..]);
        assert_eq!(block, Some(4608));
    }

    #[test]
    fn a_vp9_key_frame_header_gives_profile_depth_and_range() {
        // Profile 0 key frame: marker 10, profile 00, show_existing 0, key 0, show 1, error
        // resilient 0, sync code, colour space 1 (BT.601), studio range.
        let frame = [0b1000_0010, 0x49, 0x83, 0x42, 0b0010_0000];
        assert_eq!(
            vp9_keyframe_header(&frame),
            Some(Vp9Header {
                profile: 0,
                bit_depth: 8,
                full_range: false,
                subsampling: 0
            })
        );
        // Profile 2 (low 0, high 1), 10-bit, full range.
        let frame = [0b1001_0010, 0x49, 0x83, 0x42, 0b0001_1000];
        assert_eq!(
            vp9_keyframe_header(&frame),
            Some(Vp9Header {
                profile: 2,
                bit_depth: 10,
                full_range: true,
                subsampling: 0
            })
        );
        // An inter frame has no such header.
        assert_eq!(vp9_keyframe_header(&[0b1000_0110, 0, 0, 0]), None);
    }

    #[test]
    fn a_vpcc_record_holds_what_the_stream_says() {
        let header = Vp9Header {
            profile: 2,
            bit_depth: 10,
            full_range: false,
            subsampling: 0,
        };
        let colour = Colour {
            primaries: Some(9),
            transfer: Some(16),
            matrix: Some(9),
            ..Colour::default()
        };
        // CodecPrivate naming level 4.1 (41).
        let record = vpcc(&[2, 1, 41], Some(&header), &colour);
        assert_eq!(record, vec![1, 0, 0, 0, 2, 41, 0xa0, 9, 16, 9, 0, 0]);
    }

    #[test]
    fn vorbis_headers_give_the_short_block_size() {
        // Three headers, the first 30 bytes, the second 3; block sizes 2^8 and 2^11.
        let mut private = vec![2, 30, 3];
        let mut identification = b"\x01vorbis".to_vec();
        identification.resize(28, 0);
        identification.extend([0xb8, 1]);
        private.extend(identification);
        private.extend([3, 0, 0, 5, 5]);
        assert_eq!(vorbis_short_block(&private), Some(256));
    }

    #[test]
    fn prores_frames_get_their_header_back() {
        let frame = restore_frame(VideoCodec::ProRes, vec![1, 2, 3]);
        assert_eq!(frame, vec![0, 0, 0, 11, b'i', b'c', b'p', b'f', 1, 2, 3]);
        assert_eq!(restore_frame(VideoCodec::ProRes, frame.clone()), frame);
        assert_eq!(restore_frame(VideoCodec::H264, vec![1]), vec![1]);
    }
}
