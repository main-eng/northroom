//! The faces and lines through Core Text.

use objc2_core_foundation::{
    CFAttributedString, CFDictionary, CFNumber, CFRange, CFRetained, CFString, CFType, CFURL,
    CGAffineTransform,
};
use objc2_core_text::{
    CTFont, CTFontDescriptor, CTFontManagerRegisterFontsForURL, CTFontManagerScope, CTLine,
    CTParagraphStyle, CTParagraphStyleSetting, CTParagraphStyleSpecifier, CTTypesetter,
    kCTFontAttributeName, kCTFontFeatureSelectorIdentifierKey, kCTFontFeatureSettingsAttribute,
    kCTFontFeatureTypeIdentifierKey, kCTFontNameAttribute, kCTFontVariationAttribute,
    kCTKernAttributeName, kCTParagraphStyleAttributeName, kMonospacedNumbersSelector,
    kNumberSpacingType,
};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::process::ExitCode;
use unicode_segmentation::UnicodeSegmentation;

/// The `wght` axis of a variable face.
const WEIGHT_AXIS: i64 = 0x7767_6874;

pub fn run() -> ExitCode {
    let Some(fonts) = std::env::args().nth(1) else {
        eprintln!("usage: text-widths <fonts folder> < cases > answers");
        return ExitCode::from(2);
    };
    if let Err(missing) = register_faces(&fonts) {
        eprintln!("{}: no such face", missing.display());
        return ExitCode::from(2);
    }
    let mut faces = Faces::default();
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for line in std::io::stdin().lock().lines() {
        let line = line.expect("cases are text");
        let fields: Vec<&str> = line.split('\t').collect();
        let key = fields[1..6].join("\t");
        let face = faces.face(&fields[1..6]);
        match fields[0] {
            "W" => answer_width(&mut out, &line, &fields, &face, &key),
            "L" => answer_lines(&mut out, &line, &fields, &face, &key),
            kind => panic!("no case kind {kind}"),
        }
    }
    ExitCode::SUCCESS
}

/// Registers the app's faces in `fonts` for this process. Errors with the first missing file.
fn register_faces(fonts: &str) -> Result<(), std::path::PathBuf> {
    for name in ["Inter-latin", "GeistMono-latin"] {
        let path = std::path::Path::new(fonts).join(format!("{name}.woff2"));
        let Some(url) = CFURL::from_file_path(&path) else {
            return Err(path);
        };
        // SAFETY: `url` is a live file URL and the error out-pointer may be null.
        unsafe {
            CTFontManagerRegisterFontsForURL(
                &url,
                CTFontManagerScope::Process,
                std::ptr::null_mut(),
            )
        };
    }
    Ok(())
}

/// Answers a `W` case (five face fields, tracking, text): writes the text's fallbacks, then the
/// case with its width.
fn answer_width(out: &mut impl Write, line: &str, fields: &[&str], face: &CTFont, key: &str) {
    let tracking: f64 = fields[6].parse().expect("a tracking");
    let text = unescaped(&fields[7..].join("\t"));
    for stretch in fallbacks(&text, face, key) {
        writeln!(out, "{stretch}").expect("answers are written");
    }
    let width = width_of(&attributed(&text, face, tracking, None));
    writeln!(out, "{line}\t{width:.9}").expect("answers are written");
}

/// Answers an `L` case (five face fields, width, wraps, tab spaces, text): writes the text's
/// fallbacks, then the case with its line count and widest line.
fn answer_lines(out: &mut impl Write, line: &str, fields: &[&str], face: &CTFont, key: &str) {
    let width: f64 = fields[6].parse().expect("a width");
    let wraps = fields[7] == "1";
    let tab_spaces: usize = fields[8].parse().expect("a count of spaces");
    let text = unescaped(&fields[9..].join("\t"));
    let paragraph = (tab_spaces > 0).then(|| {
        tabs(width_of(&attributed(
            &" ".repeat(tab_spaces),
            face,
            0.0,
            None,
        )))
    });
    for stretch in fallbacks(&text, face, key) {
        writeln!(out, "{stretch}").expect("answers are written");
    }
    let string = attributed(&text, face, 0.0, paragraph.as_deref());
    let room = if wraps { width } else { f64::MAX };
    let (count, widest) = broken_lines(&string, text.encode_utf16().count() as isize, room);
    writeln!(out, "{line}\t{count} {widest:.9}").expect("answers are written");
}

