//! The encodings PDF's simple fonts use, and glyph names: Standard, WinAnsi (Windows-1252),
//! MacRoman, and the names Adobe's glyph list gives the characters they hold. A font's
//! `Differences` name glyphs; a name turns into its character here, and back, to find the glyph
//! in a font that is looked up by character or by name.

/// The glyph names of Latin text and the characters they stand for: every character the three
/// encodings hold, and common ligatures.
const NAMES: &[(&str, char)] = &[
    ("space", ' '),
    ("exclam", '!'),
    ("quotedbl", '"'),
    ("numbersign", '#'),
    ("dollar", '$'),
    ("percent", '%'),
    ("ampersand", '&'),
    ("quotesingle", '\''),
    ("parenleft", '('),
    ("parenright", ')'),
    ("asterisk", '*'),
    ("plus", '+'),
    ("comma", ','),
    ("hyphen", '-'),
    ("period", '.'),
    ("slash", '/'),
    ("zero", '0'),
    ("one", '1'),
    ("two", '2'),
    ("three", '3'),
    ("four", '4'),
    ("five", '5'),
    ("six", '6'),
    ("seven", '7'),
    ("eight", '8'),
    ("nine", '9'),
    ("colon", ':'),
    ("semicolon", ';'),
    ("less", '<'),
    ("equal", '='),
    ("greater", '>'),
    ("question", '?'),
    ("at", '@'),
    ("bracketleft", '['),
    ("backslash", '\\'),
    ("bracketright", ']'),
    ("asciicircum", '^'),
    ("underscore", '_'),
    ("grave", '`'),
    ("braceleft", '{'),
    ("bar", '|'),
    ("braceright", '}'),
    ("asciitilde", '~'),
    ("quoteleft", '‘'),
    ("quoteright", '’'),
    ("quotedblleft", '“'),
    ("quotedblright", '”'),
    ("quotesinglbase", '‚'),
    ("quotedblbase", '„'),
    ("guilsinglleft", '‹'),
    ("guilsinglright", '›'),
    ("guillemotleft", '«'),
    ("guillemotright", '»'),
    ("endash", '–'),
    ("emdash", '—'),
    ("bullet", '•'),
    ("ellipsis", '…'),
    ("dagger", '†'),
    ("daggerdbl", '‡'),
    ("perthousand", '‰'),
    ("trademark", '™'),
    ("Euro", '€'),
    ("florin", 'ƒ'),
    ("circumflex", 'ˆ'),
    ("tilde", '˜'),
    ("macron", '¯'),
    ("breve", '˘'),
    ("dotaccent", '˙'),
    ("ring", '˚'),
    ("cedilla", '¸'),
    ("hungarumlaut", '˝'),
    ("ogonek", '˛'),
    ("caron", 'ˇ'),
    ("dieresis", '¨'),
    ("acute", '´'),
    ("exclamdown", '¡'),
    ("cent", '¢'),
    ("sterling", '£'),
    ("currency", '¤'),
    ("yen", '¥'),
    ("brokenbar", '¦'),
    ("section", '§'),
    ("copyright", '©'),
    ("ordfeminine", 'ª'),
    ("logicalnot", '¬'),
    ("registered", '®'),
    ("degree", '°'),
    ("plusminus", '±'),
    ("twosuperior", '²'),
    ("threesuperior", '³'),
    ("mu", 'µ'),
    ("paragraph", '¶'),
    ("periodcentered", '·'),
    ("onesuperior", '¹'),
    ("ordmasculine", 'º'),
    ("onequarter", '¼'),
    ("onehalf", '½'),
    ("threequarters", '¾'),
    ("questiondown", '¿'),
    ("multiply", '×'),
    ("divide", '÷'),
    ("fraction", '⁄'),
    ("fi", 'ﬁ'),
    ("fl", 'ﬂ'),
    ("ff", 'ﬀ'),
    ("ffi", 'ﬃ'),
    ("ffl", 'ﬄ'),
    ("dotlessi", 'ı'),
    ("Lslash", 'Ł'),
    ("lslash", 'ł'),
    ("OE", 'Œ'),
    ("oe", 'œ'),
    ("AE", 'Æ'),
    ("ae", 'æ'),
    ("Oslash", 'Ø'),
    ("oslash", 'ø'),
    ("germandbls", 'ß'),
    ("Scaron", 'Š'),
    ("scaron", 'š'),
    ("Zcaron", 'Ž'),
    ("zcaron", 'ž'),
    ("Ydieresis", 'Ÿ'),
    ("ydieresis", 'ÿ'),
    ("Eth", 'Ð'),
    ("eth", 'ð'),
    ("Thorn", 'Þ'),
    ("thorn", 'þ'),
    ("minus", '−'),
    ("nbspace", '\u{a0}'),
    ("sfthyphen", '\u{ad}'),
    ("Agrave", 'À'),
    ("Aacute", 'Á'),
    ("Acircumflex", 'Â'),
    ("Atilde", 'Ã'),
    ("Adieresis", 'Ä'),
    ("Aring", 'Å'),
    ("Ccedilla", 'Ç'),
    ("Egrave", 'È'),
    ("Eacute", 'É'),
    ("Ecircumflex", 'Ê'),
    ("Edieresis", 'Ë'),
    ("Igrave", 'Ì'),
    ("Iacute", 'Í'),
    ("Icircumflex", 'Î'),
    ("Idieresis", 'Ï'),
    ("Ntilde", 'Ñ'),
    ("Ograve", 'Ò'),
    ("Oacute", 'Ó'),
    ("Ocircumflex", 'Ô'),
    ("Otilde", 'Õ'),
    ("Odieresis", 'Ö'),
    ("Ugrave", 'Ù'),
    ("Uacute", 'Ú'),
    ("Ucircumflex", 'Û'),
    ("Udieresis", 'Ü'),
    ("Yacute", 'Ý'),
    ("agrave", 'à'),
    ("aacute", 'á'),
    ("acircumflex", 'â'),
    ("atilde", 'ã'),
    ("adieresis", 'ä'),
    ("aring", 'å'),
    ("ccedilla", 'ç'),
    ("egrave", 'è'),
    ("eacute", 'é'),
    ("ecircumflex", 'ê'),
    ("edieresis", 'ë'),
    ("igrave", 'ì'),
    ("iacute", 'í'),
    ("icircumflex", 'î'),
    ("idieresis", 'ï'),
    ("ntilde", 'ñ'),
    ("ograve", 'ò'),
    ("oacute", 'ó'),
    ("ocircumflex", 'ô'),
    ("otilde", 'õ'),
    ("odieresis", 'ö'),
    ("ugrave", 'ù'),
    ("uacute", 'ú'),
    ("ucircumflex", 'û'),
    ("udieresis", 'ü'),
    ("yacute", 'ý'),
    ("dotlessj", 'ȷ'),
    ("lozenge", '◊'),
    ("partialdiff", '∂'),
    ("Delta", '∆'),
    ("summation", '∑'),
    ("product", '∏'),
    ("pi", 'π'),
    ("integral", '∫'),
    ("Omega", 'Ω'),
    ("radical", '√'),
    ("approxequal", '≈'),
    ("notequal", '≠'),
    ("lessequal", '≤'),
    ("greaterequal", '≥'),
    ("infinity", '∞'),
    ("apple", '\u{f8ff}'),
];

