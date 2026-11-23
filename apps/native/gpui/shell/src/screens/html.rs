//! An HTML file on the drive, run as a live page. Selected, it runs beside the columns; opened,
//! it fills the tab. Only its own folder is served and the network is closed
//! (`webhost::HtmlPage`). When the file, or a folder file the page read, changes on disk, the
//! page reloads (the core's `page::watch`). A database the page queried (`window.__nr.sql`) is
//! different: when something else changes it, the page stays and is notified.
//!
//! The web view sits under the window's GPUI view (`kit::hosted`), which leaves a hole for it.
//! Presses, drags and scrolls GPUI receives over the hole are forwarded (`kit::hosted_input`),
//! and the pressed view takes the keys. AppKit sends it every key, and keys the page ignores
//! reach the menu bar (`keys`). Off macOS the web view is offscreen: this view draws its
//! snapshots, forwards input (`drawn`) and shows, in the core's words, why a page failed to load.

#[cfg(target_os = "macos")]
use super::on_bubble;
use super::{Ask, Screen};
use crate::script::probe::Probe;
use crate::script::{Answer, Pointer, Route, Step, Table, workspace};
use gpui::{
    App, Bounds, Context, Entity, EventEmitter, FocusHandle, Hitbox, HitboxBehavior,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled, Window, canvas,
};
#[cfg(target_os = "macos")]
use gpui::{MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollEvent};
use kit::controls::placed;
use kit::core::c;
#[cfg(target_os = "macos")]
use kit::hosted_input::Forward;
use kit::nr::*;
#[cfg(not(target_os = "macos"))]
use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};
use webhost::HtmlPage;

pub struct Html {
    frame: Bounds<Pixels>,
    focus: FocusHandle,
    page: Rc<HtmlPage>,
    /// The core's id for this page.
    id: u64,
    /// Why the page failed to load, shown in its place (off macOS).
    #[cfg(not(target_os = "macos"))]
    failed: Rc<RefCell<Option<String>>>,
    #[cfg(target_os = "macos")]
    hosted: kit::hosted::HostedView,
    #[cfg(not(target_os = "macos"))]
    drawn: drawn::Drawn,
    #[cfg(target_os = "macos")]
    forward: Forward,
}

impl Html {
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    pub fn new(path: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let page = Rc::new(HtmlPage::new(Path::new(path)));
        let named = c(path);
        // SAFETY: a NUL-terminated path.
        let id = unsafe { nr_html_page_open(named.as_ptr()) };
        #[cfg(not(target_os = "macos"))]
        let failed = Rc::new(RefCell::new(None));
        #[cfg(not(target_os = "macos"))]
        let said = failed.clone();
        let loaded = Rc::downgrade(&page);
        let woken = cx.weak_entity();
        let async_cx = cx.to_async();
        page.on_loaded(move |failure| {
            let Some(page) = Weak::upgrade(&loaded) else {
                return;
            };
            if let Some(failure) = &failure {
                eprintln!("html\tthe page did not load: {failure}");
            }
            #[cfg(not(target_os = "macos"))]
            {
                *said.borrow_mut() = failure.as_ref().map(ToString::to_string);
            }
            let served: Vec<String> = page.served().into_iter().collect();
            let served = c(&served.join("\n"));
            // SAFETY: a NUL-terminated list.
            unsafe { nr_html_page_loaded(id, served.as_ptr()) };
            let _ = woken.update(&mut async_cx.clone(), |_, cx| cx.notify());
        });
        let starting = page.clone();
        webhost::run::spawn(async move { starting.start().await });
        let focus = cx.focus_handle();
        #[cfg(target_os = "macos")]
        kit::hosted_input::follow_focus(page.view(), &focus, window, cx);
        Self {
            frame: Bounds::default(),
            focus,
            #[cfg(target_os = "macos")]
            hosted: hosted(page.view(), window),
            #[cfg(not(target_os = "macos"))]
            drawn: drawn::Drawn::new(page.clone(), cx),
            page,
            id,
            #[cfg(not(target_os = "macos"))]
            failed,
            #[cfg(target_os = "macos")]
            forward: Forward::default(),
        }
    }
}

