//! 3D files other than `.glb`, read as triangle meshes and written as `.glb` so the still
//! renderer (`model_still`) draws them like any model (ADR-0015): OBJ, STL (binary and text),
//! PLY (text and binary) and USD (`.usda` as text, `.usdc` binary through `usdc`, `.usd` either,
//! and a `.usdz` package holding either).
//!
//! A mesh keeps its triangles, a colour for each corner when the file gives one (turned from
//! sRGB to the linear light glTF expects, except USD's, which are linear already), and smooth
//! normals worked out from its faces. STL is turned from Z-up to the Y-up glTF expects, as CAD
//! programs write it; USD is turned when it says it is Z-up.
//!
//! Alembic (`.abc`) is read through `alembic`; its faces wind the other way, so they are turned.
//!
//! Left out: textures, materials beyond one colour each, and transforms in USD and Alembic.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The folder in the app's caches that holds the `.glb` each converted file became.
const CONVERTED_FOLDER: &str = "model-meshes";
/// The most triangles read, so a damaged count can't ask for all memory.
const TRIANGLES_MAX: usize = 20_000_000;
/// The colour of a mesh with none of its own: a light clay grey.
const CLAY: [f32; 4] = [0.8, 0.8, 0.78, 1.0];

/// A triangle mesh: corner positions, a colour per corner if the file gives colours, and the
/// triangles as three corners each.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub colours: Option<Vec<[f32; 3]>>,
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// Adds another mesh, its corners after this one's. When only one has colours, the other's
    /// corners take the clay grey.
    fn add(&mut self, other: Mesh) {
        let offset = self.positions.len() as u32;
        let clay = [CLAY[0], CLAY[1], CLAY[2]];
        match (&mut self.colours, other.colours) {
            (Some(mine), Some(theirs)) => mine.extend(theirs),
            (Some(mine), None) => mine.extend(std::iter::repeat_n(clay, other.positions.len())),
            (None, Some(theirs)) => {
                let mut colours = vec![clay; self.positions.len()];
                colours.extend(theirs);
                self.colours = Some(colours);
            }
            (None, None) => {}
        }
        self.positions.extend(other.positions);
        self.triangles.extend(
            other
                .triangles
                .into_iter()
                .map(|triangle| triangle.map(|corner| corner + offset)),
        );
    }

    /// Drops triangles that name a corner the mesh doesn't have.
    fn checked(mut self) -> Self {
        let count = self.positions.len() as u32;
        self.triangles
            .retain(|triangle| triangle.iter().all(|corner| *corner < count));
        self
    }

    /// Each corner's normal: the sum of its faces' normals, weighted by their area.
    fn normals(&self) -> Vec<[f32; 3]> {
        let mut normals = vec![[0.0f32; 3]; self.positions.len()];
        for triangle in &self.triangles {
            let [a, b, c] = triangle.map(|corner| self.positions[corner as usize]);
            let (u, v) = (sub(b, a), sub(c, a));
            let face = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            for corner in triangle {
                let normal = &mut normals[*corner as usize];
                for axis in 0..3 {
                    normal[axis] += face[axis];
                }
            }
        }
        normals
            .into_iter()
            .map(|normal| {
                let length =
                    (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
                if length > 0.0 {
                    normal.map(|axis| axis / length)
                } else {
                    [0.0, 1.0, 0.0]
                }
            })
            .collect()
    }

    /// The mesh turned from Z-up to Y-up.
    fn y_up(mut self) -> Self {
        for position in &mut self.positions {
            *position = [position[0], position[2], -position[1]];
        }
        self
    }
}

/// An sRGB channel from 0 to 1 in linear light, as glTF's vertex colours are.
fn linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Splits a polygon into a fan of triangles.
fn fan(corners: &[u32], triangles: &mut Vec<[u32; 3]>) {
    for pair in corners.windows(2).skip(1) {
        if triangles.len() < TRIANGLES_MAX {
            triangles.push([corners[0], pair[0], pair[1]]);
        }
    }
}

