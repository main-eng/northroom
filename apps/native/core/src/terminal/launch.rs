//! How a terminal file's session starts (ADR-0005).
//!
//! The session is the person's login shell in the file's folder, so PATH and aliases match their
//! own terminal. The app's environment does not leak into it: only identity, home, locale, SSH
//! agent and proxies are kept, as in a new Terminal.app window, and nothing else of the app's,
//! secrets above all, reaches it. On Windows the session is PowerShell with the fresh
//! environment Explorer would give a program, plus the same kept variables and Northroom's own;
//! the emulator library then adds any variable of the app's that those do not name, as Zed,
//! VS Code and Alacritty pass theirs on.

use std::path::{Path, PathBuf};

/// The shell the system gives a new account, for an account record without one: zsh on a Mac,
/// bash on Linux, where zsh is often not installed.
#[cfg(target_os = "macos")]
const FALLBACK_SHELL: &str = "/bin/zsh";
#[cfg(all(unix, not(target_os = "macos")))]
const FALLBACK_SHELL: &str = "/bin/bash";
/// PowerShell 7, when installed. Windows Terminal prefers it too.
#[cfg(windows)]
const POWERSHELL_7: &str = "pwsh.exe";
/// Windows PowerShell, present on every Windows 10 and 11, under `SystemRoot`.
#[cfg(windows)]
const WINDOWS_POWERSHELL: &str = r"System32\WindowsPowerShell\v1.0\powershell.exe";
/// GUI apps often start without a locale; CLIs then draw UTF-8 as question marks.
const FALLBACK_LANG: &str = "en_US.UTF-8";
/// launchd's PATH, which Terminal.app starts with; the login shell's profile builds the rest.
const BASE_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
/// Set in every session, so a launch hook acts only inside a Northroom terminal.
pub const MARKER_VARIABLE: &str = "NORTHROOM_TERMINAL";
/// Makes Claude Code draw full screen, as Codex, Antigravity and OpenCode do unasked; set in
/// every session. It outranks the person's own `tui` setting, so one who wants the classic
/// renderer unsets it in their shell's profile.
const CLAUDE_FULL_SCREEN_VARIABLE: &str = "CLAUDE_CODE_NO_FLICKER";

const KEPT_NAMES: [&str; 15] = [
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "SSH_AUTH_SOCK",
    "__CF_USER_TEXT_ENCODING",
    "LANG",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];
const KEPT_PREFIX: &str = "LC_";

/// The kind of shell a session runs, which decides how it is started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKind {
    /// zsh, bash, fish or another Unix shell, started as a login shell.
    Posix,
    /// PowerShell, on Windows.
    PowerShell,
}

/// This platform's shell kind.
pub const HOST_SHELL: ShellKind = if cfg!(windows) {
    ShellKind::PowerShell
} else {
    ShellKind::Posix
};

/// A terminal file's bytes: two-space JSON and a newline. The file names nothing to run, so a
/// file from elsewhere can only start the person's own shell.
pub const TERMINAL_FILE: &str = "{\n  \"terminal\": 1\n}\n";

/// The session on another computer a terminal file shows: that computer's key, as hex, and
/// the session's name there. Only a computer the person paired with is ever reached, so a
/// file from elsewhere that names another shows nothing.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Elsewhere {
    pub computer: String,
    pub session: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FarFile {
    terminal: u32,
    #[serde(flatten)]
    elsewhere: Elsewhere,
}

impl Elsewhere {
    /// What the terminal file `bytes` says of a session elsewhere. `None` for a terminal of
    /// this computer's own.
    pub fn read(bytes: &[u8]) -> Option<Self> {
        let file: FarFile = serde_json::from_slice(bytes).ok()?;
        Some(file.elsewhere)
    }

    /// The bytes of the terminal file that shows this session.
    pub fn file(&self) -> String {
        let file = FarFile {
            terminal: 1,
            elsewhere: self.clone(),
        };
        let mut written = serde_json::to_string_pretty(&file).unwrap_or_default();
        written.push('\n');
        written
    }
}

