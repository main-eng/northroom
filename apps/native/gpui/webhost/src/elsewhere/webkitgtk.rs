//! The Linux engine: WebKitGTK (webkit2gtk-4.1, GTK 3) on the main thread's GLib loop (`run`).
//! Each view gets its own ephemeral web context (`new_ephemeral`) inside a hidden GTK offscreen
//! window. The core answers the page host's scheme (`files`) and decides navigations and
//! downloads. The network is blocked with the same rule list the Mac compiles, since WebKit's
//! content filters accept the same rules. Pictures are WebKit snapshots, and input reaches the
//! view as synthesized GDK events, the way WebKit's test runner sends them.

use super::files::Files;
use super::pages::{Said, Setup, bridge_script};
use super::{Button, Input, Modifiers, Opened, WebEvent};
use crate::core::{self, HostError, Reply};
use crate::picture::Picture;
use crate::run::{Once, Promise, promise};
use gtk::gdk::ffi as gdk_ffi;
use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};
use javascriptcore::ValueExt as _;
use northroom_core::page::host::ffi::{NR_HOST_RULES_FAILED, NR_HOST_RULES_STORED};
use std::cell::Cell;
use std::rc::{Rc, Weak};
use webkit2gtk::{
    LoadEvent, NavigationPolicyDecision, NavigationPolicyDecisionExt, PolicyDecisionExt,
    PolicyDecisionType, ResponsePolicyDecision, ResponsePolicyDecisionExt, SecurityManagerExt,
    SettingsExt, SnapshotOptions, SnapshotRegion, URIRequestExt, URISchemeRequest,
    URISchemeRequestExt, URISchemeResponse, URISchemeResponseExt, UserContentInjectedFrames,
    UserContentManager, UserContentManagerExt, UserScript, UserScriptInjectionTime, WebContext,
    WebContextExt, WebView, WebViewExt,
};

/// The script message handler pages post to.
const BRIDGE: &str = "nr";

/// The bridge's send function for a top-level document: WebKit's message handler.
const SEND: &str = "(window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.nr) ? function (text) { window.webkit.messageHandlers.nr.postMessage(text); } : null";

/// Initializes GTK once on the main thread, keeping the C locale the app set.
fn gtk_ready() -> Result<(), String> {
    thread_local! {
        static READY: Cell<bool> = const { Cell::new(false) };
    }
    if READY.with(Cell::get) {
        return Ok(());
    }
    gtk::disable_setlocale();
    gtk::init().map_err(|error| error.to_string())?;
    READY.with(|ready| ready.set(true));
    Ok(())
}

/// A compiled block list (`WebKitUserContentFilter`), kept for the life of the process.
#[derive(Clone, Copy)]
struct BlockList(*mut webkit2gtk::ffi::WebKitUserContentFilter);

thread_local! {
    static BLOCK_LIST: Cell<Option<BlockList>> = const { Cell::new(None) };
}

type Compiled = Result<BlockList, String>;

unsafe extern "C" fn compiled(
    store: *mut glib::gobject_ffi::GObject,
    result: *mut gio::ffi::GAsyncResult,
    data: glib::ffi::gpointer,
) {
    // SAFETY: `data` is the boxed resolver from `network_block`, reclaimed once here; `store` and
    // `result` come from that operation.
    unsafe {
        let once = Box::from_raw(data.cast::<Once<Compiled>>());
        let mut error = std::ptr::null_mut();
        let filter = webkit2gtk::ffi::webkit_user_content_filter_store_save_finish(
            store.cast(),
            result,
            &mut error,
        );
        once.resolve(if filter.is_null() {
            let error: Option<glib::Error> = glib::translate::from_glib_full(error);
            Err(error.map(|error| error.to_string()).unwrap_or_default())
        } else {
            Ok(BlockList(filter))
        });
    }
}

