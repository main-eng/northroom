//! Runs a drive HTML file as a live page in the system web view, the way a browser would. Its
//! scripts run and its folder (with subfolders) is served over the page host's scheme. Nothing
//! outside that folder is served, network requests are blocked and logged, and the page keeps no
//! state between openings. The core decides what is served, where the page may navigate, what
//! autoplays and what happens when its process ends (`core/src/page/host`); this module builds the
//! web view and carries those decisions out.

use super::web::{self, Files, WebEvent, WebEvents};
use crate::core::{self, HostError, HostLog};
use northroom_core::page::host::ffi::NR_HOST_PAGE_FAILED;
use objc2::rc::Retained;
use objc2_foundation::{NSURLRequest, NSURLRequestCachePolicy};
use objc2_web_kit::WKWebView;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

type Loaded = dyn Fn(Option<HostError>);

struct Inner {
    files: Rc<Files>,
    web_view: Retained<WKWebView>,
    events: Retained<WebEvents>,
    started: Cell<bool>,
    /// Called when the page loads, with the error if loading failed.
    on_loaded: RefCell<Option<Box<Loaded>>>,
}

impl Inner {
    fn loaded(&self, result: Option<HostError>) {
        self.web_view.setAlphaValue(1.0);
        if let Some(loaded) = self.on_loaded.borrow().as_ref() {
            loaded(result);
        }
    }

    fn reload(&self) {
        if self.started.get() {
            let request = NSURLRequest::requestWithURL_cachePolicy_timeoutInterval(
                &self.files.entry(),
                NSURLRequestCachePolicy::ReloadIgnoringLocalAndRemoteCacheData,
                60.0,
            );
            // SAFETY: plain WebKit calls on the main thread.
            unsafe { self.web_view.loadRequest(&request) };
        }
    }
}

pub struct HtmlPage {
    inner: Rc<Inner>,
    /// The HTML file, and below it the folder it is served from.
    pub file: PathBuf,
    pub folder: PathBuf,
}

impl HtmlPage {
    pub fn new(file: &Path) -> Self {
        let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let files = Files::new(core::HostCore::page(&file.to_string_lossy()));
        let events = WebEvents::new(files.clone());
        // Record script errors in the host log.
        let configuration = web::configuration(&files, &[core::page_errors_script()], &events);
        let web_view = web::web_view(
            &configuration,
            super::pixels::rect(0.0, 0.0, 0.0, 0.0),
            &events,
        );
        // Transparent until the page loads, so the parent view's background shows instead of a
        // blank page. Use alpha, not `hidden`: WebKit does not render a hidden view's page.
        web_view.setAlphaValue(0.0);
        let inner = Rc::new(Inner {
            files,
            web_view,
            events,
            started: Cell::new(false),
            on_loaded: RefCell::new(None),
        });
        let weak: Weak<Inner> = Rc::downgrade(&inner);
        inner.events.set_handler(move |event| {
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
        });
        Self {
            inner,
            file: file.to_path_buf(),
            folder,
        }
    }

    pub fn on_loaded(&self, loaded: impl Fn(Option<HostError>) + 'static) {
        *self.inner.on_loaded.borrow_mut() = Some(Box::new(loaded));
    }

    /// Blocks the network, then loads the page.
    pub async fn start(&self) {
        if self.inner.started.replace(true) {
            return;
        }
        match web::network_block().await {
            Err(error) => self.inner.loaded(Some(error)),
            Ok(list) => {
                // SAFETY: plain WebKit calls on the main thread.
                unsafe {
                    self.inner
                        .web_view
                        .configuration()
                        .userContentController()
                        .addContentRuleList(&list)
                };
                web::load(&self.inner.web_view, &self.inner.files.entry());
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

    pub fn web_view(&self) -> &WKWebView {
        &self.inner.web_view
    }

    /// The web view as a raw AppKit pointer, for a window that embeds it in its own views.
    pub fn view(&self) -> *mut std::ffi::c_void {
        Retained::as_ptr(&self.inner.web_view).cast_mut().cast()
    }

    /// Evaluates `script` in the page.
    pub async fn evaluate(&self, script: &str) -> web::Said {
        web::evaluate(&self.inner.web_view, script).await
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
}
