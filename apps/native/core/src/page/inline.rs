//! Inline marks inside a block's text: `**bold**`, `*italic*`, `~~strike~~`, `` `code` ``,
//! `[label](url)` and `![alt](src)`. Parsing only finds where they are; the text itself never
//! changes, so a block reads back exactly as it was written.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Bold,
    Italic,
    Strike,
    Code,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// Groups the segments of one link.
    pub key: u32,
    pub href: String,
}

/// `![alt](src)`: the segment's text is the alt text, and the page's host draws `src`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub src: String,
    pub alt: String,
}

/// A stretch of a block's words that shows, with what marks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    /// Where the text starts in the raw block text, in UTF-16 code units.
    pub raw_start: usize,
    pub marks: Vec<Mark>,
    pub link: Option<Link>,
    pub image: Option<Image>,
}

#[derive(Clone)]
struct Scope {
    marks: Vec<Mark>,
    link: Option<Link>,
}

impl Scope {
    fn with(&self, mark: Mark) -> Scope {
        let mut scope = self.clone();
        if !scope.marks.contains(&mark) {
            scope.marks.push(mark);
        }
        scope
    }
}

const ESCAPABLE: &str = "\\`*_~[]()#+-.!>|";

fn is_word(character: Option<char>) -> bool {
    character.is_some_and(char::is_alphanumeric)
}

/// Matches JavaScript's `\s`.
fn is_space(character: Option<char>) -> bool {
    character.is_some_and(|c| c.is_whitespace() || c == '\u{feff}')
}

struct Parser {
    raw: Vec<char>,
    /// The UTF-16 offset of each character, and of the end.
    offsets: Vec<usize>,
    segments: Vec<Segment>,
    next_link_key: u32,
}

impl Parser {
    fn at(&self, index: usize) -> Option<char> {
        self.raw.get(index).copied()
    }

    /// The first index at or after `from` where `needle` starts.
    fn find(&self, needle: &[char], from: usize) -> Option<usize> {
        if needle.is_empty() || from > self.raw.len() {
            return None;
        }
        (from..=self.raw.len().saturating_sub(needle.len()))
            .find(|&index| self.raw[index..index + needle.len()] == *needle)
            .filter(|_| self.raw.len() >= needle.len())
    }

    fn slice(&self, from: usize, until: usize) -> String {
        self.raw[from..until].iter().collect()
    }

    fn emit_plain(&mut self, from: usize, until: usize, scope: &Scope) {
        if until > from {
            self.segments.push(Segment {
                text: self.slice(from, until),
                raw_start: self.offsets[from],
                marks: scope.marks.clone(),
                link: scope.link.clone(),
                image: None,
            });
        }
    }

    /// The closing marker's index for a pair whose content starts at `from`, when it closes.
    fn closing_marker(&self, marker: &[char], from: usize, end: usize) -> Option<usize> {
        let close = self.find(marker, from)?;
        if close + marker.len() > end || close == from {
            return None;
        }
        if is_space(self.at(from))
            || is_space(close.checked_sub(1).and_then(|before| self.at(before)))
        {
            return None;
        }
        Some(close)
    }

    fn parse_range(&mut self, start: usize, end: usize, scope: &Scope) {
        let mut plain_start = start;
        let mut index = start;
        while index < end {
            let Some((found, next)) = self.match_at(index, end, scope) else {
                index += 1;
                continue;
            };
            self.emit_plain(plain_start, index, scope);
            self.emit(index, found, scope);
            index = next;
            plain_start = index;
        }
        self.emit_plain(plain_start, end, scope);
    }

    /// The construct that starts at `index`, and where it ends.
    fn match_at(&self, index: usize, end: usize, scope: &Scope) -> Option<(Found, usize)> {
        self.match_escape(index, end)
            .or_else(|| self.match_code(index, end))
            .or_else(|| self.match_double(index, end))
            .or_else(|| self.match_italic(index, end))
            .or_else(|| self.match_image(index, end, scope))
            .or_else(|| self.match_link(index, end, scope))
    }

    fn match_escape(&self, index: usize, end: usize) -> Option<(Found, usize)> {
        let escaped = self.at(index + 1)?;
        (self.at(index) == Some('\\') && index + 1 < end && ESCAPABLE.contains(escaped))
            .then_some((Found::Escape, index + 2))
    }

    fn match_code(&self, index: usize, end: usize) -> Option<(Found, usize)> {
        if self.at(index) != Some('`') {
            return None;
        }
        let close = self.find(&['`'], index + 1)?;
        (close > index + 1 && close < end).then_some((Found::Code { close }, close + 1))
    }

    fn match_double(&self, index: usize, end: usize) -> Option<(Found, usize)> {
        let pair = [self.at(index)?, self.at(index + 1)?];
        let mark = match pair {
            ['*', '*'] | ['_', '_'] => Mark::Bold,
            ['~', '~'] => Mark::Strike,
            _ => return None,
        };
        let mut close = self.closing_marker(&pair, index + 2, end)?;
        // In a run like `***`, the pair closes last so an inner `*` closes first.
        while close + 2 < end && self.at(close + 2) == Some(pair[0]) {
            close += 1;
        }
        Some((Found::Double { mark, close }, close + 2))
    }

    /// `_` only opens and closes at a word edge, so snake_case stays plain.
    fn match_italic(&self, index: usize, end: usize) -> Option<(Found, usize)> {
        let marker = self.at(index)?;
        if marker != '*' && marker != '_' {
            return None;
        }
        if self.at(index + 1) == Some(marker) {
            return None;
        }
        if marker == '_' && is_word(index.checked_sub(1).and_then(|before| self.at(before))) {
            return None;
        }
        let close = self.closing_marker(&[marker], index + 1, end)?;
        if marker == '_' && is_word(self.at(close + 1)) {
            return None;
        }
        Some((Found::Italic { close }, close + 1))
    }

