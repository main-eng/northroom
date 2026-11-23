//! The page host's URL scheme off the Mac, mirroring `mac/web.rs`. The core decides every answer.
//! Files are read and pages rewritten on a dedicated reader thread, and each reply goes back on
//! the main thread to the engine's pending request closure.

use crate::core::{HostCore, HostLog, Reply};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::{Arc, LazyLock, Mutex, mpsc};

/// Delivers a request's reply on the main thread.
pub type Answer = Box<dyn FnOnce(Reply)>;

/// A pending request: its reply callback and its address.
type Reading = (Answer, String);

pub struct Files {
    pub core: Arc<HostCore>,
    pub log: HostLog,
    reading: RefCell<HashMap<u64, Reading>>,
    next: Cell<u64>,
    id: u64,
}

thread_local! {
    /// Live `Files` by id, so a finished read can find its owner.
    static FILES: RefCell<HashMap<u64, Weak<Files>>> = RefCell::new(HashMap::new());
    static FILES_MADE: Cell<u64> = const { Cell::new(0) };
}

struct Read {
    core: Arc<HostCore>,
    address: String,
    range: String,
    files: u64,
    task: u64,
}

/// The single thread that reads files, in request order.
static READER: LazyLock<Mutex<mpsc::Sender<Read>>> = LazyLock::new(|| {
    let (sender, reads) = mpsc::channel::<Read>();
    std::thread::Builder::new()
        .name("northroom-scene-files".into())
        .spawn(move || {
            for read in reads {
                let reply = read.core.answer(&read.address, &read.range);
                let (files, task) = (read.files, read.task);
                crate::run::post_main(move || read_back(files, task, reply));
            }
        })
        .expect("the reading thread starts");
    Mutex::new(sender)
});

fn read_back(files: u64, task: u64, reply: Reply) {
    if let Some(files) = FILES.with(|all| all.borrow().get(&files).and_then(Weak::upgrade)) {
        files.answer(task, reply);
    }
}

impl Files {
    pub fn new(core: Arc<HostCore>) -> Rc<Self> {
        // The reading thread's answers come back through the main thread's queue.
        crate::run::ready();
        let id = FILES_MADE.with(|made| {
            made.set(made.get() + 1);
            made.get()
        });
        let files = Rc::new(Self {
            log: HostLog::new(core.clone()),
            core,
            reading: RefCell::new(HashMap::new()),
            next: Cell::new(0),
            id,
        });
        FILES.with(|all| all.borrow_mut().insert(id, Rc::downgrade(&files)));
        files
    }

    /// The address the web view loads first.
    pub fn entry(&self) -> String {
        self.core.entry()
    }

    /// Whether the web view may navigate to `address`. Blocked navigations are logged.
    pub fn allows(&self, address: &str, main_frame: bool, download: bool) -> bool {
        let allowed = self.core.navigate(address, main_frame, download);
        self.log.deliver();
        allowed
    }

    /// Handles a message posted through the bridge, returning the page a `reloaded` message names.
    pub fn received(&self, body: &str) -> Option<String> {
        let reloaded = self.core.page_message(body);
        self.log.deliver();
        reloaded
    }

    /// Reads `address` (with its `Range` header, empty when none) and calls `answer` on the main
    /// thread once the file is read.
    pub fn read(&self, address: &str, range: &str, answer: Answer) {
        let number = self.next.get() + 1;
        self.next.set(number);
        self.reading
            .borrow_mut()
            .insert(number, (answer, address.to_owned()));
        let read = Read {
            core: self.core.clone(),
            address: address.to_owned(),
            range: range.to_owned(),
            files: self.id,
            task: number,
        };
        let _ = READER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send(read);
    }

    fn answer(&self, number: u64, reply: Reply) {
        let taken = self.reading.borrow_mut().remove(&number);
        if let Some((answer, address)) = taken {
            crate::core::trace(|| {
                format!(
                    "{} {address} {} bytes",
                    reply.status,
                    reply.body.as_ref().map_or(0, Vec::len)
                )
            });
            answer(reply);
        }
        self.log.deliver();
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let id = self.id;
        // Thread-locals may already be destroyed when the thread exits.
        let _ = FILES.try_with(|all| all.borrow_mut().remove(&id));
    }
}
