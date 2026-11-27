//! The app's one phone link (ADR-0016): started at launch when a phone is paired, and by
//! Connect Phone otherwise, so a computer nobody paired listens to nothing.

use crate::remote::devices::{parse_key_hex, Devices, Key, FILE};
use crate::remote::run::Link;
use crate::Relock;
use std::sync::Mutex;

static LINK: Mutex<Option<Link>> = Mutex::new(None);

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

/// The paired phones, each its key and the name it gave. None while the link does not run: it
/// runs whenever a phone is paired.
pub fn devices() -> Vec<(Key, String)> {
    let link = LINK.relock();
    let Some(link) = link.as_ref() else {
        return Vec::new();
    };
    link.devices()
        .into_iter()
        .filter_map(|(key, name)| Some((parse_key_hex(&key)?, name)))
        .collect()
}

/// Forgets the paired phone with `key` and ends its links. Its error is in words.
pub fn forget(key: &Key) -> Result<(), String> {
    let link = LINK.relock();
    let Some(link) = link.as_ref() else {
        return Ok(());
    };
    link.forget_for_app(key)
        .map_err(|failed| failed.to_string())
}

/// Starts the link at launch when a phone is paired, so it can reconnect.
#[no_mangle]
pub extern "C" fn nr_phone_link_start() {
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
