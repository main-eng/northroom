//! Reads a workbook file for display. An OpenDocument spreadsheet (`.ods`) is read in
//! `opendocument.rs`, through this file's zip and XML readers. An Excel workbook (`.xlsx`,
//! `.xlsm`) is read here: a zip of XML parts located through their relationship files. Each visible worksheet becomes a sheet of formatted cell text with
//! the file's bold, alignment and column widths. Formulas show their saved result and are never
//! calculated; fills, colours, charts, images, merged cells and conditional formatting are
//! ignored. Entries are inflated by miniz_oxide (RFC 1951), rejecting what a browser's decoder
//! rejects.
//!
//! Number formatting follows JavaScript's rules (number-to-string, `Number(text)`, tie rounding)
//! because `tests/workbook_parity.rs` pins recorded output formatted that way. Strings are
//! Rust's, so a format code or `_xHHHH_` escape that splits a character outside the Basic
//! Multilingual Plane, which JavaScript would keep as a lone surrogate, is not reproduced.

use crate::archive::{CENTRAL_HEADER_SIGNATURE, END_SIGNATURE, LOCAL_HEADER_SIGNATURE};
use crate::sheets::spreadsheet::{self, Alignment, Cell, Sheet, Workbook};
use miniz_oxide::inflate::core::inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
use miniz_oxide::inflate::core::{decompress, DecompressorOxide};
use miniz_oxide::inflate::TINFLStatus;

/// The file cannot be shown: not a zip, damaged, or missing a part. The view only says the
/// workbook could not be read; the message is for logs and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkbookError(pub String);

impl std::fmt::Display for WorkbookError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub(crate) type Result<T> = std::result::Result<T, WorkbookError>;

pub(crate) fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(WorkbookError(message.into()))
}

/// Maximum inflated size of one part; a larger part is refused, not truncated.
pub const PART_MAX_BYTES: usize = 64 * 1024 * 1024;
/// The largest workbook shown as a table; larger ones open in their own application.
pub const FILE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Whether a file name has a workbook extension: Excel's or OpenDocument's.
pub fn is_workbook_name(name: &str) -> bool {
    let extension = std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    matches!(extension.as_deref(), Some("xlsx" | "xlsm" | "ods"))
}

// --- Inflate (RFC 1951) ------------------------------------------------------------------

fn too_large<T>() -> Result<T> {
    fail("The zip entry is larger than this reader opens.")
}

/// Inflates a raw deflate stream, failing when it is damaged, exceeds `max_bytes`, or has data
/// after its last block. The output is not a wrapping window, so a match reaching before the
/// first byte is damage, as in a browser.
pub fn inflate(data: &[u8], max_bytes: usize) -> Result<Vec<u8>> {
    // One byte past the limit is enough to detect an oversized part.
    let room = max_bytes.saturating_add(1);
    let mut out = vec![0; data.len().saturating_mul(4).clamp(1, room)];
    let mut decompressor = Box::<DecompressorOxide>::default();
    let (mut read, mut written) = (0, 0);
    loop {
        let (status, consumed, made) = decompress(
            &mut decompressor,
            &data[read..],
            &mut out,
            written,
            TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF,
        );
        read += consumed;
        written += made;
        if written > max_bytes {
            return too_large();
        }
        match status {
            TINFLStatus::Done => break,
            TINFLStatus::HasMoreOutput => {
                let grown = out.len().saturating_mul(2).min(room);
                out.resize(grown, 0);
            }
            _ => return fail("The zip entry is damaged."),
        }
    }
    // The rest of the last byte is padding; any further byte is junk.
    if read != data.len() {
        return fail("The zip entry is damaged: junk after its data.");
    }
    out.truncate(written);
    Ok(out)
}

// --- Zip -----------------------------------------------------------------------------------

const END_RECORD_BYTES: usize = 22;
const END_RECORD_MAX_COMMENT_BYTES: usize = 0xffff;
const CENTRAL_HEADER_BYTES: usize = 46;
const LOCAL_HEADER_BYTES: usize = 30;

#[derive(Clone, Copy)]
struct ZipEntry {
    method: u16,
    compressed_size: usize,
    local_header_offset: usize,
}

/// An in-memory zip with its index parsed; entries are inflated on read.
pub struct ZipArchive<'a> {
    bytes: &'a [u8],
    /// Entries by name in first-seen order; a later entry with the same name replaces it.
    entries: Vec<(String, ZipEntry)>,
}

fn u16_at(bytes: &[u8], at: usize) -> usize {
    u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize
}

fn u32_at(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
}

/// Decodes like `new TextDecoder().decode`: UTF-8, invalid sequences as U+FFFD, leading BOM
/// dropped.
fn decode_utf8(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned()
}

impl<'a> ZipArchive<'a> {
    /// Reads the archive's index.
    pub fn open(bytes: &'a [u8]) -> Result<Self> {
        let end = (bytes.len() >= END_RECORD_BYTES)
            .then(|| {
                let earliest = bytes
                    .len()
                    .saturating_sub(END_RECORD_BYTES + END_RECORD_MAX_COMMENT_BYTES);
                (earliest..=bytes.len() - END_RECORD_BYTES)
                    .rev()
                    .find(|&at| u32_at(bytes, at) == END_SIGNATURE as usize)
            })
            .flatten();
        let Some(end) = end else {
            return fail("Not a zip archive: it has no end-of-archive record.");
        };
        let entry_count = u16_at(bytes, end + 10);
        let directory = u32_at(bytes, end + 16);
        if entry_count == 0xffff || directory == 0xffff_ffff {
            return fail("Zip64 archives are not read.");
        }
        let mut entries: Vec<(String, ZipEntry)> = Vec::new();
        let mut at = directory;
        for index in 0..entry_count {
            if at + CENTRAL_HEADER_BYTES > bytes.len()
                || u32_at(bytes, at) != CENTRAL_HEADER_SIGNATURE as usize
            {
                return fail(format!("The zip index is damaged at entry {index}."));
            }
            let name_length = u16_at(bytes, at + 28);
            let name_start = at + CENTRAL_HEADER_BYTES;
            let name = decode_utf8(
                &bytes[name_start.min(bytes.len())..(name_start + name_length).min(bytes.len())],
            );
            if u16_at(bytes, at + 8) & 1 != 0 {
                return fail(format!("The zip entry \"{name}\" is encrypted."));
            }
            let entry = ZipEntry {
                method: u16_at(bytes, at + 10) as u16,
                compressed_size: u32_at(bytes, at + 20),
                local_header_offset: u32_at(bytes, at + 42),
            };
            set_by_name(&mut entries, name, entry);
            at += CENTRAL_HEADER_BYTES
                + name_length
                + u16_at(bytes, at + 30)
                + u16_at(bytes, at + 32);
        }
        Ok(ZipArchive { bytes, entries })
    }

    pub fn entry_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(name, _)| name.as_str())
    }

    /// An entry's stored bytes and compression method; `None` when there is no such entry.
    pub fn raw_entry(&self, name: &str) -> Result<Option<(u16, &'a [u8])>> {
        let Some(&(_, entry)) = self.entries.iter().find(|(known, _)| known == name) else {
            return Ok(None);
        };
        let bytes = self.bytes;
        let header = entry.local_header_offset;
        if header + LOCAL_HEADER_BYTES > bytes.len()
            || u32_at(bytes, header) != LOCAL_HEADER_SIGNATURE as usize
        {
            return fail(format!(
                "The zip entry \"{name}\" does not start where the index says."
            ));
        }
        let start =
            header + LOCAL_HEADER_BYTES + u16_at(bytes, header + 26) + u16_at(bytes, header + 28);
        if start + entry.compressed_size > bytes.len() {
            return fail(format!(
                "The zip entry \"{name}\" runs past the end of the file."
            ));
        }
        Ok(Some((
            entry.method,
            &bytes[start..start + entry.compressed_size],
        )))
    }

    /// An entry's inflated bytes; `None` when there is no such entry. An entry past
    /// `max_bytes` is refused, since declared sizes are never trusted.
    pub fn read_entry(&self, name: &str, max_bytes: usize) -> Result<Option<Vec<u8>>> {
        let Some((method, data)) = self.raw_entry(name)? else {
            return Ok(None);
        };
        match method {
            8 => inflate(data, max_bytes).map(Some).map_err(|failure| {
                WorkbookError(
                    failure
                        .0
                        .replace("The zip entry", &format!("The zip entry \"{name}\"")),
                )
            }),
            0 if data.len() > max_bytes => fail(format!(
                "The zip entry \"{name}\" is larger than this reader opens."
            )),
            0 => Ok(Some(data.to_vec())),
            other => fail(format!(
                "The zip entry \"{name}\" uses compression method {other}, which is not read."
            )),
        }
    }
}

// --- What JavaScript does with numbers and strings -------------------------------------------

