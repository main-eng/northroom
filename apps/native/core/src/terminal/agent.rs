//! Which agent a terminal is running (ADR-0005). The terminal is the person's shell and they
//! start whatever they like in it, so the agent is read from the foreground program and known
//! by the name it was started under, whatever its options, alias or path.

use std::time::{Duration, Instant};

/// A freshly forked program still shows its shell's arguments until it execs. A program that
/// is not an agent is re-checked for this long after taking the foreground, then left alone.
const SETTLE: Duration = Duration::from_secs(2);
/// How often it is re-checked meanwhile, however fast its output comes.
const ASK_EVERY: Duration = Duration::from_millis(100);

/// Interpreters whose script names the agent.
const INTERPRETERS: [&str; 7] = ["node", "bun", "deno", "python3", "python", "sh", "bash"];
/// Extensions a script name may add to the agent's name.
const SCRIPT_ENDINGS: [&str; 3] = [".js", ".mjs", ".cjs"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
    Antigravity,
    OpenCode,
    Gemini,
}

impl Agent {
    pub const ALL: [Agent; 6] = [
        Self::Claude,
        Self::Codex,
        Self::Cursor,
        Self::Antigravity,
        Self::OpenCode,
        Self::Gemini,
    ];

    /// The names the agent's program is started under.
    fn programs(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude"],
            Self::Codex => &["codex"],
            Self::Cursor => &["agent", "cursor-agent"],
            Self::Antigravity => &["agy", "antigravity"],
            Self::OpenCode => &["opencode"],
            Self::Gemini => &["gemini"],
        }
    }

    /// The agent's own name, the same in every language.
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Antigravity => "Antigravity",
            Self::OpenCode => "OpenCode",
            Self::Gemini => "Gemini",
        }
    }

    /// The asset name of the agent's logo. `None` when the logo needs the vendor's written
    /// permission; the terminal glyph shows instead.
    pub fn logo(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("claude-code"),
            Self::Codex => Some("codex"),
            Self::Cursor => Some("cursor"),
            Self::Antigravity => Some("antigravity"),
            Self::OpenCode | Self::Gemini => None,
        }
    }

    /// The agent a process with `arguments` is, by its program name, or by its script name when the
    /// program is an interpreter (`node /opt/bin/codex`).
    pub fn running(arguments: &[String]) -> Option<Self> {
        let mut names = arguments.iter().map(|argument| program_name(argument));
        let first = names.next()?;
        let name = if INTERPRETERS.contains(&first) {
            names.find(|name| !name.starts_with('-'))?
        } else {
            first
        };
        Self::ALL
            .into_iter()
            .find(|agent| agent.programs().contains(&name))
    }
}

/// The name a program was started under: the last path component, minus any script extension.
fn program_name(argument: &str) -> &str {
    let name = argument.rsplit('/').next().unwrap_or(argument);
    SCRIPT_ENDINGS
        .iter()
        .find_map(|ending| name.strip_suffix(ending))
        .unwrap_or(name)
}

/// Tracks the agent in one terminal's foreground, checked as the terminal's output is read.
/// A program taking or leaving the foreground always writes something.
pub struct Watch {
    /// The session's own process, the shell: no agent while it has the foreground.
    shell: i32,
    /// The process group in the foreground when last looked at, and since when.
    group: Option<i32>,
    since: Instant,
    /// When the foreground's program was last asked what it is.
    asked: Instant,
    /// The foreground program has been identified, or has had `SETTLE` to exec.
    settled: bool,
    agent: Option<Agent>,
    /// The arguments the agent was identified by; empty with no agent.
    arguments: Vec<String>,
}

impl Watch {
    pub fn new(shell: i32) -> Self {
        Self {
            shell,
            group: None,
            since: Instant::now(),
            asked: Instant::now(),
            settled: false,
            agent: None,
            arguments: Vec::new(),
        }
    }

    /// The arguments the current agent was started with; empty with no agent.
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Whether process group `group` is the session's own shell, meaning the terminal is at its
    /// prompt with nothing running.
    pub fn is_shell(&self, group: i32) -> bool {
        group == self.shell
    }

