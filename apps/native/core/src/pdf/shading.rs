//! Shadings, the colours a gradient or mesh paints at each point (ISO 32000 8.7.4.5): function
//! based (type 1), axial (2), radial between two circles (3), free-form and lattice triangle
//! meshes (4, 5), and Coons and tensor-product patch meshes (6, 7). Patches are cut into
//! triangles, as pdf.js does, and a mesh's triangles are found through a grid of cells.
//!
//! Left out: a shading's `Background` and `BBox`.

use super::colour::{Function, Space};
use super::object::{Dict, Document, Object};
use super::raster::Rgba;
use super::render::{apply, invert, Matrix};

/// Something that paints a colour at each device point.
pub trait Paint {
    /// The colour at `point`, in device pixels, or `None` where it paints nothing.
    fn colour_at(&self, point: (f64, f64)) -> Option<Rgba>;
}

/// How many colours a gradient's function is sampled at.
const STEPS: usize = 256;
/// How many rows and columns of cells a mesh's triangles are sorted into.
const CELLS: usize = 64;
/// How many triangles a mesh may have, so a damaged one can't ask for all memory.
const TRIANGLES_MAX: usize = 2_000_000;
/// How finely a patch is cut: this many steps along each side.
const PATCH_STEPS: usize = 12;

type Rgb = [f32; 3];
type Point = (f64, f64);

/// A shading placed on the page.
pub struct Shading {
    /// From device pixels to the shading's own space.
    inverse: Matrix,
    alpha: f32,
    kind: Kind,
}

enum Kind {
    Axial {
        from: Point,
        to: Point,
        ramp: Ramp,
    },
    Radial {
        c0: Point,
        r0: f64,
        c1: Point,
        r1: f64,
        ramp: Ramp,
    },
    Function {
        to_domain: Matrix,
        domain: [f64; 4],
        function: Function,
        space: Space,
    },
    Mesh(Mesh),
}

/// A gradient's colours along its parameter, and whether it continues past each end.
struct Ramp {
    colours: Vec<Rgb>,
    extend: [bool; 2],
}

impl Ramp {
    /// The colour at `t`, from 0 to 1; `None` past an end that doesn't extend.
    fn at(&self, t: f64) -> Option<Rgb> {
        if (t < 0.0 && !self.extend[0]) || (t > 1.0 && !self.extend[1]) {
            return None;
        }
        let index = (t.clamp(0.0, 1.0) * (self.colours.len() - 1) as f64).round() as usize;
        self.colours.get(index).copied()
    }
}

fn numbers(document: &Document, dict: &Dict, key: &str) -> Vec<f64> {
    document
        .entry(dict, key)
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| document.resolve(item).as_f64())
                .collect()
        })
        .unwrap_or_default()
}

/// The colours a one-input function gives over `domain`, through `space`.
fn sampled(function: &Function, space: &Space, domain: (f64, f64)) -> Vec<Rgb> {
    (0..STEPS)
        .map(|step| {
            let t = domain.0 + (domain.1 - domain.0) * step as f64 / (STEPS - 1) as f64;
            space.rgb(&function.evaluate(&[t as f32]))
        })
        .collect()
}

impl Shading {
    /// Reads the shading `object`, placed by `to_device` (its space to device pixels), painted
    /// at `alpha`. `resources` name its colour space when it is named.
    pub fn read(
        document: &Document,
        object: &Object,
        to_device: Matrix,
        alpha: f32,
        resources: Option<&Dict>,
    ) -> Option<Self> {
        let resolved = document.resolve(object);
        let dict = resolved.as_dict()?;
        let space = super::image::named_space(document, dict.get("ColorSpace")?, resources)?;
        let function = dict
            .get("Function")
            .and_then(|function| Function::read(document, function, 0));
        let kind = match document.entry(dict, "ShadingType").as_i64()? {
            1 => function_based(document, dict, function?, space)?,
            kind @ (2 | 3) => gradient(document, dict, kind, &function?, &space)?,
            kind @ 4..=7 => {
                let Object::Stream(stream) = &resolved else {
                    return None;
                };
                let data = document.stream_data(stream).ok()?;
                Kind::Mesh(Mesh::read(document, dict, kind, &data, space, function)?)
            }
            _ => return None,
        };
        Some(Self {
            inverse: invert(to_device)?,
            alpha,
            kind,
        })
    }
}

