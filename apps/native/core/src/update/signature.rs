//! Linux update signatures. The AppImage is signed with minisign (Ed25519 over the file's
//! BLAKE2b-512 and over a trusted comment), as Tauri's updater signs files, and the channel file
//! carries the base64 `.minisig` beside the digest. The public key is built into the app
//! (`update-key.pub`; DECISIONS.md, ADR-0010), so a feed or server cannot substitute another.
//! Verification uses minisign-verify (MIT, no dependencies); Ed25519 is not implemented here.
//!
//! The signed trusted comment carries `version:<version>`, which must equal the version the
//! channel file offers, itself newer than the running app. A feed serving an older signed
//! AppImage under a newer version is refused, so control of the feed cannot downgrade the app.

use minisign_verify::{Error, PublicKey, Signature};
use std::io::Read;
use std::path::Path;

/// The Linux update public key as a minisign `.pub` file. Empty in a build without one, which
/// refuses every update.
pub const BUILT_IN_KEY: &str = include_str!("update-key.pub");

/// Why a file's signature was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The app has no key, or its key is unreadable.
    NoKey,
    /// The channel file has no signature for the file, or it is unreadable.
    Unreadable(String),
    /// The signature is not from the key, or not for this file.
    Failed(String),
}

/// Parses a key from a minisign `.pub` file (comment line, then key) or the key line alone.
fn key_of(text: &str) -> Option<PublicKey> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.contains('\n') {
        PublicKey::decode(text).ok()
    } else {
        PublicKey::from_base64(text).ok()
    }
}

/// Whether a trusted comment names `version` (`version:<version>` among its fields).
fn names_version(trusted_comment: &str, version: &str) -> bool {
    trusted_comment
        .split(['\t', ' '])
        .any(|field| field.strip_prefix("version:") == Some(version))
}

/// Checks `file` against `signature` (base64 `.minisig`) and `key`, and that the signature names
/// `version`. Only prehashed signatures, minisign's default since 0.8, are accepted.
pub fn check_file(
    file: &Path,
    signature: Option<&str>,
    key: &str,
    version: &str,
) -> Result<(), Refusal> {
    let key = key_of(key).ok_or(Refusal::NoKey)?;
    let signature =
        signature.ok_or_else(|| Refusal::Unreadable("the update names no signature".into()))?;
    let text = decoded_base64(signature.trim())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| Refusal::Unreadable("the update's signature is not base64 text".into()))?;
    let signature = Signature::decode(&text).map_err(|error| {
        Refusal::Unreadable(format!("the update's signature cannot be read: {error}"))
    })?;
    let mut verifier = key.verify_stream(&signature).map_err(failed)?;
    let mut opened =
        std::fs::File::open(file).map_err(|error| Refusal::Unreadable(error.to_string()))?;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = opened
            .read(&mut buffer)
            .map_err(|error| Refusal::Unreadable(error.to_string()))?;
        if read == 0 {
            break;
        }
        verifier.update(&buffer[..read]);
    }
    verifier.finalize().map_err(failed)?;
    // The trusted comment is authentic: its global signature was verified above.
    if !names_version(signature.trusted_comment(), version) {
        return Err(Refusal::Failed(format!(
            "the signature is not of version {version} (it says \"{}\")",
            signature.trusted_comment()
        )));
    }
    Ok(())
}

fn failed(error: Error) -> Refusal {
    Refusal::Failed(match error {
        Error::UnexpectedKeyId => "the update is signed with another key".to_owned(),
        Error::InvalidSignature => "the signature does not match the update".to_owned(),
        Error::UnsupportedLegacyMode | Error::UnexpectedAlgorithm => {
            "the update is signed the legacy way (not prehashed)".to_owned()
        }
        other => other.to_string(),
    })
}

