//! Reads an embedded Type 1 font program (a PDF's `FontFile`): its cleartext part (font matrix
//! and encoding) and its eexec-encrypted part (subroutines and charstrings), and draws a glyph
//! by interpreting its charstring. Hints are read and ignored; flex is drawn as its two curves;
//! accented characters built with `seac` are composed.

use super::encoding::Base;
use crate::deck::Command;
use std::collections::HashMap;

/// The eexec and charstring keys and constants.
const EEXEC_KEY: u16 = 55665;
const CHARSTRING_KEY: u16 = 4330;
const C1: u16 = 52845;
const C2: u16 = 22719;
/// How deep subroutine calls may nest before a font counts as damaged.
const CALLS_MAX: usize = 32;

fn decrypt(data: &[u8], key: u16, skip: usize) -> Vec<u8> {
    let mut r = key;
    let mut out = Vec::with_capacity(data.len());
    for &cipher in data {
        let plain = cipher ^ (r >> 8) as u8;
        r = (u16::from(cipher).wrapping_add(r))
            .wrapping_mul(C1)
            .wrapping_add(C2);
        out.push(plain);
    }
    out.into_iter().skip(skip).collect()
}

/// The encrypted part as bytes: binary as written, or turned from hex when it is hex.
fn encrypted_bytes(data: &[u8]) -> Vec<u8> {
    let start = data
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(0);
    let body = &data[start..];
    let is_hex = body.iter().take(4).all(u8::is_ascii_hexdigit);
    if !is_hex {
        return body.to_vec();
    }
    let digits: Vec<u8> = body
        .iter()
        .filter_map(|byte| char::from(*byte).to_digit(16).map(|digit| digit as u8))
        .collect();
    digits
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| pair[0] * 16 + pair[1])
        .collect()
}

/// A Type 1 font: its glyphs' charstrings by name, its subroutines, its own encoding, and its
/// font matrix's scale (usually a thousandth).
pub struct Type1 {
    charstrings: HashMap<String, Vec<u8>>,
    subrs: Vec<Vec<u8>>,
    pub encoding: HashMap<u8, String>,
    pub scale: f64,
}

/// The binary blocks after `name` in the private dictionary: `dup i n RD <n bytes>` for
/// subroutines, `/glyph n RD <n bytes>` for charstrings, each found with its key.
fn read_blocks(private: &[u8], marker: &[u8], skip: usize) -> Vec<(String, Vec<u8>)> {
    let Some(start) = find(private, marker) else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    let mut lexer = super::object::Lexer::new(private, start + marker.len());
    let mut words: Vec<String> = Vec::new();
    while let Some(token) = lexer.next_token() {
        let word = word_of(token);
        if matches!(word.as_str(), "RD" | "-|") {
            blocks.push(read_block(private, &mut lexer, &words, skip));
            words.clear();
            continue;
        }
        if word == "end" && !blocks.is_empty() && marker == b"/CharStrings" {
            break;
        }
        words.push(word);
        if words.len() > 8 {
            words.remove(0);
        }
    }
    blocks
}

fn word_of(token: super::object::Token) -> String {
    use super::object::Token;
    match token {
        Token::Integer(value) => value.to_string(),
        Token::Name(name) => format!("/{name}"),
        Token::Word(word) => String::from_utf8_lossy(&word).into_owned(),
        _ => String::new(),
    }
}

/// The block after `RD`: its key and length are the two words before, its bytes follow one
/// space on, and the lexer moves past them.
fn read_block(
    private: &[u8],
    lexer: &mut super::object::Lexer,
    words: &[String],
    skip: usize,
) -> (String, Vec<u8>) {
    let length: usize = words
        .last()
        .and_then(|length| length.parse().ok())
        .unwrap_or(0);
    let key = words.iter().rev().nth(1).cloned().unwrap_or_default();
    let at = lexer.at + 1;
    let data = private.get(at..at + length).unwrap_or(&[]);
    lexer.at = at + length;
    (key, decrypt(data, CHARSTRING_KEY, skip))
}

fn find(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len())
        .position(|window| window == needle)
}

/// The font's own encoding: `StandardEncoding`, or `dup code /name put` entries.
fn read_encoding(clear: &[u8]) -> HashMap<u8, String> {
    let Some(start) = find(clear, b"/Encoding") else {
        return HashMap::new();
    };
    let rest = &clear[start + 9..];
    let text = String::from_utf8_lossy(&rest[..rest.len().min(40_000)]);
    if text.trim_start().starts_with("StandardEncoding") {
        return (0..=255u8)
            .filter_map(|code| {
                let character = Base::Standard.char_of(code)?;
                Some((code, super::encoding::name_of_char(character)))
            })
            .collect();
    }
    let mut encoding = HashMap::new();
    let words: Vec<&str> = text
        .split_ascii_whitespace()
        .take_while(|word| *word != "readonly" && *word != "def")
        .collect();
    for window in words.windows(4) {
        if let ["dup", code, name, "put"] = window {
            if let (Ok(code), Some(name)) = (code.parse::<u8>(), name.strip_prefix('/')) {
                encoding.insert(code, name.to_owned());
            }
        }
    }
    encoding
}