fn numbers<'a>(words: impl Iterator<Item = &'a str>) -> Vec<f32> {
    words.filter_map(|word| word.parse::<f32>().ok()).collect()
}

// --- STL -----------------------------------------------------------------------------------

/// A binary STL's size for its triangle count: an 80-byte header, the count, 50 bytes each.
fn is_binary_stl(bytes: &[u8]) -> bool {
    let count = bytes.get(80..84).map_or(0, |four| {
        u32::from_le_bytes(four.try_into().expect("four bytes")) as usize
    });
    bytes.len() == 84 + count * 50
}

fn read_binary_stl(bytes: &[u8]) -> Mesh {
    let mut mesh = Mesh::default();
    for record in bytes[84..].chunks(50).take(TRIANGLES_MAX) {
        let corner = |index: usize| -> [f32; 3] {
            std::array::from_fn(|axis| {
                let at = 12 + index * 12 + axis * 4;
                f32::from_le_bytes(record[at..at + 4].try_into().expect("four bytes"))
            })
        };
        let first = mesh.positions.len() as u32;
        mesh.positions.extend([corner(0), corner(1), corner(2)]);
        mesh.triangles.push([first, first + 1, first + 2]);
    }
    mesh
}

fn read_text_stl(text: &str) -> Mesh {
    let mut mesh = Mesh::default();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if words.next() != Some("vertex") {
            continue;
        }
        if let [x, y, z] = numbers(words)[..] {
            mesh.positions.push([x, y, z]);
        }
    }
    let triangles = (mesh.positions.len() / 3).min(TRIANGLES_MAX) as u32;
    mesh.triangles = (0..triangles)
        .map(|index| [index * 3, index * 3 + 1, index * 3 + 2])
        .collect();
    mesh
}

pub fn read_stl(bytes: &[u8]) -> Mesh {
    let mesh = if is_binary_stl(bytes) {
        read_binary_stl(bytes)
    } else {
        read_text_stl(&String::from_utf8_lossy(bytes))
    };
    mesh.y_up()
}

// --- OBJ -----------------------------------------------------------------------------------

/// A face corner's position index: OBJ counts from 1, and from the end when negative.
fn obj_index(word: &str, count: usize) -> Option<u32> {
    let index: i64 = word.split('/').next()?.parse().ok()?;
    let resolved = if index < 0 {
        count as i64 + index
    } else {
        index - 1
    };
    u32::try_from(resolved).ok()
}

/// A vertex line's position, and its colour when the writer added one after it.
fn obj_vertex(words: std::str::SplitWhitespace) -> Option<([f32; 3], Option<[f32; 3]>)> {
    match numbers(words)[..] {
        [x, y, z, r, g, b] => Some(([x, y, z], Some([r, g, b].map(linear)))),
        [x, y, z, ..] => Some(([x, y, z], None)),
        _ => None,
    }
}

pub fn read_obj(text: &str) -> Mesh {
    let mut mesh = Mesh::default();
    let mut colours: Vec<[f32; 3]> = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => {
                let Some((position, colour)) = obj_vertex(words) else {
                    continue;
                };
                mesh.positions.push(position);
                colours.extend(colour);
            }
            Some("f") => {
                let count = mesh.positions.len();
                let corners: Vec<u32> = words.filter_map(|word| obj_index(word, count)).collect();
                fan(&corners, &mut mesh.triangles);
            }
            _ => {}
        }
    }
    if !colours.is_empty() && colours.len() == mesh.positions.len() {
        mesh.colours = Some(colours);
    }
    mesh.checked()
}

// --- PLY -----------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Text,
    Little,
    Big,
}

/// A property's type, by the size of one value.
#[derive(Clone, Copy)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }
}

/// A property: a value, or a list with its count's type and its items' type.
#[derive(Clone)]
struct Property {
    name: String,
    scalar: Scalar,
    count: Option<Scalar>,
}

struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

/// Reads values one at a time from a PLY body, as text words or binary numbers.
struct Values<'a> {
    bytes: &'a [u8],
    at: usize,
    encoding: Encoding,
}

