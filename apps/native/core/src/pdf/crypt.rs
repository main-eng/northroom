//! PDF's standard security handler: RC4 and AES-128 (revisions 2 to 4) and AES-256 (revisions
//! 5 and 6). Most encrypted PDFs open with an empty user password, being encrypted to set
//! permissions rather than to keep anyone out; others open with the user's or the owner's
//! password, which the person types in (`pdf::passwords`).
//!
//! Left out: preparing a revision 6 password with SASLprep, so a password outside ASCII may not
//! match.

use super::object::{Document, Object, Ref, Stream};
use super::PdfError;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use md5::{Digest as _, Md5};
use sha2::{Sha256, Sha384, Sha512};
use std::rc::Rc;

type Result<T> = std::result::Result<T, PdfError>;

/// The padding a password is filled out with to 32 bytes (Algorithm 2).
const PADDING: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cipher {
    Rc4,
    Aes128,
    Aes256,
    Identity,
}

/// What a file is decrypted with.
pub struct Crypt {
    key: Vec<u8>,
    strings: Cipher,
    streams: Cipher,
    /// The encryption dictionary itself, which is not encrypted.
    holder: Option<Ref>,
    metadata: bool,
}

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut state: Vec<u8> = (0..=255).collect();
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
        state.swap(i, usize::from(j));
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|byte| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(state[usize::from(i)]);
            state.swap(usize::from(i), usize::from(j));
            let index = state[usize::from(i)].wrapping_add(state[usize::from(j)]);
            byte ^ state[usize::from(index)]
        })
        .collect()
}

/// AES-CBC decryption of `data` with its IV in front, its padding removed.
fn aes_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    if data.len() < 32 {
        return Vec::new();
    }
    let (iv, body) = data.split_at(16);
    let mut out = cbc_decrypt(key, iv, &body[..body.len() / 16 * 16]);
    if let Some(&pad) = out.last() {
        if (1..=16).contains(&pad) && usize::from(pad) <= out.len() {
            out.truncate(out.len() - usize::from(pad));
        }
    }
    out
}

fn cbc_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    let mut previous: [u8; 16] = iv.try_into().unwrap_or([0; 16]);
    let mut out = Vec::with_capacity(data.len());
    let decrypt = |block: &mut aes::Block| match key.len() {
        32 => aes::Aes256::new_from_slice(key).map(|cipher| cipher.decrypt_block(block)),
        _ => aes::Aes128::new_from_slice(key).map(|cipher| cipher.decrypt_block(block)),
    };
    for chunk in data.as_chunks::<16>().0 {
        let mut block = aes::Block::clone_from_slice(chunk);
        let _ = decrypt(&mut block);
        out.extend(
            block
                .iter()
                .zip(previous)
                .map(|(byte, before)| byte ^ before),
        );
        previous.copy_from_slice(chunk);
    }
    out
}

fn cbc_encrypt_128(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    let Ok(cipher) = aes::Aes128::new_from_slice(key) else {
        return Vec::new();
    };
    let mut previous: [u8; 16] = iv.try_into().unwrap_or([0; 16]);
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.as_chunks::<16>().0 {
        let mut block = aes::Block::default();
        for (index, byte) in chunk.iter().enumerate() {
            block[index] = byte ^ previous[index];
        }
        cipher.encrypt_block(&mut block);
        previous.copy_from_slice(&block);
        out.extend_from_slice(&block);
    }
    out
}

