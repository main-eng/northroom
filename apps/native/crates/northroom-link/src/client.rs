//! The dialling end: what a phone does, and what one computer does to reach another. A client
//! holds one link to one host.

use crate::{dialled, endpoint, error, runtime, wire, Address, Error, Relays, Secret, PROTOCOL};
use iroh::endpoint::{RecvStream, SendStream};
use iroh::Endpoint;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// How long dialling a host may take before it counts as out of reach.
const DIAL_MOST: Duration = Duration::from_secs(15);

/// What a wait for the host's next message ended with.
#[derive(Debug, PartialEq, Eq)]
pub enum Heard {
    Message(Vec<u8>),
    /// Nothing came in the time waited.
    Quiet,
    /// The link has closed; nothing more will come.
    Closed,
}

/// Sends messages over a client's link from any thread, for as long as the link is open.
#[derive(Clone)]
pub struct Sending(UnboundedSender<Vec<u8>>);

impl Sending {
    /// Sends `bytes` as one message. Nothing happens once the link has closed.
    pub fn send(&self, bytes: Vec<u8>) {
        let _ = self.0.send(bytes);
    }
}

/// An open link to a host. Dropping it closes the link.
pub struct Client {
    outgoing: Option<UnboundedSender<Vec<u8>>>,
    incoming: Receiver<Vec<u8>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Client {
    /// Dials the host at `address` as the device with `secret`.
    pub fn connect(secret: Secret, relays: Relays, address: Address) -> Result<Self, Error> {
        let (outgoing, queued) = unbounded_channel();
        let (received, incoming) = std::sync::mpsc::channel();
        let (started, start) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("northroom-link-client".to_owned())
            .spawn(move || run(secret, relays, address, queued, received, started))
            .map_err(error)?;
        start.recv().map_err(error)??;
        Ok(Self {
            outgoing: Some(outgoing),
            incoming,
            thread: Some(thread),
        })
    }

    /// Sends `bytes` as one message.
    pub fn send(&self, bytes: Vec<u8>) {
        if let Some(outgoing) = &self.outgoing {
            let _ = outgoing.send(bytes);
        }
    }

    /// The host's next message, waiting up to `most`. `None` when none came in time or the
    /// link has closed.
    pub fn receive(&self, most: Duration) -> Option<Vec<u8>> {
        self.incoming.recv_timeout(most).ok()
    }

    /// The host's next message, waiting up to `most`, or why there is none.
    pub fn hear(&self, most: Duration) -> Heard {
        match self.incoming.recv_timeout(most) {
            Ok(bytes) => Heard::Message(bytes),
            Err(RecvTimeoutError::Timeout) => Heard::Quiet,
            Err(RecvTimeoutError::Disconnected) => Heard::Closed,
        }
    }

    /// A way to send over this link from other threads.
    pub fn sending(&self) -> Option<Sending> {
        self.outgoing.clone().map(Sending)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.outgoing = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    secret: Secret,
    relays: Relays,
    address: Address,
    queued: UnboundedReceiver<Vec<u8>>,
    received: Sender<Vec<u8>>,
    started: Sender<Result<(), Error>>,
) {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(failed) => {
            let _ = started.send(Err(failed));
            return;
        }
    };
    runtime.block_on(async {
        match open(&secret, &relays, &address).await {
            Ok((endpoint, send, receive)) => {
                let _ = started.send(Ok(()));
                carry(send, receive, queued, received).await;
                endpoint.close().await;
            }
            Err(failed) => {
                let _ = started.send(Err(failed));
            }
        }
    });
}

async fn open(
    secret: &Secret,
    relays: &Relays,
    address: &Address,
) -> Result<(Endpoint, SendStream, RecvStream), Error> {
    let endpoint = endpoint(secret, relays, false).await?;
    let dial = async {
        let connection = endpoint
            .connect(dialled(address)?, PROTOCOL)
            .await
            .map_err(error)?;
        connection.open_bi().await.map_err(error)
    };
    let (send, receive) = tokio::time::timeout(DIAL_MOST, dial)
        .await
        .map_err(|_| Error("the host did not answer".to_owned()))??;
    Ok((endpoint, send, receive))
}

/// Sends what is queued and reports what arrives until either side ends the link.
async fn carry(
    mut send: SendStream,
    mut receive: RecvStream,
    mut queued: UnboundedReceiver<Vec<u8>>,
    received: Sender<Vec<u8>>,
) {
    loop {
        tokio::select! {
            bytes = queued.recv() => {
                let Some(bytes) = bytes else { break };
                if !wire::write(&mut send, &bytes).await {
                    break;
                }
            }
            bytes = wire::read(&mut receive) => {
                let Some(bytes) = bytes else { break };
                if received.send(bytes).is_err() {
                    break;
                }
            }
        }
    }
    let _ = send.finish();
}
