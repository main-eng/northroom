//! Reads the meshes in an Alembic file (`.abc`) in its Ogawa layout, for `mesh` to draw
//! (ADR-0015). Written from Alembic's own reader (`lib/Alembic/Ogawa`, and
//! `AbcCoreOgawa/ReadUtil.cpp`, `OrData.cpp`, `ArImpl.cpp`, `AprImpl.cpp`).
//!
//! An Ogawa file is a tree of groups whose children are groups or data. Its objects are
//! groups: the first child holds the object's properties, the last its children's headers, and
//! those between are its children. A mesh is an object with a `.geom` compound holding `P`
//! (its points), `.faceCounts` and `.faceIndices`. Each is read at its first sample.
//!
//! Left out: transforms (a mesh is drawn where its points are), later samples of animation,
//! normals and UVs the file gives (normals are worked out from the faces), and the older HDF5
//! layout, which Alembic stopped writing by default in 2013.

/// Why an Alembic file can't be read; the words are for logs and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlembicError(pub String);

type Result<T> = std::result::Result<T, AlembicError>;

fn fail<T>(why: &str) -> Result<T> {
    Err(AlembicError(why.to_owned()))
}

const SIGNATURE: &[u8] = b"Ogawa";
/// A child's place with this bit set is data; without it, a group.
const DATA_BIT: u64 = 1 << 63;
/// A sample's data starts with a 16-byte hash of itself.
const SAMPLE_KEY_BYTES: usize = 16;
/// How deep objects and compounds are followed; deeper is a loop or damage.
const DEPTH_MAX: usize = 64;
/// The most children a group is read with.
const CHILDREN_MAX: usize = 1_000_000;
/// The archive group's child holding the root object.
const ROOT_OBJECT: usize = 2;

/// Property kinds and plain data types, as Alembic numbers them.
const COMPOUND: u32 = 0;
const FLOAT32: u32 = 10;
const INT32: u32 = 6;

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// A child of a group: another group, or data, by where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Child {
    Group(usize),
    Data(usize),
}

/// The children of the group at `at`.
fn children(bytes: &[u8], at: usize) -> Vec<Child> {
    let count = u64_at(bytes, at).unwrap_or(0) as usize;
    if at == 0 || count == 0 || count > CHILDREN_MAX || count > bytes.len() / 8 {
        return Vec::new();
    }
    (0..count)
        .filter_map(|index| u64_at(bytes, at + 8 + index * 8))
        .map(|place| {
            if place & DATA_BIT != 0 {
                Child::Data((place & !DATA_BIT) as usize)
            } else {
                Child::Group(place as usize)
            }
        })
        .collect()
}

/// The bytes of the data at `at`: its size, then itself. An empty child has none.
fn data(bytes: &[u8], at: usize) -> &[u8] {
    let size = u64_at(bytes, at).unwrap_or(0) as usize;
    if at == 0 {
        return &[];
    }
    bytes
        .get(at + 8..at.saturating_add(8).saturating_add(size))
        .unwrap_or(&[])
}

/// A property as its header says: its name, kind (compound, scalar or array), and its type and
/// how many values make one item.
struct Property {
    name: String,
    kind: u32,
    pod: u32,
    extent: u32,
}

/// A number of 1, 2 or 4 bytes, as the header's size hint says.
fn hinted(buffer: &[u8], at: &mut usize, hint: u32) -> Option<u32> {
    let (value, size) = match hint {
        0 => (u32::from(*buffer.get(*at)?), 1),
        1 => (
            u32::from(u16::from_le_bytes(
                buffer.get(*at..*at + 2)?.try_into().ok()?,
            )),
            2,
        ),
        _ => (u32_at(buffer, *at)?, 4),
    };
    *at += size;
    Some(value)
}

