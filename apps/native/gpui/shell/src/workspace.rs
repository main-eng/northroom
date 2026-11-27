//! The window content: title bar, folder list or board, columns, settings, welcome and the
//! open file, each its own screen (`screens`), with overlays above (`overlay`).
//! It keeps no core state. It syncs the screens when the core's revision changes (`sync`), places them per
//! `nr_window_layout`, and handles what screens ask (`asked`). Focus is GPUI's; keys no view
//! takes go to the window (`keys`).

use crate::overlay::Overlay;
use crate::screens::board::Board;
use crate::screens::columns::Columns;
use crate::screens::list::ListScreen;
use crate::screens::onboarding::Onboarding;
use crate::screens::settings::Settings;
use crate::screens::title_bar::TitleBar;
use crate::screens::{self, Ask, Hosted, Screen};
use crate::script::probe::Probe;
use crate::{effects, keys, news};
use gpui::{
    AnyView, App, AppContext, Bounds, Context, DispatchPhase, Entity, FocusHandle, IntoElement,
    KeyDownEvent, MouseButton, MouseUpEvent, ParentElement, Pixels, Point, Render, Styled,
    Subscription, Window, canvas, div, prelude::*,
};
use kit::core::{contains, frame, rect, text};
use kit::lists::{chip, thumbs};
use kit::nr::*;
use kit::theme::Token;

/// The open file and the view showing it.
pub struct Opened {
    pub kind: NrOpenedViewKind,
    /// The view's name in check reports.
    pub name: &'static str,
    pub screen: Box<dyn Hosted>,
    _heard: Subscription,
}

