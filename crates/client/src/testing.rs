//! Deterministic stand-ins for the network and the process, for tests of
//! this crate and of those built on it (feature `testing`).
use crate::http::{HttpRequest, HttpResponse};
use crate::runtime::{Interrupted, Runtime};
use crate::transport::{Timeouts, Transport, TransportError};
use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use timestamp::Timestamp;

pub mod facade;
pub mod fuzz;
#[cfg(test)]
mod tests;

/// Whether the workspace's source defines the item a [`crate::hooks::Basis`]
/// path names. The first segment names a crate (`service_command` is
/// `crates/service-command/src`), the segments after it name modules by
/// file (`a::b` is `a/b.rs` or `a/b/mod.rs`, which its parent's file
/// declares with `mod b`), and the last names an item defined at the top
/// of that module's file (a `fn`, `struct`, `enum`, `trait`, `type`,
/// `const` or `static`) or re-exported there by a `pub use`. When the
/// second-to-last segment names a type or trait instead, the last is a
/// method: a `fn` directly in an `impl` of that type, or in the trait, in
/// that module's file (`client::call::Client::prepare` is `prepare` in an
/// `impl Client` in `crates/client/src/call.rs`). Comments never declare.
/// `workspace` is the repository's root.
///
/// The source is read line by line as rustfmt lays it out, and these do
/// not resolve: inline `mod x { .. }` and `#[path]` modules, glob and
/// nested-brace `pub use`, a method whose `impl` is in another module, and
/// a method reached through a re-exported type.
///
/// # Errors
/// A source file that cannot be read, or an ambiguous path: one whose
/// module has two files, that names both an item and a method, or whose
/// method two trait `impl`s define and no inherent `impl` does.
pub fn defines(workspace: &Path, path: &str) -> io::Result<bool> {
    let segments: Vec<&str> = path.split("::").collect();
    let resolver = Resolver { workspace, path };
    match segments.as_slice() {
        [krate, rest @ ..] if !rest.is_empty() => match resolver.crate_root(krate) {
            Some(root) => resolver.resolve(&root, &[&["crate"], rest].concat(), 0),
            None => Ok(false),
        },
        _ => Ok(false),
    }
}

/// The item kinds [`defines`] recognizes.
const KINDS: [&str; 7] = ["fn", "struct", "enum", "trait", "type", "const", "static"];

/// How many `pub use` re-exports [`defines`] follows from one path.
const REEXPORT_DEPTH: usize = 8;

/// A module: its crate's `src` directory and its path from the crate root.
#[derive(Clone)]
struct Module {
    src: PathBuf,
    path: Vec<String>,
}

/// Resolves one [`defines`] path, which its errors name.
struct Resolver<'a> {
    workspace: &'a Path,
    path: &'a str,
}

