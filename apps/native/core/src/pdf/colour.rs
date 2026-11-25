//! Colour spaces and functions: turning a colour's components into RGB.
//!
//! Device and ICC-based spaces are read by their number of components; a calibrated or Lab
//! colour is read as the device space it is closest to. Separation and DeviceN colours go
//! through their tint transform: an exponential, stitching, sampled or PostScript calculator
//! function.

use super::object::{Document, Object};
use super::raster::Rgba;

/// A colour space, as far as drawing it needs.
#[derive(Clone, Debug)]
pub enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// CIE L*a*b*, `range` bounding a* and b* (min and max of each).
    Lab {
        range: [f32; 4],
    },
    /// CalGray (one channel) or CalRGB (three): each channel raised to its gamma.
    Calibrated {
        channels: usize,
        gamma: [f32; 3],
    },
    Indexed {
        base: Box<Space>,
        high: usize,
        lookup: Vec<u8>,
    },
    /// Separation or DeviceN: its components go through `function` into `alternate`.
    Tint {
        components: usize,
        alternate: Box<Space>,
        function: Option<Function>,
    },
    /// Patterns; an uncoloured one is painted in the colour space it names, if any.
    Pattern(Option<Box<Space>>),
}

fn cmyk_to_rgb(c: f32, m: f32, y: f32, k: f32) -> [f32; 3] {
    [
        (1.0 - c) * (1.0 - k),
        (1.0 - m) * (1.0 - k),
        (1.0 - y) * (1.0 - k),
    ]
}

/// A Lab colour (L 0–100, a and b about ±100) in sRGB, with a D65 white.
/// A CIE-based space from its dictionary: CalGray and CalRGB by their gamma, Lab by its range.
fn calibrated(document: &Document, family: &str, dict: &Object) -> Space {
    let numbers = |key: &str| -> Vec<f32> {
        dict.as_dict()
            .map(|dict| document.entry(dict, key))
            .and_then(|object| {
                object.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(Object::as_f64)
                        .map(|value| value as f32)
                        .collect()
                })
            })
            .unwrap_or_default()
    };
    let gamma = dict.as_dict().map(|dict| document.entry(dict, "Gamma"));
    match family {
        "CalGray" => {
            let gamma = gamma.and_then(|gamma| gamma.as_f64()).unwrap_or(1.0) as f32;
            Space::Calibrated {
                channels: 1,
                gamma: [gamma; 3],
            }
        }
        "CalRGB" => {
            let gammas = numbers("Gamma");
            let gamma = <[f32; 3]>::try_from(gammas.as_slice()).unwrap_or([1.0; 3]);
            Space::Calibrated { channels: 3, gamma }
        }
        _ => {
            let range = <[f32; 4]>::try_from(numbers("Range").as_slice())
                .unwrap_or([-100.0, 100.0, -100.0, 100.0]);
            Space::Lab { range }
        }
    }
}

