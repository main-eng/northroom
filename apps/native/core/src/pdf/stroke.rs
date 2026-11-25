//! Turns a stroked outline into polygons to fill: the line dashed, then a rectangle each
//! segment, the pen's join where segments meet and its cap at open ends. Every polygon turns
//! the same way, so where they overlap they fill once.

/// A point in pixels.
pub type Point = (f64, f64);

/// How an open end of a line is finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

/// How two segments of a line meet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    Miter,
    Round,
    Bevel,
}

/// A line's look: its width, ends, corners and dashes, all in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Pen {
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    /// The longest a miter may be, as a share of the width, before it is bevelled.
    pub miter_limit: f64,
    /// Lengths of dash and gap in turn; empty for a solid line.
    pub dash: Vec<f64>,
    /// How far into the dash pattern the line starts.
    pub phase: f64,
}

impl Pen {
    /// PDF's starting pen: one unit wide, butt ends, mitred corners, solid.
    pub fn solid(width: f64) -> Self {
        Self {
            width,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 10.0,
            dash: Vec::new(),
            phase: 0.0,
        }
    }

    /// The cap a PDF names by number (`J`, `LC`).
    pub fn cap_of(number: i64) -> Cap {
        match number {
            1 => Cap::Round,
            2 => Cap::Square,
            _ => Cap::Butt,
        }
    }

    /// The join a PDF names by number (`j`, `LJ`).
    pub fn join_of(number: i64) -> Join {
        match number {
            1 => Join::Round,
            2 => Join::Bevel,
            _ => Join::Miter,
        }
    }

    /// The pen with its lengths scaled by `scale`, as from user space to pixels.
    pub fn scaled(&self, scale: f64) -> Self {
        Self {
            width: self.width * scale,
            dash: self.dash.iter().map(|length| length * scale).collect(),
            phase: self.phase * scale,
            ..self.clone()
        }
    }
}

/// The most dashes one line is cut into, so a tiny pattern can't run for ever.
const DASHES_MAX: usize = 200_000;

/// The polygons that stroke `lines` with `pen`.
pub fn outline(lines: &[Vec<Point>], pen: &Pen) -> Vec<Vec<Point>> {
    let half = pen.width / 2.0;
    let mut out = Vec::new();
    for line in lines {
        let closed = line.len() > 2 && line.first() == line.last();
        if pen.dash.iter().sum::<f64>() > 0.0 {
            for piece in dashes(line, &pen.dash, pen.phase) {
                stroke_piece(&piece, false, pen, half, &mut out);
            }
        } else {
            stroke_piece(line, closed, pen, half, &mut out);
        }
    }
    out.into_iter().map(turned_alike).collect()
}

fn distance(a: Point, b: Point) -> f64 {
    (b.0 - a.0).hypot(b.1 - a.1)
}

fn between(a: Point, b: Point, share: f64) -> Point {
    (a.0 + (b.0 - a.0) * share, a.1 + (b.1 - a.1) * share)
}

/// Where a dash pattern stands: which length, how much of it is left, and whether it draws.
struct Dashing<'a> {
    pattern: &'a [f64],
    index: usize,
    left: f64,
    on: bool,
}

impl<'a> Dashing<'a> {
    /// The pattern `phase` along from its start.
    fn new(pattern: &'a [f64], phase: f64) -> Self {
        let total: f64 =
            pattern.iter().sum::<f64>() * if pattern.len() % 2 == 1 { 2.0 } else { 1.0 };
        let mut dashing = Self {
            pattern,
            index: 0,
            left: pattern[0],
            on: true,
        };
        let mut skip = phase.rem_euclid(total);
        while skip > 0.0 && skip >= dashing.left {
            skip -= dashing.left;
            dashing.advance();
        }
        dashing.left -= skip;
        dashing
    }

    fn advance(&mut self) {
        self.index = (self.index + 1) % self.pattern.len();
        self.left = self.pattern[self.index].max(0.0);
        self.on = !self.on;
    }
}

