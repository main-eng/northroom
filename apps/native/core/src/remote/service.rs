//! What this computer answers a device: who may stay, and what each message does to the
//! sessions. It works on a `Desk` (the running terminals) and returns what to send, so the rules
//! are checked without a network or a terminal.

use crate::remote::devices::{Devices, Key};
use crate::remote::pairing::Offer;
use crate::remote::protocol::{parse_ask, parse_key, Ask, Pressed, Refusal, Session, Tell};
use crate::remote::screen::Screen;
use crate::sessions::{self, Running};
use crate::terminal::cells::GridSize;
use std::collections::HashMap;
use std::time::Instant;

/// The smallest and largest grid a device may fit a terminal to.
const GRID_LEAST: GridSize = GridSize {
    columns: 20,
    lines: 5,
};
const GRID_MOST: GridSize = GridSize {
    columns: 300,
    lines: 200,
};
/// The most lines one `scroll` message moves, either way.
const SCROLL_MOST_LINES: i32 = 500;

/// The running terminals, as a device may see and use them.
pub trait Desk {
    /// Every running session.
    fn sessions(&self) -> Vec<Running>;
    /// What `session`'s terminal shows; `None` when it is not running.
    fn screen(&self, session: &str) -> Option<Screen>;
    /// Holds `session`'s terminal at `grid` while a device watches it, or with `None` gives it
    /// back the size of its view here.
    fn fit(&self, session: &str, grid: Option<GridSize>);
    fn paste(&self, session: &str, text: &str);
    fn press(&self, session: &str, pressed: Pressed);
    /// Moves `session`'s terminal by `lines` as the wheel does, towards older output when
    /// positive.
    fn scroll(&self, session: &str, lines: i32);
}

/// Something to send or do on a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    Tell(u64, Tell),
    Close(u64),
}

/// One connected device.
struct Linked {
    peer: Key,
    paired: bool,
    /// The session it watches.
    watching: Option<String>,
    /// What it was last told, so only changes are sent.
    sessions_told: Option<Vec<Session>>,
    screen_told: Option<Screen>,
}

/// This computer's side of every link.
pub struct Service {
    /// This computer's name, as devices show it.
    host: String,
    devices: Devices,
    offer: Option<Offer>,
    links: HashMap<u64, Linked>,
}

/// `running` as devices are told it, in the order the sessions list shows.
fn listed(running: &[Running]) -> Vec<Session> {
    sessions::grouped(running)
        .into_iter()
        .flat_map(|(_, group)| group)
        .map(Session::of)
        .collect()
}

/// A device's grid, held to sizes a terminal can be.
fn normalize_grid(columns: usize, lines: usize) -> GridSize {
    GridSize {
        columns: columns.clamp(GRID_LEAST.columns, GRID_MOST.columns),
        lines: lines.clamp(GRID_LEAST.lines, GRID_MOST.lines),
    }
}

fn refused(link: u64, why: Refusal) -> Vec<Out> {
    vec![Out::Tell(link, Tell::Refused { why }), Out::Close(link)]
}

impl Service {
    pub fn new(host: String, devices: Devices) -> Self {
        Self {
            host,
            devices,
            offer: None,
            links: HashMap::new(),
        }
    }

    pub fn devices(&self) -> &Devices {
        &self.devices
    }

    /// Shows a new pairing code, replacing any shown before, and returns it.
    pub fn offer(&mut self, now: Instant) -> String {
        let offer = Offer::new(now);
        let code = offer.code().to_owned();
        self.offer = Some(offer);
        code
    }

    /// Takes the pairing code off screen; it no longer works.
    pub fn withdraw_offer(&mut self) {
        self.offer = None;
    }

    /// Forgets the device with `key` and ends its links.
    pub fn forget(&mut self, key: &Key, desk: &dyn Desk) -> std::io::Result<Vec<Out>> {
        self.devices.forget(key)?;
        let ended: Vec<u64> = self
            .links
            .iter()
            .filter(|(_, linked)| linked.peer == *key)
            .map(|(link, _)| *link)
            .collect();
        for link in &ended {
            self.closed(*link, desk);
        }
        Ok(ended.into_iter().map(Out::Close).collect())
    }

    /// The device with key `peer` connected on `link`.
    pub fn opened(&mut self, link: u64, peer: Key) {
        let paired = self.devices.is_paired(&peer);
        self.links.insert(
            link,
            Linked {
                peer,
                paired,
                watching: None,
                sessions_told: None,
                screen_told: None,
            },
        );
    }

    /// `link` ended. A terminal its device watched gets its own size back.
    pub fn closed(&mut self, link: u64, desk: &dyn Desk) {
        let watched = self.links.remove(&link).and_then(|linked| linked.watching);
        if let Some(session) = watched {
            self.release(&session, desk);
        }
    }

    /// Gives `session`'s terminal its own size back unless another device still watches it.
    fn release(&self, session: &str, desk: &dyn Desk) {
        let watched = self
            .links
            .values()
            .any(|linked| linked.watching.as_deref() == Some(session));
        if !watched {
            desk.fit(session, None);
        }
    }