/// JavaScript's white space and line terminators, as `trim` and `\s` match them.
fn is_js_space(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// Hex, octal and binary literal prefixes that `Number(text)` accepts.
const RADIX_PREFIXES: [(&str, u32); 6] = [
    ("0x", 16),
    ("0X", 16),
    ("0o", 8),
    ("0O", 8),
    ("0b", 2),
    ("0B", 2),
];

/// `Number(text)`: a decimal, `Infinity`, or a hexadecimal, octal or binary literal, with white
/// space around it; empty text is 0; anything else is NaN.
pub fn js_number(text: &str) -> f64 {
    let text = js_trim(text);
    if text.is_empty() {
        return 0.0;
    }
    if let Some(value) = read_radix_literal(text) {
        return value;
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned == "Infinity" {
        return if text.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // Rust also parses `inf` and `nan`; JavaScript accepts only digits, a point and an
    // exponent.
    if !unsigned
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// The value of a hex, octal or binary literal (NaN for invalid digits); `None` when the text
/// is not one.
fn read_radix_literal(text: &str) -> Option<f64> {
    let (digits, radix) = RADIX_PREFIXES
        .iter()
        .find_map(|&(prefix, radix)| Some((text.strip_prefix(prefix)?, radix)))?;
    if digits.is_empty() || !digits.chars().all(|digit| digit.is_digit(radix)) {
        return Some(f64::NAN);
    }
    Some(match u128::from_str_radix(digits, radix) {
        Ok(value) => value as f64,
        Err(_) => digits.chars().fold(0.0, |total, digit| {
            total * radix as f64 + digit.to_digit(radix).unwrap_or(0) as f64
        }),
    })
}

/// `Math.round`: halves go up.
fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// A positive finite double's exact decimal digits (no leading zero) and the power of ten of
/// its first digit.
fn exact_digits(magnitude: f64) -> (Vec<u8>, i32) {
    // Every double's expansion has fewer than 780 significant digits.
    let text = format!("{magnitude:.780e}");
    let (mantissa, exponent) = text.split_once('e').expect("an exponent");
    let digits = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    (digits, exponent.parse().expect("a whole exponent"))
}

/// Adds one to the last decimal digit, carrying left. True when the carry overflows the first
/// digit, leaving all `0`s that need a leading `1`.
fn add_one_to_last_digit(digits: &mut [u8]) -> bool {
    for digit in digits.iter_mut().rev() {
        if *digit != b'9' {
            *digit += 1;
            return false;
        }
        *digit = b'0';
    }
    true
}

/// Rounds `digits` to `count`, half up as `toPrecision` and `toExponential` do, adjusting the
/// exponent on carry.
fn round_digits(digits: &[u8], exponent: i32, count: usize) -> (Vec<u8>, i32) {
    let mut kept: Vec<u8> = digits
        .iter()
        .copied()
        .chain(std::iter::repeat(b'0'))
        .take(count)
        .collect();
    let rounds_up = digits.get(count).is_some_and(|&digit| digit >= b'5');
    if rounds_up && add_one_to_last_digit(&mut kept) {
        kept.insert(0, b'1');
        kept.truncate(count);
        return (kept, exponent + 1);
    }
    (kept, exponent)
}

/// An exponent as JavaScript writes it after `e`, always signed.
fn exponent_text(exponent: i32) -> String {
    if exponent < 0 {
        format!("-{}", -exponent)
    } else {
        format!("+{exponent}")
    }
}

/// The shortest round-tripping digits of a finite non-zero magnitude, laid out as
/// `String(value)` does.
fn lay_out_shortest(magnitude: f64) -> String {
    let shortest = format!("{magnitude:e}");
    let (mantissa, exponent) = shortest.split_once('e').expect("an exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = digits.len() as i32;
    let n = exponent.parse::<i32>().expect("a whole exponent") + 1;
    if k <= n && n <= 21 {
        return format!("{digits}{}", "0".repeat((n - k) as usize));
    }
    if 0 < n && n <= 21 {
        return format!("{}.{}", &digits[..n as usize], &digits[n as usize..]);
    }
    if -6 < n && n <= 0 {
        return format!("0.{}{digits}", "0".repeat((-n) as usize));
    }
    let power = exponent_text(n - 1);
    if k == 1 {
        format!("{digits}e{power}")
    } else {
        format!("{}.{}e{power}", &digits[..1], &digits[1..])
    }
}

/// `String(value)`: shortest round-tripping digits in JavaScript's layout.
pub fn js_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    format!("{sign}{}", lay_out_shortest(value.abs()))
}

/// JavaScript's `RangeError` for more than 100 digits, which aborts the read.
fn check_digit_count(count: usize) -> Result<()> {
    if count > 100 {
        return fail("toFixed() digits argument must be between 0 and 100");
    }
    Ok(())
}

/// `value.toFixed(digits)`: the decimal nearest the double, ties to the larger magnitude.
fn js_to_fixed(value: f64, digits: usize) -> Result<String> {
    check_digit_count(digits)?;
    if !value.is_finite() || value.abs() >= 1e21 {
        return Ok(js_to_string(value));
    }
    let exact = format!("{:.1100}", value.abs());
    let (whole, fraction) = exact.split_once('.').expect("a decimal point");
    let mut kept: Vec<u8> = whole.bytes().chain(fraction.bytes().take(digits)).collect();
    if fraction.as_bytes()[digits] >= b'5' && add_one_to_last_digit(&mut kept) {
        kept.insert(0, b'1');
    }
    let split = kept.len() - digits;
    let mut text = String::from(if value < 0.0 { "-" } else { "" });
    text.push_str(std::str::from_utf8(&kept[..split]).expect("digits"));
    if digits > 0 {
        text.push('.');
        text.push_str(std::str::from_utf8(&kept[split..]).expect("digits"));
    }
    Ok(text)
}

/// `value.toExponential(digits)` for a non-negative value.
fn js_to_exponential(value: f64, digits: usize) -> Result<String> {
    if !value.is_finite() {
        return Ok(js_to_string(value));
    }
    check_digit_count(digits)?;
    let (kept, exponent) = if value == 0.0 {
        (vec![b'0'; digits + 1], 0)
    } else {
        let (all, exponent) = exact_digits(value);
        round_digits(&all, exponent, digits + 1)
    };
    let kept = String::from_utf8(kept).expect("digits");
    let power = exponent_text(exponent);
    Ok(if digits == 0 {
        format!("{kept}e{power}")
    } else {
        format!("{}.{}e{power}", &kept[..1], &kept[1..])
    })
}

/// `Number(value.toPrecision(10))`: the double nearest the value rounded to ten digits.
fn round_to_ten_digits(value: f64) -> f64 {
    if value == 0.0 || !value.is_finite() {
        return value;
    }
    let (all, exponent) = exact_digits(value.abs());
    let (kept, exponent) = round_digits(&all, exponent, 10);
    let rounded: f64 = format!(
        "{}e{}",
        String::from_utf8(kept).expect("digits"),
        exponent - 9
    )
    .parse()
    .expect("a number");
    if value < 0.0 {
        -rounded
    } else {
        rounded
    }
}

/// Decodes Excel's `_xHHHH_` escapes for characters XML cannot carry, joining split surrogate
/// pairs.
fn decode_escaped_characters(text: &str) -> String {
    if !text.contains("_x") {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut units: Vec<u16> = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        if bytes[at..].starts_with(b"_x")
            && bytes.len() >= at + 7
            && bytes[at + 2..at + 6].iter().all(u8::is_ascii_hexdigit)
            && bytes[at + 6] == b'_'
        {
            units.push(u16::from_str_radix(&text[at + 2..at + 6], 16).expect("hex digits"));
            at += 7;
            continue;
        }
        let character = text[at..].chars().next().expect("a character");
        let mut buffer = [0u16; 2];
        units.extend_from_slice(character.encode_utf16(&mut buffer));
        at += character.len_utf8();
    }
    String::from_utf16_lossy(&units)
}

// --- XML -------------------------------------------------------------------------------------

/// An XML element: its name without namespace prefix, its attributes with prefixes (`r:id`),
/// its child elements and its direct text.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XmlElement {
    pub name: String,
    attributes: Vec<(String, String)>,
    pub children: Vec<XmlElement>,
    pub text: String,
    /// For each child, how much of `text` came before it, in bytes: where the child sits among
    /// the text (`pieces`).
    text_before: Vec<usize>,
}

/// One piece of an element's content, in document order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Piece<'a> {
    Text(&'a str),
    Child(&'a XmlElement),
}

impl XmlElement {
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn child(&self, name: &str) -> Option<&XmlElement> {
        self.children.iter().find(|child| child.name == name)
    }

    pub(crate) fn children_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a XmlElement> {
        self.children.iter().filter(move |child| child.name == name)
    }

    /// The element's text and children in document order, for content that mixes the two (an
    /// OpenDocument paragraph). Empty stretches of text are left out.
    pub(crate) fn pieces(&self) -> Vec<Piece<'_>> {
        let mut pieces = Vec::with_capacity(self.children.len() * 2 + 1);
        let mut from = 0;
        for (child, &before) in self.children.iter().zip(&self.text_before) {
            if before > from {
                pieces.push(Piece::Text(&self.text[from..before]));
            }
            pieces.push(Piece::Child(child));
            from = before;
        }
        if from < self.text.len() {
            pieces.push(Piece::Text(&self.text[from..]));
        }
        pieces
    }
}

/// The kind of character reference or entity, by what follows `&`.
#[derive(Clone, Copy)]
enum Reference {
    /// `&#x…;`
    Hexadecimal,
    /// `&#…;`
    Decimal,
    /// `&name;`
    Named,
}

impl Reference {
    /// The reference kind at `body_start` (just past `&`), and where its digits or name
    /// start.
    fn at(bytes: &[u8], body_start: usize) -> (Self, usize) {
        if bytes[body_start..].starts_with(b"#x") {
            (Reference::Hexadecimal, body_start + 2)
        } else if bytes.get(body_start) == Some(&b'#') {
            (Reference::Decimal, body_start + 1)
        } else {
            (Reference::Named, body_start)
        }
    }

    fn allows(self, byte: u8) -> bool {
        match self {
            Reference::Hexadecimal => byte.is_ascii_hexdigit(),
            Reference::Decimal => byte.is_ascii_digit(),
            Reference::Named => byte.is_ascii_alphabetic(),
        }
    }

    /// The code point of a numeric reference, `u64::MAX` on overflow; `None` for a named
    /// entity.
    fn code_point(self, digits: &str) -> Option<u64> {
        match self {
            Reference::Hexadecimal => {
                let significant = digits.trim_start_matches('0');
                let overflow = if significant.is_empty() { 0 } else { u64::MAX };
                Some(u64::from_str_radix(significant, 16).unwrap_or(overflow))
            }
            Reference::Decimal => Some(digits.parse::<u64>().unwrap_or(u64::MAX)),
            Reference::Named => None,
        }
    }
}

/// The text of one of XML's five predefined entities.
fn named_entity(name: &str) -> Option<&'static str> {
    match name {
        "lt" => Some("<"),
        "gt" => Some(">"),
        "amp" => Some("&"),
        "quot" => Some("\""),
        "apos" => Some("'"),
        _ => None,
    }
}

/// Decodes the five predefined entities and numeric references; unknown entities stay as
/// written. A reference past U+10FFFF aborts the read, as `String.fromCodePoint` throws.
fn decode_entities(text: &str) -> Result<String> {
    if !text.contains('&') {
        return Ok(text.to_owned());
    }
    let mut units: Vec<u16> = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let next = text[at..]
            .find('&')
            .map_or(text.len(), |offset| at + offset);
        units.extend(text[at..next].encode_utf16());
        if next == text.len() {
            break;
        }
        at = decode_reference(text, next, &mut units)?;
    }
    Ok(String::from_utf16_lossy(&units))
}

/// Decodes the reference at `ampersand` (`&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);`) onto `units`,
/// or a bare `&` when there is none. Returns where scanning resumes.
fn decode_reference(text: &str, ampersand: usize, units: &mut Vec<u16>) -> Result<usize> {
    let bytes = text.as_bytes();
    let (reference, digits_start) = Reference::at(bytes, ampersand + 1);
    let digits_end = (digits_start..bytes.len())
        .find(|&index| !reference.allows(bytes[index]))
        .unwrap_or(bytes.len());
    if digits_end == digits_start || bytes.get(digits_end) != Some(&b';') {
        units.extend("&".encode_utf16());
        return Ok(ampersand + 1);
    }
    let digits = &text[digits_start..digits_end];
    match reference.code_point(digits) {
        Some(code) if code > 0x10ffff => return fail(format!("Invalid code point {digits}")),
        Some(code) if (0xd800..0xe000).contains(&code) => units.push(code as u16),
        Some(code) => {
            let character = char::from_u32(code as u32).expect("a scalar value");
            units.extend_from_slice(character.encode_utf16(&mut [0u16; 2]));
        }
        None => {
            let written = named_entity(digits).unwrap_or(&text[ampersand..=digits_end]);
            units.extend(written.encode_utf16());
        }
    }
    Ok(digits_end + 1)
}

fn remove_prefix(name: &str) -> &str {
    name.split_once(':').map_or(name, |(_, rest)| rest)
}

/// Sets `name`'s value, replacing an earlier one in place.
fn set_by_name<V>(entries: &mut Vec<(String, V)>, name: String, value: V) {
    match entries.iter_mut().find(|(known, _)| *known == name) {
        Some(known) => known.1 = value,
        None => entries.push((name, value)),
    }
}

/// An attribute at the start of a tag's text: name, raw quoted value, and byte length through
/// the closing quote.
struct RawAttribute<'a> {
    name: &'a str,
    value: &'a str,
    length: usize,
}

