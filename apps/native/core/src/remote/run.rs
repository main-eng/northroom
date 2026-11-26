//! The phone link while the app runs: one thread that takes what devices send, answers by the
//! rules (`service`) against the app's own terminals, and a few times a second sends each device
//! what changed.

use crate::deck::{Deck, Kind};
use crate::remote::devices::{Devices, Key, FILE};
use crate::remote::pages::{Drawn, Pages};
use crate::remote::pairing::Invite;
use crate::remote::protocol::{Item, Listing, Piece, Pressed};
use crate::remote::screen::Screen;
use crate::remote::service::{Desk, Out, Service};
use crate::remote::tables::{Head, Row};
use crate::sessions::Running;
use crate::sheets::layout::Table;
use crate::terminal::cells::GridSize;
use crate::terminal::input::Modifiers;
use crate::Relock;
use northroom_link::{Event, Host, Relays, Secret};
use std::path::{Path, PathBuf};
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

/// A file's size and when it last changed, which say whether what was read of it still holds.
type Version = (u64, Option<std::time::SystemTime>);

fn version_of(path: &Path) -> Option<Version> {
    let details = std::fs::metadata(path).ok()?;
    Some((details.len(), details.modified().ok()))
}

/// The file last read as pages for a device, kept so each page asked for reads it once per
/// version.
struct Opened {
    path: PathBuf,
    version: Version,
    deck: Arc<Deck>,
}

/// The picture last read for a device, kept while its bytes are sent a piece at a time.
struct PictureRead {
    path: PathBuf,
    version: Version,
    picture: usize,
    bytes: Arc<Vec<u8>>,
}

/// The file last read as a table for a device, kept so each window of rows asked for reads it
/// once per version.
struct TableRead {
    path: PathBuf,
    version: Version,
    table: Arc<Table>,
}

/// The app's running terminals (`App::sessions`) and its drive's files.
#[derive(Default)]
struct AppDesk {
    opened: Mutex<Option<Opened>>,
    picture: Mutex<Option<PictureRead>>,
    table: Mutex<Option<TableRead>>,
}

/// The path of the drive's file `file`.
fn path_of(file: &str) -> Option<PathBuf> {
    crate::ffi::with_columns(|columns| columns.workspace().path_of(file))?.ok()
}

/// Reads the file at `path` as pages when its name and contents say it is a kind shown as
/// them, and it is no larger than that kind is drawn.
fn read_deck(path: &Path, version: Version) -> Option<Deck> {
    let kind = Kind::of_name(&path.file_name()?.to_string_lossy())?;
    if !kind.claims(path) || version.0 > kind.max_bytes() {
        return None;
    }
    match crate::deck::read(path, kind, &mut crate::text::typeset::width) {
        Ok(deck) => Some(deck),
        Err(failure) => {
            eprintln!("a phone's file was not read as pages: {failure}");
            None
        }
    }
}

impl AppDesk {
    /// The drive's file `file` as pages, and its path.
    fn deck_of(&self, file: &str) -> Option<(PathBuf, Version, Arc<Deck>)> {
        let path = path_of(file)?;
        let version = version_of(&path)?;
        let mut opened = self.opened.relock();
        if let Some(known) = opened.as_ref() {
            if known.path == path && known.version == version {
                return Some((path, version, Arc::clone(&known.deck)));
            }
        }
        let deck = Arc::new(read_deck(&path, version)?);
        *opened = Some(Opened {
            path: path.clone(),
            version,
            deck: Arc::clone(&deck),
        });
        Some((path, version, deck))
    }

    /// The drive's file `file` as a table, and its path.
    fn table_of(&self, file: &str) -> Option<(PathBuf, Arc<Table>)> {
        let path = path_of(file)?;
        let version = version_of(&path)?;
        let mut read = self.table.relock();
        if let Some(known) = read.as_ref() {
            if known.path == path && known.version == version {
                return Some((path, Arc::clone(&known.table)));
            }
        }
        let table = Arc::new(crate::remote::tables::read(&path, version.0)?);
        *read = Some(TableRead {
            path: path.clone(),
            version,
            table: Arc::clone(&table),
        });
        Some((path, table))
    }

