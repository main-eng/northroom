//! Turns keys, mouse, wheel and paste into the bytes xterm would send to a terminal program.
//! It is plain arithmetic on the shell's input and the program's modes, so every platform sends
//! the same bytes.

use crate::ffi::NrKey;

use crate::terminal::cells::{Modes, MouseTracking, Side};

/// Held modifier keys as xterm numbers them: 1 shift, 2 option/alt, 4 control.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Modifiers(pub u32);

impl Modifiers {
    pub const SHIFT: u32 = 1;
    pub const ALT: u32 = 2;
    pub const CONTROL: u32 = 4;

    /// Reads the boundary's modifier bits (`menus::COMMAND`…). Command never reaches a terminal
    /// program.
    pub fn held(held: u32) -> Self {
        Self(
            [
                (crate::menus::SHIFT, Self::SHIFT),
                (crate::menus::OPTION, Self::ALT),
                (crate::menus::CONTROL, Self::CONTROL),
            ]
            .into_iter()
            .filter(|(boundary, _)| held & boundary != 0)
            .fold(0, |bits, (_, own)| bits | own),
        )
    }

    /// The keys held as the boundary numbers them.
    pub fn boundary(self) -> u32 {
        [
            (Self::SHIFT, crate::menus::SHIFT),
            (Self::ALT, crate::menus::OPTION),
            (Self::CONTROL, crate::menus::CONTROL),
        ]
        .into_iter()
        .filter(|(own, _)| self.0 & own != 0)
        .fold(0, |bits, (_, boundary)| bits | boundary)
    }

    pub fn shift(self) -> bool {
        self.0 & Self::SHIFT != 0
    }

    pub fn alt(self) -> bool {
        self.0 & Self::ALT != 0
    }

    pub fn control(self) -> bool {
        self.0 & Self::CONTROL != 0
    }

    fn any(self) -> bool {
        self.0 & 7 != 0
    }

    /// xterm's modifier parameter: one more than the held keys' bits.
    fn parameter(self) -> u32 {
        1 + (self.0 & 7)
    }
}

/// A key with no character of its own, or a character typed with control or option held.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    Return,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    /// F1 to F12.
    Function(u8),
    Character(char),
}

impl Key {
    /// The key for a press with no character of its own. Space is typed as text, so it is
    /// `None`.
    pub fn from_press(key: NrKey) -> Option<Self> {
        Some(match key {
            NrKey::Return => Self::Return,
            NrKey::Tab => Self::Tab,
            NrKey::Backspace => Self::Backspace,
            NrKey::Escape => Self::Escape,
            NrKey::Up => Self::Up,
            NrKey::Down => Self::Down,
            NrKey::Right => Self::Right,
            NrKey::Left => Self::Left,
            NrKey::Home => Self::Home,
            NrKey::End => Self::End,
            NrKey::PageUp => Self::PageUp,
            NrKey::PageDown => Self::PageDown,
            NrKey::Delete => Self::Delete,
            NrKey::Insert => Self::Insert,
            NrKey::Other | NrKey::Space => return None,
            function => Self::Function(function.function()?),
        })
    }

    /// The key as the boundary names it; a character is `Other`, its text said beside it.
    pub fn boundary(self) -> NrKey {
        match self {
            Self::Character(_) => NrKey::Other,
            Self::Return => NrKey::Return,
            Self::Tab => NrKey::Tab,
            Self::Backspace => NrKey::Backspace,
            Self::Escape => NrKey::Escape,
            Self::Up => NrKey::Up,
            Self::Down => NrKey::Down,
            Self::Right => NrKey::Right,
            Self::Left => NrKey::Left,
            Self::Home => NrKey::Home,
            Self::End => NrKey::End,
            Self::PageUp => NrKey::PageUp,
            Self::PageDown => NrKey::PageDown,
            Self::Delete => NrKey::Delete,
            Self::Insert => NrKey::Insert,
            Self::Function(number) => NrKey::FUNCTIONS[usize::from(number.clamp(1, 12)) - 1],
        }
    }
}

