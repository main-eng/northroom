//! The Mac host: WebKit's `WKWebView` for pages and Core Graphics for pictures.

mod html_page;
pub mod model_still;
mod pixels;
mod web;

pub use html_page::HtmlPage;
pub use pixels::{decode, png};
pub use web::Said;
