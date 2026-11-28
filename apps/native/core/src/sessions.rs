//! All running terminal sessions as one list (ADR-0005): what the title bar's sessions button
//! shows, what its menu lists, and where the "next session" key goes.
//!
//! Sessions are grouped by what they need from the person, regardless of folder: those waiting
//! for an answer, then unseen finished ones, then working, then idle. Within a group they sort
//! by folder and name, so a row stays put while the sessions around it change.

pub mod ffi;
pub mod peek;
pub mod recent;

use crate::strings::{self, key};
use crate::terminal::activity::Activity;
use std::collections::HashMap;

/// One running session: the terminal file it belongs to and what it is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    /// The terminal file's id in the drive.
    pub id: String,
    /// The file's name as its card shows it.
    pub name: String,
    /// The file's folder path joined with " / ", or the drive's name for a file at the top of the
    /// drive.
    pub folder: String,
    /// What the program says it is doing (`doing_of`); empty when it says nothing.
    pub doing: String,
    /// The agent running in it (`Agent::label`), or empty for a plain shell.
    pub agent: String,
    /// What it asks the person (`Asked::words`); empty unless it needs them and said what for.
    pub question: String,
    pub activity: Activity,
}

/// The session groups in display order, each named by the activity its sessions share.
pub const GROUPS: [Activity; 4] = [
    Activity::NeedsYou,
    Activity::Finished,
    Activity::Working,
    Activity::Idle,
];

/// What a terminal's session is listed as doing, or `None` when it is not listed. A terminal is
/// listed while something runs in it. An agent is always listed (`agent_runs`); a shell running
/// anything else (`busy`) is working; a shell at its prompt is not listed unless what last ran
/// there still waits for the person.
pub fn listed_activity(activity: Activity, agent_runs: bool, busy: bool) -> Option<Activity> {
    match activity {
        Activity::NeedsYou | Activity::Finished | Activity::Working => Some(activity),
        Activity::Ended => None,
        Activity::Starting | Activity::Idle if agent_runs => Some(activity),
        Activity::Starting | Activity::Idle if busy => Some(Activity::Working),
        Activity::Starting | Activity::Idle => None,
    }
}

/// What a terminal's session is listed as doing for a paired device, or `None` once it ended. A
/// device lists every open terminal, so a shell waiting at its prompt can be typed into: what
/// `listed_activity` leaves out is idle.
pub fn device_activity(activity: Activity, agent_runs: bool, busy: bool) -> Option<Activity> {
    match activity {
        Activity::Ended => None,
        Activity::NeedsYou
        | Activity::Finished
        | Activity::Working
        | Activity::Starting
        | Activity::Idle => {
            Some(listed_activity(activity, agent_runs, busy).unwrap_or(Activity::Idle))
        }
    }
}

/// The group for a session doing `activity`. A starting session counts as idle, and an ended
/// one is not listed.
pub fn group_of(activity: Activity) -> Option<Activity> {
    match activity {
        Activity::NeedsYou => Some(Activity::NeedsYou),
        Activity::Finished => Some(Activity::Finished),
        Activity::Working => Some(Activity::Working),
        Activity::Starting | Activity::Idle => Some(Activity::Idle),
        Activity::Ended => None,
    }
}

/// The sessions of `running` listed in `group`, by folder and then name.
fn listed_in(running: &[Running], group: Activity) -> Vec<&Running> {
    let mut listed: Vec<&Running> = running
        .iter()
        .filter(|session| group_of(session.activity) == Some(group))
        .collect();
    listed.sort_by(|left, right| {
        (&left.folder, &left.name, &left.id).cmp(&(&right.folder, &right.name, &right.id))
    });
    listed
}

/// `running` in its groups, the empty ones left out.
pub fn grouped(running: &[Running]) -> Vec<(Activity, Vec<&Running>)> {
    GROUPS
        .into_iter()
        .map(|group| (group, listed_in(running, group)))
        .filter(|(_, listed)| !listed.is_empty())
        .collect()
}