/// `([^\s=/]+)\s*=\s*(?:"([^"]*)"|'([^']*)')` matched at the start of `text`.
fn match_attribute(text: &str) -> Option<RawAttribute<'_>> {
    let is_name = |character: char| !is_js_space(character) && character != '=' && character != '/';
    let name_length: usize = text
        .chars()
        .take_while(|&character| is_name(character))
        .map(char::len_utf8)
        .sum();
    if name_length == 0 {
        return None;
    }
    let after_name = text[name_length..].trim_start_matches(is_js_space);
    let after_equals = after_name.strip_prefix('=')?;
    let value_start = after_equals.trim_start_matches(is_js_space);
    let quote = value_start
        .chars()
        .next()
        .filter(|&quote| quote == '"' || quote == '\'')?;
    let close = value_start[1..].find(quote)?;
    Some(RawAttribute {
        name: &text[..name_length],
        value: &value_start[1..1 + close],
        length: text.len() - value_start.len() + 1 + close + 1,
    })
}

/// Every match of `([^\s=/]+)\s*=\s*(?:"([^"]*)"|'([^']*)')` in order; a repeated name replaces
/// the earlier value.
fn read_attributes(source: &str) -> Result<Vec<(String, String)>> {
    let mut attributes: Vec<(String, String)> = Vec::new();
    let mut at = 0;
    while at < source.len() {
        let rest = &source[at..];
        let Some(attribute) = match_attribute(rest) else {
            at += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let value = decode_entities(attribute.value)?;
        set_by_name(&mut attributes, attribute.name.to_owned(), value);
        at += attribute.length;
    }
    Ok(attributes)
}

/// The next `terminator` from `from`, or an error for an unclosed `what`.
fn find_closing(source: &str, terminator: &str, from: usize, what: &str) -> Result<usize> {
    source[from..]
        .find(terminator)
        .map(|found| from + found)
        .ok_or_else(|| WorkbookError(format!("Unclosed {what} at offset {from}.")))
}

/// The `>` ending the tag at `tag_start`, skipping any inside quoted attribute values.
fn find_tag_end(bytes: &[u8], tag_start: usize) -> Result<usize> {
    let mut quote = None;
    for (index, &byte) in bytes.iter().enumerate().skip(tag_start) {
        match quote {
            Some(open_quote) if byte == open_quote => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Ok(index),
            None => {}
        }
    }
    fail(format!("Unclosed tag at offset {tag_start}."))
}

/// Parser state: the open elements, innermost last, and the root once closed.
struct XmlParser<'a> {
    source: &'a str,
    open: Vec<XmlElement>,
    root: Option<XmlElement>,
}

impl XmlParser<'_> {
    /// Appends a finished element to its parent, or makes it the root.
    fn place(&mut self, element: XmlElement) {
        match self.open.last_mut() {
            Some(holder) => {
                holder.text_before.push(holder.text.len());
                holder.children.push(element);
            }
            None => self.root = Some(element),
        }
    }

    /// Decodes text between tags into the open element; text outside any element is dropped.
    fn add_text(&mut self, text: &str) -> Result<()> {
        if let Some(parent) = self.open.last_mut() {
            parent.text.push_str(&decode_entities(text)?);
        }
        Ok(())
    }

    /// Reads the markup at `tag_start` and returns where the following text starts.
    fn read_markup(&mut self, tag_start: usize) -> Result<usize> {
        let source = self.source;
        let rest = &source[tag_start..];
        if rest.starts_with("<!--") {
            return Ok(find_closing(source, "-->", tag_start, "comment")? + 3);
        }
        if rest.starts_with("<![CDATA[") {
            let end = find_closing(source, "]]>", tag_start, "CDATA section")?;
            if let Some(parent) = self.open.last_mut() {
                parent.text.push_str(&source[tag_start + 9..end]);
            }
            return Ok(end + 3);
        }
        if rest.starts_with("<?") {
            return Ok(find_closing(source, "?>", tag_start, "processing instruction")? + 2);
        }
        if rest.starts_with("<!") {
            return fail(format!(
                "A document type declaration at offset {tag_start} is not read."
            ));
        }
        let tag_end = find_tag_end(source.as_bytes(), tag_start)?;
        if source.as_bytes().get(tag_start + 1) == Some(&b'/') {
            self.close_element(tag_start, tag_end)?;
        } else {
            self.open_element(tag_start, tag_end)?;
        }
        Ok(tag_end + 1)
    }

    /// `</name>`: closes the innermost open element, which must have that name.
    fn close_element(&mut self, tag_start: usize, tag_end: usize) -> Result<()> {
        let name = remove_prefix(js_trim(&self.source[(tag_start + 2).min(tag_end)..tag_end]));
        let closed = self.open.pop();
        let Some(closed) = closed.filter(|closed| closed.name == name) else {
            return fail(format!("Unexpected </{name}> at offset {tag_start}."));
        };
        self.place(closed);
        Ok(())
    }

    /// `<name …>` opens an element; `<name …/>` is one already closed.
    fn open_element(&mut self, tag_start: usize, tag_end: usize) -> Result<()> {
        let bytes = self.source.as_bytes();
        let self_closing = bytes[tag_end - 1] == b'/' && tag_end - 1 > tag_start;
        let body_end = if self_closing { tag_end - 1 } else { tag_end };
        let body = &self.source[tag_start + 1..body_end];
        let name_end = body.find(is_js_space);
        let element = XmlElement {
            name: remove_prefix(name_end.map_or(body, |end| &body[..end])).to_owned(),
            attributes: match name_end {
                Some(end) => read_attributes(&body[end..])?,
                None => Vec::new(),
            },
            children: Vec::new(),
            text: String::new(),
            text_before: Vec::new(),
        };
        if self_closing {
            self.place(element);
        } else {
            self.open.push(element);
        }
        Ok(())
    }
}

