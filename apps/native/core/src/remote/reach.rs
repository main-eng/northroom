//! This computer as a device of another: it pairs with the link the other computer shows, stays
//! connected to it, lists the sessions running there and shows them in terminals here
//! (`terminal::Far`). What is typed here goes there, and the session's output comes back as the
//! bytes its own terminal reads, so the terminal here scrolls, selects and resizes as any other.
//!
//! The other computer is the host of `remote::service`; this side speaks the same messages
//! (`docs/phone-link.md`) a phone does. It has a key of its own, apart from the one this
//! computer is known by as a host, kept with the computers it has paired with in one file only
//! the person can read.

use crate::remote::devices::{key_hex, parse_key_hex, write_private};
use crate::remote::pairing::Invite;
use crate::remote::protocol::Ask;
use crate::terminal::cells::GridSize;
use crate::terminal::{Far, Palette, Terminal, ViewSize};
use crate::Relock;
use northroom_link::{Address, Client, Heard, Relays, Secret, Sending};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

/// The file in the app's folder that keeps this computer's key as a device and the computers
/// it has paired with.
pub const FILE: &str = "computers.json";

/// How long one wait for the other computer's next message lasts, which is also how soon a
/// link notices it was forgotten.
const HEARD_EVERY: Duration = Duration::from_millis(200);
/// How long after a link fails or closes the next try is made: first, and at most. Each try
/// that fails doubles the wait.
const RETRY_FIRST: Duration = Duration::from_secs(2);
const RETRY_MOST: Duration = Duration::from_secs(30);

/// A computer this one has paired with, and where to reach it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Computer {
    /// Its public key, as hex.
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub relay: Option<String>,
    /// Its addresses on its own networks.
    #[serde(default)]
    pub direct: Vec<String>,
}

impl Computer {
    fn of(invite: &Invite) -> Self {
        Self {
            key: key_hex(&invite.key),
            name: invite.host.clone(),
            relay: invite.relay.clone(),
            direct: invite.direct.iter().map(ToString::to_string).collect(),
        }
    }

    fn address(&self) -> Option<Address> {
        Some(Address {
            key: parse_key_hex(&self.key)?,
            relay: self.relay.clone(),
            direct: self
                .direct
                .iter()
                .filter_map(|direct| direct.parse().ok())
                .collect(),
        })
    }

    /// This side reaches the other through the relay it listens at, and uses none of its own.
    fn relays(&self) -> Relays {
        self.relay.clone().map_or(Relays::None, Relays::At)
    }
}

#[derive(Serialize, Deserialize)]
struct Kept {
    secret: String,
    computers: Vec<Computer>,
}

/// A session running on another computer, as that computer lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct FarSession {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub doing: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub activity: String,
}

/// What the other computer tells this one, of the messages this side acts on. The rest are for
/// devices that show files and screens themselves.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum Told {
    Welcome {
        host: String,
    },
    Refused {},
    Sessions {
        sessions: Vec<FarSession>,
    },
    Started {
        session: String,
    },
    Gone {
        session: String,
    },
    Output {
        session: String,
        data: String,
    },
    #[serde(other)]
    Other,
}

/// How things stand with a paired computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Being dialled, or greeted.
    Connecting,
    Connected,
    /// The last try failed; another follows.
    OutOfReach,
}

/// A paired computer as the app lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub key: String,
    pub name: String,
    pub standing: Standing,
    pub sessions: Vec<FarSession>,
}

/// What the app does with a computer's key and the session that computer started.
pub type Started = Box<dyn Fn(&str, &str) + Send + Sync>;

/// What the app does when a paired computer says something that is not a terminal's own.
pub struct Events {
    /// A computer's standing, name or sessions changed, or one was added or forgotten.
    pub changed: Box<dyn Fn() + Send + Sync>,
    /// The computer with this key started the session this computer asked it for.
    pub started: Started,
}

/// A session of another computer shown in a terminal here.
struct Shown {
    terminal: Terminal,
    side: Arc<FarSide>,
}

/// One paired computer and the link to it.
struct Linked {
    computer: Mutex<Computer>,
    /// The pairing code, until the computer has taken it.
    code: Mutex<Option<String>>,
    /// Set while the link is open.
    sending: Mutex<Option<Sending>>,
    standing: Mutex<Standing>,
    sessions: Mutex<Vec<FarSession>>,
    shown: Mutex<HashMap<String, Shown>>,
    forgotten: AtomicBool,
}