/// The byte control makes of a character, as on a VT keyboard.
fn control_byte(character: char) -> Option<u8> {
    Some(match character {
        'a'..='z' => character as u8 - 0x60,
        '@'..='_' => character as u8 & 0x1f,
        ' ' | '2' => 0,
        '3'..='7' => character as u8 - b'3' + 0x1b,
        '8' | '?' => 0x7f,
        '/' => 0x1f,
        _ => return None,
    })
}

/// `ESC [ 1 ; modifiers final`, or the short form with nothing held: `ESC O final` when the
/// program asked for application keys and this key has such a form, else `ESC [ final`.
fn cursor_sequence(last: char, modifiers: Modifiers, application: bool) -> Vec<u8> {
    if modifiers.any() {
        format!("\x1b[1;{}{last}", modifiers.parameter()).into_bytes()
    } else if application {
        format!("\x1bO{last}").into_bytes()
    } else {
        format!("\x1b[{last}").into_bytes()
    }
}

/// `ESC [ number ~`, with the modifiers after a semicolon when any are held.
fn tilde_sequence(number: u32, modifiers: Modifiers) -> Vec<u8> {
    if modifiers.any() {
        format!("\x1b[{number};{}~", modifiers.parameter()).into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

/// The bytes a key sends.
pub fn encode_key(key: Key, modifiers: Modifiers, modes: Modes) -> Vec<u8> {
    let application = modes.app_cursor;
    let only_alt = modifiers.0 & 7 == Modifiers::ALT;
    let escaped = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if modifiers.alt() {
            out.push(0x1b);
        }
        out.extend_from_slice(bytes);
        out
    };
    match key {
        // Agent programs read Shift+Return as a newline that does not submit. It sends the same
        // bytes as Option+Return.
        Key::Return if modifiers.shift() => b"\x1b\r".to_vec(),
        Key::Return => escaped(b"\r"),
        Key::Tab if modifiers.shift() => b"\x1b[Z".to_vec(),
        Key::Tab => escaped(b"\t"),
        Key::Backspace if modifiers.control() => escaped(&[0x08]),
        Key::Backspace => escaped(&[0x7f]),
        Key::Escape => escaped(&[0x1b]),
        // Option+arrow moves by word, as in every Mac terminal, using the shell's `ESC b` and
        // `ESC f`.
        Key::Left if only_alt => b"\x1bb".to_vec(),
        Key::Right if only_alt => b"\x1bf".to_vec(),
        Key::Up => cursor_sequence('A', modifiers, application),
        Key::Down => cursor_sequence('B', modifiers, application),
        Key::Right => cursor_sequence('C', modifiers, application),
        Key::Left => cursor_sequence('D', modifiers, application),
        Key::Home => cursor_sequence('H', modifiers, application),
        Key::End => cursor_sequence('F', modifiers, application),
        Key::Insert => tilde_sequence(2, modifiers),
        Key::Delete if only_alt => b"\x1bd".to_vec(),
        Key::Delete => tilde_sequence(3, modifiers),
        Key::PageUp => tilde_sequence(5, modifiers),
        Key::PageDown => tilde_sequence(6, modifiers),
        Key::Function(number @ 1..=4) => {
            cursor_sequence(char::from(b'P' + number - 1), modifiers, true)
        }
        Key::Function(number) => {
            const NUMBERS: [u32; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            match NUMBERS.get(usize::from(number).wrapping_sub(5)) {
                Some(code) => tilde_sequence(*code, modifiers),
                None => Vec::new(),
            }
        }
        Key::Character(character) => {
            let mut utf8 = [0; 4];
            match control_byte(character).filter(|_| modifiers.control()) {
                Some(byte) => escaped(&[byte]),
                None => escaped(character.encode_utf8(&mut utf8).as_bytes()),
            }
        }
    }
}

/// The US layouts, by the platform's names, where Option with a letter is Meta for shells and
/// editors. Other layouts type their own characters with Option (`@`, `[`, `|` on many European
/// keyboards), so Option is left to them.
const META_LAYOUTS: [&str; 3] = [
    "com.apple.keylayout.US",
    "com.apple.keylayout.ABC",
    "com.apple.keylayout.USExtended",
];

pub fn option_is_meta(layout: &str) -> bool {
    META_LAYOUTS.contains(&layout)
}

/// The character of a key held with control or option: what it types with shift alone
/// (`plain`). On a non-Latin layout, maps the control byte it typed (`typed`) back to its Latin
/// letter.
fn control_character(typed: &str, plain: &str) -> Option<char> {
    let plain = plain.chars().next()?;
    match typed.chars().next() {
        Some(control) if u32::from(plain) >= 0x80 && u32::from(control) < 0x20 => {
            char::from_u32(u32::from(control) + 0x60)
        }
        _ => Some(plain),
    }
}

/// The key and modifiers to send when control, or Option as Meta, is held with a character key
/// on `layout`. `None` when the input system should compose the key as text. Option on a layout
/// that types with it is not reported as held.
pub fn held_character(
    layout: &str,
    typed: &str,
    plain: &str,
    modifiers: Modifiers,
) -> Option<(Key, Modifiers)> {
    let meta = modifiers.alt() && option_is_meta(layout);
    if !modifiers.control() && !meta {
        return None;
    }
    let told = if meta {
        modifiers
    } else {
        Modifiers(modifiers.0 & !Modifiers::ALT)
    };
    Some((Key::Character(control_character(typed, plain)?), told))
}

/// Wheel travel for the terminal: a trackpad's (`precise`) points in view pixels, a wheel's
/// notches unchanged.
pub fn wheel_travel(delta: f64, precise: bool, scale: f64) -> f32 {
    (if precise { delta * scale } else { delta }) as f32
}

/// What a mouse did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseAction {
    Press,
    Release,
    /// Moved with the button held.
    Drag,
    WheelUp,
    WheelDown,
}

/// The report of a mouse action at a cell (both counted from zero) for a program that asked
/// for the mouse, or `None` when it did not ask for this kind. `button`: 0 left, 1 middle,
/// 2 right.
pub fn encode_mouse(
    action: MouseAction,
    button: u32,
    column: usize,
    line: usize,
    modifiers: Modifiers,
    modes: Modes,
) -> Option<Vec<u8>> {
    let tracking = modes.mouse?;
    if action == MouseAction::Drag && tracking == MouseTracking::Clicks {
        return None;
    }
    let sgr = modes.sgr_mouse;
    let mut code = match action {
        MouseAction::WheelUp => 64,
        MouseAction::WheelDown => 65,
        // The legacy encoding cannot say which button was released.
        MouseAction::Release if !sgr => 3,
        MouseAction::Drag => button + 32,
        MouseAction::Press | MouseAction::Release => button,
    };
    code += u32::from(modifiers.shift()) * 4
        + u32::from(modifiers.alt()) * 8
        + u32::from(modifiers.control()) * 16;
    if sgr {
        let last = if action == MouseAction::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{};{}{last}", column + 1, line + 1).into_bytes());
    }
    // One byte each, offset by 32: cells past 223 cannot be said and are not reported.
    let byte = |value: usize| u8::try_from(value + 33).ok();
    Some(vec![
        0x1b,
        b'[',
        b'M',
        (code + 32) as u8,
        byte(column)?,
        byte(line)?,
    ])
}

/// Turns the wheel's or trackpad's pixels into whole lines, keeping what is left of a line
/// for the next event so slow scrolling still moves.
#[derive(Default)]
pub struct Wheel {
    remainder: f32,
}

impl Wheel {
    /// The lines to move for `pixels` more of travel; positive is towards older output.
    pub fn lines(&mut self, pixels: f32, line_height: f32) -> i32 {
        // Reversing direction starts a new gesture.
        if self.remainder * pixels < 0.0 {
            self.remainder = 0.0;
        }
        self.remainder += pixels;
        let lines = (self.remainder / line_height).trunc();
        self.remainder -= lines * line_height;
        lines as i32
    }
}

/// What a wheel turn of `lines` (positive is up) sends to a program on the alternate screen
/// that did not ask for the mouse: the arrow keys, as many as lines.
pub fn wheel_as_arrows(lines: i32, modes: Modes) -> Vec<u8> {
    let key = if lines > 0 { Key::Up } else { Key::Down };
    encode_key(key, Modifiers::default(), modes).repeat(lines.unsigned_abs() as usize)
}

/// Pasted text as the program should read it. A program that asked for bracketed paste gets
/// the text between the markers, without the bytes that could end the paste early or interrupt
/// it; any other gets it as if typed, each line ended with Return.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend(text.bytes().filter(|byte| *byte != 0x1b && *byte != 0x03));
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// Dropped files as typed text, escaped as Terminal.app escapes them: a backslash before every
/// character a shell would act on and a space after each path. Agents read a pasted path as a
/// file reference and attach it when it is an image.
pub fn reference_text(paths: &[std::path::PathBuf]) -> String {
    let mut text = String::new();
    for path in paths {
        for character in path.to_string_lossy().chars() {
            let plain = !character.is_ascii()
                || character.is_ascii_alphanumeric()
                || "/._-+:@%,=".contains(character);
            // A control character would be typed as a key, not read as part of a name.
            if character.is_ascii_control() {
                continue;
            }
            if !plain {
                text.push('\\');
            }
            text.push(character);
        }
        text.push(' ');
    }
    text
}

/// The cell under a point in the view's pixels from the grid's top left, kept inside the grid,
/// and which half of the cell the point is in. `line` may be negative or past the last line: the point is above or
/// below the grid, and a drag there scrolls.
pub struct CellAt {
    pub column: usize,
    pub line: i32,
    pub side: Side,
}

pub fn cell_at(x: f32, y: f32, cell: (f32, f32), columns: usize) -> CellAt {
    let across = (x / cell.0).max(0.0);
    let column = (across as usize).min(columns.saturating_sub(1));
    let side = if across.fract() < 0.5 && across < columns as f32 {
        Side::Left
    } else {
        Side::Right
    };
    CellAt {
        column,
        line: (y / cell.1).floor() as i32,
        side,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_CURSOR: Modes = Modes {
        app_cursor: true,
        mouse: None,
        sgr_mouse: false,
        alt_screen: false,
        alternate_scroll: false,
        bracketed_paste: false,
        focus_reports: false,
    };

    #[test]
    fn the_boundarys_held_keys_become_xterms_and_back() {
        use crate::menus::{COMMAND, CONTROL, OPTION, SHIFT};
        assert_eq!(
            Modifiers::held(SHIFT | CONTROL),
            Modifiers(Modifiers::SHIFT | Modifiers::CONTROL)
        );
        assert_eq!(Modifiers::held(OPTION | COMMAND), Modifiers(Modifiers::ALT));
        assert_eq!(Modifiers(7).boundary(), SHIFT | OPTION | CONTROL);
        assert_eq!(Modifiers::held(0), Modifiers(0));
    }

    const NONE: Modifiers = Modifiers(0);
    const SHIFT: Modifiers = Modifiers(Modifiers::SHIFT);
    const ALT: Modifiers = Modifiers(Modifiers::ALT);
    const CONTROL: Modifiers = Modifiers(Modifiers::CONTROL);

    fn key(key: Key, modifiers: Modifiers) -> Vec<u8> {
        encode_key(key, modifiers, Modes::default())
    }

    #[test]
    fn arrows_follow_the_cursor_key_mode() {
        assert_eq!(key(Key::Up, NONE), b"\x1b[A");
        assert_eq!(encode_key(Key::Up, NONE, APP_CURSOR), b"\x1bOA");
        assert_eq!(encode_key(Key::Home, NONE, APP_CURSOR), b"\x1bOH");
        assert_eq!(key(Key::End, NONE), b"\x1b[F");
    }

    #[test]
    fn modified_arrows_carry_the_modifier_parameter() {
        assert_eq!(key(Key::Left, SHIFT), b"\x1b[1;2D");
        assert_eq!(key(Key::Right, CONTROL), b"\x1b[1;5C");
        assert_eq!(key(Key::Up, ALT), b"\x1b[1;3A");
        assert_eq!(
            key(Key::Down, Modifiers(Modifiers::SHIFT | Modifiers::CONTROL)),
            b"\x1b[1;6B"
        );
        // Application mode does not change a modified arrow.
        assert_eq!(encode_key(Key::Left, SHIFT, APP_CURSOR), b"\x1b[1;2D");
    }

    #[test]
    fn option_moves_and_deletes_by_word() {
        assert_eq!(key(Key::Left, ALT), b"\x1bb");
        assert_eq!(key(Key::Right, ALT), b"\x1bf");
        assert_eq!(key(Key::Backspace, ALT), b"\x1b\x7f");
        assert_eq!(key(Key::Delete, ALT), b"\x1bd");
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(key(Key::Delete, NONE), b"\x1b[3~");
        assert_eq!(key(Key::PageUp, NONE), b"\x1b[5~");
        assert_eq!(key(Key::PageDown, CONTROL), b"\x1b[6;5~");
        assert_eq!(key(Key::Insert, NONE), b"\x1b[2~");
        assert_eq!(key(Key::Function(1), NONE), b"\x1bOP");
        assert_eq!(key(Key::Function(4), SHIFT), b"\x1b[1;2S");
        assert_eq!(key(Key::Function(5), NONE), b"\x1b[15~");
        assert_eq!(key(Key::Function(12), NONE), b"\x1b[24~");
        assert_eq!(key(Key::Function(12), ALT), b"\x1b[24;3~");
    }

    #[test]
    fn return_tab_backspace_escape() {
        assert_eq!(key(Key::Return, NONE), b"\r");
        assert_eq!(key(Key::Return, SHIFT), b"\x1b\r");
        assert_eq!(key(Key::Return, ALT), b"\x1b\r");
        assert_eq!(key(Key::Tab, NONE), b"\t");
        assert_eq!(key(Key::Tab, SHIFT), b"\x1b[Z");
        assert_eq!(key(Key::Backspace, NONE), b"\x7f");
        assert_eq!(key(Key::Backspace, CONTROL), b"\x08");
        assert_eq!(key(Key::Escape, NONE), b"\x1b");
    }

    #[test]
    fn control_and_option_with_characters() {
        assert_eq!(key(Key::Character('c'), CONTROL), [0x03]);
        assert_eq!(key(Key::Character('C'), CONTROL), [0x03]);
        assert_eq!(key(Key::Character(' '), CONTROL), [0x00]);
        assert_eq!(key(Key::Character('['), CONTROL), [0x1b]);
        assert_eq!(key(Key::Character('/'), CONTROL), [0x1f]);
        assert_eq!(key(Key::Character('b'), ALT), b"\x1bb");
        assert_eq!(key(Key::Character('B'), ALT), b"\x1bB");
        assert_eq!(
            key(
                Key::Character('x'),
                Modifiers(Modifiers::ALT | Modifiers::CONTROL)
            ),
            [0x1b, 0x18]
        );
        // A character control has no byte for is sent as itself.
        assert_eq!(key(Key::Character('é'), CONTROL), "é".as_bytes());
    }

    #[test]
    fn keys_from_the_boundary() {
        assert_eq!(Key::from_press(NrKey::Return), Some(Key::Return));
        assert_eq!(Key::from_press(NrKey::F1), Some(Key::Function(1)));
        assert_eq!(Key::from_press(NrKey::F12), Some(Key::Function(12)));
        assert_eq!(Key::from_press(NrKey::Insert), Some(Key::Insert));
        // Space and the keys that type are text.
        assert_eq!(Key::from_press(NrKey::Space), None);
        assert_eq!(Key::from_press(NrKey::Other), None);
        for key in [
            Key::Return,
            Key::Delete,
            Key::Function(1),
            Key::Function(12),
            Key::PageUp,
        ] {
            assert_eq!(Key::from_press(key.boundary()), Some(key));
        }
        assert_eq!(Key::Character('q').boundary(), NrKey::Other);
    }

    #[test]
    fn option_is_meta_only_on_us_layouts() {
        let both = Modifiers(Modifiers::ALT);
        assert_eq!(
            held_character("com.apple.keylayout.US", "∫", "b", both),
            Some((Key::Character('b'), both))
        );
        assert_eq!(
            held_character("com.apple.keylayout.ABC", "∫", "b", both),
            Some((Key::Character('b'), both))
        );
        // A German layout types `@` with Option+L: that is text.
        assert_eq!(
            held_character("com.apple.keylayout.German", "@", "l", both),
            None
        );
        assert_eq!(held_character("", "x", "x", both), None);
        assert_eq!(
            held_character("com.apple.keylayout.US", "a", "a", NONE),
            None
        );
    }

    #[test]
    fn control_names_its_letter_on_any_layout() {
        // Control+C: the program is told control and the letter; Option held on a layout that
        // types with it is not told.
        let control = Modifiers(Modifiers::CONTROL | Modifiers::ALT);
        assert_eq!(
            held_character("com.apple.keylayout.French", "\u{3}", "c", control),
            Some((Key::Character('c'), CONTROL))
        );
        // A Russian layout's control+С types the control byte; the letter is the Latin one.
        assert_eq!(
            held_character("com.apple.keylayout.Russian", "\u{3}", "с", CONTROL),
            Some((Key::Character('c'), CONTROL))
        );
        assert_eq!(
            held_character("com.apple.keylayout.Russian", "с", "с", CONTROL),
            Some((Key::Character('с'), CONTROL))
        );
        assert_eq!(
            held_character("com.apple.keylayout.US", "", "", CONTROL),
            None,
            "a key with no character"
        );
    }

    #[test]
    fn a_trackpads_points_become_pixels() {
        assert_eq!(wheel_travel(3.5, true, 2.0), 7.0);
        assert_eq!(wheel_travel(-1.0, false, 2.0), -1.0);
    }

    #[test]
    fn mouse_is_reported_only_when_asked_for() {
        assert_eq!(
            encode_mouse(MouseAction::Press, 0, 0, 0, NONE, Modes::default()),
            None
        );
        let click = Modes {
            mouse: Some(MouseTracking::Clicks),
            ..Modes::default()
        };
        assert_eq!(
            encode_mouse(MouseAction::Press, 0, 0, 0, NONE, click),
            Some(b"\x1b[M !!".to_vec())
        );
        assert_eq!(
            encode_mouse(MouseAction::Release, 0, 2, 1, NONE, click),
            Some(b"\x1b[M##\"".to_vec())
        );
        // Clicks alone: a drag is not reported.
        assert_eq!(encode_mouse(MouseAction::Drag, 0, 2, 1, NONE, click), None);
        // The legacy encoding cannot say a cell past 223.
        assert_eq!(
            encode_mouse(MouseAction::Press, 0, 300, 1, NONE, click),
            None
        );
    }

    #[test]
    fn sgr_mouse_reports() {
        let mode = Modes {
            mouse: Some(MouseTracking::Drags),
            sgr_mouse: true,
            ..Modes::default()
        };
        assert_eq!(
            encode_mouse(MouseAction::Press, 0, 9, 4, NONE, mode),
            Some(b"\x1b[<0;10;5M".to_vec())
        );
        assert_eq!(
            encode_mouse(MouseAction::Release, 2, 9, 4, NONE, mode),
            Some(b"\x1b[<2;10;5m".to_vec())
        );
        assert_eq!(
            encode_mouse(MouseAction::Drag, 0, 300, 4, NONE, mode),
            Some(b"\x1b[<32;301;5M".to_vec())
        );
        assert_eq!(
            encode_mouse(MouseAction::WheelUp, 0, 0, 0, NONE, mode),
            Some(b"\x1b[<64;1;1M".to_vec())
        );
        assert_eq!(
            encode_mouse(MouseAction::WheelDown, 0, 0, 0, CONTROL, mode),
            Some(b"\x1b[<81;1;1M".to_vec())
        );
    }

    #[test]
    fn wheel_accumulates_pixels_into_lines() {
        let mut wheel = Wheel::default();
        // Ten pixels at a time on thirty-pixel lines: every third event moves a line.
        assert_eq!(wheel.lines(10.0, 30.0), 0);
        assert_eq!(wheel.lines(10.0, 30.0), 0);
        assert_eq!(wheel.lines(10.0, 30.0), 1);
        assert_eq!(wheel.lines(75.0, 30.0), 2);
        assert_eq!(wheel.lines(15.0, 30.0), 1);
        // Reversing direction drops the remainder.
        assert_eq!(wheel.lines(20.0, 30.0), 0);
        assert_eq!(wheel.lines(-20.0, 30.0), 0);
        assert_eq!(wheel.lines(-20.0, 30.0), -1);
    }

    #[test]
    fn wheel_on_the_alternate_screen_is_arrow_keys() {
        assert_eq!(wheel_as_arrows(2, Modes::default()), b"\x1b[A\x1b[A");
        assert_eq!(wheel_as_arrows(-1, APP_CURSOR), b"\x1bOB");
        assert_eq!(wheel_as_arrows(0, Modes::default()), b"");
    }

    #[test]
    fn paste_is_bracketed_when_asked_for() {
        assert_eq!(paste_bytes("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
        // Nothing pasted can end the paste early.
        assert_eq!(
            paste_bytes("x\x1b[201~rm", true),
            b"\x1b[200~x[201~rm\x1b[201~"
        );
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[test]
    fn dropped_files_are_written_as_a_shell_reads_them() {
        use std::path::PathBuf;
        let paths = [
            PathBuf::from(
                "/Users/a/My Workspace/Screenshots/Screenshot 2026-11-12 at 7.45.07\u{202f}PM.png",
            ),
            PathBuf::from("/tmp/it's (1) & $x.png"),
        ];
        assert_eq!(
            reference_text(&paths),
            "/Users/a/My\\ Workspace/Screenshots/Screenshot\\ 2026-11-12\\ at\\ 7.45.07\u{202f}PM.png \
             /tmp/it\\'s\\ \\(1\\)\\ \\&\\ \\$x.png "
        );
        assert_eq!(
            reference_text(&[PathBuf::from("/tmp/a\x1b\nb")]),
            "/tmp/ab "
        );
        assert_eq!(reference_text(&[]), "");
    }

    #[test]
    fn a_point_names_its_cell_and_side() {
        let at = cell_at(20.0, 65.0, (16.0, 30.0), 80);
        assert_eq!((at.column, at.line, at.side), (1, 2, Side::Left));
        let at = cell_at(30.0, 0.0, (16.0, 30.0), 80);
        assert_eq!((at.column, at.line, at.side), (1, 0, Side::Right));
        // Past the right edge is the last cell's right side; above the grid is a negative line.
        let at = cell_at(5000.0, -4.0, (16.0, 30.0), 80);
        assert_eq!((at.column, at.line, at.side), (79, -1, Side::Right));
        let at = cell_at(-10.0, 10.0, (16.0, 30.0), 80);
        assert_eq!((at.column, at.line, at.side), (0, 0, Side::Left));
    }
}
