//! A text field's editing model, separate from drawing: the text, the selection (UTF-8
//! offsets, with the caret end), an input method's marked text, and undo and redo grouped as
//! `NSTextView` groups them (typing, including a composition, is one step until the caret
//! moves). Shared by the one-line field and the message box.
//!
//! GPUI and the text are UTF-8; platform input methods count UTF-16 units. All conversions
//! between the two live here.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// The state one undo step restores.
#[derive(Clone, Debug, PartialEq)]
struct Step {
    content: String,
    selected: Range<usize>,
}

/// The last edit's kind, so a run of typing (with any composition) is one undo step.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    /// A standalone edit, such as a deletion, cut or paste.
    Alone,
    Typing,
}

#[derive(Clone, Debug, Default)]
pub struct Editing {
    pub content: String,
    /// UTF-8 offsets into `content`.
    pub selected: Range<usize>,
    /// The caret is at the selection's start rather than its end.
    pub reversed: bool,
    pub marked: Option<Range<usize>>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    group: Option<Group>,
}

impl Editing {
    pub fn new(content: &str) -> Self {
        Self {
            content: content.to_owned(),
            selected: content.len()..content.len(),
            ..Self::default()
        }
    }

    pub fn caret(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    /// Replaces the text from outside (the core changed it), moving the caret to the end and
    /// clearing undo.
    pub fn replace_all(&mut self, content: &str) {
        *self = Self::new(content);
    }

    pub fn move_to(&mut self, offset: usize) {
        self.selected = offset..offset;
        self.reversed = false;
        self.group = None;
    }

    /// Moves the caret end of the selection to `offset`, keeping the anchor.
    pub fn select_to(&mut self, offset: usize) {
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        self.group = None;
    }

    pub fn select_all(&mut self) {
        self.move_to(0);
        self.select_to(self.content.len());
    }

    /// Saves an undo step, unless this edit continues the previous typing.
    fn record(&mut self, group: Group) {
        if group == Group::Alone || self.group != Some(group) {
            self.undo.push(Step {
                content: self.content.clone(),
                selected: self.selected.clone(),
            });
            self.redo.clear();
        }
        self.group = Some(group);
    }

    /// Replaces `range` (UTF-8), else the marked text, else the selection, with `text`.
    pub fn edit(&mut self, range: Option<Range<usize>>, text: &str, group: Group) {
        let range = range
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        self.record(group);
        self.content.replace_range(range.clone(), text);
        let caret = range.start + text.len();
        self.selected = caret..caret;
        self.reversed = false;
        self.marked = None;
    }

    /// Sets an input method's composition: replaces `range` (UTF-8), else the marked text, else
    /// the selection, with marked `text`, selecting `inside` it (UTF-16 units).
    pub fn mark(&mut self, range: Option<Range<usize>>, text: &str, inside: Option<Range<usize>>) {
        let range = range
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        // A composition groups with the surrounding typing into one undo step.
        self.record(Group::Typing);
        self.content.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.selected = match inside {
            Some(inside) => {
                range.start + from_utf16(text, inside.start)
                    ..range.start + from_utf16(text, inside.end)
            }
            None => range.start + text.len()..range.start + text.len(),
        };
        self.reversed = false;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undoes the last edit. False when there is none.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.now());
        self.restore(step);
        true
    }

    /// Redoes the last undone edit. False when there is none.
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.now());
        self.restore(step);
        true
    }

    fn now(&self) -> Step {
        Step {
            content: self.content.clone(),
            selected: self.selected.clone(),
        }
    }

    fn restore(&mut self, step: Step) {
        self.content = step.content;
        self.selected = step.selected;
        self.reversed = false;
        self.marked = None;
        self.group = None;
    }

    pub fn selected_words(&self) -> &str {
        &self.content[self.selected.clone()]
    }
}

/// The UTF-8 offset of UTF-16 offset `units` in `text` (clamped to its end).
pub fn from_utf16(text: &str, units: usize) -> usize {
    let mut counted = 0;
    for (index, character) in text.char_indices() {
        if counted >= units {
            return index;
        }
        counted += character.len_utf16();
    }
    text.len()
}

