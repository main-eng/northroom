//! Compress, like Finder's: writes files and folders into one zip archive next to them, for
//! sites and apps that accept a file but not a folder.
//!
//! One item `X` becomes `X.zip` (a file keeps its extension: `photo.png.zip`); several become
//! `Archive.zip` in the interface language. A taken name is numbered the way the drive numbers
//! new files ("Archive 2.zip"). The archive is written under a hidden name and renamed only once
//! complete, so a failure leaves nothing and any visible zip is whole.
//!
//! The output opens everywhere: files deflated, folders as their own entries, symlinks kept as
//! symlinks, UTF-8 names, local times, Unix permissions, and ZIP64 records wherever a size,
//! offset or entry count overflows the standard fields. Files are streamed from disk, never
//! loaded whole. Like Finder, it skips `.DS_Store` but keeps other hidden files.

use crate::strings::{self, key};
use flate2::write::DeflateEncoder;
use flate2::{Compression, Crc, CrcReader};
use std::collections::HashSet;
use std::fs::{self, File, Metadata};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const ZIP_EXTENSION: &str = ".zip";
/// Finder metadata, never archived.
const LEFT_OUT_NAME: &str = ".DS_Store";
/// Prefix of an in-progress archive: hidden, and never added to another archive.
const TEMPORARY_PREFIX: &str = ".northroom-compress-";
const TEMPORARY_SUFFIX: &str = ".part";
/// How many numbered names ("photo 2.png") to try for a clashing top-level item.
const NUMBERED_NAME_ATTEMPTS: u32 = 1000;

/// Zip record signatures, shared with the workbook reader.
pub(crate) const LOCAL_HEADER_SIGNATURE: u32 = 0x0403_4b50;
pub(crate) const CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4b50;
const ZIP64_END_SIGNATURE: u32 = 0x0606_4b50;
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
pub(crate) const END_SIGNATURE: u32 = 0x0605_4b50;
/// The ZIP64 extra field's id.
const ZIP64_EXTRA_ID: u16 = 0x0001;
/// General purpose bit 11: the entry's name is UTF-8.
const UTF8_NAME_FLAG: u16 = 1 << 11;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;
/// Made by Unix (3 in the high byte), zip spec 4.5 (the ZIP64 version).
const MADE_BY: u16 = (3 << 8) | 45;
/// Version needed to extract: 2.0 for deflate and folders, 4.5 for ZIP64.
const NEEDED_PLAIN: u16 = 20;
const NEEDED_ZIP64: u16 = 45;
/// MS-DOS directory attribute, in the low byte of the external attributes.
const DOS_FOLDER_ATTRIBUTE: u32 = 0x10;
/// Size of a local header before its name, and the CRC-32's offset within it.
const LOCAL_HEADER_BYTES: u64 = 30;
const LOCAL_CRC_AT: u64 = 14;
/// Size of the ZIP64 extra field's id and length.
const EXTRA_HEADER_BYTES: u64 = 4;
/// Size of the ZIP64 end record after its size field.
const ZIP64_END_RECORD_BYTES: u64 = 44;
/// The range of years an MS-DOS date can hold.
const DOS_FIRST_YEAR: i32 = 1980;
const DOS_LAST_YEAR: i32 = 2107;

/// Thresholds at which values move from standard fields to ZIP64 records.
#[derive(Clone, Copy, Debug)]
struct Zip64From {
    /// For sizes and offsets in 32-bit fields.
    bytes: u64,
    /// For the entry count in a 16-bit field.
    entries: u64,
    /// Files this large get a ZIP64 field in their local header, which is written before
    /// deflating. Deflate adds at most 5 bytes per 64 KiB, so a file under 3.75 GiB stays under
    /// 4 GiB compressed.
    file_bytes: u64,
}

const ZIP64_FROM: Zip64From = Zip64From {
    bytes: 0xFFFF_FFFF,
    entries: 0xFFFF,
    file_bytes: 0xF000_0000,
};

/// The archive name for `sources`: the single item's full name plus ".zip", or
/// "Archive.zip" in `language` for several.
pub fn archive_name(sources: &[PathBuf], language: &str) -> String {
    let stem = match sources {
        [one] => one
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        _ => strings::text(language, key::DRIVE_ARCHIVE_NAME).to_owned(),
    };
    format!("{stem}{ZIP_EXTENSION}")
}

