//! Kit's overlays above the window's screens, as one layer: open menus, the core's dialog,
//! tooltips, notices and the command palette. Menu entries and dialog answers go to the core
//! (`nr_menu_run`, the dialog's own call) and the returned effect is carried out
//! (`effects::carry_out`).

use crate::effects;
use crate::screens::Folder;
use crate::workspace::Workspace;
use gpui::{
    AnyView, App, AppContext, Bounds, Context, Entity, FocusHandle, Pixels, Point, Subscription,
    Window, px, size,
};
use kit::core::c;
use kit::dialog::{Dialogs, Done};
use kit::lists::palette::{self, Palette};
use kit::lists::rename::Said;
use kit::menu::{Again, Menus, Run};
use kit::nr::*;
use kit::toast::Toasts;
use kit::tooltip::Tooltips;
use std::rc::Rc;

pub struct Overlay {
    pub menus: Entity<Menus>,
    pub dialogs: Entity<Dialogs>,
    pub tooltips: Entity<Tooltips>,
    pub toasts: Entity<Toasts>,
    pub palette: Option<Entity<Palette>>,
    /// Focus to restore when the palette closes.
    before_palette: Option<FocusHandle>,
    _palette_events: Option<Subscription>,
}

impl Overlay {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        Self {
            menus: cx.new(Menus::new),
            dialogs: cx.new(Dialogs::new),
            tooltips: cx.new(Tooltips::new),
            toasts: cx.new(Toasts::new),
            palette: None,
            before_palette: None,
            _palette_events: None,
        }
    }

    /// The layer's views, back to front. With `whole` false, leaves out tooltips and menus,
    /// which a window-only snapshot excludes.
    pub fn views(&self, whole: bool) -> Vec<AnyView> {
        let mut views: Vec<AnyView> = vec![self.dialogs.clone().into()];
        views.push(self.toasts.clone().into());
        views.extend(self.palette.clone().map(AnyView::from));
        if whole {
            views.push(self.tooltips.clone().into());
            views.push(self.menus.clone().into());
        }
        views
    }

    /// Opens the core's menu `which` at `at` (below a control when `below`), built by `again`.
    /// Entries run through the core. A folder's menu (`folder`) handles Open and Import itself,
    /// and Share anchors the share sheet where the menu opened.
    #[allow(clippy::too_many_arguments)]
    pub fn menu(
        &self,
        which: NrMenu,
        again: Again,
        at: Point<Pixels>,
        below: bool,
        folder: Option<Folder>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let menu = again(cx);
        let workspace = cx.entity();
        let run: Run = Rc::new(move |entry, window, cx| {
            let id = c(&entry.id);
            // SAFETY: a NUL-terminated string.
            let said = Said::of(&unsafe { nr_menu_run(which, id.as_ptr()) });
            match (said.effect, &folder) {
                (
                    NrEffect::Open,
                    Some(Folder {
                        open: Some(open), ..
                    }),
                ) => {
                    open(window, cx);
                    workspace.update(cx, |workspace, cx| workspace.changed(window, cx));
                    return;
                }
                (NrEffect::ImportFiles, Some(folder)) => {
                    effects::import_files(folder.column, &said.text, workspace.clone(), window, cx);
                    return;
                }
                (NrEffect::Share, _) => {
                    let place = Bounds::new(at, size(px(1.0), px(1.0)));
                    effects::share(&effects::paths_of(&said.text), place, window);
                    return;
                }
                _ => {}
            }
            workspace.update(cx, |workspace, cx| {
                effects::carry_out(workspace, said, window, cx)
            });
        });
        self.menus.update(cx, |menus, cx| {
            if below {
                menus.open_below(menu, at, run, again, window, cx);
            } else {
                menus.open_context(menu, at, run, again, window, cx);
            }
        });
    }

    /// Window-space bounds of every open menu.
    pub fn menu_frames(&self, cx: &App) -> Vec<Bounds<Pixels>> {
        self.menus.read(cx).frames()
    }

    /// Shows the dialog the core describes (`nr_prompt`). No key opens one: drive keys act at
    /// once, and the trash asks nothing.
    pub fn dialog(&self, window: &mut Window, cx: &mut Context<Workspace>) {
        let workspace = cx.entity();
        let done: Done = Rc::new(move |effect, window, cx| {
            let said = Said::of(&effect);
            let changed = said.effect == NrEffect::Changed;
            workspace.update(cx, |workspace, cx| {
                effects::carry_out(workspace, said, window, cx);
                // An answer that changed what the window shows (an unlocked file) redraws it.
                if changed {
                    if let Some(opened) = &workspace.opened {
                        opened.screen.reload(window, cx);
                    }
                    workspace.changed(window, cx);
                }
            });
        });
        self.dialogs
            .update(cx, |dialogs, cx| dialogs.show(false, done, window, cx));
    }

    /// Shows a notice with `title` and `detail`.
    pub fn toast(&self, title: &str, detail: &str, window: &mut Window, cx: &mut App) {
        self.toasts
            .update(cx, |toasts, cx| toasts.show(title, detail, window, cx));
    }

    /// Opens the command palette over the window, or closes it.
    pub fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Workspace>) {
        if self.palette.take().is_some() {
            nr_palette_close();
            self._palette_events = None;
            self.give_back_keys(window, cx);
            return;
        }
        self.before_palette = window.focused(cx);
        let palette = cx.new(|cx| Palette::open(window, cx));
        self._palette_events = Some(cx.subscribe_in(&palette, window, Workspace::palette_event));
        self.palette = Some(palette);
    }

    /// Handles the palette closing, with or without a command to run.
    pub fn palette_closed(
        &mut self,
        event: &palette::Event,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Option<Said> {
        self.palette = None;
        self._palette_events = None;
        self.give_back_keys(window, cx);
        match event {
            palette::Event::Run(id) => {
                let id = c(id);
                // SAFETY: a NUL-terminated id.
                Some(Said::of(&unsafe { nr_palette_run(id.as_ptr()) }))
            }
            palette::Event::Closed => None,
        }
    }

    fn give_back_keys(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(keys) = self.before_palette.take() {
            window.focus(&keys, cx);
        }
    }

    /// Fills the check probe with open menus, the dialog title and, while a dialog is open, its
    /// focused control.
    pub fn probe(&self, probe: &mut crate::script::probe::Probe, window: &Window, cx: &App) {
        if let Some(keys) = self.dialogs.read(cx).keys(window) {
            probe.keys = keys.to_owned();
        }
        probe.menus = self.menus.read(cx).open_rows();
        probe.dialog = match &self.palette {
            Some(_) => "Command palette".to_owned(),
            None => self
                .dialogs
                .read(cx)
                .title()
                .map(str::to_owned)
                .unwrap_or_default(),
        };
    }
}