    /// Takes the current foreground process group. `arguments_of` returns a process's arguments
    /// and is called only while the foreground program is unknown. Returns the agent when it
    /// changed (`Some(None)` when no agent runs now).
    pub fn look(
        &mut self,
        group: i32,
        now: Instant,
        arguments_of: impl Fn(i32) -> Option<Vec<String>>,
    ) -> Option<Option<Agent>> {
        if self.group != Some(group) {
            self.group = Some(group);
            self.since = now;
            self.settled = group == self.shell;
        } else if self.settled || now.duration_since(self.asked) < ASK_EVERY {
            return None;
        }
        let found = if self.settled {
            None
        } else {
            self.asked = now;
            arguments_of(group)
                .and_then(|arguments| Agent::running(&arguments).map(|agent| (agent, arguments)))
        };
        let agent = found.as_ref().map(|(agent, _)| *agent);
        if agent.is_some() || now.duration_since(self.since) >= SETTLE {
            self.settled = true;
        }
        (agent != self.agent).then(|| {
            self.agent = agent;
            self.arguments = found.map(|(_, arguments)| arguments).unwrap_or_default();
            agent
        })
    }
}

/// The process group in the foreground of the terminal whose app side is `terminal`.
#[cfg(unix)]
pub fn foreground_group(terminal: std::os::fd::RawFd) -> Option<i32> {
    // SAFETY: queries a descriptor the caller holds open and writes nothing.
    let group = unsafe { libc::tcgetpgrp(terminal) };
    (group > 0).then_some(group)
}

