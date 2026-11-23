//! Stills of 3D models (`.glb`), drawn one at a time in a hidden web view with the host's bundled
//! three.js and no network. The core serves the drawing page (`model-still.html`), which loads
//! three.js and sets up its scene once, then each model (`HostCore::draw_model`). The page frames,
//! lights and crops each still and posts it back as a PNG.

use super::pixels;
use super::web::{self, Files, StageWindow, WebEvent, WebEvents};
use crate::core::{self, HostCore, HostError};
use crate::run::{Once, promise};
use crate::still_page::{Drawn, Waiting, read_posted, time_out};
use northroom_core::page::host::ffi::NR_HOST_STILL_STOPPED;
use objc2::rc::Retained;
use objc2_web_kit::WKWebView;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

/// One web view that draws models' stills, one at a time.
pub struct StillDrawer {
    core: Arc<HostCore>,
    web_view: Retained<WKWebView>,
    window: Retained<StageWindow>,
    waiting: Rc<RefCell<Waiting>>,
    /// Kept alive while the web view uses them.
    _files: Rc<Files>,
    _events: Retained<WebEvents>,
}

impl StillDrawer {
    /// Opens a drawer once its page has loaded three.js, or fails if it does not start in time.
    pub async fn open() -> Result<Self, HostError> {
        let core = HostCore::stills();
        let files = Files::new(core.clone());
        let events = WebEvents::new(files.clone());
        let configuration = web::configuration(&files, &[], &events);
        let side = f64::from(core::still_stage());
        let frame = pixels::rect(0.0, 0.0, side, side);
        let web_view = web::web_view(&configuration, frame, &events);
        // WebKit only runs frames for a view it thinks is visible, so host it in an offscreen
        // window, as the stage does.
        let window = StageWindow::offscreen(frame, true, &web_view);
        let waiting = Rc::new(RefCell::new(Waiting::default()));
        let heard = waiting.clone();
        events.set_handler(move |event| match event {
            WebEvent::Message(body) => {
                if let Some(said) = read_posted(&body) {
                    heard.borrow_mut().heard(said);
                }
            }
            WebEvent::Ended => heard.borrow_mut().ended(),
            WebEvent::Loaded | WebEvent::Failed(_) => {}
        });
        let drawer = Self {
            core,
            web_view,
            window,
            waiting,
            _files: files.clone(),
            _events: events,
        };
        match drawer.load(&files).await {
            Ok(()) => Ok(drawer),
            Err(error) => {
                drawer.close();
                Err(error)
            }
        }
    }

    /// Loads the page with the network blocked and waits for it to report ready.
    async fn load(&self, files: &Files) -> Result<(), HostError> {
        // The page and models come from the host's scheme; WebKit blocks every other request.
        let list = web::network_block().await?;
        // SAFETY: plain WebKit calls on the main thread.
        unsafe {
            self.web_view
                .configuration()
                .userContentController()
                .addContentRuleList(&list)
        };
        let (resolver, opened) = promise();
        let opening = Rc::new(Once::new(resolver));
        self.waiting.borrow_mut().opening = Some(opening.clone());
        web::load(&self.web_view, &files.entry());
        time_out(opening);
        opened.await
    }

    /// Draws the still of the model at `file` and returns its PNG.
    pub async fn draw(&self, file: &Path) -> Result<Vec<u8>, HostError> {
        if self.waiting.borrow().ended {
            return Err(HostError::of(NR_HOST_STILL_STOPPED, ""));
        }
        let (call, turn) = self.core.draw_model(&file.to_string_lossy());
        let (resolver, drawn) = promise();
        let done: Drawn = Rc::new(Once::new(resolver));
        self.waiting.borrow_mut().drawing = Some((turn, done.clone()));
        // The call returns nothing; the page posts the still when it is drawn.
        drop(web::evaluate(&self.web_view, &call));
        time_out(done);
        drawn.await
    }

    /// Releases the web view and closes its window.
    pub fn close(&self) {
        web::remove_bridge(&self.web_view);
        self.window.orderOut(None);
        self.window.close();
    }
}