/// A linear-light channel in sRGB's encoding.
fn encode_srgb(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.003_130_8 {
        12.92 * channel
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

fn lab_to_rgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let (fx, fz) = (fy + a / 500.0, fy - b / 200.0);
    let inverse = |t: f32| {
        if t > 6.0 / 29.0 {
            t * t * t
        } else {
            3.0 * (6.0f32 / 29.0).powi(2) * (t - 4.0 / 29.0)
        }
    };
    let (x, y, z) = (0.950_47 * inverse(fx), inverse(fy), 1.088_83 * inverse(fz));
    let linear = [
        3.2406 * x - 1.5372 * y - 0.4986 * z,
        -0.9689 * x + 1.8758 * y + 0.0415 * z,
        0.0557 * x - 0.2040 * y + 1.0570 * z,
    ];
    linear.map(encode_srgb)
}

impl Space {
    pub fn components(&self) -> usize {
        match self {
            Space::Gray | Space::Indexed { .. } | Space::Pattern(_) => 1,
            Space::Rgb | Space::Lab { .. } => 3,
            Space::Calibrated { channels, .. } => *channels,
            Space::Cmyk => 4,
            Space::Tint { components, .. } => *components,
        }
    }

    /// The colour a space starts with: black, or the first entry of an indexed space.
    pub fn initial(&self) -> Vec<f32> {
        match self {
            Space::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            Space::Tint { components, .. } => vec![1.0; *components],
            other => vec![0.0; other.components()],
        }
    }

    pub fn rgb(&self, values: &[f32]) -> [f32; 3] {
        let at = |index: usize| values.get(index).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        match self {
            Space::Gray | Space::Pattern(_) => [at(0); 3],
            Space::Rgb => [at(0), at(1), at(2)],
            Space::Cmyk => cmyk_to_rgb(at(0), at(1), at(2), at(3)),
            Space::Lab { .. } => lab_to_rgb(
                values.first().copied().unwrap_or(0.0),
                values.get(1).copied().unwrap_or(0.0),
                values.get(2).copied().unwrap_or(0.0),
            ),
            Space::Calibrated { channels, gamma } => {
                let linear = |index: usize| encode_srgb(at(index).powf(gamma[index]));
                match channels {
                    1 => [linear(0); 3],
                    _ => [linear(0), linear(1), linear(2)],
                }
            }
            Space::Indexed { base, high, lookup } => {
                let index = values
                    .first()
                    .copied()
                    .unwrap_or(0.0)
                    .round()
                    .clamp(0.0, *high as f32) as usize;
                let count = base.components();
                let ranges = base.ranges();
                let entry: Vec<f32> = (0..count)
                    .map(|component| {
                        let (low, high) = ranges[component];
                        let share =
                            f32::from(*lookup.get(index * count + component).unwrap_or(&0)) / 255.0;
                        low + share * (high - low)
                    })
                    .collect();
                base.rgb(&entry)
            }
            Space::Tint {
                alternate,
                function,
                components,
            } => match function {
                Some(function) => alternate.rgb(&function.evaluate(values)),
                // Without a transform, a tint darkens like ink.
                None => {
                    let ink = values
                        .iter()
                        .take(*components)
                        .copied()
                        .fold(0.0f32, f32::max);
                    [1.0 - ink.clamp(0.0, 1.0); 3]
                }
            },
        }
    }

    /// Each component's range: what its lowest and highest samples stand for.
    pub fn ranges(&self) -> Vec<(f32, f32)> {
        match self {
            Space::Lab { range } => vec![(0.0, 100.0), (range[0], range[1]), (range[2], range[3])],
            Space::Indexed { high, .. } => vec![(0.0, *high as f32)],
            other => vec![(0.0, 1.0); other.components()],
        }
    }

    pub fn rgba(&self, values: &[f32], alpha: f32) -> Rgba {
        let [r, g, b] = self.rgb(values);
        [r, g, b, alpha]
    }

    /// The space a colour space object names, or `None` for one this reader doesn't know.
    pub fn read(document: &Document, object: &Object, depth: usize) -> Option<Space> {
        if depth > 8 {
            return None;
        }
        let object = document.resolve(object);
        if let Some(name) = object.as_name() {
            return Space::named(name);
        }
        let items = object.as_array()?;
        let family = document.resolve(items.first()?);
        let entry = |index: usize| {
            items
                .get(index)
                .map(|item| document.resolve(item))
                .unwrap_or(Object::Null)
        };
        match family.as_name()? {
            "ICCBased" => {
                let stream = entry(1);
                let count = stream
                    .as_dict()
                    .and_then(|dict| document.entry(dict, "N").as_i64())
                    .unwrap_or(3);
                Some(match count {
                    1 => Space::Gray,
                    4 => Space::Cmyk,
                    _ => Space::Rgb,
                })
            }
            "CalGray" | "CalRGB" | "Lab" => {
                Some(calibrated(document, family.as_name()?, &entry(1)))
            }
            "Indexed" | "I" => {
                let base = Space::read(document, &entry(1), depth + 1)?;
                let high = entry(2).as_i64().unwrap_or(0).clamp(0, 255) as usize;
                let lookup = match entry(3) {
                    Object::String(bytes) => bytes,
                    Object::Stream(stream) => document.stream_data(&stream).unwrap_or_default(),
                    _ => Vec::new(),
                };
                Some(Space::Indexed {
                    base: Box::new(base),
                    high,
                    lookup,
                })
            }
            "Separation" => Some(Space::Tint {
                components: 1,
                alternate: Box::new(
                    Space::read(document, &entry(2), depth + 1).unwrap_or(Space::Gray),
                ),
                function: Function::read(document, &entry(3), 0),
            }),
            "DeviceN" => Some(Space::Tint {
                components: entry(1).as_array().map_or(1, <[Object]>::len),
                alternate: Box::new(
                    Space::read(document, &entry(2), depth + 1).unwrap_or(Space::Gray),
                ),
                function: Function::read(document, &entry(3), 0),
            }),
            "Pattern" => Some(Space::Pattern(
                Space::read(document, &entry(1), depth + 1).map(Box::new),
            )),
            other => Space::named(other),
        }
    }

    fn named(name: &str) -> Option<Space> {
        match name {
            "DeviceGray" | "G" | "CalGray" => Some(Space::Gray),
            "DeviceRGB" | "RGB" | "CalRGB" => Some(Space::Rgb),
            "DeviceCMYK" | "CMYK" => Some(Space::Cmyk),
            "Pattern" => Some(Space::Pattern(None)),
            _ => None,
        }
    }
}

// --- Functions -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Function {
    /// Type 2: `C0 + x^N (C1 - C0)`.
    Exponential {
        start: Vec<f32>,
        end: Vec<f32>,
        exponent: f32,
    },
    /// Type 3: one function a part of the domain.
    Stitching {
        domain: [f32; 2],
        functions: Vec<Function>,
        bounds: Vec<f32>,
        encode: Vec<f32>,
    },
    /// Type 0, one input: samples, linearly interpolated.
    /// Type 0: a table of samples over each input, interpolated between its neighbours.
    Sampled(Box<Samples>),
    /// Type 4: a PostScript calculator program.
    Calculator(Vec<Operation>),
    /// Several functions, one an output.
    Each(Vec<Function>),
}

