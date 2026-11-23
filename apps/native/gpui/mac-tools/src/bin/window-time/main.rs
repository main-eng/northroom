//! Launches a program and prints how long until it has a window on screen, measured from outside
//! the process so any app can be timed the same way. Then waits `settle` seconds, prints the
//! memory footprint of the whole process tree, and quits the program.
//!
//! Usage: window-time <settle seconds> <executable> [arguments...]   (a Mac only)

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    mac::run()
}

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("window-time reads the Mac's window list, which only a Mac has");
    std::process::ExitCode::from(2)
}
