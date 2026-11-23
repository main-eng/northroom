//! The single wrapping rule for multi-line plain text: settings descriptions, dialog text, toast
//! reasons and the local workspace path. Lines hold as many words as fit, as in a browser, with
//! widths from the core typesetter (`typeset`). Shells draw the returned lines and never wrap
//! text themselves.

/// Where a line may break.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Breaks {
    /// Between words (`white-space: normal`): white space collapses to one space, and a word
    /// wider than a line gets a line to itself.
    Words,
    /// Between any two characters (`word-break: break-all`), for paths.
    Anywhere,
    /// As a text box wraps input (`white-space: pre-wrap` with `overflow-wrap: break-word`):
    /// line breaks are kept, spaces stay as typed and hang at the line's end, and an overlong
    /// word breaks between characters. Empty text is one line tall.
    Typed,
}

/// Wraps `text` into lines no wider than `width`, broken per `breaks` and measured by
/// `measure`.
pub fn lines(
    text: &str,
    width: f64,
    breaks: Breaks,
    measure: &mut dyn FnMut(&str) -> f64,
) -> Vec<String> {
    match breaks {
        Breaks::Words => words(text, width, measure),
        Breaks::Anywhere => {
            let mut lines = Vec::new();
            characters(text, width, measure, &mut lines);
            lines
        }
        Breaks::Typed => text
            .split('\n')
            .flat_map(|paragraph| typed(paragraph, width, measure))
            .collect(),
    }
}

/// Wraps `text` between words, collapsing white space. A word wider than a line gets its own
/// line.
fn words(text: &str, width: f64, measure: &mut dyn FnMut(&str) -> f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let longer = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && measure(&longer) > width {
            lines.push(std::mem::replace(&mut line, word.to_owned()));
        } else {
            line = longer;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Wraps `text` between characters, pushing full lines onto `lines` and returning the last,
/// unfinished one.
fn characters(
    text: &str,
    width: f64,
    measure: &mut dyn FnMut(&str) -> f64,
    lines: &mut Vec<String>,
) {
    let mut line = String::new();
    for character in text.chars() {
        let mut longer = line.clone();
        longer.push(character);
        if !line.is_empty() && measure(&longer) > width {
            lines.push(std::mem::replace(&mut line, character.to_string()));
        } else {
            line = longer;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
}

/// Wraps one typed paragraph (no line breaks), keeping each word with its trailing spaces while
/// the visible part fits.
fn typed(paragraph: &str, width: f64, measure: &mut dyn FnMut(&str) -> f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for piece in paragraph.split_inclusive(' ') {
        let longer = format!("{line}{piece}");
        if measure(longer.trim_end_matches(' ')) <= width {
            line = longer;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        let word = piece.trim_end_matches(' ');
        if measure(word) <= width {
            line = piece.to_owned();
        } else {
            // Break an overlong word; its last part and trailing spaces start the next line.
            characters(word, width, measure, &mut lines);
            line = lines.pop().unwrap_or_default() + &piece[word.len()..];
        }
    }
    lines.push(line);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character, space included, is 10 wide.
    fn tens(text: &str) -> f64 {
        text.chars().count() as f64 * 10.0
    }

    fn wrapped(text: &str, width: f64, breaks: Breaks) -> Vec<String> {
        lines(text, width, breaks, &mut tens)
    }

    #[test]
    fn words_fill_each_line_and_a_long_word_keeps_a_line() {
        assert_eq!(
            wrapped("one two three four", 90.0, Breaks::Words),
            ["one two", "three", "four"]
        );
        assert_eq!(
            wrapped("a extraordinary b", 50.0, Breaks::Words),
            ["a", "extraordinary", "b"]
        );
        assert_eq!(
            wrapped("fits exactly", 120.0, Breaks::Words),
            ["fits exactly"]
        );
    }

    #[test]
    fn words_collapse_white_space_and_nothing_takes_no_line() {
        assert_eq!(
            wrapped("  one   two\nthree ", 300.0, Breaks::Words),
            ["one two three"]
        );
        assert!(wrapped("", 100.0, Breaks::Words).is_empty());
        assert!(wrapped("   ", 100.0, Breaks::Words).is_empty());
    }

    #[test]
    fn anywhere_breaks_inside_words() {
        assert_eq!(
            wrapped("/Users/me/My Work", 60.0, Breaks::Anywhere),
            ["/Users", "/me/My", " Work"]
        );
        assert_eq!(wrapped("ab", 5.0, Breaks::Anywhere), ["a", "b"]);
        assert!(wrapped("", 60.0, Breaks::Anywhere).is_empty());
    }

    #[test]
    fn typed_text_keeps_its_breaks_and_spaces() {
        assert_eq!(wrapped("", 100.0, Breaks::Typed), [""]);
        assert_eq!(wrapped("hello\n", 100.0, Breaks::Typed), ["hello", ""]);
        assert_eq!(
            wrapped("a  b\n\nc", 100.0, Breaks::Typed),
            ["a  b", "", "c"]
        );
        // Spaces hang past the edge; the next word starts the next line.
        assert_eq!(
            wrapped("one two three", 70.0, Breaks::Typed),
            ["one two ", "three"]
        );
    }

    #[test]
    fn typed_text_breaks_a_word_wider_than_the_line() {
        assert_eq!(
            wrapped("go abcdefgh x", 40.0, Breaks::Typed),
            ["go ", "abcd", "efgh ", "x"]
        );
        assert_eq!(wrapped("abcdef", 30.0, Breaks::Typed), ["abc", "def"]);
    }
}
