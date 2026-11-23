//! Decodes a WOFF2 font into an OpenType file. The bundled faces (`typeface`) are WOFF2, and
//! the core decodes them itself so every platform gets identical faces regardless of system font
//! libraries. WOFF2 stores `glyf` and `loca` transformed; `glyf` rebuilds them as TrueType.

mod glyf;

use std::collections::BTreeMap;
use std::io::Read;

/// WOFF2's known table tags, in spec order, referenced by index.
const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

/// Flag value meaning the tag follows in the next four bytes.
const TAG_FOLLOWS: u8 = 63;
/// WOFF2 header length: signature, counts and lengths, 48 bytes.
const HEADER_LENGTH: usize = 48;

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

/// Reads a UIntBase128 at `at`: 7 bits per byte, high bit set on all but the last.
fn base128(data: &[u8], at: &mut usize) -> usize {
    let mut value = 0;
    loop {
        let byte = data[*at];
        *at += 1;
        value = (value << 7) | usize::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return value;
        }
    }
}

/// A WOFF2 directory entry: tag, length in the stream, and whether it is transformed.
struct TableEntry {
    tag: [u8; 4],
    length: usize,
    transformed: bool,
}

/// Reads the directory entry at `at` and advances past it.
fn table_entry(file: &[u8], at: &mut usize) -> TableEntry {
    let flags = file[*at];
    *at += 1;
    let tag = if flags & 0x3f == TAG_FOLLOWS {
        let tag = [file[*at], file[*at + 1], file[*at + 2], file[*at + 3]];
        *at += 4;
        tag
    } else {
        *KNOWN_TAGS[usize::from(flags & 0x3f)]
    };
    // `glyf` and `loca` are transformed at version 0; other tables at any nonzero version.
    let version = flags >> 6;
    let transformed = match &tag {
        b"glyf" | b"loca" => version == 0,
        _ => version != 0,
    };
    let original = base128(file, at);
    let length = if transformed {
        base128(file, at)
    } else {
        original
    };
    TableEntry {
        tag,
        length,
        transformed,
    }
}

/// The original tables of WOFF2 font `file`, by tag.
pub fn tables(file: &[u8]) -> BTreeMap<[u8; 4], Vec<u8>> {
    assert_eq!(&file[0..4], b"wOF2", "a WOFF2 font");
    let count = usize::from(u16_at(file, 12));
    let compressed = u32_at(file, 20) as usize;
    let mut at = HEADER_LENGTH;
    let entries: Vec<TableEntry> = (0..count).map(|_| table_entry(file, &mut at)).collect();
    let mut stream = Vec::new();
    brotli_decompressor::Decompressor::new(&file[at..at + compressed], 4096)
        .read_to_end(&mut stream)
        .expect("a WOFF2 font's tables decompress");
    let mut tables = BTreeMap::new();
    let mut offset = 0;
    for entry in entries {
        let data = &stream[offset..offset + entry.length];
        offset += entry.length;
        match (&entry.tag, entry.transformed) {
            (_, false) => {
                tables.insert(entry.tag, data.to_vec());
            }
            (b"glyf", true) => {
                let (glyf, loca) = glyf::rebuilt(data);
                tables.insert(*b"glyf", glyf);
                tables.insert(*b"loca", loca);
            }
            // `loca` is rebuilt with `glyf`; its own entry is empty.
            (b"loca", true) => {}
            (tag, true) => panic!(
                "the WOFF2 font's {} is transformed, which only glyf and loca are here",
                String::from_utf8_lossy(tag)
            ),
        }
    }
    tables
}

