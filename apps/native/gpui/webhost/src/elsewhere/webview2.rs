//! The Windows engine: Edge WebView2, preinstalled on Windows 10 and 11. The process shares one
//! environment stored in a core folder, and every view opens a private profile in its own hidden
//! window, so nothing persists between openings. The page host's scheme is registered with the
//! environment and answered through the core (`files`). Every other request is refused with a
//! 403 before it leaves. Below that, the whole browser's network is closed: traffic those
//! requests do not cover (WebSockets, WebRTC) goes to a dead proxy and no name resolves. The core
//! decides navigations, downloads and new windows. Pictures are WebView2 previews, and input is
//! posted to the window the browser draws the page in.

use super::files::Files;
use super::pages::{Said, Setup, bridge_script};
use super::{Button, Input, Modifiers, Opened, WebEvent};
use crate::core::{self, HostError, Reply};
use crate::picture::Picture;
use crate::run::{Once, Promise, promise};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, COREWEBVIEW2_WEB_ERROR_STATUS,
    COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2_4, ICoreWebView2_8,
    ICoreWebView2_21, ICoreWebView2Controller, ICoreWebView2Controller3,
    ICoreWebView2CustomSchemeRegistration, ICoreWebView2Environment, ICoreWebView2Environment10,
    ICoreWebView2EnvironmentOptions, ICoreWebView2ExecuteScriptResult,
    ICoreWebView2WebResourceRequestedEventArgs,
};
use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler, CapturePreviewCompletedHandler,
    CoreWebView2CustomSchemeRegistration, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    DownloadStartingEventHandler, ExecuteScriptWithResultCompletedHandler,
    NavigationCompletedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, ProcessFailedEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, IStream, STREAM_SEEK_SET,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME,
    VK_INSERT, VK_LEFT, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE,
    VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, EnumChildWindows, GetClassNameW, PostMessageW,
    RegisterClassW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos,
    ShowWindow, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETFOCUS, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{BOOL, Interface, PCWSTR, PWSTR, w};

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The bridge's send function for a top-level document: WebView2's message channel.
const SEND: &str = "(window.chrome && window.chrome.webview) ? function (text) { window.chrome.webview.postMessage(text); } : null";

/// Browser flags. Network traffic goes to a proxy on a port nothing listens on and no name
/// resolves, so nothing is sent, and WebRTC stays behind that proxy. A view in a hidden window
/// keeps drawing and running timers, and media autoplays so the host can seek and play it.
const BROWSER_ARGUMENTS: &str = "--proxy-server=127.0.0.1:9 --proxy-bypass-list=<-loopback> \
     --host-resolver-rules=\"MAP * ~NOTFOUND\" --dns-prefetch-disable \
     --force-webrtc-ip-handling-policy=disable_non_proxied_udp \
     --disable-features=CalculateNativeWinOcclusion --disable-background-timer-throttling \
     --disable-backgrounding-occluded-windows --disable-renderer-backgrounding \
     --autoplay-policy=no-user-gesture-required";

thread_local! {
    /// The process-wide environment, once created.
    static ENVIRONMENT: RefCell<Option<ICoreWebView2Environment>> = const { RefCell::new(None) };
}

/// The process environment, created once with the page host's scheme registered and the network
/// closed.
async fn environment() -> Result<ICoreWebView2Environment, String> {
    if let Some(made) = ENVIRONMENT.with(|slot| slot.borrow().clone()) {
        return Ok(made);
    }
    // SAFETY: COM on the main thread in a single-threaded apartment; GPUI has already set it up.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let options = CoreWebView2EnvironmentOptions::default();
    let scheme = CoreWebView2CustomSchemeRegistration::new(core::scheme());
    // SAFETY: plain setters on the options' own fields.
    unsafe {
        scheme.set_has_authority_component(true);
        scheme.set_treat_as_secure(true);
        options.set_additional_browser_arguments(BROWSER_ARGUMENTS.to_owned());
        let scheme: ICoreWebView2CustomSchemeRegistration = scheme.into();
        options.set_scheme_registrations(vec![Some(scheme)]);
    }
    let options: ICoreWebView2EnvironmentOptions = options.into();
    let folder = std::path::Path::new(&core::rules_folder()).join("webview2");
    let _ = std::fs::create_dir_all(&folder);
    let folder = wide(&folder.to_string_lossy());
    let (resolver, made) = promise::<Result<ICoreWebView2Environment, String>>();
    let once = Rc::new(Once::new(resolver));
    let told = once.clone();
    let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
        move |result, environment| {
            told.resolve(
                result
                    .map_err(|error| error.message())
                    .and_then(|()| environment.ok_or_else(String::new)),
            );
            Ok(())
        },
    ));
    // SAFETY: the loader call, with a live folder string and handler.
    if let Err(error) = unsafe {
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            PCWSTR(folder.as_ptr()),
            &options,
            &handler,
        )
    } {
        once.resolve(Err(error.message()));
    }
    let environment = made.await?;
    ENVIRONMENT.with(|slot| *slot.borrow_mut() = Some(environment.clone()));
    Ok(environment)
}

