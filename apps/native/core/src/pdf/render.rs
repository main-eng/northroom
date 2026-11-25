//! Runs a page's content: the graphics state, paths, colour, text, images, forms, gradients and
//! annotations' appearances, handing each mark to a `Sink`. The same run plans a page (which
//! parts it has: drawn layers and the JPEG pictures between them) and renders one of its layers.
//!
//! Left out: blend modes and soft masks of the graphics state, knockout and isolated groups,
//! tiling patterns (not painted), shading types other than axial and radial (painted in their
//! middle colour), text rendering's clip modes (drawn as fills), and optional content.

use super::blend::Blend;
use super::colour::{Function, Space};
use super::font::Font;
use super::image::{self, Picture};
use super::layers::Layers;
use super::object::{Dict, Document, Object, Parser, Ref, Stream, Token};
use super::raster::{Canvas, Mask, Rgba};
use super::shading::{Paint, Shading};
use super::stroke::Pen;
use crate::deck::Command;
use std::collections::HashMap;
use std::rc::Rc;

pub type Matrix = [f64; 6];
pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
/// How deep forms and Type 3 glyphs may nest.
const DEPTH_MAX: usize = 16;
/// The most operators one page runs, so a damaged stream can't run for ever.
const OPERATORS_MAX: usize = 4_000_000;