/// The block list that stops every network request, compiled once into the core's folder. If it
/// fails, nothing loads, so a page never runs with network access.
async fn network_block() -> Result<BlockList, HostError> {
    if let Some(list) = BLOCK_LIST.with(Cell::get) {
        return Ok(list);
    }
    let folder = core::rules_folder();
    std::fs::create_dir_all(&folder).map_err(|_| HostError::of(NR_HOST_RULES_STORED, &folder))?;
    let (resolver, made) = promise::<Compiled>();
    let once = Box::new(Once::new(resolver));
    let rules = glib::Bytes::from(core::block_rules().as_bytes());
    let (folder_c, name) = (
        std::ffi::CString::new(folder.clone())
            .map_err(|_| HostError::of(NR_HOST_RULES_STORED, &folder))?,
        std::ffi::CString::new(core::block_rules_name()).unwrap_or_default(),
    );
    // SAFETY: plain WebKit calls on the main thread. The operation keeps the store alive until
    // `compiled` runs and reclaims the boxed resolver.
    unsafe {
        let store = webkit2gtk::ffi::webkit_user_content_filter_store_new(folder_c.as_ptr());
        webkit2gtk::ffi::webkit_user_content_filter_store_save(
            store,
            name.as_ptr(),
            rules.to_glib_none().0,
            std::ptr::null_mut(),
            Some(compiled),
            Box::into_raw(once).cast(),
        );
        glib::gobject_ffi::g_object_unref(store.cast());
    }
    let list = made
        .await
        .map_err(|why| HostError::of(NR_HOST_RULES_FAILED, &why))?;
    BLOCK_LIST.with(|slot| slot.set(Some(list)));
    Ok(list)
}

type Handler = Box<dyn Fn(WebEvent)>;

pub(crate) struct Engine {
    files: Rc<Files>,
    view: WebView,
    window: gtk::OffscreenWindow,
    handler: Handler,
    closed: Cell<bool>,
    /// The last pressed button, for the drags that follow.
    held: Cell<Option<Button>>,
    _opened: Opened,
}

/// The reason phrase for an HTTP status, for the response line.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        304 => "Not Modified",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "",
    }
}

/// Sends the core's response to a page host scheme request.
fn respond(request: &URISchemeRequest, reply: Reply) {
    let Some(body) = reply.body else {
        // The core named a file that could not be read.
        let mut error = glib::Error::new(gio::IOErrorEnum::NotFound, "the file could not be read");
        request.finish_error(&mut error);
        return;
    };
    let length = body.len() as i64;
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(body));
    let response = URISchemeResponse::new(&stream, length);
    response.set_status(u32::from(reply.status), Some(reason(reply.status)));
    let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
    for (name, value) in &reply.headers {
        if name.eq_ignore_ascii_case("content-type") {
            response.set_content_type(value);
        }
        headers.append(name, value);
    }
    response.set_http_headers(headers);
    request.finish_with_response(&response);
}

/// A new ephemeral web context whose page host scheme is answered by `files`.
fn context(files: &Rc<Files>) -> WebContext {
    let context = WebContext::new_ephemeral();
    let scheme = core::scheme();
    if let Some(security) = context.security_manager() {
        security.register_uri_scheme_as_secure(&scheme);
        security.register_uri_scheme_as_cors_enabled(&scheme);
    }
    let weak = Rc::downgrade(files);
    context.register_uri_scheme(&scheme, move |request| {
        let Some(files) = weak.upgrade() else { return };
        let address = request.uri().map(|uri| uri.to_string()).unwrap_or_default();
        let range = request
            .http_headers()
            .and_then(|headers| headers.one("Range"))
            .map(|range| range.to_string())
            .unwrap_or_default();
        let request = request.clone();
        files.read(
            &address,
            &range,
            Box::new(move |reply| respond(&request, reply)),
        );
    });
    context
}

