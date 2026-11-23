//! Standard base64 with padding, as Python's `base64.b64encode` writes it.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `base64.b64encode(bytes).decode()`.
pub fn encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |group, (index, byte)| {
                group | u32::from(*byte) << (16 - 8 * index)
            });
        for position in 0..4 {
            if position <= chunk.len() {
                let index = (group >> (18 - 6 * position)) & 0x3f;
                text.push(char::from(ALPHABET[index as usize]));
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// `base64.b64decode(text)` (`binascii.a2b_base64`, not strict): bytes
/// outside the alphabet are skipped, padding does not end the input, and the
/// length is checked at the end. `Err` holds `binascii.Error`'s text.
pub fn decode(text: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    let (mut position, mut carry, mut pads) = (0_u8, 0_u32, 0_u8);
    for byte in text.bytes() {
        if byte == b'=' {
            if position >= 2 {
                pads = pads.saturating_add(1);
            }
            continue;
        }
        let Some(sextet) = ALPHABET.iter().position(|a| *a == byte) else {
            continue;
        };
        let sextet = u32::try_from(sextet).unwrap_or(0);
        pads = 0;
        let (out, next, left) = match position {
            0 => (None, 1, sextet),
            1 => (Some((carry << 2) | (sextet >> 4)), 2, sextet & 0x0f),
            2 => (Some((carry << 4) | (sextet >> 2)), 3, sextet & 0x03),
            _ => (Some((carry << 6) | sextet), 0, 0),
        };
        bytes.extend(out.map(|value| (value & 0xff) as u8));
        (position, carry) = (next, left);
    }
    match position {
        0 => Ok(bytes),
        _ if position + pads >= 4 => Ok(bytes),
        1 => Err(format!(
            "Invalid base64-encoded string: number of data characters ({}) cannot be 1 more \
             than a multiple of 4",
            bytes.len() / 3 * 4 + 1
        )),
        _ => Err("Incorrect padding".into()),
    }
}

#[cfg(test)]
mod tests;