/// Line count and widest line width when Core Text breaks `string` (`length` UTF-16 units) at
/// `room` pt.
fn broken_lines(string: &CFAttributedString, length: isize, room: f64) -> (usize, f64) {
    // SAFETY: a live attributed string.
    let typesetter = unsafe { CTTypesetter::with_attributed_string(string) };
    let (mut start, mut count, mut widest) = (0, 0, 0.0f64);
    while start < length {
        // SAFETY: `start` is inside the string.
        let taken = unsafe { typesetter.suggest_line_break(start, room) };
        if taken <= 0 {
            break;
        }
        // SAFETY: the range is inside the string.
        let made = unsafe {
            typesetter.line(CFRange {
                location: start,
                length: taken,
            })
        };
        widest = widest.max(typographic_width(&made));
        count += 1;
        start += taken;
    }
    (count, widest)
}

/// Faces created so far, keyed by their five fields.
#[derive(Default)]
struct Faces(HashMap<String, CFRetained<CTFont>>);

impl Faces {
    /// Geist Mono, or Inter at the given weight. Tabular uses fixed-width digits; slanted shears
    /// by a fifth of the height.
    fn face(&mut self, fields: &[&str]) -> CFRetained<CTFont> {
        let key = fields.join("\t");
        if let Some(made) = self.0.get(&key) {
            return made.clone();
        }
        let size: f64 = fields[0].parse().expect("a size");
        // SAFETY: the framework's constants, and dictionaries of the types Core Text reads.
        let mut face = unsafe {
            if fields[2] == "1" {
                CTFont::with_name(
                    &CFString::from_static_str("Geist Mono"),
                    size,
                    std::ptr::null(),
                )
            } else {
                let weight: f64 = fields[1].parse().expect("a weight");
                let axes = CFDictionary::<CFNumber, CFNumber>::from_slices(
                    &[&*CFNumber::new_i64(WEIGHT_AXIS)],
                    &[&*CFNumber::new_f64(weight)],
                );
                let name = CFString::from_static_str("Inter");
                let attributes = CFDictionary::<CFString, CFType>::from_slices(
                    &[kCTFontNameAttribute, kCTFontVariationAttribute],
                    &[name.as_ref(), axes.as_ref()],
                );
                let descriptor = CTFontDescriptor::with_attributes(attributes.as_opaque());
                CTFont::with_font_descriptor(&descriptor, size, std::ptr::null())
            }
        };
        if fields[3] == "1" {
            // SAFETY: as above.
            face = unsafe {
                let setting = CFDictionary::<CFString, CFNumber>::from_slices(
                    &[
                        kCTFontFeatureTypeIdentifierKey,
                        kCTFontFeatureSelectorIdentifierKey,
                    ],
                    &[
                        &*CFNumber::new_i64(i64::from(kNumberSpacingType)),
                        &*CFNumber::new_i64(i64::from(kMonospacedNumbersSelector)),
                    ],
                );
                let settings = objc2_core_foundation::CFArray::from_objects(&[&*setting]);
                let attributes = CFDictionary::<CFString, CFType>::from_slices(
                    &[kCTFontFeatureSettingsAttribute],
                    &[settings.as_ref()],
                );
                let descriptor = face
                    .font_descriptor()
                    .copy_with_attributes(attributes.as_opaque());
                CTFont::with_font_descriptor(&descriptor, size, std::ptr::null())
            };
        }
        if fields[4] == "1" {
            let slant = CGAffineTransform {
                a: 1.0,
                b: 0.0,
                c: 0.2,
                d: 1.0,
                tx: 0.0,
                ty: 0.0,
            };
            // SAFETY: a live matrix.
            face = unsafe { face.copy_with_attributes(size, &slant, None) };
        }
        self.0.insert(key, face.clone());
        face
    }
}