impl Engine {
    /// Creates a web view for `setup` in its own offscreen window, reporting events to
    /// `handler`. It returns with the network blocked and scripts installed, ready to load.
    pub async fn open(setup: Setup, handler: Handler) -> Result<Rc<Self>, HostError> {
        gtk_ready().map_err(|why| HostError::of(setup.failure, &why))?;
        let list = network_block().await?;
        let web = setup.files.core.web();
        let content = UserContentManager::new();
        for script in setup.scripts.iter().cloned().chain([bridge_script(SEND)]) {
            content.add_script(&UserScript::new(
                &script,
                UserContentInjectedFrames::AllFrames,
                UserScriptInjectionTime::Start,
                &[],
                &[],
            ));
        }
        content.register_script_message_handler(BRIDGE);
        // SAFETY: a live content manager and a filter kept for the process.
        unsafe {
            webkit2gtk::ffi::webkit_user_content_manager_add_filter(
                content.to_glib_none().0,
                list.0,
            )
        };
        let view = webkit2gtk::WebView::builder()
            .web_context(&context(&setup.files))
            .user_content_manager(&content)
            .build();
        if let Some(settings) = WebViewExt::settings(&view) {
            settings.set_enable_developer_extras(false);
            settings.set_enable_page_cache(false);
            settings.set_javascript_can_open_windows_automatically(web.opens_windows);
            settings.set_enable_fullscreen(web.fullscreen);
            settings.set_media_playback_requires_user_gesture(
                !(web.video_plays_alone && web.sound_plays_alone),
            );
            settings.set_enable_media_stream(false);
            settings.set_enable_webrtc(false);
        }
        view.set_is_muted(setup.silent);
        view.set_size_request(setup.width as i32, setup.height as i32);
        let window = gtk::OffscreenWindow::new();
        window.set_default_size(setup.width as i32, setup.height as i32);
        window.add(&view);
        window.show_all();
        let engine = Rc::new(Self {
            files: setup.files.clone(),
            view,
            window,
            handler,
            closed: Cell::new(false),
            held: Cell::new(None),
            _opened: Opened::new(),
        });
        engine.listen(&content);
        Ok(engine)
    }

    fn tell(&self, event: WebEvent) {
        if !self.closed.get() {
            (self.handler)(event);
        }
    }

    /// Connects the view's signals, each reported through `tell` or decided by the core. Every
    /// handler holds the engine weakly.
    fn listen(self: &Rc<Self>, content: &UserContentManager) {
        let weak = Rc::downgrade(self);
        self.listen_for_decisions(&weak);
        self.listen_for_loads(&weak);
        self.listen_for_crashes(&weak);
        Self::listen_for_messages(content, weak);
    }

    /// The core decides navigations and downloads; pages never open new windows.
    fn listen_for_decisions(&self, weak: &Weak<Self>) {
        let engine = weak.clone();
        self.view.connect_decide_policy(move |_, decision, kind| {
            let Some(engine) = engine.upgrade() else {
                return false;
            };
            let allowed = match kind {
                PolicyDecisionType::NavigationAction => {
                    let address = decision
                        .downcast_ref::<NavigationPolicyDecision>()
                        .and_then(|decision| decision.navigation_action())
                        .and_then(|action| action.request())
                        .and_then(|request| request.uri())
                        .map(|uri| uri.to_string())
                        .unwrap_or_default();
                    // WebKitGTK does not report which frame navigates, so treat the entry as the
                    // main frame and everything else as a subframe.
                    let main_frame = address == engine.files.entry();
                    engine.files.allows(&address, main_frame, false)
                }
                // Pages open no windows, as on a Mac.
                PolicyDecisionType::NewWindowAction => false,
                PolicyDecisionType::Response => {
                    let Some(response) = decision.downcast_ref::<ResponsePolicyDecision>() else {
                        return false;
                    };
                    // Never download what the view cannot show; the core logs it.
                    response.is_mime_type_supported() || {
                        let address = response
                            .request()
                            .and_then(|request| request.uri())
                            .map(|uri| uri.to_string())
                            .unwrap_or_default();
                        engine.files.allows(&address, true, true)
                    }
                }
                _ => return false,
            };
            if allowed {
                decision.use_();
            } else {
                decision.ignore();
            }
            true
        });
    }

    /// Reports loads, and failures with their reason.
    fn listen_for_loads(&self, weak: &Weak<Self>) {
        let engine = weak.clone();
        self.view.connect_load_changed(move |_, event| {
            if event == LoadEvent::Finished
                && let Some(engine) = engine.upgrade()
            {
                engine.tell(WebEvent::Loaded);
            }
        });
        let engine = weak.clone();
        self.view.connect_load_failed(move |_, _, _, error| {
            // A navigation the core blocked, or one superseded by the next, is not a failed load.
            let stopped = error.matches(webkit2gtk::NetworkError::Cancelled)
                || error.matches(webkit2gtk::PolicyError::FrameLoadInterruptedByPolicyChange);
            if !stopped && let Some(engine) = engine.upgrade() {
                engine.tell(WebEvent::Failed(error.to_string()));
            }
            false
        });
    }