/// What a session runs: an absolute program, its argv, its environment and its folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub file: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

/// The account's login shell when it is an absolute path, else the system's default.
#[cfg(unix)]
pub fn choose_shell(account_shell: Option<&str>) -> String {
    match account_shell {
        Some(shell) if shell.starts_with('/') => shell.to_owned(),
        _ => FALLBACK_SHELL.to_owned(),
    }
}

/// PowerShell 7 when it is on PATH, else Windows PowerShell. `SHELL` is ignored: Windows
/// accounts name no login shell, and only Unix-like environments such as Git Bash set it.
#[cfg(windows)]
pub fn choose_shell(_account_shell: Option<&str>) -> String {
    let on_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|folder| folder.join(POWERSHELL_7))
            .find(|program| program.is_file())
    });
    match on_path {
        Some(program) => program.to_string_lossy().into_owned(),
        None => {
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
            format!(r"{root}\{WINDOWS_POWERSHELL}")
        }
    }
}

/// The person's shell in `directory`, started as their own terminal would start it. Only the
/// kept variables are taken from the app's `environment`; `session` holds what Northroom sets
/// for programs in it (`terminal::harness`).
pub fn build_launch(
    directory: &Path,
    environment: &[(String, String)],
    shell: &str,
    session: &[(String, String)],
) -> Launch {
    build_launch_in(
        HOST_SHELL,
        directory,
        &fresh_environment(),
        environment,
        shell,
        session,
    )
}

/// The fresh environment a PowerShell session starts from: the user's and system's variables,
/// as `CreateEnvironmentBlock` builds them for the process token with nothing inherited. Empty,
/// with a log line, when Windows refuses; the session then has only what the terminal keeps and
/// adds. Empty elsewhere, since a POSIX login shell builds its own from its profile.
#[cfg(windows)]
pub fn fresh_environment() -> Vec<(String, String)> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{TOKEN_DUPLICATE, TOKEN_QUERY};
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    let mut token = HANDLE::default();
    let mut block: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: the token is the process's own and is closed once the block is made. The block
    // is read up to its terminating empty string, then freed.
    unsafe {
        let made = OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &mut token,
        )
        .and_then(|()| {
            let made = CreateEnvironmentBlock(&mut block, Some(token), false);
            let _ = CloseHandle(token);
            made
        });
        if let Err(error) = made {
            eprintln!("the terminal's environment was not made: {error}");
            return Vec::new();
        }
        let start = block.cast::<u16>().cast_const();
        let mut length = 0;
        // Nul-terminated strings back to back; an empty one ends the block.
        while !(*start.add(length) == 0 && (length == 0 || *start.add(length - 1) == 0)) {
            length += 1;
        }
        let variables = environment_block(std::slice::from_raw_parts(start, length));
        let _ = DestroyEnvironmentBlock(block);
        variables
    }
}

#[cfg(unix)]
pub fn fresh_environment() -> Vec<(String, String)> {
    Vec::new()
}

/// The variables of a Windows environment block (nul-terminated `name=value` strings), without
/// its final nul. The `=C:=C:\…` entries hold each drive's folder, not variables, and are
/// skipped.
#[cfg_attr(unix, allow(dead_code))]
fn environment_block(block: &[u16]) -> Vec<(String, String)> {
    block
        .split(|unit| *unit == 0)
        .map(String::from_utf16_lossy)
        .filter_map(|entry| {
            let (name, value) = entry.split_once('=')?;
            (!name.is_empty()).then(|| (name.to_owned(), value.to_owned()))
        })
        .collect()
}

/// Whether a variable from the app's environment passes into the terminal's.
fn is_kept(name: &str) -> bool {
    KEPT_NAMES.contains(&name) || name.starts_with(KEPT_PREFIX)
}

