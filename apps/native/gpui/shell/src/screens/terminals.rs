//! Several terminals side by side in one tab, all live (ADR-0005). Open does this for a
//! selection of terminal files. The core lays them out (`nr_terminal_tiles_load`). Each tile is
//! its own terminal screen showing its file's session (`terminal::Terminal::tile`), so typing
//! goes to the clicked tile and the others keep running.

use super::terminal::Terminal;
use super::{Ask, Screen};
use crate::script::probe::Probe;
use gpui::{
    App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, IntoElement,
    ParentElement, Pixels, Render, Styled, Window, canvas, point, px, size,
};
use kit::core::{rect, text};
use kit::draw::fill;
use kit::nr::*;
use kit::theme::{Token, color};

pub struct Terminals {
    /// Where the window placed the terminals.
    frame: Bounds<Pixels>,
    /// Each tile's terminal file and its screen, in folder order.
    tiles: Vec<(String, Entity<Terminal>)>,
    rules: Vec<Bounds<Pixels>>,
}

impl Terminals {
    pub fn new(_: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            frame: Bounds::default(),
            tiles: Vec::new(),
            rules: Vec::new(),
        }
    }

    /// Lays the terminals out again, keeping tiles that remain, adding new ones and dropping the
    /// rest. The first tile gets the keys once the tiles exist.
    fn lay_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rules.clear();
        let frame = self.frame;
        let count = nr_terminal_tiles_load(
            f32::from(frame.origin.x).into(),
            f32::from(frame.origin.y).into(),
            f32::from(frame.size.width).into(),
            f32::from(frame.size.height).into(),
        );
        let afresh = self.tiles.is_empty();
        let mut before = std::mem::take(&mut self.tiles);
        for index in 0..count {
            let tile = nr_terminal_tile(index);
            let id = text(&tile.id);
            let grid = rect(&tile.grid);
            let screen = match before.iter().position(|(shown, _)| *shown == id) {
                Some(at) => before.remove(at).1,
                None => cx.new(|cx| Terminal::tile(&id, window, cx)),
            };
            screen.update(cx, |screen, cx| screen.set_frame(grid, cx));
            self.tiles.push((id, screen));
        }
        self.rules = (0..nr_terminal_tile_rules())
            .map(|index| rect(&nr_terminal_tile_rule(index)))
            .collect();
        if afresh && let Some((_, first)) = self.tiles.first() {
            let keys = first.read(cx).focus_handle().clone();
            window.focus(&keys, cx);
        }
    }
}

impl EventEmitter<Ask> for Terminals {}

impl Screen for Terminals {
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    fn keys(&self, cx: &App) -> Option<FocusHandle> {
        self.tiles
            .first()
            .map(|(_, tile)| tile.read(cx).focus_handle().clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, cx: &App) {
        if self
            .tiles
            .iter()
            .any(|(_, tile)| tile.read(cx).focus_handle().is_focused(window))
        {
            probe.keys = "TerminalView".to_owned();
        }
    }
}

impl Render for Terminals {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.lay_out(window, cx);
        let frame = self.frame;
        // From the window origin, so each tile's window frame places it.
        let reach = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(
                frame.origin.x + frame.size.width,
                frame.origin.y + frame.size.height,
            ),
        };
        let screen = cx.entity();
        kit::controls::placed("terminals", cx.entity_id(), reach)
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, cx| {
                        let screen = screen.read(cx);
                        for rule in screen.rules.clone() {
                            fill(window, rule, color(Token::Border));
                        }
                    },
                )
                .size_full(),
            )
            .children(self.tiles.iter().map(|(_, tile)| tile.clone()))
    }
}
