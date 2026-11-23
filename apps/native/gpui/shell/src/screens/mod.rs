//! The window's screens. Each is its own GPUI view, placed at the frame the core lays out for it
//! (`nr_window_layout`): the title bar, the folder as a list or board, its columns, settings, the
//! Changes tab, the welcome screen when no folder is chosen, and the open file in its view
//! (`nr_opened_view`).
//!
//! A screen implements `Screen`. It reads its content from the core (`reload`), takes a frame
//! (`place`), may take keyboard focus (`keys`) and reports its state to checks (`probe`,
//! `point_of`). It sends requests to the window as an `Ask`. The window (`workspace.rs`) creates
//! and places the screens, routes keys and owns the overlays. A screen registers its check steps
//! in `script` (`register`, listed in `script::table`).
//!
//! To add a file view, replace the placeholder module for its kind, keep its `Screen` impl and
//! register its steps. To add another kind of screen, add a module here, a slot in `Workspace`
//! and a line in `script::table`.

pub mod board;
pub mod changes;
pub mod columns;
pub mod controls;
pub mod details;
pub mod html;
pub mod list;
pub mod model;
pub mod onboarding;
pub mod page;
pub mod page_players;
pub mod pdf;
pub mod preview;
pub mod settings;
pub mod sound;
pub mod source;
#[cfg(target_os = "macos")]
pub mod system_preview;
pub mod table;
pub mod terminal;
pub mod terminals;
pub mod title_bar;

use crate::script::probe::Probe;
use crate::workspace::Workspace;
use gpui::{
    AnyView, App, AppContext, Bounds, Context, DispatchPhase, Entity, EventEmitter, FocusHandle,
    Hitbox, MouseEvent, Pixels, Point, Render, Subscription, Window,
};
use kit::lists::rename::Said;
use kit::menu::{Again, Run};
use kit::menu_model::Menu;
use kit::nr::{NrMenu, NrOpenedViewKind, NrWindowLayout};

/// What a screen asks of the window.
pub enum Ask {
    /// The core's folders, selection, tabs or open file changed, so the window catches up
    /// (`Workspace::changed`).
    Changed,
    /// A core menu (`which`, rebuilt by `again`) at window point `at`. With `below` it hangs
    /// under a control; otherwise it is a context menu.
    Menu {
        which: NrMenu,
        again: Again,
        at: Point<Pixels>,
        below: bool,
    },
    /// A folder menu, like `Menu`, for the folder in column `folder.column`. Its Open opens the
    /// file the way the screen does, and Import copies the chosen files into that folder.
    FileMenu {
        which: NrMenu,
        again: Again,
        at: Point<Pixels>,
        folder: Folder,
    },
    /// A menu of the screen's own choices (a select's), hanging under its control at `at`. The
    /// chosen entry goes to `run`.
    Choices {
        menu: Menu,
        at: Point<Pixels>,
        run: Run,
    },
    /// The theme changed; every screen redraws in the new colours.
    ThemeChanged,
    /// The pointer rests on a control with a tooltip (its window frame and text), or left it
    /// (`None`).
    Tooltip(Option<(Bounds<Pixels>, String)>),
    /// A core reply to carry out (`effects::carry_out`).
    Said(Said),
}

/// What a folder menu's Open does: open the file the menu was opened on.
pub type Open = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// The folder a folder menu acts on: its column, and what its Open does (`None` for the
/// empty-space menu).
#[derive(Clone)]
pub struct Folder {
    pub column: usize,
    pub open: Option<Open>,
}

pub trait Screen: Render + EventEmitter<Ask> + Sized {
    /// The core changed what the screen shows; read it again.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>);
    /// The window placed the screen at `frame` (window points), laid out as `laid`.
    fn place(&mut self, frame: Bounds<Pixels>, laid: &NrWindowLayout, cx: &mut Context<Self>);
    /// The view that gets keyboard focus when the screen does, if any.
    fn keys(&self, _cx: &App) -> Option<FocusHandle> {
        None
    }
    /// What a check reads off the screen (`ev:observe`, `ev:state`).
    fn probe(&self, _probe: &mut Probe, _window: &Window, _cx: &App) {}
    /// The centre of the drawn row or card titled `title`, for a check's `at:` step.
    fn point_of(&self, _title: &str, _cx: &App) -> Option<Point<Pixels>> {
        None
    }
    /// The scroller a wheel at `at` moves, if any: its scroll offset, visible height and content
    /// height (`ev:scrolled`).
    fn scrolled_at(&self, _at: Point<Pixels>, _cx: &App) -> Option<(f32, f32, f32)> {
        None
    }
}

/// A screen the window holds, whatever its type.
pub trait Hosted {
    fn view(&self) -> AnyView;
    fn reload(&self, window: &mut Window, cx: &mut App);
    fn place(&self, frame: Bounds<Pixels>, laid: &NrWindowLayout, cx: &mut App);
    fn keys(&self, cx: &App) -> Option<FocusHandle>;
    fn probe(&self, probe: &mut Probe, window: &Window, cx: &App);
    fn point_of(&self, title: &str, cx: &App) -> Option<Point<Pixels>>;
    fn scrolled_at(&self, at: Point<Pixels>, cx: &App) -> Option<(f32, f32, f32)>;
    /// Sends the screen's `Ask`s to `hear` for as long as the subscription is kept.
    fn heard(&self, window: &mut Window, cx: &mut Context<Workspace>, hear: Hear) -> Subscription;
}

