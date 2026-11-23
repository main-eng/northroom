//! Core Text reference widths for the text-widths check (`scripts/text-widths-check.sh`). The
//! core's typesetter (`typeface`, `typeset`) is compared against these.
//!
//! Reads one case per line from stdin, tab-separated:
//!   W <font> <tracking> <text>                         one line's width
//!   L <font> <width> <wraps> <tab spaces> <text>       the lines the words take and the widest
//! where <font> is five fields: size, weight, mono, tabular, slanted (0 or 1). Line ends and tabs
//! in text are escaped as \n and \t. Echoes each case with Core Text's answer after a tab (the
//! width, or the line count and widest line). Before it, writes each run neither app face has
//! glyphs for as `F <font> <stretch> <width>`, the standalone width the core asks the platform
//! for.
//!
//! Usage: text-widths <fonts folder> < cases > answers   (core/assets/fonts; a Mac only)

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    mac::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("text-widths asks Core Text, which only a Mac has");
    std::process::ExitCode::from(2)
}
