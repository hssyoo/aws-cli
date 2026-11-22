//! The TLS end of a loopback HTTPS server, for tests (`shim/testing.h`),
//! which build.rs compiles only for this crate's `testing` feature and never
//! for Windows, which has no s2n; this module is gated the same way.
use crate::AwsByteCursor;
use std::ffi::c_int;

/// `struct zaws_test_tls`: a test CA, a leaf it issued, and an s2n server
/// configuration presenting the leaf.
#[repr(C)]
#[derive(Debug)]
pub struct TestTls {
    _private: [u8; 0],
}

// SAFETY: a configuration is destroyed once, after every call that uses
// it has returned; `serve` borrows its buffers and socket for the call.
c_functions! {
    /// A new CA, leaf and configuration; null when any could not be made.
    pub fn zaws_test_tls_new() -> *mut TestTls;
    /// The CA's certificate, PEM, borrowed from `tls`.
    pub fn zaws_test_tls_ca(tls: *const TestTls) -> AwsByteCursor;
    /// Serves one connection on the connected socket `fd`, blocking: the
    /// handshake, one request read into `out`, `response` written, then
    /// reads until the client closes. True when the request was read and
    /// answered whole, and then `length` is its length.
    pub fn zaws_test_tls_serve(
        tls: *mut TestTls,
        fd: c_int,
        response: AwsByteCursor,
        out: *mut u8,
        capacity: usize,
        length: *mut usize,
    ) -> bool;
    pub fn zaws_test_tls_destroy(tls: *mut TestTls);
}
