//! aws-c-cal's SHA-256 and HMAC-SHA256, bound only for the tests that check
//! the `sha256` crate against an independent implementation (crt's
//! `digest` tests). No product build links these declarations.
use crate::{AwsAllocator, AwsByteBuf, AwsByteCursor};
use std::ffi::c_int;

unsafe extern "C" {
    /// Appends `input`'s digest to `output`, which must have 32 bytes
    /// spare; `truncate_to` 0 keeps all 32.
    pub fn aws_sha256_compute(
        allocator: *mut AwsAllocator,
        input: *const AwsByteCursor,
        output: *mut AwsByteBuf,
        truncate_to: usize,
    ) -> c_int;
    /// Appends HMAC-SHA256 of `to_hmac` under `secret` to `output`, as
    /// [`aws_sha256_compute`] does.
    pub fn aws_sha256_hmac_compute(
        allocator: *mut AwsAllocator,
        secret: *const AwsByteCursor,
        to_hmac: *const AwsByteCursor,
        output: *mut AwsByteBuf,
        truncate_to: usize,
    ) -> c_int;
}
