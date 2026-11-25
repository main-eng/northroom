//! Reads the meshes in USD's binary form, the crate file (`.usdc`, and the layer inside most
//! `.usdz` packages), for `mesh` to draw (ADR-0015). Written from OpenUSD's own reader
//! (`pxr/usd/sdf/crateFile.cpp`, `integerCoding.cpp`, `tf/fastCompression.cpp`).
//!
//! A crate file holds tables (tokens, fields, field sets, paths, specs) and the values they
//! point to. A mesh is a prim of type `Mesh` whose `points`, `faceVertexCounts` and
//! `faceVertexIndices` attributes give its shape; its colour is the `inputs:diffuseColor` of the
//! material its `material:binding` names. Files from crate version 0.4 on are read: OpenUSD has
//! written those since 2017.
//!
//! Left out: transforms, time samples (a mesh shows its default shape), references to other
//! layers, and every other kind of prim.

use std::collections::HashMap;

/// Why a crate file can't be read; the words are for logs and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrateError(pub String);

type Result<T> = std::result::Result<T, CrateError>;

fn fail<T>(why: &str) -> Result<T> {
    Err(CrateError(why.to_owned()))
}

const SIGNATURE: &[u8] = b"PXR-USDC";
/// The oldest version read: compressed tables began with 0.4.0.
const OLDEST: (u8, u8) = (0, 4);
/// Arrays begin with a 64-bit count from 0.7.0, a 32-bit one before.
const WIDE_COUNTS: (u8, u8) = (0, 7);
/// Integer arrays shorter than this are stored plainly even when marked compressed.
const COMPRESSED_ARRAY_MIN: usize = 16;
/// The most items any table or array is read with, so a damaged count can't ask for all memory.
const ITEMS_MAX: usize = 100_000_000;
/// A field set's end marker.
const END_OF_SET: u32 = u32::MAX;

/// Spec types: a prim, the layer itself (where its metadata is), an attribute, a relationship.
const PRIM: u32 = 6;
const PSEUDO_ROOT: u32 = 7;

/// Value types, as the crate numbers them.
const TOKEN: u8 = 11;
const VEC3F: u8 = 24;
const INT: u8 = 3;
const PATH_LIST_OP: u8 = 34;

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn size_at(bytes: &[u8], at: usize) -> Result<usize> {
    match u64_at(bytes, at).map(|value| value as usize) {
        Some(value) if value <= ITEMS_MAX => Ok(value),
        Some(_) => fail("A count is larger than this reader reads."),
        None => fail("The file ends early."),
    }
}

// --- Compression ---------------------------------------------------------------------------

/// An LZ4 length: the token's four bits, and when they are all set, bytes added until one is
/// not 255.
fn lz4_length(input: &[u8], at: &mut usize, start: usize) -> Result<usize> {
    let mut length = start;
    if length != 15 {
        return Ok(length);
    }
    while let Some(&byte) = input.get(*at) {
        *at += 1;
        length += usize::from(byte);
        if byte != 255 {
            return Ok(length);
        }
    }
    fail("An LZ4 length runs past its block.")
}

/// Copies `count` bytes from `back` before the end of `out`, which may overlap what it adds.
fn lz4_match(out: &mut Vec<u8>, back: usize, count: usize) -> Result<()> {
    if back == 0 || back > out.len() || out.len() + count > ITEMS_MAX {
        return fail("An LZ4 match reaches outside its output.");
    }
    let start = out.len() - back;
    for index in 0..count {
        out.push(out[start + index]);
    }
    Ok(())
}

/// Undoes one LZ4 block: runs of literals, each followed by a match except the last.
pub fn lz4_block(input: &[u8], capacity: usize) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(capacity);
    let mut at = 0;
    while let Some(&token) = input.get(at) {
        at += 1;
        let literals = lz4_length(input, &mut at, usize::from(token >> 4))?;
        let Some(copied) = input.get(at..at + literals) else {
            return fail("LZ4 literals run past their block.");
        };
        out.extend_from_slice(copied);
        at += literals;
        let Some(offset) = input.get(at..at + 2) else {
            break;
        };
        at += 2;
        let count = lz4_length(input, &mut at, usize::from(token & 0x0f))? + 4;
        lz4_match(
            &mut out,
            usize::from(u16::from_le_bytes([offset[0], offset[1]])),
            count,
        )?;
    }
    Ok(out)
}

