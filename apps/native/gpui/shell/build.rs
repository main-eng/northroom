//! Embeds the Windows resources: the app icon (resource 1, shown by GPUI's window and Explorer)
//! and the version, read from `core/Cargo.toml`, its single source.

fn main() {
    #[cfg(windows)]
    windows_resources();
}

#[cfg(windows)]
fn windows_resources() {
    use std::path::PathBuf;
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo names the crate"));
    let core = here.join("../../core/Cargo.toml");
    println!("cargo:rerun-if-changed={}", core.display());
    println!("cargo:rerun-if-changed=windows/northroom.rc");
    println!("cargo:rerun-if-changed=Northroom.ico");
    let manifest = std::fs::read_to_string(&core).expect("the core's manifest is there");
    let version = manifest
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("the core's manifest names its version");
    let mut numbers: Vec<String> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|part| part.parse::<u16>().unwrap_or(0).to_string())
        .collect();
    numbers.resize(4, "0".to_owned());
    let template = std::fs::read_to_string(here.join("windows/northroom.rc"))
        .expect("the resource template is there");
    let icon = here
        .join("Northroom.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");
    let script = template
        .replace("@ICON@", &icon)
        .replace("@VERSION@", version)
        .replace("@NUMBERS@", &numbers.join(","));
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("cargo gives an output folder"));
    let written = out.join("northroom.rc");
    std::fs::write(&written, script).expect("the resource script is written");
    embed_resource::compile(&written, embed_resource::NONE)
        .manifest_optional()
        .expect("the resources compile");
}