/// The hash of revisions 5 and 6 (Algorithm 2.B; plain SHA-256 for 5) of `password` with
/// `salt` and, for the owner, the user's `U` (`udata`).
fn hash_2b(revision: i64, password: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
    let mut first = Sha256::new();
    first.update(password);
    first.update(salt);
    first.update(udata);
    let mut key = first.finalize().to_vec();
    if revision < 6 {
        return key;
    }
    let mut round = 0usize;
    loop {
        let mut block = Vec::with_capacity(64 * (password.len() + key.len() + udata.len()));
        for _ in 0..64 {
            block.extend_from_slice(password);
            block.extend_from_slice(&key);
            block.extend_from_slice(udata);
        }
        let encrypted = cbc_encrypt_128(&key[..16], &key[16..32], &block);
        let choice: u32 = encrypted[..16]
            .iter()
            .map(|byte| u32::from(*byte))
            .sum::<u32>()
            % 3;
        key = match choice {
            0 => Sha256::digest(&encrypted).to_vec(),
            1 => Sha384::digest(&encrypted).to_vec(),
            _ => Sha512::digest(&encrypted).to_vec(),
        };
        round += 1;
        let last = usize::from(*encrypted.last().unwrap_or(&0));
        if round >= 64 && last + 32 <= round {
            break;
        }
    }
    key.truncate(32);
    key
}

fn bytes_of(document: &Document, dict: &super::object::Dict, key: &str) -> Vec<u8> {
    document
        .entry(dict, key)
        .as_bytes()
        .map(<[u8]>::to_vec)
        .unwrap_or_default()
}

/// A password as revisions 2 to 4 take it: its first 32 bytes, filled out with `PADDING`.
fn padded(password: &[u8]) -> [u8; 32] {
    let mut out = PADDING;
    let length = password.len().min(32);
    out[..length].copy_from_slice(&password[..length]);
    out[length..].copy_from_slice(&PADDING[..32 - length]);
    out
}

/// What revisions 2 to 4 derive a file key from, besides the password.
struct Legacy {
    owner: Vec<u8>,
    user: Vec<u8>,
    permissions: i64,
    id: Vec<u8>,
    length: usize,
    revision: i64,
    metadata: bool,
}

impl Legacy {
    fn key_length(&self) -> usize {
        if self.revision == 2 {
            5
        } else {
            self.length.clamp(5, 16)
        }
    }

    /// The file key for a padded user password (Algorithm 2).
    fn key(&self, password: &[u8; 32]) -> Vec<u8> {
        let mut hash = Md5::new();
        hash.update(password);
        hash.update(self.owner.get(..32).unwrap_or(&self.owner));
        hash.update((self.permissions as i32).to_le_bytes());
        hash.update(&self.id);
        if self.revision >= 4 && !self.metadata {
            hash.update([0xff; 4]);
        }
        let mut key = hash.finalize().to_vec();
        let length = self.key_length();
        if self.revision >= 3 {
            for _ in 0..50 {
                key = Md5::digest(&key[..length]).to_vec();
            }
        }
        key.truncate(length);
        key
    }

    /// Whether `key` is the file's: its `U` computed from it matches (Algorithms 4 and 5).
    fn opens(&self, key: &[u8]) -> bool {
        if self.revision == 2 {
            return rc4(key, &PADDING) == self.user.get(..32).unwrap_or(&self.user);
        }
        let mut hash = Md5::new();
        hash.update(PADDING);
        hash.update(&self.id);
        let mut check = rc4(key, &hash.finalize());
        for round in 1..=19u8 {
            let turned: Vec<u8> = key.iter().map(|byte| byte ^ round).collect();
            check = rc4(&turned, &check);
        }
        self.user.get(..16).is_some_and(|user| user == &check[..16])
    }

    /// The padded user password the owner's password unlocks from `O` (Algorithm 7).
    fn user_from_owner(&self, owner: &[u8]) -> [u8; 32] {
        let mut hash = Md5::digest(padded(owner)).to_vec();
        let length = self.key_length();
        if self.revision >= 3 {
            for _ in 0..50 {
                hash = Md5::digest(&hash).to_vec();
            }
        }
        let key = &hash[..length];
        let mut user = self.owner.get(..32).unwrap_or(&self.owner).to_vec();
        if self.revision == 2 {
            user = rc4(key, &user);
        } else {
            for round in (0..=19u8).rev() {
                let turned: Vec<u8> = key.iter().map(|byte| byte ^ round).collect();
                user = rc4(&turned, &user);
            }
        }
        padded(&user)
    }