/// A view under the window's GPUI view, on the panel background, filled by `view` (a web view
/// or a PDF's pages).
#[cfg(target_os = "macos")]
pub fn hosted(view: *mut std::ffi::c_void, window: &mut Window) -> kit::hosted::HostedView {
    let ground = kit::theme::stated(kit::theme::Token::Panel);
    let hosted = kit::hosted::HostedView::plain(
        window,
        [
            f64::from(ground.r),
            f64::from(ground.g),
            f64::from(ground.b),
        ],
    );
    hosted.hold(view, None);
    hosted
}

/// What `page` shows, for a check: its text, the colours of its body and each element with an
/// id, where it was served from, and the requests that failed.
fn probe_page(page: Rc<HtmlPage>, cx: &mut App) {
    const SCRIPT: &str = "(() => { const look = (e) => { const s = getComputedStyle(e); return { text: e.innerText ?? e.textContent, color: s.color, background: s.backgroundColor } };\
            const ids = {}; document.querySelectorAll('[id]').forEach((e, i) => { if (i < 20) ids[e.id] = look(e) });\
            return JSON.stringify({ href: location.href, title: document.title, text: document.body ? document.body.innerText : '', body: document.body ? look(document.body) : null, ids }) })()";
    cx.foreground_executor()
        .spawn(async move {
            let said = page.evaluate(SCRIPT).await;
            let shown = match &said {
                Ok(Some(json)) => serde_json::from_str(json).unwrap_or(serde_json::Value::Null),
                _ => serde_json::Value::Null,
            };
            let log = page.log();
            let mut state = serde_json::json!({
                "page": if shown.is_null() { serde_json::json!({}) } else { shown },
                "blocked": log.blocked(),
                "refused": log.refused(),
                "missing": log.missing(),
                "errors": log.page_errors(),
                "served": page.served().into_iter().collect::<Vec<_>>(),
            });
            if let Err(error) = said {
                state["evaluate"] = serde_json::Value::String(error);
            }
            eprintln!("html\t{state}");
        })
        .detach();
}

impl Html {
    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        self.hosted.place(self.frame, window);
        #[cfg(not(target_os = "macos"))]
        match self.failed.borrow().as_ref() {
            Some(failure) => say_in_middle(failure, self.frame, window, cx),
            None => self.drawn.paint(self.frame, window),
        }
        self.listen(hitbox, window, cx);
    }

    /// Forwards presses, drags, releases and scrolls over the page to its web view. A press
    /// gives the view the keys.
    #[cfg(not(target_os = "macos"))]
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, _: &mut Context<Self>) {
        let focus = self.focus.clone();
        drawn::listen(&self.drawn.shown, hitbox, window, move |window, cx| {
            window.focus(&focus, cx)
        });
    }

    /// Forwards presses, drags, releases and scrolls over the page.
    #[cfg(target_os = "macos")]
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        pass_pointer_on(window, hitbox, move |hitbox, window, cx| {
            let over = hitbox.is_hovered(window);
            entity.update(cx, |html, cx| {
                let view = html.page.view();
                if html.forward.pass_on(view, over, window) && over {
                    window.focus(&html.focus, cx);
                }
            });
        });
    }
}

