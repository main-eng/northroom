//! What a folder's `.gitignore` files leave out, for the common patterns: `name`, `dir/`,
//! `*.ext`, `/anchored`, `**` and `!negate`. A file's own folder's rules come after its
//! parents', so the deepest file wins, and within a file the last matching line wins, as in git.

/// The file of ignore rules a folder may hold.
pub const IGNORE_FILE: &str = ".gitignore";

/// Whether a path names a folder, which `dir/` patterns need.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    /// The folder of the `.gitignore` the rule came from, relative to the root ("" for the root).
    base: String,
    /// The pattern's parts between slashes.
    parts: Vec<String>,
    /// Matched against the whole path below `base`, not only the last name.
    anchored: bool,
    folders_only: bool,
    negated: bool,
}

/// The rules read so far in one root.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    rules: Vec<Rule>,
}

impl Rules {
    /// Adds the lines of the `.gitignore` in folder `base` ("" for the root). Parents must be
    /// added before their children.
    pub fn add(&mut self, base: &str, text: &str) {
        self.rules
            .extend(text.lines().filter_map(|line| parse_rule(base, line)));
    }

    /// Whether `path` (relative to the root, `/`-separated) is left out, by itself or by a folder
    /// above it.
    pub fn ignores(&self, path: &str, kind: Kind) -> bool {
        let mut above = String::new();
        let parts: Vec<&str> = path.split('/').collect();
        for folder in &parts[..parts.len() - 1] {
            if !above.is_empty() {
                above.push('/');
            }
            above.push_str(folder);
            if self.ignores_itself(&above, Kind::Folder) {
                return true;
            }
        }
        self.ignores_itself(path, kind)
    }

    /// Whether `path` itself matches, without looking at the folders above it.
    pub fn ignores_itself(&self, path: &str, kind: Kind) -> bool {
        let mut ignored = false;
        for rule in &self.rules {
            if rule.negated == ignored && rule.matches(path, kind) {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

/// One line of a `.gitignore` in `base`; `None` for a blank line or a comment.
fn parse_rule(base: &str, line: &str) -> Option<Rule> {
    let line = line.trim_end();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (negated, line) = match line.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, line.strip_prefix('\\').unwrap_or(line)),
    };
    let (folders_only, line) = match line.strip_suffix('/') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let anchored = line.contains('/');
    let line = line.strip_prefix('/').unwrap_or(line);
    if line.is_empty() {
        return None;
    }
    Some(Rule {
        base: base.to_owned(),
        parts: line.split('/').map(str::to_owned).collect(),
        anchored,
        folders_only,
        negated,
    })
}

impl Rule {
    fn matches(&self, path: &str, kind: Kind) -> bool {
        if self.folders_only && kind != Kind::Folder {
            return false;
        }
        let Some(below) = below_base(&self.base, path) else {
            return false;
        };
        let names: Vec<&str> = below.split('/').collect();
        if self.anchored {
            return match_parts(&self.parts, &names);
        }
        names
            .last()
            .is_some_and(|name| match_name(self.parts[0].as_bytes(), name.as_bytes()))
    }
}

/// `path` below folder `base`, or `None` when it is not inside it.
fn below_base<'a>(base: &str, path: &'a str) -> Option<&'a str> {
    if base.is_empty() {
        return Some(path);
    }
    path.strip_prefix(base)?.strip_prefix('/')
}

/// Whether the pattern's parts match the path's names; `**` stands for any number of names.
fn match_parts(parts: &[String], names: &[&str]) -> bool {
    match parts.split_first() {
        None => names.is_empty(),
        Some((part, rest)) if part == "**" => {
            (0..=names.len()).any(|skipped| match_parts(rest, &names[skipped..]))
        }
        Some((part, rest)) => names.split_first().is_some_and(|(name, others)| {
            match_name(part.as_bytes(), name.as_bytes()) && match_parts(rest, others)
        }),
    }
}

/// Whether one name matches one pattern part: `*` is any run of characters, `?` one, and
/// `[abc]` or `[a-z]` one of a set.
fn match_name(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|skipped| match_name(rest, &name[skipped..])),
        Some((b'?', rest)) => !name.is_empty() && match_name(rest, &name[1..]),
        Some((b'[', rest)) => match_set(rest, name),
        Some((byte, rest)) => name.first() == Some(byte) && match_name(rest, &name[1..]),
    }
}

