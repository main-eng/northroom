//! The welcome screen, shown over everything while no folder is chosen. Its one button opens the
//! platform folder picker, and the folder picked becomes the drive (`nr_selection_choose`). The
//! core lays it out (`onboarding_layout`). Once a folder is chosen the window shows the drive.
//! It fills the window, title bar included, and its top strip moves the window as the bar does.

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
use kit::theme::{Token, color};
use northroom_core::window::screen_layout::Onboarding as Layout;

pub struct Onboarding {
    frame: Bounds<Pixels>,
    /// Why the last picked folder was refused, shown under the button; empty if none.
    refusal: String,
    /// Where the button was last drawn, in window points.
    choose: Bounds<Pixels>,
    /// How tall the strip at the top that moves the window is: the title bar's height.
    strip: Pixels,
}

impl Onboarding {
    pub fn new(_: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            frame: Bounds::default(),
            refusal: String::new(),
            choose: Bounds::default(),
            strip: Pixels::ZERO,
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left {
            return;
        }
        if contains(&self.choose, event.position) {
            self.choose(cx);
        } else if event.position.y < self.frame.origin.y + self.strip {
            // As on the title bar: a double click does what the system setting says.
            if event.click_count == 2 {
                window.titlebar_double_click();
            } else {
                window.start_window_move();
            }
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
        placed.title.draw(window, cx, origin, None);
        placed.description.draw(window, cx, origin, None);
        // A pill, as the website's primary button is.
        let radius = f32::from(self.choose.size.height) / 2.0;
        rounded(window, self.choose, color(Token::Cta), radius);
        placed.choose_label.draw(window, cx, origin, None);
        if let Some(note) = &placed.note {
            note.draw(window, cx, origin, None);
        }
        window.set_cursor_style(CursorStyle::PointingHand, button);
        let (onboarding, over) = (cx.entity(), over.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && over.is_hovered(window) {
                onboarding.update(cx, |onboarding, cx| {
                    onboarding.mouse_down(event, window, cx)
                });
            }
        });
    }
}

impl EventEmitter<Ask> for Onboarding {}

impl Screen for Onboarding {
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, laid: &NrWindowLayout, cx: &mut Context<Self>) {
        let strip = gpui::px(laid.title_bar.height as f32);
        if self.frame != frame || self.strip != strip {
            self.frame = frame;
            self.strip = strip;
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
