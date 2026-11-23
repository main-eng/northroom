//! Feed versions: semver 2.0, parsed and ordered as node's `semver` 7.7 does in strict mode. A
//! leading `v` is allowed, surrounding spaces are trimmed, and build metadata does not affect
//! order.

use std::cmp::Ordering;

/// A prerelease identifier. Numbers sort before words, as in semver; the derived order relies on
/// the variant order below.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    Number(u64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Vec<Identifier>,
}

/// `0|[1-9]\d*`, no larger than JavaScript's largest safe integer.
fn number(text: &str) -> Option<u64> {
    let valid = text == "0"
        || (!text.is_empty()
            && !text.starts_with('0')
            && text.bytes().all(|byte| byte.is_ascii_digit()));
    let value: u64 = if valid {
        text.parse().ok()?
    } else {
        return None;
    };
    (value < (1 << 53)).then_some(value)
}

fn letter_dash_number(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

/// Whether `part` is a valid prerelease or build identifier: non-empty letters, digits and
/// dashes.
fn is_identifier(part: &str) -> bool {
    !part.is_empty() && part.bytes().all(letter_dash_number)
}

/// Parses a prerelease identifier, numeric when all digits.
fn identifier(part: &str) -> Option<Identifier> {
    if !is_identifier(part) {
        None
    } else if part.bytes().all(|byte| byte.is_ascii_digit()) {
        number(part).map(Identifier::Number)
    } else {
        Some(Identifier::Text(part.to_owned()))
    }
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        if text.len() > 256 {
            return None;
        }
        let text = text.strip_prefix('v').unwrap_or(text);
        let (text, build) = match text.split_once('+') {
            Some((before, build)) => (before, Some(build)),
            None => (text, None),
        };
        if build.is_some_and(|build| !build.split('.').all(is_identifier)) {
            return None;
        }
        let (main, prerelease) = match text.split_once('-') {
            Some((main, prerelease)) => (main, Some(prerelease)),
            None => (text, None),
        };
        let mut parts = main.split('.');
        let (major, minor, patch) = (
            number(parts.next()?)?,
            number(parts.next()?)?,
            number(parts.next()?)?,
        );
        if parts.next().is_some() {
            return None;
        }
        let prerelease = match prerelease {
            None => Vec::new(),
            Some(text) => text
                .split('.')
                .map(identifier)
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Version {
            major,
            minor,
            patch,
            prerelease,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(
                // A release sorts after its prereleases. Prereleases compare part by part, and
                // the shorter sorts first when the shared parts agree.
                || match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (false, false) => self.prerelease.cmp(&other.prerelease),
                },
            )
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::Version;

    #[test]
    fn refuses_what_strict_semver_refuses() {
        for text in [
            "1.0",
            "01.0.0",
            "1.0.0-",
            "1.0.0-01",
            "1.0.0+",
            "1.0.0-a..b",
            "=1.0.0",
            "1.0.0.0",
            "",
            "1.0.0-a_b",
        ] {
            assert!(Version::parse(text).is_none(), "{text}");
        }
        assert!(Version::parse(" v1.2.3-rc.1+build.5 ").is_some());
    }
}
