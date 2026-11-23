//! Maps GPUI keystrokes to the core's `NrKeyPress`: the key, what it types and the modifiers
//! held.

use gpui::{Keystroke, Modifiers};
use northroom_core::ffi::{
    NR_HELD_COMMAND, NR_HELD_CONTROL, NR_HELD_OPTION, NR_HELD_SHIFT, NrKey, NrKeyPress,
};

/// The keys held, as the core numbers them (`NR_HELD_*`).
pub fn held(modifiers: &Modifiers) -> u32 {
    let mut held = 0;
    for (down, bit) in [
        (modifiers.platform, NR_HELD_COMMAND),
        (modifiers.control, NR_HELD_CONTROL),
        (modifiers.alt, NR_HELD_OPTION),
        (modifiers.shift, NR_HELD_SHIFT),
    ] {
        if down {
            held |= bit;
        }
    }
    held
}

/// GPUI's names for keys that type no character, with the core's key for each. Used in both
/// directions. Function keys `f1` to `f12` are handled by `NrKey::FUNCTIONS`.
const NAMED: [(&str, NrKey); 15] = [
    ("enter", NrKey::Return),
    ("tab", NrKey::Tab),
    ("space", NrKey::Space),
    ("escape", NrKey::Escape),
    ("backspace", NrKey::Backspace),
    ("delete", NrKey::Delete),
    ("up", NrKey::Up),
    ("down", NrKey::Down),
    ("left", NrKey::Left),
    ("right", NrKey::Right),
    ("home", NrKey::Home),
    ("end", NrKey::End),
    ("pageup", NrKey::PageUp),
    ("pagedown", NrKey::PageDown),
    ("insert", NrKey::Insert),
];

/// The core's key for a GPUI key name that types no character.
fn named(key: &str) -> NrKey {
    if let Some((_, named)) = NAMED.iter().find(|(name, _)| *name == key) {
        return *named;
    }
    key.strip_prefix('f')
        .and_then(|number| number.parse::<usize>().ok())
        .and_then(|number| NrKey::FUNCTIONS.get(number.wrapping_sub(1)).copied())
        .unwrap_or(NrKey::Other)
}

/// The core's key as GPUI names it; `None` for `Other`, which is named by what it types.
pub fn gpui_name(key: NrKey) -> Option<String> {
    if let Some((name, _)) = NAMED.iter().find(|(_, named)| *named == key) {
        return Some((*name).to_owned());
    }
    key.function().map(|number| format!("f{number}"))
}

/// The press for `keystroke`: what it types unless ⌘ or Control is held, and the letter or digit
/// it is on the US layout.
pub fn press(keystroke: &Keystroke) -> NrKeyPress {
    let key = named(&keystroke.key);
    let held = held(&keystroke.modifiers);
    let single = |text: &str| {
        let mut characters = text.chars();
        match (characters.next(), characters.next()) {
            (Some(one), None) => Some(one),
            _ => None,
        }
    };
    let typed = (key == NrKey::Other && held & (NR_HELD_COMMAND | NR_HELD_CONTROL) == 0)
        .then(|| keystroke.key_char.as_deref().and_then(single))
        .flatten();
    let letter = (key == NrKey::Other)
        .then(|| single(&keystroke.key))
        .flatten()
        .filter(char::is_ascii_alphanumeric);
    NrKeyPress {
        key,
        character: typed.map_or(0, u32::from),
        letter: letter.map_or(0, |letter| u32::from(letter.to_ascii_lowercase())),
        held,
    }
}

/// The characters a press types, in the form the core's fields take.
pub fn characters(keystroke: &Keystroke) -> String {
    keystroke.key_char.clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(key: &str, typed: Option<&str>, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: typed.map(str::to_owned),
        }
    }

    #[test]
    fn named_keys_typed_characters_and_held_keys_reach_the_core_as_it_numbers_them() {
        let escape = press(&stroke("escape", None, Modifiers::default()));
        assert_eq!(escape, NrKeyPress::named(NrKey::Escape));
        let shift_tab = press(&stroke("tab", None, Modifiers::shift()));
        assert_eq!(shift_tab.key, NrKey::Tab);
        assert!(shift_tab.shift());
        assert_eq!(
            press(&stroke("f2", None, Modifiers::default())).key,
            NrKey::F2
        );
        // Each named key goes back to the name it came from.
        for key in [NrKey::Escape, NrKey::PageDown, NrKey::F12] {
            let name = gpui_name(key).expect("a named key");
            assert_eq!(press(&stroke(&name, None, Modifiers::default())).key, key);
        }
        assert_eq!(gpui_name(NrKey::Other), None);
        let typed = press(&stroke("a", Some("A"), Modifiers::shift()));
        assert_eq!(
            (typed.key, typed.character, typed.letter),
            (NrKey::Other, u32::from('A'), u32::from('a'))
        );
        // ⌘ changes what a key types, so it types nothing.
        let command = press(&stroke("a", Some("a"), Modifiers::command()));
        assert_eq!((command.character, command.held), (0, NR_HELD_COMMAND));
    }
}