/// The character a glyph name stands for: from the list, or a `uniXXXX`, `uXXXX[XX]` or single
/// letter or digit name. Suffixes after a period (`a.sc`) are dropped.
pub fn char_of_name(name: &str) -> Option<char> {
    let base = name.split('.').next().unwrap_or(name);
    if let Some(&(_, character)) = NAMES.iter().find(|(known, _)| *known == base) {
        return Some(character);
    }
    let hex = base
        .strip_prefix("uni")
        .filter(|digits| digits.len() == 4)
        .or_else(|| {
            base.strip_prefix('u')
                .filter(|digits| (4..=6).contains(&digits.len()))
        });
    if let Some(code) = hex.and_then(|digits| u32::from_str_radix(digits, 16).ok()) {
        return char::from_u32(code);
    }
    let mut characters = base.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) if character.is_ascii_alphanumeric() => Some(character),
        _ => None,
    }
}

/// A glyph name for a character: the list's, a letter's or digit's own, else `uniXXXX`.
pub fn name_of_char(character: char) -> String {
    if character.is_ascii_alphanumeric() {
        return character.to_string();
    }
    NAMES
        .iter()
        .find(|(_, known)| *known == character)
        .map_or_else(
            || format!("uni{:04X}", u32::from(character)),
            |(name, _)| (*name).to_owned(),
        )
}

/// The base encodings a simple font names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Standard,
    WinAnsi,
    MacRoman,
}

impl Base {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "StandardEncoding" => Some(Base::Standard),
            "WinAnsiEncoding" => Some(Base::WinAnsi),
            "MacRomanEncoding" => Some(Base::MacRoman),
            _ => None,
        }
    }

    /// The character `code` stands for in this encoding.
    pub fn char_of(self, code: u8) -> Option<char> {
        match self {
            Base::WinAnsi => win_ansi(code),
            Base::MacRoman => mac_roman(code),
            Base::Standard => standard(code),
        }
    }
}

const WIN_ANSI_HIGH: [char; 32] = [
    '€', '\0', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\0', 'Ž', '\0', '\0', '‘',
    '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\0', 'ž', 'Ÿ',
];