/// Whether `function` of `inputs` inputs gives a value for each of `space`'s components; a
/// shading whose function doesn't is broken and paints nothing.
fn gives_colours(function: &Function, space: &Space, inputs: usize) -> Option<()> {
    (function.evaluate(&vec![0.0; inputs]).len() >= space.components()).then_some(())
}

/// A type 1 shading: a two-input function over a rectangle of its domain.
fn function_based(
    document: &Document,
    dict: &Dict,
    function: Function,
    space: Space,
) -> Option<Kind> {
    gives_colours(&function, &space, 2)?;
    let domain = numbers(document, dict, "Domain");
    let domain: [f64; 4] = if domain.len() == 4 {
        domain.try_into().ok()?
    } else {
        [0.0, 1.0, 0.0, 1.0]
    };
    let matrix: Matrix = numbers(document, dict, "Matrix")
        .try_into()
        .unwrap_or(super::render::IDENTITY);
    Some(Kind::Function {
        to_domain: invert(matrix)?,
        domain,
        function,
        space,
    })
}

/// A type 2 or 3 shading: colours along a line, or between two circles.
fn gradient(
    document: &Document,
    dict: &Dict,
    kind: i64,
    function: &Function,
    space: &Space,
) -> Option<Kind> {
    gives_colours(function, space, 1)?;
    let coords = numbers(document, dict, "Coords");
    let domain = numbers(document, dict, "Domain");
    let domain = (
        domain.first().copied().unwrap_or(0.0),
        domain.get(1).copied().unwrap_or(1.0),
    );
    let extend: Vec<bool> = document
        .entry(dict, "Extend")
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| matches!(item, Object::Bool(true)))
                .collect()
        })
        .unwrap_or_default();
    let ramp = Ramp {
        colours: sampled(function, space, domain),
        extend: [
            extend.first().copied().unwrap_or(false),
            extend.get(1).copied().unwrap_or(false),
        ],
    };
    Some(match (kind, coords.as_slice()) {
        (2, [x0, y0, x1, y1, ..]) => Kind::Axial {
            from: (*x0, *y0),
            to: (*x1, *y1),
            ramp,
        },
        (3, [x0, y0, r0, x1, y1, r1, ..]) => Kind::Radial {
            c0: (*x0, *y0),
            r0: *r0,
            c1: (*x1, *y1),
            r1: *r1,
            ramp,
        },
        _ => return None,
    })
}

/// Where a point lies between two circles: the largest `t` whose circle passes through it with
/// a radius of at least 0, within the ends the gradient extends past.
fn radial_t(
    point: Point,
    c0: Point,
    r0: f64,
    c1: Point,
    r1: f64,
    extend: [bool; 2],
) -> Option<f64> {
    let (cd, dr) = ((c1.0 - c0.0, c1.1 - c0.1), r1 - r0);
    let pd = (point.0 - c0.0, point.1 - c0.1);
    let a = cd.0 * cd.0 + cd.1 * cd.1 - dr * dr;
    let b = pd.0 * cd.0 + pd.1 * cd.1 + r0 * dr;
    let c = pd.0 * pd.0 + pd.1 * pd.1 - r0 * r0;
    let roots: Vec<f64> = if a.abs() < 1e-9 {
        if b.abs() < 1e-12 {
            return None;
        }
        vec![c / (2.0 * b)]
    } else {
        let discriminant = b * b - a * c;
        if discriminant < 0.0 {
            return None;
        }
        let root = discriminant.sqrt();
        let (t1, t2) = ((b + root) / a, (b - root) / a);
        vec![t1.max(t2), t1.min(t2)]
    };
    roots
        .into_iter()
        .find(|&t| r0 + t * dr >= 0.0 && (t >= 0.0 || extend[0]) && (t <= 1.0 || extend[1]))
}