impl Values<'_> {
    fn next(&mut self, scalar: Scalar) -> Option<f64> {
        if self.encoding == Encoding::Text {
            return self.next_word();
        }
        let raw = self.bytes.get(self.at..self.at + scalar.size())?;
        self.at += scalar.size();
        let mut buffer = [0u8; 8];
        buffer[..raw.len()].copy_from_slice(raw);
        if self.encoding == Encoding::Big {
            buffer[..raw.len()].reverse();
        }
        let le = |count: usize| -> [u8; 8] {
            let mut padded = [0u8; 8];
            padded[..count].copy_from_slice(&buffer[..count]);
            padded
        };
        Some(match scalar {
            Scalar::I8 => f64::from(buffer[0] as i8),
            Scalar::U8 => f64::from(buffer[0]),
            Scalar::I16 => f64::from(i16::from_le_bytes([buffer[0], buffer[1]])),
            Scalar::U16 => f64::from(u16::from_le_bytes([buffer[0], buffer[1]])),
            Scalar::I32 => f64::from(i32::from_le_bytes(le(4)[..4].try_into().ok()?)),
            Scalar::U32 => f64::from(u32::from_le_bytes(le(4)[..4].try_into().ok()?)),
            Scalar::F32 => f64::from(f32::from_le_bytes(le(4)[..4].try_into().ok()?)),
            Scalar::F64 => f64::from_le_bytes(buffer),
        })
    }

    fn next_word(&mut self) -> Option<f64> {
        let rest = self.bytes.get(self.at..)?;
        let start = rest.iter().position(|byte| !byte.is_ascii_whitespace())?;
        let end = rest[start..]
            .iter()
            .position(u8::is_ascii_whitespace)
            .map_or(rest.len(), |length| start + length);
        self.at += end;
        std::str::from_utf8(&rest[start..end]).ok()?.parse().ok()
    }
}

/// The header's elements and encoding, and where the body starts.
fn read_ply_header(bytes: &[u8]) -> Option<(Vec<Element>, Encoding, usize)> {
    let end = bytes
        .windows(10)
        .position(|window| window == b"end_header")?;
    let body = end + 10 + bytes[end + 10..].iter().position(|byte| *byte == b'\n')? + 1;
    let header = std::str::from_utf8(&bytes[..end]).ok()?;
    let mut encoding = Encoding::Text;
    let mut elements: Vec<Element> = Vec::new();
    for line in header.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[..] {
            ["format", "binary_little_endian", ..] => encoding = Encoding::Little,
            ["format", "binary_big_endian", ..] => encoding = Encoding::Big,
            ["element", name, count] => elements.push(Element {
                name: name.to_owned(),
                count: count.parse().ok()?,
                properties: Vec::new(),
            }),
            ["property", "list", count, scalar, name] => {
                elements.last_mut()?.properties.push(Property {
                    name: name.to_owned(),
                    scalar: Scalar::named(scalar)?,
                    count: Some(Scalar::named(count)?),
                })
            }
            ["property", scalar, name] => elements.last_mut()?.properties.push(Property {
                name: name.to_owned(),
                scalar: Scalar::named(scalar)?,
                count: None,
            }),
            _ => {}
        }
    }
    Some((elements, encoding, body))
}

/// One element's values: each property's value, or its list.
fn read_ply_item(
    values: &mut Values,
    properties: &[Property],
) -> Option<HashMap<String, Vec<f64>>> {
    let mut item = HashMap::new();
    for property in properties {
        let read = match property.count {
            None => vec![values.next(property.scalar)?],
            Some(count) => {
                let length = values.next(count)? as usize;
                (0..length.min(1024))
                    .map(|_| values.next(property.scalar))
                    .collect::<Option<Vec<f64>>>()?
            }
        };
        item.insert(property.name.clone(), read);
    }
    Some(item)
}

