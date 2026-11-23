//! Stills of 3D models for the drive views. The core decides which models need stills and
//! stores each as a PNG (`core/src/model_still.rs`). This renders them one at a time in a hidden
//! web view (`webhost::host::model_still::StillDrawer`), hands each PNG to the core and redraws
//! the windows. Views read stills as picture files off the UI thread
//! (`kit::lists::thumbs::picture_file`).
//!
//! The drawer stays warm (three.js loaded, scene built) while stills are wanted and is dropped
//! after `IDLE` with nothing to do. A drawer that fails is dropped at once and the next model
//! gets a fresh one.

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::future::{Either, select};
use gpui::{App, AsyncApp};
use northroom_core::model_still::{self, Version};
use std::sync::OnceLock;
use std::time::Duration;
use webhost::host::model_still::StillDrawer;

/// How long an idle drawer is kept.
const IDLE: Duration = Duration::from_secs(5);

/// Wakes the drawing loop when a still is wanted or has changed.
static POKE: OnceLock<UnboundedSender<()>> = OnceLock::new();

fn poke() {
    if let Some(poke) = POKE.get() {
        let _ = poke.unbounded_send(());
    }
}

/// Starts the loop that draws stills the views ask for, for the life of the app.
pub fn start(cx: &mut App) {
    let (sender, poked) = unbounded();
    let _ = POKE.set(sender);
    if !model_still::start(poke) {
        return;
    }
    cx.spawn(async move |cx| draw_as_wanted(poked, cx).await)
        .detach();
}

/// Draws each wanted still in turn and redraws the windows when one changes.
async fn draw_as_wanted(mut poked: UnboundedReceiver<()>, cx: &mut AsyncApp) {
    let mut warm: Option<StillDrawer> = None;
    loop {
        if model_still::take_changed() {
            kit::wake::wake();
        }
        if let Some(version) = model_still::next_wanted() {
            warm = draw(warm, &version).await;
            continue;
        }
        match wait(&mut poked, warm, cx).await {
            Some(kept) => warm = kept,
            None => return,
        }
    }
}

/// Draws the still of `version` with `warm`, or a new drawer, and returns the drawer if it is
/// still usable. A failure is reported to the core, which stops asking for that file until it
/// changes.
async fn draw(warm: Option<StillDrawer>, version: &Version) -> Option<StillDrawer> {
    let drawer = match warm {
        Some(drawer) => drawer,
        None => match StillDrawer::open().await {
            Ok(opened) => opened,
            Err(error) => {
                model_still::failed(version, &error.to_string());
                return None;
            }
        },
    };
    match drawer.draw(&version.path).await {
        Ok(png) => {
            model_still::drawn(version, &png);
            Some(drawer)
        }
        Err(error) => {
            model_still::failed(version, &error.to_string());
            drawer.close();
            None
        }
    }
}

/// Waits for a poke, dropping the warm drawer if none comes within `IDLE`. Returns the drawer
/// still warm, or `None` when nothing will poke again.
async fn wait(
    poked: &mut UnboundedReceiver<()>,
    warm: Option<StillDrawer>,
    cx: &AsyncApp,
) -> Option<Option<StillDrawer>> {
    let Some(drawer) = warm else {
        return poked.next().await.map(|()| None);
    };
    let idle = cx.background_executor().timer(IDLE);
    match select(poked.next(), idle).await {
        Either::Left((Some(()), _)) => Some(Some(drawer)),
        Either::Left((None, _)) => {
            drawer.close();
            None
        }
        Either::Right(_) => {
            drawer.close();
            Some(None)
        }
    }
}
