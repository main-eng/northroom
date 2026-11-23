//! The `/` menu under the edited line, on the popover background with a 1 px border, 6 px
//! corners and a shadow. Each block's glyph sits in a bordered tile, and the row highlights
//! under keyboard focus or the pointer. The core decides which blocks match, which is
//! highlighted, the layout and the scroll (`page_slash`); keyboard focus stays in the line.

use gpui::{App, Bounds, BoxShadow, Corners, Pixels, Point, Window, point, px};
use kit::controls::{Drawn, Frame};
use kit::core::inset;
use kit::draw::{rounded, stroke};
use kit::icons;
use kit::nr::*;
use kit::theme::{Token, color};
use northroom_core::menus::slash::{Layout, Row};

/// The open `/` menu from the core (`page_slash`): the block it hangs under, and the menu.
pub fn read() -> Option<(i64, Layout)> {
    page_slash().map(|(block, menu)| (block as i64, menu))
}

/// The row of `menu` under `at`, in menu coordinates.
pub fn row_at(menu: &Layout, at: Point<Pixels>) -> Option<&Row> {
    menu.rows
        .iter()
        .find(|row| kit::core::contains(&row.frame.bounds(), at))
}

/// Draws `menu` with its top-left at `origin`.
pub fn paint(menu: &Layout, window: &mut Window, cx: &mut App, origin: Point<Pixels>) {
    if menu.empty {
        return;
    }
    let bounds = Bounds {
        origin,
        size: gpui::size(px(menu.width as f32), px(menu.height as f32)),
    };
    // The larger layer of `shadow-md`.
    window.paint_drop_shadows(
        bounds,
        Corners::all(px(6.0)),
        &[BoxShadow {
            color: gpui::black().opacity(0.1),
            offset: point(px(0.0), px(4.0)),
            blur_radius: px(6.0),
            spread_radius: px(-1.0),
            inset: false,
        }],
    );
    rounded(window, inset(bounds, 0.5, 0.5), color(Token::Popover), 5.5);
    stroke(
        window,
        inset(bounds, 0.5, 0.5),
        color(Token::Border),
        5.5,
        1.0,
    );
    window.with_content_mask(
        Some(gpui::ContentMask {
            bounds: inset(bounds, 1.0, 1.0),
        }),
        |window| {
            menu.heading.draw(window, cx, origin, None);
            for row in &menu.rows {
                if row.lit {
                    rounded(
                        window,
                        row.frame.at(origin),
                        color(Token::Foreground).alpha(0.08),
                        6.0,
                    );
                }
                let tile = inset(row.tile.at(origin), 0.5, 0.5);
                rounded(window, tile, color(Token::Background), 5.5);
                stroke(window, tile, color(Token::Border), 5.5, 1.0);
                icons::draw(
                    window,
                    cx,
                    row.icon,
                    row.glyph.at(origin),
                    color(Token::Foreground),
                );
                if let Some(shortcut) = &row.shortcut {
                    shortcut.draw(window, cx, origin, None);
                }
                row.label.draw(window, cx, origin, None);
            }
        },
    );
}
