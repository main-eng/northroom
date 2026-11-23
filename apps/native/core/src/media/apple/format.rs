//! What a track's format description says: codec, bit depth and colour. Colour comes from the
//! container's tags first, then what the decoder reports about the stream, as FFmpeg reads it;
//! anything still missing takes the values players assume (BT.601 at 576 lines or fewer,
//! BT.709 above, video range, left-sited chroma).

use super::av::{
    bytes_of, four_character_code, kCMFormatDescriptionExtension_BitsPerComponent,
    kCMFormatDescriptionExtension_FullRangeVideo,
    kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms,
    kCVImageBufferChromaLocationTopFieldKey, kCVImageBufferColorPrimariesKey,
    kCVImageBufferTransferFunctionKey, kCVImageBufferYCbCrMatrixKey, number_of, string_of,
    CFDictionaryGetValue, CFTypeRef, CMFormatDescriptionGetExtension, CVBufferCopyAttachment, Cf,
};
use crate::media::colour::Tags;
use crate::media::{MediaError, MediaResult};
use northroom_media::color::{ChromaSiting, ColorDescription, Matrix, Primaries, Transfer};

/// Core Video's string for BT.470BG's matrix (ISO/IEC 23091-4 code point 5). Core Video has no
/// name for it and uses the number, in format descriptions and decoded pictures alike. Its
/// coefficients are BT.601's.
const BT470BG_MATRIX: &str = "YCbCrMatrix#5";
/// Core Video's string for BT.470BG's transfer (code point 5, "gamma 2.8"), likewise unnamed.
/// It describes the display the 625-line system assumed; the camera curve of 625-line video is
/// BT.601's, which equals BT.709's, so it is drawn as BT.709.
const BT470BG_TRANSFER: &str = "TransferFunction#5";

/// FFmpeg's short codec name for a Core Media codec type.
pub(super) fn codec_name(codec: u32) -> String {
    let known: &[(&[u8; 4], &str)] = &[
        (b"avc1", "h264"),
        (b"hvc1", "hevc"),
        (b"hev1", "hevc"),
        (b"vp09", "vp9"),
        (b"av01", "av1"),
        (b"apch", "prores"),
        (b"apcn", "prores"),
        (b"apcs", "prores"),
        (b"apco", "prores"),
        (b"ap4h", "prores"),
        (b"ap4x", "prores"),
        (b"jpeg", "mjpeg"),
        (b"mp4v", "mpeg4"),
        (b"aac ", "aac"),
        (b".mp3", "mp3"),
        (b"flac", "flac"),
        (b"opus", "opus"),
        (b"vorb", "vorbis"),
        (b"alac", "alac"),
        (b"lpcm", "pcm"),
        (b"ac-3", "ac3"),
        (b"ec-3", "eac3"),
    ];
    known
        .iter()
        .find(|(code, _)| four_character_code(code) == codec)
        .map_or_else(
            || {
                String::from_utf8_lossy(&codec.to_be_bytes())
                    .trim()
                    .to_owned()
            },
            |(_, name)| (*name).to_owned(),
        )
}

/// The codec configuration record stored as `atom` (`avcC`, `vpcC`), if there is one.
///
/// # Safety
/// `format` must be a live format description, alive while the bytes are used.
unsafe fn configuration_atom<'a>(format: CFTypeRef, atom: &str) -> Option<&'a [u8]> {
    // SAFETY: as the caller guarantees; the atoms and the record are owned by the description.
    unsafe {
        let atoms = CMFormatDescriptionGetExtension(
            format,
            kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms,
        );
        let key = Cf::string(atom).ok()?;
        if atoms.is_null() {
            return None;
        }
        bytes_of(CFDictionaryGetValue(atoms, key.0))
    }
}

/// Bits per luma sample: the description's own count, or from the codec configuration record.
///
/// # Safety
/// `format` must be a live video format description.
pub(super) unsafe fn bit_depth(format: CFTypeRef, codec: u32) -> u32 {
    // SAFETY: as the caller guarantees.
    unsafe {
        if let Some(bits) = number_of(CMFormatDescriptionGetExtension(
            format,
            kCMFormatDescriptionExtension_BitsPerComponent,
        )) {
            return bits as u32;
        }
        if codec == four_character_code(b"avc1") {
            return configuration_atom(format, "avcC").map_or(8, avc_bit_depth);
        }
        if codec == four_character_code(b"vp09") {
            return configuration_atom(format, "vpcC").map_or(8, vpc_bit_depth);
        }
    }
    8
}