/// Writes an OpenType file holding `tables`.
pub fn open_type(tables: &BTreeMap<[u8; 4], Vec<u8>>) -> Vec<u8> {
    let count = tables.len() as u16;
    let selector = 15 - count.leading_zeros() as u16;
    let range = (1u16 << selector) * 16;
    let mut file = Vec::new();
    for value in [
        0x0001u16,
        0x0000,
        count,
        range,
        selector,
        count * 16 - range,
    ] {
        file.extend(value.to_be_bytes());
    }
    let start = 12 + usize::from(count) * 16;
    let mut body = Vec::new();
    for (tag, data) in tables {
        file.extend(tag);
        // No checksum: nothing that reads these files verifies it.
        file.extend(0u32.to_be_bytes());
        file.extend(((start + body.len()) as u32).to_be_bytes());
        file.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
    }
    file.extend(body);
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shipped_face_reads_back_as_its_tables() {
        let tables = tables(crate::text::typeface::INTER);
        for tag in [
            b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"GPOS", b"GSUB", b"GDEF", b"HVAR",
            b"fvar", b"avar",
        ] {
            assert!(tables.contains_key(tag), "{}", String::from_utf8_lossy(tag));
        }
        let file = open_type(&tables);
        assert_eq!(u16_at(&file, 4) as usize, tables.len());
    }

    /// Summary sums of a face's outlines, read back from its rebuilt `glyf` and `loca`.
    #[derive(Debug, Default, PartialEq)]
    struct Outlines {
        simple: usize,
        composite: usize,
        empty: usize,
        contours: usize,
        points: usize,
        on_curve: usize,
        x: i64,
        y: i64,
        components: usize,
        boxes: i64,
    }

    fn outlines(woff2: &[u8]) -> Outlines {
        use read_fonts::tables::glyf::Glyph;
        use read_fonts::types::GlyphId;
        use read_fonts::{FontRef, TableProvider};
        let file = open_type(&tables(woff2));
        let font = FontRef::new(&file).expect("the rebuilt face reads");
        let (glyf, loca) = (font.glyf().expect("glyf"), font.loca(None).expect("loca"));
        let mut sums = Outlines::default();
        for index in 0..font.maxp().expect("maxp").num_glyphs() {
            let glyph = loca.get_glyf(GlyphId::new(u32::from(index)), &glyf);
            let Some(glyph) = glyph.expect("every glyph reads") else {
                sums.empty += 1;
                continue;
            };
            sums.boxes += [glyph.x_min(), glyph.y_min(), glyph.x_max(), glyph.y_max()]
                .iter()
                .map(|value| i64::from(*value))
                .sum::<i64>();
            match glyph {
                Glyph::Simple(simple) => {
                    sums.simple += 1;
                    sums.contours += simple.end_pts_of_contours().len();
                    for point in simple.points() {
                        sums.points += 1;
                        sums.on_curve += usize::from(point.on_curve);
                        sums.x += i64::from(point.x);
                        sums.y += i64::from(point.y);
                    }
                }
                Glyph::Composite(composite) => {
                    sums.composite += 1;
                    sums.components += composite.components().count();
                }
            }
        }
        sums
    }

    /// The expected sums come from fontTools 4.60.1, an independent WOFF2 decoder, reading the
    /// same files.
    #[test]
    fn the_shipped_faces_outlines_are_rebuilt_as_another_decoder_reads_them() {
        assert_eq!(
            outlines(crate::text::typeface::INTER),
            Outlines {
                simple: 197,
                composite: 312,
                empty: 9,
                contours: 337,
                points: 4999,
                on_curve: 2491,
                x: 3_344_427,
                y: 3_687_360,
                components: 529,
                boxes: 1_416_457,
            }
        );
        assert_eq!(
            outlines(crate::text::typeface::GEIST_MONO),
            Outlines {
                simple: 162,
                composite: 100,
                empty: 2,
                contours: 269,
                points: 3682,
                on_curve: 1912,
                x: 1_119_293,
                y: 1_299_153,
                components: 176,
                boxes: 349_751,
            }
        );
    }

    #[test]
    fn a_255_uint16_takes_one_byte_or_more_after_its_codes() {
        let read = |bytes: &[u8]| glyf::Stream::new(bytes).u255();
        assert_eq!(read(&[17]), 17);
        assert_eq!(read(&[253, 0x01, 0x02]), 258);
        assert_eq!(read(&[255, 10]), 263);
        assert_eq!(read(&[254, 10]), 516);
    }

    #[test]
    fn a_base128_number_takes_seven_bits_a_byte() {
        let mut at = 0;
        assert_eq!(base128(&[0x81, 0x00, 0x3f], &mut at), 128);
        assert_eq!(at, 2);
        assert_eq!(base128(&[0x81, 0x00, 0x3f], &mut at), 63);
    }
}
