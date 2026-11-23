//! Watches the folders the open columns list, so changes from anything (Finder, an agent in a
//! terminal, another app) show up without a refresh.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};

pub struct FolderWatch {
    watcher: RecommendedWatcher,
    watched: Vec<PathBuf>,
}

impl FolderWatch {
    /// Calls `changed` on the watcher's thread whenever a watched folder's contents change.
    /// Opens and reads are ignored: Linux reports every open of a watched folder, and the
    /// columns' own relisting after a change would loop.
    pub fn start(changed: impl Fn() + Send + 'static) -> notify::Result<Self> {
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|event| !matches!(event.kind, notify::EventKind::Access(_))) {
                changed();
            }
        })?;
        Ok(Self {
            watcher,
            watched: Vec::new(),
        })
    }

    /// Watches exactly `folders`, non-recursively.
    pub fn watch(&mut self, folders: &[PathBuf]) {
        for gone in self.watched.iter().filter(|path| !folders.contains(path)) {
            let _ = self.watcher.unwatch(gone);
        }
        for added in folders.iter().filter(|path| !self.watched.contains(path)) {
            let _ = self
                .watcher
                .watch(Path::new(added), RecursiveMode::NonRecursive);
        }
        self.watched = folders.to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::FolderWatch;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn listing_and_reading_a_watched_folder_is_no_change_and_a_new_file_is() {
        let folder = tempfile::tempdir().expect("temp dir");
        let root = folder.path().canonicalize().expect("real path");
        std::fs::write(root.join("picture.png"), "pixels").expect("write");
        let told = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&told);
        let mut watch = FolderWatch::start(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        })
        .expect("watch");
        watch.watch(std::slice::from_ref(&root));
        // FSEvents may still report the write from just before the watch began, so only later
        // events count.
        std::thread::sleep(Duration::from_secs(1));
        told.store(0, Ordering::SeqCst);
        // Mimic the columns after a change: list the folder and read its files.
        for _ in 0..3 {
            assert_eq!(std::fs::read_dir(&root).expect("list").count(), 1);
            assert_eq!(
                std::fs::read(root.join("picture.png")).expect("read"),
                b"pixels"
            );
        }
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            told.load(Ordering::SeqCst),
            0,
            "a listing or a read was told as a change"
        );
        std::fs::write(root.join("new.txt"), "new").expect("write");
        let deadline = Instant::now() + Duration::from_secs(20);
        while told.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            told.load(Ordering::SeqCst) > 0,
            "a file made in the folder was not told"
        );
    }
}
