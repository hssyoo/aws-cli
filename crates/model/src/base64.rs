//! Standard base64 with padding, as Python's `base64.b64encode` writes it
//! and `b64decode` reads it: [`pytext::base64`].
pub use pytext::base64::{decode, encode};