unsafe extern "system" fn stage_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: default handling for every message.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// A `width` × `height` px window far off every display, never activated and absent from the
/// taskbar. It must be shown because the browser only draws pages in visible windows.
fn stage_window(width: u32, height: u32) -> Result<HWND, String> {
    thread_local! {
        static REGISTERED: Cell<bool> = const { Cell::new(false) };
    }
    // SAFETY: plain Win32 calls on the main thread.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|error| error.message())?;
        if !REGISTERED.with(|registered| registered.replace(true)) {
            let class = WNDCLASSW {
                lpfnWndProc: Some(stage_procedure),
                hInstance: instance.into(),
                lpszClassName: w!("NorthroomWebStage"),
                ..Default::default()
            };
            RegisterClassW(&class);
        }
        let window = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("NorthroomWebStage"),
            w!(""),
            WS_POPUP,
            -32000,
            -32000,
            width as i32,
            height as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|error| error.message())?;
        let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        Ok(window)
    }
}

type Handler = Box<dyn Fn(WebEvent)>;

pub(crate) struct Engine {
    files: Rc<Files>,
    environment: ICoreWebView2Environment,
    controller: ICoreWebView2Controller,
    view: ICoreWebView2,
    window: HWND,
    handler: Rc<Handler>,
    closed: Cell<bool>,
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
fn respond(
    environment: &ICoreWebView2Environment,
    request: &ICoreWebView2WebResourceRequestedEventArgs,
    reply: Reply,
) {
    // The core named a file that could not be read, so send nothing.
    let (status, body) = match reply.body {
        Some(body) => (reply.status, body),
        None => (404, Vec::new()),
    };
    let headers: String = reply
        .headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    let (headers, phrase) = (wide(&headers), wide(reason(status)));
    // SAFETY: plain WebView2 calls on the main thread, with live strings and stream.
    unsafe {
        let stream: Option<IStream> = SHCreateMemStream(Some(&body));
        if let Ok(response) = environment.CreateWebResourceResponse(
            stream.as_ref(),
            i32::from(status),
            PCWSTR(phrase.as_ptr()),
            PCWSTR(headers.as_ptr()),
        ) {
            let _ = request.SetResponse(&response);
        }
    }
}

/// Who answers a request.
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    /// The page host's scheme.
    Own,
    /// The browser itself, without the network.
    Local,
    /// Anything else: the network or a file outside the served folder.
    Elsewhere,
}

impl Asked {
    fn of(address: &str) -> Self {
        let scheme = address
            .split_once(':')
            .map(|(scheme, _)| scheme.to_ascii_lowercase())
            .unwrap_or_default();
        if scheme == core::scheme() {
            Self::Own
        } else if matches!(scheme.as_str(), "data" | "blob" | "about") {
            Self::Local
        } else {
            Self::Elsewhere
        }
    }
}