/// An `avcC` record's luma bit depth: from its high-profile tail if present, else 8.
fn avc_bit_depth(record: &[u8]) -> u32 {
    let high_profile = matches!(record.get(1), Some(100 | 110 | 122 | 144 | 244));
    if !high_profile || record.len() < 6 {
        return 8;
    }
    // Skip the parameter sets: an SPS count, each with a 16-bit length, then the PPS likewise.
    let mut at = 5;
    let sps = usize::from(record[at] & 0x1f);
    at += 1;
    for _ in 0..sps {
        let Some(length) = record.get(at..at + 2) else {
            return 8;
        };
        at += 2 + usize::from(u16::from_be_bytes([length[0], length[1]]));
    }
    let Some(&pps) = record.get(at) else { return 8 };
    at += 1;
    for _ in 0..pps {
        let Some(length) = record.get(at..at + 2) else {
            return 8;
        };
        at += 2 + usize::from(u16::from_be_bytes([length[0], length[1]]));
    }
    // chroma_format, then bit_depth_luma_minus8 in the low three bits.
    record
        .get(at + 1)
        .map_or(8, |byte| u32::from(byte & 0x07) + 8)
}

/// A `vpcC` record's bit depth: the high nibble after its version, flags, profile and level.
fn vpc_bit_depth(record: &[u8]) -> u32 {
    record.get(6).map_or(8, |byte| u32::from(byte >> 4)).max(8)
}

/// The colour strings a description or decoded picture carries, each `None` if absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ColorTags {
    pub(super) primaries: Option<String>,
    pub(super) transfer: Option<String>,
    pub(super) matrix: Option<String>,
    pub(super) chroma: Option<String>,
    pub(super) full_range: Option<bool>,
}

impl ColorTags {
    /// The tags of a format description's extensions.
    ///
    /// # Safety
    /// `format` must be a live video format description.
    pub(super) unsafe fn of_format(format: CFTypeRef) -> Self {
        // SAFETY: as the caller guarantees; the keys are framework constants.
        unsafe {
            let text = |key| string_of(CMFormatDescriptionGetExtension(format, key));
            Self {
                primaries: text(kCVImageBufferColorPrimariesKey),
                transfer: text(kCVImageBufferTransferFunctionKey),
                matrix: text(kCVImageBufferYCbCrMatrixKey),
                chroma: text(kCVImageBufferChromaLocationTopFieldKey),
                full_range: number_of(CMFormatDescriptionGetExtension(
                    format,
                    kCMFormatDescriptionExtension_FullRangeVideo,
                ))
                .map(|value| value != 0.0),
            }
        }
    }

    /// The tags the decoder attached to a decoded picture, describing the stream itself.
    ///
    /// # Safety
    /// `image` must be a live `CVImageBuffer`.
    pub(super) unsafe fn of_image(image: CFTypeRef) -> Self {
        // SAFETY: as the caller guarantees; each copied attachment is released by `Cf`.
        unsafe {
            let text = |key| {
                let value =
                    Cf::retained_copy(CVBufferCopyAttachment(image, key, std::ptr::null_mut()))?;
                string_of(value.0)
            };
            Self {
                primaries: text(kCVImageBufferColorPrimariesKey),
                transfer: text(kCVImageBufferTransferFunctionKey),
                matrix: text(kCVImageBufferYCbCrMatrixKey),
                chroma: text(kCVImageBufferChromaLocationTopFieldKey),
                full_range: None,
            }
        }
    }

    /// These tags, with missing ones filled from `other`.
    pub(super) fn or(self, other: Self) -> Self {
        Self {
            primaries: self.primaries.or(other.primaries),
            transfer: self.transfer.or(other.transfer),
            matrix: self.matrix.or(other.matrix),
            chroma: self.chroma.or(other.chroma),
            full_range: self.full_range.or(other.full_range),
        }
    }

    pub(super) fn is_complete(&self) -> bool {
        self.primaries.is_some() && self.transfer.is_some() && self.matrix.is_some()
    }

