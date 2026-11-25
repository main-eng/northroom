//! The boundary for the open file drawn as pages (ADR-0001): the deck it reads as, its pages
//! placed for a view, and its pictures as files a shell can load.

use super::layout::{self, Mark, Page};
use super::{Deck, Kind, Source, FILE_MAX_BYTES};
use crate::ffi::with_columns;
use crate::geometry::Rect;
use crate::workspace::EntryKind;
use crate::Relock;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// The folder in the app's caches that holds the open deck's pictures.
const PICTURES_FOLDER: &str = "deck-pictures";

/// The open file's path and kind when it is drawn as pages: a kind by name (`Kind::of_name`), of
/// at most `FILE_MAX_BYTES`. A document package is a folder, so only its size is not checked.
fn opened_path() -> Option<(PathBuf, Kind)> {
    let (path, kind) = with_columns(|columns| {
        let opened = columns.opened()?.clone();
        let kind = Kind::of_name(&opened.title).filter(|_| opened.kind == EntryKind::File)?;
        Some((columns.workspace().path_of(&opened.id).ok()?, kind))
    })
    .flatten()?;
    let stat = std::fs::metadata(&path).ok()?;
    (stat.is_dir() || stat.len() <= FILE_MAX_BYTES).then_some((path, kind))
}

/// A version of a file: its size and when it changed.
type Version = (u64, Option<SystemTime>);

fn version_of(path: &Path) -> Version {
    let stat = std::fs::metadata(path).ok();
    (
        stat.as_ref().map_or(0, std::fs::Metadata::len),
        stat.and_then(|stat| stat.modified().ok()),
    )
}

/// A presentation read: its path and version, and the deck it is, or `None` when it could not
/// be read.
struct Read {
    path: PathBuf,
    version: Version,
    deck: Option<Arc<Deck>>,
}

/// The presentation last read, so a view asking on every change of the window reads the file
/// once per version.
static READ: Mutex<Option<Read>> = Mutex::new(None);

fn read(path: &Path, kind: Kind) -> Option<Arc<Deck>> {
    let version = version_of(path);
    let mut read = READ.relock();
    if let Some(known) = read.as_ref() {
        if known.path == path && known.version == version {
            return known.deck.clone();
        }
    }
    let deck = match super::read(path, kind, &mut crate::text::typeset::width) {
        Ok(deck) => Some(Arc::new(deck)),
        Err(failure) => {
            eprintln!("the file was not read as pages: {failure}");
            None
        }
    };
    *read = Some(Read {
        path: path.to_path_buf(),
        version,
        deck: deck.clone(),
    });
    deck
}

/// The open file as a deck; `None` when it is not a presentation or cannot be read, and it
/// shows as its details.
pub fn opened_deck() -> Option<Arc<Deck>> {
    let (path, kind) = opened_path()?;
    read(&path, kind)
}

/// Where each slide of the open deck sits in a view `width` × `height` (`layout::page`).
pub fn deck_page(width: f64, height: f64) -> Option<Page> {
    opened_deck().map(|deck| layout::page(&deck, width, height))
}

/// What the view draws for slide `slide` of the open deck at `frame` (`layout::marks`).
pub fn deck_marks(slide: usize, frame: Rect) -> Vec<Mark> {
    opened_deck().map_or_else(Vec::new, |deck| {
        layout::marks(&deck, slide, frame, &mut crate::text::typeset::width)
    })
}

/// The file name a picture is kept under: the presentation's path and version, the picture's
/// place in the deck, and its own extension, which says how to decode it.
fn kept_name(path: &Path, version: Version, index: usize, source: &Source) -> String {
    let modified = version
        .1
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    let of = format!("{}\n{}\n{modified}", path.display(), version.0);
    let digest = crate::digest::sha256_hex(of.as_bytes());
    format!("{}-{index}.{}", &digest[..16], source.extension())
}

/// Removes the pictures kept for another file or an earlier version of this one, so
/// the folder holds one deck's pictures.
fn forget_others(folder: &Path, keep_prefix: &str) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(keep_prefix) {
            // A file that stays is only wasted space until the next deck opens.
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Picture `index` of the deck read from `path`, as a file in `folder`: written on first ask.
fn keep_picture(path: &Path, deck: &Deck, index: usize, folder: &Path) -> Option<PathBuf> {
    let source = deck.pictures.get(index)?;
    let kept = kept_name(path, version_of(path), index, source);
    let file = folder.join(&kept);
    if file.is_file() {
        return Some(file);
    }
    let written = source
        .read(path)
        .map_err(|failure| failure.to_string())
        .and_then(|bytes| {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
            forget_others(folder, &kept[..17]);
            std::fs::write(&file, bytes).map_err(|error| error.to_string())
        });
    match written {
        Ok(()) => Some(file),
        Err(why) => {
            eprintln!("picture {index} of {} was not kept: {why}", path.display());
            None
        }
    }
}

/// Picture `index` of the open deck as a file a shell loads like any picture, in the app's
/// caches. `None` when there is no such picture or it cannot be written.
pub fn deck_picture(index: usize) -> Option<PathBuf> {
    let (path, kind) = opened_path()?;
    let deck = read(&path, kind)?;
    let folder = crate::paths::get().caches.join(PICTURES_FOLDER);
    keep_picture(&path, &deck, index, &folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_written_once_and_another_decks_pictures_are_removed() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deck/Pitch deck.pptx");
        let deck = super::super::pptx::read(&fixture).expect("read");
        let folder = tempfile::tempdir().expect("a folder");
        std::fs::write(folder.path().join("0000000000000000-0.png"), b"old").expect("write");

        let first = keep_picture(&fixture, &deck, 0, folder.path()).expect("kept");
        assert_eq!(first.extension().and_then(|e| e.to_str()), Some("jpeg"));
        assert!(std::fs::read(&first)
            .expect("bytes")
            .starts_with(&[0xff, 0xd8]));
        let third = keep_picture(&fixture, &deck, 2, folder.path()).expect("kept");
        assert_eq!(keep_picture(&fixture, &deck, 0, folder.path()), Some(first));
        let mut kept: Vec<PathBuf> = std::fs::read_dir(folder.path())
            .expect("the folder")
            .map(|entry| entry.expect("an entry").path())
            .collect();
        kept.sort();
        let mut expected = vec![
            keep_picture(&fixture, &deck, 0, folder.path()).unwrap(),
            third,
        ];
        expected.sort();
        assert_eq!(kept, expected);
        assert_eq!(keep_picture(&fixture, &deck, 9, folder.path()), None);
    }
}
