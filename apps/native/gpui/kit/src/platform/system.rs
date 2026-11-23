//! Parses an OS version string into the three numbers the diagnostic log records
//! (`diagnostics::Startup`). Each platform module supplies its own string (the macOS product
//! version, the Linux kernel release); Windows reports numbers directly.

/// The major, minor and patch numbers `words` starts with (`27.0`, `6.8.0-45-generic`). A
/// missing part is 0, matching `operatingSystemVersion`.
#[cfg(any(test, not(target_os = "windows")))]
pub fn version_parts(words: &str) -> (u32, u32, u32) {
    let numbered = words
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .map_or(words, |end| &words[..end]);
    let mut parts = numbered.split('.').map(|part| {
        part.parse()
            .expect("a version starts with numbers between dots")
    });
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::version_parts;

    #[test]
    fn a_version_is_read_as_the_platform_writes_it() {
        assert_eq!(version_parts("27.0"), (27, 0, 0));
        assert_eq!(version_parts("15.6.1"), (15, 6, 1));
        assert_eq!(version_parts("6.8.0-45-generic"), (6, 8, 0));
        assert_eq!(version_parts("6.12.10+deb13-amd64"), (6, 12, 10));
    }
}