fn take_vertex(mesh: &mut Mesh, colours: &mut Vec<[f32; 3]>, item: &HashMap<String, Vec<f64>>) {
    let value = |name: &str| item.get(name).and_then(|values| values.first()).copied();
    let (Some(x), Some(y), Some(z)) = (value("x"), value("y"), value("z")) else {
        return;
    };
    mesh.positions.push([x as f32, y as f32, z as f32]);
    if let (Some(r), Some(g), Some(b)) = (value("red"), value("green"), value("blue")) {
        colours.push([r, g, b].map(|channel| linear(channel as f32 / 255.0)));
    }
}

fn take_face(mesh: &mut Mesh, item: &HashMap<String, Vec<f64>>) {
    let corners: Vec<u32> = item
        .get("vertex_indices")
        .or_else(|| item.get("vertex_index"))
        .map(|list| list.iter().map(|index| *index as u32).collect())
        .unwrap_or_default();
    fan(&corners, &mut mesh.triangles);
}

pub fn read_ply(bytes: &[u8]) -> Mesh {
    let Some((elements, encoding, body)) = read_ply_header(bytes) else {
        return Mesh::default();
    };
    let mut values = Values {
        bytes,
        at: body,
        encoding,
    };
    let mut mesh = Mesh::default();
    let mut colours = Vec::new();
    let items = elements
        .iter()
        .flat_map(|element| std::iter::repeat_n(element, element.count.min(TRIANGLES_MAX)));
    for element in items {
        let Some(item) = read_ply_item(&mut values, &element.properties) else {
            break;
        };
        match element.name.as_str() {
            "vertex" => take_vertex(&mut mesh, &mut colours, &item),
            "face" => take_face(&mut mesh, &item),
            _ => {}
        }
    }
    if !colours.is_empty() && colours.len() == mesh.positions.len() {
        mesh.colours = Some(colours);
    }
    mesh.checked()
}

// --- USD as text ---------------------------------------------------------------------------

/// The numbers in the bracketed list after `name =` in `block`.
fn usd_list(block: &str, name: &str) -> Vec<f32> {
    let Some(start) = block.find(&format!("{name} = [")) else {
        return Vec::new();
    };
    let rest = &block[start..];
    let open = rest.find('[').unwrap_or(0);
    let close = rest.find(']').unwrap_or(rest.len());
    numbers(
        rest[open + 1..close]
            .split(|character: char| {
                !(character.is_ascii_digit() || matches!(character, '.' | '-' | 'e' | 'E' | '+'))
            })
            .filter(|word| !word.is_empty()),
    )
}

/// The `{ … }` block that follows `from` in `text`, braces matched.
fn usd_block(text: &str, from: usize) -> &str {
    let Some(open) = text[from..].find('{').map(|at| from + at) else {
        return "";
    };
    let mut depth = 0;
    for (index, character) in text[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open..open + index + 1];
                }
            }
            _ => {}
        }
    }
    &text[open..]
}

/// Each material's diffuse colour, by its name.
fn usd_materials(text: &str) -> HashMap<String, [f32; 3]> {
    let mut found = HashMap::new();
    for (at, _) in text.match_indices("def Material \"") {
        let name: String = text[at + 14..].chars().take_while(|c| *c != '"').collect();
        let block = usd_block(text, at);
        let Some(colour) = block.find("inputs:diffuseColor = (") else {
            continue;
        };
        let values = numbers(
            block[colour + 23..]
                .split([',', ')', ' '])
                .filter(|word| !word.is_empty())
                .take(3),
        );
        if let [r, g, b, ..] = values[..] {
            found.insert(name, [r, g, b]);
        }
    }
    found
}

