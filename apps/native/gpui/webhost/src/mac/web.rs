//! Shared Mac web view plumbing. The core answers the page host's scheme, decides navigations and
//! reads page messages; the core's block list closes the network; scripts run as futures; and
//! `StageWindow` hosts views offscreen.

use crate::core::{self, HostCore, HostLog, Reply};
use crate::run::{Once, Promise, Resolver, promise};
use block2::RcBlock;
use northroom_core::page::host::ffi::{NR_HOST_RULES_FAILED, NR_HOST_RULES_STORED};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_app_kit::{
    NSBackingStoreType, NSWindow, NSWindowCollectionBehavior, NSWindowOcclusionState,
    NSWindowStyleMask,
};
use objc2_core_foundation::CGRect;
use objc2_foundation::{
    NSData, NSDictionary, NSError, NSHTTPURLResponse, NSObjectProtocol, NSPoint, NSString, NSURL,
    NSURLRequest, ns_string,
};
use objc2_web_kit::{
    WKAudiovisualMediaTypes, WKContentRuleList, WKContentRuleListStore, WKNavigation,
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKScriptMessage,
    WKScriptMessageHandler, WKURLSchemeHandler, WKURLSchemeTask, WKUserContentController,
    WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::{Arc, LazyLock, Mutex, mpsc};

pub fn main_thread() -> MainThreadMarker {
    MainThreadMarker::new().expect("the page host runs on the main thread")
}

/// The script message handler pages post to; `window.__nrPost(message)` sends JSON to it.
const BRIDGE: &str = "nr";
const BRIDGE_SCRIPT: &str = "window.__nrPost = function (message) { window.webkit.messageHandlers.nr.postMessage(JSON.stringify(message)); };";

/// A pending scheme task.
type Reading = Retained<ProtocolObject<dyn WKURLSchemeTask>>;

/// Serves `nr://files/…` through the core, which decides every answer; this reads and sends what
/// it says. Files are read and pages rewritten off the main thread, which is busy taking every
/// picture a web view draws.
pub struct Files {
    pub core: Arc<HostCore>,
    pub log: HostLog,
    /// Pending tasks by number. A task WebKit stops is removed and never answered.
    reading: RefCell<HashMap<u64, Reading>>,
    next: Cell<u64>,
    id: u64,
}

thread_local! {
    /// Live `Files` by id, so a finished read can find its owner.
    static FILES: RefCell<HashMap<u64, Weak<Files>>> = RefCell::new(HashMap::new());
    static FILES_MADE: Cell<u64> = const { Cell::new(0) };
}

/// A request for the reader thread, and where its reply goes.
struct Read {
    core: Arc<HostCore>,
    address: String,
    range: String,
    files: u64,
    task: u64,
}

/// The single thread that reads files, in request order.
static READER: LazyLock<Mutex<mpsc::Sender<Read>>> = LazyLock::new(|| {
    let (sender, reads) = mpsc::channel::<Read>();
    std::thread::Builder::new()
        .name("northroom-scene-files".into())
        .spawn(move || {
            for read in reads {
                let reply = read.core.answer(&read.address, &read.range);
                let (files, task) = (read.files, read.task);
                dispatch2::DispatchQueue::main().exec_async(move || read_back(files, task, reply));
            }
        })
        .expect("the reading thread starts");
    Mutex::new(sender)
});

fn read_back(files: u64, task: u64, reply: Reply) {
    if let Some(files) = FILES.with(|all| all.borrow().get(&files).and_then(Weak::upgrade)) {
        files.answer(task, reply);
    }
}

impl Files {
    pub fn new(core: Arc<HostCore>) -> Rc<Self> {
        let id = FILES_MADE.with(|made| {
            made.set(made.get() + 1);
            made.get()
        });
        let files = Rc::new(Self {
            log: HostLog::new(core.clone()),
            core,
            reading: RefCell::new(HashMap::new()),
            next: Cell::new(0),
            id,
        });
        FILES.with(|all| all.borrow_mut().insert(id, Rc::downgrade(&files)));
        files
    }

    /// The address the web view loads first.
    pub fn entry(&self) -> Retained<NSURL> {
        NSURL::URLWithString(&NSString::from_str(&self.core.entry()))
            .expect("the core's entry is an address")
    }

    /// Whether the web view may follow `action`. Blocked navigations are logged.
    fn allows(&self, action: &WKNavigationAction) -> bool {
        // SAFETY: a live navigation action, on the main thread.
        let (address, main_frame, download) = unsafe {
            let address = action
                .request()
                .URL()
                .and_then(|url| url.absoluteString())
                .map(|text| text.to_string())
                .unwrap_or_default();
            (
                address,
                action.targetFrame().is_none_or(|frame| frame.isMainFrame()),
                action.shouldPerformDownload(),
            )
        };
        let allowed = self.core.navigate(&address, main_frame, download);
        self.log.deliver();
        allowed
    }

    /// Handles a message posted through the bridge, returning the page a `reloaded` message names.
    pub fn received(&self, body: &str) -> Option<String> {
        let reloaded = self.core.page_message(body);
        self.log.deliver();
        reloaded
    }

    fn start(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
        // SAFETY: a live task, on the main thread.
        let request = unsafe { task.request() };
        let Some(url) = request.URL() else { return };
        let address = url
            .absoluteString()
            .map(|text| text.to_string())
            .unwrap_or_default();
        let range = request
            .valueForHTTPHeaderField(ns_string!("Range"))
            .map(|text| text.to_string())
            .unwrap_or_default();
        let number = self.next.get() + 1;
        self.next.set(number);
        self.reading.borrow_mut().insert(number, task.retain());
        let read = Read {
            core: self.core.clone(),
            address,
            range,
            files: self.id,
            task: number,
        };
        let _ = READER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send(read);
    }

    fn stop(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
        // Drop a pending task so it is never answered. Tasks not in the map are already done.
        let pointer: *const ProtocolObject<dyn WKURLSchemeTask> = task;
        self.reading
            .borrow_mut()
            .retain(|_, held| !std::ptr::eq(Retained::as_ptr(held), pointer));
    }

    fn answer(&self, number: u64, reply: Reply) {
        let taken = self.reading.borrow_mut().remove(&number);
        if let Some(task) = taken {
            send(&task, reply);
        }
        self.log.deliver();
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let id = self.id;
        // Thread-locals may already be destroyed when the thread exits.
        let _ = FILES.try_with(|all| all.borrow_mut().remove(&id));
    }
}

fn send(task: &ProtocolObject<dyn WKURLSchemeTask>, reply: Reply) {
    // SAFETY: a live task that has not been answered, on the main thread.
    unsafe {
        let Some(url) = task.request().URL() else {
            return;
        };
        let Some(body) = reply.body else {
            // `URLError.cannotOpenFile`.
            let error =
                NSError::errorWithDomain_code_userInfo(ns_string!("NSURLErrorDomain"), -3000, None);
            return task.didFailWithError(&error);
        };
        core::trace(|| {
            format!(
                "{} {} {} bytes",
                reply.status,
                url.path().map(|path| path.to_string()).unwrap_or_default(),
                body.len()
            )
        });
        let names: Vec<Retained<NSString>> = reply
            .headers
            .iter()
            .map(|(name, _)| NSString::from_str(name))
            .collect();
        let values: Vec<Retained<NSString>> = reply
            .headers
            .iter()
            .map(|(_, value)| NSString::from_str(value))
            .collect();
        let names: Vec<&NSString> = names.iter().map(|name| &**name).collect();
        let values: Vec<&NSString> = values.iter().map(|value| &**value).collect();
        let headers = NSDictionary::from_slices(&names, &values);
        let Some(response) = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
            NSHTTPURLResponse::alloc(),
            &url,
            isize::from(reply.status as i16),
            Some(ns_string!("HTTP/1.1")),
            Some(&headers),
        ) else {
            return;
        };
        task.didReceiveResponse(&response);
        if !body.is_empty() {
            task.didReceiveData(&NSData::from_vec(body));
        }
        task.didFinish();
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "NRWebhostSceneFiles"]
    #[ivars = Rc<Files>]
    struct SchemeHandler;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for SchemeHandler {}

    // SAFETY: the signatures are WKURLSchemeHandler's.
    unsafe impl WKURLSchemeHandler for SchemeHandler {
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            self.ivars().start(task);
        }

        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            self.ivars().stop(task);
        }
    }
);