/// Converts WebView2's script result.
fn said(
    result: windows::core::Result<()>,
    value: Option<ICoreWebView2ExecuteScriptResult>,
) -> Said {
    result.map_err(|error| error.message())?;
    let value = value.ok_or_else(String::new)?;
    // SAFETY: plain WebView2 calls on a live result; the strings are freed by `take_pwstr`.
    unsafe {
        let mut succeeded = BOOL::default();
        value
            .Succeeded(&mut succeeded)
            .map_err(|error| error.message())?;
        if !succeeded.as_bool() {
            let mut message = PWSTR::null();
            let exception = value.Exception().map_err(|error| error.message())?;
            exception
                .Message(&mut message)
                .map_err(|error| error.message())?;
            return Err(take_pwstr(message));
        }
        let (mut text, mut is_string) = (PWSTR::null(), BOOL::default());
        value
            .TryGetResultAsString(&mut text, &mut is_string)
            .map_err(|error| error.message())?;
        let text = take_pwstr(text);
        Ok(is_string.as_bool().then_some(text))
    }
}

/// Reads all of `stream` from the start.
fn read_all(stream: &IStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0_u8; 1 << 16];
    // SAFETY: reads into a live buffer of the size given.
    unsafe {
        if stream.Seek(0, STREAM_SEEK_SET, None).is_err() {
            return bytes;
        }
        loop {
            let mut read = 0_u32;
            let result = stream.Read(
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                Some(&mut read),
            );
            if result.is_err() || read == 0 {
                return bytes;
            }
            bytes.extend_from_slice(&chunk[..read as usize]);
        }
    }
}

