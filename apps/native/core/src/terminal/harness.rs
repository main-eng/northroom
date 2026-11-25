//! How a terminal learns what its agent is doing (ADR-0005). The session's environment gives
//! Claude Code an app-owned plugin whose hooks report each event to the terminal. It applies
//! only to programs started in that session; a person's own settings files are never read or
//! edited.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const HARNESS_DIRECTORY: &str = "terminal-harness";
const PLUGIN_DIRECTORY: &str = "claude-plugin";
const PLUGIN_MANIFEST: &str = ".claude-plugin/plugin.json";
const PLUGIN_HOOKS: &str = "hooks/hooks.json";
const ACTIVITY_HOOK_FILE: &str = "activity-hook.sh";
/// The variable Claude Code reads the folders of a session's plugins from.
const CLAUDE_PLUGINS_VARIABLE: &str = "CLAUDE_CODE_PLUGIN_DIRS";
/// A hook that cannot reach its terminal within this many seconds gives up, so it never blocks
/// the agent.
const HOOK_TIMEOUT_SECONDS: u32 = 5;
/// Files removed from the harness folder whenever the plugin is written.
const SUPERSEDED_FILES: [&str; 2] = ["claude-settings.json", "activity-hook.sh"];

/// One shell word: single-quoted, with any single quote closed, escaped and reopened.
pub fn quote_shell_word(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The hook Claude Code runs on each event. It writes the activity to the agent's terminal as
/// an escape sequence the core reads and the screen ignores, so no socket or port is needed,
/// and does nothing outside a Northroom terminal. Claude Code runs hooks without a terminal, so
/// the hook uses the terminal of its nearest ancestor that has one. It prints nothing for
/// Claude to read. After a tool that edits a file it also reports the file's path, read from
/// the event's JSON with the shell's own patterns (the first `"file_path":"` is the tool's, as
/// quotes inside JSON strings are escaped), for the Changes tab.
pub fn activity_hook_script() -> String {
    [
        "#!/bin/sh",
        "# Northroom terminal activity: tells this terminal's Board card what the agent is doing.",
        "[ -n \"$NORTHROOM_TERMINAL\" ] || exit 0",
        "input=$(cat)",
        "q='\"'",
        "# The text of the first JSON field named $1.",
        "field() {",
        "  for sep in \":$q\" \": $q\"; do",
        "    case \"$input\" in",
        "      *\"$q$1$q$sep\"*) rest=${input#*\"$q$1$q$sep\"}; printf '%s' \"${rest%%\"$q\"*}\"; return ;;",
        "    esac",
        "  done",
        "}",
        "edited=",
        "case \"$1\" in",
        "  SessionStart) activity=idle ;;",
        "  UserPromptSubmit) activity=working ;;",
        "  PostToolUse) activity=working",
        "    case $(field tool_name) in",
        "      Edit|Write|MultiEdit) edited=$(field file_path) ;;",
        "      NotebookEdit) edited=$(field notebook_path) ;;",
        "    esac ;;",
        "  PreToolUse) case \"$input\" in *AskUserQuestion*) activity=needs-you ;; *) activity=working ;; esac ;;",
        "  PermissionRequest) activity=needs-you ;;",
        "  Notification) case \"$input\" in *permission_prompt*|*agent_needs_input*) activity=needs-you ;; *) exit 0 ;; esac ;;",
        "  Stop) activity=finished ;;",
        "  *) exit 0 ;;",
        "esac",
        "pid=$PPID",
        "terminal=",
        "while [ -z \"$terminal\" ] && [ \"${pid:-1}\" -gt 1 ]; do",
        "  name=$(ps -o tty= -p \"$pid\" 2>/dev/null | tr -d ' ')",
        "  case \"$name\" in",
        "    ''|'?'|'??'|-) pid=$(ps -o ppid= -p \"$pid\" 2>/dev/null | tr -d ' ') ;;",
        "    *) terminal=/dev/$name ;;",
        "  esac",
        "done",
        "[ -n \"$terminal\" ] || exit 0",
        "printf '\\033]6973;northroom;activity=%s\\007' \"$activity\" > \"$terminal\" 2>/dev/null",
        "# The conversation and its folder, so a relaunch can resume it (terminal::resume).",
        "session=$(field session_id | tr -d '\\007\\033')",
        "folder=$(field cwd | tr -d '\\007\\033')",
        "case \"$folder\" in *\\\\*) folder= ;; esac",
        "[ -z \"$session\" ] || printf '\\033]6973;northroom;session=%s\\007' \"$session\" > \"$terminal\" 2>/dev/null",
        "[ -z \"$folder\" ] || printf '\\033]6973;northroom;session-dir=%s\\007' \"$folder\" > \"$terminal\" 2>/dev/null",
        "# An escaped path is left out rather than reported wrong.",
        "case \"$edited\" in *\\\\*) edited= ;; esac",
        "edited=$(printf '%s' \"$edited\" | tr -d '\\007\\033')",
        "[ -z \"$edited\" ] || printf '\\033]6973;northroom;edit=%s\\007' \"$edited\" > \"$terminal\" 2>/dev/null",
        "exit 0",
        "",
    ]
    .join("\n")
}

/// The plugin manifest Claude Code reads.
fn claude_plugin_manifest() -> String {
    let manifest = serde_json::json!({
        "name": "northroom",
        "version": "1.0.0",
        "description": "Tells a Northroom terminal what Claude Code is doing in it.",
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&manifest).unwrap_or_default()
    )
}

/// The plugin's hooks: one per event it reports, each naming its event for the script.
pub fn claude_hooks(hook_path: &Path) -> String {
    let command = |event: &str| {
        serde_json::json!({
            "type": "command",
            "command": format!("{} {event}", quote_shell_word(&hook_path.to_string_lossy())),
            "timeout": HOOK_TIMEOUT_SECONDS,
        })
    };
    let on_event = |event: &str| serde_json::json!([{ "hooks": [command(event)] }]);
    let on_tool = |event: &str| serde_json::json!([{ "matcher": "*", "hooks": [command(event)] }]);
    let mut hooks = serde_json::Map::new();
    hooks.insert("SessionStart".into(), on_event("SessionStart"));
    hooks.insert("UserPromptSubmit".into(), on_event("UserPromptSubmit"));
    hooks.insert("PreToolUse".into(), on_tool("PreToolUse"));
    hooks.insert("PostToolUse".into(), on_tool("PostToolUse"));
    hooks.insert("PermissionRequest".into(), on_tool("PermissionRequest"));
    hooks.insert("Notification".into(), on_event("Notification"));
    hooks.insert("Stop".into(), on_event("Stop"));
    let file = serde_json::json!({ "hooks": hooks });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&file).unwrap_or_default()
    )
}

