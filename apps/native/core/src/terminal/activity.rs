//! What a terminal's session is doing, for its card and row (ADR-0005): starting, working,
//! needing the person, finished, idle or ended. A program reports this through the title it
//! sets and the notifications it sends or, when Northroom's hooks are installed, through a
//! private escape sequence.

use crate::strings::{self, key};
use crate::theme::Token;

/// Each discriminant is the activity's FFI number (`number`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Activity {
    Starting = 2,
    Working = 3,
    NeedsYou = 4,
    Finished = 5,
    Idle = 1,
    Ended = 6,
}

impl Activity {
    /// Every activity, in number order.
    const ALL: [Self; 6] = [
        Self::Idle,
        Self::Starting,
        Self::Working,
        Self::NeedsYou,
        Self::Finished,
        Self::Ended,
    ];

    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "starting" => Self::Starting,
            "working" => Self::Working,
            "needs-you" => Self::NeedsYou,
            "finished" => Self::Finished,
            "idle" => Self::Idle,
            "ended" => Self::Ended,
            _ => return None,
        })
    }

    /// The activity's FFI number: 1 idle, 2 starting, 3 working, 4 needs you, 5 finished, 6
    /// ended. 0 means no session is running.
    pub fn number(self) -> u32 {
        self as u32
    }

    /// The activity with `number`; `None` for 0 (no session) or any unknown number.
    pub fn numbered(number: u32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|activity| activity.number() == number)
    }

    /// Whether files dropped on the terminal are passed to its program as references, which needs
    /// a running program that reads pastes. A starting or ended program reads nothing.
    pub fn takes_references(self) -> bool {
        !matches!(self, Self::Starting | Self::Ended)
    }

    /// The word under a card's name, in `language`.
    pub fn label(self, language: &str) -> &'static str {
        let word = match self {
            Self::Starting => key::TERMINAL_ACTIVITY_STARTING,
            Self::Working => key::TERMINAL_ACTIVITY_WORKING,
            Self::NeedsYou => key::TERMINAL_ACTIVITY_NEEDS_YOU,
            Self::Finished => key::TERMINAL_ACTIVITY_FINISHED,
            Self::Idle => key::TERMINAL_ACTIVITY_IDLE,
            Self::Ended => key::TERMINAL_ACTIVITY_ENDED,
        };
        strings::text(language, word)
    }
}

/// The word under a terminal card's name for `activity`, or for no running session, in
/// `language`.
pub fn label_of(activity: Option<Activity>, language: &str) -> &'static str {
    activity.map_or_else(
        || strings::text(language, key::TERMINAL_NOT_RUNNING),
        |activity| activity.label(language),
    )
}

/// How a terminal's activity looks: the tile's ring, the corner dot and the word under a card's
/// name, as theme colour tokens. States worth returning to (working, needs you, finished) use
/// their status colour; the rest stay as muted as the card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The ring's colour, drawn at `ring_opacity`.
    pub ring: Token,
    pub ring_opacity: f64,
    /// The dot's colour, with its opacity multiplied by `dot_fade`; `None` for no dot.
    pub dot: Option<Token>,
    pub dot_fade: f64,
    /// The shape the dot takes.
    pub sign: Sign,
    pub label: Token,
}

/// The shape of a session's dot, so each state reads by shape as well as colour. Only states
/// that want attention animate: working spins and needs-you hops. Finished shows a still check
/// and idle a still empty ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sign {
    /// A plain dot.
    #[default]
    Dot,
    /// A ring with a fading tail, turning.
    Comet,
    /// A dot that hops up and back.
    Nudge,
    /// A check mark.
    Check,
    /// An empty ring.
    Ring,
}

impl Sign {
    /// Its FFI name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Dot => "dot",
            Self::Comet => "comet",
            Self::Nudge => "nudge",
            Self::Check => "check",
            Self::Ring => "ring",
        }
    }

    /// The sign named `name`; a plain dot for any other.
    pub fn named(name: &str) -> Self {
        [Self::Comet, Self::Nudge, Self::Check, Self::Ring]
            .into_iter()
            .find(|sign| sign.name() == name)
            .unwrap_or(Self::Dot)
    }

    /// Scale factor applied around the dot's centre, so a ring or check reads as clearly as a solid
    /// dot. Any ring around it scales too.
    pub fn growth(self) -> f64 {
        match self {
            Self::Comet | Self::Check => 1.5,
            Self::Ring => 1.2,
            Self::Dot | Self::Nudge => 1.0,
        }
    }
}