/// Reads an address WebView2 returns through `get`; empty when it gives none.
fn address_of(get: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> String {
    let mut uri = PWSTR::null();
    match get(&mut uri) {
        // WebView2 allocated the string; `take_pwstr` frees it.
        Ok(()) => take_pwstr(uri),
        Err(_) => String::new(),
    }
}

impl Engine {
    /// Creates a web view for `setup` in its own window, reporting events to `handler`. It
    /// returns with scripts installed, ready to load.
    pub async fn open(setup: Setup, handler: Handler) -> Result<Rc<Self>, HostError> {
        let failed = |why: String| HostError::of(setup.failure, &why);
        let environment = environment().await.map_err(failed)?;
        let window = stage_window(setup.width, setup.height).map_err(failed)?;
        let made = Self::controller(&environment, window).await;
        let controller = match made {
            Ok(controller) => controller,
            Err(why) => {
                // SAFETY: the window made above, on its thread.
                let _ = unsafe { DestroyWindow(window) };
                return Err(failed(why));
            }
        };
        // SAFETY: plain WebView2 calls on the main thread.
        let view = unsafe { controller.CoreWebView2() }.map_err(|error| failed(error.message()))?;
        let engine = Rc::new(Self {
            files: setup.files.clone(),
            environment,
            controller,
            view,
            window,
            handler: Rc::new(handler),
            closed: Cell::new(false),
            _opened: Opened::new(),
        });
        engine
            .configure(&setup)
            .await
            .map_err(|error| failed(error.message()))?;
        Ok(engine)
    }

    async fn controller(
        environment: &ICoreWebView2Environment,
        window: HWND,
    ) -> Result<ICoreWebView2Controller, String> {
        let (resolver, made) = promise::<Result<ICoreWebView2Controller, String>>();
        let once = Rc::new(Once::new(resolver));
        let told = once.clone();
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
            move |result, controller| {
                told.resolve(
                    result
                        .map_err(|error| error.message())
                        .and_then(|()| controller.ok_or_else(String::new)),
                );
                Ok(())
            },
        ));
        // SAFETY: plain WebView2 calls on the main thread with a live window and handler.
        let asked = unsafe {
            match environment.cast::<ICoreWebView2Environment10>() {
                // A private profile keeps no state between openings.
                Ok(newer) => newer
                    .CreateCoreWebView2ControllerOptions()
                    .and_then(|options| {
                        options.SetIsInPrivateModeEnabled(true)?;
                        newer.CreateCoreWebView2ControllerWithOptions(window, &options, &handler)
                    }),
                Err(_) => environment.CreateCoreWebView2Controller(window, &handler),
            }
        };
        if let Err(error) = asked {
            once.resolve(Err(error.message()));
        }
        made.await
    }

    /// Applies the view's settings, size, audio, scheme, scripts and event handlers.
    async fn configure(self: &Rc<Self>, setup: &Setup) -> windows::core::Result<()> {
        // SAFETY: plain WebView2 calls on the main thread with live objects.
        unsafe {
            let settings = self.view.Settings()?;
            settings.SetAreDefaultContextMenusEnabled(false)?;
            settings.SetAreDevToolsEnabled(false)?;
            settings.SetIsStatusBarEnabled(false)?;
            settings.SetIsZoomControlEnabled(false)?;
            settings.SetAreDefaultScriptDialogsEnabled(true)?;
            // Render at one pixel per CSS pixel on any display, so pictures match the stage size.
            if let Ok(controller) = self.controller.cast::<ICoreWebView2Controller3>() {
                controller.SetShouldDetectMonitorScaleChanges(false)?;
                controller.SetRasterizationScale(1.0)?;
            }
            self.controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: setup.width as i32,
                bottom: setup.height as i32,
            })?;
            self.controller.SetIsVisible(true)?;
            if setup.silent
                && let Ok(view) = self.view.cast::<ICoreWebView2_8>()
            {
                view.SetIsMuted(true)?;
            }
            // Filter every request, not only the page host's, so `requested` refuses foreign ones
            // before they reach the network regardless of the proxy.
            self.view
                .AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        }
        self.listen()?;
        for script in setup.scripts.iter().cloned().chain([bridge_script(SEND)]) {
            self.add_script(&script).await?;
        }
        Ok(())
    }

    /// Injects `script` first into every frame of every later document.
    async fn add_script(&self, script: &str) -> windows::core::Result<()> {
        let (resolver, added) = promise::<windows::core::Result<()>>();
        let once = Rc::new(Once::new(resolver));
        let told = once.clone();
        let handler = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
            move |result, _id| {
                told.resolve(result);
                Ok(())
            },
        ));
        let script = wide(script);
        // SAFETY: plain WebView2 calls on the main thread with a live string and handler.
        if let Err(error) = unsafe {
            self.view
                .AddScriptToExecuteOnDocumentCreated(PCWSTR(script.as_ptr()), &handler)
        } {
            once.resolve(Err(error));
        }
        added.await
    }

    fn tell(&self, event: WebEvent) {
        if !self.closed.get() {
            (self.handler)(event);
        }
    }

    /// Connects the view's events, each reported through `tell` or decided by the core. Every
    /// handler holds the engine weakly, and the view holds the handlers until it closes.
    fn listen(self: &Rc<Self>) -> windows::core::Result<()> {
        let weak = Rc::downgrade(self);
        self.listen_for_requests(&weak)?;
        self.listen_for_navigations(&weak)?;
        self.listen_for_loads(&weak)?;
        self.listen_for_messages(&weak)?;
        self.listen_for_crashes(&weak)?;
        self.refuse_new_windows()?;
        self.refuse_downloads(&weak)
    }

    /// Routes every page request to `requested`.
    fn listen_for_requests(&self, weak: &Weak<Self>) -> windows::core::Result<()> {
        let mut token = 0_i64;
        let engine = weak.clone();
        // SAFETY: a plain WebView2 call on the main thread.
        unsafe {
            self.view.add_WebResourceRequested(
                &WebResourceRequestedEventHandler::create(Box::new(move |_, request| {
                    if let (Some(engine), Some(request)) = (engine.upgrade(), request) {
                        engine.requested(request);
                    }
                    Ok(())
                })),
                &mut token,
            )
        }
    }

    /// The core decides where the page and each of its frames may navigate.
    fn listen_for_navigations(&self, weak: &Weak<Self>) -> windows::core::Result<()> {
        let mut token = 0_i64;
        // SAFETY: plain WebView2 calls on the main thread.
        unsafe {
            // `main_frame` is true for the top-level page, false for a subframe.
            let guard = |main_frame: bool| {
                let engine = weak.clone();
                NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    if let (Some(engine), Some(args)) = (engine.upgrade(), args) {
                        let address = address_of(|uri| args.Uri(uri));
                        if !engine.files.allows(&address, main_frame, false) {
                            args.SetCancel(true)?;
                        }
                    }
                    Ok(())
                }))
            };
            self.view.add_NavigationStarting(&guard(true), &mut token)?;
            self.view
                .add_FrameNavigationStarting(&guard(false), &mut token)
        }
    }

    /// Reports loads, and failures with their reason.
    fn listen_for_loads(&self, weak: &Weak<Self>) -> windows::core::Result<()> {
        let mut token = 0_i64;
        let engine = weak.clone();
        // SAFETY: plain WebView2 calls on the main thread.
        unsafe {
            self.view.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                    let (Some(engine), Some(args)) = (engine.upgrade(), args) else {
                        return Ok(());
                    };
                    let mut success = BOOL::default();
                    args.IsSuccess(&mut success)?;
                    if success.as_bool() {
                        engine.tell(WebEvent::Loaded);
                        return Ok(());
                    }
                    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                    args.WebErrorStatus(&mut status)?;
                    // A navigation the core blocked is not a failed load.
                    if status != COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED {
                        engine.tell(WebEvent::Failed(format!(
                            "WebView2 navigation error {}",
                            status.0
                        )));
                    }
                    Ok(())
                })),
                &mut token,
            )
        }
    }

    /// Reports what the page's bridge sends as messages.
    fn listen_for_messages(&self, weak: &Weak<Self>) -> windows::core::Result<()> {
        let mut token = 0_i64;
        let engine = weak.clone();
        // SAFETY: plain WebView2 calls on the main thread.
        unsafe {
            self.view.add_WebMessageReceived(
                &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                    if let (Some(engine), Some(args)) = (engine.upgrade(), args) {
                        let mut text = PWSTR::null();
                        if args.TryGetWebMessageAsString(&mut text).is_ok() {
                            engine.tell(WebEvent::Message(take_pwstr(text)));
                        }
                    }
                    Ok(())
                })),
                &mut token,
            )
        }
    }

    /// Reports a browser process failure as `Ended`.
    fn listen_for_crashes(&self, weak: &Weak<Self>) -> windows::core::Result<()> {
        let mut token = 0_i64;
        let engine = weak.clone();
        // SAFETY: a plain WebView2 call on the main thread.
        unsafe {
            self.view.add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |_, _| {
                    if let Some(engine) = engine.upgrade() {
                        engine.tell(WebEvent::Ended);
                    }
                    Ok(())
                })),
                &mut token,
            )
        }
    }

    /// Pages open no windows, as on a Mac.
    fn refuse_new_windows(&self) -> windows::core::Result<()> {
        let mut token = 0_i64;
        // SAFETY: plain WebView2 calls on the main thread.
        unsafe {
            self.view.add_NewWindowRequested(
                &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        args.SetHandled(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )
        }
    }

    /// Cancels every download; the core logs it.
    fn refuse_downloads(&self, weak: &Weak<Self>) -> windows::core::Result<()> {
        let mut token = 0_i64;
        let Ok(view) = self.view.cast::<ICoreWebView2_4>() else {
            return Ok(());
        };
        let engine = weak.clone();
        // SAFETY: plain WebView2 calls on the main thread.
        unsafe {
            view.add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                    if let (Some(engine), Some(args)) = (engine.upgrade(), args) {
                        let address = match args.DownloadOperation() {
                            Ok(download) => address_of(|uri| download.Uri(uri)),
                            Err(_) => String::new(),
                        };
                        let _ = engine.files.allows(&address, true, true);
                        args.SetCancel(true)?;
                    }
                    Ok(())
                })),
                &mut token,
            )
        }
    }

    /// Handles a request. The page host's scheme is answered through the core once its file is
    /// read. Requests the browser serves locally (`data:`, `blob:`, `about:`) pass through.
    /// Anything else gets a 403 and is logged as blocked.
    fn requested(&self, request: ICoreWebView2WebResourceRequestedEventArgs) {
        // SAFETY: plain WebView2 calls on the main thread on a live request.
        let (address, range, deferral) = unsafe {
            let Ok(asked) = request.Request() else { return };
            let address = address_of(|uri| asked.Uri(uri));
            match Asked::of(&address) {
                Asked::Own => {}
                Asked::Local => return,
                Asked::Elsewhere => {
                    let blocked = serde_json::json!({
                        "type": "blocked", "kind": "request", "url": address, "page": "browser",
                    });
                    self.files.received(&blocked.to_string());
                    let refused = Reply {
                        status: 403,
                        headers: Vec::new(),
                        body: Some(Vec::new()),
                    };
                    respond(&self.environment, &request, refused);
                    return;
                }
            }
            let range = match asked.Headers() {
                Ok(headers) => address_of(|range| headers.GetHeader(w!("Range"), range)),
                Err(_) => String::new(),
            };
            let Ok(deferral) = request.GetDeferral() else {
                return;
            };
            (address, range, deferral)
        };
        let environment = self.environment.clone();
        self.files.read(
            &address,
            &range,
            Box::new(move |reply| {
                respond(&environment, &request, reply);
                // SAFETY: the deferral taken above, completed once.
                let _ = unsafe { deferral.Complete() };
            }),
        );
    }

    pub fn load(&self, address: &str) {
        let address = wide(address);
        // SAFETY: plain WebView2 calls on the main thread with a live string.
        let _ = unsafe { self.view.Navigate(PCWSTR(address.as_ptr())) };
    }

    /// Reloads the entry from its current files.
    pub fn reload(&self) {
        // The page host's scheme has no cache in front of it, so a plain load is fresh.
        self.load(&self.files.entry());
    }

    /// Runs `script` in the page and resolves with its result.
    pub fn evaluate(&self, script: &str) -> Promise<Said> {
        let (resolver, answer) = promise();
        let once = Rc::new(Once::new(resolver));
        let told = once.clone();
        let handler =
            ExecuteScriptWithResultCompletedHandler::create(Box::new(move |result, value| {
                told.resolve(said(result, value));
                Ok(())
            }));
        let script = wide(script);
        // SAFETY: plain WebView2 calls on the main thread with a live string and handler.
        let asked = unsafe {
            self.view
                .cast::<ICoreWebView2_21>()
                .and_then(|view| view.ExecuteScriptWithResult(PCWSTR(script.as_ptr()), &handler))
        };
        if let Err(error) = asked {
            once.resolve(Err(error.message()));
        }
        answer
    }

    /// Snapshots what the view last drew, at one pixel per CSS pixel.
    pub fn snapshot(&self) -> Promise<Result<Picture, String>> {
        let (resolver, answer) = promise();
        let once = Rc::new(Once::new(resolver));
        // SAFETY: a new growable memory stream that WebView2 writes into.
        let Some(stream) = (unsafe { SHCreateMemStream(None) }) else {
            once.resolve(Err("no stream for the picture".to_owned()));
            return answer;
        };
        let (told, written) = (once.clone(), stream.clone());
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
            told.resolve(result.map_err(|error| error.message()).and_then(|()| {
                super::pictures::decode(&read_all(&written))
                    .ok_or_else(|| "the preview is not a picture".to_owned())
            }));
            Ok(())
        }));
        // SAFETY: plain WebView2 calls on the main thread with a live stream and handler.
        if let Err(error) = unsafe {
            self.view.CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                &stream,
                &handler,
            )
        } {
            once.resolve(Err(error.message()));
        }
        answer
    }

    pub fn resize(&self, width: u32, height: u32) {
        // SAFETY: plain Win32 and WebView2 calls on the main thread.
        unsafe {
            let _ = SetWindowPos(
                self.window,
                None,
                0,
                0,
                width as i32,
                height as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let _ = self.controller.SetBounds(RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            });
        }
    }

    /// Finds the child window the browser draws the page in and reads input from.
    fn page_window(&self) -> Option<HWND> {
        struct Found(Option<HWND>);
        unsafe extern "system" fn look(window: HWND, found: LPARAM) -> BOOL {
            let mut name = [0_u16; 64];
            // SAFETY: a live window and a buffer of the size given; `found` is the `Found`
            // below, alive through the enumeration.
            unsafe {
                let length = GetClassNameW(window, &mut name) as usize;
                if String::from_utf16_lossy(&name[..length]) == "Chrome_RenderWidgetHostHWND" {
                    (*(found.0 as *mut Found)).0 = Some(window);
                    return BOOL(0);
                }
            }
            BOOL(1)
        }
        let mut found = Found(None);
        // SAFETY: enumerates the stage window's children with `look`, which writes `found`.
        unsafe {
            let _ = EnumChildWindows(
                Some(self.window),
                Some(look),
                LPARAM(&mut found as *mut Found as isize),
            );
        }
        found.0
    }

    /// Posts user input to the window the browser reads input from.
    pub fn input(&self, input: &Input) {
        let Some(window) = self.page_window() else {
            return;
        };
        let page = PageWindow(window);
        match input {
            Input::Press {
                x,
                y,
                button,
                clicks,
            } => page.post(
                press_message(*button, *clicks),
                held_flag(Some(*button)),
                client_point(*x, *y),
            ),
            Input::Release { x, y, button } => {
                page.post(release_message(*button), 0, client_point(*x, *y));
            }
            Input::Move { x, y, held } => {
                page.post(WM_MOUSEMOVE, held_flag(*held), client_point(*x, *y));
            }
            Input::Scroll { x, y, dx, dy } => page.scroll(*x, *y, *dx, *dy),
            Input::Key {
                key,
                text,
                modifiers,
            } => page.key_down(key, text.as_deref(), *modifiers),
            Input::KeyUp { key, modifiers } => page.key_up(key, *modifiers),
            Input::Text(text) => page.type_text(text),
            Input::Focus(true) => page.post(WM_SETFOCUS, 0, LPARAM(0)),
            Input::Focus(false) => page.post(WM_KILLFOCUS, 0, LPARAM(0)),
        }
    }

    /// Releases the view and destroys its window.
    pub fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        // SAFETY: plain WebView2 and Win32 calls on the main thread.
        unsafe {
            let _ = self.controller.Close();
            let _ = DestroyWindow(self.window);
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.close();
    }
}

