//! The boundary for the open file drawn as pages (ADR-0001): the deck it reads as, its pages
//! placed for a view, and its pictures as files a shell can load.

use super::layout::{self, Mark, Page};
use super::{Deck, Kind, ReadError, Source};
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
/// at most its kind's largest size. A document package is a folder, so its size is not checked.
fn opened_path() -> Option<(PathBuf, Kind)> {
    let (path, kind) = with_columns(|columns| {
        let opened = columns.opened()?.clone();
        let kind = Kind::of_name(&opened.title).filter(|_| opened.kind == EntryKind::File)?;
        let path = columns.workspace().path_of(&opened.id).ok()?;
        kind.claims(&path).then_some((path, kind))
    })
    .flatten()?;
    let stat = std::fs::metadata(&path).ok()?;
    (stat.is_dir() || stat.len() <= kind.max_bytes()).then_some((path, kind))
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
    /// The file is encrypted and no password given opens it.
    locked: bool,
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
    let result = super::read(path, kind, &mut crate::text::typeset::width);
    let locked = matches!(result, Err(ReadError::Locked));
    let deck = match result {
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
        locked,
    });
    deck
}

/// The open file's path when it is locked: encrypted, and no password given opens it.
pub fn opened_locked() -> Option<PathBuf> {
    let (path, kind) = opened_path()?;
    read(&path, kind);
    let read = READ.relock();
    let known = read.as_ref()?;
    (known.path == path && known.locked).then_some(path)
}

/// The text of the open PDF's pages read so far: its path and version, and each page's glyphs.
struct Texts {
    path: PathBuf,
    version: Version,
    pages: std::collections::HashMap<usize, Arc<Vec<crate::pdf::TextGlyph>>>,
}

static TEXT: Mutex<Option<Texts>> = Mutex::new(None);

/// Page `slide`'s glyphs in the open PDF, read once a version; empty for other files.
fn page_glyphs(slide: usize) -> Arc<Vec<crate::pdf::TextGlyph>> {
    let Some((path, Kind::Pdf)) = opened_path() else {
        return Arc::default();
    };
    let version = version_of(&path);
    let mut text = TEXT.relock();
    if text
        .as_ref()
        .is_none_or(|known| known.path != path || known.version != version)
    {
        *text = Some(Texts {
            path: path.clone(),
            version,
            pages: std::collections::HashMap::new(),
        });
    }
    let pages = &mut text.as_mut().expect("just set").pages;
    pages
        .entry(slide)
        .or_insert_with(|| {
            let read = super::read_bytes(&path).ok().and_then(|bytes| {
                crate::pdf::page_text(bytes, crate::pdf::passwords::of(&path).as_deref(), slide)
                    .ok()
            });
            Arc::new(read.unwrap_or_default())
        })
        .clone()
}

/// The text of slide `slide` of the open deck, placed in a view of it at `frame`, as the
/// view's marks are (`layout::marks`). Only a PDF's pages have text to select.
pub fn deck_text(slide: usize, frame: Rect) -> Vec<super::text::Glyph> {
    let Some(deck) = opened_deck() else {
        return Vec::new();
    };
    let zoom = frame.width / deck.width;
    page_glyphs(slide)
        .iter()
        .map(|glyph| super::text::Glyph {
            text: glyph.text.clone(),
            frame: Rect::new(
                frame.x + glyph.frame.x * zoom,
                frame.y + glyph.frame.y * zoom,
                glyph.frame.width * zoom,
                glyph.frame.height * zoom,
            ),
        })
        .collect()
}

/// The find field's placeholder, in the interface language.
pub fn find_placeholder() -> &'static str {
    crate::strings::text(
        crate::ffi::interface_language(),
        crate::strings::key::DECK_FIND,
    )
}

/// Where `query` occurs in the open PDF's text, page by page: each match's page and its first
/// and last glyph (`text::find`).
pub fn deck_find(query: &str) -> Vec<(usize, usize, usize)> {
    let Some(deck) = opened_deck() else {
        return Vec::new();
    };
    (0..deck.slides.len())
        .flat_map(|slide| {
            let glyphs: Vec<super::text::Glyph> = page_glyphs(slide)
                .iter()
                .map(|glyph| super::text::Glyph {
                    text: glyph.text.clone(),
                    frame: glyph.frame,
                })
                .collect();
            super::text::find(&glyphs, query)
                .into_iter()
                .map(move |(first, last)| (slide, first, last))
        })
        .collect()
}