    /// What the tags mean for a picture `height` lines tall (`media::colour`).
    pub(super) fn describe(&self, height: u32) -> MediaResult<ColorDescription> {
        let matrix = match self.matrix.as_deref() {
            Some("ITU_R_709_2") => Some(Matrix::Bt709),
            Some("ITU_R_601_4" | BT470BG_MATRIX) => Some(Matrix::Bt601),
            Some("ITU_R_2020") => Some(Matrix::Bt2020),
            None => None,
            Some(other) => return Err(MediaError(format!("unsupported colour matrix: {other}"))),
        };
        let primaries = match self.primaries.as_deref() {
            Some("ITU_R_709_2") => Some(Primaries::Bt709),
            Some("EBU_3213") => Some(Primaries::Bt601Pal),
            Some("SMPTE_C") => Some(Primaries::Bt601Ntsc),
            Some("P3_D65") => Some(Primaries::DisplayP3),
            Some("ITU_R_2020") => Some(Primaries::Bt2020),
            None => None,
            Some(other) => {
                return Err(MediaError(format!("unsupported colour primaries: {other}")))
            }
        };
        let transfer = match self.transfer.as_deref() {
            // BT.601 and SDR BT.2020 share BT.709's curve, and Core Video names them all that way.
            Some("ITU_R_709_2" | "ITU_R_2020" | "SMPTE_170M" | BT470BG_TRANSFER) => {
                Some(Transfer::Bt709)
            }
            Some("IEC_sRGB") => Some(Transfer::Srgb),
            Some("SMPTE_ST_2084_PQ") => Some(Transfer::Pq),
            Some("ITU_R_2100_HLG") => Some(Transfer::Hlg),
            None => None,
            Some(other) => {
                return Err(MediaError(format!(
                    "unsupported transfer function: {other}"
                )))
            }
        };
        let chroma_siting = match self.chroma.as_deref() {
            Some("Center") => Some(ChromaSiting::Center),
            Some("TopLeft") => Some(ChromaSiting::TopLeft),
            _ => None,
        };
        Ok(Tags {
            matrix,
            primaries,
            transfer,
            full_range: self.full_range,
            chroma_siting,
        }
        .describe(height))
    }
}

impl Cf {
    /// Takes ownership of a reference returned by a Copy function, which may be null.
    ///
    /// # Safety
    /// `object` must be null or a live object whose reference the caller gives up.
    unsafe fn retained_copy(object: CFTypeRef) -> Option<Self> {
        // Checked lazily: a `Cf` made from null would be released when dropped.
        (!object.is_null()).then(|| Self(object))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(primaries: Option<&str>, transfer: Option<&str>, matrix: Option<&str>) -> ColorTags {
        ColorTags {
            primaries: primaries.map(str::to_owned),
            transfer: transfer.map(str::to_owned),
            matrix: matrix.map(str::to_owned),
            ..ColorTags::default()
        }
    }

    #[test]
    fn untagged_streams_take_what_players_assume() {
        let sd = ColorTags::default().describe(480).expect("sd");
        assert_eq!(
            (sd.matrix, sd.primaries, sd.transfer),
            (Matrix::Bt601, Primaries::Bt601Ntsc, Transfer::Bt709)
        );
        assert_eq!(
            ColorTags::default().describe(576).expect("pal").primaries,
            Primaries::Bt601Pal
        );
        let hd = ColorTags::default().describe(720).expect("hd");
        assert_eq!(
            (
                hd.matrix,
                hd.primaries,
                hd.transfer,
                hd.full_range,
                hd.chroma_siting
            ),
            (
                Matrix::Bt709,
                Primaries::Bt709,
                Transfer::Bt709,
                false,
                ChromaSiting::Left
            )
        );
    }

    #[test]
    fn tags_name_their_colour() {
        let pq = tags(
            Some("ITU_R_2020"),
            Some("SMPTE_ST_2084_PQ"),
            Some("ITU_R_2020"),
        )
        .describe(2160)
        .expect("pq");
        assert_eq!(
            (pq.matrix, pq.primaries, pq.transfer),
            (Matrix::Bt2020, Primaries::Bt2020, Transfer::Pq)
        );
        assert!(pq.is_hdr());
        let hlg = tags(
            Some("ITU_R_2020"),
            Some("ITU_R_2100_HLG"),
            Some("ITU_R_2020"),
        )
        .describe(1080)
        .expect("hlg");
        assert_eq!(hlg.transfer, Transfer::Hlg);
        let ntsc = tags(Some("SMPTE_C"), Some("ITU_R_709_2"), Some("ITU_R_601_4"))
            .describe(720)
            .expect("601");
        assert_eq!(
            (ntsc.matrix, ntsc.primaries),
            (Matrix::Bt601, Primaries::Bt601Ntsc)
        );
        let p3 = tags(Some("P3_D65"), Some("IEC_sRGB"), None)
            .describe(1080)
            .expect("p3");
        assert_eq!(
            (p3.matrix, p3.primaries, p3.transfer),
            (Matrix::Bt709, Primaries::DisplayP3, Transfer::Srgb)
        );
        assert!(tags(None, None, Some("SMPTE_240M_1995"))
            .describe(1080)
            .is_err());
        let full = ColorTags {
            full_range: Some(true),
            chroma: Some("Center".into()),
            ..ColorTags::default()
        }
        .describe(720)
        .expect("full");
        assert!(full.full_range);
        assert_eq!(full.chroma_siting, ChromaSiting::Center);
    }

    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        fn CVColorPrimariesGetStringForIntegerCodePoint(code: i32) -> CFTypeRef;
        fn CVTransferFunctionGetStringForIntegerCodePoint(code: i32) -> CFTypeRef;
        fn CVYCbCrMatrixGetStringForIntegerCodePoint(code: i32) -> CFTypeRef;
    }

