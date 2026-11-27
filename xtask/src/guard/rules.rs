//! The guard's rules over [`Inputs`]; see the module documentation of
//! `guard` for what each checks.

use std::collections::BTreeMap;

use super::attrs::{self, Source};
use super::diff::{Commit, FileLines, crate_of, is_test_file, matches};
use super::gates;
use super::{Inputs, PROTECTED_LIST};

pub fn check(inputs: &Inputs) -> Vec<String> {
    let mut violations = protected(inputs);
    violations.extend(suppressions(inputs));
    violations.extend(test_counts(inputs));
    violations.extend(deleted_tests(inputs));
    violations.extend(module_removals(inputs));
    violations.extend(asserts(inputs));
    violations
}

fn protected(inputs: &Inputs) -> Vec<String> {
    let mut violations = Vec::new();
    for change in &inputs.changes {
        for path in change.paths() {
            if let Some(pattern) = inputs
                .protected
                .iter()
                .find(|pattern| matches(pattern, path))
            {
                violations.push(format!(
                    "guard: protected: {path} ({}) matches `{pattern}` in {PROTECTED_LIST}; \
                     protected paths hold the oracles, so revert the change, or have it \
                     approved as a protected change on its own",
                    change.status
                ));
            }
        }
    }
    violations
}

fn source<'a>(path: &str, text: &'a str) -> Source<'a> {
    if path.ends_with(".rs") {
        Source::rust(text)
    } else {
        Source::plain(text)
    }
}

fn suppressions(inputs: &Inputs) -> Vec<String> {
    let mut violations = Vec::new();
    for file in &inputs.lines {
        let Some(path) = file.new.as_deref().filter(|path| path.ends_with(".rs")) else {
            continue;
        };
        let Some(text) = inputs.head_sources.get(path) else {
            continue;
        };
        let source = Source::rust(text);
        let names = file
            .old
            .as_deref()
            .and_then(|old| inputs.base_sources.get(old))
            .map(|base| gates::assert_macros(&Source::rust(base)))
            .unwrap_or_default();
        let mut found = attrs::suppressions(&source);
        found.extend(gates::escapes(&source, &names));
        for found in found {
            let added = file
                .added
                .iter()
                .any(|(line, _)| (found.first_line..=found.last_line).contains(line));
            if added && applies(inputs, path, &found) && !excused(inputs, &source, path, &found) {
                violations.push(format!(
                    "guard: suppression: {path}:{}: `{}` ({}) has no justification; say why \
                     in a `//` comment on its line or above its attributes, or {}",
                    found.first_line,
                    found.text,
                    found.kind,
                    remedy(found.kind)
                ));
            }
        }
    }
    violations
}

/// Whether a finding whose effect depends on where it is applies there: a
/// `cfg` not known true or a `#[path]` on a test function or a module that
/// holds tests (or, inner, on a test file), and an exit in a test.
fn applies(inputs: &Inputs, path: &str, found: &attrs::Found) -> bool {
    match found.kind {
        "cfg" | "path" => {
            let module = found.module.as_ref().is_some_and(|(name, attribute)| {
                let default = module_target(path, name, None);
                let target = module_target(path, name, attribute.as_deref());
                let moved = found.kind == "cfg" || target != default;
                moved
                    && (holds_tests(&inputs.head_files, &inputs.head_sources, &default)
                        || holds_tests(&inputs.base_files, &inputs.base_sources, &default))
            });
            (found.kind == "cfg" && found.test_item)
                || module
                || (found.inner && is_test_file(path))
        }
        "exit" => found.test_item || is_test_file(path),
        _ => true,
    }
}

fn excused(inputs: &Inputs, source: &Source<'_>, path: &str, found: &attrs::Found) -> bool {
    let skips = matches!(found.kind, "shadow" | "exit" | "return");
    justified(source, found) || (skips && trailer(&inputs.commits, "Asserts-changed", path))
}

