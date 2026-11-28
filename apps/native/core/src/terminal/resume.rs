//! The command that opens an agent's conversation again after a relaunch, typed into the
//! terminal's first new shell: `claude --resume <id>`, `codex resume --last`, and so on, with
//! the flags the agent was started with. Every value that reaches the shell is checked or
//! quoted: the id comes from the agent's hook through the terminal's output.
//!
//! An agent whose hook reports its conversation (Claude Code) resumes that conversation. The
//! others resume the latest one in the folder they ran in, which each keeps per folder; two of
//! the same agent in one folder can't tell theirs apart, so neither resumes (`unclaimed`).

use crate::terminal::agent::Agent;

/// A flag that carries over to the resumed agent.
#[derive(Clone, Copy)]
enum Flag {
    /// A flag on its own (`--yolo`).
    Switch(&'static str),
    /// A flag followed by its value, as the next argument or after `=` (`--model opus`).
    Valued(&'static str),
}

impl Flag {
    fn name(self) -> &'static str {
        match self {
            Self::Switch(name) | Self::Valued(name) => name,
        }
    }
}

/// How one agent resumes: the arguments that pick a conversation, followed by its id
/// (`by_id`), the arguments that take the folder's latest, and the flags that carry over.
/// Every other argument, the first prompt included, is left out: a resumed agent waits for the
/// person.
struct Way {
    by_id: &'static [&'static str],
    latest: &'static [&'static str],
    flags: &'static [Flag],
}

fn way(agent: Agent) -> Way {
    use Flag::{Switch, Valued};
    match agent {
        Agent::Claude => Way {
            by_id: &["--resume"],
            latest: &["--continue"],
            flags: &[
                Switch("--dangerously-skip-permissions"),
                Valued("--permission-mode"),
                Valued("--model"),
                Valued("--add-dir"),
            ],
        },
        Agent::Codex => Way {
            by_id: &["resume"],
            latest: &["resume", "--last"],
            flags: &[
                Valued("--model"),
                Valued("-m"),
                Valued("--sandbox"),
                Valued("-s"),
                Valued("--ask-for-approval"),
                Valued("-a"),
                Switch("--dangerously-bypass-approvals-and-sandbox"),
                Valued("--add-dir"),
                Valued("--profile"),
                Valued("-p"),
                Switch("--search"),
            ],
        },
        Agent::Gemini => Way {
            by_id: &["--resume"],
            latest: &["--resume", "latest"],
            flags: &[
                Valued("--model"),
                Valued("-m"),
                Switch("--yolo"),
                Switch("-y"),
                Valued("--approval-mode"),
                Valued("--include-directories"),
                Switch("--sandbox"),
                Switch("-s"),
            ],
        },
        Agent::Cursor => Way {
            by_id: &["--resume"],
            latest: &["--continue"],
            flags: &[
                Valued("--model"),
                Switch("--force"),
                Switch("-f"),
                Switch("--yolo"),
                Valued("--sandbox"),
                Valued("--mode"),
                Switch("--plan"),
                Switch("--approve-mcps"),
                Switch("--trust"),
            ],
        },
        Agent::OpenCode => Way {
            by_id: &["--session"],
            latest: &["--continue"],
            flags: &[Valued("--model"), Valued("-m"), Valued("--agent")],
        },
        Agent::Antigravity => Way {
            by_id: &["--conversation"],
            latest: &["--continue"],
            flags: &[
                Switch("--dangerously-skip-permissions"),
                Valued("--mode"),
                Valued("--model"),
                Valued("--add-dir"),
                Valued("--effort"),
                Switch("--sandbox"),
            ],
        },
    }
}

/// The longest conversation id accepted.
const ID_MAX_CHARS: usize = 128;

/// Whether `id` can be a conversation id: letters, digits, '-' and '_', and not too long.
pub fn is_conversation_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_MAX_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What resumes one terminal's agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    /// The command typed into the terminal's first new shell.
    pub command: String,
    /// For a resume of the folder's latest conversation: the agent and folder, which no other
    /// terminal may share (`unclaimed`). `None` for a conversation named by its id.
    latest_in: Option<(Agent, String)>,
}

/// How `agent`, started as `launched` in `folder`, resumes: its `conversation` when its hook
/// reported one, else the latest in `folder`. `None` without a folder for the latest, or with
/// a conversation id that is not one.
pub fn command(
    agent: Agent,
    conversation: Option<&str>,
    folder: Option<&str>,
    launched: &[String],
) -> Option<Resume> {
    let way = way(agent);
    let (picked, latest_in): (Vec<String>, _) = match conversation {
        Some(id) if is_conversation_id(id) => (
            way.by_id
                .iter()
                .copied()
                .chain(std::iter::once(id))
                .map(str::to_owned)
                .collect(),
            None,
        ),
        Some(_) => return None,
        None => (
            way.latest.iter().map(|&word| word.to_owned()).collect(),
            Some((agent, folder?.to_owned())),
        ),
    };
    let mut words = vec![agent.started_as(launched).to_owned()];
    words.extend(picked);
    words.extend(carried(&way, launched));
    let run = words.join(" ");
    Some(Resume {
        command: match folder {
            Some(folder) => format!("cd {} && {run}", quoted(folder)),
            None => run,
        },
        latest_in,
    })
}