    /// The tags Core Video uses for ISO/IEC 23091-4 code points, the way a Matroska track's
    /// `Colour` and AVFoundation's reading of an MP4 `colr` name them.
    fn named(primaries: i32, transfer: i32, matrix: i32) -> ColorTags {
        // SAFETY: the functions return a constant string or null.
        unsafe {
            ColorTags {
                primaries: string_of(CVColorPrimariesGetStringForIntegerCodePoint(primaries)),
                transfer: string_of(CVTransferFunctionGetStringForIntegerCodePoint(transfer)),
                matrix: string_of(CVYCbCrMatrixGetStringForIntegerCodePoint(matrix)),
                ..ColorTags::default()
            }
        }
    }

    #[test]
    fn standard_definition_code_points_are_bt601() {
        // SMPTE 170M (6) and BT.470BG (5), each tagged fully and by matrix alone.
        let ntsc = named(6, 6, 6).describe(480).expect("SMPTE 170M");
        assert_eq!(
            (ntsc.matrix, ntsc.primaries, ntsc.transfer),
            (Matrix::Bt601, Primaries::Bt601Ntsc, Transfer::Bt709)
        );
        let pal = named(5, 5, 5);
        assert_eq!(
            pal.matrix.as_deref(),
            Some(BT470BG_MATRIX),
            "Core Video calls it by its number"
        );
        assert_eq!(pal.transfer.as_deref(), Some(BT470BG_TRANSFER));
        let pal = pal.describe(576).expect("BT.470BG");
        assert_eq!(
            (pal.matrix, pal.primaries, pal.transfer),
            (Matrix::Bt601, Primaries::Bt601Pal, Transfer::Bt709)
        );
        let matrix_alone = named(2, 2, 5).describe(576).expect("BT.470BG matrix");
        assert_eq!(
            (matrix_alone.matrix, matrix_alone.primaries),
            (Matrix::Bt601, Primaries::Bt601Pal)
        );
        assert_eq!(
            named(2, 2, 6).describe(480).expect("BT.601 matrix").matrix,
            Matrix::Bt601
        );
        // A matrix the converter has no weights for is still rejected: FCC (4), SMPTE 240M (7).
        assert!(named(2, 2, 4).describe(480).is_err());
        assert!(named(2, 2, 7).describe(1080).is_err());
    }

    #[test]
    fn the_container_speaks_before_the_stream() {
        let container = tags(None, None, Some("ITU_R_601_4"));
        let stream = tags(Some("SMPTE_C"), Some("ITU_R_709_2"), Some("ITU_R_709_2"));
        let both = container.or(stream);
        assert_eq!(both.matrix.as_deref(), Some("ITU_R_601_4"));
        assert_eq!(both.primaries.as_deref(), Some("SMPTE_C"));
        assert!(both.is_complete());
    }

    #[test]
    fn configuration_records_give_their_bit_depth() {
        // High 10 with one 4-byte SPS and one 2-byte PPS, then chroma 4:2:0 and 10-bit luma.
        let high10 = [
            1, 110, 0, 30, 0xff, 0xe1, 0, 4, 1, 2, 3, 4, 1, 0, 2, 5, 6, 0xfd, 0xfa, 0xf8, 0,
        ];
        assert_eq!(avc_bit_depth(&high10), 10);
        // Main profile has no tail.
        assert_eq!(
            avc_bit_depth(&[1, 77, 0, 30, 0xff, 0xe1, 0, 1, 9, 1, 0, 1, 9]),
            8
        );
        // High profile without the tail, as many muxers write it.
        assert_eq!(
            avc_bit_depth(&[1, 100, 0, 40, 0xff, 0xe1, 0, 1, 9, 1, 0, 1, 9]),
            8
        );
        assert_eq!(vpc_bit_depth(&[1, 0, 0, 0, 2, 31, 0xa2, 0]), 10);
        assert_eq!(vpc_bit_depth(&[1, 0, 0, 0, 0, 31, 0x82, 0]), 8);
        assert_eq!(codec_name(four_character_code(b"hvc1")), "hevc");
        assert_eq!(codec_name(four_character_code(b"apcn")), "prores");
        assert_eq!(codec_name(four_character_code(b"wxyz")), "wxyz");
    }
}