/// The window the browser draws the page in and reads input from.
struct PageWindow(HWND);

impl PageWindow {
    fn post(&self, message: u32, wparam: usize, lparam: LPARAM) {
        // SAFETY: a message posted to a live window.
        let _ = unsafe { PostMessageW(Some(self.0), message, WPARAM(wparam), lparam) };
    }

    /// Posts a wheel scroll of `dx`, `dy` px at window point `x`, `y`.
    fn scroll(&self, x: f64, y: f64, dx: f64, dy: f64) {
        // Wheel messages carry screen coordinates.
        let mut point = POINT {
            x: x.round() as i32,
            y: y.round() as i32,
        };
        // SAFETY: a live window and point.
        let _ = unsafe { ClientToScreen(self.0, &mut point) };
        let screen = client_point(f64::from(point.x), f64::from(point.y));
        // A notch is 120 units, which Chromium scrolls 100 px.
        let notches = |pixels: f64| ((pixels * 1.2).round() as i32 as i16 as u16) as usize;
        // Wheel up is positive; horizontally, right is positive.
        if dy != 0.0 {
            self.post(WM_MOUSEWHEEL, notches(-dy) << 16, screen);
        }
        if dx != 0.0 {
            self.post(WM_MOUSEHWHEEL, notches(dx) << 16, screen);
        }
    }

