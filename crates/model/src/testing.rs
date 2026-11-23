//! Fixtures the unit tests share: scratch directories, the in-tree data
//! directory, the built payload and the toolchain's CPython as an oracle.
use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Collects mismatches, so one test can check many fields with a single
/// assertion at the end.
#[derive(Debug, Default)]
pub struct Checks(Vec<String>);

impl Checks {
    pub fn eq<A, E>(&mut self, label: &str, actual: &A, expected: &E)
    where
        A: PartialEq<E> + std::fmt::Debug + ?Sized,
        E: std::fmt::Debug + ?Sized,
    {
        if actual != expected {
            self.0.push(format!("{label}: {actual:?} != {expected:?}"));
        }
    }

    pub fn is(&mut self, label: &str, condition: bool) {
        if !condition {
            self.0.push(format!("{label}: false"));
        }
    }

    pub fn finish(self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(self.0.join("\n").into())
        }
    }
}

/// A directory under the system temporary directory, removed on drop.
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(label: &str) -> Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("model-{label}-{}-{unique}", std::process::id());
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Writes `bytes` at `relative`, creating its parent directories.
    pub fn write(&self, relative: &str, bytes: &[u8]) -> Result<()> {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(std::fs::write(path, bytes)?)
    }

    pub fn mkdir(&self, relative: &str) -> Result<()> {
        Ok(std::fs::create_dir_all(self.0.join(relative))?)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A minimal query-protocol `service-2.json` with one operation per name.
pub fn service_json(version: &str, operations: &[&str]) -> Vec<u8> {
    let operations: Vec<String> = operations
        .iter()
        .map(|name| format!(r#""{name}":{{"name":"{name}","http":{{"method":"POST"}}}}"#))
        .collect();
    format!(
        concat!(
            r#"{{"version":"2.0","metadata":{{"apiVersion":"{version}","protocol":"query"}},"#,
            r#""operations":{{{operations}}},"shapes":{{}}}}"#
        ),
        version = version,
        operations = operations.join(",")
    )
    .into_bytes()
}

/// The repository's vendored botocore package.
pub fn botocore_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../aws-cli/awscli/botocore")
}

/// botocore's built-in data directory in the repository.
pub fn data_dir() -> PathBuf {
    botocore_dir().join("data")
}

/// The fallback payload the product embeds, as `cargo xtask` provides it.
pub fn payload() -> Result<Vec<u8>> {
    let prefix = std::env::var_os("AWS_NATIVE_FALLBACK_PREFIX")
        .ok_or("AWS_NATIVE_FALLBACK_PREFIX must name the fallback payload prefix")?;
    Ok(std::fs::read(PathBuf::from(prefix).join("awscli-v2.blob"))?)
}

/// Runs `script` in the toolchain's CPython (the interpreter the payload
/// embeds) in isolated mode, feeding `input` on stdin.
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