/// Decodes standard padded base64, as `base64` and minisign write it.
fn decoded_base64(text: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = text
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || chunk[..4 - padding].contains(&b'=') {
            return None;
        }
        let mut group = 0u32;
        for byte in &chunk[..4 - padding] {
            group = group << 6 | value(*byte)?;
        }
        group <<= 6 * padding as u32;
        let decoded = [(group >> 16) as u8, (group >> 8) as u8, group as u8];
        out.extend_from_slice(&decoded[..3 - padding]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two keys and a signature in minisign format, generated for these tests with OpenSSL's
    /// Ed25519. The secret keys were discarded.
    const KEY: &str = include_str!("signature_fixtures/key.pub");
    const OTHER_KEY: &str = include_str!("signature_fixtures/other.pub");
    const SIGNED: &[u8] = b"Northroom 0.0.2 AppImage\n";
    const SIGNATURE: &str = include_str!("signature_fixtures/signed.minisig");

    fn base64(text: &str) -> String {
        super::super::sha512::base64(text.as_bytes())
    }

    fn file(dir: &Path, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join("Northroom-0.0.2-x86_64.AppImage");
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_file_signed_with_the_key_passes() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), SIGNED);
        assert_eq!(
            check_file(&path, Some(&base64(SIGNATURE)), KEY, "0.0.2"),
            Ok(())
        );
        // The bare key line, as a check passes it.
        let line = KEY.lines().last().unwrap();
        assert_eq!(
            check_file(&path, Some(&base64(SIGNATURE)), line, "0.0.2"),
            Ok(())
        );
    }

    #[test]
    fn a_changed_file_another_key_or_no_key_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let signature = base64(SIGNATURE);
        let changed = file(dir.path(), b"Northroom 0.0.2 AppImagE\n");
        assert_eq!(
            check_file(&changed, Some(&signature), KEY, "0.0.2"),
            Err(Refusal::Failed(
                "the signature does not match the update".into()
            ))
        );
        let path = file(dir.path(), SIGNED);
        assert_eq!(
            check_file(&path, Some(&signature), OTHER_KEY, "0.0.2"),
            Err(Refusal::Failed(
                "the update is signed with another key".into()
            ))
        );
        assert_eq!(
            check_file(&path, Some(&signature), "", "0.0.2"),
            Err(Refusal::NoKey)
        );
        assert_eq!(
            check_file(&path, Some(&signature), "not a key", "0.0.2"),
            Err(Refusal::NoKey)
        );
        assert!(matches!(
            check_file(&path, None, KEY, "0.0.2"),
            Err(Refusal::Unreadable(_))
        ));
        assert!(matches!(
            check_file(&path, Some("%%%%"), KEY, "0.0.2"),
            Err(Refusal::Unreadable(_))
        ));
        // A signature whose trusted comment was changed fails its global signature.
        let edited = SIGNATURE.replace("file:", "File:");
        assert_eq!(
            check_file(&path, Some(&base64(&edited)), KEY, "0.0.2"),
            Err(Refusal::Failed(
                "the signature does not match the update".into()
            ))
        );
    }

    #[test]
    fn a_signature_of_another_version_is_refused() {
        // A feed offering 0.0.3 that serves the signed 0.0.2.
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), SIGNED);
        let Err(Refusal::Failed(reason)) =
            check_file(&path, Some(&base64(SIGNATURE)), KEY, "0.0.3")
        else {
            panic!("refused")
        };
        assert!(
            reason.starts_with("the signature is not of version 0.0.3"),
            "{reason}"
        );
        assert!(names_version("file:x\tversion:0.0.2", "0.0.2"));
        assert!(!names_version("file:x version:0.0.20", "0.0.2"));
        assert!(!names_version("file:version:0.0.2", "0.0.2"));
    }

    #[test]
    fn base64_reads_what_it_writes() {
        for text in ["", "a", "ab", "abc", "abcd", "untrusted comment: x\n"] {
            assert_eq!(
                decoded_base64(&base64(text)).as_deref(),
                Some(text.as_bytes())
            );
        }
        assert_eq!(decoded_base64("abc"), None);
        assert_eq!(decoded_base64("a=bc"), None);
    }
}