/// One property header from `at`: its info word, its sample indexes, its name and its metadata.
fn read_property(buffer: &[u8], at: &mut usize) -> Option<Property> {
    let info = u32_at(buffer, *at)?;
    *at += 4;
    let kind = info & 0x3;
    let hint = (info & 0xc) >> 2;
    if kind != COMPOUND {
        hinted(buffer, at, hint)?;
        if info & 0x200 != 0 {
            hinted(buffer, at, hint)?;
            hinted(buffer, at, hint)?;
        }
        if info & 0x100 != 0 {
            hinted(buffer, at, hint)?;
        }
    }
    let length = hinted(buffer, at, hint)? as usize;
    let name = String::from_utf8_lossy(buffer.get(*at..*at + length)?).into_owned();
    *at += length;
    if (info & 0x0ff0_0000) >> 20 == 0xff {
        let metadata = hinted(buffer, at, hint)? as usize;
        *at += metadata;
    }
    Some(Property {
        name,
        kind,
        pod: (info & 0xf0) >> 4,
        extent: (info & 0xff000) >> 12,
    })
}

/// A compound property's group: each property's header with its child.
fn properties(bytes: &[u8], at: usize) -> Vec<(Property, Child)> {
    let found = children(bytes, at);
    let Some(Child::Data(headers)) = found.last().copied() else {
        return Vec::new();
    };
    let buffer = data(bytes, headers);
    let mut cursor = 0;
    let mut read = Vec::new();
    while cursor < buffer.len() {
        let Some(property) = read_property(buffer, &mut cursor) else {
            break;
        };
        read.push(property);
    }
    read.into_iter().zip(found).collect()
}

/// An array property's first sample as numbers of `pod`, `extent` to an item.
fn first_sample(bytes: &[u8], child: Child) -> Vec<[u8; 4]> {
    let Child::Group(group) = child else {
        return Vec::new();
    };
    let Some(Child::Data(sample)) = children(bytes, group).first().copied() else {
        return Vec::new();
    };
    let values = data(bytes, sample);
    values
        .get(SAMPLE_KEY_BYTES..)
        .unwrap_or(&[])
        .as_chunks::<4>()
        .0
        .to_vec()
}

/// A mesh as the file gives it: points, faces' sizes and corners.
pub struct Shape {
    pub points: Vec<[f32; 3]>,
    pub counts: Vec<i32>,
    pub corners: Vec<i32>,
}

/// The mesh a `.geom` compound describes, if it has points and faces.
fn read_geometry(bytes: &[u8], at: usize) -> Option<Shape> {
    let mut points = None;
    let mut counts = None;
    let mut corners = None;
    for (property, child) in properties(bytes, at) {
        let numbers = || first_sample(bytes, child);
        match (property.name.as_str(), property.pod, property.extent) {
            ("P", FLOAT32, 3) => {
                let floats: Vec<f32> = numbers().into_iter().map(f32::from_le_bytes).collect();
                points = Some(floats.as_chunks::<3>().0.to_vec());
            }
            (".faceCounts", INT32, 1) => {
                counts = Some(numbers().into_iter().map(i32::from_le_bytes).collect())
            }
            (".faceIndices", INT32, 1) => {
                corners = Some(numbers().into_iter().map(i32::from_le_bytes).collect())
            }
            _ => {}
        }
    }
    Some(Shape {
        points: points?,
        counts: counts?,
        corners: corners?,
    })
}

/// The meshes under the object group at `at`, its own first.
fn read_object(bytes: &[u8], at: usize, depth: usize, shapes: &mut Vec<Shape>) {
    if depth > DEPTH_MAX {
        return;
    }
    let found = children(bytes, at);
    if let Some(Child::Group(own)) = found.first().copied() {
        for (property, child) in properties(bytes, own) {
            if let (COMPOUND, ".geom", Child::Group(geometry)) =
                (property.kind, property.name.as_str(), child)
            {
                shapes.extend(read_geometry(bytes, geometry));
            }
        }
    }
    // The children between the properties and the headers are objects.
    let inner = found.len().saturating_sub(1);
    for child in found.iter().take(inner).skip(1) {
        if let Child::Group(object) = child {
            read_object(bytes, *object, depth + 1, shapes);
        }
    }
}