/// The look of a terminal doing `activity`, or with no session running.
pub fn look(activity: Option<Activity>) -> Look {
    let quiet = |ring_opacity: f64, dot: Option<Token>, dot_fade: f64, sign: Sign| Look {
        ring: Token::Foreground,
        ring_opacity,
        dot,
        dot_fade,
        sign,
        label: Token::MutedForeground,
    };
    let status = |colour: Token, ring_opacity: f64, label: Token, sign: Sign| Look {
        ring: colour,
        ring_opacity,
        dot: Some(colour),
        dot_fade: 1.0,
        sign,
        label,
    };
    match activity {
        None => quiet(0.10, None, 1.0, Sign::Dot),
        Some(Activity::Idle) => quiet(0.10, Some(Token::MutedForeground), 1.0, Sign::Ring),
        Some(Activity::Starting) => quiet(0.15, Some(Token::MutedForeground), 1.0, Sign::Dot),
        Some(Activity::Ended) => quiet(0.10, Some(Token::MutedForeground), 0.6, Sign::Dot),
        Some(Activity::Working) => status(Token::StatusInfo, 0.5, Token::Foreground, Sign::Comet),
        Some(Activity::NeedsYou) => {
            status(Token::StatusWarning, 0.7, Token::StatusWarning, Sign::Nudge)
        }
        Some(Activity::Finished) => {
            status(Token::StatusSuccess, 0.6, Token::StatusSuccess, Sign::Check)
        }
    }
}

/// What a title says the program is doing, or `None` if nothing. A leading braille spinner frame
/// is the default in Codex and many other programs; the other marks are Gemini CLI's.
pub fn read_title_activity(title: &str) -> Option<Activity> {
    let first = title.trim_start().chars().next()?;
    match first {
        '\u{2800}'..='\u{28ff}' => Some(Activity::Working),
        '✦' => Some(Activity::Working),
        '✋' => Some(Activity::NeedsYou),
        '◇' => Some(Activity::Idle),
        _ => None,
    }
}

/// A notification either asks for the person or reports that the turn is done.
pub fn read_notification_activity(text: &str) -> Activity {
    let lowered = text.to_lowercase();
    let asks = [
        "approv",
        "permission",
        "wants to",
        "need your",
        "needs your",
        "waiting for your",
        "question",
    ]
    .iter()
    .any(|needle| lowered.contains(needle));
    if asks {
        Activity::NeedsYou
    } else {
        Activity::Finished
    }
}

/// What one chunk of output reported.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// The title set last, if any.
    pub title: Option<String>,
    /// The text of the last notification, if any.
    pub notification: Option<String>,
    /// The activity a launch hook reported last, if any.
    pub activity: Option<Activity>,
    /// The agent's conversation id the hook reported last, checked (`resume::is_conversation_id`).
    pub conversation: Option<String>,
    /// The folder the agent runs in, as the hook reported it last: an absolute path.
    pub conversation_folder: Option<String>,
}

/// An OSC longer than this is not a title or notification, so the rest is skipped.
const OSC_MAX_CHARS: usize = 4096;
const TITLE_MAX_CHARS: usize = 512;
const ACTIVITY_OSC: &str = "6973";
const ACTIVITY_PREFIX: &str = "northroom;activity=";
const SESSION_PREFIX: &str = "northroom;session=";
const SESSION_FOLDER_PREFIX: &str = "northroom;session-dir=";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Parser {
    Ground,
    Escape,
    Osc,
    OscEscape,
}

/// Reads what a program reports about itself through escape sequences in its streaming output:
/// the title (OSC 0/2), notifications (OSC 9, OSC 777 notify) and the activity a launch hook
/// reports (OSC 6973). A bare bell means nothing, since shells ring it on a failed completion.
/// State carries across chunks.
pub struct Signals {
    state: Parser,
    osc: String,
    overflowed: bool,
}

