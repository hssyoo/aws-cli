//! Where service models come from: botocore's `Loader`, split into
//! locations ([`Source`]) and the lookups every location shares.
use std::path::{Path, PathBuf};

/// A model file kind, by botocore `type_name`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Kind {
    Service,
    Paginators,
    Waiters,
    EndpointRuleSet,
}

impl Kind {
    /// The file stem botocore loads, e.g. `service-2`.
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::Service => "service-2",
            Self::Paginators => "paginators-1",
            Self::Waiters => "waiters-2",
            Self::EndpointRuleSet => "endpoint-rule-set-1",
        }
    }
}

/// One location botocore searches, or a whole search path.
///
/// Names are botocore data paths without `.json`, such as
/// `sts/2011-06-15/service-2` or `partitions`.
pub trait Source {
    /// The services holding a `kind` model in some version, sorted
    /// (`Loader.list_available_services`).
    fn services(&self, kind: Kind) -> Result<Vec<String>, LoadError>;

    /// The service's versions holding a `kind` model, sorted
    /// (`Loader.list_api_versions`, before its "not found" check).
    fn versions(&self, service: &str, kind: Kind) -> Result<Vec<String>, LoadError>;

    /// The bytes of `<name>.json`, or `None` when this source has no such
    /// file (`JSONFileLoader.load_file`).
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, LoadError>;

    /// Whether [`Source::read`] would find `<name>.json`, without reading
    /// it. A source that can tell more cheaply says so.
    fn contains(&self, name: &str) -> Result<bool, LoadError> {
        Ok(self.read(name)?.is_some())
    }
}