    /// One segment for the whole image; an image never nests inside a link.
    fn match_image(&self, index: usize, end: usize, scope: &Scope) -> Option<(Found, usize)> {
        if self.at(index) != Some('!') || self.at(index + 1) != Some('[') || scope.link.is_some() {
            return None;
        }
        let alt_end = self.find(&[']', '('], index + 2)?;
        let src_end = self.find(&[')'], alt_end + 2)?;
        (src_end > alt_end + 2 && src_end < end)
            .then_some((Found::Image { alt_end, src_end }, src_end + 1))
    }

    fn match_link(&self, index: usize, end: usize, scope: &Scope) -> Option<(Found, usize)> {
        if self.at(index) != Some('[') || scope.link.is_some() {
            return None;
        }
        let label_end = self.find(&[']', '('], index + 1)?;
        let href_end = self.find(&[')'], label_end + 2)?;
        (label_end > index + 1 && href_end > label_end + 2 && href_end < end).then_some((
            Found::Link {
                label_end,
                href_end,
            },
            href_end + 1,
        ))
    }

    fn emit(&mut self, index: usize, found: Found, scope: &Scope) {
        match found {
            Found::Escape => self.emit_plain(index + 1, index + 2, scope),
            Found::Code { close } => self.emit_plain(index + 1, close, &scope.with(Mark::Code)),
            Found::Double { mark, close } => self.parse_range(index + 2, close, &scope.with(mark)),
            Found::Italic { close } => {
                self.parse_range(index + 1, close, &scope.with(Mark::Italic))
            }
            Found::Image { alt_end, src_end } => {
                let alt = self.slice(index + 2, alt_end);
                self.segments.push(Segment {
                    text: alt.clone(),
                    raw_start: self.offsets[index + 2],
                    marks: scope.marks.clone(),
                    link: None,
                    image: Some(Image {
                        src: self.slice(alt_end + 2, src_end).trim().to_owned(),
                        alt,
                    }),
                });
            }
            Found::Link {
                label_end,
                href_end,
            } => {
                let link = Link {
                    key: self.next_link_key,
                    href: self.slice(label_end + 2, href_end).trim().to_owned(),
                };
                self.next_link_key += 1;
                let scope = Scope {
                    marks: scope.marks.clone(),
                    link: Some(link),
                };
                self.parse_range(index + 1, label_end, &scope);
            }
        }
    }
}

/// One inline construct found at a position.
enum Found {
    Escape,
    Code { close: usize },
    Double { mark: Mark, close: usize },
    Italic { close: usize },
    Image { alt_end: usize, src_end: usize },
    Link { label_end: usize, href_end: usize },
}

/// The stretches of `raw` that show, each with its marks.
pub fn parse(raw: &str) -> Vec<Segment> {
    let characters: Vec<char> = raw.chars().collect();
    let mut offsets = Vec::with_capacity(characters.len() + 1);
    let mut offset = 0;
    for character in &characters {
        offsets.push(offset);
        offset += character.len_utf16();
    }
    offsets.push(offset);
    let end = characters.len();
    let mut parser = Parser {
        raw: characters,
        offsets,
        segments: Vec::new(),
        next_link_key: 0,
    };
    parser.parse_range(
        0,
        end,
        &Scope {
            marks: Vec::new(),
            link: None,
        },
    );
    parser.segments
}

/// Links open only on the web: a page's text may come from an assistant.
pub fn is_safe_link(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("mailto:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(raw: &str) -> Vec<(String, Vec<Mark>)> {
        parse(raw)
            .into_iter()
            .map(|segment| (segment.text, segment.marks))
            .collect()
    }

    #[test]
    fn finds_marks_and_leaves_snake_case_plain() {
        assert_eq!(
            shown("a **bold *both*** and `code` and snake_case_name ~~gone~~"),
            [
                ("a ".to_owned(), vec![]),
                ("bold ".to_owned(), vec![Mark::Bold]),
                ("both".to_owned(), vec![Mark::Bold, Mark::Italic]),
                (" and ".to_owned(), vec![]),
                ("code".to_owned(), vec![Mark::Code]),
                (" and snake_case_name ".to_owned(), vec![]),
                ("gone".to_owned(), vec![Mark::Strike]),
            ]
        );
    }

    #[test]
    fn reads_links_images_and_escapes() {
        let segments = parse("see [the *docs*](https://a.b/c) ![shot](asset:1) \\*not\\*");
        assert_eq!(segments[1].text, "the ");
        assert_eq!(
            segments[1].link.as_ref().map(|link| link.href.as_str()),
            Some("https://a.b/c")
        );
        assert_eq!(segments[2].marks, [Mark::Italic]);
        assert_eq!(segments[2].link.as_ref().map(|link| link.key), Some(0));
        let image = segments
            .iter()
            .find_map(|segment| segment.image.clone())
            .expect("image");
        assert_eq!(
            (image.alt.as_str(), image.src.as_str()),
            ("shot", "asset:1")
        );
        let tail: String = segments
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|segment| segment.text.as_str())
            .collect();
        assert_eq!(tail, "*not*");
        assert!(is_safe_link("HTTPS://x") && !is_safe_link("javascript:alert(1)"));
    }

    #[test]
    fn counts_positions_in_utf16_units() {
        let segments = parse("😀 **b**");
        assert_eq!(segments[1].raw_start, 5, "the emoji is two UTF-16 units");
    }
}
