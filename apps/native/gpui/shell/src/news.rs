//! Wake-ups from outside the UI thread: a picture finished loading, a folder changed on disk
//! (`nr_columns_watch`), or the core answered a request or has news (`nr_transport_open`). Each
//! wakes the window (`kit::wake`), which redraws and first takes in any core news
//! (`Workspace::changed`).
//!
//! Also forwards the core's queued requests to `transport`.

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use kit::nr::*;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Asks the UI thread to draw again.
static WAKE: OnceLock<UnboundedSender<()>> = OnceLock::new();
/// A listed folder changed on disk; the columns relist before the next draw.
static REFRESH: AtomicBool = AtomicBool::new(false);
/// The core has news; its effects are taken on the next draw.
static WOKEN: AtomicBool = AtomicBool::new(false);

fn wake() {
    if let Some(wake) = WAKE.get() {
        let _ = wake.unbounded_send(());
    }
}

/// Installs the window wake-up (`kit::wake`) and registers it with the core. Wake-ups arrive on
/// the returned receiver, which the app reads on the UI thread.
pub fn start() -> UnboundedReceiver<()> {
    let (sender, woken) = unbounded();
    let _ = WAKE.set(sender);
    kit::wake::install(wake);
    nr_transport_open(transport_woken);
    woken
}

/// Watches the shown folders once the drive is open.
pub fn watch() {
    nr_columns_watch(changed_on_disk);
}

/// Called on the core's thread when a folder changes (`nr_columns_watch`).
extern "C" fn changed_on_disk() {
    REFRESH.store(true, Ordering::SeqCst);
    WOKEN.store(true, Ordering::SeqCst);
    wake();
}

/// Called when the core answers or has news (`nr_transport_open`), or a shell request ends
/// (`transport::send`).
extern "C" fn transport_woken() {
    WOKEN.store(true, Ordering::SeqCst);
    wake();
}

/// Whether the core has news since the last call.
pub fn take_news() -> bool {
    WOKEN.swap(false, Ordering::SeqCst)
}

/// Whether folders changed on disk since the last call.
pub fn take_refresh() -> bool {
    REFRESH.swap(false, Ordering::SeqCst)
}

/// Sends the core's queued requests.
pub fn pump_transport() {
    while let Some((id, request)) = transport_next() {
        crate::transport::send(id, request, transport_woken);
    }
}
