//! The diagnostic log: one text file in the app's logs folder.
//!
//! Each line is `<UTC time> <LEVEL> pid=<pid> <message>`. At startup a file over 5 MB is renamed
//! to `<name>.1`, replacing any earlier one. Help → Open Diagnostic Logs opens the folder.

use crate::Relock;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOG_FILE_NAME: &str = "northroom-native.log";
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// A UTC time as the log writes it: `2026-11-12T09:30:05.123Z`.
pub fn iso_time(millis_since_1970: u64) -> String {
    let seconds = millis_since_1970 / 1000;
    let days = (seconds / 86_400) as i64;
    let of_day = seconds % 86_400;
    let (year, month, day) = crate::calendar::civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60,
        millis_since_1970 % 1000
    )
}

/// One log line, including its line break.
pub fn format_line(millis_since_1970: u64, level: Level, pid: u32, message: &str) -> String {
    format!(
        "{} {} pid={pid} {message}\n",
        iso_time(millis_since_1970),
        level.name()
    )
}

/// The open log file.
pub struct Log {
    directory: PathBuf,
    file: PathBuf,
}

impl Log {
    /// Creates the logs folder and rotates an oversized file.
    pub fn open(directory: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let file = directory.join(LOG_FILE_NAME);
        if std::fs::metadata(&file).is_ok_and(|kept| kept.len() > MAX_LOG_BYTES) {
            let mut previous = file.clone().into_os_string();
            previous.push(".1");
            // `rename` overwrites the previous `.1` file.
            std::fs::rename(&file, previous)?;
        }
        Ok(Self {
            directory: directory.to_owned(),
            file,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    pub fn append(
        &self,
        millis_since_1970: u64,
        level: Level,
        pid: u32,
        message: &str,
    ) -> std::io::Result<()> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)?
            .write_all(format_line(millis_since_1970, level, pid, message).as_bytes())
    }
}

/// The running app's log. `None` until a shell names the logs folder, or if it can't be created.
static LOG: Mutex<Option<Log>> = Mutex::new(None);

/// Opens the log in `directory` and routes core panics to it. Returns false if the folder is
/// unusable.
pub fn open(directory: &Path) -> bool {
    let log = Log::open(directory).ok();
    let opened = log.is_some();
    *LOG.relock() = log;
    if opened {
        static HOOKED: std::sync::Once = std::sync::Once::new();
        HOOKED.call_once(|| {
            let earlier = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |panic| {
                write(
                    Level::Error,
                    &format!("[northroom-native] core panic: {panic}"),
                );
                earlier(panic);
            }));
        });
    }
    opened
}

pub fn directory() -> Option<PathBuf> {
    LOG.relock().as_ref().map(|log| log.directory().to_owned())
}

/// Appends a line to the log. Drops it silently when there is no log or the write fails, so
/// logging never stops the app.
pub fn write(level: Level, message: &str) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    if let Some(log) = LOG.relock().as_ref() {
        let _ = log.append(millis, level, std::process::id(), message);
    }
}

/// Prefix for every message the app logs.
const PREFIX: &str = "[northroom-native] ";

/// Formats a log message: prefix, message, then the details as JSON with keys in order. Omits
/// the JSON when there are no details.
pub fn message_with(message: &str, details: &serde_json::Value) -> String {
    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                serde_json::Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), sorted(&map[key])))
                        .collect(),
                )
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(sorted).collect())
            }
            other => other.clone(),
        }
    }
    match details {
        serde_json::Value::Null => format!("{PREFIX}{message}"),
        details => format!("{PREFIX}{message} {}", sorted(details)),
    }
}

pub fn write_with(level: Level, message: &str, details: &serde_json::Value) {
    write(level, &message_with(message, details));
}

/// The version logged for a build with no version of its own.
const NO_VERSION: &str = "0.0.0";

/// Platform facts about the running app, for the log header.
pub struct Startup<'a> {
    pub app_name: &'a str,
    /// Empty for an unversioned build, which logs as 0.0.0.
    pub version: &'a str,
    /// True for an installed app bundle, false for a build run from its folder.
    pub packaged: bool,
    /// The OS version as the system describes it.
    pub system: &'a str,
}

/// Platform and CPU names as Node's `process.platform` and `process.arch` spell them.
fn platform() -> (&'static str, &'static str) {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    (platform, arch)
}

/// The two header lines: the log's path and what is running.
pub fn startup_lines(directory: &Path, support: &Path, startup: &Startup, pid: u32) -> [String; 2] {
    let logs = directory.to_string_lossy();
    let (platform, arch) = platform();
    [
        message_with(
            "logging initialized",
            &serde_json::json!({ "logFilePath": directory.join(LOG_FILE_NAME).to_string_lossy(), "logsDirectoryPath": logs }),
        ),
        message_with(
            "startup diagnostics",
            &serde_json::json!({
                "appName": startup.app_name,
                "appVersion": if startup.version.is_empty() { NO_VERSION } else { startup.version },
                "isPackaged": startup.packaged,
                "platform": platform,
                "system": startup.system,
                "arch": arch,
                "pid": pid,
                "paths": { "userData": support.to_string_lossy(), "logs": logs },
            }),
        ),
    ]
}

