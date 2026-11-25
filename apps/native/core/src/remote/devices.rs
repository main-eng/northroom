//! The phones paired with this computer and this computer's own key, kept in one file in the
//! app's folder. A device is paired once, by reading a code off this screen (`pairing`), and
//! stays paired until the person forgets it. The file holds a private key, so only the person
//! may read it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's name in the app's folder.
pub const FILE: &str = "phone-link.json";
/// The longest name a device is listed under.
const NAME_MOST: usize = 60;

/// A device's or this computer's key: 32 bytes, written as 64 hex digits.
pub type Key = [u8; 32];

pub fn key_hex(key: &Key) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Reads a key written by `key_hex`.
pub fn parse_key_hex(text: &str) -> Option<Key> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(key)
}

/// A paired device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Its public key, as `key_hex` writes it.
    pub key: String,
    /// What the device called itself when it paired.
    pub name: String,
}

#[derive(Serialize, Deserialize)]
struct Kept {
    secret: String,
    devices: Vec<Device>,
}

/// This computer's key and its paired devices, saved as they change.
pub struct Devices {
    path: PathBuf,
    secret: Key,
    devices: Vec<Device>,
}

/// A device's name as listed: without control characters or outer spaces, at most `NAME_MOST`
/// characters, and never empty.
fn normalize_name(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !c.is_control()).collect();
    let short: String = clean.trim().chars().take(NAME_MOST).collect();
    if short.is_empty() {
        "Phone".to_owned()
    } else {
        short
    }
}

fn read(path: &Path) -> Option<(Key, Vec<Device>)> {
    let kept: Kept = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some((parse_key_hex(&kept.secret)?, kept.devices))
}

/// Writes `bytes` to `path` so that only the person can read them, replacing the file whole.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let staged = path.with_extension("writing");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&staged)?, bytes)?;
    std::fs::rename(staged, path)
}

impl Devices {
    /// What `path` keeps. With no readable file there, this computer takes the key `fresh`
    /// makes and has no devices; the file is written when the first device pairs.
    pub fn open(path: &Path, fresh: impl FnOnce() -> Key) -> Self {
        let (secret, devices) = read(path).unwrap_or_else(|| (fresh(), Vec::new()));
        Self {
            path: path.to_owned(),
            secret,
            devices,
        }
    }

    /// This computer's private key.
    pub fn secret(&self) -> Key {
        self.secret
    }

    pub fn list(&self) -> &[Device] {
        &self.devices
    }

    pub fn is_paired(&self, key: &Key) -> bool {
        let key = key_hex(key);
        self.devices.iter().any(|device| device.key == key)
    }

    /// Pairs the device with `key`, or renames it when it is already paired.
    pub fn pair(&mut self, key: &Key, name: &str) -> std::io::Result<()> {
        let key = key_hex(key);
        self.devices.retain(|device| device.key != key);
        self.devices.push(Device {
            key,
            name: normalize_name(name),
        });
        self.save()
    }

    /// Forgets the device with `key`. It must pair again to connect.
    pub fn forget(&mut self, key: &Key) -> std::io::Result<()> {
        let key = key_hex(key);
        self.devices.retain(|device| device.key != key);
        self.save()
    }

    fn save(&self) -> std::io::Result<()> {
        let kept = Kept {
            secret: key_hex(&self.secret),
            devices: self.devices.clone(),
        };
        write_private(&self.path, &serde_json::to_vec_pretty(&kept)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_reads_back_from_its_hex() {
        let key: Key = std::array::from_fn(|index| index as u8 * 7);
        assert_eq!(parse_key_hex(&key_hex(&key)), Some(key));
        assert_eq!(parse_key_hex("abc"), None);
        assert_eq!(parse_key_hex(&"zz".repeat(32)), None);
        assert_eq!(parse_key_hex(&"é".repeat(32)), None);
    }

    #[test]
    fn a_paired_device_is_still_paired_after_a_restart() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(FILE);
        let mut devices = Devices::open(&path, || [7; 32]);
        assert!(!path.exists());
        devices.pair(&[1; 32], "  My\u{7} phone ").unwrap();

        let reopened = Devices::open(&path, || [9; 32]);
        assert_eq!(reopened.secret(), [7; 32]);
        assert!(reopened.is_paired(&[1; 32]));
        assert!(!reopened.is_paired(&[2; 32]));
        assert_eq!(reopened.list()[0].name, "My phone");
    }

    #[test]
    fn pairing_again_renames_and_forgetting_unpairs() {
        let folder = tempfile::tempdir().unwrap();
        let mut devices = Devices::open(&folder.path().join(FILE), || [7; 32]);
        devices.pair(&[1; 32], "Old").unwrap();
        devices.pair(&[1; 32], "").unwrap();
        assert_eq!(devices.list().len(), 1);
        assert_eq!(devices.list()[0].name, "Phone");
        devices.forget(&[1; 32]).unwrap();
        assert!(!devices.is_paired(&[1; 32]));
    }

    #[cfg(unix)]
    #[test]
    fn only_the_person_can_read_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(FILE);
        Devices::open(&path, || [7; 32])
            .pair(&[1; 32], "Phone")
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
