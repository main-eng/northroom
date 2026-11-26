//! The phone link while the app runs: one thread that takes what devices send, answers by the
//! rules (`service`) against the app's own terminals, and a few times a second sends each device
//! what changed.

use crate::remote::devices::{Devices, Key, FILE};
use crate::remote::pairing::Invite;
use crate::remote::protocol::{Item, Listing, Piece, Pressed};
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
/// The longest a pairing link waits for a host that just started to reach its relay, and how
/// often it looks.
const RELAY_WAIT: Duration = Duration::from_secs(2);
const RELAY_ASKED_EVERY: Duration = Duration::from_millis(25);
/// What Control-C sends.
const INTERRUPT: &[u8] = &[3];

/// The app's running terminals (`App::sessions`).
struct AppDesk;

impl Desk for AppDesk {
    fn sessions(&self) -> Vec<Running> {
        crate::sessions::ffi::running_for_devices()
    }

    fn start(&self) -> Option<String> {
        crate::ffi::start_unshown_session()
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

    fn keys(&self, session: &str, text: &str) {
        if let Some(terminal) = crate::app().sessions.relock().get(session) {
            terminal.input(text.as_bytes());
        }
    }

    fn folder(&self, folder: Option<&str>) -> Option<Listing> {
        let location = crate::workspace::Location {
            folder_id: folder.map(str::to_owned),
        };
        crate::ffi::with_columns(|columns| {
            let workspace = columns.workspace();
            let name = workspace.folder_title(&location).ok()?;
            let entries = workspace.all_contents(&location).ok()?;
            Some(Listing {
                name,
                items: entries.iter().map(Item::of).collect(),
            })
        })?
    }

    fn read(&self, file: &str, from: u64, length: usize) -> Option<Piece> {
        let path = crate::ffi::with_columns(|columns| columns.workspace().path_of(file))?.ok()?;
        match read_piece(&path, from, length) {
            Ok(piece) => Some(piece),
            Err(error) => {
                eprintln!("a phone's read of {} failed: {error}", path.display());
                None
            }
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

    fn scroll(&self, session: &str, lines: i32) {
        if let Some(terminal) = crate::app().sessions.relock().get(session) {
            terminal.scroll_lines(lines);
        }
    }
}

/// Up to `length` bytes of the file at `path` from byte `from`, and the file's size. A folder
/// is not a file to read.
fn read_piece(path: &std::path::Path, from: u64, length: usize) -> std::io::Result<Piece> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let details = file.metadata()?;
    if !details.is_file() {
        return Err(std::io::Error::other("not a file"));
    }
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.take(length as u64).read_to_end(&mut bytes)?;
    Ok(Piece {
        size: details.len(),
        bytes,
    })
}

/// The relays the person's `settings` choose (`settings::file::PHONE_RELAY`): the one at the
/// address written there, none for "none", and the public ones when nothing is written.
pub fn relays_of(settings: &serde_json::Map<String, serde_json::Value>) -> Relays {
    let written = settings
        .get(crate::settings::file::PHONE_RELAY)
        .and_then(serde_json::Value::as_str)
        .map(str::trim);
    match written {
        None | Some("") => Relays::Public,
        Some("none") => Relays::None,
        Some(address) => Relays::At(address.to_owned()),
    }
}

/// A running phone link. Dropping it ends every device's link: the serving thread sees it is
/// the host's last holder and lets the host go.
pub struct Link {
    host: Arc<Host>,
    service: Arc<Mutex<Service>>,
    /// This computer's name, as a pairing link gives it.
    name: String,
    /// Whether the host listens at a relay, which a pairing link then waits for.
    relayed: bool,
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
        let relayed = relays != Relays::None;
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
            relayed,
        })
    }

    /// The app's own link: its devices are kept in the app's folder and its terminals are the
    /// running sessions.
    pub fn start_for_app() -> Result<Self, northroom_link::Error> {
        let paths = crate::paths::get();
        let settings = crate::settings::file::read(&paths.settings_file).unwrap_or_default();
        Self::start(
            &paths.support,
            relays_of(&settings),
            computer_name(),
            AppDesk,
        )
    }

    /// A pairing link to show as a QR code, good for `pairing::OFFER_LASTS`. It replaces any
    /// shown before.
    pub fn invite(&self) -> String {
        let code = self.service.relock().offer(Instant::now());
        let address = self.address_with_relay();
        Invite {
            key: address.key,
            code,
            host: self.name.clone(),
            relay: address.relay,
            direct: address.direct,
        }
        .link()
    }

    /// Where the host is, waiting up to `RELAY_WAIT` for it to reach its relay just after it
    /// started: a link without the relay only works on this computer's own network.
    fn address_with_relay(&self) -> northroom_link::Address {
        let asked = Instant::now();
        loop {
            let address = self.host.address();
            let ready = !self.relayed || address.relay.is_some();
            if ready || asked.elapsed() >= RELAY_WAIT {
                return address;
            }
            std::thread::sleep(RELAY_ASKED_EVERY);
        }
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
