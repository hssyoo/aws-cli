//! Models embedded in the `pyload` payload, where the Python route reads
//! them: botocore's `awscli/botocore/data` and the CLI's `awscli/data`.
use crate::source::{Kind, LoadError, Source, check_name};
use pyload::View;
use pyload::catalog::{self, Layout};

/// botocore's built-in data directory (`Loader.BUILTIN_DATA_PATH`).
pub const BOTOCORE_DATA: &str = "awscli/botocore/data/";
/// The data directory `awscli/__init__.py` appends to `AWS_DATA_PATH`.
pub const AWSCLI_DATA: &str = "awscli/data/";

/// Why an embedded resource could not be named or read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResourceError {
    /// The service name is empty or holds a character other than an ASCII
    /// letter, digit or `-`.
    InvalidServiceName,
    /// No such resource.
    Missing,
    /// The declared length is one this target cannot address.
    InvalidLength,
    /// `DecodeFailed` or `OutputLengthMismatch`: a compressed resource is
    /// corrupt or does not decode to exactly its declared length.
    Decode(pyload::decompress::Error),
    /// A resource's bytes or its decoder scratch could not be reserved.
    OutOfMemory,
    Payload(pyload::Error),
    Catalog(catalog::GroupError),
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidServiceName => f.write_str("invalid service name"),
            Self::Missing => f.write_str("missing resource"),
            Self::InvalidLength => f.write_str("invalid resource length"),
            Self::Decode(error) => error.fmt(f),
            Self::OutOfMemory => f.write_str("out of memory"),
            Self::Payload(error) => write!(f, "invalid payload: {error}"),
            Self::Catalog(error) => write!(f, "invalid payload: {error}"),
        }
    }
}

impl std::error::Error for ResourceError {}

impl From<pyload::Error> for ResourceError {
    fn from(error: pyload::Error) -> Self {
        Self::Payload(error)
    }
}

impl From<catalog::GroupError> for ResourceError {
    fn from(error: catalog::GroupError) -> Self {
        Self::Catalog(error)
    }
}

/// The full name of the greatest version's `<basename>.json` for a service
/// in botocore's data directory, as `Loader.determine_latest_version` picks
/// it.
pub fn versioned_resource_name<'a>(
    view: View<'a>,
    service: &str,
    basename: &str,
) -> Result<&'a [u8], ResourceError> {
    if service.is_empty()
        || !service
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-')
    {
        return Err(ResourceError::InvalidServiceName);
    }
    let prefix = format!("{BOTOCORE_DATA}{service}/");
    let mut best: Option<&[u8]> = None;
    catalog::for_each_model_group(
        view,
        prefix.as_bytes(),
        basename.as_bytes(),
        Layout::Versions,
        |version| {
            if best.is_none_or(|old| old < version) {
                best = Some(version);
            }
            Ok::<(), ResourceError>(())
        },
    )?;
    let version = best.ok_or(ResourceError::Missing)?;
    let name = [
        prefix.as_bytes(),
        version,
        b"/",
        basename.as_bytes(),
        b".json",
    ]
    .concat();
    Ok(view.find(&name)?.ok_or(ResourceError::Missing)?.entry.name)
}

/// Copies or decompresses an embedded resource using its declared exact length.
///
/// It reserves the output and then the decoder scratch fallibly, so an exhausted heap returns
/// `OutOfMemory` instead of aborting. A frame that does not decode to exactly the declared length
/// returns the decoder's own error; `InvalidLength` is only a declared length this target
/// cannot address.
pub fn load_resource(view: View<'_>, name: &[u8]) -> Result<Vec<u8>, ResourceError> {
    let entry = view.find(name)?.ok_or(ResourceError::Missing)?.entry;
    let stored = view.stored_bytes(entry)?;
    if !entry.is_compressed() {
        let mut bytes = reserve(stored.len())?;
        bytes.extend_from_slice(stored);
        return Ok(bytes);
    }
    let len = usize::try_from(entry.uncompressed_len).map_err(|_| ResourceError::InvalidLength)?;
    let mut bytes = reserve(len)?;
    bytes.resize(len, 0);
    let mut scratch = reserve(pyload::decompress::ZSTD_BUFFER_LEN)?;
    pyload::decompress::decode_zstd(stored, &mut bytes, scratch.spare_capacity_mut())
        .map_err(ResourceError::Decode)?;
    Ok(bytes)
}

fn reserve(len: usize) -> Result<Vec<u8>, ResourceError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_| ResourceError::OutOfMemory)?;
    Ok(bytes)
}

/// A data directory inside the payload. Listing reads entry names only;
/// nothing is decompressed until a file is read.
#[derive(Clone, Copy, Debug)]
pub struct PayloadSource<'a> {
    view: View<'a>,
    prefix: &'static str,
}

impl<'a> PayloadSource<'a> {
    /// botocore's built-in models.
    pub const fn botocore(view: View<'a>) -> Self {
        Self {
            view,
            prefix: BOTOCORE_DATA,
        }
    }

    /// The CLI's own data directory.
    pub const fn awscli(view: View<'a>) -> Self {
        Self {
            view,
            prefix: AWSCLI_DATA,
        }
    }

    pub const fn prefix(&self) -> &'static str {
        self.prefix
    }

    fn groups(&self, prefix: &str, kind: Kind, layout: Layout) -> Result<Vec<String>, LoadError> {
        let mut found = Vec::new();
        let type_name = kind.type_name().as_bytes();
        catalog::for_each_model_group(self.view, prefix.as_bytes(), type_name, layout, |group| {
            // A name that is not UTF-8 cannot be one a caller asks for.
            if let Ok(name) = std::str::from_utf8(group) {
                found.push(name.to_owned());
            }
            Ok::<(), ResourceError>(())
        })?;
        // Entry names sort `a-b/` before `a/`; botocore sorts the names.
        found.sort();
        Ok(found)
    }
}

impl Source for PayloadSource<'_> {
    fn services(&self, kind: Kind) -> Result<Vec<String>, LoadError> {
        self.groups(self.prefix, kind, Layout::Services)
    }

    fn versions(&self, service: &str, kind: Kind) -> Result<Vec<String>, LoadError> {
        if check_name(service).is_err() || service.contains('/') {
            return Ok(Vec::new());
        }
        self.groups(
            &format!("{}{service}/", self.prefix),
            kind,
            Layout::Versions,
        )
    }

    /// The catalog's entry, found without decompressing it.
    fn contains(&self, name: &str) -> Result<bool, LoadError> {
        check_name(name)?;
        let full = format!("{}{name}.json", self.prefix);
        let found = self
            .view
            .find(full.as_bytes())
            .map_err(ResourceError::from)?;
        Ok(found.is_some())
    }

    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, LoadError> {
        check_name(name)?;
        let full = format!("{}{name}.json", self.prefix);
        match load_resource(self.view, full.as_bytes()) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(ResourceError::Missing) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests;
