//! A PDF font: which glyph each character code draws, its outline, and its width.
//!
//! Embedded programs are drawn from their own outlines: TrueType and OpenType through skrifa,
//! bare CFF wrapped as OpenType for skrifa, and Type 1 through `type1`. Type 3 glyphs are
//! content streams the interpreter runs. A font the file doesn't embed is drawn in the app's
//! own Inter, or Geist Mono for a monospaced one, at the widths the file gives.
//!
//! Left out: predefined CJK CMaps other than Identity (their codes are read as two-byte CIDs),
//! and synthetic bold and italic for fonts drawn in the fallback face.

use super::encoding::{char_of_name, name_of_char, Base};
use super::object::{Dict, Document, Object, Stream};
use super::type1::Type1;
use crate::deck::Command;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::{FontRef, TableProvider};
use skrifa::{GlyphId, MetadataProvider};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

/// A width in glyph space when the file gives none.
const DEFAULT_WIDTH: f64 = 1000.0;
/// Glyph space's units in text space: a thousandth, as PDF widths are written.
const GLYPH_UNITS: f64 = 1000.0;
/// The font descriptor's flag for a symbolic font.
const SYMBOLIC: i64 = 4;

/// Collects an outline, scaled from font units to text space.
struct Pen<'a> {
    commands: &'a mut Vec<Command>,
    scale: f64,
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::Move(
            f64::from(x) * self.scale,
            f64::from(y) * self.scale,
        ));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::Line(
            f64::from(x) * self.scale,
            f64::from(y) * self.scale,
        ));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let s = self.scale;
        self.commands.push(Command::Quad(
            f64::from(cx) * s,
            f64::from(cy) * s,
            f64::from(x) * s,
            f64::from(y) * s,
        ));
    }
    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        let s = self.scale;
        self.commands.push(Command::Cubic(
            f64::from(ax) * s,
            f64::from(ay) * s,
            f64::from(bx) * s,
            f64::from(by) * s,
            f64::from(x) * s,
            f64::from(y) * s,
        ));
    }
    fn close(&mut self) {
        self.commands.push(Command::Close);
    }
}

/// An OpenType font program held in memory, and its glyph names when asked for.
struct Sfnt {
    bytes: Vec<u8>,
    names: OnceLock<HashMap<String, u32>>,
}

impl Sfnt {
    fn new(bytes: Vec<u8>) -> Option<Self> {
        FontRef::new(&bytes).ok()?;
        Some(Self {
            bytes,
            names: OnceLock::new(),
        })
    }

    fn face(&self) -> FontRef<'_> {
        FontRef::new(&self.bytes).expect("checked when made")
    }

    fn glyph_by_name(&self, name: &str) -> Option<u32> {
        let names = self.names.get_or_init(|| {
            self.face()
                .glyph_names()
                .iter()
                .map(|(glyph, name)| (name.as_str().to_owned(), glyph.to_u32()))
                .collect()
        });
        names.get(name).copied()
    }

    fn glyph_by_char(&self, character: u32) -> Option<u32> {
        self.face().charmap().map(character).map(GlyphId::to_u32)
    }

    fn outline(&self, glyph: u32) -> Option<Vec<Command>> {
        let face = self.face();
        let units = f64::from(face.head().ok()?.units_per_em().max(1));
        let outline = face.outline_glyphs().get(GlyphId::new(glyph))?;
        let mut commands = Vec::new();
        let mut pen = Pen {
            commands: &mut commands,
            scale: 1.0 / units,
        };
        outline
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                &mut pen,
            )
            .ok()?;
        Some(commands)
    }

    fn advance(&self, glyph: u32) -> Option<f64> {
        let face = self.face();
        let units = f64::from(face.head().ok()?.units_per_em().max(1));
        let metrics = face.glyph_metrics(Size::unscaled(), LocationRef::default());
        Some(f64::from(metrics.advance_width(GlyphId::new(glyph))?) / units * GLYPH_UNITS)
    }
}

