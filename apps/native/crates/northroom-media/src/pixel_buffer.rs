//! The frame type passed from a platform video decoder to the GPU. On a Mac it is a retained
//! IOSurface-backed `CVPixelBuffer`, shared with Metal without a copy. Elsewhere it holds the
//! decoder's two planes copied into owned memory, since Media Foundation and GStreamer only
//! expose buffers while locked or mapped, and is uploaded to the GPU when drawn.

use crate::four_character_code;
use crate::{media_error, MediaResult};

#[cfg(target_os = "macos")]
pub use core_video::*;
#[cfg(not(target_os = "macos"))]
pub use elsewhere::*;

/// A biplanar 4:2:0 layout: a luma plane and a half-size plane of interleaved chroma.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelLayout {
    /// 8-bit samples.
    Nv12 { full_range: bool },
    /// 10-bit samples in the high bits of 16-bit words.
    P010 { full_range: bool },
}

impl PixelLayout {
    /// The layout that holds a stream of this bit depth and range without conversion.
    pub fn for_stream(bit_depth: u32, full_range: bool) -> MediaResult<Self> {
        match bit_depth {
            8 => Ok(Self::Nv12 { full_range }),
            10 => Ok(Self::P010 { full_range }),
            depth => media_error(format!("unsupported bit depth: {depth}")),
        }
    }

    pub fn format_type(self) -> u32 {
        four_character_code(match self {
            Self::Nv12 { full_range: false } => b"420v",
            Self::Nv12 { full_range: true } => b"420f",
            Self::P010 { full_range: false } => b"x420",
            Self::P010 { full_range: true } => b"xf20",
        })
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn from_format_type(format_type: u32) -> Option<Self> {
        [false, true]
            .into_iter()
            .flat_map(|full_range| [Self::Nv12 { full_range }, Self::P010 { full_range }])
            .find(|layout| layout.format_type() == format_type)
    }

    pub fn bit_depth(self) -> u32 {
        match self {
            Self::Nv12 { .. } => 8,
            Self::P010 { .. } => 10,
        }
    }

    pub fn full_range(self) -> bool {
        match self {
            Self::Nv12 { full_range } | Self::P010 { full_range } => full_range,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Nv12 { full_range: false } => "nv12-video",
            Self::Nv12 { full_range: true } => "nv12-full",
            Self::P010 { full_range: false } => "p010-video",
            Self::P010 { full_range: true } => "p010-full",
        }
    }
}

#[cfg(target_os = "macos")]
mod core_video {
    use super::PixelLayout;
    use objc2_io_surface::IOSurfaceRef;
    use std::ptr::NonNull;

    #[repr(C)]
    pub struct OpaquePixelBuffer {
        _private: [u8; 0],
    }

    pub type CVPixelBufferRef = *mut OpaquePixelBuffer;

    #[link(name = "CoreVideo", kind = "framework")]
    extern "C" {
        fn CVPixelBufferRetain(buffer: CVPixelBufferRef) -> CVPixelBufferRef;
        fn CVPixelBufferRelease(buffer: CVPixelBufferRef);
        fn CVPixelBufferGetIOSurface(buffer: CVPixelBufferRef) -> *mut IOSurfaceRef;
        fn CVPixelBufferGetPixelFormatType(buffer: CVPixelBufferRef) -> u32;
        fn CVPixelBufferGetWidth(buffer: CVPixelBufferRef) -> usize;
        fn CVPixelBufferGetHeight(buffer: CVPixelBufferRef) -> usize;
        fn CVPixelBufferGetWidthOfPlane(buffer: CVPixelBufferRef, plane: usize) -> usize;
        fn CVPixelBufferGetHeightOfPlane(buffer: CVPixelBufferRef, plane: usize) -> usize;
    }

    pub struct PixelBuffer(NonNull<OpaquePixelBuffer>);

    // SAFETY: CVPixelBuffer reference counting is thread-safe, and this type only reads immutable
    // properties of the buffer; pixel access goes through the GPU.
    unsafe impl Send for PixelBuffer {}
    // SAFETY: as above; no method mutates the buffer.
    unsafe impl Sync for PixelBuffer {}

    impl PixelBuffer {
        /// Takes its own reference to `raw`.
        ///
        /// # Safety
        /// `raw` must be null or a live `CVPixelBufferRef`.
        pub unsafe fn retain(raw: CVPixelBufferRef) -> Option<Self> {
            let buffer = NonNull::new(raw)?;
            // SAFETY: the caller guarantees `raw` is a live pixel buffer.
            unsafe { CVPixelBufferRetain(buffer.as_ptr()) };
            Some(Self(buffer))
        }

        /// Takes over a reference the caller already owns (a Create or Copy result).
        ///
        /// # Safety
        /// `raw` must be null or a live `CVPixelBufferRef` whose reference the caller gives up.
        pub unsafe fn from_owned(raw: CVPixelBufferRef) -> Option<Self> {
            NonNull::new(raw).map(Self)
        }