/// Which of `resumes` may run: all but those resuming the latest conversation of an agent in a
/// folder another of them shares, since each would take the same one.
pub fn unclaimed<T>(resumes: Vec<(T, Resume)>) -> Vec<(T, Resume)> {
    let shared = |key: &(Agent, String)| {
        resumes
            .iter()
            .filter(|(_, other)| other.latest_in.as_ref() == Some(key))
            .count()
            > 1
    };
    let keep: Vec<bool> = resumes
        .iter()
        .map(|(_, resume)| !resume.latest_in.as_ref().is_some_and(shared))
        .collect();
    resumes
        .into_iter()
        .zip(keep)
        .filter_map(|(each, kept)| kept.then_some(each))
        .collect()
}

/// The command that starts `agent` afresh as it was `launched`: the name it was started under
/// and the flags that carry over, without its first prompt or the conversation it opened.
pub fn again(agent: Agent, launched: &[String]) -> String {
    let mut words = vec![agent.started_as(launched).to_owned()];
    words.extend(carried(&way(agent), launched));
    words.join(" ")
}

/// The command (`again`) of the agent started most lately in any terminal since the app
/// started.
static STARTED_LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Records that `agent` took a terminal's foreground, started as `launched`.
pub fn started(agent: Agent, launched: &[String]) {
    use crate::Relock;
    *STARTED_LAST.relock() = Some(again(agent, launched));
}

/// The command of the agent started most lately, if one was since the app started.
pub fn started_last() -> Option<String> {
    use crate::Relock;
    STARTED_LAST.relock().clone()
}

/// The file in the app's own folder that keeps the last started agent's command across a quit.
const AGAIN_FILE: &str = "last-agent-command";

/// The longest kept command typed into a new terminal.
const AGAIN_MAX_CHARS: usize = 400;

/// Keeps `command` in `support` as the one a new terminal has typed for the person.
pub fn keep_again(support: &std::path::Path, command: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(support)?;
    std::fs::write(support.join(AGAIN_FILE), command)
}

/// The command kept in `support`, if it is one line a person could have typed: no control
/// characters, and not too long.
pub fn kept_again(support: &std::path::Path) -> Option<String> {
    let kept = std::fs::read_to_string(support.join(AGAIN_FILE)).ok()?;
    let command = kept.trim();
    let typeable = !command.is_empty()
        && command.chars().count() <= AGAIN_MAX_CHARS
        && !command.chars().any(char::is_control);
    typeable.then(|| command.to_owned())
}

/// The flags of `launched` that carry over (`Way::flags`), values quoted, in their order.
fn carried(way: &Way, launched: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut arguments = launched.iter();
    while let Some(argument) = arguments.next() {
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (argument.as_str(), None),
        };
        let Some(&flag) = way.flags.iter().find(|flag| flag.name() == name) else {
            continue;
        };
        match (flag, inline) {
            (Flag::Switch(name), None) => kept.push(name.to_owned()),
            (Flag::Switch(_), Some(_)) => {}
            (Flag::Valued(name), inline) => {
                if let Some(value) = inline.or_else(|| arguments.next().cloned()) {
                    kept.push(name.to_owned());
                    kept.push(quoted(&value));
                }
            }
        }
    }
    kept
}

