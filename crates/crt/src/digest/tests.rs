use crate::runtime::Library;
use crt_sys::cal::{aws_sha256_compute, aws_sha256_hmac_compute};
use crt_sys::{AwsByteBuf, AwsByteCursor, rust_crt_allocator};

/// Lengths at and around the 64-byte block and its 55/56-byte padding
/// boundary, and a few blocks on.
const EDGES: [usize; 16] = [
    0, 1, 2, 31, 32, 55, 56, 57, 63, 64, 65, 119, 120, 127, 128, 129,
];

/// splitmix64: a fixed seed, so a failure reproduces.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next().to_le_bytes()[0]).collect()
    }

    /// Bytes of a [`Self::len`] length.
    fn input(&mut self, limit: u64) -> Vec<u8> {
        let len = self.len(limit);
        self.bytes(len)
    }

    /// An edge length or, as often, any length below `limit`.
    fn len(&mut self, limit: u64) -> usize {
        let pick = self.next();
        if pick.is_multiple_of(2) {
            EDGES[usize::try_from(pick / 2 % 16).unwrap_or(0)]
        } else {
            usize::try_from(pick / 2 % limit).unwrap_or(0)
        }
    }
}

/// An empty buffer over `out`, which aws-c-cal appends the 32 bytes to.
fn output(out: &mut [u8; 32]) -> AwsByteBuf {
    AwsByteBuf {
        len: 0,
        buffer: out.as_mut_ptr(),
        capacity: out.len(),
        allocator: std::ptr::null_mut(),
    }
}

fn cal_digest(input: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    let mut buf = output(&mut out);
    let input = AwsByteCursor::from_bytes(input);
    // SAFETY: the library is initialized; the cursor borrows `input` and the
    // buffer has 32 bytes spare, both live for the call.
    let status = unsafe { aws_sha256_compute(rust_crt_allocator(), &input, &mut buf, 0) };
    assert_eq!((status, buf.len), (0, 32));
    out
}

fn cal_hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    let mut buf = output(&mut out);
    let (key, message) = (
        AwsByteCursor::from_bytes(key),
        AwsByteCursor::from_bytes(message),
    );
    // SAFETY: as in `cal_digest`.
    let status =
        unsafe { aws_sha256_hmac_compute(rust_crt_allocator(), &key, &message, &mut buf, 0) };
    assert_eq!((status, buf.len), (0, 32));
    out
}

#[test]
fn sha256_digests_as_aws_c_cal_does() {
    let _library = Library::acquire();
    let mut random = Random(0x5348_4132_3536);
    for _ in 0..3000 {
        let input = random.input(1500);
        assert_eq!(
            sha256::digest(&input),
            cal_digest(&input),
            "{} bytes",
            input.len()
        );
    }
    let mebibyte = random.bytes(1 << 20);
    assert_eq!(sha256::digest(&mebibyte), cal_digest(&mebibyte));
}

/// Keys around the block size, where HMAC switches from padding the key to
/// hashing it first.
#[test]
fn hmac_sha256_signs_as_aws_c_cal_does() {
    let _library = Library::acquire();
    let mut random = Random(0x484d_4143);
    for _ in 0..3000 {
        let key = random.input(200);
        let message = random.input(1500);
        assert_eq!(
            sha256::hmac(&key, &message),
            cal_hmac(&key, &message),
            "key {} bytes, message {} bytes",
            key.len(),
            message.len()
        );
    }
    let mebibyte = random.bytes(1 << 20);
    assert_eq!(
        sha256::hmac(&mebibyte[..64], &mebibyte),
        cal_hmac(&mebibyte[..64], &mebibyte)
    );
}