fn floats(document: &Document, object: Option<&Object>) -> Vec<f32> {
    object
        .map(|object| document.resolve(object))
        .and_then(|object| {
            object.as_array().map(|items| {
                items
                    .iter()
                    .filter_map(|item| document.resolve(item).as_f64())
                    .map(|value| value as f32)
                    .collect()
            })
        })
        .unwrap_or_default()
}

impl Function {
    pub fn read(document: &Document, object: &Object, depth: usize) -> Option<Function> {
        if depth > 8 {
            return None;
        }
        let object = document.resolve(object);
        if let Some(items) = object.as_array() {
            return Some(Function::Each(
                items
                    .iter()
                    .filter_map(|item| Function::read(document, item, depth + 1))
                    .collect(),
            ));
        }
        let dict = object.as_dict()?;
        let domain = floats(document, dict.get("Domain"));
        let domain = [
            domain.first().copied().unwrap_or(0.0),
            domain.get(1).copied().unwrap_or(1.0),
        ];
        match document.entry(dict, "FunctionType").as_i64()? {
            2 => Some(Function::Exponential {
                start: Some(floats(document, dict.get("C0")))
                    .filter(|values| !values.is_empty())
                    .unwrap_or(vec![0.0]),
                end: Some(floats(document, dict.get("C1")))
                    .filter(|values| !values.is_empty())
                    .unwrap_or(vec![1.0]),
                exponent: document.entry(dict, "N").as_f64().unwrap_or(1.0) as f32,
            }),
            3 => Some(Function::Stitching {
                domain,
                functions: document
                    .entry(dict, "Functions")
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| Function::read(document, item, depth + 1))
                            .collect()
                    })
                    .unwrap_or_default(),
                bounds: floats(document, dict.get("Bounds")),
                encode: floats(document, dict.get("Encode")),
            }),
            0 => {
                Samples::read(document, &object).map(|samples| Function::Sampled(Box::new(samples)))
            }
            4 => {
                let Object::Stream(stream) = &object else {
                    return None;
                };
                let program = document.stream_data(stream).ok()?;
                Some(Function::Calculator(parse_calculator(&program)))
            }
            _ => None,
        }
    }

    pub fn evaluate(&self, inputs: &[f32]) -> Vec<f32> {
        let x = inputs.first().copied().unwrap_or(0.0);
        match self {
            Function::Exponential {
                start,
                end,
                exponent,
            } => {
                let t = x.max(0.0).powf(*exponent);
                start
                    .iter()
                    .zip(end)
                    .map(|(a, b)| a + t * (b - a))
                    .collect()
            }
            Function::Stitching {
                domain,
                functions,
                bounds,
                encode,
            } => {
                let x = x.clamp(domain[0], domain[1]);
                let index = bounds
                    .iter()
                    .take_while(|bound| x >= **bound)
                    .count()
                    .min(functions.len().saturating_sub(1));
                let low = if index == 0 {
                    domain[0]
                } else {
                    bounds[index - 1]
                };
                let high = bounds.get(index).copied().unwrap_or(domain[1]);
                let (e0, e1) = (
                    encode.get(index * 2).copied().unwrap_or(0.0),
                    encode.get(index * 2 + 1).copied().unwrap_or(1.0),
                );
                let t = if high > low {
                    e0 + (x - low) / (high - low) * (e1 - e0)
                } else {
                    e0
                };
                functions
                    .get(index)
                    .map_or_else(Vec::new, |function| function.evaluate(&[t]))
            }
            Function::Sampled(samples) => samples.evaluate(inputs),
            Function::Calculator(program) => run_calculator(program, inputs),
            Function::Each(functions) => functions
                .iter()
                .flat_map(|function| function.evaluate(inputs))
                .collect(),
        }
    }
}