/// What a model lookup can fail with.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// botocore's `UnknownServiceError`.
    UnknownService(String),
    /// botocore's `DataNotFoundError`, naming the data path.
    DataNotFound(String),
    /// A data path with an empty, `.` or `..` segment or a NUL byte.
    InvalidName(String),
    /// A payload resource could not be read.
    Resource(crate::payload::ResourceError),
    /// The payload's catalog could not be walked.
    Catalog(pyload::catalog::GroupError),
    /// A directory or file could not be read.
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// A file is not JSON.
    Json {
        name: String,
        error: serde_json::Error,
    },
    /// A service model is not one the arena can hold.
    Model(crate::shapes::ModelError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownService(name) => write!(f, "unknown service: {name}"),
            Self::DataNotFound(name) => write!(f, "unable to load data for: {name}"),
            Self::InvalidName(name) => write!(f, "invalid data path: {name:?}"),
            Self::Resource(error) => error.fmt(f),
            Self::Catalog(error) => error.fmt(f),
            Self::Io { path, error } => write!(f, "{}: {error}", path.display()),
            Self::Json { name, error } => write!(f, "{name}: {error}"),
            Self::Model(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<crate::payload::ResourceError> for LoadError {
    fn from(error: crate::payload::ResourceError) -> Self {
        Self::Resource(error)
    }
}

impl From<pyload::catalog::GroupError> for LoadError {
    fn from(error: pyload::catalog::GroupError) -> Self {
        Self::Catalog(error)
    }
}

impl From<crate::shapes::ModelError> for LoadError {
    fn from(error: crate::shapes::ModelError) -> Self {
        Self::Model(error)
    }
}

/// Rejects data paths that could leave a location.
pub(crate) fn check_name(name: &str) -> Result<(), LoadError> {
    let valid = !name.contains('\0')
        && name
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."));
    valid
        .then_some(())
        .ok_or_else(|| LoadError::InvalidName(name.into()))
}

/// `Loader.determine_latest_version`: the greatest version name.
pub fn latest_version(
    source: &impl Source,
    service: &str,
    kind: Kind,
) -> Result<String, LoadError> {
    let versions = source.versions(service, kind)?;
    versions
        .into_iter()
        .max()
        .ok_or_else(|| LoadError::DataNotFound(service.into()))
}

/// A service file with its extras merged, as `Loader.load_service_model`
/// returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    /// The API version directory it was read from.
    pub version: String,
    pub json: serde_json::Value,
}

/// `Loader.load_service_model(service, type_name)`: the service must be
/// listed, the latest version is taken, and each `sdk` extras file found
/// beside it is merged in.
pub fn load_service_model(
    source: &impl Source,
    service: &str,
    kind: Kind,
) -> Result<Document, LoadError> {
    let files = service_files(source, service, kind)?;
    let json = files.merged()?;
    Ok(Document {
        version: files.version,
        json,
    })
}

/// [`load_service_model`] for a consumer that parses the file itself: the
/// bytes as stored when no extras apply, the merged document otherwise.
pub fn load_service_bytes(
    source: &impl Source,
    service: &str,
    kind: Kind,
) -> Result<(String, Vec<u8>), LoadError> {
    let files = service_files(source, service, kind)?;
    if files.extras.is_none() {
        return Ok((files.version, files.bytes));
    }
    let merged = files.merged()?;
    let bytes = serde_json::to_vec(&merged).map_err(|error| LoadError::Json {
        name: files.name.clone(),
        error,
    })?;
    Ok((files.version, bytes))
}

/// `Loader.load_data(name)`: the first location's file, as stored.
pub fn load_data_bytes(source: &impl Source, name: &str) -> Result<Vec<u8>, LoadError> {
    source
        .read(name)?
        .ok_or_else(|| LoadError::DataNotFound(name.into()))
}

/// A service file and its extras file, as found.
struct ServiceFiles {
    version: String,
    name: String,
    bytes: Vec<u8>,
    extras: Option<Vec<u8>>,
}

impl ServiceFiles {
    fn merged(&self) -> Result<serde_json::Value, LoadError> {
        let mut json = parse(&self.name, &self.bytes)?;
        if let Some(extras) = &self.extras {
            merge_extras(
                &mut json,
                &parse(&format!("{}.sdk-extras", self.name), extras)?,
            );
        }
        Ok(json)
    }
}

fn service_files(
    source: &impl Source,
    service: &str,
    kind: Kind,
) -> Result<ServiceFiles, LoadError> {
    if !source.services(kind)?.iter().any(|known| known == service) {
        return Err(LoadError::UnknownService(service.into()));
    }
    let version = latest_version(source, service, kind)?;
    let name = format!("{service}/{version}/{}", kind.type_name());
    let bytes = load_data_bytes(source, &name)?;
    let extras = source.read(&format!("{name}.sdk-extras"))?;
    Ok(ServiceFiles {
        version,
        name,
        bytes,
        extras,
    })
}

fn parse(name: &str, bytes: &[u8]) -> Result<serde_json::Value, LoadError> {
    serde_json::from_slice(bytes).map_err(|error| LoadError::Json {
        name: name.into(),
        error,
    })
}

/// `ExtrasProcessor._process`: an extras file's `merge` object, if any, is
/// deep-merged into the model.
fn merge_extras(model: &mut serde_json::Value, extras: &serde_json::Value) {
    if let Some(merge) = extras.get("merge") {
        deep_merge(model, merge);
    }
}

/// `botocore.utils.deep_merge`: where both sides hold an object the merge
/// recurses, otherwise the extra value replaces the base's in place.
pub fn deep_merge(base: &mut serde_json::Value, extra: &serde_json::Value) {
    let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) else {
        return;
    };
    for (key, value) in extra {
        match base.get_mut(key) {
            Some(existing) if existing.is_object() && value.is_object() => {
                deep_merge(existing, value)
            }
            Some(existing) => *existing = value.clone(),
            None => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// A directory in botocore's data layout (`<service>/<version>/<type>.json`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `os.path.isdir(path)`; the empty path names no directory.
    fn is_location(&self) -> bool {
        !self.root.as_os_str().is_empty() && self.root.is_dir()
    }

    /// `os.listdir(path)`: entry names that are Unicode, in directory order.
    /// A name that is not Unicode cannot be a service or version a caller
    /// asks for, so it is skipped.
    fn list(path: &Path) -> Result<Vec<String>, LoadError> {
        let io = |error| LoadError::Io {
            path: path.to_owned(),
            error,
        };
        let mut names = Vec::new();
        for entry in std::fs::read_dir(path).map_err(io)? {
            if let Ok(name) = entry.map_err(io)?.file_name().into_string() {
                names.push(name);
            }
        }
        Ok(names)
    }

    fn model_file(&self, service: &str, version: &str, kind: Kind) -> PathBuf {
        self.root
            .join(service)
            .join(version)
            .join(format!("{}.json", kind.type_name()))
    }
}

impl Source for DirSource {
    /// `Loader.list_available_services` for one search path.
    fn services(&self, kind: Kind) -> Result<Vec<String>, LoadError> {
        if !self.is_location() {
            return Ok(Vec::new());
        }
        let mut services = Vec::new();
        for service in Self::list(&self.root)? {
            if !self.root.join(&service).is_dir() {
                continue;
            }
            let versions = Self::list(&self.root.join(&service))?;
            if versions
                .iter()
                .any(|version| self.model_file(&service, version, kind).is_file())
            {
                services.push(service);
            }
        }
        services.sort();
        Ok(services)
    }

    /// `Loader.list_api_versions` for one search path.
    fn versions(&self, service: &str, kind: Kind) -> Result<Vec<String>, LoadError> {
        if !self.is_location() || check_name(service).is_err() || service.contains('/') {
            return Ok(Vec::new());
        }
        let directory = self.root.join(service);
        if !directory.is_dir() {
            return Ok(Vec::new());
        }
        let mut versions: Vec<String> = Self::list(&directory)?
            .into_iter()
            .filter(|version| self.model_file(service, version, kind).is_file())
            .collect();
        versions.sort();
        Ok(versions)
    }

    /// `os.path.isfile`, as `JSONFileLoader.exists` asks.
    fn contains(&self, name: &str) -> Result<bool, LoadError> {
        check_name(name)?;
        Ok(self.is_location() && self.root.join(format!("{name}.json")).is_file())
    }

    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, LoadError> {
        check_name(name)?;
        if !self.is_location() {
            return Ok(None);
        }
        let path = self.root.join(format!("{name}.json"));
        if !path.is_file() {
            return Ok(None);
        }
        std::fs::read(&path)
            .map(Some)
            .map_err(|error| LoadError::Io { path, error })
    }
}

#[cfg(test)]
mod tests;