/// What the title bar's sessions button shows: the one state most worth the person's attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// This many sessions wait for an answer.
    NeedsYou(usize),
    /// None is waiting, and one finished unseen.
    Finished,
    /// None is waiting or finished, and one is working.
    Working,
    /// Sessions are running, all idle.
    Idle,
    /// No session is running.
    Nothing,
}

/// How many of `running` are listed in `group`.
fn count_in(running: &[Running], group: Activity) -> usize {
    running
        .iter()
        .filter(|session| group_of(session.activity) == Some(group))
        .count()
}

/// The button state for `running`: the first group in `GROUPS` that holds a session.
pub fn signal(running: &[Running]) -> Signal {
    let first = GROUPS
        .into_iter()
        .find(|&group| count_in(running, group) > 0);
    match first {
        Some(Activity::NeedsYou) => Signal::NeedsYou(count_in(running, Activity::NeedsYou)),
        Some(Activity::Finished) => Signal::Finished,
        Some(Activity::Working) => Signal::Working,
        Some(Activity::Idle) => Signal::Idle,
        Some(Activity::Starting | Activity::Ended) | None => Signal::Nothing,
    }
}

/// The words for `sessions` waiting for an answer: "1 needs you", "3 need you".
pub fn needing_words(sessions: usize, language: &str) -> String {
    if sessions == 1 {
        strings::text(language, key::SESSIONS_NEEDS_YOU).to_owned()
    } else {
        counted(language, key::SESSIONS_NEED_YOU, sessions)
    }
}

fn counted(language: &str, words: strings::Key, sessions: usize) -> String {
    strings::format(language, words, &[("count", &sessions.to_string())])
}

/// The button's tooltip: the count in each group, in group order, such as
/// "2 need you · 1 finished · 3 working", or a note that none is running.
pub fn summary(running: &[Running], language: &str) -> String {
    let said: Vec<String> = GROUPS
        .into_iter()
        .map(|group| (group, count_in(running, group)))
        .filter(|&(_, sessions)| sessions > 0)
        .map(|(group, sessions)| match group {
            Activity::NeedsYou => needing_words(sessions, language),
            Activity::Finished => counted(language, key::SESSIONS_FINISHED, sessions),
            Activity::Working => counted(language, key::SESSIONS_WORKING, sessions),
            Activity::Idle | Activity::Starting | Activity::Ended => {
                counted(language, key::SESSIONS_IDLE, sessions)
            }
        })
        .collect();
    if said.is_empty() {
        strings::text(language, key::SESSIONS_NONE).to_owned()
    } else {
        said.join(" · ")
    }
}

/// The sessions waiting for the person: those that need them, then the finished ones.
pub fn waiting(running: &[Running]) -> Vec<&Running> {
    let mut waiting = listed_in(running, Activity::NeedsYou);
    waiting.extend(listed_in(running, Activity::Finished));
    waiting
}

/// The session the "next session" key goes to: the first waiting one, or, when one of them is
/// `shown`, the one after it, wrapping round. `None` when no other session is waiting.
pub fn next<'a>(running: &'a [Running], shown: Option<&str>) -> Option<&'a Running> {
    let waiting = waiting(running);
    let on_screen = waiting
        .iter()
        .position(|session| Some(session.id.as_str()) == shown);
    match on_screen {
        None => waiting.first().copied(),
        Some(_) if waiting.len() == 1 => None,
        Some(at) => Some(waiting[(at + 1) % waiting.len()]),
    }
}

/// Each session's activity at the last check, kept to notify the person of sessions that have
/// started waiting for them since.
#[derive(Debug, Default)]
pub struct Watch {
    seen: HashMap<String, Activity>,
}

impl Watch {
    /// Takes every session's current activity, keyed by terminal file id, and returns the ids of
    /// those that just started needing the person or finished. `watched` is the session on screen
    /// in the window the person is at, if any, whose change they see for themselves.
    pub fn take_news(
        &mut self,
        states: &[(String, Activity)],
        watched: Option<&str>,
    ) -> Vec<String> {
        let news = states
            .iter()
            .filter(|(id, activity)| {
                matches!(activity, Activity::NeedsYou | Activity::Finished)
                    && self.seen.get(id) != Some(activity)
                    && Some(id.as_str()) != watched
            })
            .map(|(id, _)| id.clone())
            .collect();
        self.seen = states.iter().cloned().collect();
        news
    }
}

