//! The platform-neutral page host: a drive HTML file as a live page (`HtmlPage`), a model's still
//! (`model_still`), and picture encoding. The Mac uses WebKit (`crate::mac`). Windows and Linux
//! use WebView2 and WebKitGTK under the same names (`crate::elsewhere`); there a live page is
//! shown as pictures and receives forwarded input (`Input`).

#[cfg(target_os = "macos")]
pub use crate::mac::{HtmlPage, Said, decode, model_still, png};

#[cfg(not(target_os = "macos"))]
pub use crate::elsewhere::{Button, HtmlPage, Input, Modifiers, Said, decode, model_still, png};