/// The app's own faces as OpenType programs, for fonts a file doesn't embed.
fn fallback(mono: bool) -> &'static Sfnt {
    static SANS: OnceLock<Sfnt> = OnceLock::new();
    static MONO: OnceLock<Sfnt> = OnceLock::new();
    let (cell, file) = if mono {
        (&MONO, crate::text::typeface::GEIST_MONO)
    } else {
        (&SANS, crate::text::typeface::INTER)
    };
    cell.get_or_init(|| {
        let bytes = crate::text::woff2::open_type(&crate::text::woff2::tables(file));
        Sfnt::new(bytes).expect("the app's own faces read")
    })
}

// --- CFF wrapped as OpenType ---------------------------------------------------------------

fn be16(data: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes(
        data.get(at..at + 2)?.try_into().ok()?,
    )))
}

/// An INDEX at `at`: its item count and where it ends.
fn cff_index(data: &[u8], at: usize) -> Option<(usize, usize, Vec<usize>)> {
    let count = be16(data, at)?;
    if count == 0 {
        return Some((0, at + 2, Vec::new()));
    }
    let size = usize::from(*data.get(at + 2)?);
    let offset = |index: usize| -> Option<usize> {
        let start = at + 3 + index * size;
        Some(
            data.get(start..start + size)?
                .iter()
                .fold(0usize, |value, byte| value << 8 | usize::from(*byte)),
        )
    };
    let offsets: Vec<usize> = (0..=count).map(offset).collect::<Option<_>>()?;
    let base = at + 3 + (count + 1) * size - 1;
    Some((
        count,
        base + offsets[count],
        offsets.iter().map(|offset| base + offset).collect(),
    ))
}

/// The CharStrings offset a Top DICT gives (operator 17).
fn charstrings_offset(dict: &[u8]) -> Option<usize> {
    let mut operands: Vec<i64> = Vec::new();
    let mut at = 0;
    while at < dict.len() {
        let byte = dict[at];
        at += 1;
        match byte {
            17 => return operands.last().map(|offset| *offset as usize),
            12 => {
                at += 1;
                operands.clear();
            }
            0..=21 => operands.clear(),
            30 => {
                at = skip_real(dict, at);
                operands.push(0);
            }
            _ => {
                let (operand, used) = dict_operand(byte, dict.get(at..)?)?;
                operands.extend(operand);
                at += used;
            }
        }
    }
    None
}

/// Past a real number's nibbles, ended by a nibble of 15.
fn skip_real(dict: &[u8], mut at: usize) -> usize {
    while let Some(&nibbles) = dict.get(at) {
        at += 1;
        if nibbles & 0x0f == 0x0f || nibbles >> 4 == 0x0f {
            break;
        }
    }
    at
}

/// The integer a CFF DICT operand starting with `byte` stands for, and the bytes after `byte`
/// it took; no operand for a byte that starts none.
fn dict_operand(byte: u8, rest: &[u8]) -> Option<(Option<i64>, usize)> {
    let next = || rest.first().copied().map(i64::from);
    Some(match byte {
        28 => (
            Some(i64::from(i16::from_be_bytes(
                rest.get(..2)?.try_into().ok()?,
            ))),
            2,
        ),
        29 => (
            Some(i64::from(i32::from_be_bytes(
                rest.get(..4)?.try_into().ok()?,
            ))),
            4,
        ),
        32..=246 => (Some(i64::from(byte) - 139), 0),
        247..=250 => (Some((i64::from(byte) - 247) * 256 + next()? + 108), 1),
        251..=254 => (Some(-(i64::from(byte) - 251) * 256 - next()? - 108), 1),
        _ => (None, 0),
    })
}

/// How many glyphs a bare CFF program holds.
fn cff_glyph_count(cff: &[u8]) -> Option<usize> {
    let header = usize::from(*cff.get(2)?);
    let (_, after_names, _) = cff_index(cff, header)?;
    let (_, _, tops) = cff_index(cff, after_names)?;
    let top = cff.get(*tops.first()?..*tops.get(1)?)?;
    let (count, _, _) = cff_index(cff, charstrings_offset(top)?)?;
    Some(count)
}