impl SchemeHandler {
    fn new(files: Rc<Files>) -> Retained<Self> {
        let this = Self::alloc(main_thread()).set_ivars(files);
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// An event a web view reports to its delegate.
pub enum WebEvent {
    /// The entry page finished loading.
    Loaded,
    /// The entry page failed to load, with the platform's message.
    Failed(String),
    /// The web content process ended.
    Ended,
    /// A page posted JSON through the bridge.
    Message(String),
}

type EventHandler = dyn Fn(WebEvent);

pub struct EventsIvars {
    files: Rc<Files>,
    handler: RefCell<Option<Box<EventHandler>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "NRWebhostEvents"]
    #[ivars = EventsIvars]
    pub struct WebEvents;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for WebEvents {}

    // SAFETY: the signatures are WKNavigationDelegate's.
    unsafe impl WKNavigationDelegate for WebEvents {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(
            &self,
            _web_view: &WKWebView,
            action: &WKNavigationAction,
            decision: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let allowed = self.ivars().files.allows(action);
            decision.call((if allowed {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn finished(&self, _web_view: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.tell(WebEvent::Loaded);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn failed(
            &self,
            _web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            self.tell(WebEvent::Failed(error.localizedDescription().to_string()));
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn failed_early(
            &self,
            _web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            self.tell(WebEvent::Failed(error.localizedDescription().to_string()));
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn ended(&self, _web_view: &WKWebView) {
            self.tell(WebEvent::Ended);
        }
    }

    // SAFETY: the signature is WKScriptMessageHandler's.
    unsafe impl WKScriptMessageHandler for WebEvents {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn received(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            // SAFETY: a live message, on the main thread.
            let body = unsafe { message.body() };
            if let Some(text) = body.downcast_ref::<NSString>() {
                self.tell(WebEvent::Message(text.to_string()));
            }
        }
    }
);

impl WebEvents {
    pub fn new(files: Rc<Files>) -> Retained<Self> {
        let this = Self::alloc(main_thread()).set_ivars(EventsIvars {
            files,
            handler: RefCell::new(None),
        });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    /// Sets the event handler. It must hold its owner weakly, since the web view holds this.
    pub fn set_handler(&self, handler: impl Fn(WebEvent) + 'static) {
        *self.ivars().handler.borrow_mut() = Some(Box::new(handler));
    }

    fn tell(&self, event: WebEvent) {
        // Take the handler out while it runs, since it may replace itself.
        let handler = self.ivars().handler.borrow_mut().take();
        if let Some(handler) = handler {
            handler(event);
            let mut slot = self.ivars().handler.borrow_mut();
            if slot.is_none() {
                *slot = Some(handler);
            }
        }
    }
}

/// A non-persistent web view configuration for `files`: the page host's scheme, autoplay and
/// window rules from the core, `scripts` injected first in every frame, then the bridge with
/// `events` as its handler.
pub fn configuration(
    files: &Rc<Files>,
    scripts: &[String],
    events: &WebEvents,
) -> Retained<WKWebViewConfiguration> {
    let main = main_thread();
    // SAFETY: plain WebKit calls on the main thread.
    unsafe {
        let configuration = WKWebViewConfiguration::new(main);
        configuration.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(main));
        let handler = SchemeHandler::new(files.clone());
        configuration.setURLSchemeHandler_forURLScheme(
            Some(ProtocolObject::from_ref(&*handler)),
            &NSString::from_str(&core::scheme()),
        );
        let web = files.core.web();
        let mut needs_action = WKAudiovisualMediaTypes::empty();
        if !web.video_plays_alone {
            needs_action |= WKAudiovisualMediaTypes::Video;
        }
        if !web.sound_plays_alone {
            needs_action |= WKAudiovisualMediaTypes::Audio;
        }
        configuration.setMediaTypesRequiringUserActionForPlayback(needs_action);
        let preferences = configuration.preferences();
        preferences.setJavaScriptCanOpenWindowsAutomatically(web.opens_windows);
        preferences.setElementFullscreenEnabled(web.fullscreen);
        // WebRTC reaches the network through ICE (STUN and TURN servers and their DNS lookups),
        // which the block list does not cover, so turn it off. The switch is private WebKit API.
        let peer_connections = objc2::sel!(_setPeerConnectionEnabled:);
        let switchable: bool = msg_send![&*preferences, respondsToSelector: peer_connections];
        if switchable {
            let _: () = msg_send![&*preferences, _setPeerConnectionEnabled: false];
        }
        configuration.setAllowsAirPlayForMediaPlayback(web.airplay);
        let controller = configuration.userContentController();
        for script in scripts.iter().map(String::as_str).chain([BRIDGE_SCRIPT]) {
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(main),
                &NSString::from_str(script),
                WKUserScriptInjectionTime::AtDocumentStart,
                false,
            );
            controller.addUserScript(&script);
        }
        controller.addScriptMessageHandler_name(
            ProtocolObject::from_ref(events),
            &NSString::from_str(BRIDGE),
        );
        configuration
    }
}

/// Removes the bridge handler, which the content controller retains.
pub fn remove_bridge(web_view: &WKWebView) {
    // SAFETY: plain WebKit calls on the main thread.
    unsafe {
        web_view
            .configuration()
            .userContentController()
            .removeScriptMessageHandlerForName(&NSString::from_str(BRIDGE))
    }
}

/// A web view for `configuration` at `frame`, with swipe navigation, magnification and link
/// previews turned off.
pub fn web_view(
    configuration: &WKWebViewConfiguration,
    frame: CGRect,
    events: &WebEvents,
) -> Retained<WKWebView> {
    // SAFETY: plain WebKit calls on the main thread.
    unsafe {
        let web_view = WKWebView::initWithFrame_configuration(
            WKWebView::alloc(main_thread()),
            frame,
            configuration,
        );
        web_view.setNavigationDelegate(Some(ProtocolObject::from_ref(events)));
        web_view.setAllowsBackForwardNavigationGestures(false);
        web_view.setAllowsMagnification(false);
        web_view.setAllowsLinkPreview(false);
        hold_at_ends(&web_view);
        web_view
    }
}

/// Turns off rubber-band scrolling to match every other scroller in the app. The switch is
/// private WebKit API (`_setRubberBandingEnabled:`, `_WKRectEdgeNone`); without it the view keeps
/// its rubber band.
fn hold_at_ends(web_view: &WKWebView) {
    let set = objc2::sel!(_setRubberBandingEnabled:);
    // SAFETY: plain WebKit calls on the main thread; the switch takes an `NSUInteger` edge mask,
    // here none.
    unsafe {
        let has: bool = msg_send![web_view, respondsToSelector: set];
        if has {
            let _: () = msg_send![web_view, _setRubberBandingEnabled: 0_usize];
        }
    }
}

pub fn load(web_view: &WKWebView, url: &NSURL) {
    // SAFETY: plain WebKit calls on the main thread.
    unsafe { web_view.loadRequest(&NSURLRequest::requestWithURL(url)) };
}

thread_local! {
    /// The compiled or stored list, shared by every web view in the process.
    static BLOCK_LIST: RefCell<Option<Retained<WKContentRuleList>>> = const { RefCell::new(None) };
}

/// The WebKit rule list that blocks every network request, compiled once into the core's folder.
/// If it fails, nothing loads, so a page never runs with network access.
pub async fn network_block() -> Result<Retained<WKContentRuleList>, crate::core::HostError> {
    if let Some(list) = BLOCK_LIST.with(|list| list.borrow().clone()) {
        return Ok(list);
    }
    let folder = core::rules_folder();
    let _ = std::fs::create_dir_all(&folder);
    let main = main_thread();
    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&folder), true);
    // SAFETY: plain WebKit calls on the main thread.
    let Some(store) = (unsafe { WKContentRuleListStore::storeWithURL(Some(&url), main) }) else {
        return Err(crate::core::HostError::of(NR_HOST_RULES_STORED, &folder));
    };
    let name = NSString::from_str(&core::block_rules_name());
    let (resolver, found) = promise::<Option<Retained<WKContentRuleList>>>();
    let once = Once::new(resolver);
    let block = RcBlock::new(move |list: *mut WKContentRuleList, _error: *mut NSError| {
        // SAFETY: null or a list WebKit made.
        once.resolve(unsafe { Retained::retain(list) });
    });
    // SAFETY: plain WebKit calls on the main thread.
    unsafe {
        store.lookUpContentRuleListForIdentifier_completionHandler(Some(&name), Some(&block))
    };
    let list = match found.await {
        Some(list) => list,
        None => {
            let (resolver, made) = promise::<Result<Retained<WKContentRuleList>, String>>();
            let once = Once::new(resolver);
            let block = RcBlock::new(move |list: *mut WKContentRuleList, error: *mut NSError| {
                // SAFETY: null or objects WebKit made.
                let made = unsafe { Retained::retain(list) }.ok_or_else(|| {
                    // SAFETY: as above.
                    unsafe { error.as_ref() }
                        .map(|error| error.localizedDescription().to_string())
                        .unwrap_or_default()
                });
                once.resolve(made);
            });
            let rules = NSString::from_str(&core::block_rules());
            // SAFETY: plain WebKit calls on the main thread.
            unsafe {
                store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                    Some(&name),
                    Some(&rules),
                    Some(&block),
                )
            };
            made.await
                .map_err(|why| crate::core::HostError::of(NR_HOST_RULES_FAILED, &why))?
        }
    };
    BLOCK_LIST.with(|slot| *slot.borrow_mut() = Some(list.clone()));
    Ok(list)
}

/// A script's result: its text if it returned a string, `None` for any other value, or the
/// exception message.
pub type Said = Result<Option<String>, String>;

fn said(value: *mut AnyObject, error: *mut NSError) -> Said {
    // SAFETY: null or objects WebKit made, alive for this call.
    if let Some(error) = unsafe { error.as_ref() } {
        let exception = error
            .userInfo()
            .objectForKey(ns_string!("WKJavaScriptExceptionMessage"));
        return Err(exception
            .and_then(|message| message.downcast::<NSString>().ok())
            .map_or_else(
                || error.localizedDescription().to_string(),
                |message| message.to_string(),
            ));
    }
    // SAFETY: as above.
    Ok(unsafe { value.as_ref() }
        .and_then(|value| value.downcast_ref::<NSString>())
        .map(NSString::to_string))
}

/// Runs `script` in the page and resolves with its result.
pub fn evaluate(web_view: &WKWebView, script: &str) -> Promise<Said> {
    let (resolver, answer) = promise();
    let block = answered(resolver);
    // SAFETY: plain WebKit calls on the main thread; WebKit copies the block.
    unsafe {
        web_view.evaluateJavaScript_completionHandler(&NSString::from_str(script), Some(&block))
    };
    answer
}

fn answered(resolver: Resolver<Said>) -> RcBlock<dyn Fn(*mut AnyObject, *mut NSError)> {
    let once = Once::new(resolver);
    RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| once.resolve(said(value, error)))
}

pub struct StageIvars {
    claims_visible: Cell<bool>,
}

define_class!(
    // SAFETY: NSWindow may be subclassed, and this class does not implement Drop.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "NRWebhostStageWindow"]
    #[ivars = StageIvars]
    /// An ordered-in window that is never on screen: borderless, far off every display, and
    /// ignoring mouse and keyboard. WebKit only draws video and runs animation frames for a view
    /// it thinks is visible, so this window reports itself visible. Uses public AppKit only.
    pub struct StageWindow;

    impl StageWindow {
        #[unsafe(method(occlusionState))]
        fn occlusion_state(&self) -> NSWindowOcclusionState {
            if self.ivars().claims_visible.get() {
                NSWindowOcclusionState::Visible
            } else {
                // SAFETY: defers to NSWindow's implementation.
                unsafe { msg_send![super(self), occlusionState] }
            }
        }

        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            false
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }
    }
);

impl StageWindow {
    /// A window of size `frame` holding `view`, ordered in far off every display.
    pub fn offscreen(
        frame: CGRect,
        claims_visible: bool,
        view: &objc2_app_kit::NSView,
    ) -> Retained<Self> {
        let this = Self::alloc(main_thread()).set_ivars(StageIvars {
            claims_visible: Cell::new(claims_visible),
        });
        // SAFETY: NSWindow's designated initialiser; the window is not released when closed.
        let window: Retained<Self> = unsafe {
            msg_send![super(this), initWithContentRect: frame, styleMask: NSWindowStyleMask::Borderless, backing: NSBackingStoreType::Buffered, defer: false]
        };
        // SAFETY: the window is kept by its owner, so closing it must not release it.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setContentView(Some(view));
        window.setIgnoresMouseEvents(true);
        window.setHasShadow(false);
        window.setExcludedFromWindowsMenu(true);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::FullScreenNone,
        );
        // Far enough off every display that no part is ever on screen.
        window.setFrameOrigin(NSPoint::new(-30000.0, -30000.0));
        window.orderBack(None);
        window
    }
}
