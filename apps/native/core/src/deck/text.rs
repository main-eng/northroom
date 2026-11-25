//! A page's text for selecting and copying: which glyph a point picks, and how a run of glyphs
//! reads as text, with spaces where words part and line breaks where lines do.

use crate::geometry::Rect;

/// A glyph as a view shows it: its text and its box in view points.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub text: String,
    pub frame: Rect,
}

fn middle(frame: &Rect) -> (f64, f64) {
    (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0)
}

/// Whether glyph `b` sits on the same line as `a`: its middle within `a`'s height.
fn same_line(a: &Rect, b: &Rect) -> bool {
    let (_, y) = middle(b);
    y >= a.y && y <= a.y + a.height
}

/// The glyph a point picks: the one under it, else the nearest on its line, else the nearest.
pub fn glyph_at(glyphs: &[Glyph], (x, y): (f64, f64)) -> Option<usize> {
    if let Some(under) = glyphs.iter().position(|glyph| {
        let frame = &glyph.frame;
        x >= frame.x && x <= frame.x + frame.width && y >= frame.y && y <= frame.y + frame.height
    }) {
        return Some(under);
    }
    let distance = |glyph: &Glyph| {
        let (gx, gy) = middle(&glyph.frame);
        let on_line = y >= glyph.frame.y && y <= glyph.frame.y + glyph.frame.height;
        // A glyph on the pointer's line wins over any off it.
        let line_weight = if on_line { 0.0 } else { 1e6 };
        line_weight + (gx - x).powi(2) + (gy - y).powi(2)
    };
    (0..glyphs.len()).min_by(|&a, &b| distance(&glyphs[a]).total_cmp(&distance(&glyphs[b])))
}

/// Glyphs as they read, character by character, each with the glyph it came from: a line
/// break where a glyph starts a new line and a space where it stands apart from the last by a
/// fifth of its height (unless one of them is already a space), those belonging to no glyph.
fn reading(glyphs: &[Glyph]) -> Vec<(char, Option<usize>)> {
    let mut out = Vec::new();
    let mut previous: Option<&Glyph> = None;
    for (index, glyph) in glyphs.iter().enumerate() {
        if let Some(before) = previous {
            let spaced = before.text.ends_with(' ') || glyph.text.starts_with(' ');
            if !same_line(&before.frame, &glyph.frame) {
                out.push(('\n', None));
            } else if !spaced
                && glyph.frame.x - (before.frame.x + before.frame.width) > glyph.frame.height * 0.2
            {
                out.push((' ', None));
            }
        }
        out.extend(glyph.text.chars().map(|character| (character, Some(index))));
        previous = Some(glyph);
    }
    out
}

/// Glyphs `from` to `to` (either order, both included) as text (`reading`).
pub fn text_between(glyphs: &[Glyph], from: usize, to: usize) -> String {
    let (first, last) = (
        from.min(to),
        from.max(to).min(glyphs.len().saturating_sub(1)),
    );
    let slice = glyphs.get(first..=last).unwrap_or(&[]);
    reading(slice)
        .into_iter()
        .map(|(character, _)| character)
        .collect()
}

fn folded(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

/// Where `query` occurs in the glyphs' text, case aside and a line break counting as a space:
/// each match's first and last glyph.
pub fn find(glyphs: &[Glyph], query: &str) -> Vec<(usize, usize)> {
    let wanted: Vec<char> = query.chars().map(folded).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let read = reading(glyphs);
    let plain: Vec<char> = read
        .iter()
        .map(|(character, _)| {
            if *character == '\n' {
                ' '
            } else {
                folded(*character)
            }
        })
        .collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at + wanted.len() <= plain.len() {
        if plain[at..at + wanted.len()] != wanted[..] {
            at += 1;
            continue;
        }
        let owners = read[at..at + wanted.len()]
            .iter()
            .filter_map(|(_, owner)| *owner);
        let (first, last) = owners.fold((usize::MAX, 0), |(low, high), owner| {
            (low.min(owner), high.max(owner))
        });
        if first != usize::MAX {
            found.push((first, last));
        }
        at += wanted.len();
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(text: &str, x: f64, y: f64) -> Glyph {
        Glyph {
            text: text.into(),
            frame: Rect::new(x, y, 6.0, 10.0),
        }
    }

    #[test]
    fn glyphs_read_as_words_and_lines() {
        let glyphs = vec![
            glyph("H", 0.0, 0.0),
            glyph("i", 6.0, 0.0),
            glyph("t", 20.0, 0.0),
            glyph("o", 26.0, 0.0),
            glyph("N", 0.0, 14.0),
            glyph("o", 6.0, 14.0),
        ];
        assert_eq!(text_between(&glyphs, 0, 5), "Hi to\nNo");
        assert_eq!(text_between(&glyphs, 3, 1), "i to");
        assert_eq!(find(&glyphs, "TO no"), [(2, 5)]);
        assert_eq!(find(&glyphs, "o"), [(3, 3), (5, 5)]);
        assert!(find(&glyphs, "").is_empty());
    }

    #[test]
    fn a_point_picks_the_glyph_under_it_or_the_nearest_on_its_line() {
        let glyphs = vec![
            glyph("a", 0.0, 0.0),
            glyph("b", 6.0, 0.0),
            glyph("c", 0.0, 20.0),
        ];
        assert_eq!(glyph_at(&glyphs, (8.0, 5.0)), Some(1));
        assert_eq!(glyph_at(&glyphs, (40.0, 5.0)), Some(1));
        assert_eq!(glyph_at(&glyphs, (2.0, 40.0)), Some(2));
        assert_eq!(glyph_at(&[], (0.0, 0.0)), None);
    }
}