/// A bare CFF program as an OpenType font skrifa reads: the CFF, a `head` and a `maxp`.
fn wrap_cff(cff: &[u8]) -> Option<Vec<u8>> {
    let count = cff_glyph_count(cff)?.min(65_535) as u16;
    let mut head = vec![0u8; 54];
    head[..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[48..50].copy_from_slice(&2u16.to_be_bytes());
    let mut maxp = 0x0000_5000u32.to_be_bytes().to_vec();
    maxp.extend_from_slice(&count.to_be_bytes());
    let mut tables = std::collections::BTreeMap::new();
    tables.insert(*b"CFF ", cff.to_vec());
    tables.insert(*b"head", head);
    tables.insert(*b"maxp", maxp);
    Some(crate::text::woff2::open_type(&tables))
}

// --- Fonts ---------------------------------------------------------------------------------

enum Program {
    Sfnt(Rc<Sfnt>),
    Fallback(&'static Sfnt),
    Type1(Rc<Type1>),
    Type3 {
        procedures: Dict,
        matrix: [f64; 6],
        resources: Option<Dict>,
    },
}

/// How a font's codes turn into glyphs.
pub struct Font {
    program: Program,
    /// Two-byte codes (a Type 0 font), read as CIDs.
    pub two_byte: bool,
    widths: HashMap<u32, f64>,
    default_width: f64,
    names: HashMap<u32, String>,
    chars: HashMap<u32, char>,
    symbolic: bool,
    cid_to_gid: Option<Vec<u16>>,
    outlines: RefCell<HashMap<u32, Option<Rc<Vec<Command>>>>>,
}

/// What a code draws in a Type 3 font: its procedure, run with the font's matrix and resources.
pub struct Procedure<'a> {
    pub stream: Stream,
    pub matrix: [f64; 6],
    pub resources: Option<&'a Dict>,
}

fn numbers(document: &Document, object: &Object) -> Vec<f64> {
    document
        .resolve(object)
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| document.resolve(item).as_f64())
                .collect()
        })
        .unwrap_or_default()
}

/// A simple font's widths: `FirstChar` onward.
fn simple_widths(document: &Document, dict: &Dict) -> HashMap<u32, f64> {
    let first = document
        .entry(dict, "FirstChar")
        .as_i64()
        .unwrap_or(0)
        .max(0) as u32;
    numbers(document, dict.get("Widths").unwrap_or(&Object::Null))
        .into_iter()
        .enumerate()
        .map(|(index, width)| (first + index as u32, width))
        .collect()
}

/// A CID font's widths: `[c [w…]]` runs and `[first last w]` ranges.
fn cid_widths(document: &Document, array: &Object) -> HashMap<u32, f64> {
    let items: Vec<Object> = document
        .resolve(array)
        .as_array()
        .map(<[Object]>::to_vec)
        .unwrap_or_default();
    let mut widths = HashMap::new();
    let mut at = 0;
    while at + 1 < items.len() {
        let first = document.resolve(&items[at]).as_i64().unwrap_or(0).max(0) as u32;
        match document.resolve(&items[at + 1]) {
            Object::Array(run) => {
                for (index, width) in run.iter().enumerate() {
                    widths.insert(
                        first + index as u32,
                        document.resolve(width).as_f64().unwrap_or(0.0),
                    );
                }
                at += 2;
            }
            last => {
                let last = last
                    .as_i64()
                    .unwrap_or(0)
                    .clamp(i64::from(first), i64::from(first) + 65_536)
                    as u32;
                let width = items
                    .get(at + 2)
                    .and_then(|width| document.resolve(width).as_f64())
                    .unwrap_or(0.0);
                for code in first..=last {
                    widths.insert(code, width);
                }
                at += 3;
            }
        }
    }
    widths
}

/// The glyph names a simple font's codes draw: its base encoding, its own program's, and its
/// differences.
fn read_names(
    document: &Document,
    dict: &Dict,
    built_in: Option<&HashMap<u8, String>>,
    symbolic: bool,
) -> HashMap<u32, String> {
    let encoding = document.entry(dict, "Encoding");
    let (base, differences) = match &encoding {
        Object::Name(name) => (Base::named(name), None),
        Object::Dict(encoding) => (
            document
                .entry(encoding, "BaseEncoding")
                .as_name()
                .and_then(Base::named),
            Some(document.entry(encoding, "Differences")),
        ),
        _ => (None, None),
    };
    // A named base, else the font's own, else (for a text font) the standard encoding.
    let mut names: HashMap<u32, String> = match (base, built_in) {
        (Some(base), _) => names_of(base),
        (None, Some(built_in)) => built_in
            .iter()
            .map(|(code, name)| (u32::from(*code), name.clone()))
            .collect(),
        (None, None) if !symbolic => names_of(Base::Standard),
        (None, None) => HashMap::new(),
    };
    apply_differences(document, differences.as_ref(), &mut names);
    names
}

