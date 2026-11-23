//! Thumbnails for the list, columns and board cards: image and video pictures, system
//! thumbnails for files Northroom does not draw itself (`FileRead::shows_system_thumbnail`),
//! and terminal program marks. Pictures load off the UI thread (images one at a time, video
//! frames four, system thumbnails two) and the window redraws when one is ready (`wake`).
//!
//! A picture is kept only while on screen. One a frame did not draw is released with its atlas
//! slot once that frame paints (`release_undrawn`), and one scrolled past before loading is
//! never read.

use crate::core::{c, text};
use gpui::{App, RenderImage, Window};
use northroom_core::Relock;
use northroom_core::ffi::nr_entry_path;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};

enum Slot {
    Loading,
    Ready(Option<Arc<RenderImage>>),
}

#[derive(Default)]
struct Kept {
    slots: HashMap<String, Slot>,
    /// Pictures drawn so far in the frame being painted.
    drawn: HashSet<String>,
    /// Pictures the last painted frame drew, which are still worth reading.
    shown: HashSet<String>,
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

fn kept<T>(action: impl FnOnce(&mut Kept) -> T) -> T {
    let mut kept = KEPT.relock();
    action(kept.get_or_insert_with(Kept::default))
}

/// Where a file's picture is read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// The image file itself.
    Image,
    /// A video frame at this many milliseconds; 0 is the first frame.
    VideoFrame { at_millis: u64 },
    /// The system thumbnail (Quick Look, the Windows shell), when the core says the file has
    /// one.
    System,
}

impl Source {
    /// The source's name in a picture key. It has no colon, since colons separate key parts.
    fn tag(self) -> String {
        match self {
            Source::Image => "image".to_owned(),
            Source::VideoFrame { at_millis } => format!("video@{at_millis}"),
            Source::System => "system".to_owned(),
        }
    }

    /// The reader queue for this source.
    fn queue(self) -> &'static Queue {
        match self {
            Source::Image => &PICTURES,
            Source::VideoFrame { .. } => &VIDEOS,
            Source::System => &SYSTEM,
        }
    }
}

struct Job {
    key: String,
    path: String,
    pixels: u32,
    source: Source,
}

/// Jobs waiting for a reader, and the number of running readers.
struct Queue {
    jobs: Mutex<(VecDeque<Job>, usize)>,
    ready: Condvar,
    readers: usize,
}

static PICTURES: Queue = Queue::new(1);
static VIDEOS: Queue = Queue::new(4);
/// Two readers, so one slow document does not block the next. The system renders thumbnails
/// out of process.
static SYSTEM: Queue = Queue::new(2);

impl Queue {
    const fn new(readers: usize) -> Self {
        Self {
            jobs: Mutex::new((VecDeque::new(), 0)),
            ready: Condvar::new(),
            readers,
        }
    }

    fn push(&'static self, job: Job) {
        let mut jobs = self.jobs.relock();
        jobs.0.push_back(job);
        if jobs.1 < self.readers {
            jobs.1 += 1;
            std::thread::spawn(move || self.read());
        }
        self.ready.notify_one();
    }

