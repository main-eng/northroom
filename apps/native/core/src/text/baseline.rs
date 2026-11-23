//! Baseline placement within a line box: where a browser puts it, where a browser's media
//! controls put it, and where the open page puts it. Pure arithmetic on a font's ascent and
//! descent (in points, as the platform measures them) and the screen scale, so every shell
//! lands text on the same pixel.

/// Device pixels per point, rounded to a whole number of at least 1.
fn whole_scale(scale: f64) -> f64 {
    scale.abs().round().max(1.0)
}

/// The baseline a browser gives a line box `line_height` tall at `top`, in points from the view's
/// top, at `scale` device pixels per point. Chromium works in device pixels: it rounds ascent and
/// descent to whole pixels, splits the leftover space with the smaller half on top, and rounds
/// the text top to a whole pixel. `ascent` and `descent` may come from another font when the
/// text sits on that font's baseline.
pub fn browser(top: f64, line_height: f64, ascent: f64, descent: f64, scale: f64) -> f64 {
    let scale = whole_scale(scale);
    let (up, down) = ((ascent * scale).round(), (descent * scale).round());
    let lead = ((line_height * scale - up - down) / 2.0).floor();
    ((top * scale + lead + 0.5).floor() + up) / scale
}

/// Families whose ascent Chromium on the Mac grows by 15 % of the font height, to match their
/// classic line spacing.
const TALL_ASCENT_FAMILIES: [&str; 3] = ["Helvetica", "Times", "Courier"];

/// The baseline of a line in a browser's built-in media controls, for a font of `family`.
/// Chromium on the Mac rounds the descent up to a whole device pixel (14 px Helvetica at 2x
/// descends 6.44, counted as 7) and adds 15 % of the height to the ascent of
/// `TALL_ASCENT_FAMILIES`, so control text sits lower than the font metrics suggest.
pub fn controls(
    top: f64,
    line_height: f64,
    ascent: f64,
    descent: f64,
    family: &str,
    scale: f64,
) -> f64 {
    let scale = whole_scale(scale);
    let mut up = (ascent * scale).round();
    let mut down = (descent * scale).round();
    if down < descent * scale {
        down += 1.0;
    }
    if TALL_ASCENT_FAMILIES.contains(&family) {
        up += ((up + down) * 0.15 + 0.5).floor();
    }
    browser(top, line_height, up / scale, down / scale, scale)
}

/// The open page's baseline, in points below the top of a line `line_height` tall. The text
/// system centres the rounded font in the line, raising it by half the spare room (rounded up),
/// and the result subtracts that raise. Any raise beyond the rounded descent counts half.
pub fn page(line_height: f64, ascent: f64, descent: f64) -> f64 {
    let (up, down) = (ascent.round(), descent.round());
    let lift = ((line_height - up - down) / 2.0).ceil();
    let raised = (up + lift, (down - lift).max(0.0));
    ((line_height + raised.0 - raised.1 - 1.0) / 2.0).ceil() - lift
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inter at 15 points, as Core Text measures it.
    const INTER_15: (f64, f64) = (14.53125, 3.6181640625);
    /// Inter at 28 points, weight 600.
    const INTER_28: (f64, f64) = (27.125, 6.75390625);
    /// Inter at 9 points.
    const INTER_9: (f64, f64) = (8.71875, 2.1708984375);
    /// Geist Mono at 13 points.
    const MONO_13: (f64, f64) = (13.0650634765625, 3.8349761962890625);
    /// Helvetica at 14 points.
    const HELVETICA_14: (f64, f64) = (10.7802734375, 3.2197265625);

    #[test]
    fn a_browser_centres_the_rounded_face_with_the_smaller_half_on_top() {
        let (ascent, descent) = INTER_15;
        // 24 tall at one pixel a point: 15 up and 4 down leave 5, 2 above.
        assert_eq!(browser(0.0, 24.0, ascent, descent, 1.0), 17.0);
        assert_eq!(browser(10.0, 24.0, ascent, descent, 1.0), 27.0);
        // At 2x: 29 up and 7 down of 48 leave 12, 6 above, so 35 px.
        assert_eq!(browser(0.0, 24.0, ascent, descent, 2.0), 17.5);
        // The text's top is rounded to a whole pixel.
        assert_eq!(browser(0.3, 24.0, ascent, descent, 2.0), 18.0);
        // A flipped context reports a negative scale; it is never less than 1.
        assert_eq!(browser(0.0, 24.0, ascent, descent, -2.0), 17.5);
        assert_eq!(browser(0.0, 24.0, ascent, descent, 0.4), 17.0);
    }

    #[test]
    fn the_controls_count_a_descent_rounded_down_and_helveticas_taller_ascent() {
        let (ascent, descent) = HELVETICA_14;
        // At 2x: 22 up, 6.44 down counted as 7, plus 4 up. 26 and 7 of a 46 px row leave 13,
        // 6 above, so the baseline is 32 px down.
        assert_eq!(controls(0.0, 23.0, ascent, descent, "Helvetica", 2.0), 16.0);
        // Other families keep their ascent: 22 up of the same row leaves 17, 8 above.
        assert_eq!(controls(0.0, 23.0, ascent, descent, "Roboto", 2.0), 15.0);
        assert_eq!(
            controls(0.0, 23.0, ascent, descent, "Roboto", 2.0),
            browser(0.0, 23.0, 11.0, 3.5, 2.0)
        );
    }

    #[test]
    fn the_page_sets_its_lines_where_core_text_puts_a_raised_face() {
        // Each matches Core Text laying out "x" in a line of that height.
        type Case = ((f64, f64), [(f64, f64); 7]);
        let cases: [Case; 4] = [
            (
                INTER_15,
                [
                    (10.0, 10.0),
                    (13.5, 12.0),
                    (20.15, 16.0),
                    (24.0, 17.0),
                    (36.0, 21.0),
                    (38.4, 22.0),
                    (60.0, 27.0),
                ],
            ),
            (
                INTER_28,
                [
                    (10.0, 15.0),
                    (13.5, 17.0),
                    (20.15, 20.0),
                    (24.0, 22.0),
                    (36.0, 28.0),
                    (38.4, 29.0),
                    (60.0, 37.0),
                ],
            ),
            (
                MONO_13,
                [
                    (10.0, 9.0),
                    (13.5, 11.0),
                    (20.15, 15.0),
                    (24.0, 16.0),
                    (36.0, 19.0),
                    (38.4, 20.0),
                    (60.0, 25.0),
                ],
            ),
            (
                INTER_9,
                [
                    (10.0, 8.0),
                    (13.5, 10.0),
                    (20.15, 12.0),
                    (24.0, 13.0),
                    (36.0, 16.0),
                    (38.4, 17.0),
                    (60.0, 22.0),
                ],
            ),
        ];
        for ((ascent, descent), lines) in cases {
            for (line_height, baseline) in lines {
                assert_eq!(
                    page(line_height, ascent, descent),
                    baseline,
                    "{line_height} for {ascent}/{descent}"
                );
            }
        }
    }
}