/// Each code's glyph name in encoding `base`.
fn names_of(base: Base) -> HashMap<u32, String> {
    (0..=255u8)
        .filter_map(|code| {
            base.char_of(code)
                .map(|character| (u32::from(code), name_of_char(character)))
        })
        .collect()
}

/// An encoding's `Differences`: a code, then the names from it on.
fn apply_differences(
    document: &Document,
    differences: Option<&Object>,
    names: &mut HashMap<u32, String>,
) {
    let mut code = 0u32;
    for item in differences.and_then(Object::as_array).unwrap_or(&[]) {
        match document.resolve(item) {
            Object::Integer(value) => code = value.clamp(0, 255) as u32,
            Object::Name(name) => {
                names.insert(code, name);
                code += 1;
            }
            _ => {}
        }
    }
}

/// The program a font descriptor embeds.
fn embedded(document: &Document, descriptor: &Dict) -> Option<Program> {
    if let Object::Stream(stream) = document.entry(descriptor, "FontFile2") {
        return Sfnt::new(document.stream_data(&stream).ok()?)
            .map(|sfnt| Program::Sfnt(Rc::new(sfnt)));
    }
    if let Object::Stream(stream) = document.entry(descriptor, "FontFile3") {
        let data = document.stream_data(&stream).ok()?;
        let wrapped = if data.starts_with(b"OTTO")
            || data.starts_with(&[0, 1, 0, 0])
            || data.starts_with(b"true")
        {
            data
        } else {
            wrap_cff(&data)?
        };
        return Sfnt::new(wrapped).map(|sfnt| Program::Sfnt(Rc::new(sfnt)));
    }
    if let Object::Stream(stream) = document.entry(descriptor, "FontFile") {
        let data = document.stream_data(&stream).ok()?;
        let clear = document
            .entry(&stream.dict, "Length1")
            .as_i64()
            .unwrap_or(0)
            .max(0) as usize;
        return Type1::read(&data, clear).map(|font| Program::Type1(Rc::new(font)));
    }
    None
}

fn is_monospaced(name: &str, descriptor: Option<&Dict>) -> bool {
    let fixed = descriptor
        .and_then(|descriptor| descriptor.get("Flags"))
        .and_then(Object::as_i64)
        .is_some_and(|flags| flags & 1 != 0);
    fixed || name.contains("Courier") || name.contains("Mono")
}

impl Font {
    pub fn load(document: &Document, dict: &Dict) -> Self {
        let subtype = document.entry(dict, "Subtype");
        if subtype.as_name() == Some("Type0") {
            return Self::load_composite(document, dict);
        }
        if subtype.as_name() == Some("Type3") {
            return Self::load_type3(document, dict);
        }
        let descriptor = document.entry(dict, "FontDescriptor");
        let descriptor = descriptor.as_dict();
        let flags = descriptor
            .and_then(|descriptor| document.entry(descriptor, "Flags").as_i64())
            .unwrap_or(0);
        let symbolic = flags & SYMBOLIC != 0;
        let base_name = document
            .entry(dict, "BaseFont")
            .as_name()
            .unwrap_or("")
            .to_owned();
        let program = descriptor
            .and_then(|descriptor| embedded(document, descriptor))
            .unwrap_or_else(|| Program::Fallback(fallback(is_monospaced(&base_name, descriptor))));
        let built_in = match &program {
            Program::Type1(font) => Some(font.encoding.clone()),
            _ => None,
        };
        let names = read_names(
            document,
            dict,
            built_in.as_ref(),
            symbolic && built_in.is_none(),
        );
        let chars = names
            .iter()
            .filter_map(|(code, name)| Some((*code, char_of_name(name)?)))
            .collect();
        let missing =
            descriptor.and_then(|descriptor| document.entry(descriptor, "MissingWidth").as_f64());
        Font {
            program,
            two_byte: false,
            widths: simple_widths(document, dict),
            default_width: missing.unwrap_or(-1.0),
            names,
            chars,
            symbolic,
            cid_to_gid: None,
            outlines: RefCell::default(),
        }
    }

