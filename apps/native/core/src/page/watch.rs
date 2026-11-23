//! Live HTML pages from the drive. A page reloads when its file, or a file from its folder that
//! it read, changes on disk. Those folders are watched alongside the drive (`watch`), so any
//! change redraws the window, and the page compares file times with the ones from its last
//! load. A database a page queried (`page::host`) is different: its folder is watched too, but
//! the page is told of the change instead of reloading.

use crate::Relock;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// One open page: its file, the files from its folder it read, and their modified times at its
/// last load. The same file can be open twice while one view hands over to another (beside the
/// column and filling the tab), so each page has its own id.
struct Page {
    id: u64,
    file: PathBuf,
    read: Vec<PathBuf>,
    version: Option<SystemTime>,
}

impl Page {
    /// The latest change among the page's file and the files it read.
    fn newest(&self) -> Option<SystemTime> {
        std::iter::once(&self.file)
            .chain(&self.read)
            .filter_map(|file| {
                std::fs::metadata(file)
                    .and_then(|stat| stat.modified())
                    .ok()
            })
            .max()
    }
}

/// The folders of databases the open pages have queried, and whether one was added since the
/// watched folders were last synced.
#[derive(Default)]
struct Asked {
    folders: Vec<PathBuf>,
    joined: bool,
}

/// The open pages, the next page id, and the databases they queried.
#[derive(Default)]
pub struct Pages(Vec<Page>, u64, Asked);

impl Pages {
    /// Records a newly opened page of `file`, which has read nothing yet. Returns its id.
    pub fn open(&mut self, file: &Path) -> u64 {
        self.1 += 1;
        let mut page = Page {
            id: self.1,
            file: file.to_owned(),
            read: Vec::new(),
            version: None,
        };
        page.version = page.newest();
        self.0.push(page);
        self.1
    }

    /// The page `id` loaded, having read `served` (paths in its folder).
    pub fn loaded(&mut self, id: u64, served: &[String]) {
        let Some(page) = self.0.iter_mut().find(|page| page.id == id) else {
            return;
        };
        let folder = page.file.parent().unwrap_or(Path::new("")).to_owned();
        page.read = served.iter().map(|name| folder.join(name)).collect();
        page.version = page.newest();
    }

    /// Whether a file page `id` read changed since it loaded. Each change is reported once; the
    /// new times become the baseline for the next check.
    pub fn changed(&mut self, id: u64) -> bool {
        let Some(page) = self.0.iter_mut().find(|page| page.id == id) else {
            return false;
        };
        let now = page.newest();
        if now == page.version {
            return false;
        }
        page.version = now;
        true
    }

    /// Forgets page `id`. Closing the last page also drops the database folders.
    pub fn close(&mut self, id: u64) {
        self.0.retain(|page| page.id != id);
        if self.0.is_empty() {
            self.2 = Asked::default();
        }
    }

    /// Records that a page queried the database `file`, so its folder is watched while any page is
    /// open.
    pub fn asked(&mut self, file: &Path) {
        let Some(folder) = file.parent() else { return };
        if !self.2.folders.iter().any(|known| known == folder) {
            self.2.folders.push(folder.to_owned());
            self.2.joined = true;
        }
    }

    /// Whether a database folder was added since the last call.
    pub fn take_asked_joined(&mut self) -> bool {
        std::mem::take(&mut self.2.joined)
    }

    /// The folders of the open pages, the files they read and the databases they queried,
    /// without duplicates.
    pub fn folders(&self) -> Vec<PathBuf> {
        let mut folders: Vec<PathBuf> = Vec::new();
        let read = self
            .0
            .iter()
            .flat_map(|page| std::iter::once(&page.file).chain(&page.read))
            .filter_map(|file| file.parent());
        for folder in read.chain(self.2.folders.iter().map(PathBuf::as_path)) {
            if !folders.iter().any(|known| known == folder) {
                folders.push(folder.to_owned());
            }
        }
        folders
    }
}

static PAGES: Mutex<Pages> = Mutex::new(Pages(
    Vec::new(),
    0,
    Asked {
        folders: Vec::new(),
        joined: false,
    },
));

pub fn with<T>(action: impl FnOnce(&mut Pages) -> T) -> T {
    action(&mut PAGES.relock())
}

/// The folders the open pages read from, plus their databases' folders.
pub fn folders() -> Vec<PathBuf> {
    with(|pages| pages.folders())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(file: &Path, at: u64) {
        std::fs::File::options()
            .write(true)
            .open(file)
            .and_then(|handle| {
                handle.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(at))
            })
            .expect("set the time");
    }

    #[test]
    fn a_page_loads_again_when_it_or_a_file_it_read_changes_once_for_each_change() {
        let folder = tempfile::tempdir().expect("folder");
        let page = folder.path().join("index.html");
        let art = folder.path().join("assets/art.png");
        std::fs::create_dir_all(art.parent().unwrap()).unwrap();
        std::fs::write(&page, "<p>hi</p>").unwrap();
        std::fs::write(&art, "png").unwrap();
        touch(&page, 1_000);
        touch(&art, 1_000);
        let mut pages = Pages::default();
        let id = pages.open(&page);
        pages.loaded(id, &["assets/art.png".to_owned()]);
        assert!(!pages.changed(id));
        touch(&art, 2_000);
        assert!(pages.changed(id));
        assert!(!pages.changed(id), "told once");
        touch(&page, 3_000);
        assert!(pages.changed(id));
        assert_eq!(
            pages.folders(),
            [folder.path().to_owned(), folder.path().join("assets")]
        );
        // The same file opens again as the first view hands over; closing the first keeps it watched.
        let again = pages.open(&page);
        pages.close(id);
        assert!(!pages.changed(id));
        touch(&page, 4_000);
        assert!(pages.changed(again));
        pages.close(again);
        assert!(pages.folders().is_empty());
    }

    #[test]
    fn the_folder_of_a_database_a_page_asked_is_watched_until_the_last_page_closes() {
        let folder = tempfile::tempdir().expect("folder");
        let page = folder.path().join("index.html");
        std::fs::write(&page, "<p>hi</p>").unwrap();
        let mut pages = Pages::default();
        let id = pages.open(&page);
        assert!(!pages.take_asked_joined());
        pages.asked(&folder.path().join("data/app.sqlite"));
        pages.asked(&folder.path().join("data/other.sqlite"));
        pages.asked(&folder.path().join("app.sqlite"));
        assert!(pages.take_asked_joined());
        assert!(!pages.take_asked_joined(), "told once");
        assert_eq!(
            pages.folders(),
            [folder.path().to_owned(), folder.path().join("data")]
        );
        pages.close(id);
        assert!(pages.folders().is_empty());
    }

    #[test]
    fn a_file_it_did_not_read_is_none_of_its_business() {
        let folder = tempfile::tempdir().expect("folder");
        let page = folder.path().join("index.html");
        let other = folder.path().join("notes.txt");
        std::fs::write(&page, "<p>hi</p>").unwrap();
        std::fs::write(&other, "x").unwrap();
        touch(&page, 1_000);
        let mut pages = Pages::default();
        let id = pages.open(&page);
        pages.loaded(id, &[]);
        touch(&other, 5_000);
        assert!(!pages.changed(id));
    }
}
