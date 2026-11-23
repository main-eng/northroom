//! A 3D model (`.glb`). Its still renders on studio grey where the core puts it
//! (`nr_model_view`), centred on the panel colour. While the still renders, or if it cannot, a
//! cube glyph shows in the muted colour. The still renders once per file version, is cached on
//! disk (`model_still`) and is decoded like any picture, off the UI thread.

use super::{Ask, Screen};
use crate::script::probe::Probe;
use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, Styled, Window, canvas,
};
use kit::controls::placed;
use kit::core::{rect, text};
use kit::draw::fill;
use kit::icons::Glyph;
use kit::lists::thumbs;
use kit::nr::*;
use kit::theme::{Token, color, named};
use northroom_core::model_still;
use std::path::Path;

/// The largest side the still is decoded at, which is its full size.
const STILL_PIXELS: u32 = model_still::SIDE as u32;

pub struct Model {
    frame: Bounds<Pixels>,
    focus: FocusHandle,
    path: String,
}

impl Model {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut model = Self {
            frame: Bounds::default(),
            focus: cx.focus_handle(),
            path: String::new(),
        };
        model.reload(window, cx);
        model
    }

    fn paint(&self, window: &mut Window, cx: &App) {
        fill(window, self.frame, color(Token::Panel));
        // Where the core puts the still, or the glyph while it renders (`nr_model_view`).
        let view = nr_model_view(
            f64::from(self.frame.size.width),
            f64::from(self.frame.size.height),
        );
        let at = |placed: &NrRect| {
            kit::core::offset(
                rect(placed),
                f32::from(self.frame.origin.x),
                f32::from(self.frame.origin.y),
            )
        };
        let still = model_still::picture(Path::new(&self.path))
            .and_then(|file| thumbs::picture_file(&file.to_string_lossy(), STILL_PIXELS));
        let Some(still) = still else {
            kit::icons::draw(
                window,
                cx,
                Glyph::Cube,
                at(&view.glyph),
                color(Token::MutedForeground),
            );
            return;
        };
        if !view.shows {
            return;
        }
        let shown = at(&view.still);
        fill(window, shown, named(&text(&view.backdrop)));
        let _ = window.paint_image(shown, shown, gpui::Corners::default(), still, 0, false);
    }
}

impl EventEmitter<Ask> for Model {}

impl Screen for Model {
    /// Reads which model is open from the core.
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let now = text(&nr_opened_model_path());
        if now != self.path {
            self.path = now;
            cx.notify();
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
            probe.keys = "ModelFileView".to_owned();
        }
    }
}

impl Render for Model {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = cx.entity();
        placed("model", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| model.read(cx).paint(window, cx),
                )
                .size_full(),
            )
    }
}
