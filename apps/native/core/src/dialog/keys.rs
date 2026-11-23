//! Keyboard focus and key handling for dialogs drawn over the window: the naming dialog and the
//! alert before trashing, modelled on Radix's `Dialog` and `AlertDialog`. The rules live here;
//! a shell draws the focus and performs the result. Also sets how long a notice stays.

use crate::theme::Token;

/// The dialogs, each with its own controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    /// A name to type: 0 the field, 1 Cancel, 2 the action (Save), 3 the close mark.
    Prompt,
    /// A question before something is done: 0 Cancel, 1 the action.
    Alert,
}

impl Dialog {
    /// Whether pressing `part` takes the dialog's action. Every other button, including the
    /// close mark, acts as Cancel.
    pub fn takes(self, part: usize) -> bool {
        self.primary() == part
    }

    fn parts(self) -> &'static [Part] {
        const PROMPT: [Part; 4] = [
            Part::untabbed(Role::LineField),
            Part::untabbed(Role::Button),
            Part::untabbed(Role::Button),
            Part::untabbed(Role::Close),
        ];
        const ALERT: [Part; 2] = [Part::tabbed(Role::Button), Part::tabbed(Role::Button)];
        match self {
            Dialog::Prompt => &PROMPT,
            Dialog::Alert => &ALERT,
        }
    }

    /// The button Return presses from a one-line field.
    fn primary(self) -> usize {
        match self {
            Dialog::Prompt => 2,
            Dialog::Alert => 1,
        }
    }
}

/// How a control behaves for the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// A one-line text field. Return presses the dialog's action.
    LineField,
    Button,
    /// The close mark in the corner.
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Part {
    role: Role,
    /// Whether Tab visits it.
    tab_stop: bool,
}

impl Part {
    const fn tabbed(role: Role) -> Self {
        Self {
            role,
            tab_stop: true,
        }
    }

    const fn untabbed(role: Role) -> Self {
        Self {
            role,
            tab_stop: false,
        }
    }
}

/// A key while a dialog is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tab,
    BackTab,
    Return,
    Space,
    Escape,
    Down,
    Up,
    Other,
}

/// What a key asks of the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    /// Not a dialog key. It passes to the focused control (a field types it).
    PassOn,
    /// Focus moved to this control, with its ring shown.
    Focus(usize),
    /// This button or mark is pressed.
    Press(usize),
    /// The dialog closes as Cancel closes it.
    Close,
}

/// Which control of an open dialog has focus, and whether its ring shows. Like
/// `:focus-visible`, a button shows its ring only after keyboard focus; a field always does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Focus {
    pub dialog: Dialog,
    pub focused: usize,
    pub ring: bool,
}

impl Focus {
    /// Initial focus: a naming dialog's field, or an alert's Cancel. `by_key` means a key
    /// opened the dialog.
    pub fn open(dialog: Dialog, by_key: bool) -> Self {
        let ring = match dialog {
            Dialog::Prompt => true,
            Dialog::Alert => by_key,
        };
        Self {
            dialog,
            focused: 0,
            ring,
        }
    }

    /// A click focused `part`, without the ring.
    pub fn point(&mut self, part: usize) {
        if part < self.dialog.parts().len() {
            self.focused = part;
            self.ring = false;
        }
    }

    /// Tab moves to the next control in `order` and ⇧Tab to the previous, wrapping, with the
    /// ring shown.
    fn go_round(&mut self, order: &[usize], key: Key) -> Intent {
        if order.is_empty() {
            return Intent::PassOn;
        }
        let at = order.iter().position(|index| *index == self.focused);
        let next = match (at, key) {
            (Some(at), Key::Tab) => (at + 1) % order.len(),
            (Some(at), _) => (at + order.len() - 1) % order.len(),
            (None, Key::Tab) => 0,
            (None, _) => order.len() - 1,
        };
        self.focused = order[next];
        self.ring = true;
        Intent::Focus(order[next])
    }