    /// Posts modifier key-downs, then the key, then its text (none while Control or Alt is held).
    fn key_down(&self, key: &str, text: Option<&str>, modifiers: Modifiers) {
        for virtual_key in modifier_keys(modifiers) {
            self.post(WM_KEYDOWN, virtual_key.0 as usize, LPARAM(1));
        }
        if let Some(virtual_key) = virtual_key(key) {
            let message = if modifiers.alt {
                WM_SYSKEYDOWN
            } else {
                WM_KEYDOWN
            };
            self.post(message, virtual_key.0 as usize, LPARAM(1));
        }
        let typed = text.filter(|_| !modifiers.control && !modifiers.alt);
        self.type_text(typed.unwrap_or_default());
    }

    /// Posts the key-up, then the modifier key-ups.
    fn key_up(&self, key: &str, modifiers: Modifiers) {
        // Repeat count 1, previous state down, transition up.
        let released = LPARAM(0xC000_0001_u32 as isize);
        if let Some(virtual_key) = virtual_key(key) {
            let message = if modifiers.alt { WM_SYSKEYUP } else { WM_KEYUP };
            self.post(message, virtual_key.0 as usize, released);
        }
        for virtual_key in modifier_keys(modifiers) {
            self.post(WM_KEYUP, virtual_key.0 as usize, released);
        }
    }

