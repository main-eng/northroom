//! What the board's view holds beyond its cards: the room left under the last row, how a
//! wheel that turns in clicks counts its travel for the zoom, and where the zoom's label rests.

use crate::board::{Frame, WHEEL_DELTA_LINE, WHEEL_DELTA_PAGE, WHEEL_DELTA_PIXEL};
use crate::scroll::route::{wheel_points, WHEEL_CLICKS};

/// Room under the last row.
pub const ROOM_UNDER_CARDS: f64 = 208.0;

/// Converts a wheel's travel `delta_y` in `unit` for the board's zoom. Clicks
/// (`scroll::route::WHEEL_CLICKS`) become points, as a web page receives them; lines and pages
/// keep their unit for `board::wheel_pixel_delta`.
pub fn wheel_travel(delta_y: f64, unit: u32) -> (f64, u32) {
    match unit {
        WHEEL_CLICKS => (wheel_points(delta_y, unit), WHEEL_DELTA_PIXEL),
        WHEEL_DELTA_LINE | WHEEL_DELTA_PAGE => (delta_y, unit),
        _ => (delta_y, WHEEL_DELTA_PIXEL),
    }
}

/// The board's scrollable height: the cards plus the room under them, never less than
/// `visible_height`.
pub fn canvas_height(cards_height: f64, visible_height: f64) -> f64 {
    visible_height.max(cards_height + ROOM_UNDER_CARDS)
}

/// Room either side of the zoom label's words, its 1-point border included.
const ZOOM_HUD_SIDE: f64 = 11.0;
/// The zoom label's height: 6 points above and below a 16-point line, inside a 1-point border.
const ZOOM_HUD_HEIGHT: f64 = 30.0;
/// How far below the board's top the zoom label rests.
const ZOOM_HUD_TOP: f64 = 12.0;

/// Where the zoom label rests over a board view `width` wide: centred, on whole points, 12 pt
/// down. It slides down to here as it appears (`motion::ZOOM_HUD_RISE`).
pub fn zoom_hud_rest(words_width: f64, width: f64) -> Frame {
    let hud_width = words_width.ceil() + 2.0 * ZOOM_HUD_SIDE;
    Frame {
        x: ((width - hud_width) / 2.0).round(),
        y: ZOOM_HUD_TOP,
        width: hud_width,
        height: ZOOM_HUD_HEIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_of_the_wheel_travels_forty_pixels() {
        assert_eq!(wheel_travel(-2.0, WHEEL_CLICKS), (-80.0, WHEEL_DELTA_PIXEL));
        assert_eq!(
            wheel_travel(3.5, WHEEL_DELTA_PIXEL),
            (3.5, WHEEL_DELTA_PIXEL)
        );
        assert_eq!(wheel_travel(1.0, WHEEL_DELTA_LINE), (1.0, WHEEL_DELTA_LINE));
    }

    #[test]
    fn the_zoom_label_rests_centred_twelve_points_down() {
        // The words' width is taken up to the next whole point.
        assert_eq!(
            zoom_hud_rest(42.0, 1280.0),
            Frame {
                x: 608.0,
                y: 12.0,
                width: 64.0,
                height: 30.0
            }
        );
        assert_eq!(
            zoom_hud_rest(41.2, 1280.0),
            Frame {
                x: 608.0,
                y: 12.0,
                width: 64.0,
                height: 30.0
            }
        );
        // A half point over is rounded to the right.
        assert_eq!(
            zoom_hud_rest(40.2, 1280.0),
            Frame {
                x: 609.0,
                y: 12.0,
                width: 63.0,
                height: 30.0
            }
        );
        assert_eq!(
            zoom_hud_rest(0.0, 100.0),
            Frame {
                x: 39.0,
                y: 12.0,
                width: 22.0,
                height: 30.0
            }
        );
    }

    #[test]
    fn the_board_leaves_room_under_its_cards_and_fills_its_view() {
        assert_eq!(canvas_height(1000.0, 700.0), 1208.0);
        assert_eq!(canvas_height(300.0, 700.0), 700.0);
    }
}