impl Paint for Shading {
    fn colour_at(&self, point: Point) -> Option<Rgba> {
        let point = apply(self.inverse, point);
        let rgb = match &self.kind {
            Kind::Axial { from, to, ramp } => {
                let (dx, dy) = (to.0 - from.0, to.1 - from.1);
                let length = dx * dx + dy * dy;
                if length <= 0.0 {
                    return None;
                }
                ramp.at(((point.0 - from.0) * dx + (point.1 - from.1) * dy) / length)?
            }
            Kind::Radial {
                c0,
                r0,
                c1,
                r1,
                ramp,
            } => ramp.at(radial_t(point, *c0, *r0, *c1, *r1, ramp.extend)?)?,
            Kind::Function {
                to_domain,
                domain,
                function,
                space,
            } => {
                let (x, y) = apply(*to_domain, point);
                if x < domain[0] || x > domain[1] || y < domain[2] || y > domain[3] {
                    return None;
                }
                space.rgb(&function.evaluate(&[x as f32, y as f32]))
            }
            Kind::Mesh(mesh) => mesh.colour_at(point)?,
        };
        Some([rgb[0], rgb[1], rgb[2], self.alpha])
    }
}

// --- Meshes ---------------------------------------------------------------------------------

/// A triangle's corners and the colour values at each: RGB, or the function's input in the
/// first place when the mesh has a function.
#[derive(Clone, Copy)]
struct Triangle {
    points: [Point; 3],
    values: [Rgb; 3],
}

struct Mesh {
    triangles: Vec<Triangle>,
    /// The colours of the function's input over its range, when the mesh has one.
    ramp: Option<Vec<Rgb>>,
    /// The range of the function's input, from `Decode`.
    range: (f64, f64),
    /// The triangles' bounds: left, top, cell width, cell height.
    bounds: [f64; 4],
    /// Each cell's triangles, by index; later ones paint over earlier.
    cells: Vec<Vec<u32>>,
}

/// Reads packed values of a given number of bits each.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn read(&mut self, count: usize) -> Option<u64> {
        if count == 0 || count > 32 || self.at + count > self.data.len() * 8 {
            return None;
        }
        let mut value = 0u64;
        for _ in 0..count {
            let bit = (self.data[self.at / 8] >> (7 - self.at % 8)) & 1;
            value = value << 1 | u64::from(bit);
            self.at += 1;
        }
        Some(value)
    }

    fn align(&mut self) {
        self.at = self.at.div_ceil(8) * 8;
    }

    fn has_data(&self) -> bool {
        self.at + 8 <= self.data.len() * 8
    }
}

/// How a mesh's vertices are packed: bits per flag, coordinate and component, and each
/// value's range.
struct Layout {
    flag_bits: usize,
    coordinate_bits: usize,
    component_bits: usize,
    components: usize,
    decode: Vec<f64>,
}

impl Layout {
    fn scaled(raw: u64, bits: usize, low: f64, high: f64) -> f64 {
        low + raw as f64 * (high - low) / ((1u64 << bits) - 1) as f64
    }

    fn flag(&self, bits: &mut Bits) -> Option<u64> {
        if self.flag_bits == 0 {
            return Some(0);
        }
        bits.read(self.flag_bits)
    }

    fn point(&self, bits: &mut Bits) -> Option<Point> {
        let d = &self.decode;
        let x = Self::scaled(
            bits.read(self.coordinate_bits)?,
            self.coordinate_bits,
            d[0],
            d[1],
        );
        let y = Self::scaled(
            bits.read(self.coordinate_bits)?,
            self.coordinate_bits,
            d[2],
            d[3],
        );
        Some((x, y))
    }

    /// A vertex's colour components, scaled to their ranges.
    fn components(&self, bits: &mut Bits) -> Option<Vec<f32>> {
        (0..self.components)
            .map(|index| {
                let (low, high) = (
                    self.decode.get(4 + index * 2).copied().unwrap_or(0.0),
                    self.decode.get(5 + index * 2).copied().unwrap_or(1.0),
                );
                Some(Self::scaled(
                    bits.read(self.component_bits)?,
                    self.component_bits,
                    low,
                    high,
                ) as f32)
            })
            .collect()
    }
}

