//! Renders SVG files with resvg, which GPUI also uses for its icons. Image I/O, the Windows
//! Imaging Component and the `image` crate don't read SVG. A vector has no pixel size, so it is
//! drawn at the size asked for.

use crate::platform::Pixels;
use resvg::{tiny_skia, usvg};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// The largest SVG file drawn; a larger one shows its kind's glyph.
const SVG_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// The system faces SVG text is set in, loaded by the first SVG drawn.
static SYSTEM_FACES: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();

/// Whether `path` names an SVG, by its extension.
pub fn is_svg(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
}

/// Draws the SVG at `path` with `pixels` on its long side, in the window's colour space with
/// premultiplied alpha, as the platform readers return pictures. `None` if it can't be read or
/// drawn.
pub fn svg_picture(path: &str, pixels: u32) -> Option<Pixels> {
    if std::fs::metadata(path).ok()?.len() > SVG_MAX_BYTES {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let options = usvg::Options {
        // Resolve relative image links beside the SVG.
        resources_dir: Path::new(path).parent().map(Path::to_path_buf),
        fontdb: system_faces(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(&data, &options).ok()?;
    let size = tree.size();
    let scale = pixels as f32 / size.width().max(size.height());
    let width = (size.width() * scale).round().max(1.0) as u32;
    let height = (size.height() * scale).round().max(1.0) as u32;
    let mut canvas = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut canvas.as_mut(),
    );
    // tiny-skia gives premultiplied RGBA; GPUI wants BGRA.
    let mut bgra = canvas.take();
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    crate::display::convert(&mut bgra, width, height, 4, [2, 1, 0]);
    Some(Pixels {
        width,
        height,
        bgra,
    })
}

fn system_faces() -> Arc<usvg::fontdb::Database> {
    SYSTEM_FACES
        .get_or_init(|| {
            let mut faces = usvg::fontdb::Database::new();
            faces.load_system_fonts();
            Arc::new(faces)
        })
        .clone()
}
