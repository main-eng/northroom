//! A folder card's art as shapes a shell fills: a back panel in the folder's colour, three
//! sheets fanned inside it, and a tinted front flap hinged at its bottom edge and tipped open,
//! seen in the art's perspective. The pose comes from `board::folder_pose`.

use crate::board::{FolderPose, Frame};

/// A colour in sRGB, 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

impl Rgba {
    const fn grey(level: u8) -> Self {
        let value = level as f64 / 255.0;
        Self {
            red: value,
            green: value,
            blue: value,
            alpha: 1.0,
        }
    }
}

/// One outline of a shape.
#[derive(Clone, Debug, PartialEq)]
pub enum Contour {
    /// A rectangle with its corners rounded `radius_x` across and `radius_y` down.
    Rounded {
        frame: Frame,
        radius_x: f64,
        radius_y: f64,
    },
    /// Points joined by straight lines and closed.
    Polygon(Vec<(f64, f64)>),
}

/// Outlines drawn through `transform`, filled together, then stroked `stroke_width` wide (in
/// transformed units) when `stroke` is set. `transform` is an affine `[a, b, c, d, tx, ty]`
/// that maps (x, y) to (a·x + c·y + tx, b·x + d·y + ty).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub transform: [f64; 6],
    pub contours: Vec<Contour>,
    pub fill: Rgba,
    pub stroke: Option<Rgba>,
    pub stroke_width: f64,
}

const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// The flap's outline on its 321×241 drawing grid.
const FLAP_PATH: &str = "M0 25C0 11.1929 11.1929 0 25 0H136.084C143.044 0 149.689 2.90139 154.42 8.00608L178.08 33.5343C182.811 38.639 189.456 41.5404 196.416 41.5404H296C309.807 41.5404 321 52.7333 321 66.5404V216C321 229.807 309.807 241 296 241H25C11.1929 241 0 229.807 0 216V25Z";
/// Straight segments per curve of the flap, so each point can be tipped into perspective.
const CURVE_STEPS: u32 = 12;

/// The points of a path written with absolute M, L, C, H, V and Z commands, its curves cut
/// into straight lines. Closing adds no point.
fn outline(path: &str) -> Vec<(f64, f64)> {
    let mut points = Vec::new();
    let mut current = (0.0, 0.0);
    let mut command = None;
    let mut numbers = Vec::new();
    for token in path.split_inclusive(|character: char| character.is_ascii_alphabetic()) {
        // Each piece is the numbers of the command before it, then the next command's letter.
        let (values, letter) = numbers_then_letter(token);
        numbers.extend(
            values
                .split([' ', ','])
                .filter_map(|value| value.parse::<f64>().ok()),
        );
        let Some(letter) = letter else {
            continue;
        };
        if let Some(command) = command {
            add_command_points(command, &numbers, &mut current, &mut points);
        }
        numbers.clear();
        command = Some(letter);
    }
    if let Some(command) = command {
        add_command_points(command, &numbers, &mut current, &mut points);
    }
    points
}

/// A piece of a path split after each letter: the numbers in it, and the letter it ends with.
fn numbers_then_letter(token: &str) -> (&str, Option<char>) {
    match token.chars().last() {
        Some(last) if last.is_ascii_alphabetic() => (&token[..token.len() - 1], Some(last)),
        _ => (token, None),
    }
}

/// Adds the points path command `command` draws with `numbers` from `current`, and moves
/// `current` to where it ends.
fn add_command_points(
    command: char,
    numbers: &[f64],
    current: &mut (f64, f64),
    points: &mut Vec<(f64, f64)>,
) {
    match command {
        'M' | 'L' if numbers.len() >= 2 => {
            *current = (numbers[0], numbers[1]);
            points.push(*current);
        }
        'C' => {
            for &[x1, y1, x2, y2, x, y] in numbers.as_chunks::<6>().0 {
                let end = (x, y);
                add_curve_points([*current, (x1, y1), (x2, y2), end], points);
                *current = end;
            }
        }
        'H' if !numbers.is_empty() => {
            current.0 = numbers[0];
            points.push(*current);
        }
        'V' if !numbers.is_empty() => {
            current.1 = numbers[0];
            points.push(*current);
        }
        _ => {}
    }
}

/// Adds the points of the cubic curve through `start`, `first`, `second` and `end`, cut into
/// `CURVE_STEPS` straight lines (its start is already in).
fn add_curve_points([start, first, second, end]: [(f64, f64); 4], points: &mut Vec<(f64, f64)>) {
    for step in 1..=CURVE_STEPS {
        let t = f64::from(step) / f64::from(CURVE_STEPS);
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        points.push((
            a * start.0 + b * first.0 + c * second.0 + d * end.0,
            a * start.1 + b * first.1 + c * second.1 + d * end.1,
        ));
    }
}

/// Concatenates two transforms the way a graphics context does: points go through `second`,
/// then `first`.
fn then(first: [f64; 6], second: [f64; 6]) -> [f64; 6] {
    let [a, b, c, d, tx, ty] = first;
    let [e, f, g, h, ux, uy] = second;
    [
        e * a + f * c,
        e * b + f * d,
        g * a + h * c,
        g * b + h * d,
        ux * a + uy * c + tx,
        ux * b + uy * d + ty,
    ]
}

