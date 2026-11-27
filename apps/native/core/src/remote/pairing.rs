//! Pairing a phone: this computer shows a link as a QR code, the phone reads it and connects
//! with the code inside. The link says where this computer is and carries a code that works
//! once, for a few minutes, so reading the screen is what proves the phone is the person's.

use crate::remote::devices::{key_hex, parse_key_hex, Key};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// How long a shown code can be used.
pub const OFFER_LASTS: Duration = Duration::from_secs(5 * 60);
/// The most of this computer's own addresses a link names, to keep its QR code coarse enough
/// to read from a screen.
const DIRECT_MOST: usize = 4;
const START: &str = "northroom://pair?";

/// A code on screen, waiting for a phone to use it.
pub struct Offer {
    code: String,
    made: Instant,
}

impl Offer {
    /// A fresh code: 122 random bits, as 32 hex digits.
    pub fn new(now: Instant) -> Self {
        Self {
            code: uuid::Uuid::new_v4().simple().to_string(),
            made: now,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    /// Whether `code` is this offer's and it is still fresh at `now`.
    pub fn accepts(&self, code: &str, now: Instant) -> bool {
        code == self.code && now.duration_since(self.made) <= OFFER_LASTS
    }

    /// The share of its time this offer still has at `now`: 1 when made, 0 once it is stale.
    pub fn share_left(&self, now: Instant) -> f64 {
        let spent = now.duration_since(self.made).as_secs_f64() / OFFER_LASTS.as_secs_f64();
        (1.0 - spent).clamp(0.0, 1.0)
    }
}

/// What a pairing link says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// This computer's public key.
    pub key: Key,
    pub code: String,
    /// This computer's name, to show on the phone.
    pub host: String,
    /// The relay this computer listens at, when it has one.
    pub relay: Option<String>,
    /// This computer's addresses on its own networks, for a phone on one of them.
    pub direct: Vec<SocketAddr>,
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// `text` as a URL query value.
fn escaped(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if is_unreserved(byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// A URL query value back as text. `None` when an escape is cut short or not UTF-8.
fn unescaped(value: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut rest = value.as_bytes();
    while let Some((&first, after)) = rest.split_first() {
        if first == b'%' {
            let digits = std::str::from_utf8(after.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(digits, 16).ok()?);
            rest = &after[2..];
        } else {
            bytes.push(first);
            rest = after;
        }
    }
    String::from_utf8(bytes).ok()
}

/// The addresses a link names: IPv4 first, since a phone beside this computer shares its
/// network, and at most `DIRECT_MOST`.
fn named_addresses(direct: &[SocketAddr]) -> Vec<SocketAddr> {
    let (v4, v6): (Vec<SocketAddr>, Vec<SocketAddr>) =
        direct.iter().partition(|address| address.is_ipv4());
    v4.into_iter().chain(v6).take(DIRECT_MOST).collect()
}

impl Invite {
    /// The link a phone reads: `northroom://pair?key=…&code=…&name=…`, then `relay=…` when
    /// there is one and an `at=…` for each address.
    pub fn link(&self) -> String {
        let mut link = format!(
            "{START}key={}&code={}&name={}",
            key_hex(&self.key),
            escaped(&self.code),
            escaped(&self.host)
        );
        if let Some(relay) = &self.relay {
            link.push_str(&format!("&relay={}", escaped(relay)));
        }
        for address in named_addresses(&self.direct) {
            link.push_str(&format!("&at={}", escaped(&address.to_string())));
        }
        link
    }

    /// Reads a link written by `link`. `None` when it is not one or lacks the key or code.
    pub fn parse(link: &str) -> Option<Self> {
        let mut invite = Self {
            key: [0; 32],
            code: String::new(),
            host: String::new(),
            relay: None,
            direct: Vec::new(),
        };
        let mut keyed = false;
        for pair in link.strip_prefix(START)?.split('&') {
            let (name, value) = pair.split_once('=')?;
            let value = unescaped(value)?;
            keyed |= name == "key";
            invite.take(name, value)?;
        }
        (keyed && !invite.code.is_empty()).then_some(invite)
    }

    /// Takes one of a link's values. `None` when the value can't be read; a name this version
    /// doesn't know is skipped, so a later version can add one.
    fn take(&mut self, name: &str, value: String) -> Option<()> {
        match name {
            "key" => self.key = parse_key_hex(&value)?,
            "code" => self.code = value,
            "name" => self.host = value,
            "relay" => self.relay = Some(value),
            "at" => self.direct.push(value.parse().ok()?),
            _ => {}
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> Invite {
        Invite {
            key: [0xab; 32],
            code: "c0de".to_owned(),
            host: "Tanner's Mac & more".to_owned(),
            relay: Some("https://relay.example.com/".to_owned()),
            direct: vec![
                "[2600::1]:4433".parse().unwrap(),
                "192.168.1.65:4433".parse().unwrap(),
            ],
        }
    }

    #[test]
    fn a_link_reads_back_with_the_nearest_address_first() {
        let link = invite().link();
        assert!(link.starts_with("northroom://pair?key=abab"));
        assert!(link.contains("&name=Tanner%27s%20Mac%20%26%20more&"));
        let mut read = Invite::parse(&link).unwrap();
        assert_eq!(read.direct[0], "192.168.1.65:4433".parse().unwrap());
        read.direct.reverse();
        assert_eq!(read, invite());
    }

    #[test]
    fn a_link_names_only_a_few_addresses() {
        let mut long = invite();
        long.direct = (1..=9)
            .map(|port| SocketAddr::from(([10, 0, 0, 1], port)))
            .collect();
        assert_eq!(
            Invite::parse(&long.link()).unwrap().direct.len(),
            DIRECT_MOST
        );
    }

    #[test]
    fn a_link_from_a_later_version_still_reads() {
        let link = format!("{}&colour=blue", invite().link());
        assert_eq!(Invite::parse(&link).unwrap().code, "c0de");
    }

    #[test]
    fn what_is_not_a_pairing_link_is_refused() {
        let key = "ab".repeat(32);
        assert_eq!(Invite::parse("https://example.com/?key=1"), None);
        assert_eq!(Invite::parse("northroom://pair?code=c0de"), None);
        assert_eq!(Invite::parse(&format!("northroom://pair?key={key}")), None);
        assert_eq!(
            Invite::parse(&format!("northroom://pair?key={key}&code=c0de&at=nowhere")),
            None
        );
        assert_eq!(
            Invite::parse(&format!("northroom://pair?key={key}&code=%4")),
            None
        );
    }

    #[test]
    fn a_code_works_only_while_fresh_and_only_as_shown() {
        let made = Instant::now();
        let offer = Offer::new(made);
        assert_eq!(offer.code().len(), 32);
        assert!(offer.accepts(offer.code(), made + OFFER_LASTS));
        assert!(!offer.accepts(offer.code(), made + OFFER_LASTS + Duration::from_secs(1)));
        // The share of its time left runs from all of it to none.
        assert_eq!(offer.share_left(made), 1.0);
        assert_eq!(offer.share_left(made + OFFER_LASTS / 2), 0.5);
        assert_eq!(offer.share_left(made + OFFER_LASTS * 2), 0.0);
        assert!(!offer.accepts("guess", made));
        assert_ne!(Offer::new(made).code(), offer.code());
    }
}