/// The file `mod name;` in `parent` loads: its `#[path]`, relative to the
/// parent's directory, or the default one.
pub fn module_target(parent: &str, name: &str, attribute: Option<&str>) -> String {
    match (attribute, parent.rsplit_once('/')) {
        (Some(path), Some((directory, _))) => format!("{directory}/{path}"),
        (Some(path), None) => path.to_owned(),
        (None, _) => child_path(parent, name),
    }
}

/// Whether a module file is a test file, holds test functions, or has a
/// test file below its directory.
pub fn holds_tests(files: &[String], sources: &BTreeMap<String, String>, target: &str) -> bool {
    let directory = target.strip_suffix(".rs").unwrap_or(target);
    let below = format!("{directory}/");
    let tests = |path: &str| {
        sources
            .get(path)
            .is_some_and(|text| attrs::test_count(&Source::rust(text)) > 0)
    };
    is_test_file(target)
        || files
            .iter()
            .any(|file| file.starts_with(&below) && is_test_file(file))
        || tests(target)
        || tests(&format!("{directory}/mod.rs"))
}

fn remedy(kind: &str) -> &'static str {
    match kind {
        "ignore" => "`#[ignore = \"..\"]`",
        "should_panic" => "`expected = \"..\"`",
        "cfg-off" => "a cfg that is always false switches the code off; delete the code instead",
        "cfg" => "make the cfg true in a test build (`cfg(test)`), or say why",
        "path" => "keep the module's own file",
        "shadow" | "exit" | "return" => {
            "an `Asserts-changed:` trailer on the commit; this code keeps the checks after it \
             from running"
        }
        _ => "`reason = \"..\"`",
    }
}

/// A suppression is justified by its own reason, a `//` comment on its
/// first or last line, or a comment line directly above the stack of
/// attributes it is in.
pub fn justified(source: &Source<'_>, found: &attrs::Found) -> bool {
    if found.reasoned || source.has_comment(found.first_line) || source.has_comment(found.last_line)
    {
        return true;
    }
    let mut line = found.first_line.saturating_sub(1);
    while line > 0 && source.is_attribute_line(line) {
        line -= 1;
    }
    let comment_line = source
        .line(line)
        .is_some_and(|(code, _)| code.trim_start().starts_with("//"));
    comment_line && source.has_comment(line)
}

/// Test functions per crate, counted in the changed files alone (the
/// unchanged ones count the same at both ends).
fn test_counts(inputs: &Inputs) -> Vec<String> {
    let mut base: BTreeMap<String, usize> = BTreeMap::new();
    let mut head: BTreeMap<String, usize> = BTreeMap::new();
    // The tests that files named by a `Retires:` trailer lost.
    let mut retired: BTreeMap<String, usize> = BTreeMap::new();
    let count = |sources: &BTreeMap<String, String>, path: &Option<String>| {
        let path = path.as_deref().filter(|path| path.ends_with(".rs"))?;
        Some(attrs::test_count(&Source::rust(sources.get(path)?)))
    };
    for change in &inputs.changes {
        // A file that survives is counted in its crate at head on both
        // sides, so moving a crate or making a directory one moves no test.
        let owner = match (&change.new, &change.old) {
            (Some(new), _) => crate_of(&inputs.head_crates, new),
            (None, Some(old)) => crate_of(&inputs.base_crates, old),
            (None, None) => continue,
        };
        let before = count(&inputs.base_sources, &change.old);
        let after = count(&inputs.head_sources, &change.new);
        let approved = change
            .old
            .as_deref()
            .is_some_and(|old| trailer(&inputs.commits, "Retires", old));
        if approved {
            let lost = before.unwrap_or(0).saturating_sub(after.unwrap_or(0));
            *retired.entry(owner.clone()).or_default() += lost;
        }
        *base.entry(owner.clone()).or_default() += before.unwrap_or(0);
        *head.entry(owner).or_default() += after.unwrap_or(0);
    }
    base.iter()
        .filter_map(|(name, &before)| {
            let after = head.get(name).copied().unwrap_or(0);
            let allowed = retired.get(name).copied().unwrap_or(0);
            (after + allowed < before).then(|| {
                format!(
                    "guard: test-count: {name}: test functions in the changed files fell from \
                     {before} to {after}; restore the tests (a test in a comment or a string is \
                     not counted), replace what a retired test covered so the count holds, or \
                     name the retired files in a `Retires:` trailer of the commit that drops them \
                     ({allowed} test(s) are covered by one)"
                )
            })
        })
        .collect()
}