fn translation(x: f64, y: f64) -> [f64; 6] {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

/// The paper's shades: its face, its edge and the lines written on it.
const PAPER: Rgba = Rgba::grey(0xf1);
const PAPER_EDGE: Rgba = Rgba::grey(0xe0);
const PAPER_LINES: Rgba = Rgba::grey(0xd4);

/// The art filling `rect` (a 321:270 box, y down) in the folder's `color`, posed as `pose`.
pub fn folder_art(rect: Frame, color: Rgba, pose: &FolderPose) -> Vec<Shape> {
    let (width, height) = (rect.width, rect.height);
    let (mid_x, mid_y) = (rect.x + width * 0.5, rect.y + height * 0.5);
    let mut shapes = vec![Shape {
        transform: IDENTITY,
        contours: vec![Contour::Rounded {
            frame: rect,
            radius_x: width * 0.078,
            radius_y: height * 0.093,
        }],
        fill: color,
        stroke: None,
        stroke_width: 0.0,
    }];

    // Sheets: 164×214 on the 321×270 grid, about the centre, each with its own fan.
    let (sheet_width, sheet_height) = (width * 0.511, height * 0.793);
    for sheet in pose.sheets {
        let scale = sheet_width / 164.0;
        let (sine, cosine) = (sheet.turn * std::f64::consts::PI / 180.0).sin_cos();
        let mut transform = translation(
            mid_x + sheet.x / 100.0 * sheet_width,
            mid_y + sheet.y / 100.0 * sheet_height,
        );
        transform = then(transform, [cosine, sine, -sine, cosine, 0.0, 0.0]);
        transform = then(transform, [scale, 0.0, 0.0, scale, 0.0, 0.0]);
        transform = then(transform, translation(-82.0, -107.0));
        let rounded = |x: f64, y: f64, width: f64, height: f64, radius: f64| Contour::Rounded {
            frame: Frame {
                x,
                y,
                width,
                height,
            },
            radius_x: radius,
            radius_y: radius,
        };
        shapes.push(Shape {
            transform,
            contours: vec![rounded(0.5, 0.5, 162.0, 212.0, 19.5)],
            fill: PAPER,
            stroke: Some(PAPER_EDGE),
            stroke_width: 1.0,
        });
        // The heading's bar and two columns of nine lines.
        let mut lines = vec![rounded(14.0, 31.0, 135.0, 12.0, 6.0)];
        for row in 0..9 {
            for left in [15.0, 84.5] {
                lines.push(rounded(left, 61.0 + f64::from(row) * 14.12, 64.5, 5.9, 2.9));
            }
        }
        shapes.push(Shape {
            transform,
            contours: lines,
            fill: PAPER_LINES,
            stroke: None,
            stroke_width: 0.0,
        });
    }

    // The flap: 321×241, sitting low in the back panel, turned about its bottom edge so its top
    // comes towards the eye, which is 260 card units (2.5 art widths) in front of the art's
    // centre. Nearer points draw larger and further from that centre.
    let flap_height = height * 0.893;
    let hinge = rect.y + height * 0.113 + flap_height;
    let angle = pose.flap * std::f64::consts::PI / 180.0;
    let eye = width * 2.5;
    let seen = outline(FLAP_PATH)
        .into_iter()
        .map(|(x, y)| {
            let up = (1.0 - y / 241.0) * flap_height;
            let near = eye / (eye - up * angle.sin());
            (
                mid_x + (rect.x + x / 321.0 * width - mid_x) * near,
                mid_y + (hinge - up * angle.cos() - mid_y) * near,
            )
        })
        .collect();
    shapes.push(Shape {
        transform: IDENTITY,
        contours: vec![Contour::Polygon(seen)],
        // 82% of the colour toward black, at 45%; edged with 60% of it toward white, one pixel
        // wide whatever the folder's size.
        fill: Rgba {
            red: color.red * 0.82,
            green: color.green * 0.82,
            blue: color.blue * 0.82,
            alpha: 0.45,
        },
        stroke: Some(Rgba {
            red: color.red * 0.6 + 0.4,
            green: color.green * 0.6 + 0.4,
            blue: color.blue * 0.6 + 0.4,
            alpha: 1.0,
        }),
        stroke_width: 1.0,
    });
    shapes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::FolderTarget;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_flap_outline_is_its_corners_and_lines_with_curves_cut_in_twelve() {
        let points = outline(FLAP_PATH);
        // M, then 6 curves of 12 points and 6 lines (H, L, H, V, H, V).
        assert_eq!(points.len(), 1 + 6 * 12 + 6);
        assert_eq!(points[0], (0.0, 25.0));
        assert_eq!(
            points[12],
            (25.0, 0.0),
            "the first curve ends at its end point"
        );
        assert_eq!(points[13], (136.084, 0.0));
        assert_eq!(points[26], (178.08, 33.5343), "the tab's slope is a line");
        assert_eq!(*points.last().expect("points"), (0.0, 25.0));
        let corner = points[6];
        assert!(corner.0 > 0.0 && corner.0 < 25.0 && corner.1 > 0.0 && corner.1 < 25.0);
    }

    #[test]
    fn a_path_reads_lines_and_several_curves_in_one_command() {
        assert_eq!(
            outline("M1 2L3 4H5V6Z"),
            [(1.0, 2.0), (3.0, 4.0), (5.0, 4.0), (5.0, 6.0)]
        );
        assert_eq!(outline("M0 0C0 0 1 1 1 1 1 1 2 2 2 2").len(), 1 + 2 * 12);
    }

    #[test]
    fn transforms_apply_the_later_one_first() {
        let moved_then_doubled = then(translation(10.0, 0.0), [2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
        // (1, 1) doubles to (2, 2), then moves to (12, 2).
        let [a, b, c, d, tx, ty] = moved_then_doubled;
        assert_eq!(
            (a * 1.0 + c * 1.0 + tx, b * 1.0 + d * 1.0 + ty),
            (12.0, 2.0)
        );
    }

    #[test]
    fn the_art_is_its_panel_six_sheet_shapes_and_the_flap() {
        let rect = Frame {
            x: 10.0,
            y: 20.0,
            width: 321.0,
            height: 270.0,
        };
        let color = Rgba {
            red: 0.5,
            green: 0.25,
            blue: 1.0,
            alpha: 1.0,
        };
        let shapes = folder_art(rect, color, &FolderTarget::Rest.pose());
        assert_eq!(shapes.len(), 1 + 3 * 2 + 1);
        assert_eq!(
            shapes[0].contours,
            [Contour::Rounded {
                frame: rect,
                radius_x: 321.0 * 0.078,
                radius_y: 270.0 * 0.093
            }]
        );
        assert_eq!(shapes[0].fill, color);
        assert_eq!(shapes[1].fill, Rgba::grey(0xf1));
        assert_eq!(
            (shapes[1].stroke, shapes[1].stroke_width),
            (Some(Rgba::grey(0xe0)), 1.0)
        );
        assert_eq!(shapes[2].contours.len(), 19);
        let flap = &shapes[7];
        assert_eq!(
            flap.fill,
            Rgba {
                red: 0.5 * 0.82,
                green: 0.25 * 0.82,
                blue: 0.82,
                alpha: 0.45
            }
        );
        assert_eq!(
            flap.stroke,
            Some(Rgba {
                red: 0.5 * 0.6 + 0.4,
                green: 0.25 * 0.6 + 0.4,
                blue: 1.0,
                alpha: 1.0
            })
        );
    }

    #[test]
    fn a_sheet_sits_about_the_centre_by_its_pose() {
        let rect = Frame {
            x: 0.0,
            y: 0.0,
            width: 321.0,
            height: 270.0,
        };
        let mut pose = FolderTarget::Rest.pose();
        pose.sheets[0] = crate::board::SheetPose {
            x: 0.0,
            y: 0.0,
            turn: 90.0,
        };
        let shapes = folder_art(rect, Rgba::grey(0), &pose);
        let [a, b, c, d, tx, ty] = shapes[1].transform;
        let scale = 321.0 * 0.511 / 164.0;
        // The sheet's own centre (82, 107) lands on the art's centre, its x turned onto y.
        assert!(near(a * 82.0 + c * 107.0 + tx, 160.5) && near(b * 82.0 + d * 107.0 + ty, 135.0));
        assert!(near(a, 0.0) && near(b, scale) && near(c, -scale) && near(d, 0.0));
    }

    #[test]
    fn the_flap_tips_towards_the_eye_about_its_bottom_edge() {
        let rect = Frame {
            x: 0.0,
            y: 0.0,
            width: 321.0,
            height: 270.0,
        };
        let flat = FolderPose {
            flap: 0.0,
            ..FolderTarget::Rest.pose()
        };
        let shapes = folder_art(rect, Rgba::grey(0), &flat);
        let Contour::Polygon(points) = &shapes[7].contours[0] else {
            panic!("the flap")
        };
        // Untipped, the flap is drawn on its grid, 0.893 of the art tall with its bottom on the
        // hinge, 0.113 of the art down plus the flap's height.
        let (flap, hinge) = (270.0 * 0.893, 270.0 * 0.113 + 270.0 * 0.893);
        assert!(near(points[0].0, 0.0) && near(points[0].1, hinge - (1.0 - 25.0 / 241.0) * flap));
        let tipped = folder_art(rect, Rgba::grey(0), &FolderTarget::Hover.pose());
        let Contour::Polygon(seen) = &tipped[7].contours[0] else {
            panic!("the flap")
        };
        let bottom = seen.iter().map(|point| point.1).fold(f64::MIN, f64::max);
        assert!(
            near(bottom, 270.0 * 0.113 + 270.0 * 0.893),
            "the hinge stays where it is"
        );
        assert!(
            seen[0].0 < 0.0,
            "the near top corner draws wider than the art"
        );
    }
}