    /// The file key `password` opens, as the user's or the owner's.
    fn file_key(&self, password: &[u8]) -> Option<Vec<u8>> {
        let as_user = self.key(&padded(password));
        if self.opens(&as_user) {
            return Some(as_user);
        }
        let as_owner = self.key(&self.user_from_owner(password));
        self.opens(&as_owner).then_some(as_owner)
    }
}

/// The file key `password` opens in revisions 5 and 6, as the user's or the owner's
/// (Algorithms 2.A, 11 and 12).
fn modern_key(
    document: &Document,
    encrypt: &super::object::Dict,
    revision: i64,
    password: &[u8],
) -> Option<Vec<u8>> {
    let password = &password[..password.len().min(127)];
    let (user, owner) = (
        bytes_of(document, encrypt, "U"),
        bytes_of(document, encrypt, "O"),
    );
    let (user, owner) = (user.get(..48)?, owner.get(..48)?);
    if hash_2b(revision, password, &user[32..40], &[]) == user[..32] {
        let key = hash_2b(revision, password, &user[40..48], &[]);
        return Some(cbc_decrypt(
            &key,
            &[0; 16],
            bytes_of(document, encrypt, "UE").get(..32)?,
        ));
    }
    if hash_2b(revision, password, &owner[32..40], user) == owner[..32] {
        let key = hash_2b(revision, password, &owner[40..48], user);
        return Some(cbc_decrypt(
            &key,
            &[0; 16],
            bytes_of(document, encrypt, "OE").get(..32)?,
        ));
    }
    None
}

fn cipher_named(document: &Document, encrypt: &super::object::Dict, which: &str) -> Cipher {
    let name = document.entry(encrypt, which);
    let Some(name) = name.as_name() else {
        return Cipher::Identity;
    };
    if name == "Identity" {
        return Cipher::Identity;
    }
    let filters = document.entry(encrypt, "CF");
    let method = filters
        .as_dict()
        .and_then(|filters| filters.get(name))
        .map(|filter| document.resolve(filter))
        .and_then(|filter| filter.as_dict().and_then(|dict| dict.get("CFM")).cloned());
    match method.as_ref().and_then(Object::as_name) {
        Some("AESV2") => Cipher::Aes128,
        Some("AESV3") => Cipher::Aes256,
        Some("None") => Cipher::Identity,
        _ => Cipher::Rc4,
    }
}

impl Crypt {
    /// The decryption an encrypted document needs, opened with an empty password or
    /// `password`; `None` when the document isn't encrypted.
    pub fn from_document(document: &Document, password: Option<&[u8]>) -> Result<Option<Self>> {
        let holder = match document.trailer.get("Encrypt") {
            Some(Object::Ref(reference)) => Some(*reference),
            Some(_) => None,
            None => return Ok(None),
        };
        let encrypt = document.entry(&document.trailer, "Encrypt");
        let Some(encrypt) = encrypt.as_dict() else {
            return Ok(None);
        };
        if document.entry(encrypt, "Filter").as_name() != Some("Standard") {
            return Err(PdfError::Damaged(
                "The file is encrypted by a handler this reader lacks.".into(),
            ));
        }
        let version = document.entry(encrypt, "V").as_i64().unwrap_or(0);
        let revision = document.entry(encrypt, "R").as_i64().unwrap_or(2);
        let metadata = !matches!(
            document.entry(encrypt, "EncryptMetadata"),
            Object::Bool(false)
        );
        let (strings, streams) = if version >= 4 {
            (
                cipher_named(document, encrypt, "StrF"),
                cipher_named(document, encrypt, "StmF"),
            )
        } else {
            (Cipher::Rc4, Cipher::Rc4)
        };
        // Only the attachments are encrypted (`AuthEvent /EFOpen`): the document itself opens
        // without a key.
        if strings == Cipher::Identity && streams == Cipher::Identity {
            return Ok(None);
        }
        let legacy = Legacy {
            owner: bytes_of(document, encrypt, "O"),
            user: bytes_of(document, encrypt, "U"),
            permissions: document.entry(encrypt, "P").as_i64().unwrap_or(0),
            id: document
                .entry(&document.trailer, "ID")
                .as_array()
                .and_then(|ids| ids.first())
                .and_then(|id| id.as_bytes().map(<[u8]>::to_vec))
                .unwrap_or_default(),
            length: document
                .entry(encrypt, "Length")
                .as_i64()
                .unwrap_or(40)
                .max(0) as usize
                / 8,
            revision,
            metadata,
        };
        // An empty password first: most encrypted files open with it.
        let key = [b"".as_slice(), password.unwrap_or_default()]
            .into_iter()
            .find_map(|password| match revision {
                5.. => modern_key(document, encrypt, revision, password),
                _ => legacy.file_key(password),
            })
            .ok_or(PdfError::Locked)?;
        Ok(Some(Crypt {
            key,
            strings,
            streams,
            holder,
            metadata,
        }))
    }