impl Default for Signals {
    fn default() -> Self {
        Self {
            state: Parser::Ground,
            osc: String::new(),
            overflowed: false,
        }
    }
}

impl Signals {
    pub fn observe(&mut self, piece: &str) -> Changes {
        let mut changes = Changes::default();
        for character in piece.chars() {
            self.take(character, &mut changes);
        }
        changes
    }

    /// Feeds one character of output through the escape parser.
    fn take(&mut self, character: char, changes: &mut Changes) {
        match self.state {
            Parser::Ground => {
                if character == '\u{1b}' {
                    self.state = Parser::Escape;
                }
            }
            Parser::Escape => match character {
                ']' => self.reset_osc(Parser::Osc),
                // A second ESC still waits for the character after it.
                '\u{1b}' => {}
                _ => self.state = Parser::Ground,
            },
            Parser::Osc => match character {
                '\u{7}' => self.finish(changes),
                '\u{1b}' => self.state = Parser::OscEscape,
                _ if self.overflowed => {}
                _ if self.osc.chars().count() >= OSC_MAX_CHARS => {
                    self.overflowed = true;
                    self.osc.clear();
                }
                _ => self.osc.push(character),
            },
            Parser::OscEscape if character == '\\' => self.finish(changes),
            // An ESC inside an OSC that does not terminate it abandons the OSC.
            Parser::OscEscape if character == ']' => self.reset_osc(Parser::Osc),
            Parser::OscEscape => self.reset_osc(Parser::Ground),
        }
    }

    /// Moves to `state` with an empty OSC buffer, to start a new OSC or abandon one.
    fn reset_osc(&mut self, state: Parser) {
        self.state = state;
        self.osc.clear();
        self.overflowed = false;
    }

    fn finish(&mut self, changes: &mut Changes) {
        self.state = Parser::Ground;
        if self.overflowed {
            return;
        }
        let osc = std::mem::take(&mut self.osc);
        let (code, text) = osc.split_once(';').unwrap_or((&osc, ""));
        match code {
            "0" | "2" => changes.title = Some(text.chars().take(TITLE_MAX_CHARS).collect()),
            // ConEmu's OSC 9;4 is a progress bar, not a notification.
            "9" if !text.starts_with("4;") => changes.notification = Some(text.to_owned()),
            "777" => {
                if let Some(said) = text.strip_prefix("notify;") {
                    changes.notification = Some(said.to_owned());
                }
            }
            ACTIVITY_OSC => read_hook(text, changes),
            _ => {}
        }
    }
}

/// What Northroom's hook reported in one OSC 6973: an activity, or the agent's conversation
/// and its folder. An id or folder that is not one is left out.
fn read_hook(text: &str, changes: &mut Changes) {
    if let Some(activity) = text.strip_prefix(ACTIVITY_PREFIX).and_then(Activity::named) {
        changes.activity = Some(activity);
    } else if let Some(id) = text
        .strip_prefix(SESSION_PREFIX)
        .filter(|id| crate::terminal::resume::is_conversation_id(id))
    {
        changes.conversation = Some(id.to_owned());
    } else if let Some(folder) = text
        .strip_prefix(SESSION_FOLDER_PREFIX)
        .filter(|folder| folder.starts_with('/') && !folder.chars().any(char::is_control))
    {
        changes.conversation_folder = Some(folder.to_owned());
    }
}

/// How the foreground agent was started: its arguments, and the folder it works in when it
/// could be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launched {
    pub arguments: Vec<String>,
    pub folder: Option<String>,
}

/// One session's changing activity.
pub struct Session {
    pub activity: Activity,
    pub title: String,
    /// The agent in the terminal's foreground, when one is (`terminal::agent`).
    pub agent: Option<crate::terminal::agent::Agent>,
    /// Something the person started, an agent or any other program, holds the foreground, so the
    /// shell is not at its prompt.
    pub busy: bool,
    /// The program reports through hooks, so its title and notifications are ignored.
    hooked: bool,
    /// The title last showed a working spinner, so a title without one means the turn ended.
    title_working: bool,
    /// The program has ended. The session keeps waiting for the person until they have seen it.
    ended: bool,
    /// The agent's conversation and the folder it runs in, as its hook reported them, and how
    /// it was started: what resumes it after a relaunch (`resume_command`).
    conversation: Option<String>,
    conversation_folder: Option<String>,
    launched: Launched,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            activity: Activity::Starting,
            title: String::new(),
            agent: None,
            busy: false,
            hooked: false,
            title_working: false,
            ended: false,
            conversation: None,
            conversation_folder: None,
            launched: Launched::default(),
        }
    }
}