pub struct Workspace {
    /// The window's own focus, held while no view is focused.
    focus: FocusHandle,
    pub bar: Entity<TitleBar>,
    pub list: Option<Entity<ListScreen>>,
    pub board: Option<Entity<Board>>,
    pub columns: Entity<Columns>,
    pub settings: Option<Entity<Settings>>,
    pub onboarding: Option<Entity<Onboarding>>,
    pub opened: Option<Opened>,
    pub overlay: Overlay,
    /// The core revision the screens were last synced to.
    drawn_revision: u64,
    /// Identity of the shown file (`nr_opened_identity`).
    opened_key: String,
    /// Whether the next snapshot includes overlays.
    pub picture_whole: bool,
    first_drawn: bool,
    subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let bar = cx.new(|cx| TitleBar::new(window, cx));
        let columns = cx.new(|cx| Columns::new(window, cx));
        let mut workspace = Self {
            focus,
            bar: bar.clone(),
            list: None,
            board: None,
            columns: columns.clone(),
            settings: None,
            onboarding: None,
            opened: None,
            overlay: Overlay::new(cx),
            drawn_revision: 0,
            opened_key: String::new(),
            picture_whole: true,
            first_drawn: false,
            subscriptions: Vec::new(),
        };
        workspace.listen(&bar, window, cx);
        workspace.listen(&columns, window, cx);
        workspace.sync(window, cx);
        workspace
    }

    /// Subscribes to `screen`'s asks.
    fn listen<T: Screen>(
        &mut self,
        screen: &Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.subscriptions.push(cx.subscribe_in(
            screen,
            window,
            |workspace, _, ask: &Ask, window, cx| workspace.asked(ask, window, cx),
        ));
    }

    /// Creates a screen and subscribes to it.
    fn made<T: Screen>(
        &mut self,
        make: impl FnOnce(&mut Window, &mut Context<T>) -> T,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<T> {
        let screen = cx.new(|cx| make(window, cx));
        self.listen(&screen, window, cx);
        screen
    }

    fn asked(&mut self, ask: &Ask, window: &mut Window, cx: &mut Context<Self>) {
        match ask {
            Ask::Changed => self.changed(window, cx),
            Ask::Menu {
                which,
                again,
                at,
                below,
            } => self
                .overlay
                .menu(*which, again.clone(), *at, *below, None, window, cx),
            Ask::FileMenu {
                which,
                again,
                at,
                folder,
            } => self.overlay.menu(
                *which,
                again.clone(),
                *at,
                false,
                Some(folder.clone()),
                window,
                cx,
            ),
            Ask::Choices { menu, at, run } => {
                let again_menu = menu.clone();
                let again: kit::menu::Again = std::rc::Rc::new(move |_| again_menu.clone());
                let (menu, at, run) = (menu.clone(), *at, run.clone());
                self.overlay.menus.update(cx, |menus, cx| {
                    menus.open_below(menu, at, run, again, window, cx)
                });
            }
            Ask::Said(said) => effects::carry_out(self, said.clone(), window, cx),
            Ask::ThemeChanged => self.theme_changed(window, cx),
            Ask::Tooltip(rest) => self.overlay.tooltips.update(cx, |tips, cx| match rest {
                Some((anchor, words)) => tips.rest_on(*anchor, words, window, cx),
                None => tips.hide(cx),
            }),
        }
    }

    fn screens(&self) -> Vec<&dyn Hosted> {
        let mut all: Vec<&dyn Hosted> = vec![&self.bar, &self.columns];
        if let Some(list) = &self.list {
            all.push(list);
        }
        if let Some(board) = &self.board {
            all.push(board);
        }
        if let Some(settings) = &self.settings {
            all.push(settings);
        }
        if let Some(onboarding) = &self.onboarding {
            all.push(onboarding);
        }
        if let Some(opened) = &self.opened {
            all.push(opened.screen.as_ref());
        }
        all
    }

    /// Syncs the screens with the core's columns and open file.
    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if news::take_refresh() {
            nr_columns_refresh();
        }
        if !self.sync_onboarding(window, cx) {
            return;
        }
        // A just-opened file the window can't show goes to its default app.
        let elsewhere = text(&nr_take_opened_elsewhere());
        if !elsewhere.is_empty() {
            effects::open_elsewhere(&elsewhere, cx);
        }
        let revision = nr_columns_revision();
        if revision == self.drawn_revision {
            return;
        }
        self.drawn_revision = revision;
        self.sync_folder_views(&nr_shell_view(), window, cx);
        self.columns
            .update(cx, |columns, cx| columns.reload(window, cx));
        self.sync_opened(window, cx);
        if let Some(opened) = &self.opened {
            opened.screen.reload(window, cx);
        }
        self.bar.update(cx, |bar, cx| bar.reload(window, cx));
        cx.notify();
    }

    /// Shows onboarding over everything while no folder is open. Returns whether one is open.
    fn sync_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let ready = nr_drive_ready();
        if !ready && self.onboarding.is_none() {
            self.onboarding = Some(self.made(Onboarding::new, window, cx));
        } else if ready && self.onboarding.take().is_some() {
            self.drawn_revision = 0;
        }
        ready
    }

    /// Creates or drops settings, list and board to match `shown`, and reloads those that stay.
    fn sync_folder_views(
        &mut self,
        shown: &NrShellView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match (shown.settings, self.settings.is_some()) {
            (true, false) => self.settings = Some(self.made(Settings::new, window, cx)),
            (false, true) => self.settings = None,
            _ => {}
        }
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, cx| settings.reload(window, cx));
        }
        let as_list = shown.folder == NrFolderView::List;
        match (as_list, self.list.is_some()) {
            (true, false) => self.list = Some(self.made(ListScreen::new, window, cx)),
            (false, true) => self.list = None,
            _ => {}
        }
        if let Some(list) = &self.list {
            list.update(cx, |list, cx| list.reload(window, cx));
        }
        let as_board = shown.folder == NrFolderView::Board;
        match (as_board, self.board.is_some()) {
            (true, false) => self.board = Some(self.made(Board::new, window, cx)),
            (false, true) => self.board = None,
            _ => {}
        }
        if let Some(board) = &self.board {
            board.update(cx, |board, cx| board.reload(window, cx));
        }
    }

    /// Replaces the file view when the shown file changes: another file, another tab, or the
    /// same file moving from beside its column to filling the tab.
    fn sync_opened(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = text(&nr_opened_identity());
        if key == self.opened_key {
            return;
        }
        self.opened_key = key;
        self.opened = None;
        let file = nr_opened_view();
        let Some((screen, name)) = screens::opened(file.view, &text(&file.path), window, cx) else {
            return;
        };
        // Focus a view that takes keys; a view beside its column leaves focus with the column.
        if file.takes_keys
            && let Some(keys) = screen.keys(cx)
        {
            window.focus(&keys, cx);
        }
        let heard = screen.heard(window, cx, |workspace, ask, window, cx| {
            workspace.asked(ask, window, cx)
        });
        self.opened = Some(Opened {
            kind: file.view,
            name,
            screen,
            _heard: heard,
        });
    }

    /// Updates everything after the core changes.
    pub fn changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        news::pump_transport();
        effects::carry_out_left(self, window, cx);
        self.sync(window, cx);
        self.bar.update(cx, |bar, cx| bar.reload(window, cx));
        crate::menu_bar::refresh(cx);
        crate::sessions::tell(window, cx);
        // Refresh open menus, such as the sessions menu as sessions change state.
        self.overlay
            .menus
            .update(cx, |menus, cx| menus.read_again(window, cx));
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, cx| settings.reload(window, cx));
        }
        cx.notify();
    }

    /// Redraws every screen after the theme's colours change.
    pub fn theme_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        kit::theme::forget();
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, cx| settings.reload(window, cx));
        }
        self.changed(window, cx);
        window.refresh();
    }

    /// Shows the dialog the core describes (`nr_prompt`).
    pub fn show_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.dialog(window, cx);
    }

    /// Starts an in-place rename of `id` in the list or a column, or falls back to the rename
    /// dialog (`nr_prompt_rename`). Syncs first so a just-created file (New Folder) is listed.
    pub fn rename(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.sync(window, cx);
        let in_place = match &self.list {
            Some(list) => list.update(cx, |list, cx| list.rename(id, window, cx)),
            None => self
                .columns
                .update(cx, |columns, cx| columns.rename(id, window, cx)),
        };
        if in_place {
            return;
        }
        let named = kit::core::c(id);
        // SAFETY: a NUL-terminated id.
        if unsafe { nr_prompt_rename(nr_shown_column(), named.as_ptr()) } {
            self.show_dialog(window, cx);
        }
    }

    pub fn palette_event(
        &mut self,
        _: &Entity<kit::lists::palette::Palette>,
        event: &kit::lists::palette::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(said) = self.overlay.palette_closed(event, window, cx) {
            effects::carry_out(self, said, window, cx);
        }
        cx.notify();
    }

    /// The core's layout for the current window size and content.
    fn layout(&self, window: &Window) -> NrWindowLayout {
        let view = window.viewport_size();
        nr_window_layout(
            f64::from(view.width),
            f64::from(view.height),
            nr_shell_view().columns,
            self.opened
                .as_ref()
                .map_or(NrOpenedViewKind::None, |opened| opened.kind),
            self.settings.is_some(),
        )
    }

    /// The open file's frame. Beside the columns, it scrolls with them.
    fn opened_frame(&self, laid: &NrWindowLayout, cx: &App) -> Bounds<Pixels> {
        let opened = rect(&laid.opened);
        if !laid.beside {
            return opened;
        }
        let scroll = rect(&laid.columns_scroll);
        let content = rect(&laid.columns_content);
        let scrolled = self.columns.read(cx).scroll();
        frame(
            f32::from(scroll.origin.x + content.origin.x + opened.origin.x) - scrolled,
            f32::from(scroll.origin.y + content.origin.y + opened.origin.y),
            f32::from(opened.size.width),
            f32::from(opened.size.height),
        )
    }

    // MARK: Check reports

    /// The window's state for `ev:observe` and `ev:state`.
    pub fn probe(&self, window: &Window, cx: &App) -> Probe {
        let mut probe = Probe {
            settings: self.settings.is_some(),
            folder: if self.list.is_some() {
                NrFolderView::List
            } else if self.board.is_some() {
                NrFolderView::Board
            } else {
                NrFolderView::Columns
            },
            keys: if self.focus.is_focused(window) || window.focused(cx).is_none() {
                "WorkspaceWindow".to_owned()
            } else {
                String::new()
            },
            ..Probe::default()
        };
        for screen in self.screens() {
            screen.probe(&mut probe, window, cx);
        }
        self.overlay.probe(&mut probe, window, cx);
        if let Some(opened) = &self.opened {
            probe.file = opened.name.to_owned();
            probe.beside = self.layout(window).beside;
        }
        if probe.keys.is_empty() {
            probe.keys = "none".to_owned();
        }
        probe
    }

    /// The centre of the row or card named `title`, in window points.
    pub fn point_of(&self, title: &str, cx: &App) -> Option<Point<Pixels>> {
        let folder = self
            .list
            .as_ref()
            .and_then(|list| list.point_of(title, cx))
            .or_else(|| {
                self.board
                    .as_ref()
                    .and_then(|board| board.point_of(title, cx))
            });
        folder.or_else(|| self.columns.point_of(title, cx))
    }

    /// For the scroller a wheel at `at` would move: offset, visible height and content height.
    /// All zero when nothing scrolls there (`ev:scrolled`).
    pub fn scrolled_under(&self, at: Point<Pixels>, window: &Window, cx: &App) -> (f32, f32, f32) {
        let laid = self.layout(window);
        let opened = self
            .opened
            .as_ref()
            .filter(|_| !laid.opened_hidden)
            .map(|opened| opened.screen.as_ref());
        // Open menus first (deepest wins), then the shown file, then the other screens.
        if let Some(menu) = self.overlay.menus.read(cx).scrolled() {
            return menu;
        }
        opened
            .into_iter()
            .chain(self.screens())
            .find_map(|screen| screen.scrolled_at(at, cx))
            .unwrap_or_default()
    }

    /// The name of the screen under `at`, for checks.
    pub fn hit(&self, at: Point<Pixels>, window: &Window, cx: &App) -> &'static str {
        let laid = self.layout(window);
        let opened = self.opened.as_ref().filter(|_| !laid.opened_hidden);
        if contains(&rect(&laid.title_bar), at) {
            "TitleBarView"
        } else if let Some(opened) = opened.filter(|_| contains(&self.opened_frame(&laid, cx), at))
        {
            opened.name
        } else if self.settings.is_some() {
            "SettingsView"
        } else if !laid.columns_hidden && contains(&rect(&laid.columns_scroll), at) {
            "ColumnView"
        } else if self.list.is_some() {
            "ListView"
        } else if self.board.is_some() {
            "BoardCanvas"
        } else {
            "WorkspaceRootView"
        }
    }
}

