use super::*;
use crate::remote::devices::FILE;
use crate::terminal::activity::Activity;
use crate::terminal::input::Key as Pressable;
use std::cell::RefCell;
use std::time::Duration;

const PHONE: Key = [1; 32];
const STRANGER: Key = [2; 32];

/// Terminals that record what was done to them.
#[derive(Default)]
struct FakeDesk {
    running: RefCell<Vec<Running>>,
    /// The text each running session's screen shows.
    shows: RefCell<HashMap<String, String>>,
    done: RefCell<Vec<String>>,
}

impl FakeDesk {
    fn with(sessions: &[(&str, Activity)]) -> Self {
        let desk = Self::default();
        for (id, activity) in sessions {
            desk.running.borrow_mut().push(Running {
                id: (*id).to_owned(),
                name: format!("{id} name"),
                folder: "Work".to_owned(),
                doing: String::new(),
                agent: "Claude Code".to_owned(),
                activity: *activity,
            });
            desk.shows
                .borrow_mut()
                .insert((*id).to_owned(), format!("{id} screen"));
        }
        desk
    }

    fn done(&self) -> Vec<String> {
        self.done.borrow().clone()
    }
}

fn screen_showing(text: &str) -> Screen {
    Screen {
        columns: 40,
        lines: 10,
        ink: "#ffffff".to_owned(),
        ground: "#000000".to_owned(),
        rows: vec![vec![crate::remote::screen::Run {
            text: text.to_owned(),
            ..Default::default()
        }]],
        cursor: None,
    }
}

impl Desk for FakeDesk {
    fn sessions(&self) -> Vec<Running> {
        self.running.borrow().clone()
    }

    fn screen(&self, session: &str) -> Option<Screen> {
        self.shows
            .borrow()
            .get(session)
            .map(|text| screen_showing(text))
    }

    fn fit(&self, session: &str, grid: Option<GridSize>) {
        let size = grid.map_or("own size".to_owned(), |grid| {
            format!("{}x{}", grid.columns, grid.lines)
        });
        self.done.borrow_mut().push(format!("fit {session} {size}"));
    }

    fn paste(&self, session: &str, text: &str) {
        self.done
            .borrow_mut()
            .push(format!("paste {session} {text}"));
    }

    fn press(&self, session: &str, pressed: Pressed) {
        self.done
            .borrow_mut()
            .push(format!("press {session} {pressed:?}"));
    }
}

struct Fixture {
    service: Service,
    desk: FakeDesk,
    now: Instant,
    _folder: tempfile::TempDir,
}

fn fixture(sessions: &[(&str, Activity)]) -> Fixture {
    let folder = tempfile::tempdir().unwrap();
    let devices = Devices::open(&folder.path().join(FILE), || [7; 32]);
    Fixture {
        service: Service::new("Mac".to_owned(), devices),
        desk: FakeDesk::with(sessions),
        now: Instant::now(),
        _folder: folder,
    }
}

