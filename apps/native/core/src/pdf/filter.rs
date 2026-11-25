//! Undoes a stream's filters: Flate (with PNG and TIFF predictors), LZW, ASCII hex, ASCII 85
//! and run-length. Picture codecs (DCT for JPEG, JPX for JPEG 2000) are left for the system's
//! decoders (ADR-0004), so decoding stops before them and `codec` says which it was.
//!
//! A damaged Flate stream keeps what inflated before the damage, as most readers do.

use super::object::{Dict, Document, Object, Stream};
use super::PdfError;
use std::io::Read;

type Result<T> = std::result::Result<T, PdfError>;

/// The most bytes a stream inflates to, so a small file can't ask for all memory.
const OUTPUT_MAX: usize = 512 * 1024 * 1024;

/// A stream's filters by name, and each one's parameters, resolved.
pub fn filters(stream: &Stream, document: Option<&Document>) -> Vec<(String, Dict)> {
    let resolve = |object: &Object| match document {
        Some(document) => document.resolve(object),
        None => object.clone(),
    };
    let names: Vec<String> = match stream.dict.get("Filter").map(resolve) {
        Some(Object::Name(name)) => vec![name],
        Some(Object::Array(items)) => items
            .iter()
            .filter_map(|item| resolve(item).as_name().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    let parameters: Vec<Dict> = match stream.dict.get("DecodeParms").map(resolve) {
        Some(Object::Dict(dict)) => vec![dict],
        Some(Object::Array(items)) => items
            .iter()
            .map(|item| resolve(item).as_dict().cloned().unwrap_or_default())
            .collect(),
        _ => Vec::new(),
    };
    names
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name, parameters.get(index).cloned().unwrap_or_default()))
        .collect()
}

/// The picture codec a stream's last filter names, if it is one: `"jpeg"` or `"jpx"`.
pub fn codec(stream: &Stream, document: Option<&Document>) -> Option<&'static str> {
    match filters(stream, document).last()?.0.as_str() {
        "DCTDecode" | "DCT" => Some("jpeg"),
        "JPXDecode" => Some("jpx"),
        _ => None,
    }
}

/// The stream's bytes with every filter but a picture codec undone.
pub fn decode(stream: &Stream, document: Option<&Document>) -> Result<Vec<u8>> {
    let mut data = stream.raw.as_ref().clone();
    for (name, parameters) in filters(stream, document) {
        data = match name.as_str() {
            "FlateDecode" | "Fl" => predict(inflate(&data), &parameters)?,
            "LZWDecode" | "LZW" => predict(lzw(&data, &parameters), &parameters)?,
            "ASCIIHexDecode" | "AHx" => ascii_hex(&data),
            "ASCII85Decode" | "A85" => ascii_85(&data),
            "RunLengthDecode" | "RL" => run_length(&data),
            "DCTDecode" | "DCT" | "JPXDecode" | "Crypt" => return Ok(data),
            other => return Err(PdfError(format!("The filter {other} is not read."))),
        };
    }
    Ok(data)
}

/// Inflates zlib data, or raw deflate data when the zlib header is missing, keeping what came
/// out before any damage.
fn inflate(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut zlib = flate2::read::ZlibDecoder::new(data).take(OUTPUT_MAX as u64);
    if zlib.read_to_end(&mut out).is_ok() && !out.is_empty() {
        return out;
    }
    let kept = out.len();
    let mut raw = Vec::new();
    let mut deflate = flate2::read::DeflateDecoder::new(data).take(OUTPUT_MAX as u64);
    let _ = deflate.read_to_end(&mut raw);
    if raw.len() > kept {
        raw
    } else {
        out
    }
}

fn number(parameters: &Dict, key: &str, default: i64) -> i64 {
    parameters
        .get(key)
        .and_then(Object::as_i64)
        .unwrap_or(default)
}

