//! The app's only network path (ADR-0001), used by the updater for its feed and zip (`update`).
//! The core specifies the whole request: absolute URL, every header, the file to fill (or none,
//! for text) and a delay before sending. The shell's executor sends it as a `GET`, follows
//! redirects as a browser does, and streams back the status, the bytes, then the end
//! (`Receiving`). The executor keeps no cookies, cache or credentials and adds nothing. The core
//! decides where bytes go: the reply text, or a new file written only for a 200 and never over
//! an existing one.

use crate::Relock;
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// How long a request may wait for bytes.
pub const TIMEOUT_SECONDS: u32 = 60;
/// Cap on a text reply: far above any feed, so an endless response cannot exhaust memory.
const MOST_TEXT_BYTES: usize = 32 * 1024 * 1024;

/// One fully specified request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// How long the executor waits before sending it, for retry backoff.
    pub delay_ms: u64,
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    /// A new file that a 200's bytes stream into; nothing is written for other statuses. `None`
    /// returns the reply text instead.
    pub into: Option<PathBuf>,
}

/// A request to `url` with `headers`, writing into `into` or returning text.
pub fn get(url: String, headers: Vec<(&'static str, String)>, into: Option<PathBuf>) -> Request {
    Request {
        delay_ms: 0,
        url,
        headers,
        into,
    }
}

/// The result: the HTTP status (0 when nothing answered), the reply text (empty when it went to a
/// file or the request failed), and the system's failure message (empty on success). A 200 cut
/// short has both a status and a failure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub status: u32,
    pub body: String,
    pub failure: String,
}

/// What the executor has streamed so far for one request: the status, then the bytes, kept as
/// text or written to the request's file.
#[derive(Debug)]
pub struct Receiving {
    into: Option<PathBuf>,
    status: u32,
    body: Vec<u8>,
    file: Option<std::fs::File>,
    /// Why the core stopped the bytes, if it did.
    failure: String,
    /// The core stopped the reply on purpose, so the executor's later error is not a failure.
    stopped: bool,
}

impl Receiving {
    pub fn new(request: &Request) -> Self {
        Self {
            into: request.into.clone(),
            status: 0,
            body: Vec::new(),
            file: None,
            failure: String::new(),
            stopped: false,
        }
    }

    /// Records the status. Returns whether to take the bytes: a file request takes them only for
    /// a 200, into a newly created file.
    pub fn respond(&mut self, status: u32) -> bool {
        self.status = status;
        let Some(into) = &self.into else {
            return true;
        };
        if status != 200 {
            self.stopped = true;
            return false;
        }
        // Create only, so no existing file is ever overwritten.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(into)
        {
            Ok(file) => {
                self.file = Some(file);
                true
            }
            Err(error) => self.stop(error),
        }
    }

    /// Takes a chunk of the reply. Returns whether to take more.
    pub fn receive(&mut self, bytes: &[u8]) -> bool {
        let Some(file) = &mut self.file else {
            if self.body.len() + bytes.len() > MOST_TEXT_BYTES {
                let said = format!("the reply is longer than {MOST_TEXT_BYTES} bytes");
                return self.stop(std::io::Error::other(said));
            }
            self.body.extend_from_slice(bytes);
            return true;
        };
        match file.write_all(bytes) {
            Ok(()) => true,
            Err(error) => self.stop(error),
        }
    }

    fn stop(&mut self, error: std::io::Error) -> bool {
        self.failure = error.to_string();
        self.stopped = true;
        false
    }

    /// Ends the reply. `failure` is the system's error, empty on success. A reply the core
    /// stopped reports only the core's reason.
    pub fn finish(self, failure: &str) -> Reply {
        let failure = match (self.failure.is_empty(), self.stopped) {
            (false, _) => self.failure,
            (true, true) => String::new(),
            (true, false) => failure.to_owned(),
        };
        let body = if failure.is_empty() && self.into.is_none() {
            String::from_utf8(self.body).unwrap_or_default()
        } else {
            String::new()
        };
        Reply {
            status: self.status,
            body,
            failure,
        }
    }
}

// MARK: The app's one transport

struct Transport {
    last_id: u64,
    /// The updater step each sent request answers.
    waiting: HashMap<u64, crate::update::Step>,
    outbox: VecDeque<(u64, Request)>,
    /// What the executor has streamed for each request.
    receiving: HashMap<u64, Receiving>,
}

fn with<T>(action: impl FnOnce(&mut Transport) -> T) -> T {
    static TRANSPORT: OnceLock<Mutex<Transport>> = OnceLock::new();
    let transport = TRANSPORT.get_or_init(|| {
        Mutex::new(Transport {
            last_id: 0,
            waiting: HashMap::new(),
            outbox: VecDeque::new(),
            receiving: HashMap::new(),
        })
    });
    action(&mut transport.relock())
}

static WAKE: OnceLock<extern "C" fn()> = OnceLock::new();