/// `a`, then `b`.
pub fn multiply(a: Matrix, b: Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

pub fn apply(m: Matrix, (x, y): (f64, f64)) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

pub fn invert(m: Matrix) -> Option<Matrix> {
    let determinant = m[0] * m[3] - m[1] * m[2];
    if determinant.abs() < 1e-12 {
        return None;
    }
    let inverse = [
        m[3] / determinant,
        -m[1] / determinant,
        -m[2] / determinant,
        m[0] / determinant,
        0.0,
        0.0,
    ];
    let (e, f) = apply(inverse, (m[4], m[5]));
    Some([inverse[0], inverse[1], inverse[2], inverse[3], -e, -f])
}

fn transform(command: Command, m: Matrix) -> Command {
    match command {
        Command::Move(x, y) => {
            let (x, y) = apply(m, (x, y));
            Command::Move(x, y)
        }
        Command::Line(x, y) => {
            let (x, y) = apply(m, (x, y));
            Command::Line(x, y)
        }
        Command::Quad(a, b, x, y) => {
            let ((a, b), (x, y)) = (apply(m, (a, b)), apply(m, (x, y)));
            Command::Quad(a, b, x, y)
        }
        Command::Cubic(a, b, c, d, x, y) => {
            let ((a, b), (c, d), (x, y)) = (apply(m, (a, b)), apply(m, (c, d)), apply(m, (x, y)));
            Command::Cubic(a, b, c, d, x, y)
        }
        Command::Close => Command::Close,
    }
}

/// The largest side a tiling pattern's cell is drawn at, in pixels.
const TILE_SIDE_MAX: f64 = 1024.0;

/// Paints nothing: what planning shades with, as it needs only to know a mark is made.
struct Nothing;

impl Paint for Nothing {
    fn colour_at(&self, _: (f64, f64)) -> Option<Rgba> {
        None
    }
}

/// A tiling pattern's cell, drawn, repeated across the page.
struct Tile {
    /// From device pixels to the pattern's space.
    inverse: Matrix,
    /// The cell's corner and its steps across and up, in the pattern's space.
    origin: (f64, f64),
    step: (f64, f64),
    /// The cell's size in pixels, and its drawing.
    size: (usize, usize),
    canvas: Canvas,
    /// An uncoloured pattern's colour.
    tint: Option<Rgba>,
}

impl Paint for Tile {
    fn colour_at(&self, point: (f64, f64)) -> Option<Rgba> {
        let (x, y) = apply(self.inverse, point);
        let across = ((x - self.origin.0) / self.step.0).rem_euclid(1.0);
        let up = ((y - self.origin.1) / self.step.1).rem_euclid(1.0);
        let column = ((across * self.size.0 as f64) as usize).min(self.size.0 - 1);
        let row = (((1.0 - up) * self.size.1 as f64) as usize).min(self.size.1 - 1);
        let colour = self.canvas.straight(column, row);
        if colour[3] <= 0.0 {
            return None;
        }
        Some(match self.tint {
            Some(tint) => [tint[0], tint[1], tint[2], tint[3] * colour[3]],
            None => colour,
        })
    }
}

thread_local! {
    /// The content streams being run, innermost last, so content that calls itself (a form
    /// drawing itself, two Type 3 fonts drawing each other) stops rather than multiplying.
    static RUNNING: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A content stream being run, for as long as it is kept.
struct Running(usize);

impl Running {
    /// Notes `stream` as running; `None` when it already is, or when content is nested as deep
    /// as it may be.
    fn enter(stream: &Stream) -> Option<Self> {
        let id = Rc::as_ptr(&stream.raw) as usize;
        RUNNING.with(|running| {
            let mut running = running.borrow_mut();
            if running.contains(&id) || running.len() >= DEPTH_MAX {
                return None;
            }
            running.push(id);
            Some(Running(id))
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.with(|running| {
            let mut running = running.borrow_mut();
            if let Some(at) = running.iter().rposition(|id| *id == self.0) {
                running.remove(at);
            }
        });
    }
}

/// The clip in effect: none, or a coverage mask over the canvas.
pub type Clip = Option<Rc<Mask>>;

/// Where a run's marks go.
pub trait Sink {
    /// Whether the run should load fonts and decode pictures: a planner needs only to know
    /// where marks are.
    fn wants_detail(&self) -> bool;
    fn fill(&mut self, path: &[Command], colour: Rgba, clip: &Clip);
    fn stroke(&mut self, path: &[Command], pen: &Pen, colour: Rgba, clip: &Clip);
    /// Paints each pixel within `clip` the colour `paint` gives it.
    fn shade(&mut self, paint: &dyn Paint, clip: &Clip);
    fn picture(&mut self, picture: Option<Picture>, matrix: Matrix, alpha: f32, clip: &Clip);
    /// An encoded picture the system decodes and draws on its own: a JPEG or JPEG 2000 shown
    /// in `frame` (left, top, width, height), with `crop` of each edge (left, top, right,
    /// bottom, as shares of the picture) cut off by a rectangular clip.
    fn encoded(&mut self, stream: &Stream, frame: [f64; 4], crop: [f64; 4]);
    /// The clip `clip` narrowed to `path`.
    fn clip(&mut self, clip: &Clip, path: &[Command]) -> Clip;
    /// Paints from here on mix with what is under them in `blend`.
    fn set_blend(&mut self, _blend: Blend) {}
    /// Where both clips cover.
    fn combine(&mut self, a: &Clip, _b: &Clip) -> Clip {
        a.clone()
    }
    /// Starts drawing on a canvas of its own: a transparency group, or a soft mask when `mask`.
    fn begin_group(&mut self, _mask: bool) {}
    /// Paints the group begun last onto what is under it, at `alpha` within `clip`, in `blend`.
    fn end_group(&mut self, _alpha: f32, _blend: Blend, _clip: &Clip) {}
    /// Ends the soft mask begun last: its luminosity over `backdrop`, or its alpha, each value
    /// then remapped through `transfer` when it has one.
    fn end_mask(&mut self, _backdrop: Option<[f32; 3]>, _transfer: Option<&[u8; 256]>) -> Clip {
        None
    }
}

#[derive(Clone)]
struct TextState {
    font: Option<Rc<Font>>,
    size: f64,
    character_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    mode: i64,
}

#[derive(Clone)]
struct State {
    ctm: Matrix,
    fill_space: Space,
    fill: Vec<f32>,
    fill_pattern: Option<String>,
    stroke_space: Space,
    stroke: Vec<f32>,
    /// The pen in user space.
    pen: Pen,
    fill_alpha: f32,
    stroke_alpha: f32,
    clip: Clip,
    /// The clip's shape, which decides whether a JPEG can be left to the system.
    clip_area: ClipArea,
    blend: Blend,
    /// The graphics state's soft mask, drawn (`None` while planning).
    soft_mask: Clip,
    /// Whether a soft mask is set, drawn or not, so planning and drawing agree.
    masked: bool,
    /// What paints within: the clip and soft mask together.
    paint_clip: Clip,
    text: TextState,
}

/// The shape of the clip in effect, in device space.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ClipArea {
    Whole,
    /// An upright rectangle: left, top, right, bottom.
    Rectangle([f64; 4]),
    Shaped,
}

impl ClipArea {
    /// The clip narrowed to `path`.
    fn narrowed(self, path: &[Command]) -> Self {
        match (self, rectangle_of(path)) {
            (ClipArea::Whole, Some(rectangle)) => ClipArea::Rectangle(rectangle),
            (ClipArea::Rectangle(area), Some(rectangle)) => {
                ClipArea::Rectangle(overlap(area, rectangle))
            }
            _ => ClipArea::Shaped,
        }
    }
}

/// Where two rectangles (left, top, right, bottom) overlap; empty when they don't.
fn overlap(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let (left, top) = (a[0].max(b[0]), a[1].max(b[1]));
    [left, top, a[2].min(b[2]).max(left), a[3].min(b[3]).max(top)]
}

/// The upright rectangle a path outlines (left, top, right, bottom), if it is one: a single
/// outline of four straight sides, each level or plumb.
fn rectangle_of(path: &[Command]) -> Option<[f64; 4]> {
    let mut points = Vec::new();
    for (index, command) in path.iter().enumerate() {
        match (index, *command) {
            (0, Command::Move(x, y)) | (1.., Command::Line(x, y)) => points.push((x, y)),
            (1.., Command::Close) => {}
            _ => return None,
        }
    }
    if points.len() == 5 && points[4] == points[0] {
        points.pop();
    }
    if points.len() != 4 {
        return None;
    }
    let level_or_plumb = (0..4).all(|index| {
        let (from, to) = (points[index], points[(index + 1) % 4]);
        (from.0 - to.0).abs() < 1e-6 || (from.1 - to.1).abs() < 1e-6
    });
    let xs = points.iter().map(|point| point.0);
    let ys = points.iter().map(|point| point.1);
    level_or_plumb.then(|| {
        [
            xs.clone().fold(f64::MAX, f64::min),
            ys.clone().fold(f64::MAX, f64::min),
            xs.fold(f64::MIN, f64::max),
            ys.fold(f64::MIN, f64::max),
        ]
    })
}

/// How a picture is shown.
enum Placement {
    /// Clipped away entirely.
    Hidden,
    /// Left to the system's decoder, in `frame` with `crop` cut off (`Sink::encoded`).
    System { frame: [f64; 4], crop: [f64; 4] },
    /// Decoded here and drawn into the layer, with its transparency, masks and clip.
    Drawn,
}

impl State {
    fn new(ctm: Matrix) -> Self {
        Self {
            ctm,
            fill_space: Space::Gray,
            fill: vec![0.0],
            fill_pattern: None,
            stroke_space: Space::Gray,
            stroke: vec![0.0],
            pen: Pen::solid(1.0),
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            clip: None,
            clip_area: ClipArea::Whole,
            blend: Blend::Normal,
            soft_mask: None,
            masked: false,
            paint_clip: None,
            text: TextState {
                font: None,
                size: 0.0,
                character_spacing: 0.0,
                word_spacing: 0.0,
                scale: 1.0,
                leading: 0.0,
                rise: 0.0,
                mode: 0,
            },
        }
    }

    fn fill_colour(&self) -> Rgba {
        self.fill_space.rgba(&self.fill, self.fill_alpha)
    }

    fn stroke_colour(&self) -> Rgba {
        self.stroke_space.rgba(&self.stroke, self.stroke_alpha)
    }

    /// The pen in pixels: its lengths scaled by the current matrix's average scale.
    fn device_pen(&self) -> Pen {
        let scale = (self.ctm[0] * self.ctm[3] - self.ctm[1] * self.ctm[2])
            .abs()
            .sqrt();
        self.pen.scaled(scale)
    }
}

/// Sets a pen's dashes from a `d` operator's or `D` entry's array and phase. A pattern of
/// all zeros, or with a negative length, is solid.
fn set_dash(pen: &mut Pen, pattern: Option<&Object>, phase: Option<&Object>) {
    let lengths: Vec<f64> = pattern
        .and_then(Object::as_array)
        .map(numbers_of)
        .unwrap_or_default();
    let usable = lengths.iter().all(|length| *length >= 0.0) && lengths.iter().sum::<f64>() > 0.0;
    pen.dash = if usable { lengths } else { Vec::new() };
    pen.phase = phase.map_or(0.0, number);
}

/// Fonts loaded once for a document, by their object.
#[derive(Default)]
pub struct Fonts(HashMap<Ref, Rc<Font>>);

/// Runs content on a sink.
pub struct Interpreter<'a, S: Sink> {
    document: &'a Document,
    sink: &'a mut S,
    fonts: &'a mut Fonts,
    state: State,
    saved: Vec<State>,
    path: Vec<Command>,
    point: (f64, f64),
    start: (f64, f64),
    pending_clip: bool,
    text_matrix: Matrix,
    line_matrix: Matrix,
    operators: usize,
    /// The page's own matrix, which patterns are placed by.
    base: Matrix,
    /// The resources of the content being run.
    current_resources: Option<Dict>,
    depth: usize,
    /// The document's hidden layers.
    layers: Layers,
    /// How many groups and soft masks are being drawn on canvases of their own.
    grouped: usize,
    /// Open marked content, each whether it hides what it marks (`BMC`, `BDC`, `EMC`).
    marked: Vec<bool>,
}

fn number(object: &Object) -> f64 {
    object.as_f64().unwrap_or(0.0)
}

fn numbers_of(operands: &[Object]) -> Vec<f64> {
    operands.iter().map(number).collect()
}

/// The operands of an operator as a matrix, when there are six.
fn matrix_of(operands: &[Object]) -> Option<Matrix> {
    numbers_of(operands).try_into().ok()
}

impl<'a, S: Sink> Interpreter<'a, S> {
    pub fn new(document: &'a Document, sink: &'a mut S, fonts: &'a mut Fonts, ctm: Matrix) -> Self {
        Self {
            document,
            sink,
            fonts,
            state: State::new(ctm),
            saved: Vec::new(),
            path: Vec::new(),
            point: (0.0, 0.0),
            start: (0.0, 0.0),
            pending_clip: false,
            text_matrix: IDENTITY,
            line_matrix: IDENTITY,
            operators: 0,
            base: ctm,
            current_resources: None,
            depth: 0,
            layers: Layers::of(document),
            grouped: 0,
            marked: Vec::new(),
        }
    }

    /// Whether `object`, a layer or group of layers, is hidden.
    pub fn hides(&self, object: &Object) -> bool {
        self.layers.hides(self.document, object)
    }

    /// Whether the content being run is in a hidden layer: it changes the graphics state and
    /// clips but draws nothing.
    fn hidden(&self) -> bool {
        self.marked.contains(&true)
    }

    /// `BMC`, `BDC` and `EMC`: marked content opens and closes, hiding what it marks when it
    /// names a hidden layer.
    fn marked_content(&mut self, operator: &[u8], operands: &[Object], resources: Option<&Dict>) {
        match operator {
            b"EMC" => {
                self.marked.pop();
            }
            b"BDC" if operands.first().and_then(Object::as_name) == Some("OC") => {
                let hides = match operands.get(1) {
                    Some(Object::Name(_)) => self.hides_named(operands.get(1), resources),
                    Some(inline) => self.hides(inline),
                    None => false,
                };
                self.marked.push(hides);
            }
            _ => self.marked.push(false),
        }
    }

    /// Whether a layer named in resources hides, by its reference there, as layers are known
    /// by reference.
    fn hides_named(&self, name: Option<&Object>, resources: Option<&Dict>) -> bool {
        let (Some(name), Some(resources)) = (name.and_then(Object::as_name), resources) else {
            return false;
        };
        let properties = self.document.entry(resources, "Properties");
        properties
            .as_dict()
            .and_then(|properties| properties.get(name))
            .is_some_and(|layer| self.hides(layer))
    }

    fn resource(&self, resources: Option<&Dict>, kind: &str, name: &str) -> Object {
        let Some(resources) = resources else {
            return Object::Null;
        };
        let group = self.document.entry(resources, kind);
        group
            .as_dict()
            .and_then(|group| group.get(name))
            .map_or(Object::Null, |object| self.document.resolve(object))
    }

    // --- Paths -----------------------------------------------------------------------------

    fn device(&self, x: f64, y: f64) -> (f64, f64) {
        apply(self.state.ctm, (x, y))
    }

    fn move_to(&mut self, x: f64, y: f64) {
        let point = self.device(x, y);
        self.path.push(Command::Move(point.0, point.1));
        self.point = point;
        self.start = point;
    }

    fn line_to(&mut self, x: f64, y: f64) {
        let point = self.device(x, y);
        self.path.push(Command::Line(point.0, point.1));
        self.point = point;
    }

    fn curve_to(&mut self, a: (f64, f64), b: (f64, f64), to: (f64, f64)) {
        self.path
            .push(Command::Cubic(a.0, a.1, b.0, b.1, to.0, to.1));
        self.point = to;
    }

    fn rectangle(&mut self, values: &[f64]) {
        let [x, y, width, height] = <[f64; 4]>::try_from(values).unwrap_or([0.0; 4]);
        self.move_to(x, y);
        self.line_to(x + width, y);
        self.line_to(x + width, y + height);
        self.line_to(x, y + height);
        self.path.push(Command::Close);
        self.point = self.start;
    }

    fn path_operator(&mut self, operator: &[u8], values: &[f64]) {
        let at = |index: usize| values.get(index).copied().unwrap_or(0.0);
        match operator {
            b"m" => self.move_to(at(0), at(1)),
            b"l" => self.line_to(at(0), at(1)),
            b"c" => {
                let (a, b, to) = (
                    self.device(at(0), at(1)),
                    self.device(at(2), at(3)),
                    self.device(at(4), at(5)),
                );
                self.curve_to(a, b, to);
            }
            b"v" => {
                let (b, to) = (self.device(at(0), at(1)), self.device(at(2), at(3)));
                self.curve_to(self.point, b, to);
            }
            b"y" => {
                let (a, to) = (self.device(at(0), at(1)), self.device(at(2), at(3)));
                self.curve_to(a, to, to);
            }
            b"h" => {
                self.path.push(Command::Close);
                self.point = self.start;
            }
            b"re" => self.rectangle(values),
            _ => {}
        }
    }

    /// Paints the current path as `fill` and `stroke` say, then sets a pending clip.
    fn paint(&mut self, fill: bool, stroke: bool, close: bool) {
        if close {
            self.path.push(Command::Close);
        }
        let path = std::mem::take(&mut self.path);
        let shown = !self.hidden();
        if fill && shown {
            self.fill_path(&path);
        }
        if stroke && shown {
            let pen = self.state.device_pen();
            let clip = self.ready();
            self.sink
                .stroke(&path, &pen, self.state.stroke_colour(), &clip);
        }
        if self.pending_clip {
            self.pending_clip = false;
            self.state.clip = self.sink.clip(&self.state.clip, &path);
            self.state.clip_area = self.state.clip_area.narrowed(&path);
            self.refresh_paint_clip();
        }
    }

    /// Readies the sink to paint in the current blend mode, and gives the area it paints in.
    fn ready(&mut self) -> Clip {
        self.sink.set_blend(self.state.blend);
        self.state.paint_clip.clone()
    }

    /// Recomputes where painting reaches after the clip or soft mask changes.
    fn refresh_paint_clip(&mut self) {
        self.state.paint_clip = match &self.state.soft_mask {
            Some(_) => self.sink.combine(&self.state.clip, &self.state.soft_mask),
            None => self.state.clip.clone(),
        };
    }

    fn fill_path(&mut self, path: &[Command]) {
        match self.state.fill_pattern.clone() {
            Some(name) => self.fill_with_pattern(path, &name),
            None => {
                let clip = self.ready();
                self.sink.fill(path, self.state.fill_colour(), &clip);
            }
        }
    }

    // --- Colour ----------------------------------------------------------------------------

    fn set_space(&mut self, operand: Option<&Object>, resources: Option<&Dict>, stroke: bool) {
        let space = operand
            .and_then(|object| image::named_space(self.document, object, resources))
            .unwrap_or(Space::Gray);
        let initial = space.initial();
        if stroke {
            self.state.stroke_space = space;
            self.state.stroke = initial;
        } else {
            self.state.fill_space = space;
            self.state.fill = initial;
            self.state.fill_pattern = None;
        }
    }

    fn set_colour(&mut self, operands: &[Object], stroke: bool) {
        let values: Vec<f32> = operands
            .iter()
            .filter_map(Object::as_f64)
            .map(|value| value as f32)
            .collect();
        let pattern = operands.last().and_then(Object::as_name).map(str::to_owned);
        if stroke {
            self.state.stroke = values;
        } else {
            self.state.fill = values;
            self.state.fill_pattern = pattern;
        }
    }

    fn set_device(&mut self, space: Space, operands: &[Object], stroke: bool) {
        let values: Vec<f32> = operands
            .iter()
            .filter_map(Object::as_f64)
            .map(|value| value as f32)
            .collect();
        if stroke {
            self.state.stroke_space = space;
            self.state.stroke = values;
        } else {
            self.state.fill_space = space;
            self.state.fill = values;
            self.state.fill_pattern = None;
        }
    }

    fn colour_operator(&mut self, operator: &[u8], operands: &[Object], resources: Option<&Dict>) {
        match operator {
            b"g" => self.set_device(Space::Gray, operands, false),
            b"G" => self.set_device(Space::Gray, operands, true),
            b"rg" => self.set_device(Space::Rgb, operands, false),
            b"RG" => self.set_device(Space::Rgb, operands, true),
            b"k" => self.set_device(Space::Cmyk, operands, false),
            b"K" => self.set_device(Space::Cmyk, operands, true),
            b"cs" => self.set_space(operands.first(), resources, false),
            b"CS" => self.set_space(operands.first(), resources, true),
            b"sc" | b"scn" => self.set_colour(operands, false),
            b"SC" | b"SCN" => self.set_colour(operands, true),
            _ => {}
        }
    }

    // --- Gradients -------------------------------------------------------------------------

    /// `sh`: paints a shading over the clip.
    fn shade(&mut self, name: &str, resources: Option<&Dict>) {
        if self.hidden() {
            return;
        }
        let shading = self.resource(resources, "Shading", name);
        let alpha = self.state.fill_alpha;
        let clip = self.ready();
        if !self.sink.wants_detail() {
            self.sink.shade(&Nothing, &clip);
            return;
        }
        if let Some(shading) =
            Shading::read(self.document, &shading, self.state.ctm, alpha, resources)
        {
            self.sink.shade(&shading, &clip);
        }
    }

    /// Fills `path` with the pattern `name`: a shading, or a cell of content repeated.
    fn fill_with_pattern(&mut self, path: &[Command], name: &str) {
        let pattern = self.resource(self.current_resources.as_ref(), "Pattern", name);
        let paint_clip = self.ready();
        let clip = self.sink.clip(&paint_clip, path);
        if !self.sink.wants_detail() {
            self.sink.shade(&Nothing, &clip);
            return;
        }
        let Some(dict) = pattern.as_dict() else {
            return;
        };
        let matrix: Matrix = self
            .document
            .entry(dict, "Matrix")
            .as_array()
            .and_then(|items| numbers_of(items).try_into().ok())
            .unwrap_or(IDENTITY);
        let placed = multiply(matrix, self.base);
        let paint: Option<Box<dyn Paint>> =
            match (&pattern, self.document.entry(dict, "PatternType").as_i64()) {
                (Object::Stream(stream), Some(1)) => self
                    .read_tile(stream, placed)
                    .map(|tile| Box::new(tile) as Box<dyn Paint>),
                (_, Some(2)) => {
                    let shading = self.document.entry(dict, "Shading");
                    Shading::read(
                        self.document,
                        &shading,
                        placed,
                        self.state.fill_alpha,
                        self.current_resources.as_ref(),
                    )
                    .map(|shading| Box::new(shading) as Box<dyn Paint>)
                }
                _ => None,
            };
        if let Some(paint) = paint {
            self.sink.shade(paint.as_ref(), &clip);
        }
    }

    /// A tiling pattern's cell drawn once at its size on the page, to repeat across what it
    /// fills. An uncoloured pattern's cell takes the colour given with it.
    fn read_tile(&mut self, stream: &Stream, placed: Matrix) -> Option<Tile> {
        if self.depth >= DEPTH_MAX {
            return None;
        }
        let dict = &stream.dict;
        let bbox: [f64; 4] = self
            .document
            .entry(dict, "BBox")
            .as_array()
            .map(numbers_of)?
            .try_into()
            .ok()?;
        let entry = |key: &str| self.document.entry(dict, key).as_f64().unwrap_or(0.0).abs();
        let step = (entry("XStep"), entry("YStep"));
        if step.0 < 1e-9 || step.1 < 1e-9 {
            return None;
        }
        let pixels = |length: f64, (a, b): (f64, f64)| {
            (length * a.hypot(b)).ceil().clamp(1.0, TILE_SIDE_MAX) as usize
        };
        let size = (
            pixels(step.0, (placed[0], placed[1])),
            pixels(step.1, (placed[2], placed[3])),
        );
        let origin = (bbox[0].min(bbox[2]), bbox[1].min(bbox[3]));
        let tint = (self.document.entry(dict, "PaintType").as_i64() == Some(2))
            .then(|| self.pattern_tint());
        let canvas = self.draw_cell(stream, origin, step, size)?;
        Some(Tile {
            inverse: invert(placed)?,
            origin,
            step,
            size,
            canvas,
            tint,
        })
    }

    /// The colour an uncoloured pattern paints in: the components given before its name, in
    /// the pattern space's own colour space.
    fn pattern_tint(&self) -> Rgba {
        let values = &self.state.fill;
        match &self.state.fill_space {
            Space::Pattern(Some(space)) => space.rgba(values, self.state.fill_alpha),
            _ => [0.0, 0.0, 0.0, self.state.fill_alpha],
        }
    }

    /// Draws a pattern's cell into a canvas of `size` pixels covering one `step` from
    /// `origin`, drawn again a step back on each axis so what spills past the cell wraps.
    fn draw_cell(
        &mut self,
        stream: &Stream,
        origin: (f64, f64),
        step: (f64, f64),
        size: (usize, usize),
    ) -> Option<Canvas> {
        let _running = Running::enter(stream)?;
        let content = self.document.stream_data(stream).ok()?;
        let resources = self
            .document
            .entry(&stream.dict, "Resources")
            .as_dict()
            .cloned();
        let (sx, sy) = (size.0 as f64 / step.0, size.1 as f64 / step.1);
        let to_cell = [sx, 0.0, 0.0, -sy, -origin.0 * sx, (origin.1 + step.1) * sy];
        let mut renderer = Renderer::new(size.0, size.1, 0);
        let mut fonts = Fonts::default();
        for shift in [
            (0.0, 0.0),
            (-step.0, 0.0),
            (0.0, -step.1),
            (-step.0, -step.1),
        ] {
            let matrix = multiply([1.0, 0.0, 0.0, 1.0, shift.0, shift.1], to_cell);
            let mut inner = Interpreter::new(self.document, &mut renderer, &mut fonts, matrix);
            inner.depth = self.depth + 1;
            inner.run_page(&content, resources.clone());
        }
        Some(renderer.canvas)
    }
}

// --- Text, pictures, forms and the run ----------------------------------------------------

impl<'a, S: Sink> Interpreter<'a, S> {
    fn load_font(&mut self, object: &Object) -> Option<Rc<Font>> {
        let reference = match object {
            Object::Ref(reference) => Some(*reference),
            _ => None,
        };
        if let Some(known) = reference.and_then(|reference| self.fonts.0.get(&reference)) {
            return Some(known.clone());
        }
        let dict = self.document.resolve(object);
        let font = Rc::new(Font::load(self.document, dict.as_dict()?));
        if let Some(reference) = reference {
            self.fonts.0.insert(reference, font.clone());
        }
        Some(font)
    }

    fn set_font(&mut self, operands: &[Object], resources: Option<&Dict>) {
        let (Some(name), Some(size)) = (
            operands.first().and_then(Object::as_name),
            operands.get(1).and_then(Object::as_f64),
        ) else {
            return;
        };
        self.state.text.size = size;
        if !self.sink.wants_detail() {
            return;
        }
        let object = resources
            .map(|resources| self.document.entry(resources, "Font"))
            .and_then(|fonts| fonts.as_dict().and_then(|fonts| fonts.get(name)).cloned())
            .unwrap_or(Object::Null);
        self.state.text.font = self.load_font(&object);
    }

    fn move_line(&mut self, x: f64, y: f64) {
        self.line_matrix = multiply([1.0, 0.0, 0.0, 1.0, x, y], self.line_matrix);
        self.text_matrix = self.line_matrix;
    }

    fn next_line(&mut self) {
        let leading = self.state.text.leading;
        self.move_line(0.0, -leading);
    }

    /// Draws one glyph at the text matrix: its outline, or its Type 3 procedure.
    fn draw_glyph(&mut self, font: &Rc<Font>, code: u32, glyph_matrix: Matrix) {
        let mode = self.state.text.mode;
        if mode == 3 || mode == 7 || self.hidden() {
            return;
        }
        let device = multiply(glyph_matrix, self.state.ctm);
        if font.is_type3() {
            self.draw_type3(font, code, glyph_matrix);
            return;
        }
        let Some(outline) = font.outline(code) else {
            return;
        };
        let path: Vec<Command> = outline
            .iter()
            .map(|command| transform(*command, device))
            .collect();
        let stroke = matches!(mode, 1 | 2 | 5 | 6);
        if matches!(mode, 0 | 2 | 4 | 6) {
            self.fill_path(&path);
        }
        if stroke {
            let pen = self.state.device_pen();
            let clip = self.ready();
            self.sink
                .stroke(&path, &pen, self.state.stroke_colour(), &clip);
        }
    }

    fn draw_type3(&mut self, font: &Rc<Font>, code: u32, glyph_matrix: Matrix) {
        let Some(procedure) = font.procedure(self.document, code) else {
            return;
        };
        let Some(_running) = Running::enter(&procedure.stream) else {
            return;
        };
        let Ok(content) = self.document.stream_data(&procedure.stream) else {
            return;
        };
        let resources = procedure
            .resources
            .cloned()
            .or_else(|| self.current_resources.clone());
        let ctm = multiply(procedure.matrix, multiply(glyph_matrix, self.state.ctm));
        self.run_nested(&content, resources, ctm, None);
    }

    /// Shows a string: each code's glyph, then the pen moved by its width and spacing.
    fn show(&mut self, bytes: &[u8]) {
        let text = self.state.text.clone();
        let Some(font) = text.font.clone() else {
            // Without its font, the text still counts as drawn for planning.
            if !self.sink.wants_detail() && !bytes.is_empty() && !matches!(text.mode, 3 | 7) {
                let point = self.device(0.0, 0.0);
                self.sink.fill(
                    &[Command::Move(point.0, point.1)],
                    [0.0; 4],
                    &self.state.clip,
                );
            }
            return;
        };
        for shown in font.codes(bytes) {
            let code = shown.code;
            let glyph_matrix = multiply(
                [text.size * text.scale, 0.0, 0.0, text.size, 0.0, text.rise],
                self.text_matrix,
            );
            self.draw_glyph(&font, code, glyph_matrix);
            let spacing = if shown.is_space {
                text.word_spacing
            } else {
                0.0
            };
            let advance =
                (font.width(code) * text.size + text.character_spacing + spacing) * text.scale;
            self.text_matrix = multiply([1.0, 0.0, 0.0, 1.0, advance, 0.0], self.text_matrix);
        }
    }

    fn show_adjusted(&mut self, items: &[Object]) {
        for item in items {
            match item {
                Object::String(bytes) => self.show(bytes),
                other => {
                    let shift =
                        -number(other) / 1000.0 * self.state.text.size * self.state.text.scale;
                    self.text_matrix = multiply([1.0, 0.0, 0.0, 1.0, shift, 0.0], self.text_matrix);
                }
            }
        }
    }

    fn text_operator(&mut self, operator: &[u8], operands: &[Object], resources: Option<&Dict>) {
        let values = numbers_of(operands);
        let at = |index: usize| values.get(index).copied().unwrap_or(0.0);
        let text = &mut self.state.text;
        match operator {
            b"BT" => {
                self.text_matrix = IDENTITY;
                self.line_matrix = IDENTITY;
            }
            b"Tc" => text.character_spacing = at(0),
            b"Tw" => text.word_spacing = at(0),
            b"Tz" => text.scale = at(0) / 100.0,
            b"TL" => text.leading = at(0),
            b"Ts" => text.rise = at(0),
            b"Tr" => text.mode = at(0) as i64,
            b"Tf" => self.set_font(operands, resources),
            b"Td" => self.move_line(at(0), at(1)),
            b"TD" => {
                text.leading = -at(1);
                self.move_line(at(0), at(1));
            }
            b"Tm" => {
                self.line_matrix = matrix_of(operands).unwrap_or(IDENTITY);
                self.text_matrix = self.line_matrix;
            }
            b"T*" => self.next_line(),
            _ => self.show_operator(operator, operands),
        }
    }

    fn show_operator(&mut self, operator: &[u8], operands: &[Object]) {
        let last_bytes = operands
            .last()
            .and_then(Object::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        match operator {
            b"Tj" => self.show(&last_bytes),
            b"'" => {
                self.next_line();
                self.show(&last_bytes);
            }
            b"\"" => {
                self.state.text.word_spacing = operands.first().map_or(0.0, number);
                self.state.text.character_spacing = operands.get(1).map_or(0.0, number);
                self.next_line();
                self.show(&last_bytes);
            }
            b"TJ" => {
                let items = operands
                    .first()
                    .and_then(Object::as_array)
                    .map(<[Object]>::to_vec)
                    .unwrap_or_default();
                self.show_adjusted(&items);
            }
            _ => {}
        }
    }

    // --- Pictures and forms ----------------------------------------------------------------

    /// Whether a picture can be left to the system: a JPEG drawn upright, opaque, unmasked and
    /// clipped at most to a rectangle. A JPEG 2000 never is, as only some systems decode it.
    fn placement(&self, stream: &Stream) -> Placement {
        let matrix = self.state.ctm;
        let codec = super::filter::codec(stream, Some(self.document));
        let masked = ["SMask", "Mask"]
            .iter()
            .any(|key| !matches!(self.document.entry(&stream.dict, key), Object::Null));
        let upright = matrix[1].abs() <= 1e-9 * matrix[0].abs()
            && matrix[2].abs() <= 1e-9 * matrix[3].abs()
            && matrix[0] > 0.0
            && matrix[3] < 0.0;
        let plain = upright
            && self.state.fill_alpha >= 1.0
            && !masked
            && self.state.clip_area != ClipArea::Shaped
            && self.state.blend == Blend::Normal
            && !self.state.masked
            && self.grouped == 0;
        match codec {
            Some("jpeg") if plain => system_placement(matrix, self.state.clip_area),
            _ => Placement::Drawn,
        }
    }

    fn draw_picture(&mut self, stream: &Stream) {
        if self.hidden() {
            return;
        }
        let matrix = self.state.ctm;
        match self.placement(stream) {
            Placement::Hidden => return,
            Placement::System { frame, crop } => {
                self.sink.encoded(stream, frame, crop);
                return;
            }
            Placement::Drawn => {}
        }
        let picture = if self.sink.wants_detail() {
            image::read(
                self.document,
                stream,
                self.state.fill_colour(),
                self.current_resources.as_ref(),
            )
        } else {
            None
        };
        let clip = self.ready();
        self.sink
            .picture(picture, matrix, self.state.fill_alpha, &clip);
    }

    /// Draws a form: as a transparency group on a canvas of its own when its group's alpha,
    /// blend mode or soft mask must apply to it as a whole, else straight onto the page.
    fn draw_form(&mut self, stream: &Stream) {
        let group = self.document.entry(&stream.dict, "Group");
        let is_group = group
            .as_dict()
            .is_some_and(|group| self.document.entry(group, "S").as_name() == Some("Transparency"));
        let state = &self.state;
        let composited = state.fill_alpha < 1.0 || state.blend != Blend::Normal || state.masked;
        if !(is_group && composited) {
            self.draw_form_content(stream);
            return;
        }
        let (alpha, blend, clip) = (state.fill_alpha, state.blend, state.paint_clip.clone());
        self.sink.begin_group(false);
        self.draw_fresh(stream);
        self.sink.end_group(alpha, blend, &clip);
    }

    /// Draws a form as a group or soft mask draws its content: opaque, unblended, unmasked and
    /// unclipped, the group's own alpha, blend and clip applied when it is painted.
    fn draw_fresh(&mut self, stream: &Stream) {
        let outer = self.state.clone();
        let state = &mut self.state;
        (state.fill_alpha, state.stroke_alpha, state.blend) = (1.0, 1.0, Blend::Normal);
        (state.soft_mask, state.masked, state.clip, state.clip_area) =
            (None, false, None, ClipArea::Whole);
        self.refresh_paint_clip();
        self.grouped += 1;
        self.draw_form_content(stream);
        self.grouped -= 1;
        self.state = outer;
    }

    /// A graphics state's soft mask (`SMask`): its group drawn, then its luminosity over its
    /// backdrop colour, or its alpha, as coverage. `None` while planning, which needs only
    /// to know there is one.
    fn read_soft_mask(&mut self, object: &Object) -> Clip {
        let dict = self.document.resolve(object);
        let dict = dict.as_dict()?;
        let Object::Stream(group) = self.document.entry(dict, "G") else {
            return None;
        };
        if !self.sink.wants_detail() {
            return None;
        }
        let backdrop = match self.document.entry(dict, "S").as_name() {
            Some("Alpha") => None,
            _ => Some(self.mask_backdrop(dict, &group)),
        };
        let transfer = self.mask_transfer(dict);
        self.sink.begin_group(true);
        self.draw_fresh(&group);
        self.sink.end_mask(backdrop, transfer.as_ref())
    }

    /// A soft mask's transfer function (`TR`), sampled at each mask value; `None` for the
    /// identity.
    fn mask_transfer(&self, dict: &Dict) -> Option<[u8; 256]> {
        let function = Function::read(self.document, dict.get("TR")?, 0)?;
        Some(std::array::from_fn(|value| {
            let out = function.evaluate(&[value as f32 / 255.0]);
            (out.first().copied().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8
        }))
    }

    /// A luminosity mask's backdrop (`BC`), in its group's colour space; black by default.
    fn mask_backdrop(&self, dict: &Dict, group: &Stream) -> [f32; 3] {
        let values: Vec<f32> = self
            .document
            .entry(dict, "BC")
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(Object::as_f64)
                    .map(|value| value as f32)
                    .collect()
            })
            .unwrap_or_default();
        if values.is_empty() {
            return [0.0; 3];
        }
        let group_dict = self.document.entry(&group.dict, "Group");
        let space = group_dict
            .as_dict()
            .and_then(|group| group.get("CS"))
            .and_then(|space| {
                image::named_space(self.document, space, self.current_resources.as_ref())
            })
            .unwrap_or(match values.len() {
                1 => Space::Gray,
                4 => Space::Cmyk,
                _ => Space::Rgb,
            });
        space.rgb(&values)
    }

    fn draw_form_content(&mut self, stream: &Stream) {
        let Some(_running) = Running::enter(stream) else {
            return;
        };
        let matrix = self
            .document
            .entry(&stream.dict, "Matrix")
            .as_array()
            .and_then(|items| numbers_of(items).try_into().ok())
            .unwrap_or(IDENTITY);
        let Ok(content) = self.document.stream_data(stream) else {
            return;
        };
        let resources = self
            .document
            .entry(&stream.dict, "Resources")
            .as_dict()
            .cloned()
            .or_else(|| self.current_resources.clone());
        let bbox = self
            .document
            .entry(&stream.dict, "BBox")
            .as_array()
            .map(numbers_of);
        let ctm = multiply(matrix, self.state.ctm);
        self.run_nested(&content, resources, ctm, bbox.as_deref());
    }

    /// Narrows the clip to a form's bounding box, given in its own space.
    fn clip_to_box(&mut self, bbox: &[f64]) {
        let [x0, y0, x1, y1] = <[f64; 4]>::try_from(bbox).unwrap_or([0.0; 4]);
        if x0 == x1 || y0 == y1 {
            return;
        }
        let corners =
            [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|point| self.device(point.0, point.1));
        let mut path: Vec<Command> = corners.iter().map(|&(x, y)| Command::Line(x, y)).collect();
        path[0] = Command::Move(corners[0].0, corners[0].1);
        path.push(Command::Close);
        self.state.clip = self.sink.clip(&self.state.clip, &path);
        self.state.clip_area = self.state.clip_area.narrowed(&path);
        self.refresh_paint_clip();
    }

    fn draw_object(&mut self, name: &str, resources: Option<&Dict>) {
        let Object::Stream(stream) = self.resource(resources, "XObject", name) else {
            return;
        };
        if self.hidden() || stream.dict.get("OC").is_some_and(|layer| self.hides(layer)) {
            return;
        }
        match self.document.entry(&stream.dict, "Subtype").as_name() {
            Some("Image") => self.draw_picture(&stream),
            Some("Form") => self.draw_form(&stream),
            _ => {}
        }
    }

    /// Runs content in its own graphics state with `ctm`, as a form or glyph is.
    /// A form's content is clipped to its `bbox`.
    fn run_nested(
        &mut self,
        content: &[u8],
        resources: Option<Dict>,
        ctm: Matrix,
        bbox: Option<&[f64]>,
    ) {
        if self.depth >= DEPTH_MAX {
            return;
        }
        let saved = (
            self.state.clone(),
            self.saved.len(),
            std::mem::take(&mut self.path),
            self.text_matrix,
            self.line_matrix,
        );
        let outer = std::mem::replace(&mut self.current_resources, resources);
        self.state.ctm = ctm;
        if let Some(bbox) = bbox {
            self.clip_to_box(bbox);
        }
        let marked = self.marked.len();
        self.depth += 1;
        self.run(content);
        self.depth -= 1;
        // Marked content a form leaves open ends with it.
        self.marked.truncate(marked);
        self.current_resources = outer;
        self.saved.truncate(saved.1);
        self.state = saved.0;
        self.path = saved.2;
        self.text_matrix = saved.3;
        self.line_matrix = saved.4;
    }

    /// A graphics state's blend mode (`BM`, a name or a list to choose from) and soft mask
    /// (`SMask`, a mask or `None`).
    fn transparency_state(&mut self, dict: &Dict) {
        match self.document.entry(dict, "BM") {
            Object::Name(name) => self.state.blend = Blend::named(&name),
            Object::Array(names) => {
                let known = names
                    .iter()
                    .filter_map(Object::as_name)
                    .map(Blend::named)
                    .find(|blend| *blend != Blend::Normal);
                self.state.blend = known.unwrap_or(Blend::Normal);
            }
            _ => {}
        }
        let Some(mask) = dict.get("SMask").cloned() else {
            return;
        };
        if self.document.resolve(&mask).as_name() == Some("None") {
            (self.state.soft_mask, self.state.masked) = (None, false);
        } else {
            self.state.masked = true;
            self.state.soft_mask = self.read_soft_mask(&mask);
        }
        self.refresh_paint_clip();
    }

    fn graphics_state(&mut self, name: &str, resources: Option<&Dict>) {
        let state = self.resource(resources, "ExtGState", name);
        let Some(dict) = state.as_dict() else {
            return;
        };
        let entry = |key: &str| self.document.entry(dict, key);
        self.pen_state(dict);
        self.transparency_state(dict);
        if let Some(alpha) = entry("ca").as_f64() {
            self.state.fill_alpha = alpha.clamp(0.0, 1.0) as f32;
        }
        if let Some(alpha) = entry("CA").as_f64() {
            self.state.stroke_alpha = alpha.clamp(0.0, 1.0) as f32;
        }
        if let Some(items) = entry("Font").as_array().map(<[Object]>::to_vec) {
            if let (Some(font), Some(size)) = (items.first(), items.get(1).and_then(Object::as_f64))
            {
                self.state.text.size = size;
                if self.sink.wants_detail() {
                    self.state.text.font = self.load_font(font);
                }
            }
        }
    }

    // --- The run ---------------------------------------------------------------------------

    fn state_operator(&mut self, operator: &[u8], operands: &[Object], resources: Option<&Dict>) {
        match operator {
            b"q" => {
                if self.saved.len() < 256 {
                    self.saved.push(self.state.clone());
                }
            }
            b"Q" => {
                if let Some(state) = self.saved.pop() {
                    self.state = state;
                }
            }
            b"cm" => {
                if let Some(matrix) = matrix_of(operands) {
                    self.state.ctm = multiply(matrix, self.state.ctm);
                }
            }
            b"w" | b"J" | b"j" | b"M" | b"d" => self.pen_operator(operator, operands),
            b"gs" => {
                if let Some(name) = operands.first().and_then(Object::as_name) {
                    self.graphics_state(name, resources);
                }
            }
            _ => {}
        }
    }

    /// `w`, `J`, `j`, `M` and `d`: the pen's width, ends, corners, miter limit and dashes.
    fn pen_operator(&mut self, operator: &[u8], operands: &[Object]) {
        let first = operands.first().map_or(0.0, number);
        let pen = &mut self.state.pen;
        match operator {
            b"w" => pen.width = first.max(0.0),
            b"J" => pen.cap = Pen::cap_of(first as i64),
            b"j" => pen.join = Pen::join_of(first as i64),
            b"M" => pen.miter_limit = first.max(1.0),
            _ => set_dash(pen, operands.first(), operands.get(1)),
        }
    }

    /// A graphics state's pen entries: `LW`, `LC`, `LJ`, `ML` and `D`.
    fn pen_state(&mut self, dict: &Dict) {
        let entry = |key: &str| self.document.entry(dict, key);
        let (width, cap, join, limit) = (
            entry("LW").as_f64(),
            entry("LC").as_i64(),
            entry("LJ").as_i64(),
            entry("ML").as_f64(),
        );
        let dash = entry("D");
        let pen = &mut self.state.pen;
        pen.width = width.map_or(pen.width, |width| width.max(0.0));
        pen.cap = cap.map_or(pen.cap, Pen::cap_of);
        pen.join = join.map_or(pen.join, Pen::join_of);
        pen.miter_limit = limit.map_or(pen.miter_limit, |limit| limit.max(1.0));
        if let Some(items) = dash.as_array() {
            set_dash(pen, items.first(), items.get(1));
        }
    }

    fn paint_operator(&mut self, operator: &[u8]) {
        match operator {
            b"f" | b"F" | b"f*" => self.paint(true, false, false),
            b"S" => self.paint(false, true, false),
            b"s" => self.paint(false, true, true),
            b"B" | b"B*" => self.paint(true, true, false),
            b"b" | b"b*" => self.paint(true, true, true),
            b"n" => self.paint(false, false, false),
            b"W" | b"W*" => self.pending_clip = true,
            _ => {}
        }
    }

    fn operate(&mut self, operator: &[u8], operands: &[Object]) {
        let resources = self.current_resources.clone();
        let resources = resources.as_ref();
        let name = operands
            .first()
            .and_then(Object::as_name)
            .map(str::to_owned);
        match operator {
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" => {
                self.path_operator(operator, &numbers_of(operands))
            }
            b"f" | b"F" | b"f*" | b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" | b"n" | b"W"
            | b"W*" => self.paint_operator(operator),
            b"q" | b"Q" | b"cm" | b"w" | b"J" | b"j" | b"M" | b"d" | b"gs" => {
                self.state_operator(operator, operands, resources)
            }
            b"g" | b"G" | b"rg" | b"RG" | b"k" | b"K" | b"cs" | b"CS" | b"sc" | b"scn" | b"SC"
            | b"SCN" => {
                self.colour_operator(operator, operands, resources);
            }
            b"BMC" | b"BDC" | b"EMC" => self.marked_content(operator, operands, resources),
            b"Do" => {
                if let Some(name) = name {
                    self.draw_object(&name, resources);
                }
            }
            b"sh" => {
                if let Some(name) = name {
                    self.shade(&name, resources);
                }
            }
            b"BT" | b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Ts" | b"Tr" | b"Tf" | b"Td" | b"TD"
            | b"Tm" | b"T*" | b"Tj" | b"'" | b"\"" | b"TJ" => {
                self.text_operator(operator, operands, resources);
            }
            _ => {}
        }
    }

    /// An inline image after `BI`: its entries up to `ID`, then its data up to `EI`.
    fn inline_image(&mut self, parser: &mut Parser) {
        let mut dict = Dict::new();
        loop {
            match parser.next_token() {
                Some(Token::Name(key)) => {
                    if let Some(value) = parser.object(0) {
                        dict.insert(key, value);
                    }
                }
                Some(Token::Word(word)) if word == b"ID" => break,
                Some(_) => {}
                None => return,
            }
        }
        let bytes = parser.lexer.bytes;
        let start = parser.lexer.at + 1;
        let end = (start..bytes.len().saturating_sub(2))
            .find(|&at| {
                bytes[at] == b'E'
                    && bytes[at + 1] == b'I'
                    && super::object::is_white(bytes[at - 1])
                    && bytes
                        .get(at + 2)
                        .is_none_or(|after| super::object::is_white(*after))
            })
            .unwrap_or(bytes.len());
        parser.lexer.at = (end + 2).min(bytes.len());
        let raw = Rc::new(
            bytes
                .get(start..end.saturating_sub(1).max(start))
                .unwrap_or(&[])
                .to_vec(),
        );
        self.draw_picture(&Stream { dict, raw });
    }

    /// Runs a content stream.
    pub fn run(&mut self, content: &[u8]) {
        let mut parser = Parser::new(content, 0);
        let mut operands: Vec<Object> = Vec::new();
        while let Some(token) = parser.next_token() {
            self.operators += 1;
            if self.operators > OPERATORS_MAX {
                return;
            }
            let Token::Word(word) = token else {
                parser.put_back(token);
                if let Some(operand) = parser.object(0) {
                    operands.push(operand);
                }
                continue;
            };
            match word.as_slice() {
                b"true" => operands.push(Object::Bool(true)),
                b"false" => operands.push(Object::Bool(false)),
                b"null" => operands.push(Object::Null),
                b"BI" => {
                    self.inline_image(&mut parser);
                    operands.clear();
                }
                operator => {
                    self.operate(operator, &operands);
                    operands.clear();
                }
            }
        }
    }

    /// Runs a page's content with its resources.
    pub fn run_page(&mut self, content: &[u8], resources: Option<Dict>) {
        self.current_resources = resources;
        self.run(content);
    }

    /// Draws an annotation's normal appearance in its rectangle.
    pub fn run_appearance(&mut self, stream: &Stream, rectangle: [f64; 4]) {
        let Some(_running) = Running::enter(stream) else {
            return;
        };
        let matrix: Matrix = self
            .document
            .entry(&stream.dict, "Matrix")
            .as_array()
            .and_then(|items| numbers_of(items).try_into().ok())
            .unwrap_or(IDENTITY);
        let bbox: Vec<f64> = self
            .document
            .entry(&stream.dict, "BBox")
            .as_array()
            .map(numbers_of)
            .unwrap_or_default();
        let [x0, y0, x1, y1] =
            <[f64; 4]>::try_from(bbox.as_slice()).unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|point| apply(matrix, point));
        let (left, bottom) = corners.iter().fold((f64::MAX, f64::MAX), |(x, y), point| {
            (x.min(point.0), y.min(point.1))
        });
        let (right, top) = corners.iter().fold((f64::MIN, f64::MIN), |(x, y), point| {
            (x.max(point.0), y.max(point.1))
        });
        let (width, height) = ((right - left).max(1e-9), (top - bottom).max(1e-9));
        let [rx0, ry0, rx1, ry1] = rectangle;
        let fit = [
            (rx1 - rx0) / width,
            0.0,
            0.0,
            (ry1 - ry0) / height,
            rx0 - left * (rx1 - rx0) / width,
            ry0 - bottom * (ry1 - ry0) / height,
        ];
        let Ok(content) = self.document.stream_data(stream) else {
            return;
        };
        let resources = self
            .document
            .entry(&stream.dict, "Resources")
            .as_dict()
            .cloned();
        self.run_nested(
            &content,
            resources,
            multiply(multiply(matrix, fit), self.base),
            None,
        );
    }
}