/// Sends each bubbling press, release and scroll, and each move with a button held, to
/// `pass_on`. `hitbox` tells whether the pointer is over the hosted view.
#[cfg(target_os = "macos")]
pub fn pass_pointer_on(
    window: &mut Window,
    hitbox: &Hitbox,
    pass_on: impl Fn(&Hitbox, &mut Window, &mut App) + 'static,
) {
    let pass_on = Rc::new(pass_on);
    let (on, over) = (pass_on.clone(), hitbox.clone());
    on_bubble(window, move |_: &MouseDownEvent, window, cx| {
        on(&over, window, cx)
    });
    let (on, over) = (pass_on.clone(), hitbox.clone());
    on_bubble(window, move |event: &MouseMoveEvent, window, cx| {
        if event.pressed_button.is_some() {
            on(&over, window, cx);
        }
    });
    let (on, over) = (pass_on.clone(), hitbox.clone());
    on_bubble(window, move |_: &MouseUpEvent, window, cx| {
        on(&over, window, cx)
    });
    let (on, over) = (pass_on, hitbox.clone());
    on_bubble(window, move |_: &ScrollEvent, window, cx| {
        on(&over, window, cx)
    });
}

/// Paints `words` centred in `frame` on the panel background, explaining a missing platform
/// view.
#[cfg(not(target_os = "macos"))]
pub fn say_in_middle(words: &str, frame: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    use kit::theme::{Face, color};
    kit::draw::fill(window, frame, color(kit::theme::Token::Panel));
    let face = Face::sans(13.0, 400.0);
    let room = f32::from(frame.size.width) - 32.0;
    let wide = kit::text::width(words, face).min(room);
    kit::text::draw_line(
        window,
        cx,
        words,
        face,
        color(kit::theme::Token::MutedForeground),
        f32::from(frame.origin.x) + (f32::from(frame.size.width) - wide) / 2.0,
        f32::from(frame.origin.y) + f32::from(frame.size.height) / 2.0 - 10.0,
        20.0,
        room,
        None,
    );
}

impl Drop for Html {
    fn drop(&mut self) {
        nr_html_page_close(self.id);
    }
}

impl EventEmitter<Ask> for Html {}

impl Screen for Html {
    /// On redraw, reloads the page if a file it read changed since loading, and notifies it of
    /// databases it queried that something else changed.
    fn reload(&mut self, _: &mut Window, _: &mut Context<Self>) {
        if nr_html_page_changed(self.id) {
            self.page.reload();
        } else if self.page.data_changed() {
            let page = self.page.clone();
            webhost::run::spawn(async move { page.tell_data_changed().await });
        }
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, _: &App) {
        if self.focus.is_focused(window) {
            probe.keys = "HtmlFileView".to_owned();
        }
    }
}

impl Render for Html {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let html = cx.entity();
        let placed = placed("html", cx.entity_id(), self.frame).track_focus(&self.focus);
        // VoiceOver reads the page's web view where this element stands.
        #[cfg(target_os = "macos")]
        let placed = {
            use gpui::StatefulInteractiveElement;
            placed
                .role(gpui::Role::Group)
                .accessibility_id(self.hosted.read_in_place())
        };
        // Off macOS, keys the view receives go to the page's web view.
        #[cfg(not(target_os = "macos"))]
        let placed = {
            let (down, up) = (self.drawn.shown.clone(), self.drawn.shown.clone());
            placed
                .on_key_down(move |event, _, _| down.key_down(event))
                .on_key_up(move |event, _, _| up.key_up(event))
        };
        placed.child(
            canvas(
                |bounds, window, _| {
                    // The page shows through the window here.
                    if cfg!(target_os = "macos") {
                        kit::hosted::cut_used(bounds, None);
                    }
                    window.insert_hitbox(bounds, HitboxBehavior::Normal)
                },
                move |_, hitbox, window, cx| {
                    html.update(cx, |html, cx| html.paint(&hitbox, window, cx))
                },
            )
            .size_full(),
        )
    }
}

// MARK: A check's steps

pub fn register(table: &mut Table) {
    table.answer(Route::Kind(NrScriptKind::HtmlProbe), probe_step);
}

/// Reports what the open HTML page shows, or that none is open.
fn probe_step(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let open: Option<Entity<Html>> = workspace(window)
        .read(cx)
        .opened
        .as_ref()
        .and_then(|opened| opened.screen.view().downcast::<Html>().ok());
    match open {
        Some(html) => {
            let page = html.read(cx).page.clone();
            probe_page(page, cx);
        }
        None => eprintln!("html\tnone open"),
    }
    Answer::Done
}

