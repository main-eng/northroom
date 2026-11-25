//! The app's one phone link (ADR-0016): started at launch when a phone is paired, and by
//! Connect Phone otherwise, so a computer nobody paired listens to nothing.

use crate::remote::devices::{Devices, FILE};
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

/// Takes the pairing link off screen.
pub fn withdraw_invite() {
    if let Some(link) = LINK.relock().as_ref() {
        link.withdraw_invite();
    }
}
