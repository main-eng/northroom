//! CMaps: how a composite font's string bytes split into codes and which CID each code is
//! (`Encoding`), and which text each code stands for (`ToUnicode`). Embedded CMaps are read,
//! as are the identity ones every reader knows.
//!
//! Left out: the predefined CJK CMaps (`UniGB-UCS2-H` and the like), whose tables come from
//! Adobe's CMap resources; a font naming one reads its codes as two-byte CIDs.

use super::object::{Object, Parser, Token};
use std::collections::HashMap;

/// The most entries a range adds, so a damaged range can't ask for all memory.
const RANGE_MAX: u32 = 1 << 16;

#[derive(Default)]
pub struct CMap {
    /// Code space: each range's length in bytes and its lowest and highest code.
    spaces: Vec<(usize, u32, u32)>,
    /// CID ranges: lowest and highest code, and the first's CID.
    cids: Vec<(u32, u32, u32)>,
    /// The text each code stands for.
    unicode: HashMap<u32, String>,
}

fn code_of(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0, |code, byte| code << 8 | u32::from(*byte))
}

/// UTF-16BE text, as `ToUnicode` gives it.
fn text_of(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|pair| u16::from(pair[0]) << 8 | u16::from(*pair.get(1).unwrap_or(&0)))
        .collect();
    String::from_utf16_lossy(&units)
}

/// `text` with its last character moved on by `step`, as a `bfrange` counts.
fn counted(text: &str, step: u32) -> String {
    let mut characters: Vec<char> = text.chars().collect();
    if let Some(last) = characters.last_mut() {
        *last = char::from_u32(*last as u32 + step).unwrap_or(*last);
    }
    characters.into_iter().collect()
}

/// The text of entry `step` of a `bfrange`: its first text counted on, or its own in a list.
fn range_text(target: &Object, step: usize) -> Option<String> {
    match target {
        Object::String(first) => Some(counted(&text_of(first), step as u32)),
        Object::Array(texts) => texts.get(step).and_then(Object::as_bytes).map(text_of),
        _ => None,
    }
}

impl CMap {
    /// Two-byte codes, each its own CID (`Identity-H` and `Identity-V`).
    pub fn identity() -> Self {
        Self {
            spaces: vec![(2, 0, 0xffff)],
            ..Self::default()
        }
    }

    /// Reads an embedded CMap's code space, CIDs and text.
    pub fn parse(data: &[u8]) -> Self {
        let mut map = Self::default();
        let mut parser = Parser::new(data, 0);
        let mut operands: Vec<Object> = Vec::new();
        while let Some(token) = parser.next_token() {
            let Token::Word(word) = token else {
                parser.put_back(token);
                match parser.object(0) {
                    Some(object) => operands.push(object),
                    None => break,
                }
                continue;
            };
            map.operator(&word, &operands);
            operands.clear();
        }
        map
    }

    fn operator(&mut self, word: &[u8], operands: &[Object]) {
        match word {
            b"usecmap"
                if operands
                    .first()
                    .and_then(Object::as_name)
                    .is_some_and(|name| name.starts_with("Identity")) =>
            {
                self.spaces.push((2, 0, 0xffff));
            }
            b"endcodespacerange" => self.code_spaces(operands),
            b"endcidrange" => self.cid_ranges(operands),
            b"endcidchar" => self.cid_chars(operands),
            b"endbfchar" => self.text_chars(operands),
            b"endbfrange" => self.text_ranges(operands),
            _ => {}
        }
    }

    fn code_spaces(&mut self, operands: &[Object]) {
        for pair in operands.chunks(2) {
            if let [Object::String(low), Object::String(high)] = pair {
                self.spaces
                    .push((low.len().clamp(1, 4), code_of(low), code_of(high)));
            }
        }
    }

    fn cid_ranges(&mut self, operands: &[Object]) {
        for triple in operands.chunks(3) {
            if let [Object::String(low), Object::String(high), cid] = triple {
                self.cids.push((
                    code_of(low),
                    code_of(high),
                    cid.as_i64().unwrap_or(0).max(0) as u32,
                ));
            }
        }
    }

    fn cid_chars(&mut self, operands: &[Object]) {
        for pair in operands.chunks(2) {
            if let [Object::String(code), cid] = pair {
                let code = code_of(code);
                self.cids
                    .push((code, code, cid.as_i64().unwrap_or(0).max(0) as u32));
            }
        }
    }

    fn text_chars(&mut self, operands: &[Object]) {
        for pair in operands.chunks(2) {
            if let [Object::String(code), Object::String(text)] = pair {
                self.unicode.insert(code_of(code), text_of(text));
            }
        }
    }

    fn text_ranges(&mut self, operands: &[Object]) {
        for triple in operands.chunks(3) {
            let [Object::String(low), Object::String(high), target] = triple else {
                continue;
            };
            let (low, high) = (code_of(low), code_of(high).min(code_of(low) + RANGE_MAX));
            for (step, code) in (low..=high).enumerate() {
                if let Some(text) = range_text(target, step) {
                    self.unicode.insert(code, text);
                }
            }
        }
    }

    /// Splits a string's bytes into codes, each with its length in bytes: the shortest that
    /// falls in the code space, else the code space's shortest.
    pub fn split(&self, bytes: &[u8]) -> Vec<(u32, usize)> {
        let shortest = self.spaces.iter().map(|space| space.0).min().unwrap_or(1);
        let mut codes = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let length = (1..=4)
                .filter(|length| at + length <= bytes.len())
                .find(|&length| {
                    let code = code_of(&bytes[at..at + length]);
                    self.spaces
                        .iter()
                        .any(|&(size, low, high)| size == length && (low..=high).contains(&code))
                })
                .unwrap_or(shortest.min(bytes.len() - at));
            codes.push((code_of(&bytes[at..at + length]), length));
            at += length;
        }
        codes
    }

    /// The CID `code` maps to: by the CMap's ranges, else the code itself when it has none.
    pub fn cid(&self, code: u32) -> u32 {
        if self.cids.is_empty() {
            return code;
        }
        self.cids
            .iter()
            .rev()
            .find(|(low, high, _)| (*low..=*high).contains(&code))
            .map_or(0, |(low, _, first)| first + (code - low))
    }

    /// The text `code` stands for.
    pub fn text(&self, code: u32) -> Option<&str> {
        self.unicode.get(&code).map(String::as_str)
    }

    /// Every code with text, and its text.
    pub fn texts(&self) -> impl Iterator<Item = (u32, &str)> {
        self.unicode
            .iter()
            .map(|(code, text)| (*code, text.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cmap_splits_codes_and_maps_cids_and_text() {
        let map = CMap::parse(
            b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
              2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange \
              1 begincidrange <20> <7E> 1 endcidrange \
              1 begincidchar <8140> 633 endcidchar \
              2 beginbfchar <41> <0041> <8140> <3000> endbfchar \
              1 beginbfrange <61> <63> <0061> endbfrange endcmap",
        );
        assert_eq!(
            map.split(b"A\x81\x40b"),
            [(0x41, 1), (0x8140, 2), (0x62, 1)]
        );
        assert_eq!(
            (map.cid(0x41), map.cid(0x8140), map.cid(0x10)),
            (34, 633, 0)
        );
        assert_eq!(
            (map.text(0x41), map.text(0x8140), map.text(0x63)),
            (Some("A"), Some("\u{3000}"), Some("c"))
        );
        assert_eq!(
            CMap::identity().split(b"\x01\x02\x03"),
            [(0x0102, 2), (0x03, 1)]
        );
    }
}