fn pair_message(code: &str, name: &str) -> String {
    format!(r#"{{"type":"pair","code":"{code}","name":"{name}"}}"#)
}

const WATCH_S1: &str = r#"{"type":"watch","session":"s1","columns":48,"lines":30}"#;

impl Fixture {
    fn ask(&mut self, link: u64, message: &str) -> Vec<Out> {
        self.service
            .message(link, message.as_bytes(), &self.desk, self.now)
    }

    /// Connects `PHONE` on `link` already paired and greeted.
    fn paired(&mut self, link: u64) {
        let code = self.service.offer(self.now);
        self.service.opened(link, PHONE);
        self.ask(link, &pair_message(&code, "Phone"));
        self.ask(link, r#"{"type":"hello"}"#);
    }
}

fn welcome() -> Tell {
    Tell::Welcome {
        host: "Mac".to_owned(),
    }
}

#[test]
fn a_stranger_is_refused_and_touches_nothing() {
    let mut fixture = fixture(&[("s1", Activity::Working)]);
    fixture.service.opened(1, STRANGER);
    for message in [
        r#"{"type":"hello"}"#,
        r#"{"type":"type","session":"s1","text":"rm -rf ~"}"#,
        WATCH_S1,
    ] {
        assert_eq!(fixture.ask(1, message), refused(1, Refusal::NotPaired));
    }
    assert!(fixture.desk.done().is_empty());
}

#[test]
fn the_code_on_screen_pairs_a_phone_once() {
    let mut fixture = fixture(&[]);
    let code = fixture.service.offer(fixture.now);
    let pair = pair_message(&code, "Tanner's phone");
    fixture.service.opened(1, PHONE);
    assert_eq!(fixture.ask(1, &pair), [Out::Tell(1, welcome())]);
    assert!(fixture.service.devices().is_paired(&PHONE));
    assert_eq!(fixture.service.devices().list()[0].name, "Tanner's phone");

    // The same code, read off the screen by someone else, no longer works.
    fixture.service.opened(2, STRANGER);
    assert_eq!(fixture.ask(2, &pair), refused(2, Refusal::WrongCode));
    assert!(!fixture.service.devices().is_paired(&STRANGER));
}

#[test]
fn a_wrong_old_or_withdrawn_code_pairs_nobody() {
    let mut fixture = fixture(&[]);
    fixture.service.opened(1, STRANGER);
    let guess = pair_message("guess", "x");
    assert_eq!(fixture.ask(1, &guess), refused(1, Refusal::WrongCode));

    let code = fixture.service.offer(fixture.now);
    fixture.now += crate::remote::pairing::OFFER_LASTS + Duration::from_secs(1);
    let late = pair_message(&code, "x");
    assert_eq!(fixture.ask(1, &late), refused(1, Refusal::WrongCode));

    let code = fixture.service.offer(fixture.now);
    fixture.service.withdraw_offer();
    let withdrawn = pair_message(&code, "x");
    assert_eq!(fixture.ask(1, &withdrawn), refused(1, Refusal::WrongCode));
    assert!(fixture.service.devices().list().is_empty());
}

#[test]
fn a_paired_phone_is_greeted_with_the_sessions_most_urgent_first() {
    let mut fixture = fixture(&[("idle", Activity::Idle), ("waiting", Activity::NeedsYou)]);
    let code = fixture.service.offer(fixture.now);
    fixture.service.opened(1, PHONE);
    fixture.ask(1, &pair_message(&code, "Phone"));
    let out = fixture.ask(1, r#"{"type":"hello"}"#);
    assert_eq!(out[0], Out::Tell(1, welcome()));
    let Out::Tell(1, Tell::Sessions { sessions }) = &out[1] else {
        panic!("the sessions follow the welcome");
    };
    let order: Vec<(&str, &str)> = sessions
        .iter()
        .map(|session| (session.id.as_str(), session.activity))
        .collect();
    assert_eq!(order, [("waiting", "needs-you"), ("idle", "idle")]);
}

#[test]
fn a_phone_that_reconnects_needs_no_code() {
    let mut fixture = fixture(&[]);
    fixture.paired(1);
    fixture.service.closed(1, &fixture.desk);
    fixture.service.opened(2, PHONE);
    let out = fixture.ask(2, r#"{"type":"hello"}"#);
    assert_eq!(out[0], Out::Tell(2, welcome()));
}

#[test]
fn watching_fits_the_terminal_to_the_phone_and_leaving_gives_it_back() {
    let mut fixture = fixture(&[("s1", Activity::Working)]);
    fixture.paired(1);
    let out = fixture.ask(
        1,
        r#"{"type":"watch","session":"s1","columns":48,"lines":2}"#,
    );
    assert_eq!(
        out,
        [Out::Tell(
            1,
            Tell::Screen {
                session: "s1".to_owned(),
                screen: screen_showing("s1 screen")
            }
        )]
    );
    fixture.ask(1, r#"{"type":"leave"}"#);
    // Two lines is fewer than a terminal can be, so the fewest is used.
    assert_eq!(fixture.desk.done(), ["fit s1 48x5", "fit s1 own size"]);
}

#[test]
fn a_dropped_link_gives_the_terminal_back_too() {
    let mut fixture = fixture(&[("s1", Activity::Working)]);
    fixture.paired(1);
    fixture.ask(1, WATCH_S1);
    fixture.service.closed(1, &fixture.desk);
    assert_eq!(fixture.desk.done().last().unwrap(), "fit s1 own size");
    assert!(!fixture.service.is_linked());
}

#[test]
fn typing_and_keys_reach_the_named_session() {
    let mut fixture = fixture(&[("s1", Activity::NeedsYou)]);
    fixture.paired(1);
    fixture.ask(1, r#"{"type":"type","session":"s1","text":"yes, go on"}"#);
    fixture.ask(1, r#"{"type":"press","session":"s1","key":"enter"}"#);
    fixture.ask(1, r#"{"type":"press","session":"s1","key":"interrupt"}"#);
    fixture.ask(1, r#"{"type":"press","session":"s1","key":"launch"}"#);
    assert_eq!(
        fixture.desk.done(),
        [
            "paste s1 yes, go on".to_owned(),
            format!("press s1 {:?}", Pressed::Key(Pressable::Return)),
            format!("press s1 {:?}", Pressed::Interrupt),
        ]
    );
}

#[test]
fn only_what_changed_is_sent_again() {
    let mut fixture = fixture(&[("s1", Activity::Working)]);
    fixture.paired(1);
    fixture.ask(1, WATCH_S1);
    assert_eq!(fixture.service.changes(&fixture.desk), []);

    fixture
        .desk
        .shows
        .borrow_mut()
        .insert("s1".to_owned(), "new output".to_owned());
    fixture.desk.running.borrow_mut()[0].activity = Activity::NeedsYou;
    let out = fixture.service.changes(&fixture.desk);
    assert!(matches!(&out[0], Out::Tell(1, Tell::Sessions { sessions })
        if sessions[0].activity == "needs-you"));
    assert!(matches!(&out[1], Out::Tell(1, Tell::Screen { screen, .. })
        if *screen == screen_showing("new output")));
    assert_eq!(fixture.service.changes(&fixture.desk), []);
}

#[test]
fn a_watched_session_that_ends_is_reported_gone_once() {
    let mut fixture = fixture(&[("s1", Activity::Working)]);
    fixture.paired(1);
    fixture.ask(1, WATCH_S1);
    fixture.desk.running.borrow_mut().clear();
    fixture.desk.shows.borrow_mut().clear();
    let out = fixture.service.changes(&fixture.desk);
    assert_eq!(
        out,
        [
            Out::Tell(1, Tell::Sessions { sessions: vec![] }),
            Out::Tell(
                1,
                Tell::Gone {
                    session: "s1".to_owned()
                }
            )
        ]
    );
    assert_eq!(fixture.service.changes(&fixture.desk), []);
}

#[test]
fn a_forgotten_phone_is_cut_off_and_must_pair_again() {
    let mut fixture = fixture(&[("s1", Activity::Working)]);
    fixture.paired(1);
    fixture.ask(1, WATCH_S1);
    let out = fixture.service.forget(&PHONE, &fixture.desk).unwrap();
    assert_eq!(out, [Out::Close(1)]);
    assert_eq!(fixture.desk.done().last().unwrap(), "fit s1 own size");

    fixture.service.opened(2, PHONE);
    assert_eq!(
        fixture.ask(2, r#"{"type":"hello"}"#),
        refused(2, Refusal::NotPaired)
    );
}

#[test]
fn a_message_that_can_t_be_read_ends_the_link() {
    let mut fixture = fixture(&[]);
    fixture.paired(1);
    assert_eq!(
        fixture.ask(1, "not json"),
        refused(1, Refusal::NotUnderstood)
    );
}
