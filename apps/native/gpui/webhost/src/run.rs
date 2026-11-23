//! Main-thread work as futures. The web view replies through completion handlers on the main
//! thread, so each call is a future its handler resolves (`promise`) and callers write plain
//! `async` code. Any executor can poll them. A program without its own loop calls `run`, which
//! polls on the main dispatch queue (inside the application loop on a Mac) and exits with the
//! future's code. A shell with its own loop, such as GPUI, awaits them in its tasks; `spawn`
//! posts to the same main queue, which that loop drains. Off the Mac the queue is a hidden
//! window's messages on Windows and GLib's main context on Linux (`platform`), which GPUI's loop
//! drives through `pump`.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

type Task = Pin<Box<dyn Future<Output = ()>>>;

thread_local! {
    /// Main-thread tasks by index; a finished task leaves its slot empty.
    static TASKS: RefCell<Vec<Option<Task>>> = const { RefCell::new(Vec::new()) };
}

/// Wakes a task by posting one poll to the main thread until that poll runs.
struct TaskWaker {
    task: usize,
    posted: AtomicBool,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        if !self.posted.swap(true, Ordering::AcqRel) {
            let waker = self.clone();
            platform::post(Box::new(move || {
                waker.posted.store(false, Ordering::Release);
                poll(waker.task, &waker);
            }));
        }
    }
}

fn poll(task: usize, waker: &Arc<TaskWaker>) {
    // Take the task out while it runs, since it may spawn others.
    let Some(mut future) =
        TASKS.with(|tasks| tasks.borrow_mut().get_mut(task).and_then(Option::take))
    else {
        return;
    };
    let waker = Waker::from(waker.clone());
    match future.as_mut().poll(&mut Context::from_waker(&waker)) {
        Poll::Ready(()) => {}
        Poll::Pending => TASKS.with(|tasks| tasks.borrow_mut()[task] = Some(future)),
    }
}

/// Runs `future` on the main thread. Call from the main thread.
pub fn spawn(future: impl Future<Output = ()> + 'static) {
    #[cfg(not(target_os = "macos"))]
    platform::ready();
    let task = TASKS.with(|tasks| {
        let mut tasks = tasks.borrow_mut();
        tasks.push(Some(Box::pin(future)));
        tasks.len() - 1
    });
    Arc::new(TaskWaker {
        task,
        posted: AtomicBool::new(false),
    })
    .wake();
}

/// Runs `main` on the main thread's loop, then exits with its code. On a Mac the process is a
/// prohibited-activation app, with no Dock icon or menu bar.
pub fn run(main: impl Future<Output = i32> + 'static) -> ! {
    spawn(async move {
        let code = main.await;
        std::process::exit(code);
    });
    platform::run_loop()
}

/// Posts `work` to the main thread from any thread. Off the Mac, the reader thread replies to
/// web view requests this way.
#[cfg(not(target_os = "macos"))]
pub fn post_main(work: impl FnOnce() + Send + 'static) {
    platform::post(Box::new(work));
}

/// Prepares the main-thread queue. Call from the main thread; on Windows this creates the
/// queue's window.
#[cfg(not(target_os = "macos"))]
pub(crate) fn ready() {
    platform::ready();
}

/// Runs ready main-thread work without blocking, for a shell with its own loop (GPUI on Linux).
/// Returns `true` while a web view is open; call again within a few ms then, otherwise
/// occasionally. On Windows the thread's messages run the work and this does nothing.
#[cfg(not(target_os = "macos"))]
pub fn pump() -> bool {
    platform::pump()
}

/// A value a handler delivers later on the main thread.
pub struct Promise<T> {
    slot: Rc<RefCell<(Option<T>, Option<Waker>)>>,
}

/// The handler's end of a promise, resolved once.
pub struct Resolver<T> {
    slot: Rc<RefCell<(Option<T>, Option<Waker>)>>,
}

pub fn promise<T>() -> (Resolver<T>, Promise<T>) {
    let slot = Rc::new(RefCell::new((None, None)));
    (Resolver { slot: slot.clone() }, Promise { slot })
}

