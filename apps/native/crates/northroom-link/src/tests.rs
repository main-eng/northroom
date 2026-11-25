use super::*;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

/// A host with no relay and the address a device on the same machine dials it at.
fn local_host() -> (Host, Receiver<Event>, Address, Key) {
    let secret = Secret::generate();
    let key = secret.public();
    let (events, happened) = channel();
    let host = Host::start(secret, Relays::None, events).expect("the host starts");
    let mut address = host.address();
    // The host listens on every interface; dial it on this machine.
    address.direct = address
        .direct
        .iter()
        .map(|bound| SocketAddr::from(([127, 0, 0, 1], bound.port())))
        .collect();
    (host, happened, address, key)
}

#[test]
fn a_secret_survives_being_stored() {
    let secret = Secret::generate();
    assert_eq!(
        Secret::from_bytes(&secret.to_bytes()).public(),
        secret.public()
    );
}

#[test]
fn messages_cross_both_ways_and_the_host_learns_who_dialled() {
    let (host, happened, address, host_key) = local_host();
    assert_eq!(address.key, host_key);
    let phone = Secret::generate();
    let phone_key = phone.public();
    let client = Client::connect(phone, Relays::None, address).expect("the phone connects");

    client.send(b"hello".to_vec());
    let Event::Opened { link, peer } = happened.recv_timeout(WAIT).expect("opened") else {
        panic!("the first event is the link opening");
    };
    assert_eq!(peer, phone_key);
    assert_eq!(
        happened.recv_timeout(WAIT),
        Ok(Event::Message {
            link,
            bytes: b"hello".to_vec()
        })
    );

    host.send(link, b"welcome".to_vec());
    assert_eq!(client.receive(WAIT), Some(b"welcome".to_vec()));

    drop(client);
    assert_eq!(happened.recv_timeout(WAIT), Ok(Event::Closed { link }));
}

#[test]
fn a_closed_link_ends_for_the_device() {
    let (host, happened, address, _) = local_host();
    let client =
        Client::connect(Secret::generate(), Relays::None, address).expect("the phone connects");
    client.send(b"let me in".to_vec());
    let Event::Opened { link, .. } = happened.recv_timeout(WAIT).expect("opened") else {
        panic!("the first event is the link opening");
    };
    host.send(link, b"no".to_vec());
    host.close(link);
    assert_eq!(client.receive(WAIT), Some(b"no".to_vec()));
    assert_eq!(client.receive(WAIT), None);
}

#[test]
fn a_large_message_arrives_whole() {
    let (host, happened, address, _) = local_host();
    let client =
        Client::connect(Secret::generate(), Relays::None, address).expect("the phone connects");
    let large: Vec<u8> = (0..1_000_000u32).map(|n| n as u8).collect();
    client.send(large.clone());
    let Event::Opened { link, .. } = happened.recv_timeout(WAIT).expect("opened") else {
        panic!("the first event is the link opening");
    };
    assert_eq!(
        happened.recv_timeout(WAIT),
        Ok(Event::Message { link, bytes: large })
    );
    drop(host);
}

#[test]
fn a_relay_address_must_be_a_url() {
    let (events, _happened) = channel();
    let failed = Host::start(
        Secret::generate(),
        Relays::At("not a url".to_owned()),
        events,
    );
    assert!(failed.is_err());
}
