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
    /// `--oracle`: the candidate's native route against its Python route.
    pub oracle: bool,
    pub check_corpus: bool,
    /// `--baseline SHA|PATH`: that binary (a commit's, built and cached) as
    /// A against the candidate as B, both on `--route` (default native).
    pub baseline: Option<String>,
    pub route: Option<Route>,
}

/// The options that take a value.
const VALUED: [&str; 9] = [
    "--a",
    "--baseline",
    "--route",
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
        if switch(flag, &mut options) {
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
            "--baseline" => options.baseline = Some(value),
            "--route" => options.route = Some(route(&value)?),
            _ => options.out = Some(PathBuf::from(value)),
        }
    }
    check(&options)?;
    Ok(options)
}

/// Sets the option `flag` names if it takes no value.
fn switch(flag: &str, options: &mut Options) -> bool {
    let set = match flag {
        "--self-test" => &mut options.self_test,
        "--oracle" => &mut options.oracle,
        "--check-corpus" => &mut options.check_corpus,
        _ => return false,
    };
    *set = true;
    true
}

/// Fails on options that do not go together.
fn check(options: &Options) -> Result<(), TaskError> {
    let alone = |set: Options| *options == set;
    let binary = options.aws_binary.clone();
    if options.self_test
        && !alone(Options {
            aws_binary: binary,
            self_test: true,
            ..Options::default()
        })
    {
        return Err(deps::error("--self-test takes only --aws-binary"));
    }
    if options.check_corpus
        && !alone(Options {
            check_corpus: true,
            ..Options::default()
        })
    {
        return Err(deps::error("--check-corpus takes no other option"));
    }
    if options.oracle && (options.a.is_some() || options.b.is_some()) {
        return Err(deps::error("--oracle sets both sides; drop --a and --b"));
    }
    let sides = options.oracle || options.a.is_some() || options.b.is_some();
    if options.baseline.is_some() && sides {
        return Err(deps::error(
            "--baseline sets both sides; drop --a, --b and --oracle",
        ));
    }
    if options.route.is_some() && options.baseline.is_none() {
        return Err(deps::error("--route goes with --baseline"));
    }
    Ok(())
}

fn route(text: &str) -> Result<Route, TaskError> {
    Route::parse(text)
        .ok_or_else(|| deps::error(format!("--route is native, python or unset, not {text:?}")))
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