/// How the window hears a screen's `Ask`.
pub type Hear = fn(&mut Workspace, &Ask, &mut Window, &mut Context<Workspace>);

impl<T: Screen> Hosted for Entity<T> {
    fn view(&self) -> AnyView {
        self.clone().into()
    }

    fn reload(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |screen, cx| screen.reload(window, cx));
    }

    fn place(&self, frame: Bounds<Pixels>, laid: &NrWindowLayout, cx: &mut App) {
        self.update(cx, |screen, cx| screen.place(frame, laid, cx));
    }

    fn keys(&self, cx: &App) -> Option<FocusHandle> {
        self.read(cx).keys(cx)
    }

    fn probe(&self, probe: &mut Probe, window: &Window, cx: &App) {
        self.read(cx).probe(probe, window, cx);
    }

    fn point_of(&self, title: &str, cx: &App) -> Option<Point<Pixels>> {
        self.read(cx).point_of(title, cx)
    }

    fn scrolled_at(&self, at: Point<Pixels>, cx: &App) -> Option<(f32, f32, f32)> {
        self.read(cx).scrolled_at(at, cx)
    }

    fn heard(&self, window: &mut Window, cx: &mut Context<Workspace>, hear: Hear) -> Subscription {
        cx.subscribe_in(self, window, move |workspace, _, ask: &Ask, window, cx| {
            hear(workspace, ask, window, cx)
        })
    }
}

/// The view for an open file of `kind`, made for the file the core has open (`path` is the file
/// a live page runs), and its name for checks (`Probe::file`). `None` when no file is open.
pub fn opened(
    kind: NrOpenedViewKind,
    path: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<(Box<dyn Hosted>, &'static str)> {
    fn made<T: Screen>(screen: Entity<T>, name: &'static str) -> (Box<dyn Hosted>, &'static str) {
        (Box::new(screen), name)
    }
    Some(match kind {
        NrOpenedViewKind::None => return None,
        // Terminals opened together share the tab, side by side.
        NrOpenedViewKind::Terminal if kit::nr::nr_opened_terminals() > 1 => made(
            cx.new(|cx| terminals::Terminals::new(window, cx)),
            "TerminalsView",
        ),
        NrOpenedViewKind::Terminal => made(
            cx.new(|cx| terminal::Terminal::new(window, cx)),
            "TerminalView",
        ),
        NrOpenedViewKind::Html => made(
            cx.new(|cx| html::Html::new(path, window, cx)),
            "HtmlFileView",
        ),
        NrOpenedViewKind::Page => made(cx.new(|cx| page::Page::new(window, cx)), "PageView"),
        NrOpenedViewKind::Pdf => made(cx.new(|cx| pdf::Pdf::new(window, cx)), "PdfFileView"),
        #[cfg(target_os = "macos")]
        NrOpenedViewKind::SystemPreview => made(
            cx.new(|cx| system_preview::SystemPreview::new(window, cx)),
            "SystemPreviewFileView",
        ),
        // Only a Mac has a system previewer; elsewhere the core shows the file's details.
        #[cfg(not(target_os = "macos"))]
        NrOpenedViewKind::SystemPreview => made(
            cx.new(|cx| details::Details::new(window, cx)),
            "DetailsView",
        ),
        NrOpenedViewKind::Table => {
            made(cx.new(|cx| table::Table::new(window, cx)), "TableFileView")
        }
        NrOpenedViewKind::Model => {
            made(cx.new(|cx| model::Model::new(window, cx)), "ModelFileView")
        }
        NrOpenedViewKind::Source => made(
            cx.new(|cx| source::Source::new(window, cx)),
            "SourceFileView",
        ),
        NrOpenedViewKind::Details => made(
            cx.new(|cx| details::Details::new(window, cx)),
            "DetailsView",
        ),
    })
}

/// A folder menu (`menus::ffi::menu_folder`) for column `column`: a row's (`File`) or the empty
/// space's (`Background`). `clicked` is the file pressed and `selected` the files it acts on,
/// read fresh each time. The empty-space Paste is enabled only when the clipboard holds files,
/// which the core learns first (`nr_set_clipboard_holds_files`).
pub fn folder_menu(which: NrMenu, column: usize, clicked: &str, selected: &[String]) -> Again {
    let (clicked, selected) = (clicked.to_owned(), selected.to_vec());
    std::rc::Rc::new(move |cx| {
        if which == NrMenu::Background {
            kit::nr::nr_set_clipboard_holds_files(kit::platform::clipboard_holds_files(cx));
        }
        kit::nr::menu_folder(which, column, &clicked, &selected)
    })
}

/// Handles each pointer event `E` in the bubble phase while the screen is drawn.
pub fn on_bubble<E: MouseEvent>(
    window: &mut Window,
    mut hear: impl FnMut(&E, &mut Window, &mut App) + 'static,
) {
    window.on_mouse_event(move |event: &E, phase, window, cx| {
        if phase == DispatchPhase::Bubble {
            hear(event, window, cx);
        }
    });
}

/// Handles each bubbling pointer event `E` while the pointer is over `hitbox`.
pub fn on_bubble_over<E: MouseEvent>(
    window: &mut Window,
    hitbox: &Hitbox,
    mut hear: impl FnMut(&E, &mut Window, &mut App) + 'static,
) {
    let over = hitbox.clone();
    window.on_mouse_event(move |event: &E, phase, window, cx| {
        if phase == DispatchPhase::Bubble && over.is_hovered(window) {
            hear(event, window, cx);
        }
    });
}