/// `text` as one shell word: in single quotes, each quote in it closed and escaped.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(arguments: &[&str]) -> Vec<String> {
        arguments
            .iter()
            .map(|&argument| argument.to_owned())
            .collect()
    }

    fn resume_command(
        agent: Agent,
        id: Option<&str>,
        folder: Option<&str>,
        launched: &[&str],
    ) -> String {
        command(agent, id, folder, &words(launched))
            .expect("a resume")
            .command
    }

    #[test]
    fn an_agent_starts_again_under_its_name_with_its_flags_and_no_prompt_or_conversation() {
        assert_eq!(
            again(
                Agent::Claude,
                &words(&[
                    "node",
                    "/opt/bin/claude",
                    "--resume",
                    "3f2a-c91",
                    "--model",
                    "opus",
                    "--dangerously-skip-permissions",
                    "fix the pricing page; rm -rf ~",
                ]),
            ),
            "claude --model 'opus' --dangerously-skip-permissions"
        );
        assert_eq!(again(Agent::Codex, &words(&["codex"])), "codex");
    }

    #[test]
    fn the_kept_command_is_typed_only_if_it_is_one_plain_line() {
        let support = tempfile::tempdir().expect("support");
        assert_eq!(kept_again(support.path()), None);
        keep_again(support.path(), "claude --model 'opus'").expect("kept");
        assert_eq!(
            kept_again(support.path()).as_deref(),
            Some("claude --model 'opus'")
        );
        keep_again(support.path(), "claude\rrm -rf ~").expect("kept");
        assert_eq!(
            kept_again(support.path()),
            None,
            "a line break would run it"
        );
        keep_again(support.path(), &"x".repeat(AGAIN_MAX_CHARS + 1)).expect("kept");
        assert_eq!(kept_again(support.path()), None);
    }

    #[test]
    fn claude_resumes_its_conversation_in_its_folder_with_the_flags_it_started_with() {
        assert_eq!(
            resume_command(
                Agent::Claude,
                Some("3f2a-c91"),
                Some("/Users/me/My Work"),
                &[
                    "node",
                    "/opt/bin/claude",
                    "--dangerously-skip-permissions",
                    "--model=opus",
                    "--permission-mode",
                    "plan",
                    "fix the tests",
                ],
            ),
            "cd '/Users/me/My Work' && claude --resume 3f2a-c91 \
             --dangerously-skip-permissions --model 'opus' --permission-mode 'plan'"
        );
        // The first prompt is left out, so nothing is asked of it again.
        assert_eq!(
            resume_command(Agent::Claude, Some("s"), None, &["claude", "hello"]),
            "claude --resume s"
        );
        // Without a reported conversation it takes the folder's latest.
        assert_eq!(
            resume_command(Agent::Claude, None, Some("/p"), &["claude"]),
            "cd '/p' && claude --continue"
        );
    }

    #[test]
    fn every_agent_resumes_its_folder_s_latest_its_own_way() {
        let latest = |agent, launched: &[&str]| resume_command(agent, None, Some("/p"), launched);
        assert_eq!(
            latest(
                Agent::Codex,
                &[
                    "codex",
                    "-m",
                    "gpt-5",
                    "--dangerously-bypass-approvals-and-sandbox",
                    "fix it"
                ]
            ),
            "cd '/p' && codex resume --last -m 'gpt-5' --dangerously-bypass-approvals-and-sandbox"
        );
        assert_eq!(
            latest(Agent::Gemini, &["gemini", "--yolo", "--model", "pro"]),
            "cd '/p' && gemini --resume latest --yolo --model 'pro'"
        );
        // Cursor's CLI answers to either name; the one it was started under is kept.
        assert_eq!(
            latest(Agent::Cursor, &["cursor-agent", "--force", "--mode=plan"]),
            "cd '/p' && cursor-agent --continue --force --mode 'plan'"
        );
        assert_eq!(
            latest(Agent::Cursor, &["agent"]),
            "cd '/p' && agent --continue"
        );
        assert_eq!(
            latest(Agent::OpenCode, &["opencode", "--agent", "build"]),
            "cd '/p' && opencode --continue --agent 'build'"
        );
        assert_eq!(
            latest(
                Agent::Antigravity,
                &["agy", "--dangerously-skip-permissions"]
            ),
            "cd '/p' && agy --continue --dangerously-skip-permissions"
        );
        // Without a folder there is no latest to take.
        assert_eq!(command(Agent::Codex, None, None, &[]), None);
    }

    #[test]
    fn nothing_from_the_hook_reaches_the_shell_unquoted() {
        // An id that is not one gives no command at all.
        for id in ["", "a b", "x;rm -rf ~", "$(reboot)", &"a".repeat(129)] {
            assert_eq!(command(Agent::Claude, Some(id), None, &[]), None, "{id}");
        }
        assert_eq!(
            resume_command(
                Agent::Claude,
                Some("s"),
                Some("/p/it's here; ls"),
                &["claude", "--add-dir", "a'b"]
            ),
            r"cd '/p/it'\''s here; ls' && claude --resume s --add-dir 'a'\''b'"
        );
    }

    #[test]
    fn two_of_an_agent_in_one_folder_take_no_latest_but_named_ones_still_resume() {
        let resume = |agent, id: Option<&str>, folder: &str| {
            command(agent, id, Some(folder), &words(&["x"])).expect("a resume")
        };
        let resumes = vec![
            ("codex in a", resume(Agent::Codex, None, "/a")),
            ("codex in a again", resume(Agent::Codex, None, "/a")),
            ("codex in b", resume(Agent::Codex, None, "/b")),
            ("gemini in a", resume(Agent::Gemini, None, "/a")),
            ("claude in a", resume(Agent::Claude, Some("s1"), "/a")),
            ("claude in a again", resume(Agent::Claude, Some("s2"), "/a")),
        ];
        let kept: Vec<&str> = unclaimed(resumes)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            kept,
            [
                "codex in b",
                "gemini in a",
                "claude in a",
                "claude in a again"
            ]
        );
    }
}