/// The module a Rust file belongs to: the path without `.rs` and without a
/// trailing `/mod`, so `x.rs` and `x/mod.rs` name the same module.
fn module_root(path: &str) -> Option<&str> {
    let stem = path.strip_suffix(".rs")?;
    let root = stem.strip_suffix("/mod").unwrap_or(stem);
    (!root.is_empty() && root != "mod").then_some(root)
}

/// Whether a `Retires:` value names `path`: exactly, or, for a Rust module
/// file, the module's other file form and everything below its directory
/// (`x.rs`, `x/mod.rs` and `x/**` are one module).
fn names(value: &str, path: &str) -> bool {
    if value == path {
        return true;
    }
    let (Some(root), Some(inner)) = (module_root(value), module_root(path)) else {
        return false;
    };
    inner == root
        || inner
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether a commit of the range that touches `path` has a trailer `key`
/// naming it; a `Retires:` value names a module's whole family of files.
fn trailer(commits: &[Commit], key: &str, path: &str) -> bool {
    let retires = key == "Retires";
    commits.iter().any(|commit| {
        let values = if retires {
            &commit.retires
        } else {
            &commit.asserts_changed
        };
        values.iter().any(|value| {
            if retires {
                names(value, path) && commit.paths.iter().any(|touched| names(value, touched))
            } else {
                value == path && commit.paths.iter().any(|touched| touched == path)
            }
        })
    })
}

/// Whether a commit that touches `path` names it in its message, as a
/// whole path.
fn mentioned(commits: &[Commit], path: &str) -> bool {
    let part =
        |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'));
    commits
        .iter()
        .filter(|commit| commit.paths.iter().any(|touched| touched == path))
        .any(|commit| {
            commit.message.match_indices(path).any(|(at, _)| {
                !part(commit.message[..at].chars().next_back())
                    && !part(commit.message[at + path.len()..].chars().next())
            })
        })
}

fn deleted_tests(inputs: &Inputs) -> Vec<String> {
    let mut violations = Vec::new();
    for change in &inputs.changes {
        let Some(old) = change.old.as_deref().filter(|old| is_test_file(old)) else {
            continue;
        };
        let how = match (change.status, change.new.as_deref()) {
            ('D', _) => "deleted",
            ('R', Some(new)) if !is_test_file(new) => "renamed out of the test paths",
            _ => continue,
        };
        if !trailer(&inputs.commits, "Retires", old) && !mentioned(&inputs.commits, old) {
            violations.push(format!(
                "guard: deleted-test: {old}: a test file was {how}; restore it, or name it in \
                 the message of the commit that removes it, or in its `Retires: {old}` trailer"
            ));
        }
    }
    violations
}

/// The file a `mod name;` in `parent` loads.
fn child_path(parent: &str, name: &str) -> String {
    let (directory, file) = parent.rsplit_once('/').unwrap_or(("", parent));
    let stem = file.strip_suffix(".rs").unwrap_or(file);
    let base = match (directory, stem) {
        (_, "lib" | "main" | "mod") => directory.to_owned(),
        ("", _) => stem.to_owned(),
        _ => format!("{directory}/{stem}"),
    };
    if base.is_empty() {
        format!("{name}.rs")
    } else {
        format!("{base}/{name}.rs")
    }
}

fn module_removals(inputs: &Inputs) -> Vec<String> {
    let mut violations = Vec::new();
    for file in &inputs.lines {
        let Some(old) = file.old.as_deref().filter(|old| old.ends_with(".rs")) else {
            continue;
        };
        let Some(text) = inputs.base_sources.get(old) else {
            continue;
        };
        for module in gates::modules(&Source::rust(text)) {
            let Some((line, raw)) = file.removed.iter().find(|(line, _)| *line == module.line)
            else {
                continue;
            };
            let kept = file
                .added
                .iter()
                .any(|(_, added)| added.trim() == raw.trim());
            let target = module_target(old, &module.name, module.path.as_deref());
            let retired = [old, target.as_str()]
                .iter()
                .any(|path| trailer(&inputs.commits, "Retires", path));
            let tests = holds_tests(&inputs.base_files, &inputs.base_sources, &target);
            if tests && !kept && !retired {
                violations.push(format!(
                    "guard: test-module: {old}:{line}: `{}` was removed, which stops the tests \
                     in {target} compiling; restore it, or add a `Retires: {target}` trailer \
                     to the commit",
                    raw.trim()
                ));
            }
        }
    }
    violations
}

/// The module files the `mod name;` declarations of the changed Rust
/// files load, with the `mod.rs` form, for [`holds_tests`] to read.
pub fn module_files(sources: &BTreeMap<String, String>) -> Vec<String> {
    let mut files = Vec::new();
    for (path, text) in sources.iter().filter(|(path, _)| path.ends_with(".rs")) {
        for module in gates::modules(&Source::rust(text)) {
            let target = module_target(path, &module.name, module.path.as_deref());
            let directory = target.strip_suffix(".rs").unwrap_or(&target).to_owned();
            files.push(format!("{directory}/mod.rs"));
            files.push(target);
        }
    }
    files.retain(|file| !sources.contains_key(file));
    files.sort();
    files.dedup();
    files
}

fn asserts(inputs: &Inputs) -> Vec<String> {
    let mut violations = Vec::new();
    for file in &inputs.lines {
        let (Some(old), Some(new)) = (file.old.as_deref(), file.new.as_deref()) else {
            continue;
        };
        let exempt = [old, new].iter().any(|path| {
            trailer(&inputs.commits, "Retires", path)
                || trailer(&inputs.commits, "Asserts-changed", path)
        });
        if !is_test_file(old) || exempt {
            continue;
        }
        let (Some(base), Some(head)) = (inputs.base_sources.get(old), inputs.head_sources.get(new))
        else {
            continue;
        };
        let changed = changed_asserts(&source(old, base), &source(new, head), &file.removed);
        if !changed.is_empty() {
            violations.push(format!(
                "guard: assert: {old}: {} removed or changed assert statement(s) (base line {}); \
                 restore them, or say why in an `Asserts-changed: {old}` trailer on the commit",
                changed.len(),
                changed.join(", ")
            ));
        }
    }
    violations
}

/// The first lines of the base's assert statements that a removed line
/// falls in and whose text (whitespace collapsed) is not among the head's
/// statements left once the untouched base statements are matched off.
pub fn changed_asserts(
    base: &Source<'_>,
    head: &Source<'_>,
    removed: &[(usize, String)],
) -> Vec<String> {
    let mut remaining: Vec<String> = attrs::statements(head)
        .into_iter()
        .map(|statement| statement.text)
        .collect();
    let (touched, untouched): (Vec<_>, Vec<_>) =
        attrs::statements(base).into_iter().partition(|statement| {
            removed
                .iter()
                .any(|(line, _)| (statement.first_line..=statement.last_line).contains(line))
        });
    let mut take = |text: &str| {
        let position = remaining.iter().position(|kept| kept == text);
        position
            .map(|position| remaining.swap_remove(position))
            .is_some()
    };
    for statement in &untouched {
        take(&statement.text);
    }
    touched
        .iter()
        .filter(|statement| !take(&statement.text))
        .map(|statement| statement.first_line.to_string())
        .collect()
}

/// The lines of a file diff whose paths are known; a block with lines and
/// no path means git's output was not understood.
pub fn unparsed(lines: &[FileLines]) -> Option<&FileLines> {
    lines.iter().find(|file| {
        file.old.is_none()
            && file.new.is_none()
            && !(file.added.is_empty() && file.removed.is_empty())
    })
}
