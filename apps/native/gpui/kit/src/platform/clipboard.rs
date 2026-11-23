//! The clipboard behind every copy and paste in the app, text and files alike. Normally it is
//! the system clipboard (GPUI's, plus the platform's for files). In a check (an unattended run,
//! `nr_launch`) it is private to the check: a named pasteboard on macOS, in-process elsewhere.

use crate::nr::nr_launch;
use gpui::{App, ClipboardItem};
use std::path::PathBuf;

pub fn write_clipboard(text: &str, cx: &mut App) {
    if nr_launch().unattended {
        check::write(text);
    } else {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_owned()));
    }
}

/// The clipboard text, if any.
pub fn read_clipboard(cx: &App) -> Option<String> {
    if nr_launch().unattended {
        check::read()
    } else {
        cx.read_from_clipboard().and_then(|item| item.text())
    }
}

/// Puts `paths` on the clipboard as files, plus the paths as text (one per line) for terminals
/// and text fields.
#[cfg(not(target_os = "linux"))]
pub fn write_clipboard_files(paths: &[PathBuf], _: &mut App) {
    if nr_launch().unattended {
        check::write_files(paths);
    } else {
        super::write_files(paths);
    }
}

/// Puts `paths` on the clipboard as text, one per line. GPUI's Linux clipboard (X11 and Wayland)
/// is text only, so file managers can't paste them but terminals and fields can. The app
/// remembers what it wrote (`Copied`) so its own folders can paste the files.
#[cfg(target_os = "linux")]
pub fn write_clipboard_files(paths: &[PathBuf], cx: &mut App) {
    if nr_launch().unattended {
        check::write_files(paths);
        return;
    }
    let copied = Copied::of(paths);
    cx.write_to_clipboard(ClipboardItem::new_string(copied.words.clone()));
    COPIED.with(|kept| *kept.borrow_mut() = Some(copied));
}

/// The files on the clipboard, in order; none when it holds no files.
#[cfg(not(target_os = "linux"))]
pub fn read_clipboard_files(cx: &App) -> Vec<PathBuf> {
    use gpui::ClipboardEntry;
    if nr_launch().unattended {
        return check::read_files();
    }
    let Some(item) = cx.read_from_clipboard() else {
        return Vec::new();
    };
    item.into_entries()
        .find_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
            ClipboardEntry::String(_) | ClipboardEntry::Image(_) => None,
        })
        .unwrap_or_default()
}

/// The files the app copied, as long as the clipboard still holds the exact text written for
/// them (`Copied::files_while`). GPUI's Linux clipboard never returns files, and text copied
/// elsewhere is never treated as files.
#[cfg(target_os = "linux")]
pub fn read_clipboard_files(cx: &App) -> Vec<PathBuf> {
    if nr_launch().unattended {
        return check::read_files();
    }
    let words = cx.read_from_clipboard().and_then(|item| item.text());
    COPIED.with(|kept| {
        kept.borrow()
            .as_ref()
            .map_or_else(Vec::new, |copied| copied.files_while(words.as_deref()))
    })
}

#[cfg(target_os = "linux")]
thread_local! {
    /// The files the app last copied; copy and paste run on the main thread.
    static COPIED: std::cell::RefCell<Option<Copied>> = const { std::cell::RefCell::new(None) };
}

/// Files copied to a text-only clipboard: the text written (one path per line) and the paths.
#[cfg(any(test, target_os = "linux"))]
#[derive(Clone, Debug, PartialEq)]
struct Copied {
    words: String,
    paths: Vec<PathBuf>,
}

#[cfg(any(test, target_os = "linux"))]
impl Copied {
    fn of(paths: &[PathBuf]) -> Self {
        let lines: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        Self {
            words: lines.join("\n"),
            paths: paths.to_vec(),
        }
    }

    /// The files while the clipboard text matches exactly; none once anything else is copied.
    fn files_while(&self, words: Option<&str>) -> Vec<PathBuf> {
        if words == Some(self.words.as_str()) {
            self.paths.clone()
        } else {
            Vec::new()
        }
    }
}

/// Whether the clipboard holds files, which enables a folder's Paste.
#[cfg(target_os = "macos")]
pub fn clipboard_holds_files(_: &App) -> bool {
    has_files()
}

/// Whether the clipboard holds files, which enables a folder's Paste.
#[cfg(not(target_os = "macos"))]
pub fn clipboard_holds_files(cx: &App) -> bool {
    !read_clipboard_files(cx).is_empty()
}

/// Whether the clipboard holds text or files, which enables the menu bar's Paste (fields paste
/// text, folders paste files). Reads only the types on the system clipboard.
#[cfg(target_os = "macos")]
pub(crate) fn holds_paste() -> bool {
    if nr_launch().unattended {
        check::read().is_some() || check::has_files()
    } else {
        super::mac::pasteboard::general_has_words() || has_files()
    }
}

