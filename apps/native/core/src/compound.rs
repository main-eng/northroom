//! Reads a compound file: the container Office used before 2007 (`.doc`, `.xls`, `.ppt`),
//! which keeps named streams in fixed-size sectors like a small file system. Streams are found
//! by name and read whole from memory.
//!
//! A sector chain that loops or leaves the file is refused, so a damaged file can't hang or
//! read past its end.

/// The first bytes of every compound file.
const SIGNATURE: [u8; 8] = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const HEADER_BYTES: usize = 512;
/// The sector allocation table's marker for the end of a chain.
const END_OF_CHAIN: u32 = 0xffff_fffe;
/// How many table sectors the header lists itself; more are listed in sectors of their own.
const HEADER_TABLE_SECTORS: usize = 109;
const ENTRY_BYTES: usize = 128;
/// A directory entry's kinds: a stream and the root, whose chain is the small-stream store.
const STREAM: u8 = 2;
const ROOT: u8 = 5;

/// Why a compound file can't be read; the words are for logs and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompoundError(pub String);

impl std::fmt::Display for CompoundError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

type Result<T> = std::result::Result<T, CompoundError>;

fn fail<T>(why: &str) -> Result<T> {
    Err(CompoundError(why.to_owned()))
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A named stream: its kind, first sector and size.
struct Entry {
    name: String,
    kind: u8,
    start: u32,
    size: usize,
}

/// A compound file read into memory, with its tables and directory.
pub struct Compound<'a> {
    bytes: &'a [u8],
    sector: usize,
    small_sector: usize,
    small_cutoff: usize,
    table: Vec<u32>,
    small_table: Vec<u32>,
    entries: Vec<Entry>,
    small_store: Vec<u8>,
}

/// Whether `bytes` start as a compound file does.
pub fn is_compound(bytes: &[u8]) -> bool {
    bytes.starts_with(&SIGNATURE)
}