/// Items to compress into one archive, placed in the first item's folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compress {
    pub sources: Vec<PathBuf>,
    pub directory: PathBuf,
    pub name: String,
}

impl Compress {
    /// Plans an archive of `sources` named in `language`. `None` if `sources` is empty or the
    /// first is the disk root.
    pub fn of(sources: Vec<PathBuf>, language: &str) -> Option<Self> {
        let directory = sources.first()?.parent()?.to_path_buf();
        let name = archive_name(&sources, language);
        Some(Self {
            sources,
            directory,
            name,
        })
    }

    /// Writes the archive under a hidden name, then renames it, numbering the name if taken.
    /// A failure leaves nothing. Returns the archive's path.
    pub fn run(&self) -> io::Result<PathBuf> {
        let temporary = self.directory.join(format!(
            "{TEMPORARY_PREFIX}{}{TEMPORARY_SUFFIX}",
            uuid::Uuid::new_v4().simple()
        ));
        let written = File::create_new(&temporary)
            .and_then(|file| write_zip(&self.sources, file, ZIP64_FROM))
            .and_then(|()| place(&temporary, &self.directory, &self.name));
        if written.is_err() {
            match fs::remove_file(&temporary) {
                Err(cause) if cause.kind() != io::ErrorKind::NotFound => eprintln!(
                    "an unfinished archive was left at {}: {cause}",
                    temporary.display()
                ),
                _ => {}
            }
        }
        written
    }
}

/// Moves `temporary` into `directory` as `name`, or "stem 2.ext", "stem 3.ext"… if taken,
/// numbered the way the drive numbers new files. Never replaces a file. On error the temporary
/// file stays put.
fn place(temporary: &Path, directory: &Path, name: &str) -> io::Result<PathBuf> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a file name",
        ));
    }
    let extension = Path::new(name)
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let stem = &name[..name.len() - extension.len()];
    for attempt in 1..=NUMBERED_NAME_ATTEMPTS {
        let candidate = if attempt == 1 {
            name.to_owned()
        } else {
            format!("{stem} {attempt}{extension}")
        };
        let destination = directory.join(candidate);
        match rename_new(temporary, &destination) {
            Ok(()) => return Ok(destination),
            Err(cause) if cause.kind() == io::ErrorKind::AlreadyExists => {}
            Err(cause) => return Err(cause),
        }
    }
    // Every numbered name is taken: fall back to a unique name so the archive is still visible.
    let destination = directory.join(format!(
        "{stem} {}{extension}",
        uuid::Uuid::new_v4().simple()
    ));
    rename_new(temporary, &destination).map(|()| destination)
}

/// Renames `from` to `to` only if `to` does not exist.
#[cfg(target_os = "macos")]
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path =
        |path: &Path| std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other);
    let (source, destination) = (c_path(from)?, c_path(to)?);
    // SAFETY: both pointers are NUL-terminated strings that live across the call.
    if unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) } == 0 {
        return Ok(());
    }
    let cause = io::Error::last_os_error();
    if cause.raw_os_error() != Some(libc::ENOTSUP) {
        return Err(cause);
    }
    // The volume lacks exclusive rename: check, then move.
    rename_after_looking(from, to)
}

#[cfg(not(target_os = "macos"))]
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    rename_after_looking(from, to)
}

fn rename_after_looking(from: &Path, to: &Path) -> io::Result<()> {
    match fs::symlink_metadata(to) {
        Ok(_) => Err(io::ErrorKind::AlreadyExists.into()),
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => fs::rename(from, to),
        Err(cause) => Err(cause),
    }
}

/// Writes `sources` into `out` as a zip, each at the top level under its own name.
fn write_zip(sources: &[PathBuf], out: File, limits: Zip64From) -> io::Result<()> {
    let mut writer = ZipWriter {
        out: BufWriter::new(out),
        recorded: Vec::new(),
        limits,
    };
    let mut top = HashSet::new();
    for source in sources {
        let name = source
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        if left_out(&name) {
            continue;
        }
        let name = unique_name(&name, &mut top)?;
        writer.add_tree(source, name)?;
    }
    writer.finish()?.into_inner()?.sync_all()
}

