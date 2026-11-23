//! Lets work off the UI thread ask the window to redraw. The app installs its wake-up once at
//! launch and anything in the kit calls `wake`. Until one is installed, as in the gallery, `wake`
//! does nothing.

use std::sync::OnceLock;

static WAKE: OnceLock<fn()> = OnceLock::new();

/// Sets the function that wakes the window. Call once, at launch.
pub fn install(wake: fn()) {
    let _ = WAKE.set(wake);
}

/// Asks the window to draw again; callable from any thread.
pub fn wake() {
    if let Some(wake) = WAKE.get() {
        wake();
    }
}