fn read_scale(clear: &[u8]) -> f64 {
    let Some(start) = find(clear, b"/FontMatrix") else {
        return 0.001;
    };
    let rest =
        String::from_utf8_lossy(&clear[start + 11..(start + 120).min(clear.len())]).into_owned();
    rest.trim_start()
        .trim_start_matches(['[', '{'])
        .split_ascii_whitespace()
        .next()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| *value > 0.0)
        .unwrap_or(0.001)
}

impl Type1 {
    /// Reads a font program: `clear_length` bytes of cleartext, then the encrypted part.
    pub fn read(data: &[u8], clear_length: usize) -> Option<Self> {
        let data = strip_pfb(data);
        let split = if clear_length > 0 && clear_length < data.len() {
            clear_length
        } else {
            find(&data, b"eexec")? + 5
        };
        let (clear, encrypted) = data.split_at(split);
        let private = decrypt(&encrypted_bytes(encrypted), EEXEC_KEY, 4);
        let skip = find(&private, b"/lenIV")
            .and_then(|at| {
                String::from_utf8_lossy(&private[at + 6..(at + 16).min(private.len())])
                    .split_ascii_whitespace()
                    .next()?
                    .parse::<usize>()
                    .ok()
            })
            .unwrap_or(4);
        let subrs_end = find(&private, b"/CharStrings").unwrap_or(private.len());
        let subrs = read_blocks(&private[..subrs_end], b"/Subrs", skip);
        let mut indexed: Vec<Vec<u8>> = Vec::new();
        for (index, block) in subrs {
            let index: usize = index.parse().unwrap_or(indexed.len());
            if index < 65536 {
                if indexed.len() <= index {
                    indexed.resize(index + 1, Vec::new());
                }
                indexed[index] = block;
            }
        }
        let charstrings = read_blocks(&private, b"/CharStrings", skip)
            .into_iter()
            .map(|(name, block)| (name.trim_start_matches('/').to_owned(), block))
            .collect();
        Some(Type1 {
            charstrings,
            subrs: indexed,
            encoding: read_encoding(clear),
            scale: read_scale(clear),
        })
    }

    pub fn has_glyph(&self, name: &str) -> bool {
        self.charstrings.contains_key(name)
    }

    /// The glyph named `name` in font units, and its width.
    pub fn glyph(&self, name: &str) -> Option<(Vec<Command>, f64)> {
        let charstring = self.charstrings.get(name)?;
        let mut machine = Machine::new(self);
        machine.run(charstring, 0);
        Some((machine.commands, machine.width))
    }
}

/// A PFB file's segments joined, when the program is in that wrapper.
fn strip_pfb(data: &[u8]) -> Vec<u8> {
    if data.first() != Some(&0x80) {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    while data.get(at) == Some(&0x80) && matches!(data.get(at + 1), Some(1 | 2)) {
        let length = data.get(at + 2..at + 6).map_or(0, |four| {
            u32::from_le_bytes(four.try_into().expect("four")) as usize
        });
        out.extend_from_slice(data.get(at + 6..at + 6 + length).unwrap_or(&[]));
        at += 6 + length;
    }
    out
}

/// A charstring interpreter's state.
struct Machine<'a> {
    font: &'a Type1,
    stack: Vec<f64>,
    others: Vec<f64>,
    x: f64,
    y: f64,
    left: f64,
    width: f64,
    commands: Vec<Command>,
    open: bool,
    flex: Option<Vec<(f64, f64)>>,
    done: bool,
}

impl<'a> Machine<'a> {
    fn new(font: &'a Type1) -> Self {
        Self {
            font,
            stack: Vec::new(),
            others: Vec::new(),
            x: 0.0,
            y: 0.0,
            left: 0.0,
            width: 0.0,
            commands: Vec::new(),
            open: false,
            flex: None,
            done: false,
        }
    }

    fn move_by(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        if let Some(points) = &mut self.flex {
            points.push((self.x, self.y));
            return;
        }
        if self.open {
            self.commands.push(Command::Close);
        }
        self.commands.push(Command::Move(self.x, self.y));
        self.open = true;
    }

