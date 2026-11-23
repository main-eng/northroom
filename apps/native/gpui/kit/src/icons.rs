//! The core's glyphs (`glyph::Glyph`), drawn from their SVG (`Glyph::svg`) as masks in one colour.

use gpui::{App, Bounds, Hsla, Pixels, SharedString, TransformationMatrix, Window};
use std::cell::RefCell;
use std::collections::HashMap;

pub use northroom_core::glyph::Glyph;

thread_local! {
    static DRAWINGS: RefCell<HashMap<Glyph, (SharedString, Vec<u8>)>> = RefCell::new(HashMap::new());
}

/// The glyph the core names in a view; `None` for `""`.
pub fn named(name: &str) -> Option<Glyph> {
    (!name.is_empty())
        .then(|| Glyph::named(name).unwrap_or_else(|| panic!("the core named no glyph {name}")))
}

/// The glyph's SVG and its cache key, made once.
pub fn drawing(glyph: Glyph) -> (SharedString, Vec<u8>) {
    if let Some(kept) = DRAWINGS.with(|drawings| drawings.borrow().get(&glyph).cloned()) {
        return kept;
    }
    let kept = (
        SharedString::from(format!("glyph/{}.svg", glyph.name())),
        glyph.svg().into_bytes(),
    );
    DRAWINGS.with(|drawings| drawings.borrow_mut().insert(glyph, kept.clone()));
    kept
}

/// Draws `glyph` filling `bounds` in `color`.
pub fn draw(window: &mut Window, cx: &App, glyph: Glyph, bounds: Bounds<Pixels>, color: Hsla) {
    if crate::core::is_empty(&bounds) {
        return;
    }
    let (key, svg) = drawing(glyph);
    let _ = window.paint_svg(
        bounds,
        key,
        Some(&svg),
        TransformationMatrix::unit(),
        color,
        cx,
    );
}