/// A paragraph style with no explicit tab stops and a default tab every `interval` pt.
fn tabs(interval: f64) -> CFRetained<CTParagraphStyle> {
    let stops = objc2_core_foundation::CFArray::<CFType>::from_objects(&[]);
    let stops_ref: &objc2_core_foundation::CFArray = stops.as_opaque();
    let settings = [
        CTParagraphStyleSetting {
            spec: CTParagraphStyleSpecifier::TabStops,
            valueSize: size_of::<&objc2_core_foundation::CFArray>(),
            value: std::ptr::NonNull::from(&stops_ref).cast(),
        },
        CTParagraphStyleSetting {
            spec: CTParagraphStyleSpecifier::DefaultTabInterval,
            valueSize: size_of::<f64>(),
            value: std::ptr::NonNull::from(&interval).cast(),
        },
    ];
    // SAFETY: each setting points at a value of its size, alive for the call.
    unsafe { CTParagraphStyle::new(settings.as_ptr(), settings.len()) }
}

/// `text` in `face` and `paragraph`, tracked by `tracking` pt when non-zero.
fn attributed(
    text: &str,
    face: &CTFont,
    tracking: f64,
    paragraph: Option<&CTParagraphStyle>,
) -> CFRetained<CFAttributedString> {
    // SAFETY: the framework's constants.
    let mut keys: Vec<&CFString> = vec![unsafe { kCTFontAttributeName }];
    let mut values: Vec<&CFType> = vec![face.as_ref()];
    let kern = CFNumber::new_f64(tracking);
    if tracking != 0.0 {
        // SAFETY: as above.
        keys.push(unsafe { kCTKernAttributeName });
        values.push(kern.as_ref());
    }
    if let Some(paragraph) = paragraph {
        // SAFETY: as above.
        keys.push(unsafe { kCTParagraphStyleAttributeName });
        values.push(paragraph.as_ref());
    }
    let attributes = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    // SAFETY: a string and attributes Core Text reads.
    unsafe {
        CFAttributedString::new(
            None,
            Some(&CFString::from_str(text)),
            Some(attributes.as_opaque()),
        )
    }
    .expect("an attributed string")
}

fn typographic_width(line: &CTLine) -> f64 {
    // SAFETY: null out-pointers skip the ascent, descent and leading.
    unsafe {
        line.typographic_bounds(
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    }
}

fn width_of(string: &CFAttributedString) -> f64 {
    // SAFETY: a live attributed string.
    let line = unsafe { CTLine::with_attributed_string(string) };
    typographic_width(&line)
}

fn unescaped(text: &str) -> String {
    text.replace("\\n", "\n").replace("\\t", "\t")
}

/// Whether `face` itself, without fallback, has a glyph for every unit of `character`.
fn has(face: &CTFont, character: &str) -> bool {
    let mut units: Vec<u16> = character.encode_utf16().collect();
    let mut glyphs = vec![0u16; units.len()];
    // SAFETY: `glyphs` has room for one glyph per unit.
    unsafe {
        face.glyphs_for_characters(
            std::ptr::NonNull::new(units.as_mut_ptr()).expect("a character"),
            std::ptr::NonNull::new(glyphs.as_mut_ptr()).expect("a character"),
            units.len() as isize,
        )
    }
}

/// Runs of `text` that `face` has no glyphs for, each with its standalone width.
fn fallbacks(text: &str, face: &CTFont, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stretch = String::new();
    let ended = format!("{text}a");
    for character in ended.graphemes(true) {
        if character == "\n" || character == "\t" || has(face, character) {
            if !stretch.is_empty() {
                let width = width_of(&attributed(&stretch, face, 0.0, None));
                out.push(format!("F\t{key}\t{stretch}\t{width:.9}"));
                stretch.clear();
            }
        } else {
            stretch.push_str(character);
        }
    }
    out
}
