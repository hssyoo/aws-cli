//! The process's sources of randomness: the operating system's
//! ([`os_bytes`]), for ids that must not repeat across processes, such as
//! the uuids botocore makes with `uuid.uuid4()` from `os.urandom`; and a
//! cheaper non-cryptographic one ([`word`]), for retry jitter.

use std::hash::{BuildHasher, RandomState};

/// 64 random bits: the keys of a fresh `RandomState`, which the standard
/// library draws from the operating system once per thread and then
/// advances for each new state.
pub fn word() -> u64 {
    RandomState::new().hash_one(0)
}

/// Fills an array from the operating system's random source.
#[cfg(target_os = "linux")]
pub fn os_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut bytes = [0; N];
    let mut filled = 0;
    while filled < N {
        let rest = &mut bytes[filled..];
        // SAFETY: the kernel writes at most `rest.len()` bytes to the live
        // slice `rest` points at.
        let written = unsafe { libc::getrandom(rest.as_mut_ptr().cast(), rest.len(), 0) };
        match usize::try_from(written) {
            Ok(written) => filled += written,
            Err(_) => {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
    Ok(bytes)
}
/// Fills an array from the operating system's random source.
#[cfg(target_vendor = "apple")]
pub fn os_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut bytes = [0; N];
    // SAFETY: arc4random_buf writes exactly `N` bytes to the live array and
    // cannot fail.
    unsafe { libc::arc4random_buf(bytes.as_mut_ptr().cast(), N) };
    Ok(bytes)
}
/// Fills an array from the operating system's random source.
#[cfg(all(unix, not(target_os = "linux"), not(target_vendor = "apple")))]
pub fn os_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    use std::io::Read;
    let mut bytes = [0; N];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}
/// Fills an array from the operating system's random source.
#[cfg(windows)]
pub fn os_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            output: *mut u8,
            size: u32,
            flags: u32,
        ) -> i32;
    }
    let size =
        u32::try_from(N).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut bytes = [0; N];
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;
    // SAFETY: the system-preferred provider writes exactly size bytes to the live array.
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            size,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(std::io::Error::other(format!(
            "random generator failed: {status:#x}"
        )));
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests;