/// Writes the plugin under `directory`, replacing older copies, and returns its folder.
pub fn write_harness_files(directory: &Path) -> io::Result<PathBuf> {
    let harness = directory.join(HARNESS_DIRECTORY);
    let plugin = harness.join(PLUGIN_DIRECTORY);
    let (manifest, hooks) = (plugin.join(PLUGIN_MANIFEST), plugin.join(PLUGIN_HOOKS));
    let hook = plugin.join(ACTIVITY_HOOK_FILE);
    let replace = |path: &Path, text: &str| -> io::Result<()> {
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder)?;
        }
        let staged = path.with_extension("writing");
        fs::write(&staged, text)?;
        fs::rename(staged, path)
    };
    replace(&hook, &activity_hook_script())?;
    // The hook runs as a program on a Mac and Linux; Windows reports no activity yet.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
    }
    replace(&manifest, &claude_plugin_manifest())?;
    replace(&hooks, &claude_hooks(&hook))?;
    for name in SUPERSEDED_FILES {
        match fs::remove_file(harness.join(name)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(plugin)
}

/// The environment that makes agents started in a session report their activity: Claude Code
/// loads the plugin at `plugin` while it runs there.
pub fn session_environment(plugin: &Path) -> Vec<(String, String)> {
    vec![(
        CLAUDE_PLUGINS_VARIABLE.to_owned(),
        plugin.to_string_lossy().into_owned(),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::terminal::activity::{Activity, Signals};
    #[cfg(unix)]
    use std::process::{Command, Stdio};

    #[test]
    fn the_plugin_names_one_hook_per_event() {
        let text = claude_hooks(Path::new("/x/it's/activity-hook.sh"));
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(
            parsed["hooks"]["Stop"][0]["hooks"][0]["command"],
            "'/x/it'\\''s/activity-hook.sh' Stop"
        );
        assert_eq!(parsed["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert_eq!(parsed["hooks"]["Stop"][0]["hooks"][0]["timeout"], 5);
        assert!(text.ends_with("}\n"));
        assert_eq!(
            parsed["hooks"].as_object().map(serde_json::Map::len),
            Some(7)
        );
    }

    #[test]
    fn the_plugin_is_written_whole_and_what_it_replaced_is_taken_away() {
        let directory = tempfile::tempdir().expect("directory");
        let harness = directory.path().join(HARNESS_DIRECTORY);
        fs::create_dir_all(&harness).expect("folder");
        fs::write(harness.join("claude-settings.json"), "{}").expect("an older file");
        let plugin = write_harness_files(directory.path()).expect("write");
        assert_eq!(plugin, harness.join(PLUGIN_DIRECTORY));
        let manifest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(plugin.join(PLUGIN_MANIFEST)).expect("read"))
                .expect("json");
        assert_eq!(manifest["name"], "northroom");
        let hooks = fs::read_to_string(plugin.join(PLUGIN_HOOKS)).expect("read");
        // Compared whole: JSON escapes a Windows path's backslashes.
        assert_eq!(hooks, claude_hooks(&plugin.join(ACTIVITY_HOOK_FILE)));
        assert!(!harness.join("claude-settings.json").exists());
        // Every new terminal rewrites the files over themselves.
        write_harness_files(directory.path()).expect("write again");
        assert_eq!(
            session_environment(&plugin),
            [(
                "CLAUDE_CODE_PLUGIN_DIRS".to_owned(),
                plugin.to_string_lossy().into_owned()
            )]
        );
    }

    #[cfg(unix)]
    #[derive(Clone, Copy)]
    enum TerminalContext {
        Northroom,
        OutsideNorthroom,
    }

    /// What `sh` runs as the agent: `perl` leaves the terminal's session, as Claude Code's
    /// hook runner does, and becomes the hook (`$0`) with its event (`$1`).
    #[cfg(unix)]
    const HOOK_WITH_NO_TERMINAL: &str =
        r#"perl -MPOSIX -e 'setsid() or die; exec @ARGV or die' "$0" "$1"; exit $?"#;

    /// A new pseudo-terminal: the non-blocking side the app reads, and the side a program uses as
    /// its terminal.
    #[cfg(unix)]
    fn open_terminal() -> (fs::File, std::os::fd::OwnedFd) {
        use std::os::fd::{FromRawFd, OwnedFd};
        let (mut reader, mut program) = (0, 0);
        // SAFETY: `openpty` fills the two descriptors it is given and takes nothing else;
        // `fcntl` then sets a flag on the first of them.
        let opened = unsafe {
            libc::openpty(
                &mut reader,
                &mut program,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
                && libc::fcntl(reader, libc::F_SETFL, libc::O_NONBLOCK) == 0
        };
        assert!(opened, "open a terminal");
        // SAFETY: both descriptors were opened just now and belong to nothing else.
        unsafe { (fs::File::from_raw_fd(reader), OwnedFd::from_raw_fd(program)) }
    }

    /// What reaches `terminal` until `agent` exits. It is read while the agent runs, as the app
    /// does, because macOS keeps an exiting session alive until its terminal output is drained.
    #[cfg(unix)]
    fn read_terminal(terminal: &mut fs::File, agent: &mut std::process::Child) -> String {
        use std::io::Read;
        let mut said = Vec::new();
        let mut piece = [0_u8; 256];
        let started = std::time::Instant::now();
        loop {
            assert!(started.elapsed().as_secs() < 30, "the hook did not end");
            // Check before reading, so output written before the agent exited is still read.
            let ended = agent.try_wait().expect("wait").is_some();
            while let Ok(count @ 1..) = terminal.read(&mut piece) {
                said.extend_from_slice(&piece[..count]);
            }
            if ended {
                return String::from_utf8_lossy(&said).into_owned();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Runs the written hook as Claude Code does, without a terminal, under an agent with a fresh
    /// terminal, and reads what reaches that terminal. Unix only; Windows reports no activity yet.
    #[cfg(unix)]
    fn report(
        directory: &Path,
        event: &str,
        input: &str,
        context: TerminalContext,
    ) -> Option<Activity> {
        report_changes(directory, event, input, context).activity
    }

    /// Everything the hook reported, as `report` runs it.
    #[cfg(unix)]
    fn report_changes(
        directory: &Path,
        event: &str,
        input: &str,
        context: TerminalContext,
    ) -> crate::terminal::activity::Changes {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;
        let hook = write_harness_files(directory)
            .expect("write")
            .join(ACTIVITY_HOOK_FILE);
        let (mut terminal, agent_terminal) = open_terminal();
        let agent_terminal_fd = agent_terminal.as_raw_fd();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", HOOK_WITH_NO_TERMINAL])
            .arg(&hook)
            .arg(event)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match context {
            TerminalContext::Northroom => {
                command.env("NORTHROOM_TERMINAL", "1").stdin(Stdio::piped());
            }
            // The hook ends before it reads: input piped to it here would race its exit.
            TerminalContext::OutsideNorthroom => {
                command
                    .env_remove("NORTHROOM_TERMINAL")
                    .stdin(Stdio::null());
            }
        }
        // SAFETY: between fork and exec the child only starts a session and takes the new
        // terminal as its own, two system calls on a descriptor the parent keeps open.
        unsafe {
            command.pre_exec(move || {
                let taken = libc::setsid() >= 0
                    && libc::ioctl(agent_terminal_fd, libc::TIOCSCTTY as _, 0) >= 0;
                if !taken {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut agent = command.spawn().expect("run the agent");
        if let Some(mut stream) = agent.stdin.take() {
            stream.write_all(input.as_bytes()).expect("input");
        }
        let said = read_terminal(&mut terminal, &mut agent);
        assert!(agent.wait().expect("wait").success());
        Signals::default().observe(&said)
    }

    #[test]
    #[cfg(unix)]
    fn after_an_edit_the_hook_reports_the_edited_file() {
        let directory = tempfile::tempdir().expect("directory");
        let at = directory.path();
        let edited =
            |input: &str| report_changes(at, "PostToolUse", input, TerminalContext::Northroom);
        let edit = edited(
            r#"{"session_id":"s","cwd":"/p","tool_name":"Edit","tool_input":{"file_path":"/p/my file.rs","old_string":"say \"file_path\":\"/wrong\"","new_string":"b"},"tool_response":{"filePath":"/p/my file.rs"}}"#,
        );
        assert_eq!(edit.activity, Some(Activity::Working));
        assert_eq!(edit.edits, ["/p/my file.rs"]);
        // Every event also says which conversation it is and where, to resume it later.
        assert_eq!(
            (
                edit.conversation.as_deref(),
                edit.conversation_folder.as_deref()
            ),
            (Some("s"), Some("/p"))
        );
        assert_eq!(
            edited(r#"{"tool_name": "Write", "tool_input": {"file_path": "/p/new.md"}}"#).edits,
            ["/p/new.md"]
        );
        assert_eq!(
            edited(r#"{"tool_name":"NotebookEdit","tool_input":{"notebook_path":"/p/n.ipynb"}}"#)
                .edits,
            ["/p/n.ipynb"]
        );
        // Another tool reports no file, nor does a path with an escape in it.
        let bash =
            edited(r#"{"tool_name":"Bash","tool_input":{"command":"echo \"file_path\":\"/x\""}}"#);
        assert_eq!(
            (bash.activity, bash.edits.len()),
            (Some(Activity::Working), 0)
        );
        assert!(
            edited(r#"{"tool_name":"Edit","tool_input":{"file_path":"/p/a\"b"}}"#)
                .edits
                .is_empty()
        );
    }

    #[test]
    #[cfg(unix)]
    fn the_hook_reports_each_event_and_stays_quiet_outside_northroom() {
        let directory = tempfile::tempdir().expect("directory");
        let at = directory.path();
        assert_eq!(
            report(at, "UserPromptSubmit", "{}", TerminalContext::Northroom),
            Some(Activity::Working)
        );
        assert_eq!(
            report(
                at,
                "PreToolUse",
                "{\"tool_name\":\"AskUserQuestion\"}",
                TerminalContext::Northroom
            ),
            Some(Activity::NeedsYou)
        );
        assert_eq!(
            report(
                at,
                "PreToolUse",
                "{\"tool_name\":\"Bash\"}",
                TerminalContext::Northroom
            ),
            Some(Activity::Working)
        );
        assert_eq!(
            report(at, "PermissionRequest", "{}", TerminalContext::Northroom),
            Some(Activity::NeedsYou)
        );
        assert_eq!(
            report(
                at,
                "Notification",
                "{\"type\":\"permission_prompt\"}",
                TerminalContext::Northroom
            ),
            Some(Activity::NeedsYou)
        );
        assert_eq!(
            report(
                at,
                "Notification",
                "{\"type\":\"other\"}",
                TerminalContext::Northroom
            ),
            None
        );
        assert_eq!(
            report(at, "Stop", "{}", TerminalContext::Northroom),
            Some(Activity::Finished)
        );
        assert_eq!(
            report(at, "SessionStart", "{}", TerminalContext::Northroom),
            Some(Activity::Idle)
        );
        assert_eq!(
            report(at, "Stop", "{}", TerminalContext::OutsideNorthroom),
            None,
            "outside a Northroom terminal it does nothing"
        );
    }
}
