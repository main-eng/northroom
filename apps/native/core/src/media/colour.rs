//! What a stream's colour tags mean, shared by every platform. Each backend maps the tags its
//! framework reads (Core Video strings, Media Foundation enums, GStreamer colorimetry); missing
//! tags take the values players assume: BT.601 at 576 lines or fewer, BT.709 above, video
//! range, left-sited chroma.

use northroom_media::color::{ChromaSiting, ColorDescription, Matrix, Primaries, Transfer};

/// Frames up to this height are standard definition, which untagged streams encode as BT.601.
const TALLEST_STANDARD_DEFINITION_HEIGHT: u32 = 576;
/// Standard definition at or below this height is the 525-line system.
const TALLEST_525_LINE_HEIGHT: u32 = 480;

/// A stream's colour as tagged; `None` where a tag is missing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Tags {
    pub(super) matrix: Option<Matrix>,
    pub(super) primaries: Option<Primaries>,
    pub(super) transfer: Option<Transfer>,
    pub(super) full_range: Option<bool>,
    pub(super) chroma_siting: Option<ChromaSiting>,
}

impl Tags {
    /// These tags, with missing ones filled from `other`. Media Foundation reports colour in
    /// both the container's type and the decoder's; GStreamer caps report it once, and a Mac's
    /// tags are strings (`apple::format`).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(super) fn or(self, other: Self) -> Self {
        Self {
            matrix: self.matrix.or(other.matrix),
            primaries: self.primaries.or(other.primaries),
            transfer: self.transfer.or(other.transfer),
            full_range: self.full_range.or(other.full_range),
            chroma_siting: self.chroma_siting.or(other.chroma_siting),
        }
    }

    /// What the tags mean for a picture `height` lines tall.
    pub(super) fn describe(&self, height: u32) -> ColorDescription {
        let matrix = self
            .matrix
            .unwrap_or(if height <= TALLEST_STANDARD_DEFINITION_HEIGHT {
                Matrix::Bt601
            } else {
                Matrix::Bt709
            });
        // Untagged primaries follow the matrix.
        let primaries = self.primaries.unwrap_or(match matrix {
            Matrix::Bt709 => Primaries::Bt709,
            Matrix::Bt2020 => Primaries::Bt2020,
            Matrix::Bt601 if height <= TALLEST_525_LINE_HEIGHT => Primaries::Bt601Ntsc,
            Matrix::Bt601 => Primaries::Bt601Pal,
        });
        ColorDescription {
            matrix,
            primaries,
            transfer: self.transfer.unwrap_or(Transfer::Bt709),
            full_range: self.full_range.unwrap_or(false),
            chroma_siting: self.chroma_siting.unwrap_or(ChromaSiting::Left),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untagged_streams_take_what_players_assume() {
        let sd = Tags::default().describe(480);
        assert_eq!(
            (sd.matrix, sd.primaries, sd.transfer),
            (Matrix::Bt601, Primaries::Bt601Ntsc, Transfer::Bt709)
        );
        assert_eq!(Tags::default().describe(576).primaries, Primaries::Bt601Pal);
        let hd = Tags::default().describe(720);
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
        let bt2020 = Tags {
            matrix: Some(Matrix::Bt2020),
            ..Tags::default()
        }
        .describe(480);
        assert_eq!(bt2020.primaries, Primaries::Bt2020);
    }

    #[test]
    fn the_first_tags_speak_before_the_others() {
        let container = Tags {
            matrix: Some(Matrix::Bt601),
            ..Tags::default()
        };
        let stream = Tags {
            matrix: Some(Matrix::Bt709),
            primaries: Some(Primaries::Bt601Ntsc),
            ..Tags::default()
        };
        let both = container.or(stream);
        assert_eq!(
            (both.matrix, both.primaries, both.transfer),
            (Some(Matrix::Bt601), Some(Primaries::Bt601Ntsc), None)
        );
    }
}
