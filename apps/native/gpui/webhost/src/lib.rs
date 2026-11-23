//! The page host the GPUI shell links for live HTML pages, the PDF view and 3D model stills. It
//! runs pages in the system web view with no network access and renders offscreen pictures. The
//! core makes almost every decision (`core/src/page/host`), so this crate is mostly platform calls.
//!
//! - `core`: typed wrappers over the core's `nr_*` functions (`core/src/page/host/ffi.rs`) for
//!   one web view's page host.
//! - `run`: main-thread work as futures, plus a run loop for programs without one.
//! - `host`: the platform-neutral web view API (`HtmlPage`, `model_still`, picture encoding).
//!   `mac` implements it with WebKit and Core Graphics through objc2's framework crates;
//!   `elsewhere` with WebView2 (Windows) and WebKitGTK (Linux), with pictures through the
//!   `image` crate.
//! - `picture` and `syntax` (the source highlighter's engine, JavaScriptCore on a Mac) are the
//!   same on every platform.
//!
//! The pinned libraries and fonts the host serves live in the crate's `resources` folder
//! (`core::resources`), next to the binary in the app or in the source tree.

pub mod base64;
pub mod core;
#[cfg(not(target_os = "macos"))]
mod elsewhere;
pub mod host;
#[cfg(target_os = "macos")]
mod mac;
pub mod picture;
pub mod run;
mod still_page;
pub mod syntax;

pub use core::HostError;
pub use host::HtmlPage;
#[cfg(not(target_os = "macos"))]
pub use host::{Button, Input, Modifiers};
pub use picture::Picture;