/// Undoes OpenUSD's fast compression: one LZ4 block, or a count of blocks each with its size.
fn decompress(input: &[u8], capacity: usize) -> Result<Vec<u8>> {
    let Some((&chunks, rest)) = input.split_first() else {
        return fail("A compressed buffer is empty.");
    };
    if chunks == 0 {
        return lz4_block(rest, capacity);
    }
    let mut out = Vec::with_capacity(capacity);
    let mut at = 0;
    for _ in 0..chunks {
        let size = u32_at(rest, at)
            .ok_or_else(|| CrateError("A chunk's size is missing.".into()))?
            as usize;
        let Some(chunk) = rest.get(at + 4..at + 4 + size) else {
            return fail("A chunk runs past its buffer.");
        };
        out.extend(lz4_block(chunk, capacity)?);
        at += 4 + size;
    }
    Ok(out)
}

/// Undoes OpenUSD's integer coding of `count` 32-bit integers: a common value, a 2-bit code for
/// each (common, or a difference in 1, 2 or 4 bytes), then the differences.
fn decode_integers(data: &[u8], count: usize) -> Result<Vec<i32>> {
    let common =
        u32_at(data, 0).ok_or_else(|| CrateError("Coded integers end early.".into()))? as i32;
    let codes_at = 4;
    let mut values_at = codes_at + (count * 2).div_ceil(8);
    let mut previous: i32 = 0;
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let code = (data.get(codes_at + index / 4).copied().unwrap_or(0) >> (2 * (index % 4))) & 3;
        let (step, size) = match code {
            0 => (common, 0),
            1 => (i32::from(*data.get(values_at).unwrap_or(&0) as i8), 1),
            2 => (
                i32::from(i16::from_le_bytes([
                    *data.get(values_at).unwrap_or(&0),
                    *data.get(values_at + 1).unwrap_or(&0),
                ])),
                2,
            ),
            _ => (u32_at(data, values_at).unwrap_or(0) as i32, 4),
        };
        values_at += size;
        previous = previous.wrapping_add(step);
        out.push(previous);
    }
    Ok(out)
}

/// Compressed integers at `at`: their compressed size, then the compressed coding. Returns them
/// and where the next thing starts.
fn compressed_integers(bytes: &[u8], at: usize, count: usize) -> Result<(Vec<i32>, usize)> {
    let size = size_at(bytes, at)?;
    let Some(input) = bytes.get(at + 8..at + 8 + size) else {
        return fail("Compressed integers run past the file.");
    };
    let coded = decompress(input, 4 + count * 5)?;
    Ok((decode_integers(&coded, count)?, at + 8 + size))
}

// --- Tables --------------------------------------------------------------------------------

/// A value's representation: its type, whether it is an array, inlined or compressed, and its
/// payload (the value itself when inlined, else where it is).
#[derive(Clone, Copy, Debug)]
struct Rep(u64);

impl Rep {
    fn is_array(self) -> bool {
        self.0 & (1 << 63) != 0
    }
    fn is_inlined(self) -> bool {
        self.0 & (1 << 62) != 0
    }
    fn is_compressed(self) -> bool {
        self.0 & (1 << 61) != 0
    }
    fn kind(self) -> u8 {
        ((self.0 >> 48) & 0xff) as u8
    }
    fn payload(self) -> u64 {
        self.0 & ((1 << 48) - 1)
    }
}

struct Spec {
    path: usize,
    field_set: usize,
    kind: u32,
}

/// A crate file's tables, read.
struct Crate<'a> {
    bytes: &'a [u8],
    wide_counts: bool,
    tokens: Vec<String>,
    fields: Vec<(usize, Rep)>,
    field_sets: Vec<u32>,
    paths: Vec<String>,
    specs: Vec<Spec>,
}

/// The sections the table of contents names, by name.
fn sections(bytes: &[u8]) -> Result<HashMap<String, usize>> {
    let table = u64_at(bytes, 16)
        .ok_or_else(|| CrateError("The file has no table of contents.".into()))?
        as usize;
    let count = size_at(bytes, table)?;
    let mut found = HashMap::new();
    for index in 0..count.min(64) {
        let at = table + 8 + index * 32;
        let Some(name) = bytes.get(at..at + 16) else {
            return fail("The table of contents ends early.");
        };
        let name: String = name
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| char::from(*byte))
            .collect();
        found.insert(name, u64_at(bytes, at + 16).unwrap_or(0) as usize);
    }
    Ok(found)
}

