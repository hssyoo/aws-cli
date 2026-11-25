//! `cargo xtask diff`'s options.

use std::path::PathBuf;

use crate::{TaskError, deps};

/// A side's route: the value of `AWS_CLI_NATIVE_ROUTER`, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Native,
    Python,
    Unset,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Python => "python",
            Self::Unset => "unset",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Self::Native, Self::Python, Self::Unset]
            .into_iter()
            .find(|route| route.as_str() == text)
    }
}

/// A side's binary as given: the candidate, or a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binary {
    Candidate,
    Path(PathBuf),
}

/// `--a`'s or `--b`'s value: `BINARY:ROUTE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideSpec {
    pub binary: Binary,
    pub route: Route,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub a: Option<SideSpec>,
    pub b: Option<SideSpec>,
    /// `--aws-binary PATH`, the candidate, as `harness::binary` takes it.
    pub aws_binary: Vec<String>,
    pub corpus: Vec<PathBuf>,
    pub filter: Option<String>,
    pub known: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub self_test: bool,
}

/// The options that take a value.
const VALUED: [&str; 7] = [
    "--a",
    "--b",
    "--aws-binary",
    "--corpus",
    "--filter",
    "--known",
    "--out",
];

pub fn parse(args: &[String]) -> Result<Options, TaskError> {
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        if flag == "--self-test" {
            options.self_test = true;
            continue;
        }
        if !VALUED.contains(&flag.as_str()) {
            return Err(deps::error(format!("unknown diff option: {flag}")));
        }
        let value = args
            .next()
            .cloned()
            .ok_or_else(|| deps::error(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--a" => options.a = Some(side(&value)?),
            "--b" => options.b = Some(side(&value)?),
            "--aws-binary" => options.aws_binary = vec![flag.clone(), value],
            "--corpus" => options.corpus.push(PathBuf::from(value)),
            "--filter" => options.filter = Some(value),
            "--known" => options.known = Some(PathBuf::from(value)),
            _ => options.out = Some(PathBuf::from(value)),
        }
    }
    let only_binary = Options {
        aws_binary: options.aws_binary.clone(),
        self_test: true,
        ..Options::default()
    };
    if options.self_test && options != only_binary {
        return Err(deps::error("--self-test takes only --aws-binary"));
    }
    Ok(options)
}

/// `candidate:ROUTE` or `PATH:ROUTE`; a path may itself hold colons.
fn side(text: &str) -> Result<SideSpec, TaskError> {
    let parsed = text.rsplit_once(':').and_then(|(binary, route)| {
        let binary = match binary {
            "" => return None,
            "candidate" => Binary::Candidate,
            path => Binary::Path(PathBuf::from(path)),
        };
        Some(SideSpec {
            binary,
            route: Route::parse(route)?,
        })
    });
    parsed.ok_or_else(|| {
        deps::error(format!(
            "a side is candidate:ROUTE or PATH:ROUTE with ROUTE native, python or unset, \
             not {text:?}"
        ))
    })
}

#[cfg(test)]
mod tests;
