//! The update flow as steps: when to check, what to show and when to install. The shell runs
//! each `Effect` and reports back. The controller holds the status, the update found, whether
//! it is downloaded, the version already offered this session, and the dialogs to show.
//!
//! A manual check (menu, tray, title bar button) reports every outcome; a background check stays
//! quiet unless there is an update. An update is offered once per session unless the check is
//! manual. Downloading needs consent. A downloaded update offers Restart and Install or Later,
//! with Later as default, and Later installs on quit.
//!
//! Dialog text comes from the catalogs (`strings`), localized when the shell receives the
//! dialog. A failure reason is either an app message or text from a step, kept verbatim.

use crate::strings::{key, Key, Message};
use std::collections::VecDeque;
use std::fmt;

/// Update state: `Available` is found but not downloaded, `Ready` is downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Checking,
    Available,
    Downloading,
    Ready,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::Checking => "checking",
            Status::Available => "available",
            Status::Downloading => "downloading",
            Status::Ready => "ready",
        }
    }

    /// Parses a status name, as a screenshot fixture asks for one.
    pub fn named(name: &str) -> Option<Status> {
        [
            Status::Idle,
            Status::Checking,
            Status::Available,
            Status::Downloading,
            Status::Ready,
        ]
        .into_iter()
        .find(|status| status.name() == name)
    }

    /// The Northroom menu item label for this status.
    pub fn menu_label(self) -> Key {
        match self {
            Status::Idle => key::UPDATE_MENU_CHECK,
            Status::Checking => key::UPDATE_MENU_CHECKING,
            Status::Available => key::UPDATE_MENU_DOWNLOAD,
            Status::Downloading => key::UPDATE_MENU_DOWNLOADING,
            Status::Ready => key::UPDATE_MENU_INSTALL,
        }
    }

    /// Whether the menu item is enabled.
    pub fn menu_enabled(self) -> bool {
        matches!(self, Status::Idle | Status::Available | Status::Ready)
    }

    /// The title bar update button's label, when it shows.
    pub fn button_label(self) -> Option<Key> {
        match self {
            Status::Available => Some(key::UPDATE_BUTTON_AVAILABLE),
            Status::Downloading => Some(key::UPDATE_BUTTON_DOWNLOADING),
            Status::Ready => Some(key::UPDATE_BUTTON_READY),
            Status::Idle | Status::Checking => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Manual,
    Background,
}

/// Dialog or failure text: an app message localized on demand, or a step's text kept verbatim.
/// `Display` gives English, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Words {
    Said(Message),
    Given(String),
}

impl Words {
    /// The text in `language`.
    pub fn say(&self, language: &str) -> String {
        match self {
            Words::Said(message) => message.say(language),
            Words::Given(text) => text.clone(),
        }
    }
}

impl fmt::Display for Words {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Words::Said(message) => message.fmt(formatter),
            Words::Given(text) => formatter.write_str(text),
        }
    }
}

impl From<Message> for Words {
    fn from(message: Message) -> Self {
        Words::Said(message)
    }
}

impl From<Key> for Words {
    fn from(key: Key) -> Self {
        Words::Said(Message::new(key))
    }
}

impl From<String> for Words {
    fn from(text: String) -> Self {
        Words::Given(text)
    }
}

impl From<&str> for Words {
    fn from(text: &str) -> Self {
        Words::Given(text.to_owned())
    }
}

/// A system message box: `info` shows the app icon, `error` the caution sign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    pub error: bool,
    pub message: Message,
    pub detail: Words,
    pub buttons: Vec<Key>,
    pub default_id: Option<usize>,
    pub cancel_id: Option<usize>,
}

impl Dialog {
    /// The default button for Return, when it is not the first (which a Mac dialog already
    /// treats as default).
    pub fn return_button(&self) -> Option<usize> {
        self.default_id.filter(|id| *id < self.buttons.len())
    }

    /// The button for Escape: the cancel button, unless it is the default or the only one.
    pub fn escape_button(&self) -> Option<usize> {
        self.cancel_id.filter(|id| {
            self.buttons.len() > 1 && *id < self.buttons.len() && Some(*id) != self.default_id
        })
    }
}

/// What the shell is asked to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// The status changed or was reset; the menu item and title bar follow it.
    Status(Status),
    /// Read the channel file and report it (`fetched`).
    Fetch,
    /// Download, verify and stage the update found (`downloaded`).
    Download,
    /// Show this dialog and report the button pressed (`answered`).
    Show(Dialog),
    /// Quit, install the downloaded update and relaunch.
    Relaunch,
    /// A failure for the log, in English.
    Log(String),
}

/// The update a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub available: bool,
    pub version: String,
}

