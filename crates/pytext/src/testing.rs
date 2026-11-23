//! The toolchain's CPython as an oracle, for this crate's tests and those
//! of the crates built on it (feature `testing`).
use std::io::Write;
use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Runs `script` in the toolchain's CPython (the interpreter the payload
/// embeds) in isolated mode, with a cleared environment, feeding `input` on
/// stdin.
pub fn python(script: &str, input: &[u8]) -> Result<Vec<u8>> {
    let prefix = std::env::var_os("AWS_NATIVE_CPYTHON_PREFIX")
        .ok_or("AWS_NATIVE_CPYTHON_PREFIX must name the toolchain CPython")?;
    let mut child = std::process::Command::new(PathBuf::from(prefix).join("bin/python3"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(["-I", "-c", script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    child.stdin.take().ok_or("python stdin")?.write_all(input)?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(output.stdout)
}
