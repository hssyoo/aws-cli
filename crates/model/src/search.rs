//! The search path the CLI's botocore `Loader` reads models from, user models
//! included.
//!
//! On the Python route, `awscli/__init__.py` (lines 28-37) rewrites
//! `AWS_DATA_PATH` before botocore starts: each entry is
//! `expanduser(expandvars(entry))`, and the CLI's own data directory is
//! appended. botocore's `create_loader` then splits that value again and
//! expands each entry again, and `Loader.__init__` appends `~/.aws/models`
//! and its built-in data directory. Because the variable is always set and
//! botocore's `data_path` chain reads the environment before the config file
//! (`configprovider.py:55`), a profile's `data_path` is never read.
use crate::payload::PayloadSource;
use crate::source::{DirSource, Kind, LoadError, Source};
use pyload::View;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::PathBuf;

/// `os.pathsep`.
pub const SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

/// One place the search reads, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// A directory on disk. The empty path is kept, as botocore keeps it,
    /// and names no directory.
    Directory(PathBuf),
    /// The payload's `awscli/data`, where the Python route's `awscli/data`
    /// entry points.
    AwscliData,
    /// The payload's `awscli/botocore/data` (`Loader.BUILTIN_DATA_PATH`).
    BotocoreData,
}

/// Why the search path could not be built natively.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchError {
    /// The expander could not expand this text the way Python would (for
    /// instance `~user`, or `~` with `HOME` unset), so the Python route has
    /// to decide where it points.
    Unexpandable(Vec<u8>),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unexpandable(path) => {
                write!(
                    f,
                    "cannot expand data path {:?}",
                    String::from_utf8_lossy(path)
                )
            }
        }
    }
}

impl std::error::Error for SearchError {}

/// botocore's `Loader` over the CLI's search path: services are the union
/// of every location's, versions too, and a file comes from the first
/// location that has it.
#[derive(Clone, Debug)]
pub struct SearchSource<'a> {
    view: View<'a>,
    locations: Vec<Location>,
}

impl<'a> SearchSource<'a> {
    /// The search path the CLI builds from `AWS_DATA_PATH` as the process
    /// received it. `expand` is `os.path.expanduser(os.path.expandvars(path))`
    /// over the caller's environment and home directory; it returns `None`
    /// where it cannot match Python.
    pub fn for_cli(
        view: View<'a>,
        aws_data_path: Option<&OsStr>,
        expand: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
    ) -> Result<Self, SearchError> {
        let mut locations = Vec::new();
        if let Some(value) = aws_data_path {
            let awscli = expand_entries(value.as_encoded_bytes(), expand)?;
            let joined = awscli.join(&(SEPARATOR as u8));
            for entry in expand_entries(&joined, expand)? {
                locations.push(Location::Directory(path(entry)?));
            }
        }
        locations.push(Location::AwscliData);
        let home = expand(b"~").ok_or_else(|| SearchError::Unexpandable(b"~".to_vec()))?;
        locations.push(Location::Directory(path(home)?.join(".aws").join("models")));
        locations.push(Location::BotocoreData);
        Ok(Self { view, locations })
    }

    pub fn locations(&self) -> &[Location] {
        &self.locations
    }

    fn each<T>(
        &self,
        mut visit: impl FnMut(&dyn Source) -> Result<Option<T>, LoadError>,
    ) -> Result<Option<T>, LoadError> {
        for location in &self.locations {
            let found = match location {
                Location::Directory(root) => visit(&DirSource::new(root.clone()))?,
                Location::AwscliData => visit(&PayloadSource::awscli(self.view))?,
                Location::BotocoreData => visit(&PayloadSource::botocore(self.view))?,
            };
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }

    fn union(
        &self,
        list: impl Fn(&dyn Source) -> Result<Vec<String>, LoadError>,
    ) -> Result<Vec<String>, LoadError> {
        let mut all = BTreeSet::new();
        self.each(|source| {
            all.extend(list(source)?);
            Ok(None::<()>)
        })?;
        Ok(all.into_iter().collect())
    }
}

/// `[expanduser(expandvars(p)) for p in value.split(os.pathsep)]`.
fn expand_entries(
    value: &[u8],
    expand: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, SearchError> {
    value
        .split(|byte| *byte == SEPARATOR as u8)
        .map(|entry| expand(entry).ok_or_else(|| SearchError::Unexpandable(entry.to_vec())))
        .collect()
}

#[cfg(unix)]
fn path(bytes: Vec<u8>) -> Result<PathBuf, SearchError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn path(bytes: Vec<u8>) -> Result<PathBuf, SearchError> {
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|error| SearchError::Unexpandable(error.into_bytes()))
}

impl Source for SearchSource<'_> {
    fn services(&self, kind: Kind) -> Result<Vec<String>, LoadError> {
        self.union(|source| source.services(kind))
    }

    fn versions(&self, service: &str, kind: Kind) -> Result<Vec<String>, LoadError> {
        self.union(|source| source.versions(service, kind))
    }

    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, LoadError> {
        self.each(|source| source.read(name))
    }
}

#[cfg(test)]
mod tests;