/// The UTF-16 offset of UTF-8 offset `index` in `text`.
pub fn to_utf16(text: &str, index: usize) -> usize {
    text[..index.min(text.len())].encode_utf16().count()
}

pub fn range_from_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    from_utf16(text, range.start)..from_utf16(text, range.end)
}

pub fn range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    to_utf16(text, range.start)..to_utf16(text, range.end)
}

/// The grapheme boundary before `index` (0 at the start).
pub fn previous_grapheme(text: &str, index: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(at, _)| (at < index).then_some(at))
        .unwrap_or(0)
}

/// The grapheme boundary after `index` (the end at the end).
pub fn next_grapheme(text: &str, index: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(at, _)| (at > index).then_some(at))
        .unwrap_or(text.len())
}

/// The word around `index`, as a double click selects it.
pub fn word_at(text: &str, index: usize) -> Range<usize> {
    text.split_word_bound_indices()
        .map(|(at, word)| at..at + word.len())
        .find(|word| word.contains(&index) || word.end == index && index == text.len())
        .unwrap_or(index..index)
}

/// The start of the word before `index` (⌥←).
pub fn previous_word(text: &str, index: usize) -> usize {
    text.split_word_bound_indices()
        .filter(|(at, word)| *at < index && word.chars().any(char::is_alphanumeric))
        .map(|(at, _)| at)
        .next_back()
        .unwrap_or(0)
}

/// The end of the word after `index` (⌥→).
pub fn next_word(text: &str, index: usize) -> usize {
    text.split_word_bound_indices()
        .map(|(at, word)| (at + word.len(), word))
        .find(|(end, word)| *end > index && word.chars().any(char::is_alphanumeric))
        .map_or(text.len(), |(end, _)| end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_is_one_undo_step_and_a_move_starts_another() {
        let mut field = Editing::new("");
        for typed in ["a", "b", "c"] {
            field.edit(None, typed, Group::Typing);
        }
        field.move_to(1);
        field.edit(None, "X", Group::Typing);
        assert_eq!(field.content, "aXbc");
        assert!(field.undo());
        assert_eq!(field.content, "abc");
        assert!(field.undo());
        assert_eq!(field.content, "");
        assert!(!field.undo(), "nothing is left to undo");
        assert!(field.redo() && field.redo());
        assert_eq!(field.content, "aXbc");
    }

    #[test]
    fn a_composition_marks_then_commits_in_utf16_units() {
        let mut field = Editing::new("");
        field.edit(None, "🙂", Group::Typing);
        // Kotoeri's steps for "nihon": marked kana, then the chosen kanji committed.
        field.mark(None, "に", Some(1..1));
        field.mark(None, "にほん", Some(3..3));
        assert_eq!(
            field
                .marked
                .as_ref()
                .map(|marked| range_to_utf16(&field.content, marked)),
            Some(2..5)
        );
        field.mark(None, "日本", Some(0..2));
        assert_eq!(field.selected, 4..10, "the converted clause is selected");
        field.edit(None, "日本", Group::Typing);
        assert_eq!(field.content, "🙂日本");
        assert_eq!(field.marked, None);
        assert!(field.undo());
        assert_eq!(
            field.content, "",
            "the composition is part of the typing before it"
        );
    }

    #[test]
    fn a_selection_made_backwards_keeps_the_caret_at_its_start() {
        let mut field = Editing::new("one two");
        field.select_to(4);
        assert_eq!(
            (field.selected.clone(), field.reversed, field.caret()),
            (4..7, true, 4)
        );
        field.select_to(next_word("one two", 0));
        assert_eq!(field.selected, 3..7);
        field.edit(None, "", Group::Alone);
        assert_eq!(field.content, "one");
    }

    #[test]
    fn words_and_graphemes_are_stepped_whole() {
        let text = "a👍🏽 well-made";
        assert_eq!(next_grapheme(text, 1), 1 + "👍🏽".len());
        assert_eq!(previous_grapheme(text, 1 + "👍🏽".len()), 1);
        assert_eq!(word_at(text, text.len() - 1), text.len() - 4..text.len());
        assert_eq!(previous_word(text, text.len()), text.len() - 4);
        assert_eq!(next_word(text, 0), 1);
    }
}