/// Opens the log in `directory` and writes the header. If the folder is unusable, prints to
/// stderr and runs without a log.
pub fn start(directory: &Path, support: &Path, startup: &Startup) {
    if !open(directory) {
        eprintln!(
            "{PREFIX}logging initialization failed: {}",
            directory.display()
        );
        return;
    }
    for line in startup_lines(directory, support, startup, std::process::id()) {
        write(Level::Info, &line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "northroom-diagnostics-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn times_are_written_as_utc_to_the_millisecond() {
        assert_eq!(iso_time(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_time(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(iso_time(1_784_808_300_123), "2026-07-23T12:05:00.123Z");
        assert_eq!(iso_time(1_798_761_599_999), "2026-12-31T23:59:59.999Z");
    }

    #[test]
    fn a_line_is_the_time_the_level_the_process_and_the_message() {
        assert_eq!(
            format_line(
                1_784_808_300_123,
                Level::Warn,
                4242,
                "[northroom-native] something"
            ),
            "2026-07-23T12:05:00.123Z WARN pid=4242 [northroom-native] something\n"
        );
        assert!(format_line(0, Level::Info, 1, "x").contains(" INFO pid=1 x"));
        assert!(format_line(0, Level::Error, 1, "x").contains(" ERROR pid=1 x"));
    }

    #[test]
    fn lines_are_added_to_one_file_in_the_logs_folder() {
        let directory = scratch("append").join("Logs");
        let log = Log::open(&directory).unwrap();
        assert_eq!(log.file(), directory.join("northroom-native.log"));
        log.append(1000, Level::Info, 7, "first").unwrap();
        log.append(2000, Level::Error, 7, "second").unwrap();
        // Opening again keeps what is there.
        let log = Log::open(&directory).unwrap();
        log.append(3000, Level::Info, 8, "third").unwrap();
        let text = std::fs::read_to_string(log.file()).unwrap();
        assert_eq!(
            text,
            "1970-01-01T00:00:01.000Z INFO pid=7 first\n1970-01-01T00:00:02.000Z ERROR pid=7 second\n1970-01-01T00:00:03.000Z INFO pid=8 third\n"
        );
        assert!(!directory.join("northroom-native.log.1").exists());
        let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    }

    #[test]
    fn a_file_over_five_megabytes_is_set_aside_when_the_log_opens() {
        let directory = scratch("rotate");
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("northroom-native.log");
        let previous = directory.join("northroom-native.log.1");
        std::fs::write(&previous, "older").unwrap();
        // Exactly the limit stays; one byte more goes.
        std::fs::write(&file, vec![b'a'; MAX_LOG_BYTES as usize]).unwrap();
        Log::open(&directory).unwrap();
        assert!(file.exists());
        assert_eq!(std::fs::read_to_string(&previous).unwrap(), "older");
        std::fs::write(&file, vec![b'b'; MAX_LOG_BYTES as usize + 1]).unwrap();
        let log = Log::open(&directory).unwrap();
        assert!(!file.exists());
        assert_eq!(
            std::fs::metadata(&previous).unwrap().len(),
            MAX_LOG_BYTES + 1
        );
        log.append(0, Level::Info, 1, "fresh").unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "1970-01-01T00:00:00.000Z INFO pid=1 fresh\n"
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_folder_that_cannot_be_made_opens_no_log() {
        let directory = scratch("blocked");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("file"), "").unwrap();
        assert!(Log::open(&directory.join("file").join("Logs")).is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_message_carries_its_details_with_their_keys_in_order() {
        assert_eq!(
            message_with("plain", &serde_json::Value::Null),
            "[northroom-native] plain"
        );
        assert_eq!(
            message_with(
                "the drive folder could not be opened",
                &serde_json::json!({ "folder": "/tmp/a b/c" })
            ),
            r#"[northroom-native] the drive folder could not be opened {"folder":"/tmp/a b/c"}"#
        );
        assert_eq!(
            message_with(
                "x",
                &serde_json::json!({ "z": 1, "a": { "y": true, "b": [ { "d": 1, "c": 2 } ] } })
            ),
            r#"[northroom-native] x {"a":{"b":[{"c":2,"d":1}],"y":true},"z":1}"#
        );
    }

    #[test]
    fn the_log_starts_with_where_it_is_and_what_runs() {
        let startup = Startup {
            app_name: "Northroom",
            version: "0.4.2",
            packaged: true,
            system: "Version 27.0 (Build 27A1)",
        };
        let [first, second] = startup_lines(
            Path::new("/L/Logs"),
            Path::new("/S/Northroom Native"),
            &startup,
            42,
        );
        // The file is named with the system's separator, as JSON writes it.
        let file = serde_json::to_string(&Path::new("/L/Logs").join(LOG_FILE_NAME)).unwrap();
        assert_eq!(
            first,
            format!(
                r#"[northroom-native] logging initialized {{"logFilePath":{file},"logsDirectoryPath":"/L/Logs"}}"#
            )
        );
        let (platform, arch) = platform();
        assert_eq!(
            second,
            format!(
                r#"[northroom-native] startup diagnostics {{"appName":"Northroom","appVersion":"0.4.2","arch":"{arch}","isPackaged":true,"paths":{{"logs":"/L/Logs","userData":"/S/Northroom Native"}},"pid":42,"platform":"{platform}","system":"Version 27.0 (Build 27A1)"}}"#
            )
        );
        assert_eq!(
            platform,
            if cfg!(target_os = "macos") {
                "darwin"
            } else if cfg!(windows) {
                "win32"
            } else {
                "linux"
            }
        );
        // A build run from its folder is unversioned.
        let unversioned = Startup {
            version: "",
            ..startup
        };
        let [_, second] = startup_lines(
            Path::new("/L/Logs"),
            Path::new("/S/Northroom Native"),
            &unversioned,
            42,
        );
        assert!(second.contains(r#""appVersion":"0.0.0""#), "{second}");
    }
}