fn left_out(name: &str) -> bool {
    name == LEFT_OUT_NAME
        || (name.starts_with(TEMPORARY_PREFIX) && name.ends_with(TEMPORARY_SUFFIX))
}

/// `name`, or "stem 2.ext", "stem 3.ext"… if another top-level item already has it (files from
/// different folders can share a name). Records the result in `taken`.
fn unique_name(name: &str, taken: &mut HashSet<String>) -> io::Result<String> {
    let extension = Path::new(name)
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let stem = &name[..name.len() - extension.len()];
    (1..=NUMBERED_NAME_ATTEMPTS)
        .map(|attempt| match attempt {
            1 => name.to_owned(),
            _ => format!("{stem} {attempt}{extension}"),
        })
        .find(|candidate| taken.insert(candidate.clone()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, name.to_owned()))
}

enum Body {
    Folder,
    /// A symlink's target bytes.
    Link(Vec<u8>),
    /// A file and how many of its bytes to write.
    File {
        file: File,
        size: u64,
    },
}

/// A written entry, as the central directory records it.
struct Recorded {
    name: Vec<u8>,
    needed: u16,
    method: u16,
    time: DosTime,
    crc: u32,
    compressed: u64,
    size: u64,
    offset: u64,
    external: u32,
    sizes_in_zip64: bool,
}

struct ZipWriter<W: Write + Seek> {
    out: W,
    recorded: Vec<Recorded>,
    limits: Zip64From,
}