/// A sampled function's table.
#[derive(Clone, Debug)]
pub struct Samples {
    /// How many samples along each input, the first varying fastest.
    sizes: Vec<usize>,
    domain: Vec<[f32; 2]>,
    encode: Vec<[f32; 2]>,
    outputs: usize,
    /// Each sample's outputs, decoded.
    values: Vec<f32>,
}

/// The most samples a table holds, so a damaged size can't ask for all memory.
const SAMPLES_MAX: usize = 1 << 22;
/// The most inputs interpolated between neighbours; others take the nearest sample below.
const INTERPOLATED_MAX: usize = 8;

fn pairs(values: &[f32], count: usize, default: impl Fn(usize) -> [f32; 2]) -> Vec<[f32; 2]> {
    (0..count)
        .map(|index| match values.get(index * 2..index * 2 + 2) {
            Some(pair) => [pair[0], pair[1]],
            None => default(index),
        })
        .collect()
}

impl Samples {
    fn read(document: &Document, object: &Object) -> Option<Self> {
        let Object::Stream(stream) = object else {
            return None;
        };
        let dict = &stream.dict;
        let sizes: Vec<usize> = floats(document, dict.get("Size"))
            .iter()
            .map(|size| (*size).max(1.0) as usize)
            .collect();
        let count = sizes
            .iter()
            .try_fold(1usize, |total, size| total.checked_mul(*size))?;
        let bits = document
            .entry(dict, "BitsPerSample")
            .as_i64()
            .unwrap_or(8)
            .clamp(1, 32) as usize;
        let range = floats(document, dict.get("Range"));
        let outputs = range.len() / 2;
        if sizes.is_empty() || outputs == 0 || count > SAMPLES_MAX {
            return None;
        }
        let domain = pairs(&floats(document, dict.get("Domain")), sizes.len(), |_| {
            [0.0, 1.0]
        });
        let encode = pairs(
            &floats(document, dict.get("Encode")),
            sizes.len(),
            |index| [0.0, (sizes[index] - 1) as f32],
        );
        let range = pairs(&range, outputs, |_| [0.0, 1.0]);
        let decode = pairs(&floats(document, dict.get("Decode")), outputs, |index| {
            range[index]
        });
        let data = document.stream_data(stream).ok()?;
        let max = ((1u64 << bits) - 1) as f32;
        let values = (0..count * outputs)
            .map(|index| {
                let [low, high] = decode[index % outputs];
                let [floor, ceiling] = range[index % outputs];
                (low + read_bits(&data, index * bits, bits) as f32 / max * (high - low))
                    .clamp(floor.min(ceiling), floor.max(ceiling))
            })
            .collect();
        Some(Self {
            sizes,
            domain,
            encode,
            outputs,
            values,
        })
    }