impl Session {
    /// Applies what a chunk of output reported. `watched` says whether the session is on screen;
    /// a turn that ends while watched counts as seen. Returns whether the activity changed.
    pub fn observe(&mut self, changes: &Changes, watched: bool) -> bool {
        if let Some(id) = &changes.conversation {
            self.conversation = Some(id.clone());
        }
        if let Some(folder) = &changes.conversation_folder {
            self.conversation_folder = Some(folder.clone());
        }
        let read = self.read(changes);
        if let Some(title) = &changes.title {
            self.title.clone_from(title);
        }
        let Some(mut next) = read else {
            return false;
        };
        if next == Activity::Finished && watched {
            next = Activity::Idle;
        }
        let changed = next != self.activity;
        self.activity = next;
        changed
    }

    fn read(&mut self, changes: &Changes) -> Option<Activity> {
        if let Some(reported) = changes.activity {
            self.hooked = true;
            return Some(reported);
        }
        if !self.hooked {
            if let Some(read) = self.read_unhooked(changes) {
                return Some(read);
            }
        }
        (self.activity == Activity::Starting).then_some(Activity::Idle)
    }

    /// The activity of a program without hooks, read from its notifications and title. A title that
    /// drops its working spinner ends the turn.
    fn read_unhooked(&mut self, changes: &Changes) -> Option<Activity> {
        if let Some(notification) = &changes.notification {
            return Some(read_notification_activity(notification));
        }
        let titled = read_title_activity(changes.title.as_ref()?);
        let was_working = self.title_working;
        self.title_working = titled == Some(Activity::Working);
        if titled == Some(Activity::Idle) || (titled.is_none() && was_working) {
            return Some(if was_working {
                Activity::Finished
            } else {
                Activity::Idle
            });
        }
        titled
    }

    /// Sets the foreground agent to `agent`: a different one, or none once the person is back at
    /// their shell. The previous agent's reported state is dropped. An agent that exits while
    /// working with nobody watching (a one-line run) counts as finished; any other exit leaves
    /// nothing to wait for. Returns whether it changed.
    pub fn runs(&mut self, agent: Option<crate::terminal::agent::Agent>, watched: bool) -> bool {
        if agent == self.agent {
            return false;
        }
        // Leaving an agent drops its conversation. One just found keeps what its hook reported
        // first, which can come before the foreground is looked at.
        if self.agent.is_some() {
            self.conversation = None;
            self.conversation_folder = None;
            self.launched = Launched::default();
        }
        self.agent = agent;
        self.hooked = false;
        self.title_working = false;
        if agent.is_none() && !self.ended {
            self.activity = match self.activity {
                Activity::Working if !watched => Activity::Finished,
                Activity::Working | Activity::NeedsYou => Activity::Idle,
                kept => kept,
            };
        }
        true
    }

    /// Records how the foreground agent was started (`resume_command`).
    pub fn launched(&mut self, launched: Launched) {
        self.launched = launched;
    }

    /// What opens the agent's conversation again after a relaunch (`resume::command`): the one
    /// its hook reported, else its folder's latest. The folder the hook reported wins over the
    /// one it was started in. `None` with no agent in the foreground.
    pub fn resume_command(&self) -> Option<crate::terminal::resume::Resume> {
        let folder = self
            .conversation_folder
            .as_deref()
            .or(self.launched.folder.as_deref());
        crate::terminal::resume::command(
            self.agent?,
            self.conversation.as_deref(),
            folder,
            &self.launched.arguments,
        )
    }