fn usd_mesh(block: &str, materials: &HashMap<String, [f32; 3]>) -> Mesh {
    let points = usd_list(block, "points");
    let counts = usd_list(block, "faceVertexCounts");
    let indices = usd_list(block, "faceVertexIndices");
    let positions: Vec<[f32; 3]> = points.as_chunks::<3>().0.to_vec();
    let mut mesh = Mesh {
        positions,
        ..Mesh::default()
    };
    let mut at = 0;
    for count in counts {
        let count = count as usize;
        let corners: Vec<u32> = indices
            .iter()
            .skip(at)
            .take(count)
            .map(|index| *index as u32)
            .collect();
        fan(&corners, &mut mesh.triangles);
        at += count;
    }
    let bound = block
        .find("rel material:binding = <")
        .and_then(|at| block[at..].split('>').next())
        .and_then(|path| path.rsplit('/').next())
        .and_then(|name| materials.get(name));
    if let Some(colour) = bound {
        mesh.colours = Some(vec![*colour; mesh.positions.len()]);
    }
    mesh.checked()
}

pub fn read_usda(text: &str) -> Mesh {
    let materials = usd_materials(text);
    let mut mesh = Mesh::default();
    for (at, _) in text.match_indices("def Mesh ") {
        mesh.add(usd_mesh(usd_block(text, at), &materials));
    }
    let z_up = text
        .lines()
        .take(40)
        .any(|line| line.trim().replace(' ', "") == "upAxis=\"Z\"");
    if z_up {
        mesh.y_up()
    } else {
        mesh
    }
}

/// USD in either form, told apart by its first bytes.
fn read_usd(bytes: &[u8]) -> Option<Mesh> {
    if !bytes.starts_with(b"PXR-USDC") {
        return Some(read_usda(&String::from_utf8_lossy(bytes)));
    }
    let (shapes, z_up) = crate::usdc::read_shapes(bytes)
        .map_err(|error| eprintln!("the USD file was not read: {}", error.0))
        .ok()?;
    let mut mesh = Mesh::default();
    for shape in shapes {
        let mut part = Mesh {
            positions: shape.points,
            ..Mesh::default()
        };
        let mut at = 0;
        for count in shape.counts {
            let count = usize::try_from(count).unwrap_or(0);
            let corners: Vec<u32> = shape
                .corners
                .iter()
                .skip(at)
                .take(count)
                .map(|corner| *corner as u32)
                .collect();
            fan(&corners, &mut part.triangles);
            at += count;
        }
        if let Some(colour) = shape.colour {
            part.colours = Some(vec![colour; part.positions.len()]);
        }
        mesh.add(part.checked());
    }
    Some(if z_up { mesh.y_up() } else { mesh })
}

/// An Alembic file's meshes, their faces turned to wind as glTF expects.
fn read_alembic(bytes: &[u8]) -> Option<Mesh> {
    let shapes = crate::alembic::read_shapes(bytes)
        .map_err(|error| eprintln!("the Alembic file was not read: {}", error.0))
        .ok()?;
    let mut mesh = Mesh::default();
    for shape in shapes {
        let mut part = Mesh {
            positions: shape.points,
            ..Mesh::default()
        };
        let mut at = 0;
        for count in shape.counts {
            let count = usize::try_from(count).unwrap_or(0);
            let corners: Vec<u32> = shape
                .corners
                .iter()
                .skip(at)
                .take(count)
                .map(|corner| *corner as u32)
                .collect();
            fan(&corners, &mut part.triangles);
            at += count;
        }
        for triangle in &mut part.triangles {
            triangle.swap(1, 2);
        }
        mesh.add(part.checked());
    }
    Some(mesh)
}

/// The root layer of a `.usdz` package: its first USD file.
fn usdz_layer(bytes: &[u8]) -> Option<Vec<u8>> {
    let archive = crate::package::ZipArchive::open(bytes).ok()?;
    let name = archive
        .entry_names()
        .find(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".usdc") || lower.ends_with(".usda") || lower.ends_with(".usd")
        })?
        .to_owned();
    archive
        .read_entry(&name, crate::package::PART_MAX_BYTES)
        .ok()?
}

// --- glTF ----------------------------------------------------------------------------------

fn bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    positions
        .iter()
        .fold(([f32::MAX; 3], [f32::MIN; 3]), |(low, high), position| {
            (
                std::array::from_fn(|axis| low[axis].min(position[axis])),
                std::array::from_fn(|axis| high[axis].max(position[axis])),
            )
        })
}

