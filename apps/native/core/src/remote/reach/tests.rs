use super::*;
use crate::remote::run::tests::OneTerminal;
use crate::remote::run::Link;
use std::net::SocketAddr;
use std::time::Instant;

const WAIT: Duration = Duration::from_secs(20);

/// `link` with the computer's addresses swapped for this machine's own, where the test's host
/// listens.
fn local(link: &str) -> String {
    let mut invite = Invite::parse(link).expect("the link reads back");
    invite.direct = invite
        .direct
        .iter()
        .map(|bound| SocketAddr::from(([127, 0, 0, 1], bound.port())))
        .collect();
    invite.link()
}

fn quiet() -> Events {
    Events {
        changed: Box::new(|| {}),
        started: Box::new(|_, _| {}),
    }
}

/// Waits until `look` finds something.
fn found<T>(what: &str, look: impl Fn() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(found) = look() {
            return found;
        }
        assert!(started.elapsed() < WAIT, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn connected(reach: &Reach) -> Seen {
    found("the computer is connected and lists a session", || {
        reach
            .computers()
            .into_iter()
            .find(|seen| seen.standing == Standing::Connected && !seen.sessions.is_empty())
    })
}

fn view() -> ViewSize {
    ViewSize {
        width: 800,
        height: 240,
        cell: crate::terminal::frame::CellMetrics {
            width: 16.0,
            height: 30.0,
            baseline: 22.0,
            stroke: 1.0,
        },
    }
}

fn palette() -> Palette {
    Palette::on([17, 17, 17], [238, 238, 240], [60, 60, 90])
}

#[test]
fn a_computer_pairs_with_another_shows_its_session_and_is_kept_until_forgotten() {
    let (there, here) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let desk = OneTerminal::default();
    let link = Link::start(
        there.path(),
        Relays::None,
        "Desktop".to_owned(),
        desk.clone(),
    )
    .unwrap();
    let reach = Reach::open(here.path(), "Laptop".to_owned(), quiet());
    assert!(!reach.pair("not a pairing link"));
    assert!(reach.computers().is_empty());
    assert!(reach.pair(&local(&link.invite())));

    // The other computer's name and sessions are listed here, and this one's name there.
    let seen = connected(&reach);
    assert_eq!(seen.name, "Desktop");
    assert_eq!(
        (seen.sessions[0].id.as_str(), seen.sessions[0].name.as_str()),
        ("s1", "Build")
    );
    assert_eq!(link.devices()[0].1, "Laptop");

    // A terminal here starts from the session's state, shows what it writes, and types there.
    let terminal = reach.show(&seen.key, "s1", view(), palette()).unwrap();
    found("the terminal shows the session's state", || {
        terminal.transcript().contains("state").then_some(())
    });
    assert_eq!(*desk.fitted.relock(), ["s1 50x8"]);
    desk.writes(b", then more");
    found("the terminal shows new output", || {
        terminal
            .transcript()
            .contains("state, then more")
            .then_some(())
    });
    terminal.input(b"ls\r");
    found("what was typed arrived", || {
        desk.typed
            .relock()
            .contains(&"ls\r".to_owned())
            .then_some(())
    });

    // The pairing is kept: opened again, the computer is reached with no code.
    drop(reach);
    assert!(Reach::keeps_any(here.path()));
    let reach = Reach::open(here.path(), "Laptop".to_owned(), quiet());
    assert_eq!(connected(&reach).name, "Desktop");

    // Forgotten here, it is neither listed nor kept.
    reach.forget(&seen.key);
    assert!(reach.computers().is_empty() && !Reach::keeps_any(here.path()));
}

#[test]
fn a_computer_that_forgets_this_one_or_never_took_its_code_is_not_kept() {
    let (there, here) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let desk = OneTerminal::default();
    let link = Link::start(
        there.path(),
        Relays::None,
        "Desktop".to_owned(),
        desk.clone(),
    )
    .unwrap();
    let reach = Reach::open(here.path(), "Laptop".to_owned(), quiet());

    // A code the computer has taken off its screen pairs nobody.
    let stale = local(&link.invite());
    link.withdraw_invite();
    assert!(reach.pair(&stale));
    found("the refused computer is unlisted", || {
        reach.computers().is_empty().then_some(())
    });
    assert!(!Reach::keeps_any(here.path()));

    // Paired, then forgotten on the other computer: this one is turned away and lets go.
    assert!(reach.pair(&local(&link.invite())));
    connected(&reach);
    let this = crate::remote::devices::parse_key_hex(&link.devices()[0].0).unwrap();
    link.forget(&this, &desk).unwrap();
    found("the computer that forgot this one is unlisted", || {
        reach.computers().is_empty().then_some(())
    });
    assert!(!Reach::keeps_any(here.path()));
}