    /// Reports a web process crash as `Ended`.
    fn listen_for_crashes(&self, weak: &Weak<Self>) {
        let engine = weak.clone();
        self.view.connect_web_process_terminated(move |_, _| {
            if let Some(engine) = engine.upgrade() {
                engine.tell(WebEvent::Ended);
            }
        });
    }

    /// Reports what the page's bridge sends as messages.
    fn listen_for_messages(content: &UserContentManager, engine: Weak<Self>) {
        content.connect_script_message_received(Some(BRIDGE), move |_, message| {
            if let (Some(engine), Some(value)) = (engine.upgrade(), message.js_value())
                && value.is_string()
            {
                engine.tell(WebEvent::Message(value.to_str().to_string()));
            }
        });
    }

    pub fn load(&self, address: &str) {
        self.view.load_uri(address);
    }

    /// Reloads the entry from its current files.
    pub fn reload(&self) {
        self.view.reload_bypass_cache();
    }

    /// Runs `script` in the page and resolves with its result.
    pub fn evaluate(&self, script: &str) -> Promise<Said> {
        let (resolver, answer) = promise();
        self.view.evaluate_javascript(
            script,
            None,
            None,
            None::<&gio::Cancellable>,
            move |result| {
                resolver.resolve(match result {
                    Ok(value) => Ok(value.is_string().then(|| value.to_str().to_string())),
                    Err(error) => Err(error.message().to_owned()),
                })
            },
        );
        answer
    }

    /// Snapshots what the view last drew, at one pixel per CSS pixel.
    pub fn snapshot(&self) -> Promise<Result<Picture, String>> {
        let (resolver, answer) = promise();
        self.view.snapshot(
            SnapshotRegion::Visible,
            SnapshotOptions::NONE,
            None::<&gio::Cancellable>,
            move |result| {
                resolver.resolve(
                    result
                        .map_err(|error| error.message().to_owned())
                        .and_then(|surface| picture_of(&surface)),
                )
            },
        );
        answer
    }

    pub fn resize(&self, width: u32, height: u32) {
        self.view.set_size_request(width as i32, height as i32);
        self.window.resize(width as i32, height as i32);
    }

    /// Forwards user input to the view as the GDK events GTK would deliver.
    pub fn input(&self, input: &Input) {
        let Some(target) = self.view.window() else {
            return;
        };
        match input {
            Input::Press {
                x,
                y,
                button,
                clicks,
            } => {
                self.held.set(Some(*button));
                self.view.grab_focus();
                let kind = match clicks {
                    2 => gdk_ffi::GDK_DOUBLE_BUTTON_PRESS,
                    3 => gdk_ffi::GDK_TRIPLE_BUTTON_PRESS,
                    _ => gdk_ffi::GDK_BUTTON_PRESS,
                };
                // Match GTK's order: a double press follows a plain press.
                if kind != gdk_ffi::GDK_BUTTON_PRESS {
                    send_button(&target, gdk_ffi::GDK_BUTTON_PRESS, *x, *y, *button);
                }
                send_button(&target, kind, *x, *y, *button);
            }
            Input::Release { x, y, button } => {
                self.held.set(None);
                send_button(&target, gdk_ffi::GDK_BUTTON_RELEASE, *x, *y, *button);
            }
            Input::Move { x, y, held } => send_motion(&target, *x, *y, *held),
            Input::Scroll { x, y, dx, dy } => send_scroll(&target, *x, *y, *dx, *dy),
            Input::Key { key, modifiers, .. } => {
                send_key(&target, gdk_ffi::GDK_KEY_PRESS, key, *modifiers)
            }
            Input::KeyUp { key, modifiers } => {
                send_key(&target, gdk_ffi::GDK_KEY_RELEASE, key, *modifiers)
            }
            Input::Text(text) => {
                for character in text.chars() {
                    let key = character.to_string();
                    send_key(&target, gdk_ffi::GDK_KEY_PRESS, &key, Modifiers::default());
                    send_key(
                        &target,
                        gdk_ffi::GDK_KEY_RELEASE,
                        &key,
                        Modifiers::default(),
                    );
                }
            }
            Input::Focus(focused) => {
                if *focused {
                    self.view.grab_focus();
                }
                send_focus(&target, *focused);
            }
        }
    }

