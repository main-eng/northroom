//! PDF's objects and the file that holds them: the lexer, the parser, the cross-reference
//! table (as a table or a stream, through every incremental update), objects inside object
//! streams, and streams' filters.
//!
//! A damaged cross-reference is not fatal: when it can't be read, every `n g obj` in the file is
//! found by scanning, as most readers do.

use super::filter;
use super::PdfError;
use std::collections::HashMap;
use std::rc::Rc;

type Result<T> = std::result::Result<T, PdfError>;

fn fail<T>(why: &str) -> Result<T> {
    Err(PdfError::Damaged(why.to_owned()))
}

/// A reference to an indirect object: its number and generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ref(pub u32, pub u16);

#[derive(Clone, Debug, PartialEq)]
pub enum Object {
    Null,
    Bool(bool),
    Integer(i64),
    Real(f64),
    String(Vec<u8>),
    Name(String),
    Array(Vec<Object>),
    Dict(Dict),
    Stream(Stream),
    Ref(Ref),
}

pub type Dict = HashMap<String, Object>;

#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    /// The stream's bytes as stored, before its filters are undone.
    pub raw: Rc<Vec<u8>>,
}

impl Object {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Object::Integer(value) => Some(*value as f64),
            Object::Real(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Object::Integer(value) => Some(*value),
            Object::Real(value) => Some(*value as i64),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&str> {
        match self {
            Object::Name(name) => Some(name),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(dict) => Some(dict),
            Object::Stream(stream) => Some(&stream.dict),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Object::String(bytes) => Some(bytes),
            _ => None,
        }
    }
}

// --- Lexer ---------------------------------------------------------------------------------

pub fn is_white(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

pub fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// A token of PDF's syntax.
#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Integer(i64),
    Real(f64),
    String(Vec<u8>),
    Name(String),
    /// A bare word: an operator, a keyword (`obj`, `R`, `true`), or anything else.
    Word(Vec<u8>),
    ArrayStart,
    ArrayEnd,
    DictStart,
    DictEnd,
    /// `{` or `}`, which only PostScript calculator functions use.
    Brace(u8),
}

pub struct Lexer<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
}

/// How deep strings' parentheses may nest before a file counts as damaged.
const NESTING_MAX: usize = 4096;

impl<'a> Lexer<'a> {
    /// A lexer from `at`, or from the end when `at` is past it, as a damaged offset may be.
    pub fn new(bytes: &'a [u8], at: usize) -> Self {
        Self {
            bytes,
            at: at.min(bytes.len()),
        }
    }