    /// Answers the message `bytes` from `link`.
    pub fn message(&mut self, link: u64, bytes: &[u8], desk: &dyn Desk, now: Instant) -> Vec<Out> {
        let Some(paired) = self.links.get(&link).map(|linked| linked.paired) else {
            return Vec::new();
        };
        match (parse_ask(bytes), paired) {
            (None, _) => refused(link, Refusal::NotUnderstood),
            (Some(Ask::Pair { code, name }), _) => self.pair(link, &code, &name, now),
            (Some(_), false) => refused(link, Refusal::NotPaired),
            (Some(ask), true) => self.answer(link, ask, desk),
        }
    }

    /// Pairs `link`'s device when `code` is the one on screen. The code works once.
    fn pair(&mut self, link: u64, code: &str, name: &str, now: Instant) -> Vec<Out> {
        let fresh = self
            .offer
            .as_ref()
            .is_some_and(|offer| offer.accepts(code, now));
        let Some(linked) = self.links.get_mut(&link).filter(|_| fresh) else {
            return refused(link, Refusal::WrongCode);
        };
        if self.devices.pair(&linked.peer, name).is_err() {
            return refused(link, Refusal::WrongCode);
        }
        linked.paired = true;
        self.offer = None;
        vec![Out::Tell(link, self.welcome())]
    }

    fn welcome(&self) -> Tell {
        Tell::Welcome {
            host: self.host.clone(),
        }
    }

    /// Does what a paired device asked.
    fn answer(&mut self, link: u64, ask: Ask, desk: &dyn Desk) -> Vec<Out> {
        match ask {
            Ask::Pair { .. } => Vec::new(),
            Ask::Hello => {
                let mut out = vec![Out::Tell(link, self.welcome())];
                out.extend(self.tell_sessions(link, desk));
                out
            }
            Ask::Watch {
                session,
                columns,
                lines,
            } => self.watch(link, session, normalize_grid(columns, lines), desk),
            Ask::Leave => {
                self.leave(link, desk);
                Vec::new()
            }
            Ask::Type { session, text } => {
                desk.paste(&session, &text);
                Vec::new()
            }
            Ask::Press { session, key } => {
                if let Some(pressed) = parse_key(&key) {
                    desk.press(&session, pressed);
                }
                Vec::new()
            }
            Ask::Scroll { session, lines } => {
                desk.scroll(&session, lines.clamp(-SCROLL_MOST_LINES, SCROLL_MOST_LINES));
                // What scrolled into view is sent now, not at the next look for changes.
                self.tell_screen(link, desk)
            }
        }
    }

    fn watch(&mut self, link: u64, session: String, grid: GridSize, desk: &dyn Desk) -> Vec<Out> {
        self.leave(link, desk);
        desk.fit(&session, Some(grid));
        if let Some(linked) = self.links.get_mut(&link) {
            linked.watching = Some(session);
        }
        self.tell_screen(link, desk)
    }

    fn leave(&mut self, link: u64, desk: &dyn Desk) {
        let Some(linked) = self.links.get_mut(&link) else {
            return;
        };
        linked.screen_told = None;
        if let Some(session) = linked.watching.take() {
            self.release(&session, desk);
        }
    }

    /// The sessions, when they differ from what `link`'s device was last told.
    fn tell_sessions(&mut self, link: u64, desk: &dyn Desk) -> Vec<Out> {
        let now = listed(&desk.sessions());
        let Some(linked) = self.links.get_mut(&link) else {
            return Vec::new();
        };
        if linked.sessions_told.as_ref() == Some(&now) {
            return Vec::new();
        }
        linked.sessions_told = Some(now.clone());
        vec![Out::Tell(link, Tell::Sessions { sessions: now })]
    }

    /// The watched screen, when it differs from what `link`'s device was last told, or that the
    /// session is gone.
    fn tell_screen(&mut self, link: u64, desk: &dyn Desk) -> Vec<Out> {
        let Some(linked) = self.links.get_mut(&link) else {
            return Vec::new();
        };
        let Some(session) = linked.watching.clone() else {
            return Vec::new();
        };
        let Some(screen) = desk.screen(&session) else {
            linked.watching = None;
            linked.screen_told = None;
            return vec![Out::Tell(link, Tell::Gone { session })];
        };
        if linked.screen_told.as_ref() == Some(&screen) {
            return Vec::new();
        }
        linked.screen_told = Some(screen.clone());
        vec![Out::Tell(link, Tell::Screen { session, screen })]
    }

    /// What has changed for each paired device since it was last told. Call a few times a
    /// second while any device is connected.
    pub fn changes(&mut self, desk: &dyn Desk) -> Vec<Out> {
        let mut paired: Vec<u64> = self
            .links
            .iter()
            .filter(|(_, linked)| linked.sessions_told.is_some())
            .map(|(link, _)| *link)
            .collect();
        paired.sort_unstable();
        let mut out = Vec::new();
        for link in paired {
            out.extend(self.tell_sessions(link, desk));
            out.extend(self.tell_screen(link, desk));
        }
        out
    }

    /// Whether any device is connected, so `changes` is worth calling.
    pub fn is_linked(&self) -> bool {
        !self.links.is_empty()
    }
}

#[cfg(test)]
mod tests;