    /// Releases the view and destroys its window.
    pub fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        self.view.stop_loading();
        // SAFETY: the window made in `open`, destroyed once on the main thread.
        unsafe { self.window.destroy() };
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.close();
    }
}

/// Converts a snapshot surface to the host's picture: premultiplied RGBA, top row first.
fn picture_of(surface: &cairo::Surface) -> Result<Picture, String> {
    let unreadable = |_| "the snapshot is not a picture".to_owned();
    let source = cairo::ImageSurface::try_from(surface.clone())
        .map_err(|_| "not an image surface".to_owned())?;
    let (width, height) = (source.width(), source.height());
    let mut copy =
        cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).map_err(unreadable)?;
    {
        let context = cairo::Context::new(&copy).map_err(unreadable)?;
        context
            .set_source_surface(&source, 0.0, 0.0)
            .map_err(unreadable)?;
        context.set_operator(cairo::Operator::Source);
        context.paint().map_err(unreadable)?;
    }
    copy.flush();
    let stride = copy.stride() as usize;
    let data = copy
        .data()
        .map_err(|_| "the snapshot could not be read".to_owned())?;
    let (width, height) = (width as u32, height as u32);
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in data.chunks(stride).take(height as usize) {
        // Cairo's ARGB32 is a native-endian word, so memory order here is BGRA.
        for pixel in row[..width as usize * 4].as_chunks::<4>().0 {
            rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
    }
    Ok(Picture {
        width,
        height,
        rgba,
    })
}

/// The current time in ms, as GDK timestamps events.
fn now() -> u32 {
    (glib::monotonic_time() / 1000) as u32
}

/// Delivers `event` to GTK as if the window system sent it, then frees it.
///
/// # Safety
/// `event` must come from `gdk_event_new` and be filled in for `target`.
unsafe fn dispatch(event: *mut gdk_ffi::GdkEvent, target: &gdk::Window) {
    // SAFETY: as the caller says; the event holds a reference to its window, freed with it.
    unsafe {
        (*event).any.window = target.to_glib_full();
        (*event).any.send_event = 1;
        if let Some(seat) = target.display().default_seat()
            && let Some(pointer) = seat.pointer()
        {
            gdk_ffi::gdk_event_set_device(event, pointer.to_glib_none().0);
        }
        gtk::ffi::gtk_main_do_event(event);
        gdk_ffi::gdk_event_free(event);
    }
}

fn button_number(button: Button) -> u32 {
    match button {
        Button::Left => 1,
        Button::Middle => 2,
        Button::Right => 3,
    }
}

fn held_state(held: Option<Button>) -> gdk_ffi::GdkModifierType {
    match held {
        Some(Button::Left) => gdk_ffi::GDK_BUTTON1_MASK,
        Some(Button::Middle) => gdk_ffi::GDK_BUTTON2_MASK,
        Some(Button::Right) => gdk_ffi::GDK_BUTTON3_MASK,
        None => 0,
    }
}

fn send_button(target: &gdk::Window, kind: gdk_ffi::GdkEventType, x: f64, y: f64, button: Button) {
    // SAFETY: a new event of the button kind, filled and handed over once.
    unsafe {
        let event = gdk_ffi::gdk_event_new(kind);
        let fields = &mut (*event).button;
        fields.time = now();
        (fields.x, fields.y, fields.x_root, fields.y_root) = (x, y, x, y);
        fields.button = button_number(button);
        if kind == gdk_ffi::GDK_BUTTON_RELEASE {
            fields.state = held_state(Some(button));
        }
        dispatch(event, target);
    }
}

