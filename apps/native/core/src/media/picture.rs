//! Picture size as shown on Linux, read from the header by the `image` crate, the reader the
//! GPUI shell draws pictures with (`kit::platform::image_thumbnail`). Orientations 5 to 8 swap
//! width and height.

use image::metadata::Orientation;
use image::ImageDecoder;

/// The size in pixels the picture at `path` is shown at; `None` if the reader can't read it or
/// it has no size.
pub fn picture_dimensions(path: &str) -> Option<(u32, u32)> {
    let mut decoder = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let (width, height) = decoder.dimensions();
    if width < 1 || height < 1 {
        return None;
    }
    let turned = matches!(
        decoder.orientation().ok()?,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    Some(if turned {
        (height, width)
    } else {
        (width, height)
    })
}