impl Mesh {
    fn read(
        document: &Document,
        dict: &Dict,
        kind: i64,
        data: &[u8],
        space: Space,
        function: Option<Function>,
    ) -> Option<Self> {
        let bits_of =
            |key: &str| document.entry(dict, key).as_i64().unwrap_or(0).clamp(0, 32) as usize;
        let layout = Layout {
            flag_bits: bits_of("BitsPerFlag"),
            coordinate_bits: bits_of("BitsPerCoordinate"),
            component_bits: bits_of("BitsPerComponent"),
            components: if function.is_some() {
                1
            } else {
                space.components()
            },
            decode: numbers(document, dict, "Decode"),
        };
        if layout.decode.len() < 4 + layout.components * 2 {
            return None;
        }
        let colour = |values: &[f32]| -> Rgb {
            match function {
                Some(_) => [values.first().copied().unwrap_or(0.0), 0.0, 0.0],
                None => space.rgb(values),
            }
        };
        let mut bits = Bits { data, at: 0 };
        let vertices_per_row = document
            .entry(dict, "VerticesPerRow")
            .as_i64()
            .unwrap_or(0)
            .max(0) as usize;
        let triangles = match kind {
            4 => free_form(&layout, &mut bits, &colour),
            5 => lattice(&layout, &mut bits, &colour, vertices_per_row),
            _ => patches(&layout, &mut bits, &colour, kind == 7),
        };
        let range = (layout.decode[4], layout.decode[5]);
        let ramp = function.map(|function| sampled(&function, &space, range));
        Some(Self::indexed(triangles, ramp, range))
    }

    /// Sorts the triangles into cells by their bounds, for finding the ones under a point.
    fn indexed(triangles: Vec<Triangle>, ramp: Option<Vec<Rgb>>, range: (f64, f64)) -> Self {
        let corners = triangles.iter().flat_map(|triangle| triangle.points);
        let (mut left, mut top, mut right, mut bottom) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in corners {
            (left, top, right, bottom) = (left.min(x), top.min(y), right.max(x), bottom.max(y));
        }
        let (width, height) = (
            ((right - left) / CELLS as f64).max(1e-9),
            ((bottom - top) / CELLS as f64).max(1e-9),
        );
        let mut cells = vec![Vec::new(); CELLS * CELLS];
        for (index, triangle) in triangles.iter().enumerate() {
            let cell_of = |value: f64, start: f64, size: f64| {
                (((value - start) / size) as usize).min(CELLS - 1)
            };
            let xs = triangle.points.map(|point| cell_of(point.0, left, width));
            let ys = triangle.points.map(|point| cell_of(point.1, top, height));
            for row in *ys.iter().min().unwrap_or(&0)..=*ys.iter().max().unwrap_or(&0) {
                for column in *xs.iter().min().unwrap_or(&0)..=*xs.iter().max().unwrap_or(&0) {
                    cells[row * CELLS + column].push(index as u32);
                }
            }
        }
        Self {
            triangles,
            ramp,
            range,
            bounds: [left, top, width, height],
            cells,
        }
    }

    fn colour_at(&self, point: Point) -> Option<Rgb> {
        let [left, top, width, height] = self.bounds;
        let (column, row) = ((point.0 - left) / width, (point.1 - top) / height);
        if column < 0.0 || row < 0.0 || column >= CELLS as f64 || row >= CELLS as f64 {
            return None;
        }
        let cell = &self.cells[row as usize * CELLS + column as usize];
        let values = cell
            .iter()
            .rev()
            .find_map(|&index| interpolated(&self.triangles[index as usize], point))?;
        match &self.ramp {
            Some(ramp) => {
                let share = (f64::from(values[0]) - self.range.0)
                    / (self.range.1 - self.range.0).max(1e-12);
                ramp.get((share.clamp(0.0, 1.0) * (ramp.len() - 1) as f64).round() as usize)
                    .copied()
            }
            None => Some(values),
        }
    }
}