    /// Where `inputs` fall in the table: each input's sample below and its share of the way to
    /// the next.
    fn positions(&self, inputs: &[f32]) -> Vec<(usize, f32)> {
        self.sizes
            .iter()
            .enumerate()
            .map(|(index, &size)| {
                let [d0, d1] = self.domain[index];
                let [e0, e1] = self.encode[index];
                let x = inputs
                    .get(index)
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(d0.min(d1), d0.max(d1));
                let encoded = if d1 == d0 {
                    e0
                } else {
                    e0 + (x - d0) / (d1 - d0) * (e1 - e0)
                };
                let position = encoded.clamp(0.0, (size - 1) as f32);
                let below = (position.floor() as usize).min(size - 1);
                (below, position - below as f32)
            })
            .collect()
    }

    fn evaluate(&self, inputs: &[f32]) -> Vec<f32> {
        let positions = self.positions(inputs);
        // The inputs between two samples: each corner of the cell around the point weighs in.
        let between: Vec<usize> = (0..positions.len())
            .filter(|&index| positions[index].1 > 0.0 && positions[index].0 + 1 < self.sizes[index])
            .take(INTERPOLATED_MAX)
            .collect();
        let mut out = vec![0.0f32; self.outputs];
        for corner in 0..1usize << between.len() {
            let mut weight = 1.0f32;
            let mut sample = 0usize;
            let mut stride = 1usize;
            for (index, &(below, share)) in positions.iter().enumerate() {
                let step = between
                    .iter()
                    .position(|&known| known == index)
                    .map_or(0, |bit| (corner >> bit) & 1);
                weight *= match (between.contains(&index), step) {
                    (false, _) => 1.0,
                    (true, 0) => 1.0 - share,
                    (true, _) => share,
                };
                sample += (below + step) * stride;
                stride *= self.sizes[index];
            }
            for (output, value) in out.iter_mut().enumerate() {
                *value += weight
                    * self
                        .values
                        .get(sample * self.outputs + output)
                        .copied()
                        .unwrap_or(0.0);
            }
        }
        out
    }
}

/// `count` bits from bit `at`, most significant first.
fn read_bits(data: &[u8], at: usize, count: usize) -> u64 {
    (at..at + count).fold(0u64, |value, bit| {
        let set = data
            .get(bit / 8)
            .map_or(0, |byte| (byte >> (7 - bit % 8)) & 1);
        value << 1 | u64::from(set)
    })
}

// --- PostScript calculator functions -------------------------------------------------------

#[derive(Clone, Debug)]
pub enum Operation {
    Number(f32),
    Operator(String),
    /// `{…} if`, or `{…} {…} ifelse`.
    Branch(Vec<Operation>, Option<Vec<Operation>>),
}

fn parse_block(tokens: &mut std::iter::Peekable<std::vec::IntoIter<String>>) -> Vec<Operation> {
    let mut block = Vec::new();
    while let Some(token) = tokens.next() {
        match token.as_str() {
            "{" => {
                let inner = parse_block(tokens);
                if tokens.peek().map(String::as_str) == Some("{") {
                    tokens.next();
                    let other = parse_block(tokens);
                    tokens.next();
                    block.push(Operation::Branch(inner, Some(other)));
                } else {
                    tokens.next();
                    block.push(Operation::Branch(inner, None));
                }
            }
            "}" => return block,
            word => block.push(
                word.parse::<f32>()
                    .map_or_else(|_| Operation::Operator(word.to_owned()), Operation::Number),
            ),
        }
    }
    block
}

