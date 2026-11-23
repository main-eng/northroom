//! Live pages and model stills on Windows and Linux, matching `mac/html_page.rs` and
//! `mac/model_still.rs`. The core decides, the platform engine loads and draws, and every call
//! is a future.

use super::engine::Engine;
use super::files::Files;
use super::{Input, WebEvent};
use crate::core::{self, HostError, HostLog};
use crate::picture::Picture;
use northroom_core::page::host::ffi::{
    NR_HOST_NO_PICTURE, NR_HOST_NOT_LOADED, NR_HOST_PAGE_FAILED,
};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

/// A script's result: its text if it returned a string, `None` for any other value, or the
/// exception message.
pub type Said = Result<Option<String>, String>;

/// Options for creating a web view.
pub(crate) struct Setup {
    pub files: Rc<Files>,
    /// Injected into every frame of every document before the page's own scripts.
    pub scripts: Vec<String>,
    /// The view size in CSS pixels, rendered at one pixel per CSS pixel.
    pub width: u32,
    pub height: u32,
    /// Mutes all page audio.
    pub silent: bool,
    /// The core's error code to report if the view cannot be created.
    pub failure: u32,
}

/// The bridge script. `window.__nrPost(message)` posts JSON through `send`, the engine's own
/// messaging function for a top-level document (or null). Subframes post through the top
/// frame, which shares their origin.
pub(crate) fn bridge_script(send: &str) -> String {
    format!(
        "(function () {{ var direct = window.top === window ? ({send}) : null;\
         var send = function (text) {{ if (direct) {{ direct(text); }} else if (window.top !== window && window.top.__nrSend) {{ window.top.__nrSend(text); }} }};\
         Object.defineProperty(window, '__nrSend', {{ value: send }});\
         window.__nrPost = function (message) {{ send(JSON.stringify(message)); }};\
         }})();"
    )
}

type Loaded = dyn Fn(Option<HostError>);

struct PageInner {
    files: Rc<Files>,
    setup: RefCell<Option<Setup>>,
    engine: RefCell<Option<Rc<Engine>>>,
    started: Cell<bool>,
    on_loaded: RefCell<Option<Box<Loaded>>>,
    /// The size requested before the view exists.
    size: Cell<(u32, u32)>,
}

impl PageInner {
    fn loaded(&self, result: Option<HostError>) {
        if let Some(loaded) = self.on_loaded.borrow().as_ref() {
            loaded(result);
        }
    }

    fn reload(&self) {
        if let Some(engine) = self.engine.borrow().as_ref() {
            engine.reload();
        }
    }
}

/// A drive HTML file run as a live page, like `mac/html_page.rs`: its folder served over the
/// page host's scheme, the network blocked, no state kept between openings. A shell draws its
/// snapshots (`picture`) and forwards the user's input (`input`).
pub struct HtmlPage {
    inner: Rc<PageInner>,
    /// The HTML file, and below it the folder it is served from.
    pub file: PathBuf,
    pub folder: PathBuf,
}

impl HtmlPage {
    pub fn new(file: &Path) -> Self {
        let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let files = Files::new(core::HostCore::page(&file.to_string_lossy()));
        // Placeholder until the showing view is laid out.
        let size = (800, 600);
        let setup = Setup {
            files: files.clone(),
            scripts: vec![core::page_errors_script()],
            width: size.0,
            height: size.1,
            silent: false,
            failure: NR_HOST_PAGE_FAILED,
        };
        Self {
            inner: Rc::new(PageInner {
                files,
                setup: RefCell::new(Some(setup)),
                engine: RefCell::new(None),
                started: Cell::new(false),
                on_loaded: RefCell::new(None),
                size: Cell::new(size),
            }),
            file: file.to_path_buf(),
            folder,
        }
    }

    pub fn on_loaded(&self, loaded: impl Fn(Option<HostError>) + 'static) {
        *self.inner.on_loaded.borrow_mut() = Some(Box::new(loaded));
    }

    /// Creates the web view with the network blocked, then loads the page.
    pub async fn start(&self) {
        if self.inner.started.replace(true) {
            return;
        }
        let Some(mut setup) = self.inner.setup.borrow_mut().take() else {
            return;
        };
        (setup.width, setup.height) = self.inner.size.get();
        let weak: Weak<PageInner> = Rc::downgrade(&self.inner);
        let opened = Engine::open(
            setup,
            Box::new(move |event| {
                let Some(inner) = weak.upgrade() else { return };
                match event {
                    WebEvent::Loaded => inner.loaded(None),
                    WebEvent::Failed(why) => {
                        inner.loaded(Some(HostError::of(NR_HOST_PAGE_FAILED, &why)))
                    }
                    // Reload after a crash when the core says live pages do.
                    WebEvent::Ended => {
                        if inner.files.core.crash_reloads() {
                            inner.reload();
                        }
                    }
                    WebEvent::Message(body) => drop(inner.files.received(&body)),
                }
            }),
        )
        .await;
        match opened {
            Err(error) => self.inner.loaded(Some(error)),
            Ok(engine) => {
                let (width, height) = self.inner.size.get();
                engine.resize(width, height);
                engine.load(&self.inner.files.entry());
                *self.inner.engine.borrow_mut() = Some(engine);
            }
        }
    }