fn win_ansi(code: u8) -> Option<char> {
    match code {
        0x20..=0x7e | 0xa0..=0xff => Some(char::from(code)),
        0x80..=0x9f => Some(WIN_ANSI_HIGH[usize::from(code - 0x80)]).filter(|c| *c != '\0'),
        _ => None,
    }
}

const MAC_ROMAN_HIGH: [char; 128] = [
    'Ä', 'Å', 'Ç', 'É', 'Ñ', 'Ö', 'Ü', 'á', 'à', 'â', 'ä', 'ã', 'å', 'ç', 'é', 'è', 'ê', 'ë', 'í',
    'ì', 'î', 'ï', 'ñ', 'ó', 'ò', 'ô', 'ö', 'õ', 'ú', 'ù', 'û', 'ü', '†', '°', '¢', '£', '§', '•',
    '¶', 'ß', '®', '©', '™', '´', '¨', '≠', 'Æ', 'Ø', '∞', '±', '≤', '≥', '¥', 'µ', '∂', '∑', '∏',
    'π', '∫', 'ª', 'º', 'Ω', 'æ', 'ø', '¿', '¡', '¬', '√', 'ƒ', '≈', '∆', '«', '»', '…', '\u{a0}',
    'À', 'Ã', 'Õ', 'Œ', 'œ', '–', '—', '“', '”', '‘', '’', '÷', '◊', 'ÿ', 'Ÿ', '⁄', '€', '‹', '›',
    'ﬁ', 'ﬂ', '‡', '·', '‚', '„', '‰', 'Â', 'Ê', 'Á', 'Ë', 'È', 'Í', 'Î', 'Ï', 'Ì', 'Ó', 'Ô',
    '\u{f8ff}', 'Ò', 'Ú', 'Û', 'Ù', 'ı', 'ˆ', '˜', '¯', '˘', '˙', '˚', '¸', '˝', '˛', 'ˇ',
];

fn mac_roman(code: u8) -> Option<char> {
    match code {
        0x20..=0x7e => Some(char::from(code)),
        0x80..=0xff => Some(MAC_ROMAN_HIGH[usize::from(code - 0x80)]),
        _ => None,
    }
}

/// Adobe's StandardEncoding: ASCII but for its quotes, and its own upper half.
fn standard(code: u8) -> Option<char> {
    Some(match code {
        0x27 => '’',
        0x60 => '‘',
        0x20..=0x7e => char::from(code),
        0xa1 => '¡',
        0xa2 => '¢',
        0xa3 => '£',
        0xa4 => '⁄',
        0xa5 => '¥',
        0xa6 => 'ƒ',
        0xa7 => '§',
        0xa8 => '¤',
        0xa9 => '\'',
        0xaa => '“',
        0xab => '«',
        0xac => '‹',
        0xad => '›',
        0xae => 'ﬁ',
        0xaf => 'ﬂ',
        0xb1 => '–',
        0xb2 => '†',
        0xb3 => '‡',
        0xb4 => '·',
        0xb6 => '¶',
        0xb7 => '•',
        0xb8 => '‚',
        0xb9 => '„',
        0xba => '”',
        0xbb => '»',
        0xbc => '…',
        0xbd => '‰',
        0xbf => '¿',
        0xc1 => '`',
        0xc2 => '´',
        0xc3 => 'ˆ',
        0xc4 => '˜',
        0xc5 => '¯',
        0xc6 => '˘',
        0xc7 => '˙',
        0xc8 => '¨',
        0xca => '˚',
        0xcb => '¸',
        0xcd => '˝',
        0xce => '˛',
        0xcf => 'ˇ',
        0xd0 => '—',
        0xe1 => 'Æ',
        0xe3 => 'ª',
        0xe8 => 'Ł',
        0xe9 => 'Ø',
        0xea => 'Œ',
        0xeb => 'º',
        0xf1 => 'æ',
        0xf5 => 'ı',
        0xf8 => 'ł',
        0xf9 => 'ø',
        0xfa => 'œ',
        0xfb => 'ß',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_encodings_agree() {
        assert_eq!(char_of_name("quoteright"), Some('’'));
        assert_eq!(char_of_name("uni20AC"), Some('€'));
        assert_eq!(char_of_name("u1F600"), Some('😀'));
        assert_eq!(char_of_name("a.sc"), Some('a'));
        assert_eq!(char_of_name("glyph123"), None);
        assert_eq!(name_of_char('é'), "eacute");
        assert_eq!(name_of_char('B'), "B");
        assert_eq!(Base::WinAnsi.char_of(0x93), Some('“'));
        assert_eq!(Base::MacRoman.char_of(0xd2), Some('“'));
        assert_eq!(Base::Standard.char_of(0xaa), Some('“'));
        assert_eq!(Base::Standard.char_of(0x27), Some('’'));
    }
}