/// The meshes in an Alembic file held in memory.
pub fn read_shapes(bytes: &[u8]) -> Result<Vec<Shape>> {
    if !bytes.starts_with(SIGNATURE) {
        return fail("The file is not an Ogawa Alembic file.");
    }
    let root = u64_at(bytes, 8).unwrap_or(0) as usize;
    let Some(Child::Group(object)) = children(bytes, root).get(ROOT_OBJECT).copied() else {
        return fail("The Alembic file has no root object.");
    };
    let mut shapes = Vec::new();
    read_object(bytes, object, 0, &mut shapes);
    Ok(shapes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Ogawa file built by hand: the archive group, its root object holding one child object
    /// with a `.geom` of one triangle.
    fn sample() -> Vec<u8> {
        let mut file = b"Ogawa\xff\x00\x01".to_vec();
        file.extend_from_slice(&[0; 8]);
        let put_data = |file: &mut Vec<u8>, bytes: &[u8]| {
            let at = file.len();
            file.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            file.extend_from_slice(bytes);
            DATA_BIT | at as u64
        };
        let put_group = |file: &mut Vec<u8>, children: &[u64]| {
            let at = file.len();
            file.extend_from_slice(&(children.len() as u64).to_le_bytes());
            for child in children {
                file.extend_from_slice(&child.to_le_bytes());
            }
            at as u64
        };
        let sample = |file: &mut Vec<u8>, values: Vec<u8>| {
            let mut keyed = vec![0; SAMPLE_KEY_BYTES];
            keyed.extend(values);
            let data = put_data(file, &keyed);
            put_group(file, &[data])
        };
        let points: Vec<u8> = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let p = sample(&mut file, points);
        let counts = sample(&mut file, 3i32.to_le_bytes().to_vec());
        let corners: Vec<u8> = [0i32, 1, 2]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let indices = sample(&mut file, corners);
        // Array properties: kind 2, size hint 0, pod in bits 4-7, extent in bits 12-19, then
        // one sample count byte and the name.
        let header = |pod: u32, extent: u32, name: &str| {
            let mut bytes = (2 | (pod << 4) | (extent << 12)).to_le_bytes().to_vec();
            bytes.push(1);
            bytes.push(name.len() as u8);
            bytes.extend_from_slice(name.as_bytes());
            bytes
        };
        let mut headers = header(FLOAT32, 3, "P");
        headers.extend(header(INT32, 1, ".faceCounts"));
        headers.extend(header(INT32, 1, ".faceIndices"));
        let headers = put_data(&mut file, &headers);
        let geometry = put_group(&mut file, &[p, counts, indices, headers]);
        let mut compound = 0u32.to_le_bytes().to_vec();
        compound.push(5);
        compound.extend_from_slice(b".geom");
        let compound = put_data(&mut file, &compound);
        let own = put_group(&mut file, &[geometry, compound]);
        let empty = put_data(&mut file, &[]);
        let mesh = put_group(&mut file, &[own, empty]);
        let root_own = put_group(&mut file, &[empty]);
        let root = put_group(&mut file, &[root_own, mesh, empty]);
        let archive = put_group(&mut file, &[empty, empty, root, empty, empty, empty]);
        file[8..16].copy_from_slice(&archive.to_le_bytes());
        file
    }

    #[test]
    fn a_mesh_in_a_child_object_reads_its_first_sample() {
        let shapes = read_shapes(&sample()).expect("read");
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].points[1], [1.0, 0.0, 0.0]);
        assert_eq!(
            (shapes[0].counts.clone(), shapes[0].corners.clone()),
            (vec![3], vec![0, 1, 2])
        );
        assert!(read_shapes(b"not alembic").is_err());
    }
}