    /// Reloads the page from its current files.
    pub fn reload(&self) {
        self.inner.reload();
    }

    pub fn log(&self) -> &HostLog {
        &self.inner.files.log
    }

    /// Folder-relative paths of the files the page has read so far.
    pub fn served(&self) -> std::collections::BTreeSet<String> {
        self.inner.files.log.served()
    }

    /// Evaluates `script` in the page.
    pub async fn evaluate(&self, script: &str) -> Said {
        let engine = self.inner.engine.borrow().clone();
        match engine {
            Some(engine) => engine.evaluate(script).await,
            None => Err(HostError::of(NR_HOST_NOT_LOADED, "").to_string()),
        }
    }

    /// Whether a database the page queried (`window.__nr.sql`) was changed by someone else since.
    /// Reports each change once, for `tell_data_changed`.
    pub fn data_changed(&self) -> bool {
        self.inner.files.core.data_changed()
    }

    /// Tells the page a database it queried changed by firing `nr:change` on its window.
    pub async fn tell_data_changed(&self) {
        let _ = self.evaluate(&core::data_changed_call()).await;
    }

    /// Lays the page out at `width` × `height` CSS pixels, the size of the showing view.
    pub fn resize(&self, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if self.inner.size.replace(size) == size {
            return;
        }
        if let Some(engine) = self.inner.engine.borrow().as_ref() {
            engine.resize(size.0, size.1);
        }
    }

    /// Snapshots the page at one pixel per CSS pixel.
    pub async fn picture(&self) -> Result<Picture, HostError> {
        let engine = self.inner.engine.borrow().clone();
        match engine {
            Some(engine) => engine
                .snapshot()
                .await
                .map_err(|why| HostError::of(NR_HOST_NO_PICTURE, &why)),
            None => Err(HostError::of(NR_HOST_NOT_LOADED, "")),
        }
    }

    /// Forwards a pointer or key event to the page.
    pub fn input(&self, input: &Input) {
        if let Some(engine) = self.inner.engine.borrow().as_ref() {
            engine.input(input);
        }
    }
}

impl Drop for HtmlPage {
    fn drop(&mut self) {
        if let Some(engine) = self.inner.engine.borrow_mut().take() {
            engine.close();
        }
    }
}

pub mod model_still {
    //! Stills of 3D models (`.glb`), drawn one at a time in a hidden web view by the core's page
    //! with the bundled three.js and no network (`mac/model_still.rs`, `still_page`).

    use super::{Engine, Files, Setup, WebEvent};
    use crate::core::{self, HostCore, HostError};
    use crate::run::{Once, promise};
    use crate::still_page::{Drawn, Waiting, read_posted, time_out};
    use northroom_core::page::host::ffi::{NR_HOST_STAGE_FAILED, NR_HOST_STILL_STOPPED};
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::Arc;

    /// One web view that draws models' stills, one at a time.
    pub struct StillDrawer {
        core: Arc<HostCore>,
        engine: Rc<Engine>,
        waiting: Rc<RefCell<Waiting>>,
    }

    impl StillDrawer {
        /// Opens a drawer once its page has loaded three.js, or fails if it does not start in
        /// time.
        pub async fn open() -> Result<Self, HostError> {
            let core = HostCore::stills();
            let files = Files::new(core.clone());
            let side = core::still_stage();
            let waiting = Rc::new(RefCell::new(Waiting::default()));
            let heard = waiting.clone();
            let engine = Engine::open(
                Setup {
                    files: files.clone(),
                    scripts: Vec::new(),
                    width: side,
                    height: side,
                    silent: true,
                    failure: NR_HOST_STAGE_FAILED,
                },
                Box::new(move |event| match event {
                    WebEvent::Message(body) => {
                        if let Some(said) = read_posted(&body) {
                            heard.borrow_mut().heard(said);
                        }
                    }
                    WebEvent::Ended => heard.borrow_mut().ended(),
                    WebEvent::Loaded | WebEvent::Failed(_) => {}
                }),
            )
            .await?;
            let (resolver, opened) = promise();
            let opening = Rc::new(Once::new(resolver));
            waiting.borrow_mut().opening = Some(opening.clone());
            engine.load(&files.entry());
            time_out(opening);
            let drawer = Self {
                core,
                engine,
                waiting,
            };
            match opened.await {
                Ok(()) => Ok(drawer),
                Err(error) => {
                    drawer.close();
                    Err(error)
                }
            }
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
            drop(self.engine.evaluate(&call));
            time_out(done);
            drawn.await
        }

        /// Releases the web view and its window.
        pub fn close(&self) {
            self.engine.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bridge_posts_through_the_top_document() {
        let script = bridge_script("null");
        assert!(script.contains("window.__nrPost = function (message)"));
        assert!(script.contains("window.top.__nrSend(text)"));
    }
}
