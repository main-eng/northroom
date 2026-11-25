//! The command that opens an agent's conversation again after a relaunch, typed into the
//! terminal's first new shell: `claude --resume <id>` with the flags it was started with. Only
//! Claude Code for now. The id and folder come from the agent's hook through the terminal's
//! output, so the id is checked and every value is quoted before it reaches the shell.

use crate::terminal::agent::Agent;

/// The flags that carry over to the resumed agent, each with whether it takes a value. Every
/// other argument, the first prompt included, is left out: a resumed agent waits for the
/// person.
const CLAUDE_FLAGS: [(&str, bool); 4] = [
    ("--dangerously-skip-permissions", false),
    ("--permission-mode", true),
    ("--model", true),
    ("--add-dir", true),
];

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

/// The command that resumes `agent`'s `conversation`, started in `folder` with `launched` as
/// its arguments. `None` for an agent without one, or an id that is not one.
pub fn command(
    agent: Agent,
    conversation: &str,
    folder: Option<&str>,
    launched: &[String],
) -> Option<String> {
    if agent != Agent::Claude || !is_conversation_id(conversation) {
        return None;
    }
    let mut words = vec![
        "claude".to_owned(),
        "--resume".to_owned(),
        conversation.to_owned(),
    ];
    words.extend(carried(launched));
    let run = words.join(" ");
    Some(match folder {
        Some(folder) => format!("cd {} && {run}", quoted(folder)),
        None => run,
    })
}

/// The flags of `launched` that carry over (`CLAUDE_FLAGS`), values quoted, in their order.
fn carried(launched: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut arguments = launched.iter();
    while let Some(argument) = arguments.next() {
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (argument.as_str(), None),
        };
        let Some(&(flag, takes_value)) = CLAUDE_FLAGS.iter().find(|(known, _)| *known == name)
        else {
            continue;
        };
        if !takes_value {
            kept.extend(inline.is_none().then(|| flag.to_owned()));
            continue;
        }
        if let Some(value) = inline.or_else(|| arguments.next().cloned()) {
            kept.push(flag.to_owned());
            kept.push(quoted(&value));
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

    #[test]
    fn claude_resumes_in_its_folder_with_the_flags_it_started_with() {
        let launched = words(&[
            "node",
            "/opt/bin/claude",
            "--dangerously-skip-permissions",
            "--model=opus",
            "--permission-mode",
            "plan",
            "fix the tests",
        ]);
        assert_eq!(
            command(
                Agent::Claude,
                "3f2a-c91",
                Some("/Users/me/My Work"),
                &launched
            )
            .as_deref(),
            Some(
                "cd '/Users/me/My Work' && claude --resume 3f2a-c91 \
                 --dangerously-skip-permissions --model 'opus' --permission-mode 'plan'"
            )
        );
        // The first prompt is left out, so nothing is asked of it again.
        assert_eq!(
            command(Agent::Claude, "s", None, &words(&["claude", "hello"])).as_deref(),
            Some("claude --resume s")
        );
    }

    #[test]
    fn nothing_from_the_hook_reaches_the_shell_unquoted() {
        // An id that is not one gives no command at all.
        for id in ["", "a b", "x;rm -rf ~", "$(reboot)", &"a".repeat(129)] {
            assert_eq!(command(Agent::Claude, id, None, &[]), None, "{id}");
        }
        let folder = "/p/it's here; ls";
        assert_eq!(
            command(
                Agent::Claude,
                "s",
                Some(folder),
                &words(&["claude", "--add-dir", "a'b"])
            )
            .as_deref(),
            Some(r"cd '/p/it'\''s here; ls' && claude --resume s --add-dir 'a'\''b'")
        );
    }

    #[test]
    fn only_claude_resumes_for_now() {
        assert_eq!(command(Agent::Codex, "s", None, &[]), None);
    }
}