/// What a program says it is doing: the title it set, minus the leading spinner or mark
/// (`terminal::activity::read_title_activity`) that the row's dot already shows.
pub fn doing_of(title: &str) -> String {
    title
        .trim_start_matches(|letter: char| {
            letter.is_whitespace() || (!letter.is_ascii() && !letter.is_alphanumeric())
        })
        .trim_end()
        .to_owned()
}

/// The task a session's program reports, or `None` when its title names no task: empty, only the
/// program's own name (which the terminal's name or the agent already shows), or a place, as a
/// shell's title is ("~/drive/Finance", "me@mac:~").
pub fn task_of(session: &Running) -> Option<&str> {
    task_said(&session.doing, &session.name, &session.agent)
}

/// The task in what a program says it is `doing` (`doing_of`), for a terminal called `name`
/// running `agent` (its label, or empty): `task_of` for a session that is not listed.
pub fn task_said<'a>(doing: &'a str, name: &str, agent: &str) -> Option<&'a str> {
    let said = doing.to_lowercase();
    let only_a_name =
        said.is_empty() || name.to_lowercase().starts_with(&said) || said == agent.to_lowercase();
    let a_place = doing.starts_with('~')
        || doing.starts_with('/')
        || (doing.contains('@') && doing.contains(':'));
    (!only_a_name && !a_place).then_some(doing)
}

/// A row's main line: the session's task when it reports one, else its terminal's name.
pub fn heading_of(session: &Running) -> String {
    task_of(session).map_or_else(|| session.name.clone(), str::to_owned)
}

/// A row's muted second line: what the session asks the person when it said, else where and
/// what it is (`place_of`).
pub fn detail_of(session: &Running) -> String {
    if session.question.is_empty() {
        place_of(session)
    } else {
        session.question.clone()
    }
}

/// The session's folder, then the agent unless the terminal's name already starts with it, then
/// the terminal's name when the main line shows the task instead.
fn place_of(session: &Running) -> String {
    let mut said = vec![session.folder.as_str()];
    let agent = session.agent.as_str();
    if !agent.is_empty()
        && !session
            .name
            .to_lowercase()
            .starts_with(&agent.to_lowercase())
    {
        said.push(agent);
    }
    if task_of(session).is_some() {
        said.push(&session.name);
    }
    said.join(" · ")
}