impl<W: Write + Seek> ZipWriter<W> {
    /// Adds `path` as `name`: folders recursively, symlinks as symlinks, files deflated.
    /// Sockets, pipes and devices are skipped.
    fn add_tree(&mut self, path: &Path, name: String) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path).map_err(|cause| naming(path, cause))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            let target = fs::read_link(path).map_err(|cause| naming(path, cause))?;
            return self.add(name, &metadata, Body::Link(path_bytes(&target)));
        }
        if file_type.is_file() {
            let file = File::open(path).map_err(|cause| naming(path, cause))?;
            let size = metadata.len();
            return self
                .add(name, &metadata, Body::File { file, size })
                .map_err(|cause| naming(path, cause));
        }
        if !file_type.is_dir() {
            return Ok(());
        }
        self.add(format!("{name}/"), &metadata, Body::Folder)?;
        let mut children = fs::read_dir(path)
            .and_then(|listed| {
                listed
                    .map(|child| child.map(|child| child.file_name()))
                    .collect::<io::Result<Vec<_>>>()
            })
            .map_err(|cause| naming(path, cause))?;
        children.sort();
        for child in children {
            let child_name = child.to_string_lossy();
            if left_out(&child_name) {
                continue;
            }
            self.add_tree(&path.join(&child), format!("{name}/{child_name}"))?;
        }
        Ok(())
    }

    /// Writes an entry's local header and data, then patches its CRC-32 and sizes into the
    /// header.
    fn add(&mut self, name: String, metadata: &Metadata, body: Body) -> io::Result<()> {
        let offset = self.out.stream_position()?;
        let (method, sizes_in_zip64) = match &body {
            Body::File { size, .. } => (DEFLATED, *size >= self.limits.file_bytes),
            Body::Folder | Body::Link(_) => (STORED, false),
        };
        let needed = if sizes_in_zip64 || offset >= self.limits.bytes {
            NEEDED_ZIP64
        } else {
            NEEDED_PLAIN
        };
        let entry = Recorded {
            name: name.into_bytes(),
            needed,
            method,
            time: DosTime::of(metadata),
            crc: 0,
            compressed: 0,
            size: 0,
            offset,
            external: external_attributes(metadata),
            sizes_in_zip64,
        };
        self.write_local_header(&entry)?;
        let (crc, compressed, size) = self.write_body(body)?;
        let end = self.out.stream_position()?;
        self.out.seek(SeekFrom::Start(offset + LOCAL_CRC_AT))?;
        put_u32(&mut self.out, crc)?;
        if sizes_in_zip64 {
            let values = offset + LOCAL_HEADER_BYTES + entry.name.len() as u64 + EXTRA_HEADER_BYTES;
            self.out.seek(SeekFrom::Start(values))?;
            put_u64(&mut self.out, size)?;
            put_u64(&mut self.out, compressed)?;
        } else {
            put_u32(&mut self.out, narrow(compressed)?)?;
            put_u32(&mut self.out, narrow(size)?)?;
        }
        self.out.seek(SeekFrom::Start(end))?;
        self.recorded.push(Recorded {
            crc,
            compressed,
            size,
            ..entry
        });
        Ok(())
    }

    /// Writes a local header with the CRC-32 and sizes zeroed (or in a ZIP64 field), to be
    /// patched once the data is written.
    fn write_local_header(&mut self, entry: &Recorded) -> io::Result<()> {
        let sizes = if entry.sizes_in_zip64 { u32::MAX } else { 0 };
        let extra_bytes: u16 = if entry.sizes_in_zip64 { 20 } else { 0 };
        let out = &mut self.out;
        put_u32(out, LOCAL_HEADER_SIGNATURE)?;
        put_u16(out, entry.needed)?;
        put_u16(out, UTF8_NAME_FLAG)?;
        put_u16(out, entry.method)?;
        put_u16(out, entry.time.time)?;
        put_u16(out, entry.time.date)?;
        put_u32(out, 0)?;
        put_u32(out, sizes)?;
        put_u32(out, sizes)?;
        put_u16(out, narrow_name(&entry.name)?)?;
        put_u16(out, extra_bytes)?;
        out.write_all(&entry.name)?;
        if entry.sizes_in_zip64 {
            put_u16(out, ZIP64_EXTRA_ID)?;
            put_u16(out, 16)?;
            put_u64(out, 0)?;
            put_u64(out, 0)?;
        }
        Ok(())
    }

    /// Writes an entry's data, deflating files as they stream. Returns the CRC-32, compressed
    /// size and uncompressed size.
    fn write_body(&mut self, body: Body) -> io::Result<(u32, u64, u64)> {
        match body {
            Body::Folder => Ok((0, 0, 0)),
            Body::Link(target) => {
                self.out.write_all(&target)?;
                let mut crc = Crc::new();
                crc.update(&target);
                Ok((crc.sum(), target.len() as u64, target.len() as u64))
            }
            Body::File { file, size } => {
                let start = self.out.stream_position()?;
                let mut reader = CrcReader::new(file.take(size));
                let mut encoder = DeflateEncoder::new(&mut self.out, Compression::default());
                let read = io::copy(&mut reader, &mut encoder)?;
                encoder.finish()?;
                let end = self.out.stream_position()?;
                Ok((reader.crc().sum(), end - start, read))
            }
        }
    }

    /// Writes the central directory and end records, adding ZIP64 records where a value
    /// overflows. Returns the writer.
    fn finish(mut self) -> io::Result<W> {
        let start = self.out.stream_position()?;
        for entry in &self.recorded {
            write_central_header(&mut self.out, entry, self.limits)?;
        }
        let end = self.out.stream_position()?;
        let (count, size) = (self.recorded.len() as u64, end - start);
        let zip64 =
            count >= self.limits.entries || size >= self.limits.bytes || start >= self.limits.bytes;
        let out = &mut self.out;
        if zip64 {
            put_u32(out, ZIP64_END_SIGNATURE)?;
            put_u64(out, ZIP64_END_RECORD_BYTES)?;
            put_u16(out, MADE_BY)?;
            put_u16(out, NEEDED_ZIP64)?;
            put_u32(out, 0)?;
            put_u32(out, 0)?;
            put_u64(out, count)?;
            put_u64(out, count)?;
            put_u64(out, size)?;
            put_u64(out, start)?;
            put_u32(out, ZIP64_LOCATOR_SIGNATURE)?;
            put_u32(out, 0)?;
            put_u64(out, end)?;
            put_u32(out, 1)?;
        }
        let (count, size, start) = if zip64 {
            (u16::MAX, u32::MAX, u32::MAX)
        } else {
            (narrow(count)?, narrow(size)?, narrow(start)?)
        };
        put_u32(out, END_SIGNATURE)?;
        put_u16(out, 0)?;
        put_u16(out, 0)?;
        put_u16(out, count)?;
        put_u16(out, count)?;
        put_u32(out, size)?;
        put_u32(out, start)?;
        put_u16(out, 0)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Writes one central directory header. Values that overflow their field go in the ZIP64
/// extra field, in spec order: size, compressed size, offset.
fn write_central_header(
    out: &mut impl Write,
    entry: &Recorded,
    limits: Zip64From,
) -> io::Result<()> {
    let sizes_in_zip64 =
        entry.sizes_in_zip64 || entry.size >= limits.bytes || entry.compressed >= limits.bytes;
    let offset_in_zip64 = entry.offset >= limits.bytes;
    let mut extra = Vec::new();
    if sizes_in_zip64 {
        extra.extend(entry.size.to_le_bytes());
        extra.extend(entry.compressed.to_le_bytes());
    }
    if offset_in_zip64 {
        extra.extend(entry.offset.to_le_bytes());
    }
    let (size, compressed) = if sizes_in_zip64 {
        (u32::MAX, u32::MAX)
    } else {
        (narrow(entry.size)?, narrow(entry.compressed)?)
    };
    let offset = if offset_in_zip64 {
        u32::MAX
    } else {
        narrow(entry.offset)?
    };
    let extra_bytes = if extra.is_empty() {
        0
    } else {
        EXTRA_HEADER_BYTES as usize + extra.len()
    };
    put_u32(out, CENTRAL_HEADER_SIGNATURE)?;
    put_u16(out, MADE_BY)?;
    put_u16(out, entry.needed)?;
    put_u16(out, UTF8_NAME_FLAG)?;
    put_u16(out, entry.method)?;
    put_u16(out, entry.time.time)?;
    put_u16(out, entry.time.date)?;
    put_u32(out, entry.crc)?;
    put_u32(out, compressed)?;
    put_u32(out, size)?;
    put_u16(out, narrow_name(&entry.name)?)?;
    put_u16(out, narrow(extra_bytes as u64)?)?;
    // No comment, disk 0, no internal attributes.
    put_u16(out, 0)?;
    put_u16(out, 0)?;
    put_u16(out, 0)?;
    put_u32(out, entry.external)?;
    put_u32(out, offset)?;
    out.write_all(&entry.name)?;
    if !extra.is_empty() {
        put_u16(out, ZIP64_EXTRA_ID)?;
        put_u16(out, narrow(extra.len() as u64)?)?;
        out.write_all(&extra)?;
    }
    Ok(())
}

/// An MS-DOS time and date in local time, to 2 s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DosTime {
    time: u16,
    date: u16,
}

