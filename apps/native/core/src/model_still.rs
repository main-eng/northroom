//! Still images of 3D models (`.glb`) for the drive's views: the picture shown on a model's
//! card, its row and its opened view. The shell renders each off screen (a three.js web view,
//! `model-still.html`) and hands the PNG here. It is cached under a name derived from the file's
//! path, size and modified time plus a digest of the rendering code (`DRAWING`), so a model
//! renders once until it changes, across launches too, and views load the still like any other
//! image, off the UI thread.
//!
//! Views ask on every paint, so asking is cheap: a model's file is re-checked at most once per
//! `RECHECK`. A still missing from disk becomes a want. The shell renders wants one at a time
//! (`next_wanted`), recently requested ones first, in request (paint) order, so scrolling a
//! folder renders what is on screen before what was scrolled past. A model that fails is not
//! retried until its file changes. The cache stays under `LIMIT_BYTES`, evicting the oldest
//! first.
//!
//! Also lays out an opened model's still in its view.

use crate::board::Frame;
use crate::diagnostics::{self, Level};
use crate::digest::sha256_hex;
use crate::Relock;

include!(concat!(env!("OUT_DIR"), "/drawing.rs"));
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A still's side in points, and the largest an opened model's still is shown.
pub const SIDE: f64 = 640.0;
/// Minimum margin between an opened model's still and its view's edges.
const VIEW_PADDING: f64 = 32.0;
/// Size of the model glyph an opened model shows while its still renders.
const GLYPH_SIDE: f64 = 64.0;
/// The folder name in the app's caches.
const FOLDER: &str = "Model stills";
/// Bump when a still's contents or naming change, so stale files are never read.
const FORMAT: &str = "model still 1";
/// Cache size limit for all stills together.
const LIMIT_BYTES: u64 = 256 * 1024 * 1024;
/// How often, at most, a model's file is re-checked for changes.
const RECHECK: Duration = Duration::from_secs(1);
/// A want not repeated within this long is for a model that has left the screen, so more recent
/// wants go first.
const STALE: Duration = Duration::from_secs(2);

/// A file's identity at one moment; each still belongs to one version.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
}

impl Version {
    pub fn new(path: &Path, size: u64, modified: Option<SystemTime>) -> Self {
        Self {
            path: path.to_path_buf(),
            size,
            modified,
        }
    }

    /// The current version of the file at `path`; `None` when it cannot be read.
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        metadata
            .is_file()
            .then(|| Self::new(path, metadata.len(), metadata.modified().ok()))
    }

    /// The still's file name: a digest of what it was rendered from.
    fn file_name(&self) -> String {
        let modified_nanos = self
            .modified
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        let key = format!(
            "{FORMAT}\n{DRAWING}\n{}\n{}\n{modified_nanos}",
            self.path.to_string_lossy(),
            self.size
        );
        format!("{}.png", &sha256_hex(key.as_bytes())[..32])
    }
}

/// What is known about one model file.
struct Model {
    /// The file at the last check; `None` when it could not be read.
    version: Option<Version>,
    checked: Instant,
    /// Its still is on disk.
    on_disk: bool,
}

/// A still the shell should render.
struct Want {
    version: Version,
    /// Its position in first-request order.
    asked: u64,
    /// When it was last requested.
    last: Instant,
    /// The shell has taken it.
    taken: bool,
}

impl Want {
    /// Which of two wants the shell takes first: a recently requested one, then the older request.
    fn before(&self, other: &Self, now: Instant) -> std::cmp::Ordering {
        let fresh = |want: &Self| now.duration_since(want.last) < STALE;
        fresh(self)
            .cmp(&fresh(other))
            .then(other.asked.cmp(&self.asked))
    }
}

/// The known stills and the wanted ones.
pub struct Stills {
    /// `<caches>/Model stills`.
    folder: PathBuf,
    models: HashMap<PathBuf, Model>,
    wants: Vec<Want>,
    /// How many wants were ever made, which orders the next one.
    asks: u64,
    /// Versions that failed to render.
    failed: HashSet<Version>,
    /// A still changed since `take_changed`.
    changed: bool,
    /// The shell has work: a new want, or a change.
    poke: bool,
}