/// `words` cut to at most `most` characters, ending in "…" when cut.
pub fn shortened(words: &str, most: usize) -> String {
    if words.chars().count() <= most {
        return words.to_owned();
    }
    let kept: String = words.chars().take(most.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, folder: &str, name: &str, activity: Activity) -> Running {
        Running {
            id: id.to_owned(),
            name: name.to_owned(),
            folder: folder.to_owned(),
            doing: String::new(),
            agent: String::new(),
            question: String::new(),
            activity,
        }
    }

    fn ids(listed: &[&Running]) -> Vec<String> {
        listed.iter().map(|session| session.id.clone()).collect()
    }

    #[test]
    fn a_device_lists_a_shell_at_its_prompt_as_idle_and_an_ended_one_not_at_all() {
        assert_eq!(listed_activity(Activity::Idle, false, false), None);
        assert_eq!(
            device_activity(Activity::Idle, false, false),
            Some(Activity::Idle)
        );
        assert_eq!(
            device_activity(Activity::Idle, false, true),
            Some(Activity::Working)
        );
        assert_eq!(device_activity(Activity::Ended, true, true), None);
    }

    #[test]
    fn sessions_are_grouped_by_what_they_ask_and_ordered_by_folder_and_name() {
        let running = [
            session("w", "Finance", "Bookkeeping", Activity::Working),
            session("n2", "Sales", "Outreach", Activity::NeedsYou),
            session("s", "Website", "Copy", Activity::Starting),
            session("n1", "Legal", "Review", Activity::NeedsYou),
            session("f", "Marketing", "Film", Activity::Finished),
            session("e", "Legal", "Old", Activity::Ended),
            session("i", "Finance", "Shell", Activity::Idle),
        ];
        let groups: Vec<(Activity, Vec<String>)> = grouped(&running)
            .iter()
            .map(|(group, listed)| (*group, ids(listed)))
            .collect();
        assert_eq!(
            groups,
            [
                (Activity::NeedsYou, vec!["n1".to_owned(), "n2".to_owned()]),
                (Activity::Finished, vec!["f".to_owned()]),
                (Activity::Working, vec!["w".to_owned()]),
                (Activity::Idle, vec!["i".to_owned(), "s".to_owned()]),
            ],
            "a starting session is listed with the idle, an ended one not at all"
        );
        assert_eq!(grouped(&running[5..6]), [], "only an ended session");
    }

    #[test]
    fn a_terminal_is_listed_while_something_runs_in_it() {
        use Activity::{Ended, Finished, Idle, NeedsYou, Starting, Working};
        // An agent, whatever it does.
        assert_eq!(listed_activity(Idle, true, true), Some(Idle));
        assert_eq!(listed_activity(Starting, true, true), Some(Starting));
        assert_eq!(listed_activity(NeedsYou, true, true), Some(NeedsYou));
        // Another program running in the shell is at work.
        assert_eq!(listed_activity(Idle, false, true), Some(Working));
        assert_eq!(listed_activity(Starting, false, true), Some(Working));
        // A shell at its prompt is not listed, but an agent that finished unseen still is.
        assert_eq!(listed_activity(Idle, false, false), None);
        assert_eq!(listed_activity(Starting, false, false), None);
        assert_eq!(listed_activity(Finished, false, false), Some(Finished));
        assert_eq!(listed_activity(Working, false, false), Some(Working));
        assert_eq!(listed_activity(Ended, true, true), None);
    }

    #[test]
    fn the_button_says_the_first_thing_that_asks() {
        let running = [
            session("a", "A", "a", Activity::NeedsYou),
            session("b", "A", "b", Activity::NeedsYou),
            session("c", "A", "c", Activity::Finished),
            session("d", "A", "d", Activity::Working),
            session("e", "A", "e", Activity::Starting),
        ];
        assert_eq!(signal(&running), Signal::NeedsYou(2));
        assert_eq!(signal(&running[2..]), Signal::Finished);
        assert_eq!(signal(&running[3..]), Signal::Working);
        assert_eq!(signal(&running[4..]), Signal::Idle, "one starting is idle");
        assert_eq!(signal(&[]), Signal::Nothing);
        assert_eq!(
            signal(&[session("x", "A", "x", Activity::Ended)]),
            Signal::Nothing
        );
    }

    #[test]
    fn the_button_names_how_many_sessions_each_group_holds() {
        let running = [
            session("a", "A", "a", Activity::NeedsYou),
            session("b", "A", "b", Activity::NeedsYou),
            session("c", "A", "c", Activity::Finished),
            session("d", "A", "d", Activity::Working),
            session("e", "A", "e", Activity::Idle),
            session("f", "A", "f", Activity::Starting),
        ];
        assert_eq!(
            summary(&running, "en"),
            "2 need you · 1 finished · 1 working · 2 idle"
        );
        assert_eq!(summary(&running[1..3], "en"), "1 needs you · 1 finished");
        assert_eq!(summary(&[], "en"), "No agents running");
        assert_eq!(needing_words(1, "en"), "1 needs you");
        assert_eq!(needing_words(12, "en"), "12 need you");
    }

    #[test]
    fn the_next_session_is_one_that_waits_and_goes_round_them() {
        let running = [
            session("w", "A", "w", Activity::Working),
            session("f", "A", "f", Activity::Finished),
            session("n1", "A", "n1", Activity::NeedsYou),
            session("n2", "B", "n2", Activity::NeedsYou),
        ];
        let next_id = |shown: Option<&str>| next(&running, shown).map(|session| session.id.clone());
        assert_eq!(next_id(None), Some("n1".to_owned()));
        assert_eq!(
            next_id(Some("w")),
            Some("n1".to_owned()),
            "from one at work"
        );
        assert_eq!(next_id(Some("n1")), Some("n2".to_owned()));
        assert_eq!(
            next_id(Some("n2")),
            Some("f".to_owned()),
            "then the finished"
        );
        assert_eq!(next_id(Some("f")), Some("n1".to_owned()), "and round again");
        assert_eq!(
            next(&running[2..3], Some("n1")),
            None,
            "the only one waiting is already on screen"
        );
        assert_eq!(next(&running[..1], None), None, "nothing waits");
    }

    #[test]
    fn a_session_is_news_once_when_it_comes_to_wait_unseen() {
        let states = |pairs: &[(&str, Activity)]| -> Vec<(String, Activity)> {
            pairs
                .iter()
                .map(|(id, activity)| ((*id).to_owned(), *activity))
                .collect()
        };
        let mut watch = Watch::default();
        let working = states(&[("a", Activity::Working), ("b", Activity::Working)]);
        assert_eq!(watch.take_news(&working, None), Vec::<String>::new());
        let asked = states(&[("a", Activity::NeedsYou), ("b", Activity::Working)]);
        assert_eq!(watch.take_news(&asked, None), ["a"]);
        assert_eq!(
            watch.take_news(&asked, None),
            Vec::<String>::new(),
            "said once"
        );
        // Answered, at work again, and asking a second time: news again.
        watch.take_news(&working, None);
        assert_eq!(watch.take_news(&asked, None), ["a"]);
        // The session the person is looking at is not news; the other finishing is.
        let both = states(&[("a", Activity::Finished), ("b", Activity::Finished)]);
        assert_eq!(watch.take_news(&both, Some("a")), ["b"]);
        assert_eq!(watch.take_news(&[], None), Vec::<String>::new());
    }

    #[test]
    fn what_a_session_is_doing_is_its_title_without_the_spinner() {
        assert_eq!(doing_of("⠋ Reconciling October"), "Reconciling October");
        assert_eq!(doing_of("✳ Fix the login form "), "Fix the login form");
        assert_eq!(doing_of("✋  Approve?"), "Approve?");
        assert_eq!(doing_of("~/drive/Finance"), "~/drive/Finance");
        assert_eq!(doing_of("日本語"), "日本語");
        assert_eq!(doing_of("⠙"), "");
    }

    #[test]
    fn a_row_leads_with_the_task_and_names_folder_agent_and_terminal_below() {
        let mut row = session("a", "Sales / Q4 leads", "Terminal 2", Activity::Working);
        // Nothing said: the terminal's name leads.
        assert_eq!(heading_of(&row), "Terminal 2");
        assert_eq!(detail_of(&row), "Sales / Q4 leads");
        row.agent = "Claude Code".to_owned();
        assert_eq!(detail_of(&row), "Sales / Q4 leads · Claude Code");
        // A task: it leads, and the terminal's name moves below.
        row.doing = "Drafting emails".to_owned();
        assert_eq!(heading_of(&row), "Drafting emails");
        assert_eq!(
            detail_of(&row),
            "Sales / Q4 leads · Claude Code · Terminal 2"
        );
        // The agent's own name is not a task.
        row.doing = "Claude Code".to_owned();
        assert_eq!(heading_of(&row), "Terminal 2");
        // Nor is a shell's place.
        row.doing = "~/drive/Sales".to_owned();
        assert_eq!(heading_of(&row), "Terminal 2");
        row.doing = "me@mac:~/drive".to_owned();
        assert_eq!(task_of(&row), None);
        // A terminal named after its agent doesn't repeat it.
        row.name = "Claude Code 2".to_owned();
        row.doing = "Drafting emails".to_owned();
        assert_eq!(detail_of(&row), "Sales / Q4 leads · Claude Code 2");
        // A session that says what it asks shows that below instead.
        row.activity = Activity::NeedsYou;
        row.question = "Allow Bash: rm -rf build/?".to_owned();
        assert_eq!(heading_of(&row), "Drafting emails");
        assert_eq!(detail_of(&row), "Allow Bash: rm -rf build/?");
    }

    #[test]
    fn long_words_are_cut_with_an_ellipsis() {
        assert_eq!(shortened("Fix the login form", 30), "Fix the login form");
        assert_eq!(shortened("Research three competitors", 12), "Research th…");
        assert_eq!(shortened("Research three", 10), "Research…");
    }
}