impl DosTime {
    /// Midnight, 1 January 1980: the earliest value the format holds.
    const FIRST: Self = Self {
        time: 0,
        date: (1 << 5) | 1,
    };

    /// The file's modification time in local time, clamped to the format's range.
    fn of(metadata: &Metadata) -> Self {
        let seconds = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| {
                i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
            });
        crate::list::local(seconds).map_or(Self::FIRST, |local| Self::from_local(&local))
    }

    fn from_local(local: &crate::list::Local) -> Self {
        if local.year < DOS_FIRST_YEAR {
            return Self::FIRST;
        }
        let year = local.year.min(DOS_LAST_YEAR) - DOS_FIRST_YEAR;
        let field = |value: i32| u16::try_from(value).unwrap_or(0);
        Self {
            time: (field(local.hour) << 11) | (field(local.minute) << 5) | field(local.second / 2),
            date: (field(year) << 9) | ((local.month as u16 + 1) << 5) | field(local.day),
        }
    }
}

/// External attributes: the Unix mode (with file type) in the high 16 bits, plus the DOS
/// directory bit for folders.
fn external_attributes(metadata: &Metadata) -> u32 {
    let folder = if metadata.is_dir() {
        DOS_FOLDER_ATTRIBUTE
    } else {
        0
    };
    (unix_mode(metadata) << 16) | folder
}

#[cfg(unix)]
fn unix_mode(metadata: &Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    metadata.mode()
}

/// A Unix mode synthesized from Windows metadata: folder, link or file, read-only or not.
#[cfg(not(unix))]
fn unix_mode(metadata: &Metadata) -> u32 {
    const FOLDER: u32 = 0o040_755;
    const LINK: u32 = 0o120_777;
    const FILE: u32 = 0o100_644;
    const READ_ONLY_FILE: u32 = 0o100_444;
    if metadata.file_type().is_symlink() {
        LINK
    } else if metadata.is_dir() {
        FOLDER
    } else if metadata.permissions().readonly() {
        READ_ONLY_FILE
    } else {
        FILE
    }
}

