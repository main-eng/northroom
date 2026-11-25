//! The accepting end: a host takes links from any device that dials it and reports them. It
//! refuses nobody itself; the core closes the link of a device it has not paired.

use crate::{address_of, endpoint, error, runtime, wire, Address, Error, Key, Relays, Secret};
use iroh::endpoint::{Connection, Incoming, SendStream};
use iroh::Endpoint;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// What happened on a host's links, in order for each link.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// The device with key `peer` connected. Its identity is proven by the connection.
    Opened {
        link: u64,
        peer: Key,
    },
    Message {
        link: u64,
        bytes: Vec<u8>,
    },
    /// The link ended, from either side. Nothing follows for it.
    Closed {
        link: u64,
    },
}

enum Command {
    Send { link: u64, bytes: Vec<u8> },
    Close { link: u64 },
    Stop,
}

/// Each open link's outgoing messages.
type Links = Arc<Mutex<HashMap<u64, UnboundedSender<Vec<u8>>>>>;

/// A running host. Dropping it ends every link.
pub struct Host {
    endpoint: Endpoint,
    commands: UnboundedSender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Host {
    /// Starts accepting links as the device with `secret`, reporting them to `events`.
    pub fn start(secret: Secret, relays: Relays, events: Sender<Event>) -> Result<Self, Error> {
        let (commands, received) = unbounded_channel();
        let (started, start) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("northroom-link-host".to_owned())
            .spawn(move || run(secret, relays, events, received, started))
            .map_err(error)?;
        let endpoint = start.recv().map_err(error)??;
        Ok(Self {
            endpoint,
            commands,
            thread: Some(thread),
        })
    }

    /// Where the host can be reached now. The relay is `None` until the host has connected to
    /// one, a moment after it starts.
    pub fn address(&self) -> Address {
        address_of(&self.endpoint)
    }

    /// Sends `bytes` as one message on `link`. Nothing happens when the link has closed.
    pub fn send(&self, link: u64, bytes: Vec<u8>) {
        let _ = self.commands.send(Command::Send { link, bytes });
    }

    /// Ends `link`. Its `Closed` event follows.
    pub fn close(&self, link: u64) {
        let _ = self.commands.send(Command::Close { link });
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The host's thread: binds, hands the endpoint back through `started`, then serves until told
/// to stop.
fn run(
    secret: Secret,
    relays: Relays,
    events: Sender<Event>,
    commands: UnboundedReceiver<Command>,
    started: Sender<Result<Endpoint, Error>>,
) {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(failed) => {
            let _ = started.send(Err(failed));
            return;
        }
    };
    runtime.block_on(async {
        match endpoint(&secret, &relays, true).await {
            Ok(endpoint) => {
                let _ = started.send(Ok(endpoint.clone()));
                serve(&endpoint, events, commands).await;
                endpoint.close().await;
            }
            Err(failed) => {
                let _ = started.send(Err(failed));
            }
        }
    });
}

async fn serve(
    endpoint: &Endpoint,
    events: Sender<Event>,
    mut commands: UnboundedReceiver<Command>,
) {
    let links = Links::default();
    let mut next_link = 0u64;
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { return };
                next_link += 1;
                tokio::spawn(carry(incoming, next_link, links.clone(), events.clone()));
            }
            command = commands.recv() => {
                if !carry_out(command, &links) {
                    return;
                }
            }
        }
    }
}

/// Does what `command` asks. False when the host should stop.
fn carry_out(command: Option<Command>, links: &Links) -> bool {
    let mut links = links.lock().unwrap_or_else(PoisonError::into_inner);
    match command {
        Some(Command::Send { link, bytes }) => {
            if let Some(outgoing) = links.get(&link) {
                let _ = outgoing.send(bytes);
            }
            true
        }
        // Dropping the link's sender ends its writer, which closes the connection.
        Some(Command::Close { link }) => {
            links.remove(&link);
            true
        }
        Some(Command::Stop) | None => false,
    }
}

/// One link from its handshake to its end.
async fn carry(incoming: Incoming, link: u64, links: Links, events: Sender<Event>) {
    let Ok(connection) = incoming.await else {
        return;
    };
    let Ok((send, mut receive)) = connection.accept_bi().await else {
        return;
    };
    let (outgoing, queued) = unbounded_channel();
    links
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(link, outgoing);
    let peer = *connection.remote_id().as_bytes();
    let _ = events.send(Event::Opened { link, peer });
    let writer = tokio::spawn(write_queued(send, queued, connection.clone()));
    while let Some(bytes) = wire::read(&mut receive).await {
        let _ = events.send(Event::Message { link, bytes });
    }
    links
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&link);
    let _ = writer.await;
    let _ = events.send(Event::Closed { link });
}

/// Sends each queued message, then closes the connection once the queue's sender is dropped
/// (the link was closed) or a send fails.
async fn write_queued(
    mut send: SendStream,
    mut queued: UnboundedReceiver<Vec<u8>>,
    connection: Connection,
) {
    while let Some(bytes) = queued.recv().await {
        if !wire::write(&mut send, &bytes).await {
            break;
        }
    }
    let _ = send.finish();
    let _ = send.stopped().await;
    connection.close(0u32.into(), b"closed");
}