    fn type_text(&self, text: &str) {
        for unit in text.encode_utf16() {
            self.post(WM_CHAR, usize::from(unit), LPARAM(1));
        }
    }
}

/// The press message for `button`; the left button's second click is a double click.
fn press_message(button: Button, clicks: u32) -> u32 {
    match (button, clicks >= 2) {
        (Button::Left, true) => WM_LBUTTONDBLCLK,
        (Button::Left, false) => WM_LBUTTONDOWN,
        (Button::Middle, _) => WM_MBUTTONDOWN,
        (Button::Right, _) => WM_RBUTTONDOWN,
    }
}

/// The release message for `button`.
fn release_message(button: Button) -> u32 {
    match button {
        Button::Left => WM_LBUTTONUP,
        Button::Middle => WM_MBUTTONUP,
        Button::Right => WM_RBUTTONUP,
    }
}

/// Packs a window point into a mouse message's `lParam`.
fn client_point(x: f64, y: f64) -> LPARAM {
    let (x, y) = (x.round() as i32, y.round() as i32);
    LPARAM(((y & 0xFFFF) << 16 | (x & 0xFFFF)) as isize)
}

/// The `MK_` flag for a held button, for a mouse message's `wParam`.
fn held_flag(button: Option<Button>) -> usize {
    match button {
        Some(Button::Left) => 0x0001,
        Some(Button::Right) => 0x0002,
        Some(Button::Middle) => 0x0010,
        None => 0,
    }
}