/// Off macOS a live page is a web view in an offscreen window (`webhost`). The view draws
/// snapshots of it, frequently while the person interacts and occasionally otherwise, and
/// forwards presses, drags, scrolls and keys.
#[cfg(not(target_os = "macos"))]
pub mod drawn {
    use crate::screens::{on_bubble, on_bubble_over};
    use gpui::{
        App, AsyncApp, Bounds, Context, Hitbox, KeyDownEvent, KeyUpEvent, MouseButton,
        MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, RenderImage, ScrollEvent, Task,
        WeakEntity, Window,
    };
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;
    use webhost::{Button, HtmlPage, Input, Modifiers};

    /// Snapshot interval while the person interacts with the page, and otherwise.
    const BUSY: Duration = Duration::from_millis(33);
    const IDLE: Duration = Duration::from_millis(250);
    /// How long after the last press or key snapshots stay frequent.
    const BUSY_SECONDS: f64 = 1.5;

    pub struct Shown {
        pub page: Rc<HtmlPage>,
        image: RefCell<Option<Arc<RenderImage>>>,
        /// Snapshots stay frequent until this time (motion clock seconds).
        busy_until: Cell<f64>,
        /// The button pressed over the page, for the drags and release that follow.
        pressed: Cell<Option<Button>>,
        /// Where the page was last drawn, in window points.
        frame: Cell<Bounds<Pixels>>,
    }

    /// A live page shown in a view, with snapshots kept current while the view lives.
    pub struct Drawn {
        pub shown: Rc<Shown>,
        _drawing: Task<()>,
    }

    /// Converts a page host snapshot (premultiplied sRGB RGBA) to a GPUI image.
    fn image(picture: webhost::Picture) -> Option<Arc<RenderImage>> {
        let mut bgra = picture.rgba;
        for pixel in bgra.as_chunks_mut::<4>().0 {
            gpui::swap_rgba_pa_to_bgra(pixel);
        }
        kit::display::image(picture.width, picture.height, bgra)
    }

    impl Drawn {
        /// Shows `page` in `cx`'s view, which redraws with each new snapshot.
        pub fn new<T: 'static>(page: Rc<HtmlPage>, cx: &mut Context<T>) -> Self {
            let shown = Rc::new(Shown {
                page,
                image: RefCell::new(None),
                busy_until: Cell::new(kit::motion::now() + BUSY_SECONDS),
                pressed: Cell::new(None),
                frame: Cell::new(Bounds::default()),
            });
            let kept = shown.clone();
            let drawing = cx.spawn(async move |view: WeakEntity<T>, cx: &mut AsyncApp| {
                keep_pictures(&kept, view, cx).await;
            });
            Self {
                shown,
                _drawing: drawing,
            }
        }