    fn object_key(&self, reference: Ref, cipher: Cipher) -> Vec<u8> {
        if cipher == Cipher::Aes256 {
            return self.key.clone();
        }
        let mut hash = Md5::new();
        hash.update(&self.key);
        hash.update(&reference.0.to_le_bytes()[..3]);
        hash.update(reference.1.to_le_bytes());
        if cipher == Cipher::Aes128 {
            hash.update(b"sAlT");
        }
        let mut key = hash.finalize().to_vec();
        key.truncate((self.key.len() + 5).min(16));
        key
    }

    fn decrypt(&self, reference: Ref, cipher: Cipher, data: &[u8]) -> Vec<u8> {
        match cipher {
            Cipher::Identity => data.to_vec(),
            Cipher::Rc4 => rc4(&self.object_key(reference, cipher), data),
            Cipher::Aes128 | Cipher::Aes256 => {
                aes_decrypt(&self.object_key(reference, cipher), data)
            }
        }
    }

    fn strings_in(&self, reference: Ref, object: Object) -> Object {
        match object {
            Object::String(bytes) => Object::String(self.decrypt(reference, self.strings, &bytes)),
            Object::Array(items) => Object::Array(
                items
                    .into_iter()
                    .map(|item| self.strings_in(reference, item))
                    .collect(),
            ),
            Object::Dict(dict) => Object::Dict(
                dict.into_iter()
                    .map(|(key, value)| (key, self.strings_in(reference, value)))
                    .collect(),
            ),
            other => other,
        }
    }

    /// An object as read from the file, with its strings and stream decrypted.
    pub fn decrypt_object(&self, reference: Ref, object: Object) -> Object {
        if Some(reference) == self.holder {
            return object;
        }
        let Object::Stream(stream) = object else {
            return self.strings_in(reference, object);
        };
        let kind = stream.dict.get("Type").and_then(Object::as_name);
        let plain = kind == Some("XRef") || (kind == Some("Metadata") && !self.metadata);
        let dict = match self.strings_in(reference, Object::Dict(stream.dict)) {
            Object::Dict(dict) => dict,
            _ => unreachable!("a dictionary stays a dictionary"),
        };
        let raw = if plain {
            stream.raw
        } else {
            Rc::new(self.decrypt(reference, self.streams, &stream.raw))
        };
        Object::Stream(Stream { dict, raw })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_matches_its_test_vector() {
        // From RFC 6229's predecessor: key "Key", plaintext "Plaintext".
        assert_eq!(
            rc4(b"Key", b"Plaintext"),
            [0xBB, 0xF3, 0x16, 0xE8, 0xD9, 0x40, 0xAF, 0x0A, 0xD3]
        );
    }

    #[test]
    fn aes_round_trips_through_cbc() {
        let key = [7u8; 16];
        let iv = [3u8; 16];
        let plain = b"sixteen byte msgsixteen byte msg";
        let encrypted = cbc_encrypt_128(&key, &iv, plain);
        assert_eq!(cbc_decrypt(&key, &iv, &encrypted), plain);
    }
}