/// Whether the clipboard holds files. Reads only the types on the system clipboard.
#[cfg(target_os = "macos")]
fn has_files() -> bool {
    if nr_launch().unattended {
        check::has_files()
    } else {
        super::mac::pasteboard::general_has_files()
    }
}

#[cfg(target_os = "macos")]
mod check {
    pub use super::super::mac::pasteboard::{
        check_has_files as has_files, check_read as read, check_read_files as read_files,
        check_write as write, check_write_files as write_files,
    };
}

#[cfg(not(target_os = "macos"))]
mod check {
    use std::cell::RefCell;
    use std::path::PathBuf;

    /// The check's clipboard contents: text, or files with their paths as text.
    #[derive(Default)]
    struct Kept {
        words: Option<String>,
        files: Vec<PathBuf>,
    }

    thread_local! {
        /// The check's clipboard; copy and paste run on the main thread.
        static KEPT: RefCell<Kept> = RefCell::new(Kept::default());
    }

    pub fn write(text: &str) {
        KEPT.with(|kept| {
            *kept.borrow_mut() = Kept {
                words: Some(text.to_owned()),
                files: Vec::new(),
            }
        });
    }

    pub fn read() -> Option<String> {
        KEPT.with(|kept| kept.borrow().words.clone())
    }

    pub fn write_files(paths: &[PathBuf]) {
        let lines: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        KEPT.with(|kept| {
            *kept.borrow_mut() = Kept {
                words: Some(lines.join("\n")),
                files: paths.to_vec(),
            }
        });
    }

    pub fn read_files() -> Vec<PathBuf> {
        KEPT.with(|kept| kept.borrow().files.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{Copied, check};

    /// On a text-only clipboard (Linux), copied files paste only while the clipboard holds the
    /// exact text written for them. The same paths copied elsewhere, a later copy, or an empty
    /// clipboard give no files.
    #[test]
    fn copied_files_are_pasted_only_while_their_words_are_on_the_clipboard() {
        let paths = vec![
            std::path::PathBuf::from("/drive/a picture ✓.png"),
            std::path::PathBuf::from("/drive/Notes"),
        ];
        let copied = Copied::of(&paths);
        assert_eq!(copied.words, "/drive/a picture ✓.png\n/drive/Notes");
        assert_eq!(copied.files_while(Some(&copied.words)), paths);
        assert!(copied.files_while(Some("/drive/Notes")).is_empty());
        assert!(
            copied
                .files_while(Some("/drive/a picture ✓.png\n/drive/Notes\n"))
                .is_empty()
        );
        assert!(copied.files_while(None).is_empty());
    }

    /// The check's pasteboard is shared by the process, so these tests take turns.
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A check's copy lands on its own clipboard and reads back, and the general pasteboard's
    /// change count doesn't move.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_check_never_touches_the_persons_pasteboard() {
        let _turn = TURN.lock().expect("no test panicked holding the turn");
        use crate::platform::mac::pasteboard::{check_change_count, general_change_count};
        let (general, own) = (general_change_count(), check_change_count());
        let copied = "a check's copy ✓\nsecond line";
        check::write(copied);
        assert_eq!(check::read().as_deref(), Some(copied));
        assert!(
            check_change_count() > own,
            "the copy reached the check's own pasteboard"
        );
        assert_eq!(
            general_change_count(),
            general,
            "the person's pasteboard was not written"
        );
    }

    /// A check's file copy lands on its own clipboard as files plus paths, one per line, and the
    /// general pasteboard is never written.
    #[test]
    fn a_checks_copied_files_are_read_back_as_files_and_paths() {
        let _turn = TURN.lock().expect("no test panicked holding the turn");
        #[cfg(target_os = "macos")]
        use crate::platform::mac::pasteboard::general_change_count;
        #[cfg(target_os = "macos")]
        let general = general_change_count();
        let folder = std::env::temp_dir().join("northroom clipboard ✓");
        let paths = vec![folder.join("a picture.png"), folder.join("Notes")];
        check::write_files(&paths);
        assert_eq!(check::read_files(), paths);
        assert_eq!(
            check::read().as_deref(),
            Some(format!("{}\n{}", paths[0].display(), paths[1].display()).as_str())
        );
        #[cfg(target_os = "macos")]
        {
            assert!(check::has_files());
            assert_eq!(general_change_count(), general);
        }
        check::write("words only");
        assert!(check::read_files().is_empty());
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn a_check_keeps_its_copies_in_the_process() {
        let _turn = TURN.lock().expect("no test panicked holding the turn");
        let copied = "a check's copy ✓\nsecond line";
        check::write(copied);
        assert_eq!(check::read().as_deref(), Some(copied));
    }
}