    /// The bytes of the picture numbered `picture` on `file`'s pages.
    fn picture_of(&self, file: &str, picture: usize) -> Option<Arc<Vec<u8>>> {
        let (path, version, deck) = self.deck_of(file)?;
        let mut read = self.picture.relock();
        if let Some(known) = read.as_ref() {
            if known.path == path && known.version == version && known.picture == picture {
                return Some(Arc::clone(&known.bytes));
            }
        }
        let bytes = match deck.pictures.get(picture)?.read(&path) {
            Ok(bytes) => Arc::new(bytes),
            Err(failure) => {
                eprintln!("a phone's picture was not read: {failure}");
                return None;
            }
        };
        *read = Some(PictureRead {
            path,
            version,
            picture,
            bytes: Arc::clone(&bytes),
        });
        Some(bytes)
    }
}

/// Up to `length` of `bytes` from byte `from`.
fn piece_of(bytes: &[u8], from: u64, length: usize) -> Piece {
    let start = usize::try_from(from).map_or(bytes.len(), |from| from.min(bytes.len()));
    let end = start.saturating_add(length).min(bytes.len());
    Piece {
        size: bytes.len() as u64,
        bytes: bytes[start..end].to_vec(),
    }
}

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

    fn table(&self, file: &str) -> Option<Vec<Head>> {
        let (_, table) = self.table_of(file)?;
        crate::remote::tables::heads(&table)
    }

    fn rows(&self, file: &str, sheet: usize, from: usize, count: usize) -> Option<Vec<Row>> {
        let (path, table) = self.table_of(file)?;
        match table.as_ref() {
            Table::Sheets(workbook) => {
                let sheet = workbook.sheets.get(sheet)?;
                Some(crate::remote::tables::rows_held(sheet, from, count))
            }
            // A database's rows are read as they are asked for.
            Table::Database(_) => {
                let rows = crate::sheets::database::rows(&path, sheet, from..from + count);
                Some(
                    rows.iter()
                        .map(|row| crate::remote::tables::row_of(row))
                        .collect(),
                )
            }
            Table::Unreadable(_) => None,
        }
    }

    fn pages(&self, file: &str) -> Option<Pages> {
        let (_, _, deck) = self.deck_of(file)?;
        Some(Pages::of(&deck))
    }

    fn marks(&self, file: &str, page: usize, width: f64) -> Option<Vec<Drawn>> {
        let (_, _, deck) = self.deck_of(file)?;
        Some(crate::remote::pages::marks(&deck, page, width))
    }

    fn read(&self, file: &str, picture: Option<usize>, from: u64, length: usize) -> Option<Piece> {
        if let Some(picture) = picture {
            let bytes = self.picture_of(file, picture)?;
            return Some(piece_of(&bytes, from, length));
        }
        let path = path_of(file)?;
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
fn read_piece(path: &Path, from: u64, length: usize) -> std::io::Result<Piece> {
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
        folder: &Path,
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
            AppDesk::default(),
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

    /// Forgets the device with `key` among the app's own, giving a terminal it watched its size
    /// back.
    pub fn forget_for_app(&self, key: &Key) -> std::io::Result<()> {
        self.forget(key, &AppDesk::default())
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

/// Answers `event` by the rules, and tells the app when a device paired, so its menu lists it.
fn answer(service: &Mutex<Service>, event: Event, desk: &dyn Desk) -> Vec<Out> {
    let mut service = service.relock();
    let paired_before = service.devices().list().len();
    let out = answer_by_rules(&mut service, event, desk);
    if service.devices().list().len() != paired_before {
        crate::notify_changed();
    }
    out
}

fn answer_by_rules(service: &mut Service, event: Event, desk: &dyn Desk) -> Vec<Out> {
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
