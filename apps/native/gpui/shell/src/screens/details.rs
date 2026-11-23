//! A file with no dedicated view, shown the way Finder's Quick Look shows it. The core decides
//! what shows and where (`nr_details_load`, `details_layout`). A picture, video or sound shows
//! alone, as large as the pane allows, beside its column or filling the tab (on black,
//! `NrWindowLayout::details_fills`). Any other file shows its system thumbnail or kind glyph
//! above its name, kind and size.
//!
//! Video and sound play in the core's preview. Its player (`preview`) sits where the layout puts
//! the media (`Layout::media`), laid out again at the video's own size once it opens.

use super::preview::Player;
use super::{Ask, Screen};
use crate::script::probe::Probe;
use gpui::{
    App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, Styled, Window, canvas, div, point,
};
use kit::controls::placed;
use kit::controls::{Drawn, Frame};
use kit::core::{text, zeroed};
use kit::icons::{self, Glyph};
use kit::lists::thumbs;
use kit::nr::*;
use kit::theme::{Token, color};
use northroom_core::preview::details::{Layout, Media};

pub struct Details {
    focus: FocusHandle,
    frame: Bounds<Pixels>,
    /// The file shown (`nr_details_load`), its glyph, and what the pane shows of it.
    id: String,
    glyph: Option<Glyph>,
    shows: NrDetailsShows,
    /// The file plays in the core's preview, with the player at the media's place.
    pub plays: bool,
    picture_pixels: u32,
    /// Opened to fill the tab.
    fills: bool,
    /// The playing video or sound, at the media's place.
    player: Option<Entity<Player>>,
}

impl Details {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            frame: Bounds::default(),
            id: String::new(),
            glyph: None,
            shows: NrDetailsShows::Glyph,
            plays: false,
            picture_pixels: 0,
            fills: false,
            player: None,
        }
    }

    /// The decoded picture, once ready.
    fn picture(&self) -> Option<std::sync::Arc<gpui::RenderImage>> {
        match self.shows {
            NrDetailsShows::Picture => thumbs::picture(&self.id, self.picture_pixels, false),
            NrDetailsShows::SystemPicture => thumbs::system_picture(&self.id, self.picture_pixels),
            NrDetailsShows::Glyph | NrDetailsShows::Video | NrDetailsShows::Sound => None,
        }
    }

    /// The pane laid out for its size, with the picture or video at its known size.
    fn laid(
        &self,
        picture: Option<&std::sync::Arc<gpui::RenderImage>>,
        cx: &App,
    ) -> Option<Layout> {
        let video = self
            .player
            .as_ref()
            .and_then(|player| player.read(cx).video_size());
        let media = match (self.shows, picture) {
            (NrDetailsShows::Video, _) => video.map_or(Media::Unknown, |(w, h)| Media::Sized(w, h)),
            (_, Some(image)) => {
                let size = image.size(0);
                Media::Sized(f64::from(size.width.0), f64::from(size.height.0))
            }
            (NrDetailsShows::Picture, None)
                if thumbs::picture_unreadable(&self.id, self.picture_pixels) =>
            {
                Media::Unreadable
            }
            _ => Media::Unknown,
        };
        details_layout(
            f64::from(self.frame.size.width),
            f64::from(self.frame.size.height),
            media,
            self.fills,
        )
    }

    fn paint(&self, window: &mut Window, cx: &mut App) {
        let picture = self.picture();
        let Some(laid) = self.laid(picture.as_ref(), cx) else {
            return;
        };
        let origin = self.frame.origin;
        if laid.fills {
            // Paint around the video so it shows through the hole the player cut.
            kit::hosted::paint_around(window, self.frame, gpui::black());
        }
        if !self.plays
            && let (Some(image), Some(media)) = (picture, laid.media)
        {
            let at = media.at(origin);
            let _ = window.paint_image(at, at, Default::default(), image, 0, false);
        }
        if let (Some(place), Some(glyph)) = (laid.glyph, self.glyph) {
            icons::draw(
                window,
                cx,
                glyph,
                place.at(origin),
                color(Token::Foreground),
            );
        }
        for line in &laid.words {
            line.draw(window, cx, point(origin.x, origin.y), None);
        }
    }
}

impl EventEmitter<Ask> for Details {}

impl Screen for Details {
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut loaded: NrDetails = zeroed();
        // SAFETY: storage for one `NrDetails`.
        if !unsafe { nr_details_load(&mut loaded) } {
            return;
        }
        let id = text(&loaded.id);
        if id != self.id {
            // A different file: stop the current one before the next opens.
            self.player = None;
            if loaded.plays {
                let (path, video, bytes) = (
                    text(&loaded.path),
                    loaded.shows == NrDetailsShows::Video,
                    loaded.file_bytes,
                );
                self.player = Some(cx.new(|cx| Player::open(&path, video, bytes, window, cx)));
                // Opened from its card's play button, so it plays at once.
                if nr_opened_takes_play() {
                    nr_preview_play();
                }
            }
        }
        self.id = id;
        self.glyph = icons::named(&text(&loaded.glyph));
        self.shows = loaded.shows;
        self.plays = loaded.plays;
        self.picture_pixels = loaded.picture_pixels;
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, laid: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame || self.fills != laid.details_fills {
            self.frame = frame;
            self.fills = laid.details_fills;
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, _: &App) {
        if self.focus.is_focused(window) {
            probe.keys = "DetailsView".to_owned();
        }
    }
}

impl Render for Details {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let details = cx.entity();
        let media = self
            .laid(self.picture().as_ref(), cx)
            .and_then(|laid| laid.media)
            .map(|media| media.at(self.frame.origin));
        if let (Some(player), Some(media)) = (&self.player, media) {
            player.update(cx, |player, cx| player.place(media, cx));
        }
        // The player draws in window points.
        let player = self.player.clone().map(|player| {
            div()
                .absolute()
                .left(-self.frame.origin.x)
                .top(-self.frame.origin.y)
                .child(player)
        });
        placed("details", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        details.update(cx, |details, cx| details.paint(window, cx))
                    },
                )
                .size_full(),
            )
            .children(player)
    }
}