    /// A reader thread; takes jobs for as long as the app runs.
    fn read(&self) {
        loop {
            let job = {
                let mut jobs = self.jobs.relock();
                loop {
                    if let Some(job) = jobs.0.pop_front() {
                        break job;
                    }
                    jobs = self
                        .ready
                        .wait(jobs)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            };
            // Scrolled away before its turn: skip it. It is requested again if it comes back.
            if !kept(|kept| kept.shown.contains(&job.key)) {
                kept(|kept| kept.slots.remove(&job.key));
                continue;
            }
            // Already in the window's colour space (`platform::Pixels`).
            let image = read(&job).and_then(|(width, height, bgra)| {
                image::ImageBuffer::from_raw(width, height, bgra)
                    .map(|buffer| Arc::new(RenderImage::new([image::Frame::new(buffer)])))
            });
            kept(|kept| kept.slots.insert(job.key, Slot::Ready(image)));
            crate::wake::wake();
        }
    }
}

/// Reads the job's picture as BGRA pixels through the platform.
fn read(job: &Job) -> Option<(u32, u32, Vec<u8>)> {
    let read_job = || match job.source {
        Source::Image if crate::svg_picture::is_svg(&job.path) => {
            crate::svg_picture::svg_picture(&job.path, job.pixels)
        }
        Source::Image => crate::platform::image_thumbnail(&job.path, job.pixels),
        Source::VideoFrame { at_millis } => {
            crate::platform::video_still(&job.path, at_millis as f64 / 1000.0, job.pixels)
        }
        Source::System => northroom_core::workspace::shows_system_thumbnail(Path::new(&job.path))
            .then(|| crate::platform::system_thumbnail(&job.path, job.pixels))
            .flatten(),
    };
    // Readers never exit, so drain the autorelease pool after each picture, or Image I/O,
    // AVFoundation and Quick Look objects pile up.
    #[cfg(target_os = "macos")]
    let read = objc2::rc::autoreleasepool(|_| read_job());
    #[cfg(not(target_os = "macos"))]
    let read = read_job();
    read.map(|pixels| (pixels.width, pixels.height, pixels.bgra))
}

/// File `id`'s picture at most `pixels` (a video's first frame when `video`). `None` while
/// loading or when it has none. Counts as drawn in the current frame.
pub fn picture(id: &str, pixels: u32, video: bool) -> Option<Arc<RenderImage>> {
    let source = if video {
        Source::VideoFrame { at_millis: 0 }
    } else {
        Source::Image
    };
    picture_from(id, pixels, source)
}

/// Whether image file `id` at `pixels` was read and could not be decoded.
pub fn picture_unreadable(id: &str, pixels: u32) -> bool {
    let key = format!("{id}:{pixels}:{}", Source::Image.tag());
    kept(|kept| matches!(kept.slots.get(&key), Some(Slot::Ready(None))))
}

/// Video file `id`'s frame at `seconds`, at most `pixels`, for skimming a card. `None` while
/// loading or when it has none. Counts as drawn in the current frame.
pub fn video_moment(id: &str, pixels: u32, seconds: f64) -> Option<Arc<RenderImage>> {
    let at_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    picture_from(id, pixels, Source::VideoFrame { at_millis })
}

/// The frame at `seconds` of the video at `path`, at most `pixels`, for a scrub bar preview.
/// `None` while loading or when it has none. Once read, the caller owns the frame, drawn or
/// not, and must return it with `let_go` to free its atlas slot.
pub fn video_moment_at(path: &str, pixels: u32, seconds: f64) -> Option<Arc<RenderImage>> {
    let source = Source::VideoFrame {
        at_millis: (seconds.max(0.0) * 1000.0).round() as u64,
    };
    let key = format!("{path}:{pixels}:{}", source.tag());
    let image = read_into_slot(key.clone(), || Some(path.to_owned()), pixels, source)?;
    kept(|kept| {
        kept.slots.remove(&key);
        kept.drawn.remove(&key);
        kept.shown.remove(&key);
    });
    Some(image)
}

/// Frames from `video_moment_at` returned by their owner, to drop from the atlas after the
/// frame paints.
static LET_GO: Mutex<Vec<Arc<RenderImage>>> = Mutex::new(Vec::new());

/// Returns frames from `video_moment_at`, freeing their atlas slots once the current frame
/// paints. Needs no window, so an owner can call it from `Drop`.
pub fn let_go(images: impl IntoIterator<Item = Arc<RenderImage>>) {
    LET_GO.relock().extend(images);
}

/// The picture file at `path` outside the drive (a 3D model's still,
/// `northroom_core::model_still`), at most `pixels`. `None` while loading or when unreadable.
/// Counts as drawn in the current frame.
pub fn picture_file(path: &str, pixels: u32) -> Option<Arc<RenderImage>> {
    read_into_slot(
        format!("{path}:{pixels}:{}", Source::Image.tag()),
        || Some(path.to_owned()),
        pixels,
        Source::Image,
    )
}

/// The system thumbnail Finder or Explorer shows for file `id`, at most `pixels`. `None` while
/// loading, when the core draws the file itself (`FileRead::shows_system_thumbnail`), or when
/// the system has only an icon. Counts as drawn in the current frame.
pub fn system_picture(id: &str, pixels: u32) -> Option<Arc<RenderImage>> {
    picture_from(id, pixels, Source::System)
}

/// File `id`'s picture from `source`, at most `pixels`. `None` while loading or when it has
/// none. Counts as drawn in the current frame.
fn picture_from(id: &str, pixels: u32, source: Source) -> Option<Arc<RenderImage>> {
    let key = format!("{id}:{pixels}:{}", source.tag());
    read_into_slot(
        key,
        || {
            let named = c(id);
            // SAFETY: a NUL-terminated id.
            let path = text(&unsafe { nr_entry_path(named.as_ptr()) });
            (!path.is_empty()).then_some(path)
        },
        pixels,
        source,
    )
}

/// The picture cached as `key`, queued from `path` on first request. `None` while loading or
/// when it has none.
fn read_into_slot(
    key: String,
    path: impl FnOnce() -> Option<String>,
    pixels: u32,
    source: Source,
) -> Option<Arc<RenderImage>> {
    let asked = kept(|kept| {
        let slot = match kept.slots.get(&key) {
            Some(Slot::Ready(image)) => Some(image.clone()),
            Some(Slot::Loading) => Some(None),
            None => None,
        };
        // Mark it wanted now, so a reader that reaches it before the frame ends still reads it.
        kept.shown.insert(key.clone());
        kept.drawn.insert(key.clone());
        slot
    });
    if let Some(slot) = asked {
        return slot;
    }
    let path = path()?;
    kept(|kept| kept.slots.insert(key.clone(), Slot::Loading));
    source.queue().push(Job {
        key,
        path,
        pixels,
        source,
    });
    None
}

/// The largest cached picture of file `id` at any size, for a drag leaving the window
/// (`chip::carry_out`).
pub fn kept_picture(id: &str) -> Option<Arc<RenderImage>> {
    kept(|kept| {
        kept.slots
            .iter()
            .filter_map(|(key, slot)| {
                // Keys are `<id>:<pixels>:<source>`, and ids may contain colons.
                let (owner, _) = key.rsplit_once(':')?.0.rsplit_once(':')?;
                match slot {
                    Slot::Ready(Some(image)) if owner == id => Some(image.clone()),
                    _ => None,
                }
            })
            .max_by_key(|image| image.size(0).width.0)
    })
}

/// Releases, with their atlas slots, every picture the painted frame did not draw and every
/// one returned with `let_go`. Call once per painted frame.
pub fn release_undrawn(window: &mut Window) {
    let gone: Vec<Arc<RenderImage>> = kept(|kept| {
        kept.shown = std::mem::take(&mut kept.drawn);
        let shown = &kept.shown;
        let mut gone = Vec::new();
        kept.slots.retain(|key, slot| match slot {
            Slot::Ready(image) if !shown.contains(key) => {
                gone.extend(image.take());
                false
            }
            _ => true,
        });
        gone
    });
    let given_back = std::mem::take(&mut *LET_GO.relock());
    for image in gone.into_iter().chain(given_back) {
        let _ = window.drop_image(image);
    }
}

/// How many pictures are loading.
pub fn reading() -> usize {
    kept(|kept| {
        kept.slots
            .values()
            .filter(|slot| matches!(slot, Slot::Loading))
            .count()
    })
}

/// Bundled marks for the terminal programs the core names (`NrFileRow::terminal_logo`), in
/// their own sRGB colours.
const LOGOS: [(&str, &[u8]); 4] = [
    (
        "antigravity",
        include_bytes!("../../assets/antigravity.svg"),
    ),
    (
        "claude-code",
        include_bytes!("../../assets/claude-code.svg"),
    ),
    ("codex", include_bytes!("../../assets/codex.svg")),
    ("cursor", include_bytes!("../../assets/cursor.svg")),
];

/// A logo's viewBox side, in its own units.
const LOGO_BOX: f32 = 24.0;
/// Logo render scales, in pixels per viewBox unit. 2 covers a row's 16 pt slot at 2×; a board
/// card takes the first scale that covers it. Few sizes keep zooming from caching one render
/// per width.
const LOGO_SCALES: [u32; 4] = [2, 4, 8, 16];

/// Rendered logos by program name and scale; `None` when the app has no mark for the name.
type DrawnLogos = HashMap<(String, u32), Option<Arc<RenderImage>>>;

thread_local! {
    static DRAWN_LOGOS: RefCell<DrawnLogos> = RefCell::new(HashMap::new());
}

/// The mark for terminal program `name` in a slot `pixels` wide. `None` for the plain shell
/// (empty name) or an unknown program.
pub fn logo(name: &str, pixels: f32, cx: &App) -> Option<Arc<RenderImage>> {
    if name.is_empty() {
        return None;
    }
    let scale = LOGO_SCALES
        .into_iter()
        .find(|scale| *scale as f32 * LOGO_BOX >= pixels)
        .unwrap_or(LOGO_SCALES[LOGO_SCALES.len() - 1]);
    let key = (name.to_owned(), scale);
    if let Some(drawn) = DRAWN_LOGOS.with(|logos| logos.borrow().get(&key).cloned()) {
        return drawn;
    }
    let drawn = LOGOS
        .iter()
        .find(|(logo, _)| *logo == name)
        .and_then(|(_, svg)| {
            cx.svg_renderer()
                .render_single_frame(svg, scale as f32)
                .ok()
        })
        .and_then(|drawn| {
            // Convert to the window's colour space (`crate::display`). The size is unchanged.
            let size = drawn.size(0);
            let bgra = drawn.as_bytes(0)?.to_vec();
            crate::display::image(size.width.0 as u32, size.height.0 as u32, bgra)
        });
    DRAWN_LOGOS.with(|logos| logos.borrow_mut().insert(key, drawn.clone()));
    drawn
}