    fn load_composite(document: &Document, dict: &Dict) -> Self {
        let descendant = document
            .entry(dict, "DescendantFonts")
            .as_array()
            .and_then(|fonts| fonts.first())
            .map(|font| document.resolve(font))
            .and_then(|font| font.as_dict().cloned())
            .unwrap_or_default();
        let descriptor = document.entry(&descendant, "FontDescriptor");
        let descriptor = descriptor.as_dict();
        let base_name = document
            .entry(dict, "BaseFont")
            .as_name()
            .unwrap_or("")
            .to_owned();
        let program = descriptor
            .and_then(|descriptor| embedded(document, descriptor))
            .unwrap_or_else(|| Program::Fallback(fallback(is_monospaced(&base_name, descriptor))));
        let cid_to_gid = match document.entry(&descendant, "CIDToGIDMap") {
            Object::Stream(stream) => document.stream_data(&stream).ok().map(|data| {
                data.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_be_bytes(*pair))
                    .collect()
            }),
            _ => None,
        };
        Font {
            program,
            two_byte: true,
            widths: cid_widths(document, descendant.get("W").unwrap_or(&Object::Null)),
            default_width: document
                .entry(&descendant, "DW")
                .as_f64()
                .unwrap_or(DEFAULT_WIDTH),
            names: HashMap::new(),
            chars: HashMap::new(),
            symbolic: false,
            cid_to_gid,
            outlines: RefCell::default(),
        }
    }

    fn load_type3(document: &Document, dict: &Dict) -> Self {
        let matrix = numbers(document, dict.get("FontMatrix").unwrap_or(&Object::Null));
        let matrix: [f64; 6] = matrix
            .try_into()
            .unwrap_or([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
        let names = read_names(document, dict, None, true);
        Font {
            program: Program::Type3 {
                procedures: document
                    .entry(dict, "CharProcs")
                    .as_dict()
                    .cloned()
                    .unwrap_or_default(),
                matrix,
                resources: document.entry(dict, "Resources").as_dict().cloned(),
            },
            two_byte: false,
            // Type 3 widths are in glyph space, which the font matrix maps to text space.
            widths: simple_widths(document, dict)
                .into_iter()
                .map(|(code, width)| (code, width * matrix[0] * GLYPH_UNITS))
                .collect(),
            default_width: 0.0,
            chars: HashMap::new(),
            names,
            symbolic: true,
            cid_to_gid: None,
            outlines: RefCell::default(),
        }
    }

    /// The codes a string holds: one byte each, or two in a composite font.
    pub fn codes(&self, bytes: &[u8]) -> Vec<u32> {
        if self.two_byte {
            return bytes
                .chunks(2)
                .map(|pair| u32::from(pair[0]) << 8 | u32::from(*pair.get(1).unwrap_or(&0)))
                .collect();
        }
        bytes.iter().map(|byte| u32::from(*byte)).collect()
    }

    /// The glyph an OpenType program draws for `code`.
    fn sfnt_glyph(&self, sfnt: &Sfnt, code: u32) -> Option<u32> {
        if self.two_byte {
            return Some(match &self.cid_to_gid {
                Some(map) => u32::from(*map.get(code as usize)?),
                None => sfnt.glyph_by_name(&format!("cid{code:05}")).unwrap_or(code),
            });
        }
        let named = self
            .names
            .get(&code)
            .and_then(|name| sfnt.glyph_by_name(name));
        let by_char = || {
            self.chars
                .get(&code)
                .and_then(|character| sfnt.glyph_by_char(u32::from(*character)))
        };
        let symbol = || {
            sfnt.glyph_by_char(0xF000 + code)
                .or_else(|| sfnt.glyph_by_char(code))
        };
        let found = if self.symbolic {
            symbol().or(named).or_else(by_char)
        } else {
            named.or_else(by_char).or_else(symbol)
        };
        found.filter(|glyph| *glyph != 0)
    }

    fn fallback_glyph(&self, sfnt: &Sfnt, code: u32) -> Option<u32> {
        let character = match self.chars.get(&code) {
            Some(character) => u32::from(*character),
            None if self.two_byte => return None,
            None => code,
        };
        sfnt.glyph_by_char(character)
    }

    fn make_outline(&self, code: u32) -> Option<Vec<Command>> {
        match &self.program {
            Program::Sfnt(sfnt) => sfnt.outline(self.sfnt_glyph(sfnt, code)?),
            Program::Fallback(sfnt) => sfnt.outline(self.fallback_glyph(sfnt, code)?),
            Program::Type1(font) => {
                let name = self.names.get(&code)?;
                let (commands, _) = font.glyph(name)?;
                let scale = font.scale;
                Some(
                    commands
                        .into_iter()
                        .map(|command| scaled(command, scale))
                        .collect(),
                )
            }
            Program::Type3 { .. } => None,
        }
    }

    /// The outline `code` draws, in text space (an em is 1).
    pub fn outline(&self, code: u32) -> Option<Rc<Vec<Command>>> {
        if let Some(known) = self.outlines.borrow().get(&code) {
            return known.clone();
        }
        let made = self.make_outline(code).map(Rc::new);
        self.outlines.borrow_mut().insert(code, made.clone());
        made
    }

    /// How far `code` moves the pen, in text space.
    pub fn width(&self, code: u32) -> f64 {
        if let Some(width) = self.widths.get(&code) {
            return width / GLYPH_UNITS;
        }
        if self.default_width >= 0.0 {
            return self.default_width / GLYPH_UNITS;
        }
        let advance = match &self.program {
            Program::Fallback(sfnt) => self
                .fallback_glyph(sfnt, code)
                .and_then(|glyph| sfnt.advance(glyph)),
            Program::Sfnt(sfnt) => self
                .sfnt_glyph(sfnt, code)
                .and_then(|glyph| sfnt.advance(glyph)),
            Program::Type1(font) => self
                .names
                .get(&code)
                .and_then(|name| font.glyph(name))
                .map(|(_, width)| width),
            Program::Type3 { .. } => None,
        };
        advance.unwrap_or(DEFAULT_WIDTH * 0.5) / GLYPH_UNITS
    }

    /// The procedure that draws `code` in a Type 3 font.
    pub fn procedure(&self, document: &Document, code: u32) -> Option<Procedure<'_>> {
        let Program::Type3 {
            procedures,
            matrix,
            resources,
        } = &self.program
        else {
            return None;
        };
        let name = self.names.get(&code)?;
        let Object::Stream(stream) = document.entry(procedures, name) else {
            return None;
        };
        Some(Procedure {
            stream,
            matrix: *matrix,
            resources: resources.as_ref(),
        })
    }

    pub fn is_type3(&self) -> bool {
        matches!(self.program, Program::Type3 { .. })
    }
}

fn scaled(command: Command, scale: f64) -> Command {
    match command {
        Command::Move(x, y) => Command::Move(x * scale, y * scale),
        Command::Line(x, y) => Command::Line(x * scale, y * scale),
        Command::Quad(a, b, x, y) => Command::Quad(a * scale, b * scale, x * scale, y * scale),
        Command::Cubic(a, b, c, d, x, y) => Command::Cubic(
            a * scale,
            b * scale,
            c * scale,
            d * scale,
            x * scale,
            y * scale,
        ),
        Command::Close => Command::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fallback_faces_draw_and_measure_letters() {
        let sans = fallback(false);
        let glyph = sans.glyph_by_char(u32::from('A')).expect("a glyph for A");
        let outline = sans.outline(glyph).expect("an outline");
        assert!(outline.len() > 4);
        let width = sans.advance(glyph).expect("an advance");
        assert!((500.0..900.0).contains(&width), "{width}");
        assert!(fallback(true).glyph_by_char(u32::from('m')).is_some());
    }
}