fn read_tokens(bytes: &[u8], at: usize) -> Result<Vec<String>> {
    let count = size_at(bytes, at)?;
    let size = size_at(bytes, at + 8)?;
    let compressed = size_at(bytes, at + 16)?;
    let Some(input) = bytes.get(at + 24..at + 24 + compressed) else {
        return fail("The tokens run past the file.");
    };
    let text = decompress(input, size)?;
    Ok(text
        .split(|byte| *byte == 0)
        .take(count)
        .map(|token| String::from_utf8_lossy(token).into_owned())
        .collect())
}

fn read_fields(bytes: &[u8], at: usize) -> Result<Vec<(usize, Rep)>> {
    let count = size_at(bytes, at)?;
    let (names, next) = compressed_integers(bytes, at + 8, count)?;
    let size = size_at(bytes, next)?;
    let Some(input) = bytes.get(next + 8..next + 8 + size) else {
        return fail("The fields' values run past the file.");
    };
    let reps = decompress(input, count * 8)?;
    Ok(names
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name as usize, Rep(u64_at(&reps, index * 8).unwrap_or(0))))
        .collect())
}

/// The paths section as parallel arrays: each item's path index, its name's token (negative for
/// a property), and its jump (to its sibling, or -1 for a child only, -2 for neither).
struct PathTree<'a> {
    indexes: Vec<i32>,
    elements: Vec<i32>,
    jumps: Vec<i32>,
    tokens: &'a [String],
}

impl PathTree<'_> {
    /// The text of item `index` under `parent`.
    fn text_of(&self, index: usize, parent: &str) -> String {
        let element = self.elements[index];
        let name = self
            .tokens
            .get(element.unsigned_abs() as usize)
            .map_or("", String::as_str);
        match (parent, element < 0) {
            ("", _) => "/".to_owned(),
            (_, true) => format!("{parent}.{name}"),
            ("/", false) => format!("/{name}"),
            (_, false) => format!("{parent}/{name}"),
        }
    }

    /// Walks a run of items from `index` under `parent`: each item's children follow it, and a
    /// sibling after children is queued in `pending`.
    fn walk(
        &self,
        start: (usize, String),
        paths: &mut [String],
        pending: &mut Vec<(usize, String)>,
    ) {
        let (mut index, mut parent) = start;
        while index
            < self
                .indexes
                .len()
                .min(self.jumps.len())
                .min(self.elements.len())
        {
            let text = self.text_of(index, &parent);
            if let Some(slot) = paths.get_mut(self.indexes[index] as usize) {
                slot.clone_from(&text);
            }
            let jump = self.jumps[index];
            let (child, sibling) = (jump > 0 || jump == -1, jump >= 0);
            if child && sibling {
                pending.push((index + jump as usize, parent.clone()));
            }
            if child {
                parent = text;
            } else if !sibling {
                return;
            }
            index += 1;
        }
    }
}

/// Each path's text, from the tree the paths section lays out as parallel arrays.
fn read_paths(bytes: &[u8], at: usize, tokens: &[String]) -> Result<Vec<String>> {
    let total = size_at(bytes, at)?;
    let count = size_at(bytes, at + 8)?;
    let (indexes, next) = compressed_integers(bytes, at + 16, count)?;
    let (elements, next) = compressed_integers(bytes, next, count)?;
    let (jumps, _) = compressed_integers(bytes, next, count)?;
    let tree = PathTree {
        indexes,
        elements,
        jumps,
        tokens,
    };
    let mut paths = vec![String::new(); total];
    let mut pending: Vec<(usize, String)> = vec![(0, String::new())];
    while let Some(start) = pending.pop() {
        if pending.len() > ITEMS_MAX {
            return fail("The paths' tree loops.");
        }
        tree.walk(start, &mut paths, &mut pending);
    }
    Ok(paths)
}

fn read_specs(bytes: &[u8], at: usize) -> Result<Vec<Spec>> {
    let count = size_at(bytes, at)?;
    let (paths, next) = compressed_integers(bytes, at + 8, count)?;
    let (sets, next) = compressed_integers(bytes, next, count)?;
    let (kinds, _) = compressed_integers(bytes, next, count)?;
    Ok((0..count)
        .map(|index| Spec {
            path: paths[index] as usize,
            field_set: sets[index] as usize,
            kind: kinds[index] as u32,
        })
        .collect())
}