    pub fn skip_white(&mut self) {
        while let Some(&byte) = self.bytes.get(self.at) {
            if is_white(byte) {
                self.at += 1;
            } else if byte == b'%' {
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|byte| !matches!(byte, b'\n' | b'\r'))
                {
                    self.at += 1;
                }
            } else {
                return;
            }
        }
    }

    fn word(&mut self) -> &'a [u8] {
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| !is_white(*byte) && !is_delimiter(*byte))
        {
            self.at += 1;
        }
        &self.bytes[start..self.at]
    }

    fn name(&mut self) -> String {
        self.at += 1;
        let raw = self.word();
        let mut name = Vec::with_capacity(raw.len());
        let mut index = 0;
        while index < raw.len() {
            let hex = raw
                .get(index + 1..index + 3)
                .and_then(|digits| u8::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok());
            match (raw[index], hex) {
                (b'#', Some(value)) => {
                    name.push(value);
                    index += 3;
                }
                (byte, _) => {
                    name.push(byte);
                    index += 1;
                }
            }
        }
        String::from_utf8_lossy(&name).into_owned()
    }

    fn escape(&mut self, out: &mut Vec<u8>) {
        let Some(&next) = self.bytes.get(self.at) else {
            return;
        };
        self.at += 1;
        match next {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'\r' => {
                if self.bytes.get(self.at) == Some(&b'\n') {
                    self.at += 1;
                }
            }
            b'\n' => {}
            b'0'..=b'7' => {
                let mut value = u32::from(next - b'0');
                for _ in 0..2 {
                    match self.bytes.get(self.at) {
                        Some(digit @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(digit - b'0');
                            self.at += 1;
                        }
                        _ => break,
                    }
                }
                out.push(value as u8);
            }
            other => out.push(other),
        }
    }

    fn literal(&mut self) -> Vec<u8> {
        self.at += 1;
        let mut out = Vec::new();
        let mut depth = 1usize;
        while let Some(&byte) = self.bytes.get(self.at) {
            self.at += 1;
            match byte {
                b'\\' => self.escape(&mut out),
                b'(' => {
                    depth = (depth + 1).min(NESTING_MAX);
                    out.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(byte);
                }
                _ => out.push(byte),
            }
        }
        out
    }

    fn hex_string(&mut self) -> Vec<u8> {
        self.at += 1;
        let mut digits = Vec::new();
        while let Some(&byte) = self.bytes.get(self.at) {
            self.at += 1;
            if byte == b'>' {
                break;
            }
            if let Some(digit) = char::from(byte).to_digit(16) {
                digits.push(digit as u8);
            }
        }
        if digits.len() % 2 == 1 {
            digits.push(0);
        }
        digits
            .chunks(2)
            .map(|pair| pair[0] * 16 + pair[1])
            .collect()
    }

    fn number(word: &[u8]) -> Option<Token> {
        let text = std::str::from_utf8(word).ok()?;
        if let Ok(value) = text.parse::<i64>() {
            return Some(Token::Integer(value));
        }
        // Some writers put several signs or a trailing point.
        let cleaned = text.trim_start_matches(['+', '-']);
        let negative = text.starts_with('-');
        let value: f64 = cleaned
            .parse()
            .ok()
            .or_else(|| (cleaned == "." || cleaned.is_empty()).then_some(0.0))?;
        Some(Token::Real(if negative { -value } else { value }))
    }

    pub fn next_token(&mut self) -> Option<Token> {
        self.skip_white();
        let &byte = self.bytes.get(self.at)?;
        Some(match byte {
            b'/' => Token::Name(self.name()),
            b'(' => Token::String(self.literal()),
            b'<' if self.bytes.get(self.at + 1) == Some(&b'<') => {
                self.at += 2;
                Token::DictStart
            }
            b'<' => Token::String(self.hex_string()),
            b'>' if self.bytes.get(self.at + 1) == Some(&b'>') => {
                self.at += 2;
                Token::DictEnd
            }
            b'[' | b']' | b'{' | b'}' | b'>' | b')' => {
                self.at += 1;
                match byte {
                    b'[' => Token::ArrayStart,
                    b']' => Token::ArrayEnd,
                    other => Token::Brace(other),
                }
            }
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                let word = self.word();
                Self::number(word).unwrap_or_else(|| Token::Word(word.to_vec()))
            }
            _ => Token::Word(self.word().to_vec()),
        })
    }
}

// --- Parser --------------------------------------------------------------------------------

/// How deep arrays and dictionaries may nest before a file counts as damaged.
const DEPTH_MAX: usize = 256;

/// Parses objects from tokens, turning `n g R` into references.
pub struct Parser<'a> {
    pub lexer: Lexer<'a>,
    pending: Vec<Token>,
}

impl<'a> Parser<'a> {
    pub fn new(bytes: &'a [u8], at: usize) -> Self {
        Self {
            lexer: Lexer::new(bytes, at),
            pending: Vec::new(),
        }
    }

    pub fn next_token(&mut self) -> Option<Token> {
        self.pending.pop().or_else(|| self.lexer.next_token())
    }

    pub(crate) fn put_back(&mut self, token: Token) {
        self.pending.push(token);
    }

    /// An integer, or a reference when two integers and `R` follow.
    fn integer(&mut self, value: i64) -> Object {
        let Some(second) = self.next_token() else {
            return Object::Integer(value);
        };
        let Token::Integer(generation) = second else {
            self.put_back(second);
            return Object::Integer(value);
        };
        match self.next_token() {
            Some(Token::Word(word)) if word == b"R" => {
                Object::Ref(Ref(value.max(0) as u32, generation.max(0) as u16))
            }
            third => {
                if let Some(third) = third {
                    self.put_back(third);
                }
                self.put_back(Token::Integer(generation));
                Object::Integer(value)
            }
        }
    }

    pub fn object(&mut self, depth: usize) -> Option<Object> {
        if depth > DEPTH_MAX {
            return None;
        }
        Some(match self.next_token()? {
            Token::Integer(value) => self.integer(value),
            Token::Real(value) => Object::Real(value),
            Token::String(bytes) => Object::String(bytes),
            Token::Name(name) => Object::Name(name),
            Token::ArrayStart => {
                let mut items = Vec::new();
                loop {
                    match self.next_token()? {
                        Token::ArrayEnd => break,
                        token => {
                            self.put_back(token);
                            items.push(self.object(depth + 1)?);
                        }
                    }
                }
                Object::Array(items)
            }
            Token::DictStart => Object::Dict(self.dict_body(depth)?),
            Token::Word(word) => match word.as_slice() {
                b"true" => Object::Bool(true),
                b"false" => Object::Bool(false),
                _ => Object::Null,
            },
            Token::ArrayEnd | Token::DictEnd | Token::Brace(_) => Object::Null,
        })
    }