/// A launch's variables, each set once. Names match as `kind`'s system matches them
/// (case-insensitive for PowerShell).
struct LaunchVariables {
    kind: ShellKind,
    variables: Vec<(String, String)>,
}

impl LaunchVariables {
    /// Sets `name` to `value`, replacing any existing value.
    fn set(&mut self, name: &str, value: &str) {
        let kind = self.kind;
        let same = |existing: &str| match kind {
            ShellKind::Posix => existing == name,
            ShellKind::PowerShell => existing.eq_ignore_ascii_case(name),
        };
        match self
            .variables
            .iter_mut()
            .find(|(existing, _)| same(existing))
        {
            Some(entry) => entry.1 = value.to_owned(),
            None => self.variables.push((name.to_owned(), value.to_owned())),
        }
    }
}

/// The shell in `directory` with only the kept variables from `environment`, Northroom's own and
/// the `session`'s. A POSIX shell runs as a login shell whose profile builds the rest; PowerShell
/// starts from the system's `fresh` environment, with names matched case-insensitively.
fn build_launch_in(
    kind: ShellKind,
    directory: &Path,
    fresh: &[(String, String)],
    environment: &[(String, String)],
    shell: &str,
    session: &[(String, String)],
) -> Launch {
    let mut variables = LaunchVariables {
        kind,
        variables: Vec::new(),
    };
    if kind == ShellKind::PowerShell {
        for (name, value) in fresh {
            variables.set(name, value);
        }
    }
    for (name, value) in environment.iter().filter(|(name, _)| is_kept(name)) {
        variables.set(name, value);
    }
    if kind == ShellKind::Posix {
        if !environment.iter().any(|(name, _)| name == "LANG") {
            variables.set("LANG", FALLBACK_LANG);
        }
        variables.set("SHELL", shell);
        variables.set("PATH", BASE_PATH);
    }
    variables.set("TERM", "xterm-256color");
    variables.set("COLORTERM", "truecolor");
    variables.set("TERM_PROGRAM", "Northroom");
    variables.set(MARKER_VARIABLE, "1");
    variables.set(CLAUDE_FULL_SCREEN_VARIABLE, "1");
    for (name, value) in session {
        variables.set(name, value);
    }
    let args = match kind {
        ShellKind::Posix => vec![shell.to_owned(), "-l".to_owned()],
        ShellKind::PowerShell => vec![shell.to_owned(), "-NoLogo".to_owned()],
    };
    Launch {
        file: shell.to_owned(),
        args,
        env: variables.variables,
        cwd: directory.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn a_terminal_file_names_nothing_to_run() {
        let value: serde_json::Value = serde_json::from_str(TERMINAL_FILE).expect("json");
        assert_eq!(value, serde_json::json!({ "terminal": 1 }));
        assert!(TERMINAL_FILE.ends_with("}\n"));
    }

    #[test]
    fn a_terminal_is_the_login_shell_with_only_what_is_kept() {
        let launch = build_launch_in(
            ShellKind::Posix,
            Path::new("/Users/someone/Work"),
            &[],
            &environment(&[
                ("HOME", "/Users/someone"),
                ("ANTHROPIC_API_KEY", "secret"),
                ("LC_ALL", "en_GB.UTF-8"),
                ("PATH", "/opt/bin"),
            ]),
            "/bin/zsh",
            &environment(&[("CLAUDE_CODE_PLUGIN_DIRS", "/support/plugin")]),
        );
        assert_eq!(launch.file, "/bin/zsh");
        assert_eq!(launch.args, ["/bin/zsh", "-l"]);
        assert_eq!(launch.cwd, Path::new("/Users/someone/Work"));
        let value = |name: &str| {
            launch
                .env
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(value("HOME"), Some("/Users/someone"));
        assert_eq!(value("LC_ALL"), Some("en_GB.UTF-8"));
        assert_eq!(value("ANTHROPIC_API_KEY"), None);
        assert_eq!(value("LANG"), Some("en_US.UTF-8"));
        assert_eq!(value("SHELL"), Some("/bin/zsh"));
        assert_eq!(value("PATH"), Some("/usr/bin:/bin:/usr/sbin:/sbin"));
        assert_eq!(value("TERM"), Some("xterm-256color"));
        assert_eq!(value("TERM_PROGRAM"), Some("Northroom"));
        assert_eq!(value(MARKER_VARIABLE), Some("1"));
        assert_eq!(value("CLAUDE_CODE_NO_FLICKER"), Some("1"));
        assert_eq!(value("CLAUDE_CODE_PLUGIN_DIRS"), Some("/support/plugin"));

        let german = build_launch_in(
            ShellKind::Posix,
            Path::new("/tmp"),
            &[],
            &environment(&[("LANG", "de_DE.UTF-8")]),
            "/bin/bash",
            &[],
        );
        assert_eq!(german.args, ["/bin/bash", "-l"]);
        assert!(german
            .env
            .contains(&("LANG".to_owned(), "de_DE.UTF-8".to_owned())));
    }

    #[test]
    fn powershell_starts_from_the_fresh_environment_and_keeps_none_of_the_apps_secrets() {
        let fresh = environment(&[
            ("Path", r"C:\Windows\system32"),
            ("SystemRoot", r"C:\Windows"),
            ("USERPROFILE", r"C:\Users\someone"),
        ]);
        let app = environment(&[
            ("PATH", r"C:\app\bin"),
            ("ANTHROPIC_API_KEY", "secret"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "secret"),
            ("NR_SECRET_PROBE", "secret"),
            ("LC_NORTHROOM_PROBE", "kept"),
            ("HTTPS_PROXY", "http://proxy.test"),
        ]);
        let shell = r"C:\Program Files\PowerShell\7\pwsh.exe";
        let launch = build_launch_in(
            ShellKind::PowerShell,
            Path::new(r"C:\Users\someone\Work"),
            &fresh,
            &app,
            shell,
            &[],
        );
        assert_eq!(launch.file, shell);
        assert_eq!(launch.args, [shell, "-NoLogo"]);
        let value = |name: &str| {
            launch
                .env
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.as_str())
        };
        // The system's value wins over the app's, and each name appears once in any case.
        assert_eq!(value("PATH"), Some(r"C:\Windows\system32"));
        assert_eq!(
            launch
                .env
                .iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case("path"))
                .count(),
            1
        );
        assert_eq!(value("SystemRoot"), Some(r"C:\Windows"));
        for secret in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "NR_SECRET_PROBE",
        ] {
            assert_eq!(value(secret), None, "{secret}");
        }
        // Kept app variables and the terminal's own.
        assert_eq!(value("LC_NORTHROOM_PROBE"), Some("kept"));
        assert_eq!(value("HTTPS_PROXY"), Some("http://proxy.test"));
        assert_eq!(value("SHELL"), None);
        assert_eq!(value("TERM"), Some("xterm-256color"));
        assert_eq!(value(MARKER_VARIABLE), Some("1"));
    }

    #[test]
    fn a_windows_environment_block_is_read_into_its_variables() {
        let block: Vec<u16> = "=C:=C:\\work\0Path=C:\\bin\0USERNAME=someone\0EMPTY=\0"
            .encode_utf16()
            .collect();
        assert_eq!(
            environment_block(&block),
            environment(&[("Path", r"C:\bin"), ("USERNAME", "someone"), ("EMPTY", "")])
        );
        assert!(environment_block(&[]).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn falls_back_to_the_systems_shell_without_an_absolute_account_shell() {
        assert_eq!(
            choose_shell(Some("/opt/homebrew/bin/fish")),
            "/opt/homebrew/bin/fish"
        );
        let fallback = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };
        assert_eq!(choose_shell(Some("zsh")), fallback);
        assert_eq!(choose_shell(None), fallback);
    }
}