fn send_motion(target: &gdk::Window, x: f64, y: f64, held: Option<Button>) {
    // SAFETY: a new motion event, filled and handed over once.
    unsafe {
        let event = gdk_ffi::gdk_event_new(gdk_ffi::GDK_MOTION_NOTIFY);
        let fields = &mut (*event).motion;
        fields.time = now();
        (fields.x, fields.y, fields.x_root, fields.y_root) = (x, y, x, y);
        fields.state = held_state(held);
        dispatch(event, target);
    }
}

/// WebKitGTK scrolls 40 px per smooth-scroll step.
const PIXELS_A_STEP: f64 = 40.0;

fn send_scroll(target: &gdk::Window, x: f64, y: f64, dx: f64, dy: f64) {
    // SAFETY: a new smooth scroll event, filled and handed over once.
    unsafe {
        let event = gdk_ffi::gdk_event_new(gdk_ffi::GDK_SCROLL);
        let fields = &mut (*event).scroll;
        fields.time = now();
        (fields.x, fields.y, fields.x_root, fields.y_root) = (x, y, x, y);
        fields.direction = gdk_ffi::GDK_SCROLL_SMOOTH;
        fields.delta_x = dx / PIXELS_A_STEP;
        fields.delta_y = dy / PIXELS_A_STEP;
        dispatch(event, target);
    }
}

/// The X keysym for `name`.
fn keysym(name: &str) -> Option<u32> {
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: a NUL-terminated name.
    let keyval = unsafe { gdk_ffi::gdk_keyval_from_name(name.as_ptr()) };
    (keyval != 0 && keyval != 0x00ff_ffff).then_some(keyval)
}

/// The X keysym for a GPUI key name.
fn keyval(key: &str) -> Option<u32> {
    let named = match key {
        "enter" => "Return",
        "backspace" => "BackSpace",
        "tab" => "Tab",
        "escape" => "Escape",
        "space" => "space",
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        "home" => "Home",
        "end" => "End",
        "pageup" => "Page_Up",
        "pagedown" => "Page_Down",
        "delete" => "Delete",
        "insert" => "Insert",
        _ => {
            let mut characters = key.chars();
            let first = characters.next()?;
            if characters.next().is_none() {
                // SAFETY: a plain conversion of a code point.
                return Some(unsafe { gdk_ffi::gdk_unicode_to_keyval(u32::from(first)) });
            }
            if let Some(number) = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                return keysym(&format!("F{number}"));
            }
            return None;
        }
    };
    keysym(named)
}

fn send_key(target: &gdk::Window, kind: gdk_ffi::GdkEventType, key: &str, modifiers: Modifiers) {
    let Some(keyval) = keyval(key) else { return };
    let mut state = 0;
    if modifiers.shift {
        state |= gdk_ffi::GDK_SHIFT_MASK;
    }
    if modifiers.control {
        state |= gdk_ffi::GDK_CONTROL_MASK;
    }
    if modifiers.alt {
        state |= gdk_ffi::GDK_MOD1_MASK;
    }
    // WebKit also reads the hardware keycode for this keyboard.
    let keycode = gdk::Keymap::for_display(&target.display())
        .and_then(|keymap| {
            keymap
                .entries_for_keyval(keyval)
                .first()
                .map(|key| key.keycode())
        })
        .unwrap_or_default();
    // SAFETY: a new key event, filled and handed over once; its string stays null.
    unsafe {
        let event = gdk_ffi::gdk_event_new(kind);
        let fields = &mut (*event).key;
        fields.time = now();
        fields.state = state;
        fields.keyval = keyval;
        fields.hardware_keycode = keycode as u16;
        dispatch(event, target);
    }
}

fn send_focus(target: &gdk::Window, focused: bool) {
    // SAFETY: a new focus event, filled and handed over once.
    unsafe {
        let event = gdk_ffi::gdk_event_new(gdk_ffi::GDK_FOCUS_CHANGE);
        (*event).focus_change.in_ = i16::from(focused);
        dispatch(event, target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_the_keysyms_x_names() {
        assert_eq!(keyval("a"), Some(0x61));
        assert_eq!(keyval("enter"), Some(0xff0d));
        assert_eq!(keyval("f5"), Some(0xffc2));
        assert_eq!(keyval("nothing"), None);
    }
}