    fn dict_body(&mut self, depth: usize) -> Option<Dict> {
        let mut dict = Dict::new();
        loop {
            match self.next_token()? {
                Token::DictEnd => return Some(dict),
                Token::Name(key) => {
                    let value = self.object(depth + 1)?;
                    dict.insert(key, value);
                }
                _ => {}
            }
        }
    }
}

// --- The document --------------------------------------------------------------------------

/// Where an object is: at a byte offset, or inside an object stream.
#[derive(Clone, Copy, Debug)]
enum Place {
    Offset(usize),
    InStream(u32, usize),
}

/// A PDF file in memory: its bytes, where each object is, its trailer, and the objects read so
/// far.
pub struct Document {
    pub bytes: Vec<u8>,
    places: HashMap<u32, Place>,
    pub trailer: Dict,
    cache: std::cell::RefCell<HashMap<u32, Rc<Object>>>,
    /// Object streams decoded once, by their object number.
    object_streams: std::cell::RefCell<HashMap<u32, Rc<ObjectStream>>>,
    pub(super) crypt: Option<super::crypt::Crypt>,
}

/// An object stream's decoded bytes and where each object in it starts.
struct ObjectStream {
    data: Vec<u8>,
    /// Each object's number and its offset from `first`.
    offsets: Vec<(u32, usize)>,
    first: usize,
}

/// The entries of a cross-reference section of `count` from object `first`: each object in use
/// and where it starts.
fn read_xref_section(parser: &mut Parser, first: i64, count: i64) -> Result<Vec<(u32, usize)>> {
    let mut places = Vec::new();
    for index in 0..count.max(0) {
        let entry = (
            parser.next_token(),
            parser.next_token(),
            parser.next_token(),
        );
        let (Some(Token::Integer(offset)), Some(_), Some(Token::Word(kind))) = entry else {
            return fail("A cross-reference entry is damaged.");
        };
        if kind == b"n" && offset > 0 {
            places.push(((first + index) as u32, offset as usize));
        }
    }
    Ok(places)
}

/// The object number and start of the `n g obj` that ends at `end`, if the bytes before it are
/// an object's number and generation.
fn object_before(bytes: &[u8], end: usize) -> Option<(u32, usize)> {
    let white_before = |mut at: usize| {
        while at > 0 && is_white(bytes[at - 1]) {
            at -= 1;
        }
        at
    };
    let digits_before = |mut at: usize| {
        while at > 0 && bytes[at - 1].is_ascii_digit() {
            at -= 1;
        }
        at
    };
    let generation_end = white_before(end);
    let generation_start = digits_before(generation_end);
    let number_end = white_before(generation_start);
    let number_start = digits_before(number_end);
    let separated = generation_start < generation_end
        && number_end < generation_start
        && number_start < number_end;
    if !separated || end - number_start > 24 {
        return None;
    }
    std::str::from_utf8(&bytes[generation_start..generation_end])
        .ok()?
        .parse::<u16>()
        .ok()?;
    let number = std::str::from_utf8(&bytes[number_start..number_end])
        .ok()?
        .parse::<u32>()
        .ok()?;
    Some((number, number_start))
}

fn find_last(bytes: &[u8], word: &[u8]) -> Option<usize> {
    bytes.windows(word.len()).rposition(|window| window == word)
}

impl Document {
    pub fn open(bytes: Vec<u8>) -> Result<Self> {
        Self::open_with(bytes, None)
    }

