//! Renders each kit component in its states, where the core lays it out, and saves PNGs.
//!
//! Usage: kit-gallery <folder> <scene> ...
//! The scenes are the form controls' (`form-*`, `controls.rs`).

mod controls;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let folder = std::path::PathBuf::from(
        arguments
            .next()
            .expect("usage: kit-gallery <folder> <scene> ..."),
    );
    let scenes: Vec<String> = arguments.collect();
    match scenes.first() {
        Some(first) if controls::owns(first) => controls::run(folder, scenes),
        Some(first) => eprintln!("gallery\tno scene named {first}"),
        None => controls::run(folder, scenes),
    }
}
