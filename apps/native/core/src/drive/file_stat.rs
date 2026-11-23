//! File facts the drive needs beyond std's `Metadata`, with one API on every platform: identity
//! that survives renames (the drive's ids), last access, attribute change time, owner and
//! permissions. Unix reads the inode. On Windows the identity is the volume serial number plus
//! the file index (unstable in std), and the other values are not read.

use std::fs::Metadata;
use std::path::Path;

/// Seconds and nanoseconds since the Unix epoch.
pub type Moment = (i64, i64);

/// Device and inode, from which a file's id is made.
#[cfg(unix)]
pub fn identity(_path: &Path, stat: &Metadata) -> (String, String) {
    use std::os::unix::fs::MetadataExt;
    (stat.dev().to_string(), stat.ino().to_string())
}

/// Volume serial number and file index, read from the file (or from a link itself, as
/// `symlink_metadata` does). Empty when it can't be opened.
#[cfg(windows)]
pub fn identity(path: &Path, _stat: &Metadata) -> (String, String) {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        attributes: u32,
        created: FileTime,
        accessed: FileTime,
        written: FileTime,
        volume_serial_number: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }
    // A folder opens only with backup semantics; a link opens as itself.
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let Ok(file) = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    else {
        return (String::new(), String::new());
    };
    let mut information = ByHandleFileInformation::default();
    // SAFETY: an open handle, and a structure of the layout the call fills.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return (String::new(), String::new());
    }
    let index = (u64::from(information.index_high) << 32) | u64::from(information.index_low);
    (
        information.volume_serial_number.to_string(),
        index.to_string(),
    )
}

/// Modification time.
#[cfg(unix)]
pub fn modified(stat: &Metadata) -> Moment {
    use std::os::unix::fs::MetadataExt;
    (stat.mtime(), stat.mtime_nsec())
}

/// Modification time, as std reads it on Windows.
#[cfg(windows)]
pub fn modified(stat: &Metadata) -> Moment {
    let since = stat
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .unwrap_or_default();
    (seconds(since), i64::from(since.subsec_nanos()))
}

/// Last access time.
#[cfg(unix)]
pub fn accessed(stat: &Metadata) -> Option<Moment> {
    use std::os::unix::fs::MetadataExt;
    Some((stat.atime(), stat.atime_nsec()))
}

/// Last access time, as std reads it on Windows.
#[cfg(windows)]
pub fn accessed(stat: &Metadata) -> Option<Moment> {
    let since = stat
        .accessed()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some((seconds(since), i64::from(since.subsec_nanos())))
}

/// Attribute change time (`ctime`).
#[cfg(unix)]
pub fn changed(stat: &Metadata) -> Option<Moment> {
    use std::os::unix::fs::MetadataExt;
    Some((stat.ctime(), stat.ctime_nsec()))
}

/// Windows records no attribute change time.
#[cfg(windows)]
pub fn changed(_stat: &Metadata) -> Option<Moment> {
    None
}

/// Creation time in Unix seconds, clamped to the modification time on every platform as macOS
/// does. macOS moves the creation date back when the modification date is set earlier (a copy
/// that keeps its date, `touch -t`). NTFS and Linux file systems keep the copy time, which would
/// show a file created after its last change. `None` where the file system has no creation time.
pub fn created(stat: &Metadata) -> Option<i64> {
    let made = seconds(
        stat.created()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?,
    );
    Some(made.min(modified(stat).0))
}

/// Whole seconds of `since` from the epoch. Saturates at `i64::MAX` for times out of range.
pub fn seconds(since: std::time::Duration) -> i64 {
    i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
}

/// The owner's account name, or its uid when the account has no name.
#[cfg(unix)]
pub fn owner(stat: &Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    let uid = stat.uid();
    // SAFETY: getpwuid returns null or a pointer to a record valid until the next call on
    // this thread; the name is copied out at once.
    let record = unsafe { libc::getpwuid(uid) };
    if record.is_null() {
        return uid.to_string();
    }
    // SAFETY: a non-null record's name is a NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr((*record).pw_name) }
        .to_string_lossy()
        .into_owned()
}

/// A Windows file's owner is in its security descriptor, which is not read.
#[cfg(windows)]
pub fn owner(_stat: &Metadata) -> String {
    String::new()
}

/// Permission bits as four octal digits.
#[cfg(unix)]
pub fn permissions(stat: &Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!("{:04o}", stat.mode() & 0o7777)
}

/// Windows files have access lists, not permission bits.
#[cfg(windows)]
pub fn permissions(_stat: &Metadata) -> String {
    String::new()
}

/// Whether `cause` is a symlink loop: `ELOOP`, or `ERROR_CANT_RESOLVE_FILENAME` on Windows.
pub fn is_link_loop(cause: &std::io::Error) -> bool {
    #[cfg(unix)]
    let code = libc::ELOOP;
    #[cfg(windows)]
    let code = 1921;
    cause.raw_os_error() == Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_dated_back_was_made_no_later_than_that_date() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("old.txt");
        std::fs::write(&path, "old").expect("write");
        // 2024-03-14 09:26 UTC, as the scratch drive dates its files (`scripts/scratch-drive.sh`).
        let then = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_710_408_360);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(then)
            .expect("date");
        let stat = std::fs::metadata(&path).expect("stat");
        assert_eq!(created(&stat), Some(1_710_408_360));
    }

    #[test]
    fn a_file_keeps_its_identity_when_renamed_and_another_file_has_its_own() {
        let folder = tempfile::tempdir().expect("folder");
        let (first, second, moved) = (
            folder.path().join("a"),
            folder.path().join("b"),
            folder.path().join("c"),
        );
        std::fs::write(&first, "a").expect("write");
        std::fs::write(&second, "b").expect("write");
        let identity_of =
            |path: &Path| identity(path, &std::fs::symlink_metadata(path).expect("stat"));
        let kept = identity_of(&first);
        assert!(!kept.0.is_empty() && !kept.1.is_empty());
        assert_ne!(kept, identity_of(&second));
        std::fs::rename(&first, &moved).expect("rename");
        assert_eq!(identity_of(&moved), kept);
    }

    #[test]
    fn times_are_read() {
        let folder = tempfile::tempdir().expect("folder");
        let stat = std::fs::metadata(folder.path()).expect("stat");
        assert!(modified(&stat).0 > 0);
        assert!(accessed(&stat).is_some_and(|(seconds, _)| seconds > 0));
    }
}