    /// Opens a document, decrypting it with `password` when an empty one doesn't open it.
    pub fn open_with(bytes: Vec<u8>, password: Option<&[u8]>) -> Result<Self> {
        // A PDF begins `%PDF-`, though some lose it; their objects show what they are.
        let head = &bytes[..bytes.len().min(1024)];
        let has_header = head.windows(5).any(|window| window == b"%PDF-");
        if !has_header && !head.windows(4).any(|window| window == b" obj") {
            return fail("The file is not a PDF.");
        }
        let mut document = Document {
            bytes,
            places: HashMap::new(),
            trailer: Dict::new(),
            cache: Default::default(),
            object_streams: Default::default(),
            crypt: None,
        };
        if document.read_xref_chain().is_err() || !document.trailer.contains_key("Root") {
            document.places.clear();
            document.scan_objects();
        }
        document.crypt = super::crypt::Crypt::from_document(&document, password)?;
        // Objects read while finding the encryption were read before it applied.
        document.cache.borrow_mut().clear();
        document.object_streams.borrow_mut().clear();
        Ok(document)
    }

    /// Reads every cross-reference section from the last `startxref`, newest first: an entry
    /// already known is not replaced by an older one.
    fn read_xref_chain(&mut self) -> Result<()> {
        let start = find_last(&self.bytes, b"startxref")
            .ok_or_else(|| PdfError::Damaged("No startxref.".into()))?;
        let mut parser = Parser::new(&self.bytes, start + 9);
        let Some(Token::Integer(mut at)) = parser.next_token() else {
            return fail("startxref names no offset.");
        };
        let mut seen = std::collections::HashSet::new();
        while at > 0 && seen.insert(at) {
            let trailer = self.read_xref_at(at as usize)?;
            for (key, value) in &trailer {
                self.trailer
                    .entry(key.clone())
                    .or_insert_with(|| value.clone());
            }
            if let Some(stream) = trailer.get("XRefStm").and_then(Object::as_i64) {
                let _ = self.read_xref_at(stream as usize);
            }
            at = trailer.get("Prev").and_then(Object::as_i64).unwrap_or(0);
        }
        Ok(())
    }

    fn read_xref_at(&mut self, at: usize) -> Result<Dict> {
        let mut lexer = Lexer::new(&self.bytes, at);
        lexer.skip_white();
        if self.bytes[lexer.at..].starts_with(b"xref") {
            return self.read_xref_table(lexer.at + 4);
        }
        self.read_xref_stream(lexer.at)
    }

    fn read_xref_table(&mut self, at: usize) -> Result<Dict> {
        let mut parser = Parser::new(&self.bytes, at);
        loop {
            match parser.next_token() {
                Some(Token::Integer(first)) => {
                    let Some(Token::Integer(count)) = parser.next_token() else {
                        return fail("A cross-reference section has no count.");
                    };
                    let places = read_xref_section(&mut parser, first, count)?;
                    for (number, offset) in places {
                        self.places.entry(number).or_insert(Place::Offset(offset));
                    }
                }
                Some(Token::Word(word)) if word == b"trailer" => {
                    return parser
                        .object(0)
                        .and_then(|trailer| trailer.as_dict().cloned())
                        .ok_or_else(|| PdfError::Damaged("The trailer is damaged.".into()));
                }
                _ => return fail("A cross-reference table is damaged."),
            }
        }
    }

