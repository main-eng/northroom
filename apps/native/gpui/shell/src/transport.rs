//! Sends the core's requests (`core/src/transport.rs`) exactly as given, as `GET`, and streams
//! back the status, the body and the end. Keeps no cookies, cache or credentials, asks for no
//! compression, follows redirects like a browser (never `https` to plain HTTP), and stops when the
//! core cancels. Each request runs on its own thread after its delay, and its end wakes the
//! window (`news`).
//!
//! The client is ureq over the platform's TLS and trust store.

use kit::core::c;
use kit::nr::{nr_transport_data, nr_transport_done, nr_transport_response};
use northroom_core::transport::{Request, TIMEOUT_SECONDS};
use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;
use ureq::http;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// Bytes handed to the core per read.
const CHUNK: usize = 64 * 1024;
/// Redirect limit, matching the system's URL loader.
const MOST_REDIRECTS: u32 = 16;
const USER_AGENT: &str = "Northroom";

/// The app's one client. It keeps no cookies or connections and adds only `User-Agent` and
/// `Accept: */*`, as the system's loader does.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::NativeTls)
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .accept_encoding("")
            .max_idle_connections(0)
            .build()
            .new_agent()
    })
}

/// Sends request `id` on its own thread after its delay, then calls `ended`.
pub fn send(id: u64, request: Request, ended: extern "C" fn()) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(request.delay_ms));
        let failure = carry(id, &request).err().unwrap_or_default();
        let failure = c(&failure);
        // SAFETY: a NUL-terminated reason.
        unsafe { nr_transport_done(id, failure.as_ptr()) };
        ended();
    });
}

/// Sends request `id` and streams the reply to the core. Errors carry the system's message.
fn carry(id: u64, spec: &Request) -> Result<(), String> {
    let mut request = http::Request::builder()
        .method(http::Method::GET)
        .uri(&spec.url);
    for (name, value) in &spec.headers {
        request = request.header(*name, value);
    }
    // No body, not an empty one, so no `Content-Length` is sent.
    let request = request.body(()).map_err(|e| e.to_string())?;
    let reply = run(spec, request).map_err(|e| e.to_string())?;
    hand_over(id, reply)
}

/// Sends `request`, following redirects (never `https` to plain HTTP), with
/// `TIMEOUT_SECONDS` on connect and on each read and write.
fn run(
    spec: &Request,
    request: http::Request<()>,
) -> Result<http::Response<ureq::Body>, ureq::Error> {
    let agent = agent();
    let timeout = Some(Duration::from_secs(u64::from(TIMEOUT_SECONDS)));
    let configured = agent
        .configure_request(request)
        .max_redirects(MOST_REDIRECTS)
        .https_only(spec.url.starts_with("https://"))
        .timeout_connect(timeout)
        .timeout_per_read(timeout)
        .timeout_per_write(timeout)
        .build();
    agent.run(configured)
}

/// Hands the core the status, then the body as it arrives, until the core cancels.
fn hand_over(id: u64, reply: http::Response<ureq::Body>) -> Result<(), String> {
    if !nr_transport_response(id, u32::from(reply.status().as_u16())) {
        return Err("cancelled".to_owned());
    }
    let mut reader = reply.into_body().into_reader();
    let mut chunk = vec![0_u8; CHUNK];
    loop {
        let read = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if read == 0 {
            return Ok(());
        }
        // SAFETY: `read` bytes of `chunk`.
        if !unsafe { nr_transport_data(id, chunk.as_ptr(), read) } {
            return Err("cancelled".to_owned());
        }
    }
}