fn parse_calculator(program: &[u8]) -> Vec<Operation> {
    let text = String::from_utf8_lossy(program)
        .replace('{', " { ")
        .replace('}', " } ");
    let mut tokens = text
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>()
        .into_iter()
        .peekable();
    // The program is one block; its outer braces open it.
    if tokens.peek().map(String::as_str) == Some("{") {
        tokens.next();
    }
    parse_block(&mut tokens)
}

/// Runs a calculator program on `inputs`. Taking from an empty stack gives no number (NaN),
/// which spreads through what uses it; a program that does so gives no outputs, as a broken
/// function draws nothing.
fn run_calculator(program: &[Operation], inputs: &[f32]) -> Vec<f32> {
    let mut stack: Vec<f32> = inputs.to_vec();
    execute(program, &mut stack, 0);
    if stack.iter().any(|value| value.is_nan()) {
        return Vec::new();
    }
    stack
}

fn execute(program: &[Operation], stack: &mut Vec<f32>, depth: usize) {
    if depth > 32 {
        return;
    }
    for operation in program {
        match operation {
            Operation::Number(value) => stack.push(*value),
            Operation::Branch(then, otherwise) => {
                let condition = stack.pop().unwrap_or(f32::NAN) != 0.0;
                match (condition, otherwise) {
                    (true, _) => execute(then, stack, depth + 1),
                    (false, Some(otherwise)) => execute(otherwise, stack, depth + 1),
                    (false, None) => {}
                }
            }
            Operation::Operator(name) => operate(name, stack),
        }
        if stack.len() > 1000 {
            return;
        }
    }
}

fn flag(value: bool) -> f32 {
    if value {
        1.0
    } else {
        0.0
    }
}

/// `a` over `b`, or 0 when `b` is 0, as a calculator divides.
fn divided(a: f32, b: f32, divide: fn(f32, f32) -> f32) -> f32 {
    if b == 0.0 {
        0.0
    } else {
        divide(a, b)
    }
}

/// An operator of two operands, `None` when `name` is not one.
fn binary(name: &str, a: f32, b: f32) -> Option<f32> {
    Some(match name {
        "add" => a + b,
        "sub" => a - b,
        "mul" => a * b,
        "div" => divided(a, b, |a, b| a / b),
        "idiv" => divided(a, b, |a, b| (a / b).trunc()),
        "mod" => divided(a, b, |a, b| a % b),
        "exp" => a.powf(b),
        "atan" => a.atan2(b).to_degrees().rem_euclid(360.0),
        "eq" => flag(a == b),
        "ne" => flag(a != b),
        "gt" => flag(a > b),
        "ge" => flag(a >= b),
        "lt" => flag(a < b),
        "le" => flag(a <= b),
        "and" => flag(a != 0.0 && b != 0.0),
        "or" => flag(a != 0.0 || b != 0.0),
        "xor" => flag((a != 0.0) != (b != 0.0)),
        "bitshift" => ((a as i64) << (b as i64).clamp(-31, 31)) as f32,
        _ => return None,
    })
}

/// An operator of one operand, `None` when `name` is not one.
fn unary(name: &str, a: f32) -> Option<f32> {
    Some(match name {
        "neg" => -a,
        "abs" => a.abs(),
        "sqrt" => a.max(0.0).sqrt(),
        "sin" => a.to_radians().sin(),
        "cos" => a.to_radians().cos(),
        "ln" => a.ln(),
        "log" => a.log10(),
        "floor" => a.floor(),
        "ceiling" => a.ceil(),
        "round" => a.round(),
        "not" => flag(a == 0.0),
        "truncate" | "cvi" | "cvr" => a.trunc(),
        _ => return None,
    })
}

const BINARY: [&str; 18] = [
    "add", "sub", "mul", "div", "idiv", "mod", "exp", "atan", "eq", "ne", "gt", "ge", "lt", "le",
    "and", "or", "xor", "bitshift",
];