impl Stills {
    /// Stills kept under `caches`.
    pub fn new(caches: &Path) -> Self {
        Self {
            folder: caches.join(FOLDER),
            models: HashMap::new(),
            wants: Vec::new(),
            asks: 0,
            failed: HashSet::new(),
            changed: false,
            poke: false,
        }
    }

    fn file_of(&self, version: &Version) -> PathBuf {
        self.folder.join(version.file_name())
    }

    /// The model at `path`, re-checked if `RECHECK` has passed since the last check.
    fn model(&mut self, path: &Path, now: Instant) -> &Model {
        let due = self
            .models
            .get(path)
            .is_none_or(|model| now.duration_since(model.checked) >= RECHECK);
        if due {
            let version = Version::of(path);
            // Drop wants for the file's old version.
            self.wants.retain(|want| {
                want.version.path != path || Some(&want.version) == version.as_ref()
            });
            let on_disk = version
                .as_ref()
                .is_some_and(|version| self.file_of(version).is_file());
            let model = Model {
                version,
                checked: now,
                on_disk,
            };
            self.models.insert(path.to_path_buf(), model);
        }
        &self.models[path]
    }

    /// The PNG still for the current version of the model at `path`. `None` while it renders, when
    /// it fails, and when the file cannot be read. A still not on disk becomes a want.
    pub fn picture(&mut self, path: &Path, now: Instant) -> Option<PathBuf> {
        let model = self.model(path, now);
        let (version, on_disk) = (model.version.clone()?, model.on_disk);
        if on_disk {
            return Some(self.file_of(&version));
        }
        if !self.failed.contains(&version) {
            self.want(version, now);
        }
        None
    }

    fn want(&mut self, version: Version, now: Instant) {
        if let Some(want) = self.wants.iter_mut().find(|want| want.version == version) {
            want.last = now;
            return;
        }
        self.asks += 1;
        self.wants.push(Want {
            version,
            asked: self.asks,
            last: now,
            taken: false,
        });
        self.poke = true;
    }

    /// The model the shell renders next (`Want::before`). The shell owns it until `drawn` or
    /// `failed`.
    pub fn next_wanted(&mut self, now: Instant) -> Option<Version> {
        let want = self
            .wants
            .iter_mut()
            .filter(|want| !want.taken)
            .max_by(|one, other| one.before(other, now))?;
        want.taken = true;
        Some(want.version.clone())
    }

    /// Stores `png` as the still of `version`, then evicts the oldest stills while the cache is
    /// over `LIMIT_BYTES`.
    pub fn drawn(&mut self, version: &Version, png: &[u8]) -> std::io::Result<()> {
        self.wants.retain(|want| want.version != *version);
        let file = self.file_of(version);
        std::fs::create_dir_all(&self.folder)?;
        let written = file.with_extension("png.part");
        std::fs::write(&written, png)?;
        std::fs::rename(&written, &file)?;
        trim(&self.folder, LIMIT_BYTES);
        if let Some(model) = self
            .models
            .get_mut(&version.path)
            .filter(|model| model.version.as_ref() == Some(version))
        {
            model.on_disk = true;
        }
        self.changed = true;
        self.poke = true;
        Ok(())
    }

    /// Marks `version` as failed, so it is not retried until the file changes.
    pub fn failed(&mut self, version: &Version) {
        self.wants.retain(|want| want.version != *version);
        self.failed.insert(version.clone());
    }

    /// Whether a still changed since the last call.
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    fn take_poke(&mut self) -> bool {
        std::mem::take(&mut self.poke)
    }
}

/// Deletes the oldest stills in `folder` while their total exceeds `limit_bytes`.
fn trim(folder: &Path, limit_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            Some((modified, metadata.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, bytes, _)| bytes).sum();
    files.sort();
    for (_, bytes, path) in files {
        if total <= limit_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= bytes;
        }
    }
}

struct Shared {
    stills: Mutex<Stills>,
    poke: fn(),
}

static SHARED: OnceLock<Shared> = OnceLock::new();

/// Runs `action` on the app's stills, or returns `None` before `start`. Pokes the shell
/// afterwards if there is work for it.
fn with<T>(action: impl FnOnce(&mut Stills) -> T) -> Option<T> {
    let shared = SHARED.get()?;
    let (result, poke) = {
        let mut stills = shared.stills.relock();
        let result = action(&mut stills);
        (result, stills.take_poke())
    };
    if poke {
        (shared.poke)();
    }
    Some(result)
}