        pub fn as_ptr(&self) -> CVPixelBufferRef {
            self.0.as_ptr()
        }

        pub fn layout(&self) -> Option<PixelLayout> {
            PixelLayout::from_format_type(self.format_type())
        }

        pub fn format_type(&self) -> u32 {
            // SAFETY: `self.0` is a live pixel buffer for the lifetime of `self`.
            unsafe { CVPixelBufferGetPixelFormatType(self.as_ptr()) }
        }

        pub fn size(&self) -> (u32, u32) {
            // SAFETY: `self.0` is a live pixel buffer for the lifetime of `self`.
            unsafe {
                (
                    CVPixelBufferGetWidth(self.as_ptr()) as u32,
                    CVPixelBufferGetHeight(self.as_ptr()) as u32,
                )
            }
        }

        pub fn plane_size(&self, plane: usize) -> (u32, u32) {
            // SAFETY: `self.0` is a live pixel buffer for the lifetime of `self`.
            unsafe {
                (
                    CVPixelBufferGetWidthOfPlane(self.as_ptr(), plane) as u32,
                    CVPixelBufferGetHeightOfPlane(self.as_ptr(), plane) as u32,
                )
            }
        }

        /// The buffer's IOSurface, if any. Only such buffers can be shared with the GPU.
        pub fn io_surface(&self) -> Option<&IOSurfaceRef> {
            // SAFETY: `self.0` is a live pixel buffer; the buffer owns the returned surface, so it
            // lives at least as long as `self`.
            unsafe { CVPixelBufferGetIOSurface(self.as_ptr()).as_ref() }
        }
    }

    impl Clone for PixelBuffer {
        fn clone(&self) -> Self {
            // SAFETY: `self.0` is a live pixel buffer.
            unsafe { CVPixelBufferRetain(self.as_ptr()) };
            Self(self.0)
        }
    }

    impl Drop for PixelBuffer {
        fn drop(&mut self) {
            // SAFETY: releases the reference taken in `retain`, `from_owned` or `clone`.
            unsafe { CVPixelBufferRelease(self.as_ptr()) };
        }
    }
}

/// Off the Mac: a decoded frame's planes in memory, shared between clones.
#[cfg(not(target_os = "macos"))]
mod elsewhere {
    use super::PixelLayout;
    use crate::{media_error, MediaResult};
    use std::sync::Arc;

    #[derive(Clone)]
    pub struct PixelBuffer(Arc<Planes>);

    struct Planes {
        layout: PixelLayout,
        width: u32,
        height: u32,
        /// Tightly packed rows of `width` samples.
        luma: Vec<u8>,
        /// Half-width rows of interleaved Cb/Cr pairs, half as many rows (rounded up).
        chroma: Vec<u8>,
    }

    /// One plane of a frame in the decoder's memory.
    #[derive(Clone, Copy)]
    pub struct PlaneSource<'a> {
        pub bytes: &'a [u8],
        /// Bytes from the start of one row to the next.
        pub stride: usize,
    }

    impl PixelBuffer {
        /// Copies a `width` × `height` frame of `layout` out of a decoder's planes, row by row.
        /// 10-bit samples keep only their own ten bits, so a converter that fills the low bits by
        /// repeating the top ones (GStreamer's `videoconvert`) yields the same frame as a decoder
        /// that leaves them zero.
        pub fn copy_from(
            layout: PixelLayout,
            width: u32,
            height: u32,
            luma: PlaneSource<'_>,
            chroma: PlaneSource<'_>,
        ) -> MediaResult<Self> {
            if width == 0 || height == 0 {
                return media_error("a frame has no pixels");
            }
            let luma = copy_plane(layout, 0, plane_size(width, height, 0), luma)?;
            let chroma = copy_plane(layout, 1, plane_size(width, height, 1), chroma)?;
            Ok(Self(Arc::new(Planes {
                layout,
                width,
                height,
                luma,
                chroma,
            })))
        }

        pub fn layout(&self) -> Option<PixelLayout> {
            Some(self.0.layout)
        }

        pub fn format_type(&self) -> u32 {
            self.0.layout.format_type()
        }

        pub fn size(&self) -> (u32, u32) {
            (self.0.width, self.0.height)
        }

        pub fn plane_size(&self, plane: usize) -> (u32, u32) {
            plane_size(self.0.width, self.0.height, plane)
        }

        /// The bytes of `plane` (0 luma, 1 chroma), packed at `row_bytes` per row.
        pub fn plane(&self, plane: usize) -> &[u8] {
            if plane == 0 {
                &self.0.luma
            } else {
                &self.0.chroma
            }
        }

        pub fn row_bytes(&self, plane: usize) -> usize {
            row_bytes(self.0.layout, self.plane_size(plane).0, plane)
        }
    }