    /// Records whether something the person started holds the foreground (`busy`). Returns whether
    /// that changed.
    pub fn holds(&mut self, busy: bool) -> bool {
        let changed = busy != self.busy;
        self.busy = busy;
        changed
    }

    /// Marks the session as seen, as when the terminal is opened or typed into: a finished turn
    /// becomes idle, and a program that ended unseen becomes ended.
    pub fn seen(&mut self) -> bool {
        let waits = matches!(self.activity, Activity::Finished | Activity::NeedsYou);
        if self.ended && waits {
            self.activity = Activity::Ended;
            return true;
        }
        let changed = self.activity == Activity::Finished;
        if changed {
            self.activity = Activity::Idle;
        }
        changed
    }

    /// Records that the program ended with `exit`. With nobody watching, the session waits for the
    /// person to look instead of leaving the list unnoticed: it needs them if the run failed or
    /// was signalled, and is finished if it succeeded.
    pub fn ended(&mut self, exit: crate::terminal::exit::Exit, watched: bool) {
        self.ended = true;
        self.activity = match (watched, exit.code) {
            (true, _) => Activity::Ended,
            (false, Some(0)) => Activity::Finished,
            (false, Some(_) | None) => Activity::NeedsYou,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hook_s_conversation_resumes_the_agent_until_it_leaves() {
        use crate::terminal::agent::Agent;
        let mut signals = Signals::default();
        let mut session = Session::default();
        // An id that is not one, or a folder that is not a path, is left out.
        let bad = signals.observe(
            "\u{1b}]6973;northroom;session=a;rm -rf ~\u{7}\u{1b}]6973;northroom;session-dir=p\u{7}",
        );
        assert_eq!((bad.conversation, bad.conversation_folder), (None, None));
        // The hook can report before the foreground is looked at; the agent keeps it.
        let heard = signals.observe(
            "\u{1b}]6973;northroom;session=3f2a-c91\u{7}\u{1b}]6973;northroom;session-dir=/p/My Work\u{7}",
        );
        session.observe(&heard, true);
        let command = |session: &Session| session.resume_command().map(|resume| resume.command);
        assert_eq!(command(&session), None, "no agent in the foreground yet");
        session.runs(Some(Agent::Claude), true);
        session.launched(Launched {
            arguments: vec![
                "claude".to_owned(),
                "--dangerously-skip-permissions".to_owned(),
            ],
            folder: Some("/p".to_owned()),
        });
        assert_eq!(
            command(&session).as_deref(),
            Some("cd '/p/My Work' && claude --resume 3f2a-c91 --dangerously-skip-permissions")
        );
        // Back at the shell (/exit), there is nothing to resume.
        session.runs(None, true);
        assert_eq!(command(&session), None);
        // An agent without a hook resumes its folder's latest, from the folder it started in.
        session.runs(Some(Agent::Codex), true);
        session.launched(Launched {
            arguments: vec!["codex".to_owned()],
            folder: Some("/p/Codex Work".to_owned()),
        });
        assert_eq!(
            command(&session).as_deref(),
            Some("cd '/p/Codex Work' && codex resume --last")
        );
    }

    #[test]
    fn activities_are_numbered_and_running_ones_take_references() {
        for number in 1..=6 {
            assert_eq!(
                Activity::numbered(number).map(Activity::number),
                Some(number)
            );
        }
        assert_eq!(Activity::numbered(0), None);
        let taking: Vec<u32> = (1..=6)
            .filter(|number| Activity::numbered(*number).is_some_and(Activity::takes_references))
            .collect();
        assert_eq!(
            taking,
            [1, 3, 4, 5],
            "idle, working, needs you and finished read what is pasted"
        );
        assert_eq!(label_of(None, "en"), "Not running");
        assert_eq!(label_of(Some(Activity::NeedsYou), "en"), "Needs you");
        assert_eq!(
            [
                Activity::Starting,
                Activity::Working,
                Activity::Finished,
                Activity::Idle,
                Activity::Ended
            ]
            .map(|activity| activity.label("en")),
            ["Starting", "Working", "Finished", "Idle", "Ended"]
        );
        assert_eq!(
            label_of(None, "en"),
            strings::text("en", key::TERMINAL_NOT_RUNNING)
        );
    }

    #[test]
    fn each_state_has_its_own_shape_and_only_what_wants_the_person_moves() {
        assert_eq!(look(Some(Activity::Working)).sign, Sign::Comet);
        assert_eq!(look(Some(Activity::NeedsYou)).sign, Sign::Nudge);
        assert_eq!(look(Some(Activity::Finished)).sign, Sign::Check);
        assert_eq!(look(Some(Activity::Idle)).sign, Sign::Ring);
        assert_eq!(look(Some(Activity::Starting)).sign, Sign::Dot);
        assert_eq!(look(Some(Activity::Ended)).sign, Sign::Dot);
    }

    #[test]
    fn a_status_worth_going_back_to_takes_its_colour() {
        assert_eq!(
            look(Some(Activity::Working)),
            Look {
                ring: Token::StatusInfo,
                ring_opacity: 0.5,
                dot: Some(Token::StatusInfo),
                dot_fade: 1.0,
                sign: Sign::Comet,
                label: Token::Foreground
            }
        );
        assert_eq!(look(Some(Activity::NeedsYou)).label, Token::StatusWarning);
        assert_eq!(look(Some(Activity::Finished)).ring_opacity, 0.6);
        assert_eq!(look(Some(Activity::Starting)).ring_opacity, 0.15);
        assert_eq!(look(Some(Activity::Ended)).dot_fade, 0.6);
        assert_eq!(look(None).dot, None, "no session, no dot");
        assert_eq!(look(Some(Activity::Idle)).dot, Some(Token::MutedForeground));
    }

    #[test]
    fn titles_and_notifications_are_read_across_pieces() {
        let mut signals = Signals::default();
        assert_eq!(
            signals.observe("plain \u{7} text"),
            Changes::default(),
            "a bell alone says nothing"
        );
        let first = signals.observe("\u{1b}]0;⠋ Think");
        assert_eq!(first.title, None, "the title is not finished yet");
        assert_eq!(
            signals.observe("ing\u{7}").title.as_deref(),
            Some("⠋ Thinking")
        );
        assert_eq!(
            signals.observe("\u{1b}]2;done\u{1b}\\").title.as_deref(),
            Some("done")
        );
        assert_eq!(
            signals.observe("\u{1b}]9;4;1;50\u{7}").notification,
            None,
            "a progress bar is no notification"
        );
        assert_eq!(
            signals
                .observe("\u{1b}]9;Turn complete\u{7}")
                .notification
                .as_deref(),
            Some("Turn complete")
        );
        assert_eq!(
            signals
                .observe("\u{1b}]777;notify;Codex;Approve?\u{7}")
                .notification
                .as_deref(),
            Some("Codex;Approve?")
        );
        assert_eq!(
            signals
                .observe("\u{1b}]6973;northroom;activity=needs-you\u{7}")
                .activity,
            Some(Activity::NeedsYou)
        );
        assert_eq!(
            signals
                .observe("\u{1b}]6973;northroom;activity=dancing\u{7}")
                .activity,
            None
        );
        // A sequence split across chunks is read once it ends.
        assert_eq!(signals.observe("\u{1b}]6973;north").activity, None);
        assert_eq!(
            signals.observe("room;activity=working\u{7}").activity,
            Some(Activity::Working)
        );
        let long = format!("\u{1b}]0;{}\u{7}", "x".repeat(5000));
        assert_eq!(
            signals.observe(&long).title,
            None,
            "an over-long sequence is skipped"
        );
    }

    fn titled(title: &str) -> Changes {
        Changes {
            title: Some(title.to_owned()),
            ..Changes::default()
        }
    }

    #[test]
    fn a_spinner_title_is_working_and_its_end_is_a_finished_turn() {
        let mut session = Session::default();
        assert!(
            session.observe(&Changes::default(), false),
            "first output: starting becomes idle"
        );
        assert_eq!(session.activity, Activity::Idle);
        assert!(session.observe(&titled("⠙ Editing"), false));
        assert_eq!(session.activity, Activity::Working);
        assert!(session.observe(&titled("codex"), false));
        assert_eq!(session.activity, Activity::Finished);
        assert!(session.seen());
        assert_eq!(session.activity, Activity::Idle);

        // A turn that ends while its terminal is on screen is already seen.
        session.observe(&titled("⠙ Editing"), true);
        session.observe(&titled("codex"), true);
        assert_eq!(session.activity, Activity::Idle);
    }

    #[test]
    fn a_notification_asks_or_finishes_and_hooks_are_trusted_over_both() {
        let mut session = Session::default();
        let notified = |text: &str| Changes {
            notification: Some(text.to_owned()),
            ..Changes::default()
        };
        session.observe(
            &notified("Claude needs your permission to run a command"),
            false,
        );
        assert_eq!(session.activity, Activity::NeedsYou);
        session.observe(&notified("Turn complete"), false);
        assert_eq!(session.activity, Activity::Finished);

        let hooked = Changes {
            activity: Some(Activity::Working),
            ..Changes::default()
        };
        session.observe(&hooked, false);
        assert_eq!(session.activity, Activity::Working);
        // Once a program reports through hooks, its title and notifications are ignored.
        assert!(!session.observe(&notified("Turn complete"), false));
        assert!(!session.observe(&titled("◇ ready"), false));
        assert_eq!(session.activity, Activity::Working);
        session.ended(crate::terminal::exit::Exit { code: Some(0) }, true);
        assert_eq!(session.activity.label("en"), "Ended");
    }

    #[test]
    fn an_agent_that_leaves_takes_what_it_said_with_it() {
        use crate::terminal::agent::Agent;
        let reported = |activity| Changes {
            activity: Some(activity),
            ..Changes::default()
        };
        let mut session = Session::default();
        assert!(session.runs(Some(Agent::Claude), true));
        assert!(!session.runs(Some(Agent::Claude), true));
        session.observe(&reported(Activity::Working), true);
        // Back at the shell while someone watches: nothing to wait for.
        assert!(session.runs(None, true));
        assert_eq!((session.agent, session.activity), (None, Activity::Idle));
        // The next program is read by its title again, not bound to the last one's hooks.
        session.observe(
            &Changes {
                title: Some("\u{2819} building".to_owned()),
                ..Changes::default()
            },
            true,
        );
        assert_eq!(session.activity, Activity::Working);

        // A run that ends with nobody watching has finished, until it is looked at.
        let mut unseen = Session::default();
        unseen.runs(Some(Agent::Codex), false);
        unseen.observe(&reported(Activity::Working), false);
        unseen.runs(None, false);
        assert_eq!(unseen.activity, Activity::Finished);
        // An agent that was asking and is gone asks nothing; a finished turn stays finished.
        let mut asking = Session::default();
        asking.runs(Some(Agent::Claude), false);
        asking.observe(&reported(Activity::NeedsYou), false);
        asking.runs(None, false);
        assert_eq!(asking.activity, Activity::Idle);
        let mut done = Session::default();
        done.runs(Some(Agent::Claude), false);
        done.observe(&reported(Activity::Finished), false);
        done.runs(None, false);
        assert_eq!(done.activity, Activity::Finished);
    }

    #[test]
    fn a_program_that_ends_unseen_waits_until_it_is_looked_at() {
        use crate::terminal::exit::Exit;
        // Succeeded with nobody watching: finished until the terminal is looked at.
        let mut session = Session::default();
        session.ended(Exit { code: Some(0) }, false);
        assert_eq!(session.activity, Activity::Finished);
        assert!(session.seen());
        assert_eq!(session.activity, Activity::Ended);
        assert!(!session.seen());
        // Failed, or stopped by a signal: it needs the person, until they look.
        for code in [Some(1), None] {
            let mut session = Session::default();
            session.ended(Exit { code }, false);
            assert_eq!(session.activity, Activity::NeedsYou);
            assert!(session.seen());
            assert_eq!(session.activity, Activity::Ended);
        }
        // A session still running that needs the person is not answered by looking at it.
        let mut asking = Session::default();
        asking.observe(
            &Changes {
                activity: Some(Activity::NeedsYou),
                ..Changes::default()
            },
            false,
        );
        assert!(!asking.seen());
        assert_eq!(asking.activity, Activity::NeedsYou);
    }
}