/// Registers the callback the core uses, from any thread, to wake the shell when a request or
/// effect is ready outside a shell call. Only the first registration counts.
pub fn open(wake: extern "C" fn()) {
    let _ = WAKE.set(wake);
}

pub(crate) fn wake() {
    if let Some(wake) = WAKE.get() {
        wake();
    }
}

/// Moves the updater's next request into the outbox. False when there is none.
fn gather() -> bool {
    let Some((step, request)) = crate::update::ffi::next_request() else {
        return false;
    };
    with(|transport| {
        transport.last_id += 1;
        let id = transport.last_id;
        transport.waiting.insert(id, step);
        transport.outbox.push_back((id, request));
    });
    true
}

/// The next request for the shell to send, with its id; `None` when there is none.
pub fn next() -> Option<(u64, Request)> {
    loop {
        let sent = with(|transport| {
            let (id, request) = transport.outbox.pop_front()?;
            transport.receiving.insert(id, Receiving::new(&request));
            Some((id, request))
        });
        if sent.is_some() {
            return sent;
        }
        if !gather() {
            return None;
        }
    }
}

/// Records the status of request `id`. Returns whether to take its bytes
/// (`Receiving::respond`).
pub fn respond(id: u64, status: u32) -> bool {
    with(|transport| {
        transport
            .receiving
            .get_mut(&id)
            .is_some_and(|receiving| receiving.respond(status))
    })
}

/// Takes a chunk of request `id`. Returns whether to take more (`Receiving::receive`).
pub fn receive(id: u64, bytes: &[u8]) -> bool {
    with(|transport| {
        transport
            .receiving
            .get_mut(&id)
            .is_some_and(|receiving| receiving.receive(bytes))
    })
}

/// Ends request `id`, with `failure` empty on success, and answers the updater on this thread
/// (`Receiving::finish`).
pub fn finished(id: u64, failure: &str) {
    let (receiving, step) = with(|transport| {
        (
            transport.receiving.remove(&id),
            transport.waiting.remove(&id),
        )
    });
    if let (Some(receiving), Some(step)) = (receiving, step) {
        crate::update::ffi::answered(step, &receiving.finish(failure));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receiving(into: Option<PathBuf>) -> Receiving {
        Receiving::new(&get("https://northroom.com/x".into(), Vec::new(), into))
    }

    #[test]
    fn a_reply_kept_as_text_is_its_utf8_and_a_failure_keeps_none() {
        let mut reply = receiving(None);
        assert!(reply.respond(200));
        assert!(reply.receive(b"{\"ok\""));
        assert!(reply.receive(b":true}"));
        assert_eq!(
            reply.finish(""),
            Reply {
                status: 200,
                body: r#"{"ok":true}"#.into(),
                failure: String::new(),
            }
        );
        // Invalid UTF-8 reads as no text; a failure keeps no text.
        let mut broken = receiving(None);
        assert!(broken.respond(200) && broken.receive(&[0xff, 0xfe]));
        assert_eq!(broken.finish("").body, "");
        let mut cut = receiving(None);
        assert!(cut.respond(200) && cut.receive(b"half"));
        assert_eq!(
            (cut.finish("The network connection was lost.")).failure,
            "The network connection was lost."
        );
    }

    #[test]
    fn a_reply_kept_as_text_stops_at_its_most() {
        let mut reply = receiving(None);
        assert!(reply.respond(200));
        assert!(reply.receive(&vec![b'a'; MOST_TEXT_BYTES]));
        assert!(!reply.receive(b"a"), "one byte more is not taken");
        let reply = reply.finish("cancelled");
        assert_eq!((reply.status, reply.body.as_str()), (200, ""));
        assert_eq!(
            reply.failure,
            format!("the reply is longer than {MOST_TEXT_BYTES} bytes")
        );
    }

    #[test]
    fn a_file_is_filled_only_for_a_200_and_only_when_it_is_new() {
        let folder = tempfile::tempdir().unwrap();
        let into = folder.path().join("update.zip");
        let mut refused = receiving(Some(into.clone()));
        assert!(!refused.respond(404), "a 404's bytes are not taken");
        assert_eq!(
            refused.finish("cancelled"),
            Reply {
                status: 404,
                ..Reply::default()
            },
            "stopped on purpose: no failure"
        );
        assert!(!into.exists());

        let mut filled = receiving(Some(into.clone()));
        assert!(filled.respond(200));
        assert!(filled.receive(b"zip bytes"));
        let reply = filled.finish("");
        assert_eq!((reply.status, reply.body.as_str()), (200, ""));
        assert_eq!(std::fs::read(&into).unwrap(), b"zip bytes");

        // An existing file is never overwritten.
        let mut again = receiving(Some(into.clone()));
        assert!(!again.respond(200));
        let reply = again.finish("cancelled");
        assert_eq!(reply.status, 200);
        assert!(reply.failure.contains("exists"), "{}", reply.failure);
        assert_eq!(std::fs::read(&into).unwrap(), b"zip bytes");
    }
}