impl Linked {
    fn new(computer: Computer, code: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            computer: Mutex::new(computer),
            code: Mutex::new(code),
            sending: Mutex::new(None),
            standing: Mutex::new(Standing::Connecting),
            sessions: Mutex::new(Vec::new()),
            shown: Mutex::new(HashMap::new()),
            forgotten: AtomicBool::new(false),
        })
    }

    /// Asks the other computer, if the link is open.
    fn ask(&self, ask: &Ask) {
        if let Some(sending) = self.sending.relock().as_ref() {
            sending.send(serde_json::to_vec(ask).unwrap_or_default());
        }
    }

    fn follow(&self, session: &str, grid: GridSize) {
        self.ask(&Ask::Follow {
            session: session.to_owned(),
            columns: grid.columns,
            lines: grid.lines,
        });
    }

    /// The terminal here that shows `session`.
    fn terminal(&self, session: &str) -> Option<Terminal> {
        self.shown
            .relock()
            .get(session)
            .map(|shown| shown.terminal.same())
    }

    fn seen(&self) -> Seen {
        let computer = self.computer.relock().clone();
        Seen {
            key: computer.key,
            name: computer.name,
            standing: *self.standing.relock(),
            sessions: self.sessions.relock().clone(),
        }
    }
}

/// A terminal's hold on the session it shows: what it sends there.
struct FarSide {
    linked: Weak<Linked>,
    session: String,
    /// The grid the terminal here last asked for.
    grid: Mutex<GridSize>,
    /// Set once the session is followed, after which a new grid is sent as it changes.
    followed: AtomicBool,
}

impl Far for FarSide {
    fn input(&self, bytes: Vec<u8>) {
        if let Some(linked) = self.linked.upgrade() {
            linked.ask(&Ask::Input {
                session: self.session.clone(),
                data: crate::remote::bytes::written(&bytes),
            });
        }
    }

    fn fit(&self, grid: GridSize) {
        *self.grid.relock() = grid;
        let Some(linked) = self.linked.upgrade() else {
            return;
        };
        if self.followed.load(Ordering::Relaxed) {
            linked.ask(&Ask::Fit {
                session: self.session.clone(),
                columns: grid.columns,
                lines: grid.lines,
            });
        }
    }

    fn leave(&self) {
        let Some(linked) = self.linked.upgrade() else {
            return;
        };
        linked.shown.relock().remove(&self.session);
        linked.ask(&Ask::Unfollow {
            session: self.session.clone(),
        });
    }
}

struct Inner {
    path: PathBuf,
    secret: [u8; 32],
    /// This computer's name, as the other lists it.
    name: String,
    links: Mutex<Vec<Arc<Linked>>>,
    events: Events,
    closed: AtomicBool,
}

/// This computer's links to the computers it has paired with. Dropping it closes them.
pub struct Reach {
    inner: Arc<Inner>,
}

impl Drop for Reach {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::Relaxed);
    }
}

fn read(path: &Path) -> Option<([u8; 32], Vec<Computer>)> {
    let kept: Kept = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some((parse_key_hex(&kept.secret)?, kept.computers))
}

impl Reach {
    /// Opens the links kept in `folder` and starts reaching each computer. `name` is what
    /// those computers list this one as.
    pub fn open(folder: &Path, name: String, events: Events) -> Self {
        let path = folder.join(FILE);
        let (secret, computers) =
            read(&path).unwrap_or_else(|| (Secret::generate().to_bytes(), Vec::new()));
        let inner = Arc::new(Inner {
            path,
            secret,
            name,
            links: Mutex::new(Vec::new()),
            events,
            closed: AtomicBool::new(false),
        });
        for computer in computers {
            inner.reach(Linked::new(computer, None));
        }
        Self { inner }
    }

    /// Whether `folder` keeps any computer, without opening the links.
    pub fn keeps_any(folder: &Path) -> bool {
        read(&folder.join(FILE)).is_some_and(|(_, computers)| !computers.is_empty())
    }

    /// Pairs with the computer that showed the pairing `link`, replacing what was kept of it.
    /// False when the text is no pairing link. Whether the computer took it shows in
    /// `computers`: it is listed once it has, and not if it refused.
    pub fn pair(&self, link: &str) -> bool {
        let Some(invite) = Invite::parse(link.trim()) else {
            return false;
        };
        let computer = Computer::of(&invite);
        self.inner.drop_link(&computer.key);
        self.inner
            .reach(Linked::new(computer, Some(invite.code.clone())));
        (self.inner.events.changed)();
        true
    }

