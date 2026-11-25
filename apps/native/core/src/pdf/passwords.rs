//! Passwords the person typed for locked PDFs, kept in memory for the session by file and
//! never written anywhere, so a locked file asks again next time the app starts.

use crate::Relock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static PASSWORDS: Mutex<Option<HashMap<PathBuf, Vec<u8>>>> = Mutex::new(None);

/// Keeps `password` for the file at `path` until the app quits.
pub fn remember(path: &Path, password: &[u8]) {
    PASSWORDS
        .relock()
        .get_or_insert_with(HashMap::new)
        .insert(path.to_path_buf(), password.to_vec());
}

/// The password typed for `path`, if any.
pub fn of(path: &Path) -> Option<Vec<u8>> {
    PASSWORDS.relock().as_ref()?.get(path).cloned()
}

/// Forgets the password for `path`, as when it didn't open the file.
pub fn forget(path: &Path) {
    if let Some(passwords) = PASSWORDS.relock().as_mut() {
        passwords.remove(path);
    }
}
