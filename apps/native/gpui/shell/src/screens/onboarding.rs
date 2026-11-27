//! The welcome screen, shown over everything while no folder is chosen. Its one button opens the
//! platform folder picker, and the folder picked becomes the drive (`nr_selection_choose`). The
//! core lays it out (`onboarding_layout`). Once a folder is chosen the window shows the drive.

use super::{Ask, Screen};
use gpui::{
    Bounds, Context, CursorStyle, DispatchPhase, EventEmitter, Hitbox, HitboxBehavior, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, PathPromptOptions, Pixels, Render, Styled, Window,
    canvas,
};
use kit::controls::{Drawn, Frame};
use kit::core::{c, contains, text};
use kit::draw::{fill, rounded};
use kit::nr::*;
use kit::theme::{CONTROL_RADIUS, Token, color};
use northroom_core::window::screen_layout::Onboarding as Layout;

pub struct Onboarding {
    frame: Bounds<Pixels>,
    /// Why the last picked folder was refused, shown under the button; empty if none.
    refusal: String,
    /// Where the button was last drawn, in window points.
    choose: Bounds<Pixels>,
}

impl Onboarding {
    pub fn new(_: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            frame: Bounds::default(),
            refusal: String::new(),
            choose: Bounds::default(),
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if event.button == MouseButton::Left && contains(&self.choose, event.position) {
            self.choose(cx);
        }
    }

    /// Opens the platform folder picker; the chosen folder becomes the drive.
    fn choose(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(text(&nr_folder_picker_button()).into()),
        });
        cx.spawn(async move |onboarding, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(folder) = paths.first() else {
                return;
            };
            let support = c(&text(&nr_path(NrPath::Support)));
            let folder = c(&folder.to_string_lossy());
            // SAFETY: NUL-terminated paths.
            let refusal = text(&unsafe { nr_selection_choose(support.as_ptr(), folder.as_ptr()) });
            let _ = onboarding.update(cx, |onboarding, cx| onboarding.answered(refusal, cx));
        })
        .detach();
    }

    /// A folder became the drive (`refusal` empty), or the reason it was refused.
    fn answered(&mut self, refusal: String, cx: &mut Context<Self>) {
        let chosen = refusal.is_empty();
        self.refusal = refusal;
        cx.notify();
        if chosen {
            cx.emit(Ask::Changed);
        }
    }

    /// Lays the screen out at its current place. The button gets the pointing-hand cursor.
    fn lay_out(&mut self, window: &mut Window) -> (Layout, Hitbox) {
        let frame = self.frame;
        let placed = onboarding_layout(
            f64::from(frame.size.width),
            f64::from(frame.size.height),
            &self.refusal,
        );
        self.choose = placed.choose.at(frame.origin);
        let button = window.insert_hitbox(self.choose, HitboxBehavior::Normal);
        (placed, button)
    }

    fn paint(
        &self,
        placed: &Layout,
        button: &Hitbox,
        over: &Hitbox,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origin = self.frame.origin;
        fill(window, self.frame, color(Token::Background));
        placed.description.draw(window, cx, origin, None);
        rounded(window, self.choose, color(Token::Cta), CONTROL_RADIUS);
        placed.choose_label.draw(window, cx, origin, None);
        if let Some(note) = &placed.note {
            note.draw(window, cx, origin, None);
        }
        window.set_cursor_style(CursorStyle::PointingHand, button);
        let (onboarding, over) = (cx.entity(), over.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && over.is_hovered(window) {
                onboarding.update(cx, |onboarding, cx| onboarding.mouse_down(event, cx));
            }
        });
    }
}

impl EventEmitter<Ask> for Onboarding {}

impl Screen for Onboarding {
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }
}

impl Render for Onboarding {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (laying, painting) = (cx.entity(), cx.entity());
        kit::controls::placed("onboarding", cx.entity_id(), self.frame).child(
            canvas(
                move |bounds, window, cx| {
                    let over = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                    let (placed, button) =
                        laying.update(cx, |onboarding, _| onboarding.lay_out(window));
                    (over, placed, button)
                },
                move |_, (over, placed, button), window, cx| {
                    painting.update(cx, |onboarding, cx| {
                        onboarding.paint(&placed, &button, &over, window, cx)
                    })
                },
            )
            .size_full(),
        )
    }
}