/// Parses a whole XML document and returns its root element. A DOCTYPE is refused, so no
/// entity a file declares is ever expanded.
pub fn parse_xml(source: &str) -> Result<XmlElement> {
    let mut parser = XmlParser {
        source,
        open: Vec::new(),
        root: None,
    };
    let mut offset = 0;
    while offset < source.len() {
        let Some(tag_start) = source[offset..].find('<').map(|found| offset + found) else {
            if let Some(parent) = parser.open.last() {
                return fail(format!("Unclosed element <{}>.", parent.name));
            }
            break;
        };
        if tag_start > offset {
            parser.add_text(&source[offset..tag_start])?;
        }
        offset = parser.read_markup(tag_start)?;
    }
    parser
        .root
        .ok_or_else(|| WorkbookError("The document has no root element.".into()))
}

// --- Number formats ----------------------------------------------------------------------

const GENERAL: &str = "General";

/// Excel's built-in format codes by id; custom codes start at 164.
fn built_in_format(id: f64) -> Option<&'static str> {
    if id.fract() != 0.0 || !(0.0..=49.0).contains(&id) {
        return None;
    }
    Some(match id as u32 {
        0 => GENERAL,
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "m/d/yyyy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yyyy h:mm",
        37 | 38 => "#,##0;(#,##0)",
        39 | 40 => "#,##0.00;(#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// The epoch for serial dates: the 1900 system, or the 1904 system of early Mac Excel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateSystem {
    From1900,
    From1904,
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Literal(String),
    /// A pattern character: a digit placeholder, date letter or separator.
    Pattern(char),
    /// `[h]`, `[m]` or `[s]`: an elapsed total that does not wrap at a day, hour or minute.
    Elapsed(char, usize),
}

/// Tokenizes a format code's `;`-separated sections.
fn tokenize_sections(code: &str) -> Vec<Vec<Token>> {
    let characters: Vec<char> = code.chars().collect();
    let mut sections: Vec<Vec<Token>> = vec![Vec::new()];
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        let section = sections.last_mut().expect("a section");
        match character {
            '"' => {
                let close = (index + 1..characters.len())
                    .find(|&at| characters[at] == '"')
                    .unwrap_or(characters.len());
                section.push(Token::Literal(
                    characters[index + 1..close].iter().collect(),
                ));
                index = close;
            }
            '\\' => {
                section.push(Token::Literal(
                    characters
                        .get(index + 1)
                        .map(char::to_string)
                        .unwrap_or_default(),
                ));
                index += 1;
            }
            // Reserves the width of the next character; a space is close enough.
            '_' => {
                section.push(Token::Literal(" ".into()));
                index += 1;
            }
            // Repeats the next character to fill the column; ignored here.
            '*' => index += 1,
            '[' => {
                let close = (index..characters.len())
                    .find(|&at| characters[at] == ']')
                    .unwrap_or(characters.len());
                let body: String = characters[(index + 1).min(close)..close].iter().collect();
                let first = body.chars().next().map(|first| first.to_ascii_lowercase());
                let elapsed = matches!(first, Some('h' | 'm' | 's'))
                    && body
                        .chars()
                        .all(|letter| same_letter(letter, first.unwrap_or(' ')));
                if elapsed {
                    section.push(Token::Elapsed(
                        first.expect("a letter"),
                        body.chars().count(),
                    ));
                } else if let Some(currency) = body.strip_prefix('$') {
                    // `[$€-407]`: a currency symbol, then a locale id.
                    section.push(Token::Literal(
                        currency.split('-').next().unwrap_or("").to_owned(),
                    ));
                }
                index = close;
            }
            ';' => sections.push(Vec::new()),
            _ => section.push(Token::Pattern(character)),
        }
        index += 1;
    }
    sections
}

fn join_pattern(tokens: &[Token]) -> String {
    tokens
        .iter()
        .filter_map(|token| {
            if let Token::Pattern(character) = token {
                Some(*character)
            } else {
                None
            }
        })
        .collect()
}

/// Whether the pattern contains "general", case-insensitively.
fn holds_general(pattern: &str) -> bool {
    pattern.to_ascii_lowercase().contains("general")
}

fn is_date_section(tokens: &[Token]) -> bool {
    if tokens
        .iter()
        .any(|token| matches!(token, Token::Elapsed(..)))
    {
        return true;
    }
    let pattern = join_pattern(tokens);
    // `/[dmyhs]/iu`, under which the long s (U+017F) folds to `s`.
    !holds_general(&pattern)
        && pattern.chars().any(|letter| {
            matches!(letter.to_ascii_lowercase(), 'd' | 'm' | 'y' | 'h' | 's')
                || letter == '\u{17f}'
        })
}

/// A plain number as Excel's General shows it: up to ten significant digits.
fn format_general(value: f64) -> String {
    if !value.is_finite() {
        return js_to_string(value);
    }
    js_to_string(round_to_ten_digits(value)).replacen('e', "E", 1)
}

/// `value` rounded half away from zero to `decimals` places, as fixed-point text.
fn format_fixed(value: f64, decimals: usize) -> Result<String> {
    let shifted = js_number(&format!("{}e{decimals}", js_to_string(value)));
    // A value written with an exponent cannot be shifted as text, and has no fraction left.
    if !shifted.is_finite() {
        return js_to_fixed(value, decimals);
    }
    js_to_fixed(
        js_number(&format!("{}e-{decimals}", js_to_string(js_round(shifted)))),
        decimals,
    )
}

/// `/\B(?=(\d{3})+(?!\d))/g` replaced with commas: a comma before each run of three digits
/// that ends a run of digits, where the character before is a word character.
fn group_thousands(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let word = |character: char| character.is_ascii_alphanumeric() || character == '_';
    let mut out = String::with_capacity(text.len() + text.len() / 3);
    for (index, &character) in characters.iter().enumerate() {
        if index > 0 && word(characters[index - 1]) && character.is_ascii_digit() {
            let run = characters[index..]
                .iter()
                .take_while(|digit| digit.is_ascii_digit())
                .count();
            if run % 3 == 0 {
                out.push(',');
            }
        }
        out.push(character);
    }
    out
}

/// A section's literal text and percent signs, split into prefix and suffix around its digit
/// pattern.
fn read_affixes(tokens: &[Token]) -> (String, String) {
    let (mut prefix, mut suffix) = (String::new(), String::new());
    let mut seen_digits = false;
    for token in tokens {
        let text = match token {
            Token::Elapsed(..) => continue,
            Token::Pattern('0' | '#' | '?' | '@') => {
                seen_digits = true;
                suffix.clear();
                continue;
            }
            // Separators belong to the digit pattern. An exponent's `E+` is dropped with them,
            // and the placeholders after it restart the suffix.
            Token::Pattern('.' | ',') => continue,
            Token::Pattern(character) => character.to_string(),
            Token::Literal(text) => text.clone(),
        };
        if seen_digits {
            suffix.push_str(&text);
        } else {
            prefix.push_str(&text);
        }
    }
    (prefix, suffix)
}

/// `/[0#?]/`: a character that stands for a digit.
fn is_digit_placeholder(character: char) -> bool {
    matches!(character, '0' | '#' | '?')
}

/// A section's digit pattern (`#,##0.00`), split at its point.
struct DigitPattern<'a> {
    integer: &'a str,
    fraction: &'a str,
    /// Commas after the last integer placeholder; each divides the number by 1000.
    scale_commas: usize,
}

impl<'a> DigitPattern<'a> {
    fn new(digits: &'a str) -> Self {
        let mut parts = digits.split('.');
        let integer = parts.next().unwrap_or("");
        DigitPattern {
            integer,
            fraction: parts.next().unwrap_or(""),
            scale_commas: integer.len() - integer.trim_end_matches(',').len(),
        }
    }

