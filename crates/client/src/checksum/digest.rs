//! The digests botocore's checksum classes compute, hand-rolled to keep
//! the dependency set small: the reflected CRCs the CRT computes
//! (`awscrt.checksums.crc32`, `crc32c`, `crc64nvme`, each started from 0)
//! and SHA-1 (FIPS 180-4) as `hashlib.sha1` does. SHA-256 is the
//! workspace's own (`sha256`, through the signer).

/// A reflected CRC-32's table for the reflected polynomial `poly`.
const fn table32(poly: u32) -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ poly
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

/// A reflected CRC-64's table for the reflected polynomial `poly`.
const fn table64(poly: u64) -> [u64; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u64;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ poly
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

/// CRC-32 (ISO-HDLC), `binascii.crc32`'s.
const CRC32: [u32; 256] = table32(0xEDB8_8320);
/// CRC-32C (Castagnoli, iSCSI).
const CRC32C: [u32; 256] = table32(0x82F6_3B78);
/// CRC-64/NVME.
const CRC64NVME: [u64; 256] = table64(0x9A6C_9329_AC4B_C9B5);

fn crc32_with(table: &[u32; 256], bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc = table[usize::from(crc.to_le_bytes()[0] ^ byte)] ^ (crc >> 8);
    }
    !crc
}

/// `binascii.crc32(bytes)`.
pub(super) fn crc32(bytes: &[u8]) -> u32 {
    crc32_with(&CRC32, bytes)
}

/// `awscrt.checksums.crc32c(bytes)`.
pub(super) fn crc32c(bytes: &[u8]) -> u32 {
    crc32_with(&CRC32C, bytes)
}

/// `awscrt.checksums.crc64nvme(bytes)`.
pub(super) fn crc64nvme(bytes: &[u8]) -> u64 {
    let mut crc = !0_u64;
    for byte in bytes {
        crc = CRC64NVME[usize::from(crc.to_le_bytes()[0] ^ byte)] ^ (crc >> 8);
    }
    !crc
}

/// `hashlib.sha1(bytes).digest()`.
pub(super) fn sha1(bytes: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let bits = u64::try_from(bytes.len())
        .unwrap_or(u64::MAX)
        .wrapping_mul(8);
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bits.to_be_bytes());
    for block in padded.chunks_exact(64) {
        sha1_block(&mut state, block);
    }
    let mut digest = [0; 20];
    for (out, word) in digest.chunks_exact_mut(4).zip(state) {
        out.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

fn sha1_block(state: &mut [u32; 5], block: &[u8]) {
    let mut w = [0_u32; 80];
    for (word, bytes) in w.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    }
    for t in 16..80 {
        w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (t, word) in w.iter().enumerate() {
        let (f, k) = match t {
            0..20 => ((b & c) | (!b & d), 0x5A82_7999),
            20..40 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..60 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*word);
        (e, d, c, b, a) = (d, c, b.rotate_left(30), a, next);
    }
    for (slot, value) in state.iter_mut().zip([a, b, c, d, e]) {
        *slot = slot.wrapping_add(value);
    }
}

/// `hashlib.sha256(bytes).digest()`, from the signer's hex.
pub(super) fn sha256(bytes: &[u8]) -> Vec<u8> {
    let hex = sigv4::sha256_hex(bytes);
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| (nibble(pair[0]) << 4) | nibble(pair[1]))
        .collect()
}

/// A lowercase hex digit's value.
const fn nibble(digit: u8) -> u8 {
    match digit {
        b'a'..=b'f' => digit - b'a' + 10,
        _ => digit.wrapping_sub(b'0'),
    }
}
