//! A terminal's last output, kept as the app quits and shown again, dimmed, above the first new
//! session in that terminal (`Terminal::start`), as Warp brings back recent output. It is plain
//! text: colours, and what a full-screen program drew, are not kept.

use crate::strings::{self, key};
use std::fs;
use std::path::{Path, PathBuf};

/// How many of the last lines are kept.
const LINES_KEPT: usize = 2000;
/// The folder, in the drive's app data, that holds each terminal's kept text.
const FOLDER: &str = "terminals";

const RESET: &str = "\u{1b}[0m";
const DIM: &str = "\u{1b}[2m";

/// Where terminal `id`'s text is kept in the drive's app data `metadata`.
fn file(metadata: &Path, id: &str) -> PathBuf {
    let name: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    metadata.join(FOLDER).join(format!("{name}.txt"))
}

/// The last `LINES_KEPT` lines of `text`, without trailing blank lines, line ends or control
/// characters.
fn last_lines(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            line.chars()
                .filter(|c| !c.is_control())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    let end = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(0, |last| last + 1);
    lines[end.saturating_sub(LINES_KEPT)..end].join("\n")
}

/// Keeps `text`'s last lines for terminal `id`. Text with nothing to show keeps nothing, and
/// clears what was kept before.
pub fn keep(metadata: &Path, id: &str, text: &str) -> std::io::Result<()> {
    let path = file(metadata, id);
    let kept = last_lines(text);
    if kept.is_empty() {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let staged = path.with_extension("writing");
    fs::write(&staged, kept)?;
    fs::rename(staged, path)
}

/// Takes terminal `id`'s kept text, which shows once: it is gone once read.
pub fn take(metadata: &Path, id: &str) -> Option<String> {
    let path = file(metadata, id);
    let text = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(&path);
    (!text.is_empty()).then_some(text)
}

/// Keeps the command that resumes terminal `id`'s agent (`resume::command`), or clears it for
/// `None`.
pub fn keep_resume(metadata: &Path, id: &str, command: Option<&str>) -> std::io::Result<()> {
    let path = file(metadata, id).with_extension("resume");
    let Some(command) = command else {
        return match fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    };
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    fs::write(path, command)
}

/// Takes terminal `id`'s resume command, which runs once: it is gone once read.
pub fn take_resume(metadata: &Path, id: &str) -> Option<String> {
    let path = file(metadata, id).with_extension("resume");
    let command = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(&path);
    (!command.trim().is_empty()).then_some(command)
}

/// What a session shows before its shell's first output: `text` dimmed, then a dimmed line in
/// `language` saying it was restored.
pub fn shown(text: &str, language: &str) -> Vec<u8> {
    let mut bytes = format!("{RESET}{DIM}").into_bytes();
    for line in text.lines() {
        bytes.extend_from_slice(line.as_bytes());
        bytes.extend_from_slice(b"\r\n");
    }
    let said = strings::text(language, key::TERMINAL_RESTORED);
    bytes.extend_from_slice(format!("── {said} ──{RESET}\r\n\r\n").as_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_s_last_lines_are_kept_once() {
        let metadata = tempfile::tempdir().expect("metadata");
        let text = "% ls\nAGENTS.md  Films   \n\u{7}% \n\n\n";
        keep(metadata.path(), "16777234:42", text).expect("kept");
        assert_eq!(
            take(metadata.path(), "16777234:42").as_deref(),
            Some("% ls\nAGENTS.md  Films\n%")
        );
        // Shown once: the next session starts clean.
        assert_eq!(take(metadata.path(), "16777234:42"), None);
    }

    #[test]
    fn only_the_last_lines_are_kept_and_blank_text_keeps_nothing() {
        let metadata = tempfile::tempdir().expect("metadata");
        let many: String = (0..LINES_KEPT + 5).map(|n| format!("line {n}\n")).collect();
        keep(metadata.path(), "t", &many).expect("kept");
        let kept = take(metadata.path(), "t").expect("text");
        assert_eq!(kept.lines().count(), LINES_KEPT);
        assert_eq!(kept.lines().next(), Some("line 5"));

        keep(metadata.path(), "t", "old").expect("kept");
        keep(metadata.path(), "t", "  \n \n").expect("cleared");
        assert_eq!(take(metadata.path(), "t"), None);
    }

    #[test]
    fn a_resume_command_runs_once_and_none_clears_it() {
        let metadata = tempfile::tempdir().expect("metadata");
        keep_resume(metadata.path(), "t", Some("claude --resume s")).expect("kept");
        assert_eq!(
            take_resume(metadata.path(), "t").as_deref(),
            Some("claude --resume s")
        );
        assert_eq!(take_resume(metadata.path(), "t"), None);
        keep_resume(metadata.path(), "t", Some("claude --resume s")).expect("kept");
        keep_resume(metadata.path(), "t", None).expect("cleared");
        assert_eq!(take_resume(metadata.path(), "t"), None);
    }

    #[test]
    fn restored_text_shows_dimmed_above_a_line_that_says_so() {
        let shown = String::from_utf8(shown("% ls\nFilms", "en")).expect("text");
        assert_eq!(
            shown,
            "\u{1b}[0m\u{1b}[2m% ls\r\nFilms\r\n── Restored from your last session ──\u{1b}[0m\r\n\r\n"
        );
    }
}