    fn max_decimals(&self) -> usize {
        self.fraction.replace(',', "").len()
    }

    fn min_decimals(&self) -> usize {
        self.fraction.matches('0').count()
    }

    fn min_integer_digits(&self) -> usize {
        self.integer.matches('0').count()
    }

    /// Whether a comma among the integer placeholders asks for thousands separators.
    fn groups_thousands(&self) -> bool {
        self.integer[..self.integer.len() - self.scale_commas].contains(',')
    }
}

/// Formats `scaled` in scientific notation: mantissa to the pattern's decimals, exponent signed
/// and zero-padded to the placeholder count after `E`.
fn format_scientific(scaled: f64, digits: &DigitPattern, pattern: &str) -> Result<String> {
    let written = js_to_exponential(scaled, digits.max_decimals())?;
    let (mantissa, exponent) = written.split_once('e').unwrap_or((&written, "0"));
    let sign = if exponent.starts_with('-') { "-" } else { "+" };
    let exponent_digits = pattern
        .split('E')
        .nth(1)
        .unwrap_or("")
        .chars()
        .filter(|&character| is_digit_placeholder(character))
        .count();
    let exponent = exponent.replacen(['+', '-'], "", 1);
    Ok(format!("{mantissa}E{sign}{exponent:0>exponent_digits$}"))
}

/// Formats `scaled` in fixed point: rounded to the pattern's maximum decimals, trailing zeros
/// trimmed to its minimum, the integer zero-padded and grouped when the pattern asks.
fn format_decimal(scaled: f64, digits: &DigitPattern) -> Result<String> {
    let fixed = format_fixed(scaled, digits.max_decimals())?;
    let mut pieces = fixed.split('.');
    let mut integer = pieces.next().unwrap_or("0").to_owned();
    let mut fraction = pieces.next().unwrap_or("").to_owned();
    let min_decimals = digits.min_decimals();
    while fraction.len() > min_decimals && fraction.ends_with('0') {
        fraction.pop();
    }
    let min_integer_digits = digits.min_integer_digits();
    if min_integer_digits == 0 && integer == "0" && !fraction.is_empty() {
        integer.clear();
    }
    let integer_digits = integer.encode_utf16().count();
    if integer_digits < min_integer_digits {
        integer = format!(
            "{}{integer}",
            "0".repeat(min_integer_digits - integer_digits)
        );
    }
    if digits.groups_thousands() {
        integer = group_thousands(&integer);
    }
    if fraction.is_empty() {
        return Ok(integer);
    }
    Ok(format!("{integer}.{fraction}"))
}