/// A `[…]` set at the start of `pattern` (past its `[`) against the first byte of `name`. An
/// unclosed `[` is a plain character.
fn match_set(pattern: &[u8], name: &[u8]) -> bool {
    let Some(close) = pattern.iter().position(|&byte| byte == b']') else {
        return name.first() == Some(&b'[') && match_name(pattern, &name[1..]);
    };
    let Some(&first) = name.first() else {
        return false;
    };
    let (set, rest) = (&pattern[..close], &pattern[close + 1..]);
    let (negated, set) = match set.split_first() {
        Some((b'!' | b'^', others)) => (true, others),
        _ => (false, set),
    };
    in_set(set, first) != negated && match_name(rest, &name[1..])
}

fn in_set(set: &[u8], byte: u8) -> bool {
    let mut index = 0;
    while index < set.len() {
        if index + 2 < set.len() && set[index + 1] == b'-' {
            if (set[index]..=set[index + 2]).contains(&byte) {
                return true;
            }
            index += 3;
            continue;
        }
        if set[index] == byte {
            return true;
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str) -> Rules {
        let mut rules = Rules::default();
        rules.add("", text);
        rules
    }

    #[test]
    fn a_bare_name_matches_at_any_depth_and_a_slash_anchors_it() {
        let rules = rules("secret.txt\n/top.log\nlogs/\n# a comment\n\n");
        assert!(rules.ignores("secret.txt", Kind::File));
        assert!(rules.ignores("deep/down/secret.txt", Kind::File));
        assert!(rules.ignores("top.log", Kind::File));
        assert!(!rules.ignores("deep/top.log", Kind::File));
        // `logs/` is only a folder, and everything in it goes with it.
        assert!(rules.ignores("a/logs", Kind::Folder));
        assert!(!rules.ignores("a/logs", Kind::File));
        assert!(rules.ignores("a/logs/today.txt", Kind::File));
        assert!(!rules.ignores("# a comment", Kind::File));
    }

    #[test]
    fn globs_and_double_stars_match_as_git_does() {
        let rules = rules("*.o\nbuild-?\ndocs/**/*.pdf\n**/cache\n[ab]x.txt\nv[0-9].bin\n");
        assert!(rules.ignores("src/main.o", Kind::File));
        assert!(!rules.ignores("src/main.oo", Kind::File));
        assert!(rules.ignores("build-1", Kind::Folder));
        assert!(!rules.ignores("build-10", Kind::Folder));
        assert!(rules.ignores("docs/a.pdf", Kind::File));
        assert!(rules.ignores("docs/x/y/a.pdf", Kind::File));
        assert!(!rules.ignores("other/docs/a.pdf", Kind::File));
        assert!(rules.ignores("one/two/cache", Kind::Folder));
        assert!(rules.ignores("bx.txt", Kind::File) && !rules.ignores("cx.txt", Kind::File));
        assert!(rules.ignores("v7.bin", Kind::File) && !rules.ignores("vx.bin", Kind::File));
    }

    #[test]
    fn a_later_negation_takes_a_file_back_and_deeper_files_win() {
        let mut rules = rules("*.log\n!keep.log\n");
        assert!(rules.ignores("a.log", Kind::File));
        assert!(!rules.ignores("keep.log", Kind::File));
        rules.add("app", "!debug.log\n/local.txt\n");
        assert!(!rules.ignores("app/debug.log", Kind::File));
        assert!(rules.ignores("debug.log", Kind::File));
        assert!(rules.ignores("app/local.txt", Kind::File));
        assert!(!rules.ignores("app/x/local.txt", Kind::File));
        assert!(!rules.ignores("local.txt", Kind::File));
    }

    #[test]
    fn an_escaped_first_character_is_plain() {
        let rules = rules("\\#notes\n\\!bang\n");
        assert!(rules.ignores("#notes", Kind::File));
        assert!(rules.ignores("!bang", Kind::File));
    }
}