/// The dashes of `line`: the stretches the pattern draws.
fn dashes(line: &[Point], pattern: &[f64], phase: f64) -> Vec<Vec<Point>> {
    let mut dashing = Dashing::new(pattern, phase);
    let mut pieces = Vec::new();
    let mut current: Vec<Point> = if dashing.on {
        line.iter().take(1).copied().collect()
    } else {
        Vec::new()
    };
    for pair in line.windows(2) {
        dashing.cut(pair[0], pair[1], &mut current, &mut pieces);
    }
    if dashing.on && !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

impl Dashing<'_> {
    /// Follows the segment `from` to `to`, ending a dash or starting one wherever the pattern
    /// turns within it.
    fn cut(
        &mut self,
        mut from: Point,
        to: Point,
        current: &mut Vec<Point>,
        pieces: &mut Vec<Vec<Point>>,
    ) {
        let mut length = distance(from, to);
        while length > self.left && pieces.len() < DASHES_MAX {
            let point = between(from, to, self.left / length);
            if self.on {
                current.push(point);
                pieces.push(std::mem::take(current));
            } else {
                *current = vec![point];
            }
            length -= self.left;
            from = point;
            self.advance();
        }
        self.left -= length;
        if self.on {
            current.push(to);
        }
    }
}

/// The unit direction from `a` to `b`, or `None` where they meet.
fn direction(a: Point, b: Point) -> Option<Point> {
    let length = distance(a, b);
    (length > 1e-9).then(|| ((b.0 - a.0) / length, (b.1 - a.1) / length))
}

/// The left normal of direction `d`, `half` long.
fn normal(d: Point, half: f64) -> Point {
    (-d.1 * half, d.0 * half)
}

/// Strokes one stretch of line: its segments, joins and, when open, caps.
fn stroke_piece(points: &[Point], closed: bool, pen: &Pen, half: f64, out: &mut Vec<Vec<Point>>) {
    let mut points: Vec<Point> = points.to_vec();
    points.dedup_by(|a, b| distance(*a, *b) < 1e-9);
    let Some(&first) = points.first() else {
        return;
    };
    if points.len() == 1 {
        // A dot: only a round or square cap shows it.
        if pen.cap == Cap::Round {
            out.push(disc(first, half));
        }
        return;
    }
    for pair in points.windows(2) {
        let d = direction(pair[0], pair[1]).unwrap_or((1.0, 0.0));
        let n = normal(d, half);
        out.push(vec![
            (pair[0].0 + n.0, pair[0].1 + n.1),
            (pair[1].0 + n.0, pair[1].1 + n.1),
            (pair[1].0 - n.0, pair[1].1 - n.1),
            (pair[0].0 - n.0, pair[0].1 - n.1),
        ]);
    }
    let directions: Vec<Point> = points
        .windows(2)
        .filter_map(|pair| direction(pair[0], pair[1]))
        .collect();
    for (index, pair) in directions.windows(2).enumerate() {
        out.extend(join(points[index + 1], pair[0], pair[1], pen, half));
    }
    match (closed, directions.first(), directions.last()) {
        (true, Some(&start), Some(&end)) => out.extend(join(first, end, start, pen, half)),
        (false, Some(&start), Some(&end)) => {
            let last = points[points.len() - 1];
            out.extend(cap(first, (-start.0, -start.1), pen, half));
            out.extend(cap(last, end, pen, half));
        }
        _ => {}
    }
}

/// The corner where a segment heading `d0` meets one heading `d1` at `point`.
fn join(point: Point, d0: Point, d1: Point, pen: &Pen, half: f64) -> Option<Vec<Point>> {
    let cross = d0.0 * d1.1 - d0.1 * d1.0;
    let dot = d0.0 * d1.0 + d0.1 * d1.1;
    if cross.abs() < 1e-9 && dot > 0.0 {
        return None;
    }
    if pen.join == Join::Round {
        return Some(disc(point, half));
    }
    // The corner's outer side: right of the line when it turns left, else left.
    let side = if cross > 0.0 { -1.0 } else { 1.0 };
    let (n0, n1) = (normal(d0, half * side), normal(d1, half * side));
    let a = (point.0 + n0.0, point.1 + n0.1);
    let b = (point.0 + n1.0, point.1 + n1.1);
    let sum = (n0.0 + n1.0, n0.1 + n1.1);
    let reach = sum.0 * sum.0 + sum.1 * sum.1;
    // The miter's length over the width is 1 / sin of half the angle between the segments.
    let ratio = 2.0 * half / reach.sqrt().max(1e-12);
    if pen.join == Join::Bevel || ratio > pen.miter_limit || reach < 1e-12 {
        return Some(vec![point, a, b]);
    }
    let stretch = 2.0 * half * half / reach;
    let tip = (point.0 + sum.0 * stretch, point.1 + sum.1 * stretch);
    Some(vec![point, a, tip, b])
}