impl<T> Resolver<T> {
    pub fn resolve(self, value: T) {
        let waker = {
            let mut slot = self.slot.borrow_mut();
            slot.0 = Some(value);
            slot.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Future for Promise<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<T> {
        let mut slot = self.slot.borrow_mut();
        match slot.0.take() {
            Some(value) => Poll::Ready(value),
            None => {
                slot.1 = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// A resolver for a handler that may fire more than once; only the first call resolves.
pub struct Once<T>(RefCell<Option<Resolver<T>>>);

impl<T> Once<T> {
    pub fn new(resolver: Resolver<T>) -> Self {
        Self(RefCell::new(Some(resolver)))
    }

    pub fn resolve(&self, value: T) {
        if let Some(resolver) = self.0.borrow_mut().take() {
            resolver.resolve(value);
        }
    }
}

/// The sending end of a one-value channel, usable from any thread.
pub struct Sender<T> {
    slot: Arc<Mutex<(Option<T>, Option<Waker>)>>,
}

/// The awaiting end of a one-value channel.
pub struct Receiver<T> {
    slot: Arc<Mutex<(Option<T>, Option<Waker>)>>,
}

pub fn channel<T: Send>() -> (Sender<T>, Receiver<T>) {
    let slot = Arc::new(Mutex::new((None, None)));
    (Sender { slot: slot.clone() }, Receiver { slot })
}

impl<T> Sender<T> {
    /// Sends the value. A later send replaces a value not yet received.
    pub fn send(&self, value: T) {
        let waker = {
            let mut slot = self
                .slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            slot.0 = Some(value);
            slot.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<T> {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match slot.0.take() {
            Some(value) => Poll::Ready(value),
            None => {
                slot.1 = Some(context.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// Runs blocking `work` on a new thread (reading stdin, waiting on a process) and awaits it.
pub async fn off_main<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = channel();
    std::thread::spawn(move || sender.send(work()));
    receiver.await
}

/// Waits `seconds` without holding the main thread.
pub async fn sleep(seconds: f64) {
    let duration = std::time::Duration::from_secs_f64(seconds.max(0.0));
    off_main(move || std::thread::sleep(duration)).await
}

#[cfg(target_os = "macos")]
mod platform {
    use dispatch2::DispatchQueue;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    pub fn post(work: Box<dyn FnOnce() + Send>) {
        DispatchQueue::main().exec_async(work);
    }

    pub fn run_loop() -> ! {
        let main = MainThreadMarker::new().expect("the program runs on the main thread");
        let application = NSApplication::sharedApplication(main);
        application.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        application.run();
        std::process::exit(0)
    }
}

/// Windows: main-thread work is posted to a hidden message-only window whose procedure runs it.
/// GPUI's loop and `run_loop` both dispatch its messages, as they do WebView2's.
#[cfg(target_os = "windows")]
mod platform {
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::{LazyLock, Mutex};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
        PostMessageW, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
        WNDCLASSW,
    };
    use windows::core::w;

    type Work = Box<dyn FnOnce() + Send>;

    /// Posted when work is queued.
    const RUN: u32 = WM_APP + 0x4E52;

    static QUEUE: LazyLock<Mutex<Vec<Work>>> = LazyLock::new(|| Mutex::new(Vec::new()));
    /// The work window, once the main thread has created it.
    static WINDOW: AtomicIsize = AtomicIsize::new(0);

    pub fn post(work: Work) {
        QUEUE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(work);
        let window = WINDOW.load(Ordering::Acquire);
        if window != 0 {
            // SAFETY: a window of this process; posting to it is allowed from any thread.
            let _ = unsafe { PostMessageW(Some(HWND(window as _)), RUN, WPARAM(0), LPARAM(0)) };
        }
    }

    /// Creates the work window once, on the main thread. Work posted earlier runs as soon as the
    /// thread reads its messages.
    pub fn ready() {
        if WINDOW.load(Ordering::Acquire) != 0 {
            return;
        }
        // SAFETY: plain Win32 calls on the main thread; the class is registered once.
        unsafe {
            let instance = GetModuleHandleW(None).unwrap_or_default();
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance.into(),
                lpszClassName: w!("NorthroomMainThreadWork"),
                ..Default::default()
            };
            RegisterClassW(&class);
            let Ok(window) = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("NorthroomMainThreadWork"),
                w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                None,
            ) else {
                return;
            };
            WINDOW.store(window.0 as isize, Ordering::Release);
            let _ = PostMessageW(Some(window), RUN, WPARAM(0), LPARAM(0));
        }
    }

    unsafe extern "system" fn procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == RUN {
            run_waiting();
            return LRESULT(0);
        }
        // SAFETY: default handling for every other message.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// Runs queued work in posting order.
    fn run_waiting() {
        loop {
            let waiting: Vec<Work> = std::mem::take(
                &mut *QUEUE
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            if waiting.is_empty() {
                return;
            }
            for work in waiting {
                work();
            }
        }
    }

    /// The thread's message loop, for queued work and every window's messages.
    pub fn run_loop() -> ! {
        // SAFETY: COM on the main thread in a single-threaded apartment, as WebView2 asks.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        ready();
        let mut message = MSG::default();
        // SAFETY: the thread's own message loop.
        unsafe {
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::process::exit(0)
    }

    /// No-op: the thread's messages run the work.
    pub fn pump() -> bool {
        false
    }
}

/// Linux: main-thread work is a source on GLib's default main context, where WebKitGTK replies.
/// `run_loop` runs that context's loop; a shell with its own loop (GPUI) drives it with `pump`.
#[cfg(target_os = "linux")]
mod platform {
    use gtk::glib;

    type Work = Box<dyn FnOnce() + Send>;

    pub fn post(work: Work) {
        glib::MainContext::default().spawn_with_priority(glib::Priority::DEFAULT, async move {
            work();
        });
    }

    pub fn ready() {}

    pub fn run_loop() -> ! {
        glib::MainLoop::new(None, false).run();
        std::process::exit(0)
    }

    /// Runs whatever the main context has ready without blocking. Returns whether a web view is
    /// open, so the context should be driven again soon.
    pub fn pump() -> bool {
        let context = glib::MainContext::default();
        while context.iteration(false) {}
        crate::elsewhere::open_views() > 0
    }
}