/// Forgets the file last read, so the next ask reads it again, as after a password is typed.
pub fn forget_read() {
    *READ.relock() = None;
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

/// Reads a picture's bytes and writes them as `file`, through a temporary name so a view never
/// loads half a picture.
fn write_picture(
    path: &Path,
    source: &Source,
    folder: &Path,
    file: &Path,
    prefix: &str,
) -> Result<(), String> {
    let bytes = source.read(path).map_err(|failure| failure.to_string())?;
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    forget_others(folder, prefix);
    let writing = file.with_extension("writing");
    std::fs::write(&writing, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&writing, file).map_err(|error| error.to_string())
}

/// A picture to write off the UI thread.
struct Job {
    path: PathBuf,
    source: Source,
    folder: PathBuf,
    file: PathBuf,
    prefix: String,
}

/// Pictures waiting to be written, those being written, and those that failed.
#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    busy: std::collections::HashSet<PathBuf>,
    failed: std::collections::HashSet<PathBuf>,
}

static QUEUE: Mutex<Option<Queue>> = Mutex::new(None);
static WAITING: std::sync::Condvar = std::sync::Condvar::new();
static WAKE: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();
static WORKER: std::sync::Once = std::sync::Once::new();

/// Lets the core ask the window to redraw when a picture written off the UI thread is ready.
/// The shell calls it once at launch.
pub fn start_rendering(wake: fn()) {
    let _ = WAKE.set(wake);
}

/// Whether pictures are waiting to be written or being written.
pub fn drawing() -> bool {
    QUEUE
        .relock()
        .as_ref()
        .is_some_and(|queue| !queue.busy.is_empty())
}

/// Writes queued pictures, newest asked first, so the page in view comes before pages scrolled
/// past.
fn work() {
    loop {
        let job = {
            let mut queue = QUEUE.relock();
            loop {
                if let Some(job) = queue.get_or_insert_with(Queue::default).jobs.pop() {
                    break job;
                }
                queue = WAITING
                    .wait(queue)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let written = write_picture(&job.path, &job.source, &job.folder, &job.file, &job.prefix);
        {
            let mut queue = QUEUE.relock();
            let queue = queue.get_or_insert_with(Queue::default);
            queue.busy.remove(&job.file);
            if let Err(why) = written {
                eprintln!("a picture of {} was not drawn: {why}", job.path.display());
                queue.failed.insert(job.file);
            }
        }
        if let Some(wake) = WAKE.get() {
            wake();
        }
    }
}

/// Asks for a slow picture to be written; it shows once the window redraws.
fn queue_picture(job: Job) {
    let mut queue = QUEUE.relock();
    let queue = queue.get_or_insert_with(Queue::default);
    if queue.busy.contains(&job.file) || queue.failed.contains(&job.file) {
        return;
    }
    queue.busy.insert(job.file.clone());
    queue.jobs.push(job);
    WORKER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("deck-pictures".into())
            .spawn(work);
    });
    WAITING.notify_one();
}

/// Picture `index` of the deck read from `path`, as a file in `folder`: written on first ask,
/// off the UI thread when it is slow to make (a PDF page's layer).
fn keep_picture(path: &Path, deck: &Deck, index: usize, folder: &Path) -> Option<PathBuf> {
    let source = deck.pictures.get(index)?;
    let kept = kept_name(path, version_of(path), index, source);
    let file = folder.join(&kept);
    if file.is_file() {
        return Some(file);
    }
    let prefix = kept[..17].to_owned();
    if source.is_slow() && !cfg!(test) {
        queue_picture(Job {
            path: path.to_path_buf(),
            source: source.clone(),
            folder: folder.to_path_buf(),
            file,
            prefix,
        });
        return None;
    }
    match write_picture(path, source, folder, &file, &prefix) {
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