/// A section's literal text, joined.
fn join_literals(tokens: &[Token]) -> String {
    tokens
        .iter()
        .filter_map(|token| {
            if let Token::Literal(text) = token {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect()
}

/// `/[0#?][0#?.,]*/`: the first run of digit placeholders with their separators.
fn find_digit_pattern(pattern: &str) -> Option<&str> {
    let start = pattern.find(is_digit_placeholder)?;
    let length = pattern[start..]
        .find(|character: char| {
            !is_digit_placeholder(character) && character != '.' && character != ','
        })
        .unwrap_or(pattern.len() - start);
    Some(&pattern[start..start + length])
}

fn format_number_section(magnitude: f64, tokens: &[Token]) -> Result<String> {
    let pattern = join_pattern(tokens);
    let (prefix, suffix) = read_affixes(tokens);
    if holds_general(&pattern) {
        return Ok(format!(
            "{}{}",
            join_literals(tokens),
            format_general(magnitude)
        ));
    }
    let Some(digits) = find_digit_pattern(&pattern) else {
        // No digit placeholder: the section is literal text, or `@` for the raw value.
        return Ok(if pattern.contains('@') {
            format!("{prefix}{}{suffix}", format_general(magnitude))
        } else {
            prefix
        });
    };
    if pattern.contains('/') {
        return Ok(format_general(magnitude));
    }
    let digits = DigitPattern::new(digits);
    let percent_signs = pattern.matches('%').count();
    let scaled =
        (magnitude * 100f64.powf(percent_signs as f64)) / 1000f64.powf(digits.scale_commas as f64);
    let number = if pattern.contains("E+") || pattern.contains("E-") {
        format_scientific(scaled, &digits, &pattern)?
    } else {
        format_decimal(scaled, &digits)?
    };
    Ok(format!("{prefix}{number}{suffix}"))
}

#[derive(Clone, Debug, PartialEq)]
enum DatePart {
    Literal(String),
    /// A date or time letter (`M` months, `m` minutes) and its repeat count.
    Field(char, usize),
    Elapsed(char, usize),
    Meridiem {
        short: bool,
    },
    Fraction(usize),
}

/// Whether two letters match under a case-insensitive JavaScript regex.
fn same_letter(a: char, b: char) -> bool {
    let fold = |letter: char| {
        if letter == '\u{17f}' {
            's'
        } else if letter == '\u{212a}' {
            'k'
        } else {
            letter.to_lowercase().next().unwrap_or(letter)
        }
    };
    fold(a) == fold(b)
}

/// Groups a date section's characters into fields; an `m` run next to hours or seconds is
/// minutes.
fn read_date_parts(tokens: &[Token]) -> Vec<DatePart> {
    let mut parts: Vec<DatePart> = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let (part, used) = match &tokens[index] {
            Token::Literal(text) => (DatePart::Literal(text.clone()), 1),
            Token::Elapsed(unit, width) => (DatePart::Elapsed(*unit, *width), 1),
            Token::Pattern(character) => {
                read_pattern_part(*character, &pattern_characters(&tokens[index..]))
            }
        };
        parts.push(part);
        index += used;
    }
    mark_minutes(&mut parts);
    parts
}

/// The leading pattern characters of `tokens`, up to the first literal or elapsed total.
fn pattern_characters(tokens: &[Token]) -> Vec<char> {
    tokens
        .iter()
        .map_while(|token| {
            if let Token::Pattern(character) = token {
                Some(*character)
            } else {
                None
            }
        })
        .collect()
}

/// Whether `characters` start with lowercase `word`, case-insensitively.
fn starts_with_word(characters: &[char], word: &str) -> bool {
    characters.len() >= word.len()
        && characters
            .iter()
            .zip(word.chars())
            .all(|(&a, b)| a.to_ascii_lowercase() == b)
}

/// The date part starting at `character` (the first of `rest`) and how many characters it
/// consumes.
fn read_pattern_part(character: char, rest: &[char]) -> (DatePart, usize) {
    if starts_with_word(rest, "am/pm") {
        return (DatePart::Meridiem { short: false }, 5);
    }
    if starts_with_word(rest, "a/p") {
        return (DatePart::Meridiem { short: true }, 3);
    }
    let letter = character.to_lowercase().collect::<String>();
    if matches!(letter.as_str(), "y" | "m" | "d" | "h" | "s") {
        let width = rest
            .iter()
            .take_while(|&&next| same_letter(next, rest[0]))
            .count();
        let letter = letter.chars().next().expect("a letter");
        return (
            DatePart::Field(if letter == 'm' { 'M' } else { letter }, width),
            width,
        );
    }
    if character == '.' && rest.get(1) == Some(&'0') {
        let digits = rest[1..].iter().take_while(|&&zero| zero == '0').count();
        return (DatePart::Fraction(digits), digits + 1);
    }
    (DatePart::Literal(character.to_string()), 1)
}

/// Turns each month field into minutes when preceded by hours (or an elapsed total) or followed
/// by seconds.
fn mark_minutes(parts: &mut [DatePart]) {
    let times: Vec<usize> = (0..parts.len())
        .filter(|&at| matches!(parts[at], DatePart::Field(..) | DatePart::Elapsed(..)))
        .collect();
    for (position, &at) in times.iter().enumerate() {
        let DatePart::Field('M', width) = parts[at] else {
            continue;
        };
        let after_hours = position.checked_sub(1).is_some_and(|before| {
            matches!(
                parts[times[before]],
                DatePart::Elapsed(..) | DatePart::Field('h', _)
            )
        });
        let before_seconds = times
            .get(position + 1)
            .is_some_and(|&after| matches!(parts[after], DatePart::Field('s', _)));
        if after_hours || before_seconds {
            parts[at] = DatePart::Field('m', width);
        }
    }
}

const MS_PER_DAY: f64 = 86_400_000.0;
/// 1 January 1970 as a 1900-system serial.
const UNIX_EPOCH_SERIAL: f64 = 25_569.0;
/// Days between the 1900 and 1904 epochs.
const DATE_1904_OFFSET_DAYS: f64 = 1462.0;
/// Excel counts a nonexistent 29 February 1900, so serials before it are one day early.
const FIRST_SERIAL_AFTER_PHANTOM_LEAP_DAY: f64 = 61.0;
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// UTC fields as JavaScript's `Date` gives them; `None` for an invalid date, whose fields are
/// NaN.
#[derive(Clone, Copy)]
struct UtcDate {
    year: i64,
    month: usize,
    day: i64,
    weekday: usize,
    hours: i64,
    minutes: i64,
    seconds: i64,
    milliseconds: i64,
}

fn utc_date(milliseconds: f64) -> Option<UtcDate> {
    if !milliseconds.is_finite() || milliseconds.abs() > 8.64e15 {
        return None;
    }
    let time = milliseconds.trunc() as i64;
    let days = time.div_euclid(86_400_000);
    let within = time.rem_euclid(86_400_000);
    let (year, month, day) = crate::calendar::civil_from_days(days);
    Some(UtcDate {
        year,
        month: (month - 1) as usize,
        day,
        weekday: (days + 4).rem_euclid(7) as usize,
        hours: within / 3_600_000,
        minutes: within / 60_000 % 60,
        seconds: within / 1000 % 60,
        milliseconds: within % 1000,
    })
}

/// `String(value).padStart(width, "0")`, with NaN for a field of an invalid date.
fn pad(value: Option<i64>, width: usize) -> String {
    let text = value.map_or_else(|| "NaN".to_owned(), |value| value.to_string());
    format!("{text:0>width$}")
}

/// `names[index]`, or JavaScript's `TypeError` for slicing an invalid date's name, which aborts
/// the read.
fn name_at(names: &[&'static str], index: Option<usize>) -> Result<&'static str> {
    index
        .map(|index| names[index])
        .ok_or_else(|| WorkbookError("Cannot read properties of undefined (reading '0')".into()))
}

/// Values for a date section: the serial's moment, its whole seconds for elapsed totals, and
/// whether the clock is 12-hour.
struct DateFields {
    date: Option<UtcDate>,
    elapsed_seconds: f64,
    twelve_hours: bool,
}

impl DateFields {
    fn new(serial: f64, parts: &[DatePart], system: DateSystem) -> Self {
        let fraction_digits = parts
            .iter()
            .find_map(|part| {
                if let DatePart::Fraction(digits) = part {
                    Some(*digits)
                } else {
                    None
                }
            })
            .unwrap_or(0);
        // Round to the shown precision, so 0.999999 s never reads as :59.
        let unit = 1000.0 / 10f64.powi(fraction_digits.min(3) as i32);
        let serial_1900 = match system {
            DateSystem::From1904 => serial + DATE_1904_OFFSET_DAYS,
            DateSystem::From1900 => serial,
        };
        let days = if serial_1900 < FIRST_SERIAL_AFTER_PHANTOM_LEAP_DAY {
            serial_1900 + 1.0
        } else {
            serial_1900
        };
        DateFields {
            date: utc_date(js_round(((days - UNIX_EPOCH_SERIAL) * MS_PER_DAY) / unit) * unit),
            elapsed_seconds: ((js_round((serial * MS_PER_DAY) / unit) * unit) / 1000.0).floor(),
            twelve_hours: parts
                .iter()
                .any(|part| matches!(part, DatePart::Meridiem { .. })),
        }
    }

    fn write_part(&self, part: &DatePart, out: &mut String) -> Result<()> {
        match *part {
            DatePart::Literal(ref text) => out.push_str(text),
            DatePart::Meridiem { short } => {
                let name = self.meridiem();
                out.push_str(if short { &name[..1] } else { name });
            }
            DatePart::Fraction(digits) => {
                out.push('.');
                let milliseconds = pad(self.date.map(|date| date.milliseconds), 3);
                out.extend(milliseconds.chars().take(digits));
            }
            DatePart::Elapsed(unit, width) => {
                let unit_seconds = match unit {
                    'h' => 3600.0,
                    'm' => 60.0,
                    _ => 1.0,
                };
                let total = (self.elapsed_seconds / unit_seconds).floor();
                out.push_str(&format!("{:0>width$}", js_to_string(total)));
            }
            DatePart::Field(letter, width) => self.write_field(letter, width, out)?,
        }
        Ok(())
    }

    fn meridiem(&self) -> &'static str {
        if self.date.is_some_and(|date| date.hours < 12) {
            "AM"
        } else {
            "PM"
        }
    }

    /// The displayed hour: `hour % 12 || 12` on a 12-hour clock. NaN is falsy, so an invalid
    /// date's hour reads 12.
    fn hour(&self) -> Option<i64> {
        match (self.date, self.twelve_hours) {
            (Some(date), true) if date.hours % 12 == 0 => Some(12),
            (Some(date), true) => Some(date.hours % 12),
            (None, true) => Some(12),
            (date, false) => date.map(|date| date.hours),
        }
    }

    fn write_field(&self, letter: char, width: usize, out: &mut String) -> Result<()> {
        let date = self.date;
        match letter {
            'y' if width <= 2 => out.push_str(&pad(date.map(|date| date.year % 100), 2)),
            'y' => out.push_str(&pad(date.map(|date| date.year), 0)),
            'M' if width >= 5 => {
                out.push_str(&name_at(&MONTH_NAMES, date.map(|date| date.month))?[..1])
            }
            // For an invalid date the bare name is undefined and joins as empty; slicing it
            // throws.
            'M' if width == 4 => out.push_str(date.map_or("", |date| MONTH_NAMES[date.month])),
            'M' if width == 3 => {
                out.push_str(&name_at(&MONTH_NAMES, date.map(|date| date.month))?[..3])
            }
            'M' => out.push_str(&pad(date.map(|date| date.month as i64 + 1), width)),
            'd' if width >= 4 => out.push_str(date.map_or("", |date| WEEKDAY_NAMES[date.weekday])),
            'd' if width == 3 => {
                out.push_str(&name_at(&WEEKDAY_NAMES, date.map(|date| date.weekday))?[..3])
            }
            'd' => out.push_str(&pad(date.map(|date| date.day), width)),
            'h' => out.push_str(&pad(self.hour(), width.min(2))),
            'm' => out.push_str(&pad(date.map(|date| date.minutes), width.min(2))),
            _ => out.push_str(&pad(date.map(|date| date.seconds), width.min(2))),
        }
        Ok(())
    }
}

fn format_date_section(serial: f64, tokens: &[Token], system: DateSystem) -> Result<String> {
    let parts = read_date_parts(tokens);
    let fields = DateFields::new(serial, &parts, system);
    let mut out = String::new();
    for part in &parts {
        fields.write_part(part, &mut out)?;
    }
    Ok(out)
}

/// Formats a number under a format code with up to three sections, `positive;negative;zero`. A
/// negative number without its own section keeps its minus sign.
pub fn format_number(value: f64, code: &str, system: DateSystem) -> Result<String> {
    if !value.is_finite() {
        return Ok(js_to_string(value));
    }
    let sections = tokenize_sections(code);
    let positive = &sections[0];
    if is_date_section(positive) {
        // A negative serial is not a date; Excel fills the cell with # signs.
        return if value < 0.0 {
            Ok(format_general(value))
        } else {
            format_date_section(value, positive, system)
        };
    }
    if value < 0.0 {
        return match sections.get(1).filter(|negative| !negative.is_empty()) {
            Some(negative) => format_number_section(-value, negative),
            None => Ok(format!("-{}", format_number_section(-value, positive)?)),
        };
    }
    if let Some(zero) = sections
        .get(2)
        .filter(|zero| value == 0.0 && !zero.is_empty())
    {
        return format_number_section(0.0, zero);
    }
    format_number_section(value, positive)
}

// --- The workbook ------------------------------------------------------------------------

/// How a cell style renders its cell.
#[derive(Clone, Debug)]
struct CellFormat {
    code: String,
    bold: bool,
    /// `None` for `general`: text left, numbers right, booleans and errors centred.
    alignment: Option<Alignment>,
}

struct Context {
    shared_strings: Vec<String>,
    cell_formats: Vec<CellFormat>,
    system: DateSystem,
}

/// Indexes like JavaScript's `array[Number(text)]`.
fn at_index<T>(items: &[T], index: f64) -> Option<&T> {
    (index.fract() == 0.0 && index >= 0.0 && index < items.len() as f64)
        .then(|| &items[index as usize])
}

pub(crate) fn read_part(archive: &ZipArchive, path: &str) -> Result<Option<XmlElement>> {
    match archive.read_entry(path, PART_MAX_BYTES)? {
        Some(bytes) => parse_xml(&decode_utf8(&bytes)).map(Some),
        None => Ok(None),
    }
}

fn require_part(archive: &ZipArchive, path: &str) -> Result<XmlElement> {
    read_part(archive, path)?
        .ok_or_else(|| WorkbookError(format!("The workbook has no \"{path}\" part.")))
}

fn directory_of(path: &str) -> &str {
    path.rfind('/').map_or("", |slash| &path[..slash])
}

/// A relationship target as a path from the package root, with `.` and `..` resolved.
fn resolve_part_path(base: &str, target: &str) -> String {
    let joined = if target.starts_with('/') {
        target.to_owned()
    } else {
        format!("{base}/{target}")
    };
    let mut segments: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }
    segments.join("/")
}

/// The relationships part path for `part` in `directory` (the package's own when `part` is
/// empty).
fn relationships_path(part: &str, directory: &str) -> String {
    if part.is_empty() {
        "_rels/.rels".to_owned()
    } else if directory.is_empty() {
        format!("_rels/{part}.rels")
    } else {
        format!("{directory}/_rels/{}.rels", &part[directory.len() + 1..])
    }
}

/// A `<Relationship>`'s id, type and target path; `None` when any is missing or the target is
/// outside the package.
fn read_relationship(
    relationship: &XmlElement,
    directory: &str,
) -> Option<(String, String, String)> {
    let (Some(id), Some(kind), Some(target)) = (
        relationship.attribute("Id"),
        relationship.attribute("Type"),
        relationship.attribute("Target"),
    ) else {
        return None;
    };
    if relationship.attribute("TargetMode") == Some("External") {
        return None;
    }
    Some((
        id.to_owned(),
        kind.to_owned(),
        resolve_part_path(directory, target),
    ))
}

/// The relationships of `part` (the package's own when empty) as id, type and target path, in
/// first-seen order.
fn read_relationships(archive: &ZipArchive, part: &str) -> Result<Vec<(String, String, String)>> {
    let directory = directory_of(part);
    let mut relationships: Vec<(String, String, String)> = Vec::new();
    let Some(root) = read_part(archive, &relationships_path(part, directory))? else {
        return Ok(relationships);
    };
    for relationship in root.children_named("Relationship") {
        let Some(entry) = read_relationship(relationship, directory) else {
            continue;
        };
        match relationships
            .iter_mut()
            .find(|(known, _, _)| *known == entry.0)
        {
            Some(known) => *known = entry,
            None => relationships.push(entry),
        }
    }
    Ok(relationships)
}

fn find_relationship<'a>(
    relationships: &'a [(String, String, String)],
    suffix: &str,
) -> Option<&'a str> {
    relationships
        .iter()
        .find(|(_, kind, _)| kind.ends_with(suffix))
        .map(|(_, _, path)| path.as_str())
}