/// The held modifiers as virtual keys.
fn modifier_keys(modifiers: Modifiers) -> Vec<VIRTUAL_KEY> {
    [
        (modifiers.shift, VK_SHIFT),
        (modifiers.control, VK_CONTROL),
        (modifiers.alt, VK_MENU),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .map(|(_, key)| key)
    .collect()
}

/// The virtual key for a GPUI key name.
fn virtual_key(key: &str) -> Option<VIRTUAL_KEY> {
    Some(match key {
        "enter" => VK_RETURN,
        "backspace" => VK_BACK,
        "tab" => VK_TAB,
        "escape" => VK_ESCAPE,
        "space" => VK_SPACE,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        "delete" => VK_DELETE,
        "insert" => VK_INSERT,
        _ => {
            let mut characters = key.chars();
            let first = characters.next()?;
            if let Some(number) = key.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
                && (1..=24).contains(&number)
            {
                return Some(VIRTUAL_KEY(VK_F1.0 + number - 1));
            }
            if characters.next().is_some() || !first.is_ascii_alphanumeric() {
                return None;
            }
            VIRTUAL_KEY(first.to_ascii_uppercase() as u16)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_page_hosts_scheme_and_the_browsers_own_are_let_through() {
        let own = format!("{}://files/scenes/a.html", core::scheme());
        assert_eq!(Asked::of(&own), Asked::Own);
        assert_eq!(Asked::of(&own.to_uppercase()), Asked::Own);
        for local in ["data:text/plain,a", "blob:nr://files/1", "about:blank"] {
            assert_eq!(Asked::of(local), Asked::Local, "{local}");
        }
        for elsewhere in [
            "https://example.com/ping",
            "http://127.0.0.1:9/ping",
            "file:///C:/Windows/win.ini",
            "\\\\server\\share\\a.png",
            "",
        ] {
            assert_eq!(Asked::of(elsewhere), Asked::Elsewhere, "{elsewhere}");
        }
    }

    #[test]
    fn keys_are_the_virtual_keys_windows_names() {
        assert_eq!(virtual_key("a"), Some(VIRTUAL_KEY(b'A'.into())));
        assert_eq!(virtual_key("7"), Some(VIRTUAL_KEY(b'7'.into())));
        assert_eq!(virtual_key("enter"), Some(VK_RETURN));
        assert_eq!(virtual_key("f5"), Some(VIRTUAL_KEY(VK_F1.0 + 4)));
        assert_eq!(virtual_key("é"), None);
    }
}
