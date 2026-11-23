//! SHA-512 (FIPS 180-4) for checking a downloaded update against its channel file digest.
//! Implemented here, as `digest.rs` does SHA-256, to avoid a dependency for one check.

use std::io::Read;
use std::path::Path;

const ROUND_CONSTANTS: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

/// An incremental SHA-512 hasher.
pub struct Sha512 {
    state: [u64; 8],
    pending: Vec<u8>,
    length: u128,
}

impl Default for Sha512 {
    fn default() -> Self {
        Sha512 {
            state: [
                0x6a09e667f3bcc908,
                0xbb67ae8584caa73b,
                0x3c6ef372fe94f82b,
                0xa54ff53a5f1d36f1,
                0x510e527fade682d1,
                0x9b05688c2b3e6c1f,
                0x1f83d9abfb41bd6b,
                0x5be0cd19137e2179,
            ],
            pending: Vec::with_capacity(128),
            length: 0,
        }
    }
}

impl Sha512 {
    pub fn update(&mut self, mut bytes: &[u8]) {
        self.length += bytes.len() as u128;
        if !self.pending.is_empty() {
            let wanted = (128 - self.pending.len()).min(bytes.len());
            self.pending.extend_from_slice(&bytes[..wanted]);
            bytes = &bytes[wanted..];
            if self.pending.len() < 128 {
                return;
            }
            let block = std::mem::take(&mut self.pending);
            self.compress(&block);
        }
        let (blocks, rest) = bytes.as_chunks::<128>();
        for block in blocks {
            self.compress(block);
        }
        self.pending.extend_from_slice(rest);
    }

    fn compress(&mut self, block: &[u8]) {
        let mut schedule = [0_u64; 80];
        for (word, eight) in schedule.iter_mut().zip(block.as_chunks::<8>().0) {
            *word = u64::from_be_bytes(*eight);
        }
        for index in 16..80 {
            let (before, recent) = (schedule[index - 15], schedule[index - 2]);
            let small = before.rotate_right(1) ^ before.rotate_right(8) ^ (before >> 7);
            let large = recent.rotate_right(19) ^ recent.rotate_right(61) ^ (recent >> 6);
            schedule[index] = schedule[index - 16]
                .wrapping_add(small)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(large);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for index in 0..80 {
            let sum1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let choice = (e & f) ^ (!e & g);
            let first = h
                .wrapping_add(sum1)
                .wrapping_add(choice)
                .wrapping_add(ROUND_CONSTANTS[index])
                .wrapping_add(schedule[index]);
            let sum0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let second = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(first);
            d = c;
            c = b;
            b = a;
            a = first.wrapping_add(second);
        }
        for (word, add) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *word = word.wrapping_add(add);
        }
    }

    pub fn finish(mut self) -> [u8; 64] {
        let bits = self.length * 8;
        let mut tail = std::mem::take(&mut self.pending);
        tail.push(0x80);
        while tail.len() % 128 != 112 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        for block in tail.as_chunks::<128>().0 {
            self.compress(block);
        }
        let mut out = [0_u8; 64];
        for (eight, word) in out.as_chunks_mut::<8>().0.iter_mut().zip(self.state) {
            *eight = word.to_be_bytes();
        }
        out
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, as node's `digest("base64")` writes it.
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for three in bytes.chunks(3) {
        let value = (u32::from(three[0]) << 16)
            | (u32::from(*three.get(1).unwrap_or(&0)) << 8)
            | u32::from(*three.get(2).unwrap_or(&0));
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= three.len() {
                out.push(ALPHABET[((value >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether the expected digest is hex: 128 characters with no `+`, `Z` or `=`. Anything else is
/// base64 (electron-updater's rule).
pub fn expected_is_hex(expected: &str) -> bool {
    expected.len() == 128
        && !expected.contains('+')
        && !expected.contains('Z')
        && !expected.contains('=')
}

/// The file's SHA-512 in the same encoding as `expected`, read 1 MB at a time.
pub fn file_digest(path: &Path, expected: &str) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha512::default();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let bytes = digest.finish();
    Ok(if expected_is_hex(expected) {
        hex(&bytes)
    } else {
        base64(&bytes)
    })
}

/// Checks a downloaded file against its channel file's digest; a mismatch names both.
pub fn check_file(path: &Path, expected: &str) -> Result<(), String> {
    let actual = file_digest(path, expected)
        .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "sha512 checksum mismatch, expected {expected}, got {actual}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(bytes: &[u8]) -> String {
        let mut sha = Sha512::default();
        sha.update(bytes);
        hex(&sha.finish())
    }

    #[test]
    fn known_digests() {
        assert_eq!(
            digest(b""),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
        assert_eq!(
            digest(b"abc"),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            digest(b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"),
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"
        );
    }

    #[test]
    fn pieces_give_the_whole_digest() {
        let bytes: Vec<u8> = (0..1000_u32).map(|index| (index * 7 % 251) as u8).collect();
        let whole = digest(&bytes);
        for cut in [1, 63, 127, 128, 129, 500] {
            let mut sha = Sha512::default();
            for piece in bytes.chunks(cut) {
                sha.update(piece);
            }
            assert_eq!(hex(&sha.finish()), whole, "{cut}");
        }
    }

    #[test]
    fn base64_as_node_writes_it() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