/// A symlink target as stored in the archive: bytes with `/` separators.
fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().replace('\\', "/").into_bytes()
    }
}

/// Adds the file path to an error.
fn naming(path: &Path, cause: io::Error) -> io::Error {
    io::Error::new(cause.kind(), format!("{}: {cause}", path.display()))
}

/// Narrows a value, failing if it does not fit.
fn narrow<T: TryFrom<u64>>(value: u64) -> io::Result<T> {
    T::try_from(value).map_err(|_| io::Error::other(format!("{value} does not fit its zip field")))
}

/// A name's length as the zip's 16-bit field.
fn narrow_name(name: &[u8]) -> io::Result<u16> {
    narrow(name.len() as u64)
}

fn put_u16(out: &mut impl Write, value: u16) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

fn put_u32(out: &mut impl Write, value: u32) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

fn put_u64(out: &mut impl Write, value: u64) -> io::Result<()> {
    out.write_all(&value.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test folder with a nested folder, a symlink, a UTF-8 name, an empty file, a hidden
    /// file, a `.DS_Store` and a file with custom permissions.
    fn film_folder(root: &Path) -> PathBuf {
        let film = root.join("Film.timeline");
        fs::create_dir_all(film.join("scenes/empty")).expect("folders");
        fs::write(film.join("scenes/one.html"), "<p>one</p>".repeat(500)).expect("page");
        fs::write(film.join("Café ünïcode 日本.txt"), "naïve").expect("utf-8 name");
        fs::write(film.join("empty.txt"), b"").expect("empty");
        fs::write(film.join(".hidden"), b"kept").expect("hidden");
        fs::write(film.join(".DS_Store"), b"finder").expect("ds_store");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = film.join("run.sh");
            fs::write(&script, b"#!/bin/sh\n").expect("script");
            fs::set_permissions(&script, fs::Permissions::from_mode(0o750)).expect("mode");
            std::os::unix::fs::symlink("scenes/one.html", film.join("link.html")).expect("link");
        }
        film
    }

    fn compress(sources: Vec<PathBuf>) -> PathBuf {
        Compress::of(sources, "en")
            .expect("a plan")
            .run()
            .expect("compressed")
    }

    /// Names and sizes from the archive's central directory, parsed from the end record and
    /// each header.
    fn listed(archive: &Path) -> Vec<String> {
        let bytes = fs::read(archive).expect("archive");
        let end = bytes
            .windows(4)
            .rposition(|window| window == END_SIGNATURE.to_le_bytes())
            .expect("end record");
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
        let u32_at = |at: usize| {
            u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
        };
        let (count, mut at) = (u16_at(end + 10), u32_at(end + 16));
        let mut names = Vec::new();
        for _ in 0..count {
            let name_bytes = u16_at(at + 28);
            let skip = 46 + name_bytes + u16_at(at + 30) + u16_at(at + 32);
            names.push(
                String::from_utf8(bytes[at + 46..at + 46 + name_bytes].to_vec()).expect("utf-8"),
            );
            at += skip;
        }
        names
    }

    #[test]
    fn one_item_is_named_after_itself_and_several_are_an_archive() {
        let paths = |names: &[&str]| {
            names
                .iter()
                .map(|name| PathBuf::from("/drive").join(name))
                .collect::<Vec<_>>()
        };
        assert_eq!(archive_name(&paths(&["photo.png"]), "en"), "photo.png.zip");
        assert_eq!(
            archive_name(&paths(&["Film.timeline"]), "en"),
            "Film.timeline.zip"
        );
        assert_eq!(archive_name(&paths(&["Notes"]), "en"), "Notes.zip");
        assert_eq!(archive_name(&paths(&["a.png", "b"]), "en"), "Archive.zip");
        let plan = Compress::of(paths(&["Notes/a.png", "b"]), "en").expect("a plan");
        assert_eq!(
            plan.directory,
            Path::new("/drive/Notes"),
            "beside the first"
        );
        assert!(Compress::of(Vec::new(), "en").is_none());
    }

    #[test]
    fn a_taken_name_gets_the_drives_number_and_nothing_unfinished_is_left() {
        let root = tempfile::tempdir().expect("root");
        let photo = root.path().join("photo.png");
        fs::write(&photo, b"png").expect("photo");
        let notes = root.path().join("Notes");
        fs::create_dir(&notes).expect("notes");
        assert_eq!(
            compress(vec![photo.clone()]),
            root.path().join("photo.png.zip")
        );
        assert_eq!(
            compress(vec![photo.clone()]),
            root.path().join("photo.png 2.zip")
        );
        assert_eq!(compress(vec![notes.clone()]), root.path().join("Notes.zip"));
        assert_eq!(
            compress(vec![photo.clone(), notes.clone()]),
            root.path().join("Archive.zip")
        );
        assert_eq!(
            compress(vec![photo.clone(), notes.clone()]),
            root.path().join("Archive 2.zip")
        );
        // An unreadable file fails the whole archive and leaves nothing behind.
        let failed = Compress::of(vec![photo, root.path().join("gone.png")], "en")
            .expect("a plan")
            .run()
            .expect_err("gone");
        assert!(failed.to_string().contains("gone.png"), "{failed}");
        let mut left: Vec<String> = fs::read_dir(root.path())
            .expect("listed")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "Archive 2.zip",
                "Archive.zip",
                "Notes",
                "Notes.zip",
                "photo.png",
                "photo.png 2.zip",
                "photo.png.zip"
            ]
        );
    }

    #[test]
    fn a_folder_goes_in_whole_with_its_links_names_and_empty_files_but_no_ds_store() {
        let root = tempfile::tempdir().expect("root");
        let film = film_folder(root.path());
        let archive = compress(vec![film]);
        let mut names = listed(&archive);
        names.sort();
        let mut expected = vec![
            "Film.timeline/",
            "Film.timeline/.hidden",
            "Film.timeline/Café ünïcode 日本.txt",
            "Film.timeline/empty.txt",
            "Film.timeline/scenes/",
            "Film.timeline/scenes/empty/",
            "Film.timeline/scenes/one.html",
        ];
        if cfg!(unix) {
            expected.extend(["Film.timeline/link.html", "Film.timeline/run.sh"]);
        }
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    #[test]
    fn two_items_of_one_name_are_numbered_at_the_top_of_the_archive() {
        let root = tempfile::tempdir().expect("root");
        for folder in ["a", "b"] {
            fs::create_dir(root.path().join(folder)).expect("folder");
            fs::write(root.path().join(folder).join("x.png"), folder).expect("x");
        }
        let archive = compress(vec![
            root.path().join("a/x.png"),
            root.path().join("b/x.png"),
        ]);
        assert_eq!(listed(&archive), ["x.png", "x 2.png"]);
    }

    #[test]
    fn an_mtime_is_an_ms_dos_time_on_the_persons_clock() {
        let local = crate::list::Local {
            year: 2026,
            month: 10,
            day: 17,
            hour: 13,
            minute: 45,
            second: 59,
            day_number: 0,
        };
        let time = DosTime::from_local(&local);
        assert_eq!(time.date, ((2026 - 1980) << 9) | (11 << 5) | 17);
        assert_eq!(time.time, (13 << 11) | (45 << 5) | 29);
        let old = crate::list::Local {
            year: 1970,
            ..local
        };
        assert_eq!(DosTime::from_local(&old), DosTime::FIRST);
    }

    /// Tests and unpacks `archive` with the system tools (`unzip -t`, `ditto -x -k`) and
    /// compares the result with `source`, whose parent holds the archive.
    #[cfg(target_os = "macos")]
    fn opens_with_the_systems_tools(archive: &Path, source: &Path) {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let tested = Command::new("/usr/bin/unzip")
            .arg("-t")
            .arg(archive)
            .output()
            .expect("unzip");
        let said = String::from_utf8_lossy(&tested.stdout);
        assert!(
            tested.status.success(),
            "unzip -t: {said}{}",
            String::from_utf8_lossy(&tested.stderr)
        );
        assert!(said.contains("No errors detected"), "{said}");
        let out = tempfile::tempdir().expect("out");
        let unpacked = Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(archive)
            .arg(out.path())
            .output()
            .expect("ditto");
        assert!(
            unpacked.status.success(),
            "ditto: {}",
            String::from_utf8_lossy(&unpacked.stderr)
        );
        let film = out.path().join(source.file_name().expect("a name"));
        let read = |path: &Path| fs::read(path).expect("read");
        assert_eq!(
            read(&film.join("scenes/one.html")),
            read(&source.join("scenes/one.html"))
        );
        assert_eq!(
            read(&film.join("Café ünïcode 日本.txt")),
            "naïve".as_bytes()
        );
        assert_eq!(read(&film.join("empty.txt")), b"");
        assert_eq!(read(&film.join(".hidden")), b"kept");
        assert!(film.join("scenes/empty").is_dir());
        assert!(!film.join(".DS_Store").exists());
        assert_eq!(
            fs::read_link(film.join("link.html")).expect("a link"),
            Path::new("scenes/one.html")
        );
        let mode = fs::metadata(film.join("run.sh"))
            .expect("script")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o750);
        // Times survive to the format's 2 s precision.
        let modified = |path: &Path| {
            fs::metadata(path)
                .expect("stat")
                .modified()
                .expect("mtime")
                .duration_since(std::time::UNIX_EPOCH)
                .expect("after 1970")
                .as_secs()
        };
        let drift = modified(&film.join("empty.txt")).abs_diff(modified(&source.join("empty.txt")));
        assert!(drift <= 2, "{drift} s apart");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_systems_own_tools_open_what_is_written() {
        let root = tempfile::tempdir().expect("root");
        let film = film_folder(root.path());
        let archive = compress(vec![film.clone()]);
        opens_with_the_systems_tools(&archive, &film);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zip64_records_open_with_the_systems_own_tools() {
        let root = tempfile::tempdir().expect("root");
        let film = film_folder(root.path());
        // Force ZIP64 for every value, as past 4 GiB or 65,535 entries.
        let everything = Zip64From {
            bytes: 0,
            entries: 0,
            file_bytes: 0,
        };
        let archive = root.path().join("film.zip");
        write_zip(
            std::slice::from_ref(&film),
            File::create_new(&archive).expect("archive"),
            everything,
        )
        .expect("written");
        let bytes = fs::read(&archive).expect("archive");
        let has = |signature: u32| {
            bytes
                .windows(4)
                .any(|window| window == signature.to_le_bytes())
        };
        assert!(has(ZIP64_END_SIGNATURE) && has(ZIP64_LOCATOR_SIGNATURE));
        opens_with_the_systems_tools(&archive, &film);
    }

    #[test]
    fn the_zip64_end_records_carry_what_the_end_record_cannot() {
        let mut writer = ZipWriter {
            out: io::Cursor::new(Vec::new()),
            recorded: Vec::new(),
            limits: ZIP64_FROM,
        };
        let offset = 5 * (1 << 30);
        for index in 0..70_000u64 {
            writer.recorded.push(Recorded {
                name: format!("{index}").into_bytes(),
                needed: NEEDED_ZIP64,
                method: DEFLATED,
                time: DosTime::FIRST,
                crc: 0,
                compressed: offset,
                size: offset,
                offset,
                external: 0,
                sizes_in_zip64: true,
            });
        }
        let bytes = writer.finish().expect("finished").into_inner();
        let u64_at = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"));
        let end = bytes.len() - 22;
        let locator = end - 20;
        let record = usize::try_from(u64_at(locator + 8)).expect("an offset");
        assert_eq!(
            &bytes[record..record + 4],
            ZIP64_END_SIGNATURE.to_le_bytes()
        );
        assert_eq!(u64_at(record + 24), 70_000, "entries");
        assert_eq!(
            u64_at(record + 48),
            0,
            "the directory starts the bytes here"
        );
        assert_eq!(
            &bytes[end + 8..end + 10],
            [0xFF, 0xFF],
            "the end record's count is ZIP64's"
        );
        // The first header's ZIP64 field: size, compressed size, offset.
        let extra = 46 + 1;
        assert_eq!(
            [u64_at(extra + 4), u64_at(extra + 12), u64_at(extra + 20)],
            [offset; 3]
        );
        assert_eq!(&bytes[20..28], [0xFF; 8], "both sizes are ZIP64's");
    }
}