    fn line_by(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        self.commands.push(Command::Line(self.x, self.y));
    }

    fn curve_by(&mut self, d: [f64; 6]) {
        let a = (self.x + d[0], self.y + d[1]);
        let b = (a.0 + d[2], a.1 + d[3]);
        self.x = b.0 + d[4];
        self.y = b.1 + d[5];
        self.commands
            .push(Command::Cubic(a.0, a.1, b.0, b.1, self.x, self.y));
    }

    fn take(&mut self, count: usize) -> Vec<f64> {
        let start = self.stack.len().saturating_sub(count);
        let mut taken = self.stack.split_off(start);
        while taken.len() < count {
            taken.insert(0, 0.0);
        }
        taken
    }

    /// Ends flex: the seven points collected make two curves.
    fn end_flex(&mut self) {
        let Some(points) = self.flex.take() else {
            return;
        };
        if points.len() >= 7 {
            let p = &points;
            self.commands.push(Command::Cubic(
                p[1].0, p[1].1, p[2].0, p[2].1, p[3].0, p[3].1,
            ));
            self.commands.push(Command::Cubic(
                p[4].0, p[4].1, p[5].0, p[5].1, p[6].0, p[6].1,
            ));
            self.x = p[6].0;
            self.y = p[6].1;
        }
    }

    fn other_subr(&mut self) {
        let index = self.stack.pop().unwrap_or(0.0) as i64;
        let count = self.stack.pop().unwrap_or(0.0).max(0.0) as usize;
        let arguments = self.take(count);
        match index {
            0 => {
                self.end_flex();
                self.others = vec![self.y, self.x];
            }
            1 => self.flex = Some(Vec::new()),
            2 => {}
            3 => self.others = vec![3.0],
            _ => self.others = arguments.into_iter().rev().collect(),
        }
    }

    /// `seac`: the base glyph, then the accent moved by its offset.
    fn compose(&mut self, arguments: &[f64]) {
        let [side, dx, dy, base, accent] = [
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
            arguments[4],
        ];
        let name = |code: f64| {
            Base::Standard
                .char_of(code as u8)
                .map(super::encoding::name_of_char)
        };
        let width = self.width;
        for (code, offset) in [(base, (0.0, 0.0)), (accent, (dx - side + self.left, dy))] {
            let Some((commands, _)) = name(code).and_then(|name| self.font.glyph(&name)) else {
                continue;
            };
            self.commands
                .extend(commands.into_iter().map(|command| shift(command, offset)));
        }
        self.width = width;
        self.done = true;
    }

    fn escape(&mut self, operator: u8) {
        match operator {
            6 => {
                let arguments = self.take(5);
                self.compose(&arguments);
            }
            7 => {
                let arguments = self.take(4);
                self.left = arguments[0];
                self.x = arguments[0];
                self.y = arguments[1];
                self.width = arguments[2];
            }
            12 => {
                let pair = self.take(2);
                self.stack.push(if pair[1] == 0.0 {
                    0.0
                } else {
                    pair[0] / pair[1]
                });
            }
            16 => self.other_subr(),
            17 => {
                let value = self.others.pop().unwrap_or(0.0);
                self.stack.push(value);
            }
            33 => {
                let point = self.take(2);
                self.x = point[0];
                self.y = point[1];
            }
            _ => self.stack.clear(),
        }
    }

    fn operator(&mut self, operator: u8, depth: usize) {
        match operator {
            13 => {
                let arguments = self.take(2);
                self.left = arguments[0];
                self.x = arguments[0];
                self.width = arguments[1];
            }
            21 => {
                let d = self.take(2);
                self.move_by(d[0], d[1]);
            }
            22 => {
                let d = self.take(1);
                self.move_by(d[0], 0.0);
            }
            4 => {
                let d = self.take(1);
                self.move_by(0.0, d[0]);
            }
            5 => {
                let d = self.take(2);
                self.line_by(d[0], d[1]);
            }
            6 => {
                let d = self.take(1);
                self.line_by(d[0], 0.0);
            }
            7 => {
                let d = self.take(1);
                self.line_by(0.0, d[0]);
            }
            8 => {
                let d = self.take(6);
                self.curve_by([d[0], d[1], d[2], d[3], d[4], d[5]]);
            }
            30 => {
                let d = self.take(4);
                self.curve_by([0.0, d[0], d[1], d[2], d[3], 0.0]);
            }
            31 => {
                let d = self.take(4);
                self.curve_by([d[0], 0.0, d[1], d[2], 0.0, d[3]]);
            }
            9 => {
                if self.open {
                    self.commands.push(Command::Close);
                    self.open = false;
                }
            }
            10 => {
                let index = self.stack.pop().unwrap_or(-1.0);
                if let Some(subr) = self
                    .font
                    .subrs
                    .get(index.max(0.0) as usize)
                    .filter(|_| index >= 0.0)
                {
                    let subr = subr.clone();
                    self.run(&subr, depth + 1);
                }
            }
            14 => {
                if self.open {
                    self.commands.push(Command::Close);
                    self.open = false;
                }
                self.done = true;
            }
            _ => self.stack.clear(),
        }
    }

