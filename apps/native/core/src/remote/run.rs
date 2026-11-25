//! The phone link while the app runs: one thread that takes what devices send, answers by the
//! rules (`service`) against the app's own terminals, and a few times a second sends each device
//! what changed.

use crate::remote::devices::{Devices, Key, FILE};
use crate::remote::pairing::Invite;
use crate::remote::protocol::Pressed;
use crate::remote::screen::Screen;
use crate::remote::service::{Desk, Out, Service};
use crate::sessions::Running;
use crate::terminal::cells::GridSize;
use crate::terminal::input::Modifiers;
use crate::Relock;
use northroom_link::{Event, Host, Relays, Secret};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often connected devices are sent what changed.
const CHANGES_EVERY: Duration = Duration::from_millis(150);
/// What Control-C sends.
const INTERRUPT: &[u8] = &[3];

/// The app's running terminals (`App::sessions`).
struct AppDesk;

impl Desk for AppDesk {
    fn sessions(&self) -> Vec<Running> {
        crate::sessions::ffi::running_now()
    }

    fn screen(&self, session: &str) -> Option<Screen> {
        crate::app().sessions.relock().get(session)?.remote_screen()
    }

    fn fit(&self, session: &str, grid: Option<GridSize>) {
        if let Some(terminal) = crate::app().sessions.relock().get(session) {
            terminal.hold_grid(grid);
        }
    }

    fn paste(&self, session: &str, text: &str) {
        if let Some(terminal) = crate::app().sessions.relock().get(session) {
            terminal.paste(text);
        }
    }

    fn press(&self, session: &str, pressed: Pressed) {
        let sessions = crate::app().sessions.relock();
        let Some(terminal) = sessions.get(session) else {
            return;
        };
        match pressed {
            Pressed::Key(key) => terminal.key(key, Modifiers::default()),
            Pressed::Interrupt => terminal.input(INTERRUPT),
        }
    }
}

/// A running phone link. Dropping it ends every device's link: the serving thread sees it is
/// the host's last holder and lets the host go.
pub struct Link {
    host: Arc<Host>,
    service: Arc<Mutex<Service>>,
    /// This computer's name, as a pairing link gives it.
    name: String,
}

impl Link {
    /// Starts the link for the devices kept in `folder`, reachable through `relays`, serving
    /// the terminals of `desk`.
    pub fn start(
        folder: &std::path::Path,
        relays: Relays,
        name: String,
        desk: impl Desk + Send + 'static,
    ) -> Result<Self, northroom_link::Error> {
        let devices = Devices::open(&folder.join(FILE), || Secret::generate().to_bytes());
        let secret = Secret::from_bytes(&devices.secret());
        let (events, happened) = std::sync::mpsc::channel();
        let host = Arc::new(Host::start(secret, relays, events)?);
        let service = Arc::new(Mutex::new(Service::new(name.clone(), devices)));
        let (serving, served) = (Arc::clone(&host), Arc::clone(&service));
        // The thread is left to itself: it ends within `CHANGES_EVERY` of this link's drop.
        std::thread::Builder::new()
            .name("northroom-phone-link".to_owned())
            .spawn(move || serve(&serving, &served, &happened, &desk))
            .map_err(|failed| northroom_link::Error(failed.to_string()))?;
        Ok(Self {
            host,
            service,
            name,
        })
    }

    /// The app's own link: its devices are kept in the app's folder and its terminals are the
    /// running sessions.
    pub fn start_for_app(relays: Relays) -> Result<Self, northroom_link::Error> {
        let folder = crate::paths::get().support;
        Self::start(&folder, relays, computer_name(), AppDesk)
    }

    /// A pairing link to show as a QR code, good for `pairing::OFFER_LASTS`. It replaces any
    /// shown before.
    pub fn invite(&self) -> String {
        let code = self.service.relock().offer(Instant::now());
        let address = self.host.address();
        Invite {
            key: address.key,
            code,
            host: self.name.clone(),
            relay: address.relay,
            direct: address.direct,
        }
        .link()
    }

    /// Takes the pairing link off screen; it no longer works.
    pub fn withdraw_invite(&self) {
        self.service.relock().withdraw_offer();
    }

    /// The paired devices, as (key, name).
    pub fn devices(&self) -> Vec<(String, String)> {
        let service = self.service.relock();
        let devices = service.devices().list().iter();
        devices
            .map(|device| (device.key.clone(), device.name.clone()))
            .collect()
    }

    /// Forgets the device with `key` and ends its links.
    pub fn forget(&self, key: &Key, desk: &dyn Desk) -> std::io::Result<()> {
        let out = self.service.relock().forget(key, desk)?;
        send(&self.host, out);
        Ok(())
    }
}

fn send(host: &Host, out: Vec<Out>) {
    for out in out {
        match out {
            Out::Tell(link, tell) => host.send(link, tell.encoded()),
            Out::Close(link) => host.close(link),
        }
    }
}

/// Answers `event` by the rules.
fn answer(service: &Mutex<Service>, event: Event, desk: &dyn Desk) -> Vec<Out> {
    let mut service = service.relock();
    match event {
        Event::Opened { link, peer } => {
            service.opened(link, peer);
            Vec::new()
        }
        Event::Message { link, bytes } => service.message(link, &bytes, desk, Instant::now()),
        Event::Closed { link } => {
            service.closed(link, desk);
            Vec::new()
        }
    }
}

/// Serves devices until the link is dropped: answers each event as it comes, and between them
/// sends what changed. With no device connected it only waits.
fn serve(host: &Arc<Host>, service: &Mutex<Service>, happened: &Receiver<Event>, desk: &dyn Desk) {
    // The loop's own hold on the host must not keep it alive: it ends when `Link` drops its one.
    while Arc::strong_count(host) > 1 {
        match happened.recv_timeout(CHANGES_EVERY) {
            Ok(event) => send(host, answer(service, event, desk)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let changed = {
            let mut service = service.relock();
            if service.is_linked() {
                service.changes(desk)
            } else {
                Vec::new()
            }
        };
        send(host, changed);
    }
}

/// This computer's name as the person knows it, without a network suffix.
fn computer_name() -> String {
    let name = system_name().unwrap_or_default();
    let short = name.split('.').next().unwrap_or_default().trim();
    if short.is_empty() {
        "Computer".to_owned()
    } else {
        short.to_owned()
    }
}

#[cfg(unix)]
fn system_name() -> Option<String> {
    let mut name = [0u8; 256];
    // SAFETY: `name` is writable for its whole length, which is the length passed.
    let failed = unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } != 0;
    if failed {
        return None;
    }
    let end = name.iter().position(|byte| *byte == 0)?;
    String::from_utf8(name[..end].to_vec()).ok()
}

#[cfg(not(unix))]
fn system_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

#[cfg(test)]
mod tests;