/// The end of a line at `point`, heading out along `d`.
fn cap(point: Point, d: Point, pen: &Pen, half: f64) -> Option<Vec<Point>> {
    match pen.cap {
        Cap::Butt => None,
        Cap::Round => Some(disc(point, half)),
        Cap::Square => {
            let n = normal(d, half);
            let out = (point.0 + d.0 * half, point.1 + d.1 * half);
            Some(vec![
                (point.0 + n.0, point.1 + n.1),
                (out.0 + n.0, out.1 + n.1),
                (out.0 - n.0, out.1 - n.1),
                (point.0 - n.0, point.1 - n.1),
            ])
        }
    }
}

/// A disc `half` in radius about `center`, with enough sides to look round at its size.
fn disc(center: Point, half: f64) -> Vec<Point> {
    let sides = (half * 4.0).ceil().clamp(8.0, 64.0) as usize;
    (0..sides)
        .map(|step| {
            let angle = step as f64 / sides as f64 * std::f64::consts::TAU;
            (center.0 + half * angle.cos(), center.1 + half * angle.sin())
        })
        .collect()
}

/// `polygon`, reversed if needed so it turns the way every stroke polygon does.
fn turned_alike(mut polygon: Vec<Point>) -> Vec<Point> {
    let twice_area: f64 = polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1))
        .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
        .sum();
    if twice_area > 0.0 {
        polygon.reverse();
    }
    polygon
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(polygons: &[Vec<Point>]) -> [f64; 4] {
        let points = polygons.iter().flatten();
        [
            points.clone().map(|p| p.0).fold(f64::MAX, f64::min),
            points.clone().map(|p| p.1).fold(f64::MAX, f64::min),
            points.clone().map(|p| p.0).fold(f64::MIN, f64::max),
            points.map(|p| p.1).fold(f64::MIN, f64::max),
        ]
    }

    #[test]
    fn caps_end_a_line_flush_round_or_square() {
        let line = vec![vec![(10.0, 10.0), (30.0, 10.0)]];
        let mut pen = Pen::solid(4.0);
        assert_eq!(bounds(&outline(&line, &pen)), [10.0, 8.0, 30.0, 12.0]);
        pen.cap = Cap::Square;
        assert_eq!(bounds(&outline(&line, &pen)), [8.0, 8.0, 32.0, 12.0]);
        pen.cap = Cap::Round;
        let round = bounds(&outline(&line, &pen));
        assert!((round[0] - 8.0).abs() < 1e-9 && (round[2] - 32.0).abs() < 1e-9);
    }

    #[test]
    fn a_right_angle_mitres_to_its_corner_and_a_sharp_one_bevels() {
        let corner = vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]];
        let pen = Pen::solid(2.0);
        // The mitre fills out to the outer corner at (11, -1).
        assert_eq!(bounds(&outline(&corner, &pen)), [0.0, -1.0, 11.0, 10.0]);
        let sharp = vec![vec![(0.0, 0.0), (10.0, 0.0), (0.0, 0.5)]];
        let mitred = bounds(&outline(&sharp, &pen));
        assert!(mitred[2] < 12.0, "{mitred:?}");
    }

    #[test]
    fn a_dash_pattern_cuts_a_line_from_its_phase() {
        let line = [(0.0, 0.0), (10.0, 0.0)];
        let pieces = dashes(&line, &[2.0, 3.0], 0.0);
        let starts: Vec<f64> = pieces.iter().map(|piece| piece[0].0).collect();
        assert_eq!(starts, [0.0, 5.0]);
        assert_eq!(pieces[0].last(), Some(&(2.0, 0.0)));
        // A phase of 1 starts a unit into the first dash.
        let shifted = dashes(&line, &[2.0, 3.0], 1.0);
        assert_eq!(shifted[0], vec![(0.0, 0.0), (1.0, 0.0)]);
        // A pattern of one length dashes and gaps alike.
        assert_eq!(dashes(&line, &[5.0], 0.0).len(), 1);
    }

    #[test]
    fn every_polygon_turns_the_same_way() {
        let square = vec![vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]];
        let mut pen = Pen::solid(3.0);
        pen.join = Join::Round;
        for polygon in outline(&square, &pen) {
            let twice_area: f64 = polygon
                .iter()
                .zip(polygon.iter().cycle().skip(1))
                .map(|(a, b)| a.0 * b.1 - b.0 * a.1)
                .sum();
            assert!(twice_area <= 0.0);
        }
    }
}