    fn run(&mut self, charstring: &[u8], depth: usize) {
        if depth > CALLS_MAX {
            return;
        }
        let mut at = 0;
        while at < charstring.len() && !self.done {
            let byte = charstring[at];
            at += 1;
            match byte {
                11 => return,
                12 => {
                    let next = charstring.get(at).copied().unwrap_or(0);
                    at += 1;
                    self.escape(next);
                }
                0..=31 => self.operator(byte, depth),
                32..=246 => self.stack.push(f64::from(byte) - 139.0),
                247..=250 => {
                    let next = f64::from(charstring.get(at).copied().unwrap_or(0));
                    at += 1;
                    self.stack
                        .push((f64::from(byte) - 247.0) * 256.0 + next + 108.0);
                }
                251..=254 => {
                    let next = f64::from(charstring.get(at).copied().unwrap_or(0));
                    at += 1;
                    self.stack
                        .push(-(f64::from(byte) - 251.0) * 256.0 - next - 108.0);
                }
                255 => {
                    let value = charstring
                        .get(at..at + 4)
                        .map_or(0, |four| i32::from_be_bytes(four.try_into().expect("four")));
                    at += 4;
                    self.stack.push(f64::from(value));
                }
            }
        }
    }
}

fn shift(command: Command, (dx, dy): (f64, f64)) -> Command {
    match command {
        Command::Move(x, y) => Command::Move(x + dx, y + dy),
        Command::Line(x, y) => Command::Line(x + dx, y + dy),
        Command::Quad(a, b, x, y) => Command::Quad(a + dx, b + dy, x + dx, y + dy),
        Command::Cubic(a, b, c, d, x, y) => {
            Command::Cubic(a + dx, b + dy, c + dx, d + dy, x + dx, y + dy)
        }
        Command::Close => Command::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encrypts like a Type 1 writer, with four bytes of lead-in.
    fn encrypt(plain: &[u8], key: u16) -> Vec<u8> {
        let mut r = key;
        let mut out = Vec::new();
        for &byte in [0u8; 4].iter().chain(plain) {
            let cipher = byte ^ (r >> 8) as u8;
            r = (u16::from(cipher).wrapping_add(r))
                .wrapping_mul(C1)
                .wrapping_add(C2);
            out.push(cipher);
        }
        out
    }

    #[test]
    fn a_glyph_draws_from_its_charstring() {
        // hsbw 50 600; rmoveto 0 0; rlineto 100 0; rlineto 0 100; closepath; endchar.
        let numbers = |values: &[i32]| -> Vec<u8> {
            values.iter().map(|value| (value + 139) as u8).collect()
        };
        let mut glyph = numbers(&[50]);
        glyph.extend([248, 236]); // 600
        glyph.push(13);
        glyph.extend(numbers(&[0, 0]));
        glyph.push(21);
        glyph.extend(numbers(&[100, 0]));
        glyph.push(5);
        glyph.extend(numbers(&[0, 100]));
        glyph.extend([5, 9, 14]);
        let charstring = encrypt(&glyph, CHARSTRING_KEY);
        let mut private =
            b"/lenIV 4 def /Subrs 0 array /CharStrings 1 dict dup begin /box ".to_vec();
        private.extend(format!("{} RD ", charstring.len()).as_bytes());
        private.extend(&charstring);
        private.extend(b" ND end");
        let mut font = b"%!PS-AdobeFont-1.0\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n/Encoding StandardEncoding def\ncurrentfile eexec\n".to_vec();
        let clear = font.len();
        font.extend(encrypt(&private, EEXEC_KEY));
        let read = Type1::read(&font, clear).expect("a font");
        assert_eq!(read.scale, 0.001);
        assert_eq!(read.encoding.get(&b'A').map(String::as_str), Some("A"));
        let (commands, width) = read.glyph("box").expect("the glyph");
        assert_eq!(width, 600.0);
        assert_eq!(
            commands,
            [
                Command::Move(50.0, 0.0),
                Command::Line(150.0, 0.0),
                Command::Line(150.0, 100.0),
                Command::Close
            ]
        );
    }
}