/// Undoes a predictor: TIFF 2 (each sample less the one before), or PNG's per-row filters.
fn predict(data: Vec<u8>, parameters: &Dict) -> Result<Vec<u8>> {
    let predictor = number(parameters, "Predictor", 1);
    if predictor < 2 {
        return Ok(data);
    }
    let colours = number(parameters, "Colors", 1).clamp(1, 32) as usize;
    let bits = number(parameters, "BitsPerComponent", 8).clamp(1, 16) as usize;
    let columns = number(parameters, "Columns", 1).clamp(1, 1 << 20) as usize;
    let pixel = (colours * bits).div_ceil(8);
    let row = (colours * bits * columns).div_ceil(8);
    if predictor == 2 {
        return Ok(tiff_predict(data, pixel, row, bits));
    }
    Ok(png_predict(&data, pixel, row))
}

fn tiff_predict(mut data: Vec<u8>, pixel: usize, row: usize, bits: usize) -> Vec<u8> {
    if bits != 8 {
        return data;
    }
    for line in data.chunks_mut(row) {
        for index in pixel..line.len() {
            line[index] = line[index].wrapping_add(line[index - pixel]);
        }
    }
    data
}

fn paeth(left: u8, up: u8, corner: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(up) - i16::from(corner);
    let (a, b, c) = (
        (estimate - i16::from(left)).abs(),
        (estimate - i16::from(up)).abs(),
        (estimate - i16::from(corner)).abs(),
    );
    if a <= b && a <= c {
        left
    } else if b <= c {
        up
    } else {
        corner
    }
}

fn png_predict(data: &[u8], pixel: usize, row: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(data.len());
    let mut previous = vec![0u8; row];
    for line in data.chunks(row + 1) {
        let (kind, bytes) = (line[0], &line[1..]);
        let current = png_row(kind, bytes, &previous, pixel);
        out.extend_from_slice(&current[..bytes.len().min(row)]);
        previous = current;
    }
    out
}

/// One row with its PNG filter `kind` undone against the row above, `previous`.
fn png_row(kind: u8, bytes: &[u8], previous: &[u8], pixel: usize) -> Vec<u8> {
    let mut current = vec![0u8; previous.len()];
    for index in 0..bytes.len().min(previous.len()) {
        let before = index.checked_sub(pixel);
        let left = before.map_or(0, |at| current[at]);
        let corner = before.map_or(0, |at| previous[at]);
        let up = previous[index];
        let base = match kind {
            1 => left,
            2 => up,
            3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
            4 => paeth(left, up, corner),
            _ => 0,
        };
        current[index] = bytes[index].wrapping_add(base);
    }
    current
}

/// Undoes LZW with 9- to 12-bit codes; `EarlyChange` (1 by default) widens codes one early.
fn lzw(data: &[u8], parameters: &Dict) -> Vec<u8> {
    let mut decoder = Lzw::new(number(parameters, "EarlyChange", 1).clamp(0, 1) as usize);
    let (mut buffer, mut held) = (0u32, 0usize);
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        held += 8;
        while held >= decoder.width {
            let code = ((buffer >> (held - decoder.width)) & ((1 << decoder.width) - 1)) as usize;
            held -= decoder.width;
            if !decoder.take(code) {
                return decoder.out;
            }
        }
    }
    decoder.out
}

/// LZW's table and output as codes arrive.
struct Lzw {
    early: usize,
    table: Vec<Vec<u8>>,
    previous: Option<Vec<u8>>,
    width: usize,
    out: Vec<u8>,
}

impl Lzw {
    fn new(early: usize) -> Self {
        let mut table: Vec<Vec<u8>> = (0..=255u8).map(|byte| vec![byte]).collect();
        table.extend([Vec::new(), Vec::new()]);
        Self {
            early,
            table,
            previous: None,
            width: 9,
            out: Vec::new(),
        }
    }