/// The mesh as a binary glTF: one node, one mesh, one primitive with positions, normals,
/// colours when it has them, and indices, in a single buffer.
pub fn write_glb(mesh: &Mesh) -> Vec<u8> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut add = |bytes: Vec<u8>, target: u32| {
        let offset = buffer.len();
        buffer.extend_from_slice(&bytes);
        while !buffer.len().is_multiple_of(4) {
            buffer.push(0);
        }
        views.push(format!(
            r#"{{"buffer":0,"byteOffset":{offset},"byteLength":{},"target":{target}}}"#,
            bytes.len()
        ));
        views.len() - 1
    };
    let floats = |values: &[[f32; 3]]| {
        values
            .iter()
            .flatten()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<u8>>()
    };
    let count = mesh.positions.len();
    let positions = add(floats(&mesh.positions), 34962);
    let normals = add(floats(&mesh.normals()), 34962);
    let colours = mesh
        .colours
        .as_ref()
        .map(|colours| add(floats(colours), 34962));
    let indices = add(
        mesh.triangles
            .iter()
            .flatten()
            .flat_map(|index| index.to_le_bytes())
            .collect(),
        34963,
    );
    let (low, high) = bounds(&mesh.positions);
    let vector = |values: [f32; 3]| format!("[{},{},{}]", values[0], values[1], values[2]);
    let mut accessors = vec![
        format!(
            r#"{{"bufferView":{positions},"componentType":5126,"count":{count},"type":"VEC3","min":{},"max":{}}}"#,
            vector(low),
            vector(high)
        ),
        format!(r#"{{"bufferView":{normals},"componentType":5126,"count":{count},"type":"VEC3"}}"#),
        format!(
            r#"{{"bufferView":{indices},"componentType":5125,"count":{},"type":"SCALAR"}}"#,
            mesh.triangles.len() * 3
        ),
    ];
    let mut attributes = r#""POSITION":0,"NORMAL":1"#.to_owned();
    if let Some(view) = colours {
        accessors.push(format!(
            r#"{{"bufferView":{view},"componentType":5126,"count":{count},"type":"VEC3"}}"#
        ));
        attributes.push_str(r#","COLOR_0":3"#);
    }
    let base = if colours.is_some() { [1.0; 4] } else { CLAY };
    let json = format!(
        r#"{{"asset":{{"version":"2.0","generator":"Northroom"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],"materials":[{{"pbrMetallicRoughness":{{"baseColorFactor":[{},{},{},{}],"metallicFactor":0,"roughnessFactor":0.6}}}}],"meshes":[{{"primitives":[{{"attributes":{{{attributes}}},"indices":2,"material":0,"mode":4}}]}}],"buffers":[{{"byteLength":{}}}],"bufferViews":[{}],"accessors":[{}]}}"#,
        base[0],
        base[1],
        base[2],
        base[3],
        buffer.len(),
        views.join(","),
        accessors.join(",")
    );
    let mut json = json.into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + buffer.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&json);
    glb.extend_from_slice(&(buffer.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&buffer);
    glb
}

// --- Files ---------------------------------------------------------------------------------

/// Whether a file extension is one read here.
pub fn is_mesh_extension(extension: &str) -> bool {
    matches!(
        extension,
        "obj" | "stl" | "ply" | "usda" | "usdc" | "usd" | "usdz" | "abc"
    )
}

/// The mesh the file at `path` holds; `None` when it holds none this reader draws.
pub fn read(path: &Path) -> Option<Mesh> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let bytes = std::fs::read(path).ok()?;
    let mesh = match extension.as_str() {
        "stl" => read_stl(&bytes),
        "obj" => read_obj(&String::from_utf8_lossy(&bytes)),
        "ply" => read_ply(&bytes),
        "usda" | "usdc" | "usd" => read_usd(&bytes)?,
        "usdz" => read_usd(&usdz_layer(&bytes)?)?,
        "abc" => read_alembic(&bytes)?,
        _ => return None,
    };
    (!mesh.triangles.is_empty()).then_some(mesh)
}

