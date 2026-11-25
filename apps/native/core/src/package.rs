//! Reads the zip packages and XML documents office files are made of: Excel, PowerPoint, Word
//! and OpenDocument files alike. A package is read from memory; an entry is inflated by
//! miniz_oxide (RFC 1951) only when it is read, rejecting what a browser's decoder rejects.
//! XML is parsed into a tree of elements with namespace prefixes dropped from their names and
//! kept on their attributes; a document type declaration is refused, so no entity a file
//! declares is ever expanded. Office Open XML parts find each other through relationship
//! files (`read_relationships`).
//!
//! White space is trimmed as JavaScript trims it, because `tests/workbook_parity.rs` pins what
//! a browser reader made of the same files.

use crate::archive::{CENTRAL_HEADER_SIGNATURE, END_SIGNATURE, LOCAL_HEADER_SIGNATURE};
use miniz_oxide::inflate::core::inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
use miniz_oxide::inflate::core::{decompress, DecompressorOxide};
use miniz_oxide::inflate::TINFLStatus;

/// A package or document cannot be read: not a zip, damaged, or missing a part. Views only say
/// the file could not be read; the message is for logs and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageError(pub String);

impl std::fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, PackageError>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(PackageError(message.into()))
}

/// Maximum inflated size of one part; a larger part is refused, not truncated.
pub const PART_MAX_BYTES: usize = 64 * 1024 * 1024;

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
pub(crate) fn decode_utf8(bytes: &[u8]) -> String {
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
                PackageError(
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

// --- White space -----------------------------------------------------------------------------

/// JavaScript's white space and line terminators, as `trim` and `\s` match them.
pub(crate) fn is_js_space(character: char) -> bool {
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

pub(crate) fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
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
        .ok_or_else(|| PackageError(format!("Unclosed {what} at offset {from}.")))
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
        .ok_or_else(|| PackageError("The document has no root element.".into()))
}

// --- Parts and relationships ---------------------------------------------------------------

pub(crate) fn read_part(archive: &ZipArchive, path: &str) -> Result<Option<XmlElement>> {
    match archive.read_entry(path, PART_MAX_BYTES)? {
        Some(bytes) => parse_xml(&decode_utf8(&bytes)).map(Some),
        None => Ok(None),
    }
}

pub(crate) fn require_part(archive: &ZipArchive, path: &str) -> Result<XmlElement> {
    read_part(archive, path)?
        .ok_or_else(|| PackageError(format!("The package has no \"{path}\" part.")))
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
pub(crate) fn read_relationships(
    archive: &ZipArchive,
    part: &str,
) -> Result<Vec<(String, String, String)>> {
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

pub(crate) fn find_relationship<'a>(
    relationships: &'a [(String, String, String)],
    suffix: &str,
) -> Option<&'a str> {
    relationships
        .iter()
        .find(|(_, kind, _)| kind.ends_with(suffix))
        .map(|(_, _, path)| path.as_str())
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
    }
}