/// What is being waited on while an update is presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waiting {
    Nothing,
    Check,
    DownloadChoice,
    Download,
    RestartChoice,
    /// An informational dialog: up to date, or a failure.
    Telling,
}

pub struct Controller {
    current_version: String,
    supported: bool,
    available: Option<Found>,
    downloaded: bool,
    manual_requested: bool,
    offered_version: Option<String>,
    status: Status,
    waiting: Waiting,
    effects: VecDeque<Effect>,
}

impl Controller {
    pub fn new(current_version: &str, supported: bool) -> Controller {
        Controller {
            current_version: current_version.to_owned(),
            supported,
            available: None,
            downloaded: false,
            manual_requested: false,
            offered_version: None,
            status: Status::Idle,
            waiting: Waiting::Nothing,
            effects: VecDeque::new(),
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn supported(&self) -> bool {
        self.supported
    }

    /// Whether a downloaded update waits to be installed when the app quits.
    pub fn downloaded(&self) -> bool {
        self.downloaded
    }

    /// The version found, when it is an update.
    pub fn available_version(&self) -> Option<&str> {
        self.available
            .as_ref()
            .filter(|found| found.available)
            .map(|found| found.version.as_str())
    }

    pub fn next_effect(&mut self) -> Option<Effect> {
        self.effects.pop_front()
    }

    fn set_status(&mut self, status: Status) {
        self.status = status;
        self.effects.push_back(Effect::Status(status));
    }

    /// The status between operations.
    fn settled(&self) -> Status {
        if self.downloaded {
            Status::Ready
        } else if self.available.as_ref().is_some_and(|found| found.available) {
            Status::Available
        } else {
            Status::Idle
        }
    }

    /// Starts a check that presents what it finds. One runs at a time; a request during a check
    /// joins it, and a manual request makes it report every outcome.
    pub fn request(&mut self, origin: Origin) {
        self.manual_requested |= origin == Origin::Manual;
        if self.waiting != Waiting::Nothing {
            return;
        }
        self.waiting = Waiting::Check;
        if !self.supported {
            self.fail(key::UPDATE_UNSUPPORTED.into());
            return;
        }
        if self.downloaded && self.available.is_some() {
            self.present();
            return;
        }
        // A found update stays available through a re-check, and through one that fails.
        if !self.available.as_ref().is_some_and(|found| found.available) {
            self.set_status(Status::Checking);
        }
        self.effects.push_back(Effect::Fetch);
    }

    /// Handles the channel file result, or why it could not be read.
    pub fn fetched(&mut self, result: Result<Found, Words>) {
        if self.waiting != Waiting::Check {
            return;
        }
        match result {
            Ok(found) => {
                self.available = Some(found);
                let settled = self.settled();
                self.set_status(settled);
                self.present();
            }
            Err(reason) => {
                let settled = self.settled();
                self.set_status(settled);
                self.fail(reason);
            }
        }
    }

    /// Presents what the check found.
    fn present(&mut self) {
        let Some(found) = self.available.clone() else {
            return self.finish();
        };
        if !found.available {
            if self.manual_requested {
                let detail = Message::new(key::UPDATE_UP_TO_DATE_DETAIL)
                    .with("version", &self.current_version);
                self.tell(false, key::UPDATE_UP_TO_DATE, detail.into());
            } else {
                self.finish();
            }
            return;
        }
        if !self.manual_requested && self.offered_version.as_deref() == Some(found.version.as_str())
        {
            return self.finish();
        }
        self.offered_version = Some(found.version.clone());
        if !self.downloaded {
            self.waiting = Waiting::DownloadChoice;
            self.effects.push_back(Effect::Show(Dialog {
                error: false,
                message: Message::new(key::UPDATE_AVAILABLE).with("version", &found.version),
                detail: key::UPDATE_AVAILABLE_DETAIL.into(),
                buttons: vec![key::UPDATE_DOWNLOAD, key::UPDATE_LATER],
                default_id: Some(0),
                cancel_id: Some(1),
            }));
            return;
        }
        self.offer_restart();
    }

    fn offer_restart(&mut self) {
        self.waiting = Waiting::RestartChoice;
        self.effects.push_back(Effect::Show(Dialog {
            error: false,
            message: Message::new(key::UPDATE_READY),
            detail: key::UPDATE_READY_DETAIL.into(),
            buttons: vec![key::UPDATE_RESTART, key::UPDATE_LATER],
            default_id: Some(1),
            cancel_id: Some(1),
        }));
    }

    /// Handles a button press in the shown dialog.
    pub fn answered(&mut self, response: usize) {
        match self.waiting {
            Waiting::DownloadChoice if response == 0 => {
                self.waiting = Waiting::Download;
                self.set_status(Status::Downloading);
                self.effects.push_back(Effect::Download);
            }
            Waiting::RestartChoice if response == 0 => {
                self.effects.push_back(Effect::Relaunch);
                self.finish();
            }
            Waiting::DownloadChoice | Waiting::RestartChoice | Waiting::Telling => self.finish(),
            Waiting::Nothing | Waiting::Check | Waiting::Download => {}
        }
    }

    /// Handles the end of the download: verified and ready, or why not.
    pub fn downloaded_result(&mut self, result: Result<(), Words>) {
        if self.waiting != Waiting::Download {
            return;
        }
        match result {
            Ok(()) => {
                self.downloaded = true;
                let settled = self.settled();
                self.set_status(settled);
                self.offer_restart();
            }
            Err(reason) => {
                let settled = self.settled();
                self.set_status(settled);
                self.fail(reason);
            }
        }
    }

    /// Handles a failed check: the version may be offered again, the failure is logged, and
    /// only a manual check shows it.
    fn fail(&mut self, reason: Words) {
        self.offered_version = None;
        self.effects.push_back(Effect::Log(reason.to_string()));
        if self.manual_requested {
            self.tell(true, key::UPDATE_FAILED, reason);
        } else {
            self.finish();
        }
    }

    /// Restart and Install failed; shows a dialog with the reason.
    pub fn install_failed(&mut self, reason: Words) {
        self.tell(true, key::UPDATE_FAILED, reason);
    }

    fn tell(&mut self, error: bool, message: Key, detail: Words) {
        self.waiting = Waiting::Telling;
        self.effects.push_back(Effect::Show(Dialog {
            error,
            message: Message::new(message),
            detail,
            buttons: vec![key::UPDATE_OK],
            default_id: None,
            cancel_id: None,
        }));
    }

    fn finish(&mut self) {
        self.waiting = Waiting::Nothing;
        self.manual_requested = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings;

    fn dialog(buttons: Vec<Key>, default_id: Option<usize>, cancel_id: Option<usize>) -> Dialog {
        Dialog {
            error: false,
            message: Message::new(key::UPDATE_OK),
            detail: "".into(),
            buttons,
            default_id,
            cancel_id,
        }
    }

    #[test]
    fn return_and_escape_go_to_the_default_and_the_cancel_button() {
        let download = dialog(
            vec![key::UPDATE_DOWNLOAD, key::UPDATE_LATER],
            Some(0),
            Some(1),
        );
        assert_eq!(
            (download.return_button(), download.escape_button()),
            (Some(0), Some(1))
        );
        // Later is the default for Return, and Escape is unbound.
        let restart = dialog(
            vec![key::UPDATE_RESTART, key::UPDATE_LATER],
            Some(1),
            Some(1),
        );
        assert_eq!(
            (restart.return_button(), restart.escape_button()),
            (Some(1), None)
        );
        let ok = dialog(vec![key::UPDATE_OK], None, None);
        assert_eq!((ok.return_button(), ok.escape_button()), (None, None));
        let only = dialog(vec![key::UPDATE_OK], None, Some(0));
        assert_eq!(
            only.escape_button(),
            None,
            "a single button takes no Escape"
        );
        let outside = dialog(vec![key::UPDATE_OK, key::UPDATE_LATER], Some(5), Some(7));
        assert_eq!(
            (outside.return_button(), outside.escape_button()),
            (None, None)
        );
    }

    #[test]
    fn the_menu_and_the_button_say_each_status_in_the_language_asked_for() {
        let menu: Vec<&str> = [
            Status::Idle,
            Status::Checking,
            Status::Available,
            Status::Downloading,
            Status::Ready,
        ]
        .map(|status| strings::text("en", status.menu_label()))
        .to_vec();
        assert_eq!(
            menu,
            [
                "Check for Updates…",
                "Checking for Updates…",
                "Download Update…",
                "Downloading Update…",
                "Install Downloaded Update…"
            ]
        );
        assert_eq!(
            Status::Ready
                .button_label()
                .map(|label| strings::text("en", label)),
            Some("Restart to update")
        );
    }

    #[test]
    fn a_reason_is_said_in_the_language_asked_for_or_kept_as_given() {
        let said: Words = Message::new(key::UPDATE_NOT_UNPACKED)
            .with("exit", "1")
            .into();
        assert_eq!(
            said.to_string(),
            "The update could not be unpacked (ditto exited with 1)."
        );
        assert_eq!(said.say("en"), said.to_string());
        let given: Words = "HttpError: 500".into();
        assert_eq!(
            (given.to_string(), given.say("en")),
            ("HttpError: 500".to_owned(), "HttpError: 500".to_owned())
        );
    }
}