    fn read_xref_stream(&mut self, at: usize) -> Result<Dict> {
        let (_, object) = self.parse_indirect(at)?;
        let Object::Stream(stream) = object else {
            return fail("A cross-reference stream is not a stream.");
        };
        let data = filter::decode(&stream, None)?;
        let widths: Vec<usize> = stream
            .dict
            .get("W")
            .and_then(Object::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.as_i64().unwrap_or(0).max(0) as usize)
                    .collect()
            })
            .unwrap_or_default();
        if widths.len() < 3 || widths.iter().any(|width| *width > 8) {
            return fail("A cross-reference stream's widths are damaged.");
        }
        let size = stream
            .dict
            .get("Size")
            .and_then(Object::as_i64)
            .unwrap_or(0);
        let index: Vec<i64> = stream
            .dict
            .get("Index")
            .and_then(Object::as_array)
            .map(|items| items.iter().filter_map(Object::as_i64).collect())
            .unwrap_or_else(|| vec![0, size]);
        let row = widths.iter().sum::<usize>();
        let mut rows = data.chunks_exact(row.max(1));
        for pair in index.as_chunks::<2>().0 {
            for number in pair[0]..pair[0] + pair[1].max(0) {
                let Some(entry) = rows.next() else {
                    break;
                };
                self.take_stream_entry(number as u32, entry, &widths);
            }
        }
        Ok(stream.dict)
    }

    fn take_stream_entry(&mut self, number: u32, entry: &[u8], widths: &[usize]) {
        let field = |index: usize, default: u64| -> u64 {
            let start: usize = widths[..index].iter().sum();
            if widths[index] == 0 {
                return default;
            }
            entry[start..start + widths[index]]
                .iter()
                .fold(0u64, |value, byte| value << 8 | u64::from(*byte))
        };
        let place = match field(0, 1) {
            1 => Place::Offset(field(1, 0) as usize),
            2 => Place::InStream(field(1, 0) as u32, field(2, 0) as usize),
            _ => return,
        };
        self.places.entry(number).or_insert(place);
    }

    /// Finds every object by scanning for `n g obj`, and the last trailer or the catalog.
    fn scan_objects(&mut self) {
        let mut at = 0;
        while let Some(found) = self.bytes[at..]
            .windows(3)
            .position(|window| window == b"obj")
        {
            let end = at + found;
            at = end + 3;
            if let Some((number, start)) = object_before(&self.bytes, end) {
                self.places.insert(number, Place::Offset(start));
            }
        }
        if let Some(trailer) = find_last(&self.bytes, b"trailer") {
            let mut parser = Parser::new(&self.bytes, trailer + 7);
            if let Some(Object::Dict(dict)) = parser.object(0) {
                self.trailer = dict;
            }
        }
        if !self.trailer.contains_key("Root") {
            if let Some(number) = self.find_catalog() {
                self.trailer
                    .insert("Root".into(), Object::Ref(Ref(number, 0)));
            }
        }
    }

    /// The first object whose type is `Catalog`.
    fn find_catalog(&self) -> Option<u32> {
        let mut numbers: Vec<u32> = self.places.keys().copied().collect();
        numbers.sort_unstable();
        numbers.into_iter().find(|number| {
            self.get(*number)
                .as_dict()
                .and_then(|dict| dict.get("Type"))
                .and_then(Object::as_name)
                == Some("Catalog")
        })
    }

    /// The indirect object at `at` (`n g obj … endobj`), with its stream if it has one.
    fn parse_indirect(&self, at: usize) -> Result<(Ref, Object)> {
        let mut parser = Parser::new(&self.bytes, at);
        let (
            Some(Token::Integer(number)),
            Some(Token::Integer(generation)),
            Some(Token::Word(word)),
        ) = (
            parser.next_token(),
            parser.next_token(),
            parser.next_token(),
        )
        else {
            return fail("An object does not start where the cross-reference says.");
        };
        if word != b"obj" {
            return fail("An object does not start with obj.");
        }
        let object = parser.object(0).unwrap_or(Object::Null);
        let reference = Ref(number as u32, generation as u16);
        let Object::Dict(dict) = object else {
            return Ok((reference, object));
        };
        let after = parser.lexer.at;
        match parser.next_token() {
            Some(Token::Word(word)) if word == b"stream" => {}
            _ => return Ok((reference, Object::Dict(dict))),
        }
        let mut start = parser.lexer.at;
        if self.bytes.get(start) == Some(&b'\r') {
            start += 1;
        }
        if self.bytes.get(start) == Some(&b'\n') {
            start += 1;
        }
        let declared = dict
            .get("Length")
            .map(|length| self.resolve(length))
            .and_then(|length| length.as_i64());
        let end = self.stream_end(start, declared);
        let _ = after;
        let raw = Rc::new(self.bytes[start..end].to_vec());
        Ok((reference, Object::Stream(Stream { dict, raw })))
    }

    /// Where a stream's data ends: at its declared length when `endstream` follows it, else
    /// before the next `endstream`.
    fn stream_end(&self, start: usize, declared: Option<i64>) -> usize {
        if let Some(length) = declared.filter(|length| *length >= 0) {
            let end = start.saturating_add(length as usize).min(self.bytes.len());
            let mut lexer = Lexer::new(&self.bytes, end);
            lexer.skip_white();
            if self.bytes[lexer.at..].starts_with(b"endstream") {
                return end;
            }
        }
        let found = self.bytes[start..]
            .windows(9)
            .position(|window| window == b"endstream")
            .map_or(self.bytes.len(), |offset| start + offset);
        let mut end = found;
        while end > start && matches!(self.bytes[end - 1], b'\r' | b'\n') {
            end -= 1;
        }
        end
    }

    fn load(&self, number: u32) -> Option<Object> {
        match *self.places.get(&number)? {
            Place::Offset(at) => {
                let (reference, object) = self.parse_indirect(at).ok()?;
                Some(self.decrypted(reference, object))
            }
            Place::InStream(container, index) => self.load_from_stream(container, index, number),
        }
    }

    fn decrypted(&self, reference: Ref, object: Object) -> Object {
        match &self.crypt {
            Some(crypt) => crypt.decrypt_object(reference, object),
            None => object,
        }
    }

    /// Object stream `container`, decoded and indexed on first use.
    fn object_stream(&self, container: u32) -> Option<Rc<ObjectStream>> {
        if let Some(known) = self.object_streams.borrow().get(&container) {
            return Some(known.clone());
        }
        let holder = self.get(container);
        let Object::Stream(stream) = holder.as_ref() else {
            return None;
        };
        let data = filter::decode(stream, None).ok()?;
        let count = stream
            .dict
            .get("N")
            .and_then(Object::as_i64)?
            .clamp(0, 100_000) as usize;
        let first = stream.dict.get("First").and_then(Object::as_i64)?.max(0) as usize;
        let mut parser = Parser::new(&data, 0);
        let mut offsets = Vec::with_capacity(count);
        for _ in 0..count {
            let (Some(Token::Integer(object)), Some(Token::Integer(offset))) =
                (parser.next_token(), parser.next_token())
            else {
                break;
            };
            offsets.push((object as u32, offset.max(0) as usize));
        }
        let decoded = Rc::new(ObjectStream {
            data,
            offsets,
            first,
        });
        self.object_streams
            .borrow_mut()
            .insert(container, decoded.clone());
        Some(decoded)
    }

    fn load_from_stream(&self, container: u32, index: usize, number: u32) -> Option<Object> {
        let decoded = self.object_stream(container)?;
        let (_, offset) = decoded
            .offsets
            .get(index)
            .filter(|(object, _)| *object == number)
            .or_else(|| decoded.offsets.iter().find(|(object, _)| *object == number))?;
        Parser::new(&decoded.data, decoded.first + offset).object(0)
    }

    /// Finds every object by scanning the file, for when its cross-references lead nowhere.
    pub fn rescan(&mut self) {
        self.places.clear();
        self.cache.borrow_mut().clear();
        self.object_streams.borrow_mut().clear();
        let trailer = std::mem::take(&mut self.trailer);
        self.scan_objects();
        // Keep what the old trailer said that the scan didn't find, such as its encryption.
        for (key, value) in trailer {
            self.trailer.entry(key).or_insert(value);
        }
    }

    /// The numbers of the objects the file has, in order.
    pub fn numbers(&self) -> Vec<u32> {
        let mut numbers: Vec<u32> = self.places.keys().copied().collect();
        numbers.sort_unstable();
        numbers
    }

    /// Object `number`, read once.
    pub fn get(&self, number: u32) -> Rc<Object> {
        if let Some(known) = self.cache.borrow().get(&number) {
            return known.clone();
        }
        let object = Rc::new(self.load(number).unwrap_or(Object::Null));
        self.cache.borrow_mut().insert(number, object.clone());
        object
    }

    /// `object` itself, or what it refers to (following a few references in a row).
    pub fn resolve(&self, object: &Object) -> Object {
        let mut current = object.clone();
        for _ in 0..16 {
            match current {
                Object::Ref(Ref(number, _)) => current = (*self.get(number)).clone(),
                other => return other,
            }
        }
        Object::Null
    }

    /// A dictionary's entry, resolved.
    pub fn entry(&self, dict: &Dict, key: &str) -> Object {
        dict.get(key)
            .map_or(Object::Null, |value| self.resolve(value))
    }

    /// A stream's bytes with its filters undone.
    pub fn stream_data(&self, stream: &Stream) -> Result<Vec<u8>> {
        filter::decode(stream, Some(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_parse_with_strings_names_and_references() {
        let mut parser = Parser::new(
            b"<< /Type /Page /Name#20One (a\\(b\\)\\101) /Kids [1 0 R 2 3.5] /H <48 49> /N null >>",
            0,
        );
        let object = parser.object(0).expect("an object");
        let dict = object.as_dict().expect("a dictionary");
        assert_eq!(dict["Type"], Object::Name("Page".into()));
        assert_eq!(dict["Name One"], Object::String(b"a(b)A".to_vec()));
        assert_eq!(
            dict["Kids"],
            Object::Array(vec![
                Object::Ref(Ref(1, 0)),
                Object::Integer(2),
                Object::Real(3.5)
            ])
        );
        assert_eq!(dict["H"], Object::String(b"HI".to_vec()));
        assert_eq!(dict["N"], Object::Null);
    }
}