/// The app's caches folder for stills; `None` under test.
fn caches() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(crate::paths::get().caches)
}

/// Starts the still cache for a shell. `poke` is called from any thread when there is work: a
/// new want, or a changed still (`take_changed`). Returns false under test, where nothing is
/// cached.
pub fn start(poke: fn()) -> bool {
    let Some(caches) = caches() else {
        return false;
    };
    SHARED
        .set(Shared {
            stills: Mutex::new(Stills::new(&caches)),
            poke,
        })
        .is_ok()
}

/// The PNG still of the model at `path` (`Stills::picture`): of the file itself when it is a
/// `.glb`, else of the `.glb` it converts to (`mesh::glb_for`). Cheap enough to call on every
/// paint once converted.
pub fn picture(path: &Path) -> Option<PathBuf> {
    let model = crate::mesh::glb_for(path)?;
    with(|stills| stills.picture(&model, Instant::now())).flatten()
}

/// The model the shell renders next (`Stills::next_wanted`).
pub fn next_wanted() -> Option<Version> {
    with(|stills| stills.next_wanted(Instant::now())).flatten()
}

/// The shell rendered `version` as `png` (`Stills::drawn`). A write failure is logged and the
/// model is not retried until the file changes.
pub fn drawn(version: &Version, png: &[u8]) {
    let written = with(|stills| {
        let written = stills.drawn(version, png);
        if written.is_err() {
            stills.failed(version);
        }
        written
    });
    if let Some(Err(error)) = written {
        log_failure(version, &format!("the still could not be written: {error}"));
    }
}

/// The shell could not render `version`, for reason `why`. It is logged, and the model has no
/// still until its file changes.
pub fn failed(version: &Version, why: &str) {
    log_failure(version, why);
    with(|stills| stills.failed(version));
}

fn log_failure(version: &Version, why: &str) {
    diagnostics::write_with(
        Level::Warn,
        "a model's still could not be drawn",
        &serde_json::json!({ "model": version.path.to_string_lossy(), "why": why }),
    );
}

/// Whether a still changed since the last call, so the views should redraw.
pub fn take_changed() -> bool {
    with(Stills::take_changed).unwrap_or(false)
}