// --- Planning and rendering ----------------------------------------------------------------

/// A part of a page: a layer of drawn marks, or an encoded picture with its frame in page
/// points (left, top, width, height) and the shares of each edge a clip cuts off.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Layer,
    Encoded {
        extension: &'static str,
        frame: [f64; 4],
        crop: [f64; 4],
    },
}

/// Where a picture the system draws shows: its frame cut to the clip's rectangle, and the
/// shares of each edge cut off.
fn system_placement(matrix: Matrix, clip: ClipArea) -> Placement {
    let [left, top, width, height] = frame_of(matrix);
    let whole = [left, top, left + width, top + height];
    let shown = match clip {
        ClipArea::Rectangle(area) => overlap(whole, area),
        ClipArea::Whole | ClipArea::Shaped => whole,
    };
    if shown[2] <= shown[0] || shown[3] <= shown[1] || width <= 0.0 || height <= 0.0 {
        return Placement::Hidden;
    }
    Placement::System {
        frame: [shown[0], shown[1], shown[2] - shown[0], shown[3] - shown[1]],
        crop: [
            (shown[0] - whole[0]) / width,
            (shown[1] - whole[1]) / height,
            (whole[2] - shown[2]) / width,
            (whole[3] - shown[3]) / height,
        ],
    }
}