    /// Copies `columns` × `rows` samples of `plane` out of decoder memory, dropping row padding
    /// and keeping only each sample's own bits.
    fn copy_plane(
        layout: PixelLayout,
        plane: usize,
        (columns, rows): (u32, u32),
        source: PlaneSource<'_>,
    ) -> MediaResult<Vec<u8>> {
        let row_bytes = row_bytes(layout, columns, plane);
        let needed = (rows as usize - 1) * source.stride + row_bytes;
        if source.stride < row_bytes || source.bytes.len() < needed {
            return media_error(format!(
                "plane {plane} holds {} bytes in rows {} apart, not {rows} rows of {row_bytes}",
                source.bytes.len(),
                source.stride
            ));
        }
        let mut copied = Vec::new();
        copied.reserve_exact(row_bytes * rows as usize);
        for row in 0..rows as usize {
            let start = row * source.stride;
            copied.extend_from_slice(&source.bytes[start..start + row_bytes]);
        }
        keep_sample_bits(layout, &mut copied);
        Ok(copied)
    }

    /// For deeper-than-8-bit layouts, clears the bits below the sample in each little-endian word.
    fn keep_sample_bits(layout: PixelLayout, plane: &mut [u8]) {
        if layout.bit_depth() <= 8 {
            return;
        }
        let kept = u16::MAX << (16 - layout.bit_depth());
        for word in plane.as_chunks_mut::<2>().0 {
            *word = (u16::from_le_bytes(*word) & kept).to_le_bytes();
        }
    }

    /// Plane 0 is full size; plane 1 is half each way, rounded up.
    fn plane_size(width: u32, height: u32, plane: usize) -> (u32, u32) {
        if plane == 0 {
            (width, height)
        } else {
            (width.div_ceil(2), height.div_ceil(2))
        }
    }

    /// Bytes in a row of `columns` samples of `plane`. A luma sample is one value and a chroma
    /// sample two, each a byte at 8 bits or a little-endian word when deeper.
    fn row_bytes(layout: PixelLayout, columns: u32, plane: usize) -> usize {
        let values = if plane == 0 { 1 } else { 2 };
        let bytes = if layout.bit_depth() > 8 { 2 } else { 1 };
        columns as usize * values * bytes
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn planes_are_copied_without_the_decoders_padding() {
            let layout = PixelLayout::Nv12 { full_range: false };
            // 3 x 3: luma rows of 3 in strides of 4, chroma rows of 2 pairs in strides of 6.
            let luma = [1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9];
            let chroma = [10, 11, 12, 13, 0, 0, 14, 15, 16, 17];
            let frame = PixelBuffer::copy_from(
                layout,
                3,
                3,
                PlaneSource {
                    bytes: &luma,
                    stride: 4,
                },
                PlaneSource {
                    bytes: &chroma,
                    stride: 6,
                },
            )
            .expect("frame");
            assert_eq!(frame.plane(0), [1, 2, 3, 4, 5, 6, 7, 8, 9]);
            assert_eq!(frame.plane(1), [10, 11, 12, 13, 14, 15, 16, 17]);
            assert_eq!((frame.plane_size(1), frame.row_bytes(1)), ((2, 2), 4));
        }

        #[test]
        fn a_plane_shorter_than_its_rows_is_refused() {
            let layout = PixelLayout::P010 { full_range: false };
            // Two rows of two 16-bit samples, and one row of one pair.
            let short = PlaneSource {
                bytes: &[0; 7],
                stride: 4,
            };
            let luma = PlaneSource {
                bytes: &[0; 8],
                stride: 4,
            };
            let chroma = PlaneSource {
                bytes: &[0; 4],
                stride: 4,
            };
            assert!(PixelBuffer::copy_from(layout, 2, 2, short, chroma).is_err());
            assert!(PixelBuffer::copy_from(layout, 2, 2, luma, chroma).is_ok());
        }

        #[test]
        fn ten_bit_samples_keep_only_their_ten_bits() {
            let layout = PixelLayout::P010 { full_range: false };
            // 940 widened by repeating its top bits (0xEB3A), and neutral chroma likewise.
            let luma = 0xeb3a_u16.to_le_bytes().repeat(4);
            let chroma = 0x8020_u16.to_le_bytes().repeat(2);
            let frame = PixelBuffer::copy_from(
                layout,
                2,
                2,
                PlaneSource {
                    bytes: &luma,
                    stride: 4,
                },
                PlaneSource {
                    bytes: &chroma,
                    stride: 4,
                },
            )
            .expect("frame");
            assert_eq!(frame.plane(0), (940_u16 << 6).to_le_bytes().repeat(4));
            assert_eq!(frame.plane(1), (512_u16 << 6).to_le_bytes().repeat(2));
        }
    }
}