/// Layout for an opened model's `width` × `height` view: the still on studio grey at full size,
/// shrunk only to keep `VIEW_PADDING`, centred on whole points (`None` when there is no room);
/// and the model glyph, centred, shown while the still renders or when it fails.
pub fn view_layout(width: f64, height: f64) -> (Option<Frame>, Frame) {
    let (mid_x, mid_y) = (width / 2.0, height / 2.0);
    let side = SIDE
        .min(width - VIEW_PADDING * 2.0)
        .min(height - VIEW_PADDING * 2.0);
    let still = (side > 0.0).then(|| Frame {
        x: (mid_x - side / 2.0).round(),
        y: (mid_y - side / 2.0).round(),
        width: side,
        height: side,
    });
    let glyph = Frame {
        x: mid_x - GLYPH_SIDE / 2.0,
        y: mid_y - GLYPH_SIDE / 2.0,
        width: GLYPH_SIDE,
        height: GLYPH_SIDE,
    };
    (still, glyph)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model file at `folder/name` holding `bytes`.
    fn model(folder: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = folder.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_still_is_wanted_once_drawn_and_then_found_on_disk_by_a_new_run() {
        let folder = tempfile::tempdir().unwrap();
        let chair = model(folder.path(), "Chair.glb", b"glTF one");
        let caches = folder.path().join("caches");
        let mut stills = Stills::new(&caches);
        let start = Instant::now();

        assert_eq!(stills.picture(&chair, start), None);
        assert!(stills.take_poke(), "the shell is told");
        assert_eq!(stills.picture(&chair, start), None);
        let version = stills.next_wanted(start).expect("wanted");
        assert_eq!(version.path, chair);
        assert_eq!(
            stills.next_wanted(start),
            None,
            "wanted once, however often asked"
        );

        stills.drawn(&version, b"png").unwrap();
        assert!(stills.take_changed());
        let file = stills.picture(&chair, start).expect("on disk");
        assert_eq!(std::fs::read(&file).unwrap(), b"png");

        let mut again = Stills::new(&caches);
        assert_eq!(again.picture(&chair, start), Some(file), "kept across runs");
        assert_eq!(again.next_wanted(start), None);
    }

    #[test]
    fn a_changed_file_is_drawn_again_once_it_is_looked_at_again() {
        let folder = tempfile::tempdir().unwrap();
        let chair = model(folder.path(), "Chair.glb", b"glTF one");
        let mut stills = Stills::new(&folder.path().join("caches"));
        let start = Instant::now();
        stills.picture(&chair, start);
        let version = stills.next_wanted(start).unwrap();
        stills.drawn(&version, b"png").unwrap();

        std::fs::write(&chair, b"glTF changed").unwrap();
        assert!(
            stills.picture(&chair, start).is_some(),
            "not looked at again yet"
        );
        let later = start + RECHECK;
        assert_eq!(stills.picture(&chair, later), None);
        let next = stills
            .next_wanted(later)
            .expect("the new version is wanted");
        assert_ne!(next, version);
        assert_eq!(stills.next_wanted(later), None, "the old one no longer");
    }

    #[test]
    fn a_failed_still_is_drawn_again_only_once_the_file_changes() {
        let folder = tempfile::tempdir().unwrap();
        let chair = model(folder.path(), "Chair.glb", b"glTF one");
        let mut stills = Stills::new(&folder.path().join("caches"));
        let start = Instant::now();
        stills.picture(&chair, start);
        let version = stills.next_wanted(start).unwrap();
        stills.failed(&version);

        let later = start + RECHECK;
        assert_eq!(stills.picture(&chair, later), None);
        assert_eq!(stills.next_wanted(later), None);

        std::fs::write(&chair, b"glTF fixed").unwrap();
        let after = later + RECHECK;
        stills.picture(&chair, after);
        assert!(stills.next_wanted(after).is_some());
    }

    #[test]
    fn models_still_on_screen_are_drawn_first_in_the_order_they_were_painted() {
        let folder = tempfile::tempdir().unwrap();
        let [first, second, third] =
            ["A.glb", "B.glb", "C.glb"].map(|name| model(folder.path(), name, name.as_bytes()));
        let mut stills = Stills::new(&folder.path().join("caches"));
        let start = Instant::now();
        for path in [&first, &second] {
            stills.picture(path, start);
        }
        // Scrolled further: the first two are off screen and the third is painted.
        let later = start + STALE;
        stills.picture(&third, later);
        let order: Vec<PathBuf> = std::iter::from_fn(|| stills.next_wanted(later))
            .map(|version| version.path)
            .collect();
        assert_eq!(order, [third, first, second]);
    }

    #[test]
    fn a_file_that_cannot_be_read_has_no_still_and_wants_none() {
        let folder = tempfile::tempdir().unwrap();
        let mut stills = Stills::new(&folder.path().join("caches"));
        let start = Instant::now();
        assert_eq!(stills.picture(&folder.path().join("Gone.glb"), start), None);
        assert_eq!(stills.picture(folder.path(), start), None, "a folder");
        assert_eq!(stills.next_wanted(start), None);
    }

    #[test]
    fn the_oldest_stills_are_let_go_past_the_limit() {
        let folder = tempfile::tempdir().unwrap();
        for (name, seconds) in [("old.png", 1), ("mid.png", 2), ("new.png", 3)] {
            let path = folder.path().join(name);
            std::fs::write(&path, [0; 10]).unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
                .unwrap();
        }
        trim(folder.path(), 20);
        let mut left: Vec<String> = std::fs::read_dir(folder.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["mid.png", "new.png"]);
    }

    #[test]
    fn an_opened_still_is_its_own_size_until_the_view_is_too_small() {
        let (still, glyph) = view_layout(1001.0, 900.0);
        assert_eq!(
            still,
            Some(Frame {
                x: 181.0,
                y: 130.0,
                width: 640.0,
                height: 640.0
            })
        );
        assert_eq!(
            glyph,
            Frame {
                x: 468.5,
                y: 418.0,
                width: 64.0,
                height: 64.0
            }
        );
        let (still, _) = view_layout(400.0, 300.0);
        assert_eq!(
            still.map(|frame| (frame.x, frame.width)),
            Some((82.0, 236.0))
        );
        assert_eq!(view_layout(60.0, 300.0).0, None);
    }
}