    /// What `key` does. Escape closes. Tab and ⇧Tab cycle focus. Return presses the focused
    /// button, or the action from a one-line field. Space acts like Return except in a field,
    /// where it types.
    pub fn key(&mut self, key: Key) -> Intent {
        let parts = self.dialog.parts();
        match key {
            Key::Escape => Intent::Close,
            Key::Tab | Key::BackTab => {
                let order: Vec<usize> = (0..parts.len())
                    .filter(|index| parts[*index].tab_stop)
                    .collect();
                self.go_round(&order, key)
            }
            Key::Return | Key::Space => match parts[self.focused].role {
                Role::LineField if key == Key::Return => Intent::Press(self.dialog.primary()),
                Role::LineField => Intent::PassOn,
                Role::Button | Role::Close => Intent::Press(self.focused),
            },
            Key::Down | Key::Up | Key::Other => Intent::PassOn,
        }
    }
}

/// Text colour for a dialog's action button. On the accent it uses the accent foreground. On
/// the warning colour it uses ink in a dark theme and the on-accent text colour in a light one.
pub fn action_text_color(destructive: bool, dark: bool) -> Token {
    match (destructive, dark) {
        (false, _) => Token::PrimaryForeground,
        (true, true) => Token::Ink,
        (true, false) => Token::OnAccent,
    }
}

/// Notices show one at a time. A new one replaces the current one, and each closes after 5 s
/// or on a click.
pub const TOAST_SECONDS: f64 = 5.0;
pub const TOASTS_AT_ONCE: usize = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_action_takes_a_dialogs_answer() {
        assert!(Dialog::Prompt.takes(2) && Dialog::Alert.takes(1));
        // Cancel, and the close mark of a dialog that asks for a name, answer as Cancel does.
        assert!(!Dialog::Prompt.takes(1) && !Dialog::Prompt.takes(3) && !Dialog::Alert.takes(0));
    }

    #[test]
    fn the_boundary_names_the_parts_as_the_dialogs_number_them() {
        use crate::menus::ffi::*;
        assert!(Dialog::Alert.takes(NR_ALERT_ACTION) && !Dialog::Alert.takes(NR_ALERT_CANCEL));
    }

    #[test]
    fn an_alert_opens_on_cancel_its_ring_shown_only_when_a_key_opened_it() {
        assert_eq!(
            Focus::open(Dialog::Alert, false),
            Focus {
                dialog: Dialog::Alert,
                focused: 0,
                ring: false
            }
        );
        let mut alert = Focus::open(Dialog::Alert, true);
        assert!(alert.ring);
        assert_eq!(
            alert.key(Key::Return),
            Intent::Press(0),
            "Return presses Cancel, which has the keys"
        );
        assert_eq!(alert.key(Key::Tab), Intent::Focus(1));
        assert_eq!(alert.key(Key::Space), Intent::Press(1));
        assert_eq!(alert.key(Key::BackTab), Intent::Focus(0));
        assert_eq!(alert.key(Key::Escape), Intent::Close);
        assert_eq!(alert.key(Key::Down), Intent::PassOn);
    }

    #[test]
    fn a_naming_dialog_takes_its_action_from_the_field() {
        let mut prompt = Focus::open(Dialog::Prompt, false);
        assert_eq!(prompt.focused, 0);
        assert_eq!(prompt.key(Key::Return), Intent::Press(2));
        assert_eq!(prompt.key(Key::Space), Intent::PassOn, "a space is typed");
        assert_eq!(
            prompt.key(Key::Tab),
            Intent::PassOn,
            "Tab stays the field's"
        );
        assert_eq!(prompt.key(Key::Escape), Intent::Close);
    }

    #[test]
    fn a_destructive_action_is_written_in_ink_on_a_dark_theme() {
        assert_eq!(action_text_color(false, true), Token::PrimaryForeground);
        assert_eq!(action_text_color(true, true), Token::Ink);
        assert_eq!(action_text_color(true, false), Token::OnAccent);
    }
}