/// The frame a unit square covers under `matrix`, as left, top, width, height.
pub fn frame_of(matrix: Matrix) -> [f64; 4] {
    let corners =
        [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|point| apply(matrix, point));
    let left = corners.iter().map(|point| point.0).fold(f64::MAX, f64::min);
    let top = corners.iter().map(|point| point.1).fold(f64::MAX, f64::min);
    let right = corners.iter().map(|point| point.0).fold(f64::MIN, f64::max);
    let bottom = corners.iter().map(|point| point.1).fold(f64::MIN, f64::max);
    [left, top, right - left, bottom - top]
}

/// Finds a page's parts without drawing: marks only say which layer they fall in.
#[derive(Default)]
pub struct Planner {
    pub parts: Vec<Part>,
    marked: bool,
}

impl Planner {
    pub fn finish(mut self) -> Vec<Part> {
        if self.marked {
            self.parts.push(Part::Layer);
        }
        self.parts
    }
}

impl Sink for Planner {
    fn wants_detail(&self) -> bool {
        false
    }
    fn fill(&mut self, _: &[Command], _: Rgba, _: &Clip) {
        self.marked = true;
    }
    fn stroke(&mut self, _: &[Command], _: &Pen, _: Rgba, _: &Clip) {
        self.marked = true;
    }
    fn shade(&mut self, _: &dyn Paint, _: &Clip) {
        self.marked = true;
    }
    fn picture(&mut self, _: Option<Picture>, _: Matrix, _: f32, _: &Clip) {
        self.marked = true;
    }
    fn encoded(&mut self, stream: &Stream, frame: [f64; 4], crop: [f64; 4]) {
        if std::mem::take(&mut self.marked) {
            self.parts.push(Part::Layer);
        }
        let extension = super::filter::codec(stream, None).unwrap_or("jpeg");
        self.parts.push(Part::Encoded {
            extension,
            frame,
            crop,
        });
    }
    fn clip(&mut self, _: &Clip, _: &[Command]) -> Clip {
        None
    }
}

