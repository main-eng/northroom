//! The notice a terminal prints when its shell exits (ADR-0005). The terminal fills the view,
//! so this line is the only place to say the shell ended and that Return starts it again.

use crate::strings::{self, key};

const RESET: &str = "\u{1b}[0m";
const DIM: &str = "\u{1b}[2m";

/// How a shell ended: its exit code, or `None` when a signal ended it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
}

/// The dimmed line printed under the shell's last output, in `language`. It resets whatever
/// colour the shell left on first.
pub fn ended_notice(language: &str) -> String {
    format!(
        "{RESET}\r\n{DIM}{}{RESET}\r\n",
        strings::text(language, key::TERMINAL_EXIT_ENDED)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_that_ended_says_return_starts_it_again() {
        assert_eq!(
            ended_notice("en"),
            "\u{1b}[0m\r\n\u{1b}[2mTerminal ended. Press Return to start it again.\u{1b}[0m\r\n"
        );
    }
}
