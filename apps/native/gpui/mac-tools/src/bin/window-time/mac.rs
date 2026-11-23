//! The window list through Core Graphics, the footprint through `proc_pid_rusage`.

use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGRect,
};
use objc2_core_graphics::{
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerPID,
};
use std::collections::HashMap;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

pub fn run() -> ExitCode {
    let arguments: Vec<String> = std::env::args().collect();
    let settle = arguments
        .get(1)
        .and_then(|settle| settle.parse::<f64>().ok());
    let (Some(settle), Some(program)) = (settle, arguments.get(2)) else {
        eprintln!("usage: window-time <settle seconds> <executable> [arguments...]");
        return ExitCode::from(2);
    };
    let started = Instant::now();
    let mut child = match Command::new(program)
        .args(&arguments[3..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("{program}: {error}");
            return ExitCode::from(2);
        }
    };
    let pid = child.id() as i32;
    let mut window_milliseconds = -1.0;
    while child.try_wait().ok().flatten().is_none() {
        if has_window(pid) {
            window_milliseconds = started.elapsed().as_secs_f64() * 1e3;
            break;
        }
        if started.elapsed() > Duration::from_secs(30) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_secs_f64(settle));
    let tree = descendants(pid);
    let total: f64 = tree.iter().map(|pid| footprint_megabytes(*pid)).sum();
    println!(
        "first window {window_milliseconds:.0} ms; {} processes; footprint {total:.0} MB",
        tree.len()
    );
    for pid in tree.iter().rev() {
        // SAFETY: signals only processes in the tree this tool started.
        unsafe { libc::kill(*pid, libc::SIGTERM) };
    }
    let _ = child.wait();
    ExitCode::SUCCESS
}

/// `root` and every process under it, parents first, as `ps` lists them.
fn descendants(root: i32) -> Vec<i32> {
    let listed = Command::new("/bin/ps")
        .args(["-axo", "pid=,ppid="])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for line in listed.lines() {
        let parts: Vec<i32> = line
            .split(' ')
            .filter_map(|part| part.parse().ok())
            .collect();
        if let [pid, parent] = parts[..] {
            children.entry(parent).or_default().push(pid);
        }
    }
    let mut all = vec![root];
    let mut index = 0;
    while index < all.len() {
        all.extend(children.get(&all[index]).into_iter().flatten());
        index += 1;
    }
    all
}

/// Whether `owner` has a window on screen in the normal layer larger than 300 × 200 points.
fn has_window(owner: i32) -> bool {
    let Some(windows) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return false;
    };
    // SAFETY: the window list is an array of string-keyed dictionaries of CF values.
    let windows: CFRetained<CFArray<CFDictionary<CFString, CFType>>> =
        unsafe { CFRetained::cast_unchecked(windows) };
    windows.iter().any(|window| {
        // SAFETY: the framework's keys.
        let (owner_key, layer_key, bounds_key) =
            unsafe { (kCGWindowOwnerPID, kCGWindowLayer, kCGWindowBounds) };
        let number = |key: &CFString| {
            window
                .get(key)
                .and_then(|value| value.downcast::<CFNumber>().ok())
                .and_then(|number| number.as_i64())
        };
        let mut bounds = CGRect::default();
        let measured = window
            .get(bounds_key)
            .and_then(|value| value.downcast::<CFDictionary>().ok())
            // SAFETY: a rectangle's dictionary, read into a live rectangle.
            .is_some_and(|dictionary| unsafe {
                CGRectMakeWithDictionaryRepresentation(Some(&dictionary), &mut bounds)
            });
        number(owner_key) == Some(i64::from(owner))
            && number(layer_key) == Some(0)
            && measured
            && bounds.size.width > 300.0
            && bounds.size.height > 200.0
    })
}

/// `pid`'s footprint in MB as Activity Monitor's Memory column counts it, or 0 if it has exited.
fn footprint_megabytes(pid: i32) -> f64 {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a writable `rusage_info_v2`.
    let result = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&raw mut info).cast::<libc::rusage_info_t>(),
        )
    };
    if result == 0 {
        info.ri_phys_footprint as f64 / 1_048_576.0
    } else {
        0.0
    }
}