impl Workspace {
    /// Places each screen per the core's layout.
    fn place_screens(&self, laid: &NrWindowLayout, cx: &mut Context<Self>) {
        let content = rect(&laid.content);
        self.bar.place(rect(&laid.title_bar), laid, cx);
        if let Some(onboarding) = &self.onboarding {
            onboarding.place(content, laid, cx);
        }
        if let Some(settings) = &self.settings {
            settings.place(content, laid, cx);
        }
        if let Some(list) = &self.list {
            list.place(content, laid, cx);
        }
        if let Some(board) = &self.board {
            board.place(content, laid, cx);
        }
        self.columns.place(rect(&laid.columns_scroll), laid, cx);
        let opened_frame = self.opened_frame(laid, cx);
        if let Some(opened) = &self.opened {
            opened.screen.place(opened_frame, laid, cx);
        }
    }

    /// Logs the first frame once, for window-time and checks.
    fn note_first_frame(&mut self) {
        if !self.first_drawn {
            self.first_drawn = true;
            eprintln!(
                "window\tfirst frame {:.1} ms after start",
                kit::motion::now() * 1000.0
            );
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Focus the window when no view has focus (say, the focused list was replaced by the
        // board), as AppKit makes the window first responder. Otherwise keys would never reach
        // the menu bar shortcuts (`keys`).
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
        self.sync(window, cx);
        let laid = self.layout(window);
        self.place_screens(&laid, cx);
        // Back to front: folder, columns and shown file, settings, title bar,
        // welcome, then overlays.
        let mut shown: Vec<AnyView> = Vec::new();
        if !laid.folder_hidden {
            shown.extend(self.list.as_ref().map(|list| list.view()));
            shown.extend(self.board.as_ref().map(|board| board.view()));
        }
        let opened = (!laid.opened_hidden)
            .then(|| self.opened.as_ref().map(|opened| opened.screen.view()))
            .flatten();
        // The columns and the file beside them share one scroll area, which VoiceOver reads.
        let columns = (!laid.columns_hidden).then(|| {
            div()
                .id("columns-scroll")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .role(kit::lists::voiceover::SCROLL_AREA)
                .a11y_synthetic_children(kit::lists::voiceover::scroll_area_at(
                    rect(&laid.columns_scroll),
                    window.scale_factor(),
                ))
                .child(self.columns.view())
                .children(opened.clone())
        });
        if columns.is_none() {
            shown.extend(opened);
        }
        let mut above: Vec<AnyView> = Vec::new();
        above.extend(self.settings.as_ref().map(|settings| settings.view()));
        above.push(self.bar.view());
        above.extend(self.onboarding.as_ref().map(|onboarding| onboarding.view()));
        // A full-screen video replaces everything. The player draws in window coordinates, so it
        // sits at the window origin.
        let filling = screens::preview::filling_screen(cx)
            .map(|player| div().absolute().top_0().left_0().size_full().child(player));
        let (shown, columns, above) = match filling {
            Some(_) => (Vec::new(), None, Vec::new()),
            None => (shown, columns, above),
        };
        let whole = self.picture_whole;
        let workspace = cx.entity();
        div()
            .id("northroom")
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|workspace, event: &KeyDownEvent, window, cx| {
                keys::window_key(workspace, event, window, cx)
            }))
            // Background, leaving holes for native hosted views (`kit::hosted`).
            .child(kit::hosted::ground(Token::Background))
            // Added first so it hears a release last and can end a drag no view took.
            .child(
                canvas(
                    |_, _, _| (),
                    |_, _, window, _| {
                        window.on_mouse_event(|event: &MouseUpEvent, phase, window, _| {
                            if phase == DispatchPhase::Bubble
                                && event.button == MouseButton::Left
                                && chip::end().is_some()
                            {
                                window.refresh();
                            }
                        });
                    },
                )
                .size_full(),
            )
            .children(shown)
            .children(columns)
            .children(above)
            .children(filling)
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        if whole {
                            chip::paint(window, cx);
                        }
                        // Release pictures no screen drew this frame.
                        thumbs::release_undrawn(window);
                        // Restore the window if a full-screen player went away.
                        screens::preview::give_back_window(window);
                        crate::frame_times::drawn();
                        workspace.update(cx, |workspace, _| workspace.note_first_frame());
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(self.overlay.views(whole))
    }
}