impl<'a> Crate<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self> {
        if !bytes.starts_with(SIGNATURE) {
            return fail("The file is not a crate file.");
        }
        // Major and minor, after the signature.
        let version = (
            bytes.get(8).copied().unwrap_or(0),
            bytes.get(9).copied().unwrap_or(0),
        );
        if version < OLDEST {
            return fail("The crate file is older than this reader reads.");
        }
        let found = sections(bytes)?;
        let section = |name: &str| {
            found
                .get(name)
                .copied()
                .ok_or_else(|| CrateError(format!("The crate file has no {name} section.")))
        };
        let tokens = read_tokens(bytes, section("TOKENS")?)?;
        let field_sets_at = section("FIELDSETS")?;
        let field_set_count = size_at(bytes, field_sets_at)?;
        let (field_sets, _) = compressed_integers(bytes, field_sets_at + 8, field_set_count)?;
        Ok(Crate {
            bytes,
            wide_counts: version >= WIDE_COUNTS,
            fields: read_fields(bytes, section("FIELDS")?)?,
            field_sets: field_sets.into_iter().map(|value| value as u32).collect(),
            paths: read_paths(bytes, section("PATHS")?, &tokens)?,
            specs: read_specs(bytes, section("SPECS")?)?,
            tokens,
        })
    }

    /// A spec's fields: each name and value.
    fn fields_of(&self, spec: &Spec) -> Vec<(&str, Rep)> {
        self.field_sets
            .iter()
            .skip(spec.field_set)
            .take_while(|field| **field != END_OF_SET)
            .filter_map(|field| {
                let (name, rep) = self.fields.get(*field as usize)?;
                Some((self.tokens.get(*name)?.as_str(), *rep))
            })
            .collect()
    }

    fn field(&self, spec: &Spec, name: &str) -> Option<Rep> {
        self.fields_of(spec)
            .into_iter()
            .find(|(known, _)| *known == name)
            .map(|(_, rep)| rep)
    }

    fn token(&self, rep: Rep) -> Option<&str> {
        (rep.kind() == TOKEN && rep.is_inlined())
            .then(|| self.tokens.get(rep.payload() as usize))
            .flatten()
            .map(String::as_str)
    }

    /// An array's count at `at` and where its items start.
    fn array_count(&self, at: usize) -> Option<(usize, usize)> {
        let count = if self.wide_counts {
            u64_at(self.bytes, at)? as usize
        } else {
            u32_at(self.bytes, at)? as usize
        };
        let width = if self.wide_counts { 8 } else { 4 };
        (count <= ITEMS_MAX).then_some((count, at + width))
    }

    fn vectors(&self, rep: Rep) -> Option<Vec<[f32; 3]>> {
        if rep.kind() != VEC3F || !rep.is_array() {
            return None;
        }
        let (count, at) = self.array_count(rep.payload() as usize)?;
        let floats = self.bytes.get(at..at + count * 12)?;
        Some(
            floats
                .as_chunks::<12>()
                .0
                .iter()
                .map(|vector| {
                    std::array::from_fn(|axis| {
                        f32::from_le_bytes(
                            vector[axis * 4..axis * 4 + 4]
                                .try_into()
                                .expect("four bytes"),
                        )
                    })
                })
                .collect(),
        )
    }

    fn integers(&self, rep: Rep) -> Option<Vec<i32>> {
        if rep.kind() != INT || !rep.is_array() {
            return None;
        }
        let (count, at) = self.array_count(rep.payload() as usize)?;
        if rep.is_compressed() && count >= COMPRESSED_ARRAY_MIN {
            return compressed_integers(self.bytes, at, count)
                .ok()
                .map(|(values, _)| values);
        }
        let raw = self.bytes.get(at..at + count * 4)?;
        Some(
            raw.as_chunks::<4>()
                .0
                .iter()
                .map(|four| i32::from_le_bytes(*four))
                .collect(),
        )
    }

    /// A single vector: inlined as three signed bytes, or stored as three floats.
    fn vector(&self, rep: Rep) -> Option<[f32; 3]> {
        if rep.kind() != VEC3F || rep.is_array() {
            return None;
        }
        if rep.is_inlined() {
            let bytes = (rep.payload() as u32).to_le_bytes();
            return Some([0, 1, 2].map(|axis| f32::from(bytes[axis] as i8)));
        }
        let at = rep.payload() as usize;
        let floats = self.bytes.get(at..at + 12)?;
        Some(std::array::from_fn(|axis| {
            f32::from_le_bytes(
                floats[axis * 4..axis * 4 + 4]
                    .try_into()
                    .expect("four bytes"),
            )
        }))
    }

    /// The first path a path list op names: its explicit, added, prepended or appended items.
    fn first_target(&self, rep: Rep) -> Option<&str> {
        if rep.kind() != PATH_LIST_OP {
            return None;
        }
        let at = rep.payload() as usize;
        let header = *self.bytes.get(at)?;
        // Explicit, added, prepended and appended items, in the order they are written.
        let lists = [1 << 1, 1 << 2, 1 << 5, 1 << 6];
        let mut cursor = at + 1;
        for bit in lists {
            if header & bit == 0 {
                continue;
            }
            let count = u64_at(self.bytes, cursor)? as usize;
            if count > 0 {
                let path = u32_at(self.bytes, cursor + 8)? as usize;
                return self.paths.get(path).map(String::as_str);
            }
            cursor += 8 + count * 4;
        }
        None
    }
}

