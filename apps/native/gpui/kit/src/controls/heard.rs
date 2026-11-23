//! A field's accessibility text, shaped like AppKit's text fields and views: the text as one
//! run plus its selection (`AXSelectedTextRange`), so VoiceOver follows the caret and selection.

use gpui::A11ySubtreeBuilder;
use gpui::accesskit::{Node, Role, TextPosition, TextSelection};
use std::ops::Range;

/// A field's accessibility node: `words` and the `selected` range (UTF-8 offsets), with the
/// caret at its start when `reversed`.
pub fn heard_words(
    words: String,
    selected: Range<usize>,
    reversed: bool,
) -> impl FnOnce(&mut A11ySubtreeBuilder) + 'static {
    move |builder| {
        // Positions count characters, each as long as its UTF-8 byte length.
        let at = |offset: usize| words[..offset].chars().count();
        let (anchor, focus) = if reversed {
            (at(selected.end), at(selected.start))
        } else {
            (at(selected.start), at(selected.end))
        };
        let mut run = Node::new(Role::TextRun);
        run.set_character_lengths(
            words
                .chars()
                .map(|character| character.len_utf8() as u8)
                .collect::<Vec<_>>(),
        );
        run.set_value(words);
        let id = builder.synthetic_node_id("words");
        builder.push_child(id, run);
        builder.parent_node().set_text_selection(TextSelection {
            anchor: TextPosition {
                node: id,
                character_index: anchor,
            },
            focus: TextPosition {
                node: id,
                character_index: focus,
            },
        });
    }
}
