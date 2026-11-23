//! The protocol of the model still page (`model-still.html`), shared by every platform. The page
//! reports ready once three.js loads, then answers each model with its turn and either a PNG or an
//! error. Each platform's `model_still::StillDrawer` feeds it the models.

use crate::core::{self, HostError};
use crate::run::{Once, sleep, spawn};
use northroom_core::page::host::ffi::{
    NR_HOST_STILL_STOPPED, NR_HOST_STILL_TIMEOUT, NR_HOST_STILL_UNREADABLE,
};
use std::rc::Rc;

/// A message the drawing page posts.
#[derive(serde::Deserialize)]
struct Posted {
    #[serde(default)]
    ready: bool,
    turn: Option<u64>,
    error: Option<String>,
    png: Option<String>,
}

pub enum Said {
    Ready,
    /// The page failed to start, with its reason.
    NotStarted(String),
    /// The result for the model of this turn.
    Drawn(u64, Result<Vec<u8>, HostError>),
}

pub fn read_posted(body: &str) -> Option<Said> {
    let posted: Posted = serde_json::from_str(body).ok()?;
    if posted.ready {
        return Some(Said::Ready);
    }
    let Some(turn) = posted.turn else {
        return Some(Said::NotStarted(posted.error.unwrap_or_default()));
    };
    if let Some(error) = posted.error {
        return Some(Said::Drawn(turn, Err(HostError::Host(error))));
    }
    let png = posted
        .png
        .and_then(|address| address.split_once(',').map(|(_, data)| data.to_owned()))
        .and_then(|data| crate::base64::decode(&data))
        .ok_or_else(|| HostError::of(NR_HOST_STILL_UNREADABLE, ""));
    Some(Said::Drawn(turn, png))
}

pub type Drawn = Rc<Once<Result<Vec<u8>, HostError>>>;

/// Pending waits on the page: for it to start, and for the still being drawn.
#[derive(Default)]
pub struct Waiting {
    pub opening: Option<Rc<Once<Result<(), HostError>>>>,
    pub drawing: Option<(u64, Drawn)>,
    /// The web content process ended, so nothing more will be drawn.
    pub ended: bool,
}

impl Waiting {
    pub fn heard(&mut self, said: Said) {
        match said {
            Said::Ready => self.opened(Ok(())),
            Said::NotStarted(why) => self.opened(Err(HostError::Host(why))),
            Said::Drawn(turn, result) => match self.drawing.take() {
                Some((waited, drawn)) if waited == turn => drawn.resolve(result),
                other => self.drawing = other,
            },
        }
    }

    fn opened(&mut self, result: Result<(), HostError>) {
        if let Some(opening) = self.opening.take() {
            opening.resolve(result);
        }
    }

    pub fn ended(&mut self) {
        self.ended = true;
        let stopped = || HostError::of(NR_HOST_STILL_STOPPED, "");
        self.opened(Err(stopped()));
        if let Some((_, drawn)) = self.drawing.take() {
            drawn.resolve(Err(stopped()));
        }
    }
}

/// Fails `waiter` if the page does not answer within the host's start timeout.
pub fn time_out<T: 'static>(waiter: Rc<Once<Result<T, HostError>>>) {
    spawn(async move {
        sleep(core::start_timeout_seconds()).await;
        waiter.resolve(Err(HostError::of(NR_HOST_STILL_TIMEOUT, "")));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::promise;

    /// Starts waiting on the still for `turn`.
    fn drawing(waiting: &mut Waiting, turn: u64) {
        let (resolver, _) = promise();
        waiting.drawing = Some((turn, Rc::new(Once::new(resolver))));
    }

    #[test]
    fn the_page_says_it_is_ready_or_why_it_could_not_start() {
        assert!(matches!(
            read_posted(r#"{"ready":true}"#),
            Some(Said::Ready)
        ));
        assert!(matches!(
            read_posted(r#"{"error":"three.js did not load"}"#),
            Some(Said::NotStarted(why)) if why == "three.js did not load"
        ));
        assert!(read_posted("not json").is_none());
    }

    #[test]
    fn a_still_is_its_png_or_why_there_is_none_under_its_turn() {
        let Some(Said::Drawn(3, Ok(png))) =
            read_posted(r#"{"turn":3,"png":"data:image/png;base64,iVBORw=="}"#)
        else {
            panic!("a drawn still")
        };
        assert_eq!(png, [0x89, b'P', b'N', b'G']);
        assert!(matches!(
            read_posted(r#"{"turn":4,"error":"the model has no vertices"}"#),
            Some(Said::Drawn(4, Err(HostError::Host(why)))) if why == "the model has no vertices"
        ));
        assert!(matches!(
            read_posted(r#"{"turn":5,"png":"no comma"}"#),
            Some(Said::Drawn(5, Err(_)))
        ));
    }

    #[test]
    fn an_answer_for_another_turn_leaves_the_still_being_drawn_waiting() {
        let mut waiting = Waiting::default();
        drawing(&mut waiting, 2);
        waiting.heard(Said::Drawn(1, Ok(vec![1])));
        assert_eq!(waiting.drawing.as_ref().map(|(turn, _)| *turn), Some(2));
        waiting.heard(Said::Drawn(2, Ok(vec![2])));
        assert!(waiting.drawing.is_none());
    }

    #[test]
    fn a_page_that_ends_ends_the_drawing_and_every_later_one() {
        let mut waiting = Waiting::default();
        drawing(&mut waiting, 1);
        waiting.ended();
        assert!(waiting.ended && waiting.drawing.is_none());
    }
}