    /// Forgets the computer with `key`: its link closes and it is no longer kept. The terminals
    /// here that showed its sessions say they ended.
    pub fn forget(&self, key: &str) {
        self.inner.drop_link(key);
        self.inner.keep();
        (self.inner.events.changed)();
    }

    pub fn computers(&self) -> Vec<Seen> {
        let links = self.inner.links.relock();
        links.iter().map(|linked| linked.seen()).collect()
    }

    /// Asks the computer with `key` to start a session. `Events::started` hears which.
    pub fn start(&self, key: &str) {
        if let Some(linked) = self.inner.linked(key) {
            linked.ask(&Ask::Start);
        }
    }

    /// A terminal here in `view` that shows `session` of the computer with `key`. It shows
    /// nothing until that computer is reached, and catches up whenever it is reached again.
    pub fn show(
        &self,
        key: &str,
        session: &str,
        view: ViewSize,
        palette: Palette,
    ) -> Option<Terminal> {
        let linked = self.inner.linked(key)?;
        let side = Arc::new(FarSide {
            linked: Arc::downgrade(&linked),
            session: session.to_owned(),
            grid: Mutex::new(GridSize {
                columns: 0,
                lines: 0,
            }),
            followed: AtomicBool::new(false),
        });
        let far: Arc<dyn Far> = side.clone();
        let terminal = Terminal::of_far(view, far, palette);
        let grid = *side.grid.relock();
        linked.shown.relock().insert(
            session.to_owned(),
            Shown {
                terminal: terminal.same(),
                side: Arc::clone(&side),
            },
        );
        side.followed.store(true, Ordering::Relaxed);
        linked.follow(session, grid);
        Some(terminal)
    }
}

/// How a conversation with a computer ended.
#[derive(PartialEq, Eq)]
enum Ended {
    /// The link closed or was never opened; try again.
    Lost,
    /// The computer refused this one, or the person forgot it.
    Over,
}

impl Inner {
    fn linked(&self, key: &str) -> Option<Arc<Linked>> {
        let links = self.links.relock();
        links
            .iter()
            .find(|linked| linked.computer.relock().key == key)
            .cloned()
    }

    /// Stops and unlists the link to the computer with `key`. Its terminals here end.
    fn drop_link(&self, key: &str) {
        let dropped: Vec<Arc<Linked>> = {
            let mut links = self.links.relock();
            let (dropped, kept) = links
                .drain(..)
                .partition(|linked| linked.computer.relock().key == key);
            *links = kept;
            dropped
        };
        for linked in dropped {
            linked.forgotten.store(true, Ordering::Relaxed);
            *linked.sending.relock() = None;
            let shown: Vec<Terminal> = linked
                .shown
                .relock()
                .values()
                .map(|shown| shown.terminal.same())
                .collect();
            for terminal in shown {
                terminal.far_ended();
            }
        }
    }

    /// Saves the key and every computer that has taken this one's pairing.
    fn keep(&self) {
        let computers = self
            .links
            .relock()
            .iter()
            .filter(|linked| linked.code.relock().is_none())
            .map(|linked| linked.computer.relock().clone())
            .collect();
        let kept = Kept {
            secret: key_hex(&self.secret),
            computers,
        };
        let saved = serde_json::to_vec_pretty(&kept)
            .map_err(std::io::Error::other)
            .and_then(|bytes| write_private(&self.path, &bytes));
        if let Err(failed) = saved {
            eprintln!("computers\tthe paired computers were not saved: {failed}");
        }
    }

    /// Lists `linked` and reaches its computer on a thread of its own until it is forgotten.
    fn reach(self: &Arc<Self>, linked: Arc<Linked>) {
        self.links.relock().push(Arc::clone(&linked));
        let inner = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("northroom-computer-link".to_owned())
            .spawn(move || inner.serve(&linked));
        if let Err(failed) = spawned {
            eprintln!("computers\ta link's thread did not start: {failed}");
        }
    }

    fn over(&self, linked: &Linked) -> bool {
        self.closed.load(Ordering::Relaxed) || linked.forgotten.load(Ordering::Relaxed)
    }