/// Draws one layer of a page into a canvas, counting layers as the planner does.
pub struct Renderer {
    pub canvas: Canvas,
    target: usize,
    layer: usize,
    marked: bool,
    /// Groups and soft masks being drawn on canvases of their own, innermost last; `None`
    /// for one begun outside the layer being drawn.
    groups: Vec<Option<Canvas>>,
    /// How many soft masks are being drawn: their marks are not the page's.
    masking: usize,
}

impl Renderer {
    pub fn new(width: usize, height: usize, target: usize) -> Self {
        Self {
            canvas: Canvas::new(width, height),
            target,
            layer: 0,
            marked: false,
            groups: Vec::new(),
            masking: 0,
        }
    }

    /// Notes a mark, and whether it is drawn: only in the layer being drawn.
    fn mark(&mut self) -> bool {
        if self.masking == 0 {
            self.marked = true;
        }
        self.layer == self.target
    }

    /// The canvas marks go on: the innermost group's, or the page's.
    fn top(&mut self) -> &mut Canvas {
        match self.groups.iter_mut().rev().find_map(Option::as_mut) {
            Some(group) => group,
            None => &mut self.canvas,
        }
    }
}

impl Sink for Renderer {
    fn wants_detail(&self) -> bool {
        self.layer <= self.target
    }
    fn fill(&mut self, path: &[Command], colour: Rgba, clip: &Clip) {
        if self.mark() {
            self.top().fill(path, colour, clip.as_deref());
        }
    }
    fn stroke(&mut self, path: &[Command], pen: &Pen, colour: Rgba, clip: &Clip) {
        if self.mark() {
            self.top().stroke(path, pen, colour, clip.as_deref());
        }
    }
    fn shade(&mut self, paint: &dyn Paint, clip: &Clip) {
        if self.mark() {
            self.top()
                .shade(|point| paint.colour_at(point), clip.as_deref());
        }
    }
    fn picture(&mut self, picture: Option<Picture>, matrix: Matrix, alpha: f32, clip: &Clip) {
        if !self.mark() {
            return;
        }
        if let Some(Picture {
            rgba,
            width,
            height,
        }) = picture
        {
            self.top()
                .image(&rgba, (width, height), matrix, alpha, clip.as_deref());
        }
    }
    fn encoded(&mut self, _: &Stream, _: [f64; 4], _: [f64; 4]) {
        if std::mem::take(&mut self.marked) {
            self.layer += 1;
        }
    }
    fn clip(&mut self, clip: &Clip, path: &[Command]) -> Clip {
        if self.layer > self.target {
            return clip.clone();
        }
        Some(Rc::new(self.canvas.clip_mask(path, clip.as_deref())))
    }
    fn set_blend(&mut self, blend: Blend) {
        self.top().mode = blend;
    }
    fn combine(&mut self, a: &Clip, b: &Clip) -> Clip {
        if self.layer != self.target {
            return a.clone();
        }
        super::raster::combine(a.as_deref(), b.as_deref()).map(Rc::new)
    }
    fn begin_group(&mut self, mask: bool) {
        if mask {
            self.masking += 1;
        }
        let canvas =
            (self.layer == self.target).then(|| Canvas::new(self.canvas.width, self.canvas.height));
        self.groups.push(canvas);
    }
    fn end_group(&mut self, alpha: f32, blend: Blend, clip: &Clip) {
        if let Some(Some(group)) = self.groups.pop() {
            let top = self.top();
            let mode = std::mem::replace(&mut top.mode, blend);
            top.composite(&group, alpha, clip.as_deref());
            top.mode = mode;
        }
    }
    fn end_mask(&mut self, backdrop: Option<[f32; 3]>, transfer: Option<&[u8; 256]>) -> Clip {
        self.masking = self.masking.saturating_sub(1);
        let Some(Some(group)) = self.groups.pop() else {
            return None;
        };
        let mut mask = group.mask_of(backdrop);
        if let Some(transfer) = transfer {
            mask.iter_mut()
                .for_each(|value| *value = transfer[usize::from(*value)]);
        }
        Some(Rc::new(mask))
    }
}

/// Finds a page's encoded pictures in order, keeping the bytes of picture `wanted`.
pub struct Extractor {
    wanted: usize,
    seen: usize,
    pub found: Option<Vec<u8>>,
}

impl Extractor {
    pub fn new(wanted: usize) -> Self {
        Self {
            wanted,
            seen: 0,
            found: None,
        }
    }
}

impl Sink for Extractor {
    fn wants_detail(&self) -> bool {
        false
    }
    fn fill(&mut self, _: &[Command], _: Rgba, _: &Clip) {}
    fn stroke(&mut self, _: &[Command], _: &Pen, _: Rgba, _: &Clip) {}
    fn shade(&mut self, _: &dyn Paint, _: &Clip) {}
    fn picture(&mut self, _: Option<Picture>, _: Matrix, _: f32, _: &Clip) {}
    fn encoded(&mut self, stream: &Stream, _: [f64; 4], _: [f64; 4]) {
        if self.seen == self.wanted {
            self.found = super::filter::decode(stream, None).ok();
        }
        self.seen += 1;
    }
    fn clip(&mut self, _: &Clip, _: &[Command]) -> Clip {
        None
    }
}