    /// Takes one code; `false` once the data ends or is damaged.
    fn take(&mut self, code: usize) -> bool {
        match code {
            256 => {
                self.table.truncate(258);
                self.width = 9;
                self.previous = None;
                return true;
            }
            257 => return false,
            _ => {}
        }
        let Some(entry) = self.entry(code) else {
            return false;
        };
        if let Some(mut before) = self.previous.take() {
            before.push(entry[0]);
            self.table.push(before);
        }
        self.out.extend_from_slice(&entry);
        self.previous = Some(entry);
        if self.table.len() + self.early >= 1 << self.width && self.width < 12 {
            self.width += 1;
        }
        self.out.len() <= OUTPUT_MAX
    }

    /// The bytes `code` stands for: a known entry, or the previous one and its first byte.
    fn entry(&self, code: usize) -> Option<Vec<u8>> {
        if let Some(known) = self.table.get(code) {
            return Some(known.clone());
        }
        let before = self.previous.as_ref()?;
        let mut made = before.clone();
        made.push(before[0]);
        Some(made)
    }
}

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = data
        .iter()
        .take_while(|byte| **byte != b'>')
        .filter_map(|byte| char::from(*byte).to_digit(16).map(|digit| digit as u8))
        .collect();
    digits
        .chunks(2)
        .map(|pair| pair[0] * 16 + pair.get(1).copied().unwrap_or(0))
        .collect()
}

fn ascii_85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group: Vec<u32> = Vec::with_capacity(5);
    let flush = |group: &mut Vec<u32>, out: &mut Vec<u8>| {
        let count = group.len();
        if count < 2 {
            group.clear();
            return;
        }
        group.resize(5, 84);
        let value = group.iter().fold(0u32, |value, digit| {
            value.wrapping_mul(85).wrapping_add(*digit)
        });
        out.extend_from_slice(&value.to_be_bytes()[..count - 1]);
        group.clear();
    };
    let body = data.strip_prefix(b"<~").unwrap_or(data);
    for &byte in body {
        match byte {
            b'~' => break,
            b'z' if group.is_empty() => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group.push(u32::from(byte - b'!'));
                if group.len() == 5 {
                    flush(&mut group, &mut out);
                }
            }
            _ => {}
        }
    }
    flush(&mut group, &mut out);
    out
}

fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(&length) = data.get(at) {
        at += 1;
        match length {
            128 => break,
            0..=127 => {
                let end = (at + usize::from(length) + 1).min(data.len());
                out.extend_from_slice(&data[at..end]);
                at = end;
            }
            _ => {
                if let Some(&value) = data.get(at) {
                    out.extend(std::iter::repeat_n(value, 257 - usize::from(length)));
                }
                at += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_encodings_and_runs_decode() {
        assert_eq!(ascii_hex(b"48 65 6C6c6f>"), b"Hello");
        assert_eq!(ascii_85(b"<~87cURD]i,\"Ebo80~>"), b"Hello World!");
        assert_eq!(ascii_85(b"z~>"), [0, 0, 0, 0]);
        assert_eq!(
            run_length(&[2, b'a', b'b', b'c', 254, b'x', 128]),
            b"abcxxx"
        );
    }

    #[test]
    fn png_rows_undo_their_filters() {
        // Two rows of three bytes: "sub" then "up".
        let data = [1, 10, 5, 5, 2, 1, 1, 1];
        assert_eq!(png_predict(&data, 1, 3), [10, 15, 20, 11, 16, 21]);
    }

    #[test]
    fn lzw_decodes_the_specifications_example() {
        // The PDF reference's example: "-----A---B" as 9-bit codes.
        let codes: [u16; 8] = [256, 45, 258, 258, 65, 259, 66, 257];
        let mut bits: u64 = 0;
        let mut count = 0;
        let mut bytes = Vec::new();
        for code in codes {
            bits = (bits << 9) | u64::from(code);
            count += 9;
            while count >= 8 {
                bytes.push((bits >> (count - 8)) as u8);
                count -= 8;
            }
        }
        if count > 0 {
            bytes.push((bits << (8 - count)) as u8);
        }
        assert_eq!(lzw(&bytes, &Dict::new()), b"-----A---B");
    }
}