impl Resolver<'_> {
    fn ambiguous(&self, why: &str) -> io::Error {
        io::Error::other(format!("Basis path `{}` is ambiguous: {why}", self.path))
    }

    fn crate_root(&self, krate: &str) -> Option<Module> {
        let src = self
            .workspace
            .join("crates")
            .join(krate.replace('_', "-"))
            .join("src");
        src.is_dir().then_some(Module {
            src,
            path: Vec::new(),
        })
    }

    /// The file that holds `module`, if one does.
    fn file(&self, module: &Module) -> io::Result<Option<PathBuf>> {
        let Some((last, parents)) = module.path.split_last() else {
            let root = [module.src.join("lib.rs"), module.src.join("main.rs")];
            return Ok(root.into_iter().find(|file| file.is_file()));
        };
        let directory = parents
            .iter()
            .fold(module.src.clone(), |at, name| at.join(name));
        let files = [
            directory.join(format!("{last}.rs")),
            directory.join(last).join("mod.rs"),
        ];
        let found: Vec<PathBuf> = files.into_iter().filter(|file| file.is_file()).collect();
        match found.as_slice() {
            [] => Ok(None),
            [file] => Ok(Some(file.clone())),
            _ => Err(self.ambiguous(&format!(
                "module `{}` has two files",
                module.path.join("::")
            ))),
        }
    }

    /// The module `segments` name from `from`: `crate`, `self`, `super`,
    /// a child module or, first, another crate.
    fn module(&self, from: &Module, segments: &[&str]) -> io::Result<Option<Module>> {
        let mut module = from.clone();
        for (index, segment) in segments.iter().enumerate() {
            match *segment {
                "crate" => module.path.clear(),
                "self" => {}
                "super" => {
                    if module.path.pop().is_none() {
                        return Ok(None);
                    }
                }
                name => {
                    let mut child = module.clone();
                    child.path.push(name.to_owned());
                    if self.file(&child)?.is_some() && self.declares_module(&module, name)? {
                        module = child;
                    } else if let Some(root) = self.crate_root(name).filter(|_| index == 0) {
                        module = root;
                    } else {
                        return Ok(None);
                    }
                }
            }
        }
        Ok(Some(module))
    }

    /// Whether `segments`, read from `from`, name an item or a method, and
    /// not both.
    fn resolve(&self, from: &Module, segments: &[&str], depth: usize) -> io::Result<bool> {
        let Some((item, parents)) = segments.split_last() else {
            return Ok(false);
        };
        if depth > REEXPORT_DEPTH {
            return Ok(false);
        }
        let as_item = match self.module(from, parents)? {
            Some(module) => self.item(&module, item, depth)?,
            None => false,
        };
        let as_method = match parents.split_last() {
            Some((owner, modules)) => match self.module(from, modules)? {
                Some(module) => self.method(&module, owner, item)?,
                None => false,
            },
            None => false,
        };
        if as_item && as_method {
            let named = segments.join("::");
            return Err(self.ambiguous(&format!("`{named}` names an item and a method")));
        }
        Ok(as_item || as_method)
    }

    /// Whether `parent`'s file declares `mod name` at its top.
    fn declares_module(&self, parent: &Module, name: &str) -> io::Result<bool> {
        let source = self.source(parent)?.unwrap_or_default();
        Ok(source
            .lines()
            .any(|line| !indented(line) && declares(line, &["mod"], name)))
    }

    /// The source of the file that holds `module`, if one does.
    fn source(&self, module: &Module) -> io::Result<Option<String>> {
        self.file(module)?.map(std::fs::read_to_string).transpose()
    }

    /// Whether `module` defines `name` at its top or names it in a
    /// `pub use` that resolves.
    fn item(&self, module: &Module, name: &str, depth: usize) -> io::Result<bool> {
        let Some(source) = self.source(module)? else {
            return Ok(false);
        };
        if source
            .lines()
            .any(|line| !indented(line) && declares(line, &KINDS, name))
        {
            return Ok(true);
        }
        for (visible, target) in reexports(&source) {
            let target: Vec<&str> = target.iter().map(String::as_str).collect();
            if visible == name && self.resolve(module, &target, depth + 1)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether `module` has a `fn method` directly in an inherent `impl` of
    /// `owner` or in the trait `owner`, or else in exactly one trait `impl`
    /// for `owner` (in two, the path is ambiguous).
    fn method(&self, module: &Module, owner: &str, method: &str) -> io::Result<bool> {
        let Some(source) = self.source(module)? else {
            return Ok(false);
        };
        let (mut lines, mut trait_impls) = (source.lines(), 0);
        while let Some(header) = lines.next() {
            if indented(header) || header.trim_end().ends_with('}') {
                continue;
            }
            let Some((name, trait_impl)) = owner_of(header) else {
                continue;
            };
            let block = lines.by_ref().take_while(|line| !line.starts_with('}'));
            let member = |line: &&str| line.starts_with("    ") && !indented(&line[4..]);
            if name == owner
                && block
                    .filter(member)
                    .any(|line| declares(line, &KINDS, method))
            {
                if !trait_impl {
                    return Ok(true);
                }
                trait_impls += 1;
            }
        }
        if trait_impls > 1 {
            return Err(self.ambiguous(&format!(
                "{trait_impls} trait impls for `{owner}` define `{method}`"
            )));
        }
        Ok(trait_impls == 1)
    }
}

fn indented(line: &str) -> bool {
    line.starts_with(char::is_whitespace)
}

/// The leading identifier of `text`.
fn identifier(text: &str) -> &str {
    let end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    &text[..end]
}

/// Whether `line`, outside its `//` comment, declares a `kinds` item
/// called `name`.
fn declares(line: &str, kinds: &[&str], name: &str) -> bool {
    let words: Vec<&str> = code(line).split_whitespace().collect();
    words
        .windows(2)
        .any(|pair| kinds.contains(&pair[0]) && identifier(pair[1]) == name)
}

/// `line` up to any `//` comment (`///` and `//!` docs included).
fn code(line: &str) -> &str {
    line.split("//").next().unwrap_or_default()
}

/// The type an `impl` header implements for, with whether the `impl` is
/// of a trait, or the trait a `trait` header declares; read with its
/// generics removed, and only from a header that opens with the keyword
/// (after `pub` or `unsafe`).
fn owner_of(header: &str) -> Option<(String, bool)> {
    let mut depth = 0;
    let bare: String = code(header)
        .chars()
        .filter(|&c| {
            depth += i32::from(c == '<') - i32::from(c == '>');
            depth == 0 && c != '>'
        })
        .collect();
    let words: Vec<&str> = bare.split_whitespace().collect();
    let keyword = words
        .iter()
        .position(|word| !(word.starts_with("pub") || *word == "unsafe"))
        .filter(|&at| matches!(words[at], "impl" | "trait"))?;
    let named = match words[keyword] {
        "impl" => words.iter().rposition(|word| *word == "for"),
        _ => None,
    };
    let target = words.get(named.unwrap_or(keyword) + 1)?;
    let last = target.trim_start_matches('&').rsplit("::").next()?;
    Some((identifier(last).to_owned(), named.is_some()))
}

/// Each name a top-level `pub use` in `source` makes visible
/// (`pub use a::{b, c as d};`), with the path it names.
fn reexports(source: &str) -> Vec<(String, Vec<String>)> {
    let mut found = Vec::new();
    let mut statement: Option<String> = None;
    for line in source.lines() {
        let line = code(line);
        if let Some(text) = statement.as_mut() {
            text.push_str(line);
        } else if !indented(line) {
            let mut words = line.split_whitespace();
            if words.next().is_some_and(|word| word.starts_with("pub"))
                && words.next() == Some("use")
            {
                statement = line.split_once(" use ").map(|(_, tree)| tree.to_owned());
            }
        }
        let Some(text) = statement.take_if(|text| text.contains(';')) else {
            continue;
        };
        let tree = text.split(';').next().unwrap_or_default();
        let (prefix, leaves) = match tree.split_once('{') {
            Some((prefix, leaves)) => (prefix, leaves.trim_end_matches(['}', ' '])),
            None => ("", tree),
        };
        for leaf in leaves.split(',').filter(|leaf| !leaf.trim().is_empty()) {
            let (path, alias) = leaf.split_once(" as ").unwrap_or((leaf, ""));
            let target: Vec<String> = format!("{prefix}::{path}")
                .split("::")
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .collect();
            let visible = Some(alias.trim()).filter(|alias| !alias.is_empty());
            if let Some(visible) = visible.or(target.last().map(String::as_str)) {
                found.push((visible.to_owned(), target.clone()));
            }
        }
    }
    found
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Answers each request with the next scripted result, in order, and
/// records every request with its timeouts. A request beyond the script
/// fails as `TransportError::Client`.
#[derive(Debug, Default)]
pub struct InMemoryTransport {
    script: Mutex<VecDeque<Result<HttpResponse, TransportError>>>,
    sent: Mutex<Vec<(HttpRequest, Timeouts)>>,
}

impl InMemoryTransport {
    pub fn new() -> Self {
        Self::default()
    }
    /// Answers a later request with `response`.
    pub fn respond(&self, response: HttpResponse) {
        lock(&self.script).push_back(Ok(response));
    }
    /// Fails a later request with `error`.
    pub fn fail(&self, error: TransportError) {
        lock(&self.script).push_back(Err(error));
    }
    /// Every request sent so far, with its timeouts.
    pub fn sent(&self) -> Vec<(HttpRequest, Timeouts)> {
        lock(&self.sent).clone()
    }
}

impl Transport for InMemoryTransport {
    fn send(
        &self,
        request: &HttpRequest,
        timeouts: Timeouts,
    ) -> Result<HttpResponse, TransportError> {
        lock(&self.sent).push((request.clone(), timeouts));
        lock(&self.script).pop_front().unwrap_or_else(|| {
            Err(TransportError::Client {
                reason: "the in-memory transport has no response left".to_owned(),
            })
        })
    }
}

/// A runtime that never waits: a fixed clock, uuids numbered from 1, one
/// fixed draw, and sleeps that are recorded instead of slept.
#[derive(Debug)]
pub struct FixedRuntime {
    now: Timestamp,
    random: f64,
    issued: Mutex<u64>,
    slept: Mutex<Vec<Duration>>,
}

impl FixedRuntime {
    pub fn new(now: Timestamp, random: f64) -> Self {
        Self {
            now,
            random,
            issued: Mutex::new(0),
            slept: Mutex::new(Vec::new()),
        }
    }
    /// Every sleep asked for so far.
    pub fn slept(&self) -> Vec<Duration> {
        lock(&self.slept).clone()
    }
}

impl Runtime for FixedRuntime {
    fn now(&self) -> Timestamp {
        self.now
    }
    fn uuid4(&self) -> String {
        let mut issued = lock(&self.issued);
        *issued += 1;
        format!("00000000-0000-4000-8000-{:012x}", *issued)
    }
    fn sleep(&self, duration: Duration) -> Result<(), Interrupted> {
        lock(&self.slept).push(duration);
        Ok(())
    }
    fn random(&self) -> f64 {
        self.random
    }
}