impl<'a> Compound<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self> {
        if !is_compound(bytes) || bytes.len() < HEADER_BYTES {
            return fail("The file is not a compound file.");
        }
        let header = |at| u32_at(bytes, at).unwrap_or(0);
        let shift = u16_at(bytes, 0x1e).unwrap_or(9);
        let small_shift = u16_at(bytes, 0x20).unwrap_or(6);
        if !matches!(shift, 9 | 12) || small_shift != 6 {
            return fail("The compound file's sector size is not one Office writes.");
        }
        let mut compound = Compound {
            bytes,
            sector: 1 << shift,
            small_sector: 1 << small_shift,
            small_cutoff: header(0x38) as usize,
            table: Vec::new(),
            small_table: Vec::new(),
            entries: Vec::new(),
            small_store: Vec::new(),
        };
        compound.table = compound.read_table(header(0x2c) as usize, header(0x44))?;
        compound.small_table = compound
            .chain_bytes(header(0x3c), usize::MAX)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|four| u32::from_le_bytes(*four))
            .collect();
        compound.entries = compound.read_directory(header(0x30))?;
        if let Some(root) = compound.entries.iter().find(|entry| entry.kind == ROOT) {
            let (start, size) = (root.start, root.size);
            compound.small_store = compound.chain_bytes(start, size)?;
        }
        Ok(compound)
    }

    fn sector_bytes(&self, id: u32) -> Result<&'a [u8]> {
        let start = (id as usize + 1).checked_mul(self.sector);
        let range = start.and_then(|start| Some(start..start.checked_add(self.sector)?));
        match range.and_then(|range| self.bytes.get(range)) {
            Some(sector) => Ok(sector),
            None => fail("A sector lies past the end of the compound file."),
        }
    }

    /// The sector allocation table, from the sectors the header and the extension sectors list.
    fn read_table(&self, count: usize, mut extension: u32) -> Result<Vec<u32>> {
        let mut listed: Vec<u32> = (0..HEADER_TABLE_SECTORS)
            .filter_map(|index| u32_at(self.bytes, 0x4c + index * 4))
            .collect();
        let per_sector = self.sector / 4 - 1;
        let mut guard = 0;
        while extension < END_OF_CHAIN && listed.len() < count {
            guard += 1;
            if guard > self.bytes.len() / self.sector {
                return fail("The compound file's table list loops.");
            }
            let sector = self.sector_bytes(extension)?;
            listed.extend((0..per_sector).filter_map(|index| u32_at(sector, index * 4)));
            extension = u32_at(sector, per_sector * 4).unwrap_or(END_OF_CHAIN);
        }
        let mut table = Vec::with_capacity(count * self.sector / 4);
        for &id in listed.iter().take(count) {
            let sector = self.sector_bytes(id)?;
            table.extend(
                sector
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|four| u32::from_le_bytes(*four)),
            );
        }
        Ok(table)
    }

    /// The sectors of the chain from `start`, in order.
    fn chain(table: &[u32], start: u32) -> Result<Vec<u32>> {
        let mut chain = Vec::new();
        let mut next = start;
        while next < END_OF_CHAIN - 3 {
            if chain.len() > table.len() {
                return fail("A sector chain loops.");
            }
            chain.push(next);
            next = match table.get(next as usize) {
                Some(&following) => following,
                None => return fail("A sector chain leaves the table."),
            };
        }
        Ok(chain)
    }

    /// The bytes of the chain from `start` in regular sectors, at most `size` of them.
    fn chain_bytes(&self, start: u32, size: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        for id in Self::chain(&self.table, start)? {
            bytes.extend_from_slice(self.sector_bytes(id)?);
            if bytes.len() >= size {
                break;
            }
        }
        bytes.truncate(size);
        Ok(bytes)
    }

    fn small_chain_bytes(&self, start: u32, size: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(size);
        for id in Self::chain(&self.small_table, start)? {
            let from = id as usize * self.small_sector;
            let Some(sector) = self.small_store.get(from..from + self.small_sector) else {
                return fail("A small sector lies past the small-stream store.");
            };
            bytes.extend_from_slice(sector);
            if bytes.len() >= size {
                break;
            }
        }
        bytes.truncate(size);
        Ok(bytes)
    }

    fn read_directory(&self, start: u32) -> Result<Vec<Entry>> {
        let directory = self.chain_bytes(start, usize::MAX)?;
        Ok(directory
            .as_chunks::<ENTRY_BYTES>()
            .0
            .iter()
            .filter_map(|entry| {
                let length = usize::from(u16_at(entry, 0x40)?).min(64);
                let units: Vec<u16> = entry[..length]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|two| u16::from_le_bytes(*two))
                    .take_while(|unit| *unit != 0)
                    .collect();
                Some(Entry {
                    name: String::from_utf16_lossy(&units),
                    kind: entry[0x42],
                    start: u32_at(entry, 0x74)?,
                    size: u32_at(entry, 0x78)? as usize,
                })
            })
            .collect())
    }

    /// The stream named `name`, read whole; `None` when there is none.
    pub fn stream(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.kind == STREAM && entry.name == name)
        else {
            return Ok(None);
        };
        let bytes = if entry.size < self.small_cutoff {
            self.small_chain_bytes(entry.start, entry.size)?
        } else {
            self.chain_bytes(entry.start, entry.size)?
        };
        if bytes.len() < entry.size {
            return fail("A stream is shorter than the directory says.");
        }
        Ok(Some(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compound file of 512-byte sectors after its header: sector 0 the table, 1 the
    /// directory, 2 a stream of
    /// 600 bytes running into 3, the small-stream store in 4 holding a 10-byte stream, and 5 the
    /// small table.
    fn sample() -> Vec<u8> {
        let mut bytes = vec![0u8; 512 * 7];
        bytes[..8].copy_from_slice(&SIGNATURE);
        let put = |bytes: &mut Vec<u8>, at: usize, value: u32| {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes())
        };
        bytes[0x1e] = 9;
        bytes[0x20] = 6;
        put(&mut bytes, 0x2c, 1);
        put(&mut bytes, 0x30, 1);
        // Streams under 512 bytes live in small sectors.
        put(&mut bytes, 0x38, 512);
        put(&mut bytes, 0x3c, 5);
        put(&mut bytes, 0x44, END_OF_CHAIN);
        put(&mut bytes, 0x4c, 0);
        for index in 1..HEADER_TABLE_SECTORS {
            put(&mut bytes, 0x4c + index * 4, 0xffff_ffff);
        }
        // The table: 0 is itself, 1 and 4 and 5 end, 2 runs into 3.
        let table = 512;
        for (id, next) in [
            (0, 0xffff_fffd),
            (1, END_OF_CHAIN),
            (2, 3),
            (3, END_OF_CHAIN),
            (4, END_OF_CHAIN),
            (5, END_OF_CHAIN),
        ] {
            put(&mut bytes, table + id * 4, next);
        }
        let entry =
            |bytes: &mut Vec<u8>, index: usize, name: &str, kind: u8, start: u32, size: u32| {
                let at = 1024 + index * ENTRY_BYTES;
                let units: Vec<u16> = name.encode_utf16().chain([0]).collect();
                for (offset, unit) in units.iter().enumerate() {
                    bytes[at + offset * 2..at + offset * 2 + 2]
                        .copy_from_slice(&unit.to_le_bytes());
                }
                bytes[at + 0x40..at + 0x42]
                    .copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
                bytes[at + 0x42] = kind;
                put(bytes, at + 0x74, start);
                put(bytes, at + 0x78, size);
            };
        entry(&mut bytes, 0, "Root Entry", ROOT, 4, 64);
        entry(&mut bytes, 1, "Big", STREAM, 2, 600);
        entry(&mut bytes, 2, "Small", STREAM, 0, 10);
        for (index, byte) in bytes[1536..1536 + 600].iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        bytes[2560..2570].copy_from_slice(b"small data");
        // The small table: small sector 0 ends its chain.
        put(&mut bytes, 3072, END_OF_CHAIN);
        bytes
    }

    #[test]
    fn streams_read_across_sectors_and_from_the_small_store() {
        let bytes = sample();
        let compound = Compound::open(&bytes).expect("open");
        let big = compound.stream("Big").expect("read").expect("a stream");
        assert_eq!(big.len(), 600);
        assert_eq!((big[0], big[511], big[512], big[599]), (0, 9, 10, 97));
        assert_eq!(
            compound.stream("Small").expect("read"),
            Some(b"small data".to_vec())
        );
        assert_eq!(compound.stream("None").expect("read"), None);
    }

    #[test]
    fn a_loop_or_a_foreign_file_is_refused() {
        let mut bytes = sample();
        // Sector 3 points back to 2.
        bytes[512 + 12..512 + 16].copy_from_slice(&2u32.to_le_bytes());
        let compound = Compound::open(&bytes).expect("open");
        assert!(compound.stream("Big").is_err());
        assert!(Compound::open(b"PK\x03\x04 not a compound file").is_err());
    }
}