        /// Sizes the page to `frame` and draws its last snapshot there.
        pub fn paint(&self, frame: Bounds<Pixels>, window: &mut Window) {
            self.shown.frame.set(frame);
            self.shown.page.resize(
                f32::from(frame.size.width).round() as u32,
                f32::from(frame.size.height).round() as u32,
            );
            if let Some(image) = self.shown.image.borrow().clone() {
                let _ = window.paint_image(frame, frame, Default::default(), image, 0, false);
            }
        }
    }

    /// Takes snapshots of the page, redrawing `view` after each, while the view lives.
    async fn keep_pictures<T: 'static>(shown: &Shown, view: WeakEntity<T>, cx: &mut AsyncApp) {
        loop {
            if let Ok(picture) = shown.page.picture().await {
                *shown.image.borrow_mut() = image(picture);
                if view.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
            cx.background_executor()
                .timer(shown.picture_interval())
                .await;
            if view.upgrade().is_none() {
                return;
            }
        }
    }

    impl Shown {
        /// Time until the next snapshot, short while the person interacts with the page.
        fn picture_interval(&self) -> Duration {
            if kit::motion::now() < self.busy_until.get() {
                BUSY
            } else {
                IDLE
            }
        }

        /// Forwards a press over the page after giving it the keys. Returns whether the page
        /// handles that button.
        fn press(&self, event: &MouseDownEvent) -> bool {
            let Some(button) = button(event.button) else {
                return false;
            };
            let (x, y) = self.at(event.position);
            self.pressed.set(Some(button));
            self.pass_on(Input::Focus(true));
            self.pass_on(Input::Press {
                x,
                y,
                button,
                clicks: event.click_count as u32,
            });
            true
        }

        /// Sends `input` to the page and makes snapshots frequent for a while.
        pub fn pass_on(&self, input: Input) {
            self.busy_until.set(kit::motion::now() + BUSY_SECONDS);
            self.page.input(&input);
        }

        /// Converts `position` (window points) to page coordinates.
        fn at(&self, position: gpui::Point<Pixels>) -> (f64, f64) {
            let frame = self.frame.get();
            (
                f64::from(f32::from(position.x - frame.origin.x)),
                f64::from(f32::from(position.y - frame.origin.y)),
            )
        }

        /// Forwards a key the view received.
        pub fn key_down(&self, event: &KeyDownEvent) {
            self.pass_on(Input::Key {
                key: event.keystroke.key.clone(),
                text: event.keystroke.key_char.clone(),
                modifiers: modifiers(&event.keystroke.modifiers),
            });
        }

        pub fn key_up(&self, event: &KeyUpEvent) {
            self.pass_on(Input::KeyUp {
                key: event.keystroke.key.clone(),
                modifiers: modifiers(&event.keystroke.modifiers),
            });
        }
    }

    fn modifiers(held: &gpui::Modifiers) -> Modifiers {
        Modifiers {
            shift: held.shift,
            control: held.control,
            alt: held.alt,
            platform: held.platform,
        }
    }

    fn button(pressed: MouseButton) -> Option<Button> {
        match pressed {
            MouseButton::Left => Some(Button::Left),
            MouseButton::Middle => Some(Button::Middle),
            MouseButton::Right => Some(Button::Right),
            MouseButton::Navigate(_) => None,
        }
    }

    /// Forwards presses over the page, the drags and release that follow, and moves and scrolls
    /// over it. `pressed` is called on a press over the page, so the view can take the keys.
    pub fn listen(
        shown: &Rc<Shown>,
        hitbox: &Hitbox,
        window: &mut Window,
        pressed: impl Fn(&mut Window, &mut App) + 'static,
    ) {
        let page = shown.clone();
        on_bubble_over(window, hitbox, move |event: &MouseDownEvent, window, cx| {
            if page.press(event) {
                pressed(window, cx);
            }
        });
        let (page, over) = (shown.clone(), hitbox.clone());
        on_bubble(window, move |event: &MouseMoveEvent, window, _| {
            let held = page.pressed.get();
            if held.is_some() || over.is_hovered(window) {
                let (x, y) = page.at(event.position);
                page.page.input(&Input::Move { x, y, held });
            }
        });
        let page = shown.clone();
        on_bubble(window, move |event: &MouseUpEvent, _, _| {
            if let Some(button) = page.pressed.take() {
                let (x, y) = page.at(event.position);
                page.pass_on(Input::Release { x, y, button });
            }
        });
        let page = shown.clone();
        on_bubble_over(window, hitbox, move |event: &ScrollEvent, _, _| {
            let (x, y) = page.at(event.position);
            let delta = event.delta.pixel_delta(gpui::px(20.0));
            page.pass_on(Input::Scroll {
                x,
                y,
                dx: -f64::from(f32::from(delta.x)),
                dy: -f64::from(f32::from(delta.y)),
            });
        });
    }
}