/// The `.glb` the still renderer draws for the model at `path`: the file itself when it is one,
/// else the file converted, kept in the app's caches by the file's path and version.
pub fn glb_for(path: &Path) -> Option<PathBuf> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if extension == "glb" {
        return Some(path.to_path_buf());
    }
    if !is_mesh_extension(&extension) || cfg!(test) {
        return None;
    }
    convert_into(path, &crate::paths::get().caches.join(CONVERTED_FOLDER))
}

fn convert_into(path: &Path, folder: &Path) -> Option<PathBuf> {
    let stat = std::fs::metadata(path).ok()?;
    let modified = stat
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    let of_path = crate::digest::sha256_hex(path.to_string_lossy().as_bytes());
    let of_version = crate::digest::sha256_hex(format!("{}\n{modified}", stat.len()).as_bytes());
    let file = folder.join(format!("{}-{}.glb", &of_path[..16], &of_version[..16]));
    if file.is_file() {
        return Some(file);
    }
    let glb = write_glb(&read(path)?);
    std::fs::create_dir_all(folder).ok()?;
    // Earlier versions of this file are not needed any more.
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(&of_path[..16])
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    std::fs::write(&file, glb).ok()?;
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_obj_reads_its_faces_as_triangles_counting_from_one_or_from_the_end() {
        let mesh = read_obj(
            "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1/1/1 2/2/2 3/3/3 4/4/4\nf -4 -3 -2\nf 1 2 9\n",
        );
        assert_eq!(mesh.triangles, [[0, 1, 2], [0, 2, 3], [0, 1, 2]]);
        assert_eq!(mesh.colours, None);
    }

    #[test]
    fn a_text_stl_reads_and_turns_y_up() {
        let mesh = read_stl(b"solid x\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 2 3\nendloop\nendfacet\nendsolid\n");
        assert_eq!(
            mesh.positions,
            [[0.0, 0.0, -0.0], [1.0, 0.0, -0.0], [0.0, 3.0, -2.0]]
        );
        assert_eq!(mesh.triangles, [[0, 1, 2]]);
    }

    #[test]
    fn a_binary_ply_reads_its_vertices_colours_and_faces() {
        let mut bytes = b"ply\nformat binary_little_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n".to_vec();
        for (x, colour) in [(0.0f32, 255u8), (1.0, 0), (2.0, 51)] {
            for value in [x, 0.0, 0.0] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&[colour, colour, colour]);
        }
        bytes.push(3);
        for index in [0i32, 1, 2] {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        let mesh = read_ply(&bytes);
        assert_eq!(mesh.positions[2], [2.0, 0.0, 0.0]);
        assert_eq!(mesh.triangles, [[0, 1, 2]]);
        // 51 of 255 is 0.2 in sRGB, about 0.033 in linear light.
        let grey = mesh.colours.expect("colours")[2][0];
        assert!((grey - 0.0331).abs() < 0.001, "{grey}");
    }

    #[test]
    fn a_glb_holds_its_json_and_buffer_aligned() {
        let mesh = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            colours: None,
            triangles: vec![[0, 1, 2]],
        };
        let glb = write_glb(&mesh);
        assert!(glb.starts_with(b"glTF"));
        assert_eq!(
            u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
            glb.len()
        );
        let json_length = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        assert_eq!(json_length % 4, 0);
        let json = std::str::from_utf8(&glb[20..20 + json_length]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json.trim_end()).expect("JSON");
        let max: Vec<f64> = parsed["accessors"][0]["max"]
            .as_array()
            .expect("a bound")
            .iter()
            .filter_map(serde_json::Value::as_f64)
            .collect();
        assert_eq!(max, [1.0, 1.0, 0.0]);
        assert_eq!(&glb[20 + json_length + 4..20 + json_length + 8], b"BIN\0");
        // The normal faces up the Z axis.
        assert_eq!(mesh.normals()[0], [0.0, 0.0, 1.0]);
    }
}
