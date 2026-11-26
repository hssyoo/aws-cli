//! The feature census: every model feature a set of botocore-format models
//! uses, as `family/part/...` names, each with the services that use it.
//!
//! A feature is a key botocore reads, crossed with what changes how it is
//! read: the protocol botocore resolves and the direction (`in` serialized,
//! `out` parsed, `err` parsed as a modeled error) for wire keys, the shape
//! type for the validator's keys.
//!
//! Keys the model loader ignores are no feature. Their lists are the
//! `*_IGNORED` constants of `model`'s sources, found by content, not path,
//! so a module move leaves this file unchanged. That makes the census rely
//! on those lists being right: `model`'s own tests check each list against
//! botocore's source. Any other key is a feature, so a key AWS adds becomes
//! one by itself.
//!
//! The models are read here with `serde_json` as botocore's `Loader` reads
//! them (latest version, `sdk-extras` merge), not through `model::source`,
//! so the check does not share the loader it checks.
use super::forms::{paginator_features, rule_set_features, waiter_features};
use super::walk::service_features;
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(crate) type Error = Box<dyn std::error::Error>;
/// Each feature, with the services that use it.
pub(crate) type Census = BTreeMap<String, BTreeSet<String>>;
pub(crate) type Features = BTreeSet<String>;

/// The loader's ignored keys and the shape keys that are structure, not
/// traits.
#[derive(Debug)]
pub(crate) struct Ignored {
    pub(crate) document: BTreeSet<String>,
    pub(crate) metadata: BTreeSet<String>,
    pub(crate) operation: BTreeSet<String>,
    pub(crate) http: BTreeSet<String>,
    pub(crate) shape: BTreeSet<String>,
    pub(crate) shape_spine: BTreeSet<String>,
    pub(crate) reference: BTreeSet<String>,
    pub(crate) client_context_param: BTreeSet<String>,
}

impl Ignored {
    /// The lists, each defined exactly once in the `.rs` files under
    /// `sources` (the `model` crate's `src`).
    pub(crate) fn read(sources: &Path) -> Result<Self, Error> {
        let mut texts = Vec::new();
        for path in rust_files(sources)? {
            texts.push((std::fs::read_to_string(&path)?, path));
        }
        let list = |name: &str| defined_list(&texts, name);
        Ok(Self {
            document: list("DOCUMENT_IGNORED")?,
            metadata: list("METADATA_IGNORED")?,
            operation: list("OPERATION_IGNORED")?,
            http: list("HTTP_IGNORED")?,
            shape: list("SHAPE_IGNORED")?,
            shape_spine: list("SHAPE")?,
            reference: list("REFERENCE_IGNORED")?,
            client_context_param: list("CLIENT_CONTEXT_PARAM_IGNORED")?,
        })
    }
}

fn rust_files(directory: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(rust_files(&path)?);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// The list `name` from the one text of `texts` that defines it.
pub(crate) fn defined_list(
    texts: &[(String, PathBuf)],
    name: &str,
) -> Result<BTreeSet<String>, Error> {
    let mut found = Vec::new();
    for (text, path) in texts {
        if let Some(list) = string_list(text, name)? {
            found.push((list, path));
        }
    }
    match found.len() {
        1 => Ok(found.remove(0).0),
        0 => Err(format!("no source defines `const {name}: &[&str]`").into()),
        _ => {
            let paths: Vec<_> = found.iter().map(|(_, path)| path.display()).collect();
            Err(format!("`{name}` is defined more than once: {paths:?}").into())
        }
    }
}

/// The string literals of `const <name>: &[&str] = &[...];` in `source`,
/// line comments stripped; `None` when `source` does not define it.
pub(crate) fn string_list(source: &str, name: &str) -> Result<Option<BTreeSet<String>>, Error> {
    let head = format!("const {name}: &[&str] = &[");
    let Some(start) = source.find(&head) else {
        return Ok(None);
    };
    if source[start + head.len()..].contains(&head) {
        return Err(format!("`{name}` is defined twice in one file").into());
    }
    let body = &source[start + head.len()..];
    let end = body
        .find("];")
        .ok_or_else(|| format!("`{name}` is not closed"))?;
    let code: Vec<&str> = body[..end]
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .collect();
    let items: BTreeSet<String> = code
        .join("\n")
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    if items.is_empty() {
        return Err(format!("`{name}` lists no key").into());
    }
    Ok(Some(items))
}

/// A data file and its `sdk-extras` file's `merge`, deep-merged
/// (`Loader.load_service_model`), or `None` when the file is absent.
pub(crate) fn load(directory: &Path, kind: &str) -> Result<Option<Json>, Error> {
    let read = |name: String| -> Result<Option<Json>, Error> {
        let path = directory.join(name);
        if !path.is_file() {
            return Ok(None);
        }
        let parsed = serde_json::from_slice(&std::fs::read(&path)?)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Some(parsed))
    };
    let Some(mut json) = read(format!("{kind}.json"))? else {
        return Ok(None);
    };
    if let Some(extras) = read(format!("{kind}.sdk-extras.json"))?
        && let Some(merge) = extras.get("merge")
    {
        deep_merge(&mut json, merge);
    }
    Ok(Some(json))
}

/// `botocore.utils.deep_merge`.
fn deep_merge(base: &mut Json, extra: &Json) {
    let (Some(base), Some(extra)) = (base.as_object_mut(), extra.as_object()) else {
        return;
    };
    for (key, value) in extra {
        match base.get_mut(key) {
            Some(existing) if existing.is_object() && value.is_object() => {
                deep_merge(existing, value);
            }
            Some(existing) => *existing = value.clone(),
            None => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Each service under `root` with the directory of its latest version
/// holding a `service-2.json` (`Loader.determine_latest_version`).
fn services(root: &Path) -> Result<Vec<(String, PathBuf)>, Error> {
    let mut services = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !path.is_dir() {
            continue;
        }
        let mut versions = Vec::new();
        for version in std::fs::read_dir(&path)? {
            let version = version?.path();
            if version.join("service-2.json").is_file() {
                versions.push(version);
            }
        }
        if let Some(latest) = versions.into_iter().max() {
            services.push((name.to_owned(), latest));
        }
    }
    services.sort();
    Ok(services)
}

/// The census of every service in the botocore data directory `root`. A
/// service's rule set, paginators and waiters come from its service
/// model's version, as a botocore client loads them.
pub(crate) fn census(root: &Path, ignored: &Ignored) -> Result<Census, Error> {
    let mut census = Census::new();
    for (service, directory) in services(root)? {
        let mut features = Features::new();
        let context = |error| format!("{service}: {error}");
        let model = load(&directory, "service-2").map_err(context)?;
        if let Some(json) = model {
            service_features(&json, ignored, &mut features);
        }
        if let Some(json) = load(&directory, "endpoint-rule-set-1").map_err(context)? {
            rule_set_features(&json, &mut features);
        }
        if let Some(json) = load(&directory, "paginators-1").map_err(context)? {
            paginator_features(&json, &mut features);
        }
        if let Some(json) = load(&directory, "waiters-2").map_err(context)? {
            waiter_features(&json, &mut features);
        }
        for feature in features {
            census.entry(feature).or_default().insert(service.clone());
        }
    }
    Ok(census)
}
