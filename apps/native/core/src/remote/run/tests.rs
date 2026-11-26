use super::*;
use crate::remote::pages::{Drawn, Pages};
use crate::remote::protocol::{Listing, Piece};
use crate::remote::screen::Run;
use crate::terminal::activity::Activity;
use northroom_link::{Address, Client};
use std::net::SocketAddr;

const WAIT: Duration = Duration::from_secs(10);

/// One terminal whose screen shows whatever was last typed into it.
#[derive(Clone, Default)]
struct OneTerminal {
    typed: Arc<Mutex<Vec<String>>>,
}

impl Desk for OneTerminal {
    fn sessions(&self) -> Vec<Running> {
        vec![Running {
            id: "s1".to_owned(),
            name: "Build".to_owned(),
            folder: "Work".to_owned(),
            doing: String::new(),
            agent: "Claude Code".to_owned(),
            activity: Activity::NeedsYou,
        }]
    }

    fn screen(&self, _session: &str) -> Option<Screen> {
        Some(Screen {
            columns: 48,
            lines: 30,
            ink: "#ffffff".to_owned(),
            ground: "#000000".to_owned(),
            rows: vec![vec![Run {
                text: self.typed.relock().join(" "),
                ..Default::default()
            }]],
            cursor: None,
        })
    }

    fn fit(&self, _session: &str, _grid: Option<GridSize>) {}

    fn paste(&self, _session: &str, text: &str) {
        self.typed.relock().push(text.to_owned());
    }

    fn press(&self, _session: &str, pressed: Pressed) {
        self.typed.relock().push(format!("{pressed:?}"));
    }

    fn scroll(&self, _session: &str, _lines: i32) {}

    fn start(&self) -> Option<String> {
        None
    }

    fn keys(&self, _session: &str, text: &str) {
        self.typed.relock().push(text.to_owned());
    }

    fn folder(&self, _folder: Option<&str>) -> Option<Listing> {
        None
    }

    fn read(
        &self,
        _file: &str,
        _picture: Option<usize>,
        _from: u64,
        _length: usize,
    ) -> Option<Piece> {
        None
    }

    fn pages(&self, _file: &str) -> Option<Pages> {
        None
    }

    fn marks(&self, _file: &str, _page: usize, _width: f64) -> Option<Vec<Drawn>> {
        None
    }
}

/// A phone that read `invite` off the screen, dialling this machine.
fn phone(invite: &Invite, secret: Secret) -> Client {
    let direct = invite
        .direct
        .iter()
        .map(|bound| SocketAddr::from(([127, 0, 0, 1], bound.port())))
        .collect();
    let address = Address {
        key: invite.key,
        relay: None,
        direct,
    };
    Client::connect(secret, Relays::None, address).expect("the phone connects")
}

fn told(client: &Client) -> serde_json::Value {
    let bytes = client.receive(WAIT).expect("a message in time");
    serde_json::from_slice(&bytes).expect("JSON")
}

fn ask(client: &Client, message: &str) {
    client.send(message.as_bytes().to_vec());
}

#[test]
fn a_phone_pairs_from_the_link_then_watches_and_types_over_a_real_connection() {
    let folder = tempfile::tempdir().unwrap();
    let desk = OneTerminal::default();
    let link = Link::start(folder.path(), Relays::None, "Mac".to_owned(), desk.clone()).unwrap();
    let invite = Invite::parse(&link.invite()).expect("the link reads back");
    assert_eq!(invite.host, "Mac");

    let secret = Secret::generate();
    let client = phone(&invite, secret.clone());
    ask(
        &client,
        &format!(
            r#"{{"type":"pair","code":"{}","name":"Test phone"}}"#,
            invite.code
        ),
    );
    assert_eq!(told(&client)["type"], "welcome");
    assert_eq!(link.devices()[0].1, "Test phone");

    ask(&client, r#"{"type":"hello"}"#);
    assert_eq!(told(&client)["host"], "Mac");
    let sessions = told(&client);
    assert_eq!(sessions["sessions"][0]["activity"], "needs-you");

    ask(
        &client,
        r#"{"type":"watch","session":"s1","columns":48,"lines":30}"#,
    );
    assert_eq!(told(&client)["screen"]["rows"][0][0]["text"], "");

    // What the phone types changes the screen, and the change is sent without being asked for.
    ask(&client, r#"{"type":"type","session":"s1","text":"yes"}"#);
    ask(&client, r#"{"type":"press","session":"s1","key":"enter"}"#);
    let mut shown = String::new();
    while shown != "yes Key(Return)" {
        shown = told(&client)["screen"]["rows"][0][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
    }

    // The phone comes back later and is let in by its key alone.
    drop(client);
    let again = phone(&invite, secret);
    ask(&again, r#"{"type":"hello"}"#);
    assert_eq!(told(&again)["type"], "welcome");
}

#[test]
fn a_device_without_the_code_is_turned_away_over_a_real_connection() {
    let folder = tempfile::tempdir().unwrap();
    let desk = OneTerminal::default();
    let link = Link::start(folder.path(), Relays::None, "Mac".to_owned(), desk.clone()).unwrap();
    let invite = Invite::parse(&link.invite()).unwrap();

    let stranger = phone(&invite, Secret::generate());
    ask(
        &stranger,
        r#"{"type":"type","session":"s1","text":"rm -rf ~"}"#,
    );
    let refused = told(&stranger);
    assert_eq!(
        (refused["type"].as_str(), refused["why"].as_str()),
        (Some("refused"), Some("not-paired"))
    );
    assert_eq!(stranger.receive(WAIT), None);
    assert!(desk.typed.relock().is_empty());
    assert!(link.devices().is_empty());
}

#[test]
fn the_computer_has_a_name() {
    assert!(!computer_name().is_empty());
    assert!(!computer_name().contains('.'));
}

#[test]
fn the_settings_file_chooses_the_relay() {
    let settings = |text: &str| match serde_json::from_str(text).expect("JSON") {
        serde_json::Value::Object(settings) => settings,
        _ => panic!("settings are an object"),
    };
    assert_eq!(relays_of(&settings("{}")), Relays::Public);
    assert_eq!(
        relays_of(&settings(r#"{"phone_relay": 5}"#)),
        Relays::Public
    );
    assert_eq!(
        relays_of(&settings(r#"{"phone_relay": "none"}"#)),
        Relays::None
    );
    assert_eq!(
        relays_of(&settings(
            r#"{"phone_relay": " https://relay.example.com "}"#
        )),
        Relays::At("https://relay.example.com".to_owned())
    );
}