/// A running process's arguments as it was started; `None` when it has exited or belongs to
/// another user.
#[cfg(target_os = "macos")]
pub fn arguments_of(process: i32) -> Option<Vec<String>> {
    let mut size: libc::c_int = 0;
    let mut size_length = std::mem::size_of::<libc::c_int>();
    let mut longest = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: the system writes one `c_int`, matching the length passed in `size`.
    let asked = unsafe {
        libc::sysctl(
            longest.as_mut_ptr(),
            2,
            std::ptr::from_mut(&mut size).cast(),
            &mut size_length,
            std::ptr::null_mut(),
            0,
        )
    };
    if asked != 0 || size <= 0 {
        return None;
    }
    let mut bytes = vec![0_u8; usize::try_from(size).ok()?];
    let mut length = bytes.len();
    let mut of_process = [libc::CTL_KERN, libc::KERN_PROCARGS2, process];
    // SAFETY: the system writes at most `length` bytes into `bytes`, which is that long, and
    // says how many it wrote.
    let read = unsafe {
        libc::sysctl(
            of_process.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    read_process_arguments(bytes.get(..length)?)
}

/// Parses macOS's `KERN_PROCARGS2` layout: the argument count, the program path, nul padding
/// to a word boundary, then each argument nul-terminated, followed by the environment.
#[cfg(any(target_os = "macos", test))]
fn read_process_arguments(bytes: &[u8]) -> Option<Vec<String>> {
    let (count, rest) = bytes.split_first_chunk::<4>()?;
    let count = usize::try_from(i32::from_ne_bytes(*count)).ok()?;
    let path_end = rest.iter().position(|byte| *byte == 0)?;
    let after_path = &rest[path_end..];
    let first = after_path.iter().position(|byte| *byte != 0)?;
    let arguments: Vec<String> = after_path[first..]
        .split(|byte| *byte == 0)
        .take(count)
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect();
    (arguments.len() == count).then_some(arguments)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn arguments_of(process: i32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{process}/cmdline")).ok()?;
    let arguments: Vec<String> = bytes
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .map(|argument| String::from_utf8_lossy(argument).into_owned())
        .collect();
    (!arguments.is_empty()).then_some(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(arguments: &[&str]) -> Vec<String> {
        arguments.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn an_agent_is_known_by_its_programs_name_whatever_its_options_or_path() {
        assert_eq!(Agent::running(&words(&["claude"])), Some(Agent::Claude));
        assert_eq!(
            Agent::running(&words(&["claude", "--dangerously-skip-permissions"])),
            Some(Agent::Claude)
        );
        assert_eq!(
            Agent::running(&words(&["/Users/someone/.local/bin/claude", "--resume"])),
            Some(Agent::Claude)
        );
        assert_eq!(
            Agent::running(&words(&["node", "/opt/homebrew/bin/codex", "resume"])),
            Some(Agent::Codex)
        );
        assert_eq!(
            Agent::running(&words(&["node", "--no-warnings", "/x/bin/gemini.js"])),
            Some(Agent::Gemini)
        );
        assert_eq!(Agent::running(&words(&["agent"])), Some(Agent::Cursor));
        assert_eq!(Agent::running(&words(&["agy"])), Some(Agent::Antigravity));
        assert_eq!(Agent::running(&words(&["opencode"])), Some(Agent::OpenCode));
        // The shell, another program, a program that only mentions an agent, and nothing.
        assert_eq!(Agent::running(&words(&["-zsh"])), None);
        assert_eq!(Agent::running(&words(&["vim", "claude"])), None);
        assert_eq!(Agent::running(&words(&["node", "server.js"])), None);
        assert_eq!(Agent::running(&words(&["node"])), None);
        assert_eq!(Agent::running(&[]), None);
    }

    #[test]
    fn agents_keep_their_own_names_and_only_some_have_a_mark() {
        let named: Vec<_> = Agent::ALL
            .iter()
            .map(|agent| (agent.label(), agent.logo()))
            .collect();
        assert_eq!(
            named,
            [
                ("Claude Code", Some("claude-code")),
                ("Codex", Some("codex")),
                ("Cursor", Some("cursor")),
                ("Antigravity", Some("antigravity")),
                ("OpenCode", None),
                ("Gemini", None),
            ]
        );
    }

    #[test]
    fn the_foregrounds_agent_is_followed_as_programs_come_and_go() {
        let now = Instant::now();
        let mut watch = Watch::new(10);
        let none = |_: i32| -> Option<Vec<String>> { panic!("nothing is asked about") };
        // The shell itself: no agent, and nothing changed.
        assert_eq!(watch.look(10, now, none), None);
        // Claude Code takes the foreground, and is asked about once.
        assert_eq!(
            watch.look(20, now, |process| {
                assert_eq!(process, 20);
                Some(words(&["claude", "--resume"]))
            }),
            Some(Some(Agent::Claude))
        );
        assert_eq!(watch.look(20, now, none), None);
        // Back to the shell: no agent any more.
        assert_eq!(watch.look(10, now, none), Some(None));
        // A program still showing its shell's arguments is re-checked until it execs.
        assert_eq!(watch.look(30, now, |_| Some(words(&["-zsh"]))), None);
        assert_eq!(watch.look(30, now + ASK_EVERY / 2, none), None);
        assert_eq!(
            watch.look(30, now + ASK_EVERY, |_| Some(words(&["codex"]))),
            Some(Some(Agent::Codex))
        );
        // One agent after another without the shell between them.
        assert_eq!(
            watch.look(40, now, |_| Some(words(&["agy"]))),
            Some(Some(Agent::Antigravity))
        );
        // A program that has exited reports nothing.
        assert_eq!(watch.look(50, now, |_| None), Some(None));
    }

    #[test]
    fn a_program_that_is_no_agent_is_left_alone_once_it_has_settled() {
        let mut watch = Watch::new(10);
        let started = Instant::now();
        assert_eq!(watch.look(20, started, |_| Some(words(&["vim"]))), None);
        assert_eq!(
            watch.look(20, started + SETTLE, |_| Some(words(&["vim"]))),
            None
        );
        let later = |_: i32| -> Option<Vec<String>> { panic!("a settled program is not asked") };
        assert_eq!(watch.look(20, started + SETTLE * 2, later), None);
    }

    #[test]
    fn a_macs_record_of_a_process_is_read_into_its_arguments() {
        let mut record = 2_i32.to_ne_bytes().to_vec();
        record.extend_from_slice(b"/usr/local/bin/node\0\0\0\0node\0/opt/bin/codex\0HOME=/x\0");
        assert_eq!(
            read_process_arguments(&record),
            Some(words(&["node", "/opt/bin/codex"]))
        );
        // Fewer arguments than it says, or none of it at all.
        let mut short = 3_i32.to_ne_bytes().to_vec();
        short.extend_from_slice(b"/bin/sh\0sh\0");
        assert_eq!(read_process_arguments(&short), None);
        assert_eq!(read_process_arguments(&[]), None);
    }

    #[test]
    #[cfg(unix)]
    fn a_running_process_is_asked_for_its_arguments() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("start");
        let arguments = i32::try_from(child.id()).ok().and_then(arguments_of);
        child.kill().expect("stop");
        child.wait().expect("wait");
        assert_eq!(arguments, Some(words(&["/bin/sleep", "30"])));
    }
}
