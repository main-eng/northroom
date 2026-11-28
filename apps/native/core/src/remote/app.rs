//! The app's one link for devices (ADR-0016): started at launch when a phone or another
//! computer is paired, and by Connect Device otherwise, so a computer nobody paired listens to
//! nothing. With it, the app's links out to the computers this one paired with (`reach`).

use crate::remote::devices::{parse_key_hex, Devices, Key, FILE};
use crate::remote::reach::{Events, Reach, Seen};
use crate::remote::run::Link;
use crate::terminal::{Palette, Terminal, ViewSize};
use crate::Relock;
use std::sync::{Arc, Mutex};

static LINK: Mutex<Option<Link>> = Mutex::new(None);
static REACH: Mutex<Option<Arc<Reach>>> = Mutex::new(None);

/// The links to the computers this one paired with, opened when first needed.
fn reach() -> Arc<Reach> {
    let mut reach = REACH.relock();
    Arc::clone(reach.get_or_insert_with(|| {
        let events = Events {
            changed: Box::new(crate::notify_changed),
            started: Box::new(crate::ffi::show_far_session),
        };
        let folder = crate::paths::get().support;
        Arc::new(Reach::open(
            &folder,
            crate::remote::run::computer_name(),
            events,
        ))
    }))
}

/// The links, if they were opened: a computer that paired with none has none to open.
fn reach_if_open() -> Option<Arc<Reach>> {
    REACH.relock().clone()
}

/// The computers this one paired with, and the sessions each lists.
pub fn computers() -> Vec<Seen> {
    reach_if_open().map_or_else(Vec::new, |reach| reach.computers())
}

/// Pairs with the computer that showed the pairing `link`. False when the text is not one.
pub fn pair_with(link: &str) -> bool {
    reach().pair(link)
}

/// Asks the computer with `key` to start a session, which then opens here.
pub fn start_far(key: &str) {
    if let Some(reach) = reach_if_open() {
        reach.start(key);
    }
}

/// A terminal here that shows `session` of the paired computer with `key`.
pub fn show_far(key: &str, session: &str, view: ViewSize, palette: Palette) -> Option<Terminal> {
    reach_if_open()?.show(key, session, view, palette)
}

/// Starts the link unless it runs. Its error is in words for a notice.
fn started(link: &mut Option<Link>) -> Result<&Link, String> {
    if link.is_none() {
        *link = Some(Link::start_for_app().map_err(|failed| failed.to_string())?);
    }
    link.as_ref().ok_or_else(String::new)
}

/// Whether a phone was ever paired with this computer.
fn has_devices() -> bool {
    let file = crate::paths::get().support.join(FILE);
    !Devices::open(&file, || [0; 32]).list().is_empty()
}

/// Everything paired with this computer, each its key and name: the phones and computers that
/// connect to it, then the computers it connects to. The link for the first runs whenever one
/// is paired.
pub fn devices() -> Vec<(Key, String)> {
    let connecting = LINK
        .relock()
        .as_ref()
        .map_or_else(Vec::new, Link::devices)
        .into_iter();
    let connected_to = computers()
        .into_iter()
        .map(|computer| (computer.key, computer.name));
    connecting
        .chain(connected_to)
        .filter_map(|(key, name)| Some((parse_key_hex(&key)?, name)))
        .collect()
}

/// Forgets what is paired under `key`, a device of this computer or a computer it connects
/// to, and ends its links. Its error is in words.
pub fn forget(key: &Key) -> Result<(), String> {
    if let Some(reach) = reach_if_open() {
        reach.forget(&crate::remote::devices::key_hex(key));
    }
    let link = LINK.relock();
    let Some(link) = link.as_ref() else {
        return Ok(());
    };
    link.forget_for_app(key)
        .map_err(|failed| failed.to_string())
}

/// Starts the links at launch: the one devices connect to when one is paired, so it can
/// reconnect, and the ones out to the computers this one paired with.
#[no_mangle]
pub extern "C" fn nr_phone_link_start() {
    if Reach::keeps_any(&crate::paths::get().support) {
        reach();
    }
    if !has_devices() {
        return;
    }
    if let Err(failed) = started(&mut LINK.relock()) {
        eprintln!("phone link\tdid not start: {failed}");
    }
}

/// A fresh pairing link to show, starting the link if it does not run.
pub fn invite() -> Result<String, String> {
    Ok(started(&mut LINK.relock())?.invite())
}

/// The share of its time the pairing link on screen still has. Nothing once a phone has used
/// it or it has gone stale.
pub fn invite_left() -> f64 {
    LINK.relock().as_ref().map_or(0.0, Link::invite_left)
}

/// Takes the pairing link off screen.
pub fn withdraw_invite() {
    if let Some(link) = LINK.relock().as_ref() {
        link.withdraw_invite();
    }
}