/// A string item's text: its `<t>`, or its rich-text runs joined, skipping phonetic guides.
fn read_string_item(item: &XmlElement) -> String {
    let mut text = item.child("t").map(|t| t.text.clone()).unwrap_or_default();
    for run in item.children_named("r") {
        text.push_str(run.child("t").map_or("", |t| t.text.as_str()));
    }
    decode_escaped_characters(&text)
}

fn is_flag_set(flag: Option<&XmlElement>) -> bool {
    flag.is_some_and(|flag| !matches!(flag.attribute("val"), Some("0" | "false")))
}

fn read_alignment(horizontal: Option<&str>) -> Option<Alignment> {
    match horizontal? {
        "left" => Some(Alignment::Start),
        "right" => Some(Alignment::End),
        "center" | "centerContinuous" => Some(Alignment::Center),
        _ => None,
    }
}

/// Whether two `Number(numFmtId)` values name the same format; NaN is one id.
fn same_format_id(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

/// The styles part's custom format codes by `Number(numFmtId)`; a later code replaces an
/// earlier one with the same id.
fn read_format_codes(styles: &XmlElement) -> Vec<(f64, String)> {
    let mut codes: Vec<(f64, String)> = Vec::new();
    for format in styles
        .child("numFmts")
        .unwrap_or(styles)
        .children_named("numFmt")
    {
        let (Some(id), Some(code)) = (format.attribute("numFmtId"), format.attribute("formatCode"))
        else {
            continue;
        };
        let id = js_number(id);
        match codes
            .iter_mut()
            .find(|(known, _)| same_format_id(*known, id))
        {
            Some(known) => known.1 = code.to_owned(),
            None => codes.push((id, code.to_owned())),
        }
    }
    codes
}

/// The styles part's cell formats, in the index order cells use.
fn read_cell_formats(styles: Option<&XmlElement>) -> Vec<CellFormat> {
    let Some(styles) = styles else {
        return Vec::new();
    };
    let codes = read_format_codes(styles);
    let bold_fonts: Vec<bool> = styles
        .child("fonts")
        .unwrap_or(styles)
        .children_named("font")
        .map(|font| is_flag_set(font.child("b")))
        .collect();
    let Some(formats) = styles.child("cellXfs") else {
        return Vec::new();
    };
    formats
        .children_named("xf")
        .map(|format| {
            let id = js_number(format.attribute("numFmtId").unwrap_or("0"));
            let code = codes
                .iter()
                .find(|(known, _)| same_format_id(*known, id))
                .map(|(_, code)| code.clone())
                .or_else(|| built_in_format(id).map(str::to_owned))
                .unwrap_or_else(|| GENERAL.to_owned());
            CellFormat {
                code,
                bold: at_index(
                    &bold_fonts,
                    js_number(format.attribute("fontId").unwrap_or("0")),
                )
                .copied()
                .unwrap_or(false),
                alignment: read_alignment(
                    format
                        .child("alignment")
                        .and_then(|alignment| alignment.attribute("horizontal")),
                ),
            }
        })
        .collect()
}

/// The zero-based column of a reference such as `B12`; `None` if it is not one.
fn column_index(reference: Option<&str>) -> Option<f64> {
    let reference = reference?;
    let letters = reference.bytes().take_while(u8::is_ascii_uppercase).count();
    let digits = &reference[letters..];
    if letters == 0 || digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(
        reference[..letters]
            .bytes()
            .fold(0.0, |column, letter| column * 26.0 + (letter - 64) as f64)
            - 1.0,
    )
}

/// A cell's text and its type's default alignment.
fn read_cell_text(
    cell: &XmlElement,
    format: &CellFormat,
    context: &Context,
) -> Result<(String, Alignment)> {
    let raw = cell.child("v").map_or("", |v| v.text.as_str());
    Ok(match cell.attribute("t") {
        Some("s") => (
            at_index(&context.shared_strings, js_number(raw))
                .cloned()
                .unwrap_or_default(),
            Alignment::Start,
        ),
        Some("inlineStr") => (
            cell.child("is").map(read_string_item).unwrap_or_default(),
            Alignment::Start,
        ),
        Some("b") => (
            (if raw == "1" { "TRUE" } else { "FALSE" }).to_owned(),
            Alignment::Center,
        ),
        Some("e") => (raw.to_owned(), Alignment::Center),
        None | Some("n") => (
            if raw.is_empty() {
                String::new()
            } else {
                format_number(js_number(raw), &format.code, context.system)?
            },
            Alignment::End,
        ),
        // `str` (a formula's text result), `d` (an ISO date) and unknown types show the value
        // as written.
        Some(_) => (decode_escaped_characters(raw), Alignment::Start),
    })
}

/// Excel's limits: 16,384 columns (XFD) and 1,048,576 rows. A file past them is refused rather
/// than filled with empty cells.
pub(crate) const COLUMN_LIMIT: f64 = 16_384.0;
pub(crate) const ROW_LIMIT: f64 = 1_048_576.0;

/// Converts Excel column widths: points per character, plus padding. Excel measures columns in
/// default-font characters; the grid's text is a little wider and its padding larger, so a
/// column that fits in Excel still fits here.
const COLUMN_WIDTH_CHARACTER: f64 = 8.0;
const COLUMN_WIDTH_PADDING: f64 = 17.0;

/// Sets `items[index]`, padding with defaults up to it.
pub(crate) fn place_at<T: Clone + Default>(items: &mut Vec<T>, index: usize, item: T) {
    if items.len() <= index {
        items.resize(index + 1, T::default());
    }
    items[index] = item;
}

/// A row's zero-based index; a row without a number follows the previous one.
fn read_row_index(sheet: &str, row: &XmlElement, next: usize) -> Result<usize> {
    let index = match row.attribute("r") {
        None => next as f64,
        Some(number) => js_number(number) - 1.0,
    };
    if index.fract() != 0.0 || index < 0.0 || !index.is_finite() || index >= ROW_LIMIT {
        return fail(format!(
            "Sheet \"{sheet}\" has a row numbered \"{}\".",
            row.attribute("r").unwrap_or("undefined")
        ));
    }
    Ok(index as usize)
}

/// A row's cells by column. A cell without a reference follows the previous one; empty cells
/// are skipped.
fn read_row_cells(sheet: &str, row: &XmlElement, context: &Context) -> Result<Vec<Option<Cell>>> {
    let general = CellFormat {
        code: GENERAL.to_owned(),
        bold: false,
        alignment: None,
    };
    let mut cells: Vec<Option<Cell>> = Vec::new();
    for element in row.children_named("c") {
        let column = column_index(element.attribute("r")).unwrap_or(cells.len() as f64);
        let format = at_index(
            &context.cell_formats,
            js_number(element.attribute("s").unwrap_or("0")),
        )
        .unwrap_or(&general);
        let (text, alignment) = read_cell_text(element, format, context)?;
        if text.is_empty() {
            continue;
        }
        if column >= COLUMN_LIMIT {
            return fail(format!(
                "Sheet \"{sheet}\" has a cell past the last column."
            ));
        }
        let cell = Cell {
            text,
            alignment: format.alignment.unwrap_or(alignment),
            bold: format.bold,
        };
        place_at(&mut cells, column as usize, Some(cell));
    }
    Ok(cells)
}

/// Applies a `<col>` width to the columns it spans, up to the last column with a cell.
fn apply_column_width(widths: &mut [f64], column: &XmlElement, column_count: usize) {
    let first = js_number(column.attribute("min").unwrap_or("undefined"));
    let max = js_number(column.attribute("max").unwrap_or("undefined"));
    // `Math.min`: NaN stays NaN.
    let last = if max.is_nan() {
        max
    } else {
        max.min(column_count as f64)
    };
    let width = js_number(column.attribute("width").unwrap_or("undefined"));
    if !first.is_finite() || !last.is_finite() || !width.is_finite() {
        return;
    }
    let mut index = first.max(1.0);
    while index <= last {
        // A fractional index names no column (JavaScript would set a non-index property).
        if index.fract() == 0.0 {
            widths[index as usize - 1] =
                js_round(width * COLUMN_WIDTH_CHARACTER) + COLUMN_WIDTH_PADDING;
        }
        index += 1.0;
    }
}

fn read_sheet(name: &str, worksheet: &XmlElement, context: &Context) -> Result<Sheet> {
    let mut rows: Vec<Vec<Option<Cell>>> = Vec::new();
    let mut column_count = 0;
    let row_elements = worksheet
        .child("sheetData")
        .into_iter()
        .flat_map(|data| data.children_named("row"));
    for row in row_elements {
        let index = read_row_index(name, row, rows.len())?;
        let cells = read_row_cells(name, row, context)?;
        column_count = column_count.max(cells.len());
        place_at(&mut rows, index, cells);
    }
    let mut widths = spreadsheet::measure_column_widths(&rows, column_count);
    for column in worksheet
        .child("cols")
        .into_iter()
        .flat_map(|columns| columns.children_named("col"))
    {
        apply_column_width(&mut widths, column, column_count);
    }
    Ok(spreadsheet::sheet(name, rows, column_count, widths))
}

/// Reads the visible sheets of a workbook held in memory: an OpenDocument spreadsheet when the
/// zip is one (`opendocument::is_spreadsheet`), else an Excel workbook.
pub fn parse(bytes: &[u8]) -> Result<Workbook> {
    let archive = ZipArchive::open(bytes)?;
    if crate::sheets::opendocument::is_spreadsheet(&archive) {
        return crate::sheets::opendocument::parse(&archive);
    }
    let Some(workbook_path) =
        find_relationship(&read_relationships(&archive, "")?, "/officeDocument").map(str::to_owned)
    else {
        return fail("The file is a zip, but not an Excel workbook.");
    };
    let workbook = require_part(&archive, &workbook_path)?;
    let relationships = read_relationships(&archive, &workbook_path)?;
    let shared_strings = match find_relationship(&relationships, "/sharedStrings") {
        Some(path) => read_part(&archive, path)?,
        None => None,
    };
    let styles = match find_relationship(&relationships, "/styles") {
        Some(path) => read_part(&archive, path)?,
        None => None,
    };
    let date_1904 = workbook
        .child("workbookPr")
        .and_then(|properties| properties.attribute("date1904"));
    let context = Context {
        shared_strings: shared_strings
            .map(|strings| strings.children_named("si").map(read_string_item).collect())
            .unwrap_or_default(),
        cell_formats: read_cell_formats(styles.as_ref()),
        system: if matches!(date_1904, Some("1" | "true")) {
            DateSystem::From1904
        } else {
            DateSystem::From1900
        },
    };
    let mut sheets = Vec::new();
    for sheet in workbook
        .child("sheets")
        .into_iter()
        .flat_map(|list| list.children_named("sheet"))
    {
        let relationship = relationships
            .iter()
            .find(|(id, _, _)| id == sheet.attribute("r:id").unwrap_or(""));
        let visible = sheet.attribute("state").unwrap_or("visible") == "visible";
        // Skip chart and macro sheets, which are not cell grids.
        let Some((_, _, path)) =
            relationship.filter(|(_, kind, _)| kind.ends_with("/worksheet") && visible)
        else {
            continue;
        };
        sheets.push(read_sheet(
            sheet.attribute("name").unwrap_or(""),
            &require_part(&archive, path)?,
            &context,
        )?);
    }
    if sheets.is_empty() {
        return fail("The workbook has no visible worksheet.");
    }
    Ok(Workbook { sheets })
}

/// Reads the workbook at `path` for display.
pub fn read(path: &std::path::Path) -> Result<Workbook> {
    let bytes = std::fs::read(path)
        .map_err(|error| WorkbookError(format!("The workbook could not be read ({error})")))?;
    parse(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decoder rejects a truncated stream, trailing junk, the reserved block type, a stored
    /// block whose length and complement disagree, a distance before the start, and an
    /// oversized part.
    #[test]
    fn refuses_what_the_browser_refuses() {
        // "abc" as one fixed block, and as one stored block.
        let fixed = [0x4b, 0x4c, 0x4a, 0x06, 0x00];
        assert_eq!(inflate(&fixed, 100).unwrap(), b"abc");
        assert!(inflate(&fixed[..3], 100).is_err());
        assert!(inflate(&[&fixed[..], &[0]].concat(), 100).is_err());
        assert!(inflate(&fixed, 2).is_err());
        let stored = [0x01, 0x03, 0x00, 0xfc, 0xff, b'a', b'b', b'c'];
        assert_eq!(inflate(&stored, 3).unwrap(), b"abc");
        assert!(inflate(&[0x01, 0x03, 0x00, 0xfc, 0xfe, b'a', b'b', b'c'], 100).is_err());
        assert!(inflate(&stored[..7], 100).is_err());
        assert!(inflate(&[0x07], 100).is_err());
        // A fixed block opening with a length-3 match at distance 1, before any output.
        assert!(inflate(&[0x03, 0x02, 0x00], 100).is_err());
        assert!(inflate(&[], 100).is_err());
    }

    #[test]
    fn reads_numbers_and_writes_them_as_javascript() {
        for (text, value) in [
            ("", 0.0),
            ("  12 ", 12.0),
            ("0x1F", 31.0),
            ("1e3", 1000.0),
            (".5", 0.5),
            ("5.", 5.0),
            ("-Infinity", f64::NEG_INFINITY),
            ("\u{feff}7\u{2028}", 7.0),
        ] {
            assert_eq!(js_number(text), value, "{text:?}");
        }
        for text in [
            "inf", "nan", "NaN", "1_0", "-0x1", "0x", "e5", "1e", "--1", "12px", "infinity",
        ] {
            assert!(js_number(text).is_nan(), "{text:?}");
        }
        for (value, text) in [
            (1e21, "1e+21"),
            (1.5e-7, "1.5e-7"),
            (123456.789, "123456.789"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (-0.0, "0"),
            (2f64.powi(70), "1.1805916207174113e+21"),
            (1e20, "100000000000000000000"),
        ] {
            assert_eq!(js_to_string(value), text);
        }
        assert_eq!(js_to_fixed(0.5, 0).unwrap(), "1");
        assert_eq!(js_to_fixed(2.5, 0).unwrap(), "3");
        assert_eq!(js_to_fixed(1.005, 2).unwrap(), "1.00");
        assert_eq!(js_to_fixed(-0.001, 2).unwrap(), "-0.00");
        assert!(js_to_fixed(1.0, 101).is_err());
        assert_eq!(js_to_exponential(12345.0, 2).unwrap(), "1.23e+4");
        assert_eq!(js_to_exponential(0.0, 2).unwrap(), "0.00e+0");
        assert_eq!(js_to_exponential(9.995, 2).unwrap(), "9.99e+0");
        assert_eq!(js_to_exponential(99.5, 1).unwrap(), "1.0e+2");
        assert_eq!(round_to_ten_digits(1_234_567_890.5), 1_234_567_891.0);
        assert_eq!(group_thousands("1234567"), "1,234,567");
        assert_eq!(group_thousands("123"), "123");
        assert_eq!(group_thousands("1e+21"), "1e+21");
    }

    #[test]
    fn parses_xml_with_entities_and_cdata_and_refuses_a_doctype() {
        let root = parse_xml("<?xml?><a:root x:y='1' z=\"&lt;&#65;&#x42;&zz;\"><b>t&amp;<!-- c --><![CDATA[<d>]]></b><e/></a:root>").unwrap();
        assert_eq!(root.name, "root");
        assert_eq!(root.attribute("x:y"), Some("1"));
        assert_eq!(root.attribute("z"), Some("<AB&zz;"));
        assert_eq!(root.children[0].text, "t&<d>");
        assert_eq!(root.children[1].name, "e");
        assert!(parse_xml("<a><b></a>").is_err());
        assert!(parse_xml("<a>").is_err());
        assert!(parse_xml("<!DOCTYPE a><a/>").is_err());
        assert!(parse_xml("just text").is_err());
        assert_eq!(
            decode_escaped_characters("a_x000D_b_xD83D__xDE00_"),
            "a\rb😀"
        );
    }
}