    /// Reaches `linked`'s computer again and again until it is forgotten or refuses.
    fn serve(self: &Arc<Self>, linked: &Arc<Linked>) {
        let mut wait = RETRY_FIRST;
        while !self.over(linked) {
            self.stands(linked, Standing::Connecting);
            let (ended, welcomed) = self.connect(linked);
            *linked.sending.relock() = None;
            if ended == Ended::Over {
                break;
            }
            self.stands(linked, Standing::OutOfReach);
            wait = if welcomed {
                RETRY_FIRST
            } else {
                (wait * 2).min(RETRY_MOST)
            };
            self.rest(linked, wait);
        }
    }

    /// Waits `most` before the next try, or less once the link is over.
    fn rest(&self, linked: &Linked, most: Duration) {
        let started = std::time::Instant::now();
        while started.elapsed() < most && !self.over(linked) {
            std::thread::sleep(HEARD_EVERY);
        }
    }

    fn stands(&self, linked: &Linked, standing: Standing) {
        let changed = std::mem::replace(&mut *linked.standing.relock(), standing) != standing;
        if changed {
            (self.events.changed)();
        }
    }

    /// Dials the computer and talks with it until the link ends. Also whether it welcomed this
    /// one, which makes the next try a prompt one.
    fn connect(self: &Arc<Self>, linked: &Arc<Linked>) -> (Ended, bool) {
        let computer = linked.computer.relock().clone();
        let Some(address) = computer.address() else {
            return (Ended::Over, false);
        };
        let secret = Secret::from_bytes(&self.secret);
        let Ok(client) = Client::connect(secret, computer.relays(), address) else {
            return (Ended::Lost, false);
        };
        *linked.sending.relock() = client.sending();
        if let Some(code) = linked.code.relock().clone() {
            linked.ask(&Ask::Pair {
                code,
                name: self.name.clone(),
            });
        }
        linked.ask(&Ask::Hello);
        let mut welcomed = false;
        loop {
            match client.hear(HEARD_EVERY) {
                Heard::Closed => return (Ended::Lost, welcomed),
                Heard::Quiet if self.over(linked) => return (Ended::Over, welcomed),
                Heard::Quiet => {}
                Heard::Message(bytes) => match self.told(linked, &bytes) {
                    Some(Ended::Over) => return (Ended::Over, welcomed),
                    Some(Ended::Lost) | None => welcomed |= self.is_connected(linked),
                },
            }
        }
    }

    fn is_connected(&self, linked: &Linked) -> bool {
        *linked.standing.relock() == Standing::Connected
    }

    /// Acts on one message from `linked`'s computer. Says so when it ends the link for good.
    fn told(self: &Arc<Self>, linked: &Arc<Linked>, bytes: &[u8]) -> Option<Ended> {
        match serde_json::from_slice(bytes).unwrap_or(Told::Other) {
            Told::Welcome { host } => self.welcomed(linked, host),
            Told::Refused {} => {
                self.refused(linked);
                return Some(Ended::Over);
            }
            Told::Sessions { sessions } => {
                *linked.sessions.relock() = sessions;
                (self.events.changed)();
            }
            Told::Started { session } => {
                let key = linked.computer.relock().key.clone();
                (self.events.started)(&key, &session);
            }
            Told::Gone { session } => {
                if let Some(terminal) = linked.terminal(&session) {
                    terminal.far_ended();
                }
            }
            Told::Output { session, data } => {
                let heard = crate::remote::bytes::read(&data).zip(linked.terminal(&session));
                if let Some((bytes, terminal)) = heard {
                    terminal.heard_far(&bytes);
                }
            }
            Told::Other => {}
        }
        None
    }

    /// The computer let this one in: it is kept from now on, under the name it gives itself,
    /// and every terminal here that shows one of its sessions follows that session again.
    fn welcomed(&self, linked: &Linked, host: String) {
        let paired_now = linked.code.relock().take().is_some();
        let renamed = std::mem::replace(&mut linked.computer.relock().name, host.clone()) != host;
        if paired_now || renamed {
            self.keep();
        }
        if *linked.standing.relock() == Standing::Connected {
            return;
        }
        let following: Vec<(String, GridSize)> = linked
            .shown
            .relock()
            .iter()
            .map(|(session, shown)| (session.clone(), *shown.side.grid.relock()))
            .collect();
        for (session, grid) in following {
            linked.follow(&session, grid);
        }
        self.stands(linked, Standing::Connected);
    }

    /// The computer turned this one away: the code was wrong or stale, or the person there
    /// forgot this computer. It is no longer kept.
    fn refused(&self, linked: &Linked) {
        let key = linked.computer.relock().key.clone();
        self.drop_link(&key);
        self.keep();
        (self.events.changed)();
    }
}

#[cfg(test)]
mod tests;