fn operate(name: &str, stack: &mut Vec<f32>) {
    let mut pop = || stack.pop().unwrap_or(f32::NAN);
    if BINARY.contains(&name) {
        let (b, a) = (pop(), pop());
        stack.extend(binary(name, a, b));
        return;
    }
    if unary(name, 0.0).is_some() {
        let a = pop();
        stack.extend(unary(name, a));
        return;
    }
    rearrange(name, stack);
}

/// The operators that move the stack's values about.
fn rearrange(name: &str, stack: &mut Vec<f32>) {
    let mut pop = || stack.pop().unwrap_or(f32::NAN);
    let result: Vec<f32> = match name {
        "true" => vec![1.0],
        "false" => vec![0.0],
        "dup" => {
            let a = pop();
            vec![a, a]
        }
        "exch" => {
            let (b, a) = (pop(), pop());
            vec![b, a]
        }
        "pop" => {
            pop();
            Vec::new()
        }
        "copy" => {
            let count = pop().max(0.0) as usize;
            stack[stack.len().saturating_sub(count)..].to_vec()
        }
        "index" => {
            let index = pop().max(0.0) as usize;
            vec![stack.iter().rev().nth(index).copied().unwrap_or(0.0)]
        }
        "roll" => {
            let pair = pop_pair(stack);
            roll(stack, pair);
            Vec::new()
        }
        _ => Vec::new(),
    };
    stack.extend(result);
}

/// The count and shift a `roll` takes off the stack.
fn pop_pair(stack: &mut Vec<f32>) -> (usize, i64) {
    let shift = stack.pop().unwrap_or(f32::NAN) as i64;
    let count = stack.pop().unwrap_or(f32::NAN).max(0.0) as usize;
    (count, shift)
}

fn roll(stack: &mut [f32], (count, shift): (usize, i64)) {
    let start = stack.len().saturating_sub(count);
    if count > 0 {
        stack[start..].rotate_right(shift.rem_euclid(count as i64) as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_turn_into_rgb() {
        assert_eq!(Space::Cmyk.rgb(&[0.0, 1.0, 1.0, 0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(Space::Gray.rgb(&[0.5]), [0.5; 3]);
        let white = lab_to_rgb(100.0, 0.0, 0.0);
        assert!(
            white.iter().all(|channel| (channel - 1.0).abs() < 0.01),
            "{white:?}"
        );
        let indexed = Space::Indexed {
            base: Box::new(Space::Rgb),
            high: 1,
            lookup: vec![0, 0, 0, 255, 0, 0],
        };
        assert_eq!(indexed.rgb(&[1.0]), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn functions_evaluate() {
        let exponential = Function::Exponential {
            start: vec![0.0],
            end: vec![1.0],
            exponent: 2.0,
        };
        assert_eq!(exponential.evaluate(&[0.5]), [0.25]);
        let program = parse_calculator(b"{ dup 0.5 gt { 1 sub } { 2 mul } ifelse exch pop 3 }");
        assert_eq!(run_calculator(&program, &[0.0, 0.25]), [0.5, 3.0]);
        let line = Samples {
            sizes: vec![2],
            domain: vec![[0.0, 1.0]],
            encode: vec![[0.0, 1.0]],
            outputs: 1,
            values: vec![0.0, 1.0],
        };
        assert_eq!(Function::Sampled(Box::new(line)).evaluate(&[0.25]), [0.25]);
        // Two inputs: a 2 × 2 table, bilinear between its corners.
        let square = Samples {
            sizes: vec![2, 2],
            domain: vec![[0.0, 1.0]; 2],
            encode: vec![[0.0, 1.0]; 2],
            outputs: 1,
            values: vec![0.0, 1.0, 1.0, 2.0],
        };
        assert_eq!(square.evaluate(&[0.5, 0.5]), [1.0]);
        assert_eq!(square.evaluate(&[1.0, 0.0]), [1.0]);
        // A broken calculator gives nothing.
        assert!(run_calculator(&parse_calculator(b"{ + }"), &[]).is_empty());
    }
}
