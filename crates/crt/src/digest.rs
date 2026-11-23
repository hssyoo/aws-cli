//! Nothing but its tests: the `sha256` crate, which signs every request,
//! checked against aws-c-cal's SHA-256 and HMAC-SHA256, an independent
//! implementation, over a few thousand pseudo-random inputs beyond the
//! crate's fixed vectors.
mod tests;
