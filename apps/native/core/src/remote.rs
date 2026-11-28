//! A paired phone's view of the running sessions. The phone connects to this computer over
//! `northroom-link`, lists what `sessions` lists, watches one terminal's screen and types into
//! it. Nothing is stored or run anywhere but here: the phone is a second screen and keyboard.
//!
//! This module holds the rules: which device may connect (`devices`, `pairing`), what each
//! message means (`protocol`, written down for other people's devices in `docs/phone-link.md`),
//! what a watched terminal looks like (`screen`), what a file's pages and tables look like (`pages`, `tables`) and
//! what to answer (`service`). `northroom-link`
//! only carries the bytes.

pub mod app;
pub mod bytes;
pub mod code;
pub mod devices;
pub mod pages;
pub mod pairing;
pub mod protocol;
pub mod reach;
pub mod run;
pub mod screen;
pub mod service;
pub mod tables;
