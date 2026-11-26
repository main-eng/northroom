use super::*;
use crate::remote::devices::FILE;
use crate::remote::pages::{Drawn, Pages};
use crate::remote::protocol::{Item, Piece};
use crate::remote::tables::Shown;
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

    fn start(&self) -> Option<String> {
        let id = format!("s{}", self.running.borrow().len() + 1);
        self.running.borrow_mut().push(Running {
            id: id.clone(),
            name: "Terminal".to_owned(),
            folder: "Work".to_owned(),
            doing: String::new(),
            agent: String::new(),
            activity: Activity::Idle,
        });
        Some(id)
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

    fn keys(&self, session: &str, text: &str) {
        self.done
            .borrow_mut()
            .push(format!("keys {session} {text}"));
    }

    /// The page `p1` holds "hello, phone", and picture 0 of the document `d1` holds "png".
    fn read(&self, file: &str, picture: Option<usize>, from: u64, length: usize) -> Option<Piece> {
        let held: &[u8] = match (file, picture) {
            ("p1", None) => b"hello, phone",
            ("d1", Some(0)) => b"png",
            _ => return None,
        };
        let start = usize::try_from(from).ok()?.min(held.len());
        let end = start.saturating_add(length).min(held.len());
        Some(Piece {
            size: held.len() as u64,
            bytes: held[start..end].to_vec(),
        })
    }

    /// The table `t1` has one sheet of two columns and 500 rows.
    fn table(&self, file: &str) -> Option<Vec<Head>> {
        (file == "t1").then(|| {
            vec![Head {
                name: "Prices".to_owned(),
                columns: vec!["A".to_owned(), "B".to_owned()],
                widths: vec![120.0, 56.0],
                rows: 500,
            }]
        })
    }

    /// Each row of `t1` holds its own number.
    fn rows(&self, file: &str, _sheet: usize, from: usize, count: usize) -> Option<Vec<Row>> {
        (file == "t1").then(|| {
            (from..(from + count).min(500))
                .map(|row| {
                    vec![Some(Shown {
                        text: row.to_string(),
                        align: None,
                        bold: false,
                    })]
                })
                .collect()
        })
    }

    /// The document `d1` has two pages.
    fn pages(&self, file: &str) -> Option<Pages> {
        (file == "d1").then_some(Pages {
            width: 612.0,
            height: 792.0,
            count: 2,
        })
    }

    /// Each page of `d1` draws one picture as wide as the page.
    fn marks(&self, file: &str, _page: usize, width: f64) -> Option<Vec<Drawn>> {
        (file == "d1").then(|| {
            vec![Drawn::Picture {
                frame: [0.0, 0.0, width, width],
                picture: 0,
                crop: [0.0; 4],
            }]
        })
    }

    /// The top of the drive holds one folder, `f1`, which holds one page.
    fn folder(&self, folder: Option<&str>) -> Option<Listing> {
        let item = |id: &str, name: &str, kind, size| Item {
            id: id.to_owned(),
            name: name.to_owned(),
            kind,
            size,
            extension: None,
        };
        match folder {
            None => Some(Listing {
                name: "Drive".to_owned(),
                items: vec![item("f1", "Work", "folder", None)],
            }),
            Some("f1") => Some(Listing {
                name: "Work".to_owned(),
                items: vec![item("p1", "Notes", "page", Some(12))],
            }),
            Some(_) => None,
        }
    }

    fn press(&self, session: &str, pressed: Pressed) {
        self.done
            .borrow_mut()
            .push(format!("press {session} {pressed:?}"));
    }

    fn scroll(&self, session: &str, lines: i32) {
        self.done
            .borrow_mut()
            .push(format!("scroll {session} {lines}"));
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
fn scrolling_moves_the_named_session_by_at_most_a_message_s_limit() {
    let mut fixture = fixture(&[("s1", Activity::Working)]);
    fixture.paired(1);
    fixture.ask(1, r#"{"type":"scroll","session":"s1","lines":12}"#);
    fixture.ask(1, r#"{"type":"scroll","session":"s1","lines":-100000}"#);
    assert_eq!(fixture.desk.done(), ["scroll s1 12", "scroll s1 -500"]);
}

#[test]
fn a_started_session_is_listed_before_the_device_is_told_which_it_is() {
    let mut fixture = fixture(&[("s1", Activity::Working)]);
    fixture.paired(1);
    let out = fixture.ask(1, r#"{"type":"start"}"#);
    let [Out::Tell(1, Tell::Sessions { sessions }), Out::Tell(1, Tell::Started { session })] =
        out.as_slice()
    else {
        panic!("expected the sessions, then the started one: {out:?}");
    };
    assert_eq!(session, "s2");
    assert!(sessions.iter().any(|listed| listed.id == "s2"));
}

#[test]
fn keys_are_typed_not_pasted() {
    let mut fixture = fixture(&[("s1", Activity::Idle)]);
    fixture.paired(1);
    fixture.ask(1, r#"{"type":"keys","session":"s1","text":"ls"}"#);
    assert_eq!(fixture.desk.done(), ["keys s1 ls"]);
}

#[test]
fn a_folder_is_listed_from_the_top_of_the_drive_down() {
    let mut fixture = fixture(&[]);
    fixture.paired(1);
    let told = |out: Vec<Out>| match out.as_slice() {
        [Out::Tell(1, tell)] => String::from_utf8(tell.encoded()).unwrap(),
        other => panic!("expected one message: {other:?}"),
    };
    assert_eq!(
        told(fixture.ask(1, r#"{"type":"list"}"#)),
        r#"{"type":"folder","name":"Drive","items":[{"id":"f1","name":"Work","kind":"folder"}]}"#
    );
    assert_eq!(
        told(fixture.ask(1, r#"{"type":"list","folder":"f1"}"#)),
        r#"{"type":"folder","folder":"f1","name":"Work","items":[{"id":"p1","name":"Notes","kind":"page","size":12}]}"#
    );
    assert_eq!(
        told(fixture.ask(1, r#"{"type":"list","folder":"gone"}"#)),
        r#"{"type":"no-folder","folder":"gone"}"#
    );
}

#[test]
fn a_file_is_read_a_piece_at_a_time_as_base64() {
    let mut fixture = fixture(&[]);
    fixture.paired(1);
    let told = |out: Vec<Out>| match out.as_slice() {
        [Out::Tell(1, tell)] => String::from_utf8(tell.encoded()).unwrap(),
        other => panic!("expected one message: {other:?}"),
    };
    assert_eq!(
        told(fixture.ask(1, r#"{"type":"read","file":"p1","length":5}"#)),
        r#"{"type":"bytes","file":"p1","from":0,"size":12,"data":"aGVsbG8="}"#
    );
    assert_eq!(
        told(fixture.ask(
            1,
            r#"{"type":"read","file":"p1","from":7,"length":99999999}"#
        )),
        r#"{"type":"bytes","file":"p1","from":7,"size":12,"data":"cGhvbmU="}"#
    );
    assert_eq!(
        told(fixture.ask(1, r#"{"type":"read","file":"nope","length":5}"#)),
        r#"{"type":"no-file","file":"nope"}"#
    );
}

#[test]
fn a_document_is_its_pages_their_marks_and_its_pictures() {
    let mut fixture = fixture(&[]);
    fixture.paired(1);
    let told = |out: Vec<Out>| match out.as_slice() {
        [Out::Tell(1, tell)] => String::from_utf8(tell.encoded()).unwrap(),
        other => panic!("expected one message: {other:?}"),
    };
    assert_eq!(
        told(fixture.ask(1, r#"{"type":"pages","file":"d1"}"#)),
        r#"{"type":"pages","file":"d1","width":612.0,"height":792.0,"count":2}"#
    );
    // A width no page can be is held to one that can.
    assert_eq!(
        told(fixture.ask(1, r#"{"type":"page","file":"d1","page":1,"width":5}"#)),
        concat!(
            r#"{"type":"marks","file":"d1","page":1,"width":100.0,"marks":"#,
            r#"[{"mark":"picture","frame":[0.0,0.0,100.0,100.0],"picture":0,"crop":[0.0,0.0,0.0,0.0]}]}"#
        )
    );
    assert_eq!(
        told(fixture.ask(1, r#"{"type":"read","file":"d1","picture":0,"length":9}"#)),
        r#"{"type":"bytes","file":"d1","picture":0,"from":0,"size":3,"data":"cG5n"}"#
    );
    assert_eq!(
        told(fixture.ask(1, r#"{"type":"pages","file":"p1"}"#)),
        r#"{"type":"no-pages","file":"p1"}"#
    );
}

#[test]
fn a_table_is_its_sheets_and_a_window_of_rows_no_longer_than_a_message_holds() {
    let mut fixture = fixture(&[]);
    fixture.paired(1);
    let out = fixture.ask(1, r#"{"type":"table","file":"t1"}"#);
    assert_eq!(
        String::from_utf8(match &out[..] {
            [Out::Tell(1, tell)] => tell.encoded(),
            other => panic!("expected one message: {other:?}"),
        })
        .unwrap(),
        r#"{"type":"table","file":"t1","sheets":[{"name":"Prices","columns":["A","B"],"widths":[120.0,56.0],"rows":500}]}"#
    );
    let out = fixture.ask(
        1,
        r#"{"type":"rows","file":"t1","sheet":0,"from":10,"count":100000}"#,
    );
    let [Out::Tell(1, Tell::Rows { from: 10, rows, .. })] = &out[..] else {
        panic!("expected rows: {out:?}");
    };
    assert_eq!(rows.len(), ROWS_MOST);
    assert_eq!(
        rows[0][0].as_ref().map(|cell| cell.text.as_str()),
        Some("10")
    );
    let out = fixture.ask(1, r#"{"type":"table","file":"p1"}"#);
    assert_eq!(
        out,
        [Out::Tell(
            1,
            Tell::NoTable {
                file: "p1".to_owned()
            }
        )]
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