// --- Meshes --------------------------------------------------------------------------------

/// A mesh as the crate gives it: its points, its faces' sizes and corners, and the material it
/// is bound to.
pub struct Shape {
    pub points: Vec<[f32; 3]>,
    pub counts: Vec<i32>,
    pub corners: Vec<i32>,
    pub colour: Option<[f32; 3]>,
}

/// The meshes in a crate file held in memory, and whether the file says it is Z-up.
pub fn read_shapes(bytes: &[u8]) -> Result<(Vec<Shape>, bool)> {
    let crate_file = Crate::open(bytes)?;
    let by_path: HashMap<&str, &Spec> = crate_file
        .specs
        .iter()
        .filter_map(|spec| Some((crate_file.paths.get(spec.path)?.as_str(), spec)))
        .collect();
    let attribute = |prim: &str, name: &str| -> Option<Rep> {
        crate_file.field(by_path.get(format!("{prim}.{name}").as_str())?, "default")
    };
    let mut colours: HashMap<String, [f32; 3]> = HashMap::new();
    for (path, spec) in &by_path {
        let Some(shader) = path.strip_suffix(".inputs:diffuseColor") else {
            continue;
        };
        let Some(colour) = crate_file
            .field(spec, "default")
            .and_then(|rep| crate_file.vector(rep))
        else {
            continue;
        };
        if let Some((material, _)) = shader.rsplit_once('/') {
            colours.insert(material.to_owned(), colour);
        }
    }
    let mut shapes = Vec::new();
    for spec in crate_file.specs.iter().filter(|spec| spec.kind == PRIM) {
        let Some(path) = crate_file.paths.get(spec.path) else {
            continue;
        };
        let is_mesh = crate_file
            .field(spec, "typeName")
            .and_then(|rep| crate_file.token(rep))
            == Some("Mesh");
        if !is_mesh {
            continue;
        }
        let points = attribute(path, "points").and_then(|rep| crate_file.vectors(rep));
        let counts = attribute(path, "faceVertexCounts").and_then(|rep| crate_file.integers(rep));
        let corners = attribute(path, "faceVertexIndices").and_then(|rep| crate_file.integers(rep));
        let (Some(points), Some(counts), Some(corners)) = (points, counts, corners) else {
            continue;
        };
        let binding = by_path
            .get(format!("{path}.material:binding").as_str())
            .and_then(|spec| crate_file.field(spec, "targetPaths"))
            .and_then(|rep| crate_file.first_target(rep));
        shapes.push(Shape {
            points,
            counts,
            corners,
            colour: binding.and_then(|material| colours.get(material)).copied(),
        });
    }
    let z_up = crate_file
        .specs
        .iter()
        .filter(|spec| spec.kind == PSEUDO_ROOT)
        .filter_map(|spec| crate_file.field(spec, "upAxis"))
        .any(|rep| crate_file.token(rep) == Some("Z"));
    Ok((shapes, z_up))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_lz4_block_copies_literals_and_matches() {
        // "abcabcabca": three literals, then a match 3 back for 7.
        let block = [0x33, b'a', b'b', b'c', 3, 0];
        assert_eq!(lz4_block(&block, 10).unwrap(), b"abcabcabca");
        assert!(lz4_block(&[0x03, 1, 0], 10).is_err());
    }

    #[test]
    fn coded_integers_add_up_their_differences() {
        // Common value 1; codes: common, small (+5), medium (-300), large (+70000).
        let mut data = 1i32.to_le_bytes().to_vec();
        data.push(0b11_10_01_00);
        data.push(5);
        data.extend_from_slice(&(-300i16).to_le_bytes());
        data.extend_from_slice(&70_000i32.to_le_bytes());
        assert_eq!(decode_integers(&data, 4).unwrap(), [1, 6, -294, 69_706]);
    }
}
