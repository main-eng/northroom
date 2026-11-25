//! The link between a Northroom host and a paired device, such as a phone. Each side has a key
//! pair and is known by its public key; messages travel directly between the two where their
//! networks allow it and through a relay where they don't, encrypted end to end either way.
//!
//! This crate is the one place that knows the networking library, as `terminal::engine` is for
//! the emulator: it carries bytes and decides nothing. Who may connect and what a message means
//! are the core's rules (`northroom_core::remote`). A link is one connection carrying messages
//! both ways, each sent as its length (four bytes, big-endian) and then its bytes.

mod client;
mod host;
mod wire;

pub use client::Client;
pub use host::{Event, Host};

use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, PublicKey, RelayMap, RelayMode, RelayUrl, SecretKey};
use std::net::SocketAddr;

/// The protocol both sides must name to connect. A change that an older side can't read takes
/// the next number.
pub const PROTOCOL: &[u8] = b"northroom/1";

/// The longest message a side accepts. A longer one ends the link.
pub const MOST_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// A device's public key: how the other side names it and whom it encrypts to.
pub type Key = [u8; 32];

/// A device's private key. It never leaves the device.
#[derive(Clone)]
pub struct Secret(SecretKey);

impl Secret {
    pub fn generate() -> Self {
        Self(SecretKey::generate())
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(SecretKey::from_bytes(bytes))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn public(&self) -> Key {
        *self.0.public().as_bytes()
    }
}

/// Which relays carry a link when the two devices can't reach each other directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Relays {
    /// The networking library's public relays.
    Public,
    /// The relay at this address, such as one the person runs.
    At(String),
    /// None: the devices must reach each other directly, as on one network.
    None,
}

/// Where a host can be reached: its key, the relay it listens at and the addresses it has on
/// its own networks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub key: Key,
    pub relay: Option<String>,
    pub direct: Vec<SocketAddr>,
}

/// Why a link could not start or connect, in words for a log.
#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn error(what: impl std::fmt::Display) -> Error {
    Error(what.to_string())
}

fn relay_mode(relays: &Relays) -> Result<RelayMode, Error> {
    Ok(match relays {
        Relays::Public => RelayMode::Default,
        Relays::None => RelayMode::Disabled,
        Relays::At(address) => {
            let url: RelayUrl = address.parse().map_err(error)?;
            RelayMode::Custom(RelayMap::from_iter([url]))
        }
    })
}

/// A device's end of every link it makes or accepts. No address is published or looked up
/// anywhere: a device is dialled by an `Address` it was handed.
async fn endpoint(secret: &Secret, relays: &Relays, accepts: bool) -> Result<Endpoint, Error> {
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret.0.clone())
        .relay_mode(relay_mode(relays)?);
    if accepts {
        builder = builder.alpns(vec![PROTOCOL.to_vec()]);
    }
    builder.bind().await.map_err(error)
}

fn address_of(endpoint: &Endpoint) -> Address {
    let address = endpoint.addr();
    let relay = address.relay_urls().next().map(ToString::to_string);
    let direct = address.ip_addrs().copied().collect();
    Address {
        key: *address.id.as_bytes(),
        relay,
        direct,
    }
}

fn dialled(address: &Address) -> Result<EndpointAddr, Error> {
    let key = PublicKey::from_bytes(&address.key).map_err(error)?;
    let mut dialled = EndpointAddr::new(key);
    if let Some(relay) = &address.relay {
        dialled = dialled.with_relay_url(relay.parse().map_err(error)?);
    }
    for direct in &address.direct {
        dialled = dialled.with_ip_addr(*direct);
    }
    Ok(dialled)
}

/// A runtime of one thread, for the thread a host or client runs on.
fn runtime() -> Result<tokio::runtime::Runtime, Error> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(error)
}

#[cfg(test)]
mod tests;