/// The triangle's values at `point`, weighed by where it falls; `None` outside it.
fn interpolated(triangle: &Triangle, point: Point) -> Option<Rgb> {
    let [a, b, c] = triangle.points;
    let area = (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
    if area.abs() < 1e-12 {
        return None;
    }
    let wa = ((b.0 - point.0) * (c.1 - point.1) - (c.0 - point.0) * (b.1 - point.1)) / area;
    let wb = ((c.0 - point.0) * (a.1 - point.1) - (a.0 - point.0) * (c.1 - point.1)) / area;
    let wc = 1.0 - wa - wb;
    const EDGE: f64 = -1e-6;
    if wa < EDGE || wb < EDGE || wc < EDGE {
        return None;
    }
    let [va, vb, vc] = triangle.values;
    Some([0, 1, 2].map(|channel| {
        (f64::from(va[channel]) * wa + f64::from(vb[channel]) * wb + f64::from(vc[channel]) * wc)
            as f32
    }))
}

/// A vertex: where it is and its colour values.
type Vertex = (Point, Rgb);

fn vertex(layout: &Layout, bits: &mut Bits, colour: &impl Fn(&[f32]) -> Rgb) -> Option<Vertex> {
    let point = layout.point(bits)?;
    Some((point, colour(&layout.components(bits)?)))
}

fn triangle_of(a: Vertex, b: Vertex, c: Vertex) -> Triangle {
    Triangle {
        points: [a.0, b.0, c.0],
        values: [a.1, b.1, c.1],
    }
}

/// A type 4 mesh: each vertex's flag starts a triangle (0) or makes one with the last two
/// vertices (1) or with the first and last of the last triangle (2).
fn free_form(layout: &Layout, bits: &mut Bits, colour: &impl Fn(&[f32]) -> Rgb) -> Vec<Triangle> {
    let mut triangles = Vec::new();
    let mut last: Vec<Vertex> = Vec::new();
    let mut pending: Vec<Vertex> = Vec::new();
    while bits.has_data() && triangles.len() < TRIANGLES_MAX {
        let (Some(flag), Some(next)) = (layout.flag(bits), vertex(layout, bits, colour)) else {
            break;
        };
        bits.align();
        if pending.is_empty() {
            pending = shared_corners(flag, &last);
        }
        pending.push(next);
        if pending.len() == 3 {
            triangles.push(triangle_of(pending[0], pending[1], pending[2]));
            last = std::mem::take(&mut pending);
        }
    }
    triangles
}

/// The corners of the last triangle a new one shares, by the flag of its first vertex.
fn shared_corners(flag: u64, last: &[Vertex]) -> Vec<Vertex> {
    match (flag, last) {
        (1, [_, b, c]) => vec![*b, *c],
        (2, [a, _, c]) => vec![*a, *c],
        _ => Vec::new(),
    }
}

/// A type 5 mesh: rows of vertices, each two rows making a strip of triangles.
fn lattice(
    layout: &Layout,
    bits: &mut Bits,
    colour: &impl Fn(&[f32]) -> Rgb,
    per_row: usize,
) -> Vec<Triangle> {
    if per_row < 2 {
        return Vec::new();
    }
    let mut vertices = Vec::new();
    while bits.has_data() && vertices.len() < TRIANGLES_MAX {
        let Some(next) = vertex(layout, bits, colour) else {
            break;
        };
        vertices.push(next);
    }
    let rows = vertices.len() / per_row;
    let mut triangles = Vec::new();
    for row in 0..rows.saturating_sub(1) {
        for column in 0..per_row - 1 {
            let q = row * per_row + column;
            triangles.push(triangle_of(
                vertices[q],
                vertices[q + 1],
                vertices[q + per_row],
            ));
            triangles.push(triangle_of(
                vertices[q + 1],
                vertices[q + per_row + 1],
                vertices[q + per_row],
            ));
        }
    }
    triangles
}

/// One patch: its 4 × 4 grid of control points (row by row) and its corner colours, in the
/// order of grid points 0, 3, 12 and 15.
struct Patch {
    grid: [Point; 16],
    corners: [Rgb; 4],
}

/// Where the points a patch reads go in its grid, and which of the last patch's grid points
/// and corner colours it shares, by its flag (as pdf.js lays them out).
const NEW_POINTS: [[usize; 12]; 4] = [
    [0, 4, 8, 12, 13, 14, 15, 11, 7, 3, 2, 1],
    [13, 14, 15, 11, 7, 3, 2, 1, 0, 0, 0, 0],
    [13, 14, 15, 11, 7, 3, 2, 1, 0, 0, 0, 0],
    [13, 14, 15, 11, 7, 3, 2, 1, 0, 0, 0, 0],
];
/// For flags 1 to 3, the shared points: (new grid place, old grid place).
const SHARED_POINTS: [[(usize, usize); 4]; 3] = [
    [(0, 12), (4, 13), (8, 14), (12, 15)],
    [(0, 15), (4, 11), (8, 7), (12, 3)],
    [(0, 3), (4, 2), (8, 1), (12, 0)],
];
/// For flags 1 to 3, the shared corner colours: (new corner, old corner).
const SHARED_CORNERS: [[(usize, usize); 2]; 3] =
    [[(0, 2), (2, 3)], [(0, 3), (2, 1)], [(0, 1), (2, 0)]];
/// The tensor patch's inner points, read after the outer twelve: grid places.
const INNER_POINTS: [usize; 4] = [5, 9, 10, 6];

/// Reads the next patch, sharing an edge with `last` when its flag says so.
fn next_patch(
    layout: &Layout,
    bits: &mut Bits,
    colour: &impl Fn(&[f32]) -> Rgb,
    tensor: bool,
    last: Option<&Patch>,
) -> Option<Patch> {
    let flag = layout.flag(bits)?.min(3) as usize;
    let mut patch = Patch {
        grid: [(0.0, 0.0); 16],
        corners: [[0.0; 3]; 4],
    };
    let shared = match (flag, last) {
        (0, _) => 0,
        (_, Some(last)) => patch.share_edge(last, flag),
        (_, None) => return None,
    };
    let Patch { grid, corners } = &mut patch;
    for &place in &NEW_POINTS[flag][..12 - shared] {
        grid[place] = layout.point(bits)?;
    }
    if tensor {
        for place in INNER_POINTS {
            grid[place] = layout.point(bits)?;
        }
    }
    let new_corners: &[usize] = if shared == 0 { &[0, 2, 3, 1] } else { &[3, 1] };
    for &corner in new_corners {
        corners[corner] = colour(&layout.components(bits)?);
    }
    if !tensor {
        coons_inner_points(grid);
    }
    Some(patch)
}

/// A Coons patch's inner control points, from its boundary, so it draws as a tensor patch.
fn coons_inner_points(grid: &mut [Point; 16]) {
    let g = *grid;
    let mix = |a: usize, b: usize, c: [usize; 2], d: [usize; 2], e: [usize; 2]| -> Point {
        let combine = |pick: fn(Point) -> f64| {
            (-4.0 * pick(g[a]) - pick(g[b]) + 6.0 * (pick(g[c[0]]) + pick(g[c[1]]))
                - 2.0 * (pick(g[d[0]]) + pick(g[d[1]]))
                + 3.0 * (pick(g[e[0]]) + pick(g[e[1]])))
                / 9.0
        };
        (combine(|point| point.0), combine(|point| point.1))
    };
    grid[5] = mix(0, 15, [4, 1], [12, 3], [13, 7]);
    grid[6] = mix(3, 12, [2, 7], [0, 15], [4, 14]);
    grid[9] = mix(12, 3, [8, 13], [0, 15], [11, 1]);
    grid[10] = mix(15, 0, [11, 14], [12, 3], [2, 8]);
}

fn bernstein(t: f64) -> [f64; 4] {
    let u = 1.0 - t;
    [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t]
}

impl Patch {
    /// Takes the edge and two corner colours of `last` that flag `flag` (1 to 3) shares,
    /// giving how many points that is.
    fn share_edge(&mut self, last: &Patch, flag: usize) -> usize {
        for (new, old) in SHARED_POINTS[flag - 1] {
            self.grid[new] = last.grid[old];
        }
        for (new, old) in SHARED_CORNERS[flag - 1] {
            self.corners[new] = last.corners[old];
        }
        4
    }

    /// The patch cut into triangles: its surface sampled on a grid, colours bilinear between
    /// its corners.
    fn triangles(&self) -> Vec<Triangle> {
        let steps = PATCH_STEPS;
        let sample = |row: usize, column: usize| -> Vertex {
            let (v, u) = (row as f64 / steps as f64, column as f64 / steps as f64);
            let (bv, bu) = (bernstein(v), bernstein(u));
            let mut point = (0.0, 0.0);
            for (index, control) in self.grid.iter().enumerate() {
                let weight = bv[index / 4] * bu[index % 4];
                point.0 += weight * control.0;
                point.1 += weight * control.1;
            }
            let [c0, c1, c2, c3] = self.corners;
            let colour = [0, 1, 2].map(|channel| {
                let top = c0[channel] as f64 * (1.0 - u) + c1[channel] as f64 * u;
                let bottom = c2[channel] as f64 * (1.0 - u) + c3[channel] as f64 * u;
                (top * (1.0 - v) + bottom * v) as f32
            });
            (point, colour)
        };
        let mut triangles = Vec::with_capacity(steps * steps * 2);
        for row in 0..steps {
            for column in 0..steps {
                let (a, b) = (sample(row, column), sample(row, column + 1));
                let (c, d) = (sample(row + 1, column), sample(row + 1, column + 1));
                triangles.push(triangle_of(a, b, c));
                triangles.push(triangle_of(b, d, c));
            }
        }
        triangles
    }
}

/// A type 6 or 7 mesh: patches, each sharing an edge with the last unless its flag is 0.
fn patches(
    layout: &Layout,
    bits: &mut Bits,
    colour: &impl Fn(&[f32]) -> Rgb,
    tensor: bool,
) -> Vec<Triangle> {
    let mut triangles = Vec::new();
    let mut last: Option<Patch> = None;
    while bits.has_data() && triangles.len() < TRIANGLES_MAX {
        let Some(patch) = next_patch(layout, bits, colour, tensor, last.as_ref()) else {
            break;
        };
        triangles.extend(patch.triangles());
        last = Some(patch);
    }
    triangles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_radial_gradient_runs_between_its_two_circles() {
        let (c0, c1) = ((0.0, 0.0), (0.0, 0.0));
        // Concentric: t is the distance's share of the way from radius 10 to 20.
        let t = radial_t((15.0, 0.0), c0, 10.0, c1, 20.0, [false, false]).expect("inside");
        assert!((t - 0.5).abs() < 1e-9, "{t}");
        assert_eq!(
            radial_t((5.0, 0.0), c0, 10.0, c1, 20.0, [false, false]),
            None
        );
        // A focal point inside a larger circle reaches every point of it.
        let focal = radial_t(
            (0.0, 9.0),
            (0.0, 5.0),
            0.0,
            (0.0, 0.0),
            10.0,
            [false, false],
        );
        assert!(focal.is_some_and(|t| t > 0.0 && t <= 1.0), "{focal:?}");
    }

    #[test]
    fn a_triangle_blends_its_corners_colours() {
        let triangle = Triangle {
            points: [(0.0, 0.0), (10.0, 0.0), (0.0, 10.0)],
            values: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        };
        assert_eq!(interpolated(&triangle, (0.0, 0.0)), Some([1.0, 0.0, 0.0]));
        let middle = interpolated(&triangle, (5.0, 0.0)).expect("on an edge");
        assert!((middle[0] - 0.5).abs() < 1e-6 && (middle[1] - 0.5).abs() < 1e-6);
        assert_eq!(interpolated(&triangle, (9.0, 9.0)), None);
    }

    #[test]
    fn a_flat_patch_covers_its_square_with_bilinear_colours() {
        let grid: [Point; 16] = std::array::from_fn(|index| {
            (
                (index % 4) as f64 * 10.0 / 3.0,
                (index / 4) as f64 * 10.0 / 3.0,
            )
        });
        let patch = Patch {
            grid,
            corners: [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
        };
        let mesh = Mesh::indexed(patch.triangles(), None, (0.0, 1.0));
        let centre = mesh.colour_at((5.0, 5.0)).expect("inside");
        assert!(
            (centre[0] - 0.5).abs() < 0.01 && (centre[1] - 0.5).abs() < 0.01,
            "{centre:?}"
        );
        assert_eq!(mesh.colour_at((20.0, 5.0)), None);
    }
}
