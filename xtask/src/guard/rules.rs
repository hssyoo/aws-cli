//! The guard's rules over [`Inputs`]; see the module documentation of
//! `guard` for what each checks.

use std::collections::BTreeMap;

use super::attrs::{self, Source};
use super::diff::{Commit, FileLines, crate_of, is_test_file, matches};
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
        for found in attrs::suppressions(&source) {
            let added = file
                .added
                .iter()
                .any(|(line, _)| (found.first_line..=found.last_line).contains(line));
            if added && !justified(&source, &found) {
                violations.push(format!(
                    "guard: suppression: {path}:{}: `{}` ({}) has no justification; say why \
                     in a `//` comment on its line or above its attributes, or in the \
                     attribute itself ({})",
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

fn remedy(kind: &str) -> &'static str {
    match kind {
        "ignore" => "`#[ignore = \"..\"]`",
        "should_panic" => "`expected = \"..\"`",
        "cfg-off" => "a cfg that is always false switches the code off; delete the code instead",
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
        if let Some(count) = count(&inputs.base_sources, &change.old) {
            *base.entry(owner.clone()).or_default() += count;
        }
        if let Some(count) = count(&inputs.head_sources, &change.new) {
            *head.entry(owner).or_default() += count;
        }
    }
    base.iter()
        .filter_map(|(name, &before)| {
            let after = head.get(name).copied().unwrap_or(0);
            (after < before).then(|| {
                format!(
                    "guard: test-count: {name}: test functions in the changed files fell from \
                     {before} to {after}; restore the tests (a test in a comment or a string is \
                     not counted), or replace what a retired test covered so the count holds"
                )
            })
        })
        .collect()
}

/// Whether a commit of the range that touches `path` has a trailer `key`
/// naming exactly `path`.
fn trailer(commits: &[Commit], key: &str, path: &str) -> bool {
    commits.iter().any(|commit| {
        let values = if key == "Retires" {
            &commit.retires
        } else {
            &commit.asserts_changed
        };
        commit.paths.iter().any(|touched| touched == path)
            && values.iter().any(|value| value == path)
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

/// The test module a line declares (`mod tests;`, `pub(crate) mod testing {`).
fn test_module(code: &str) -> Option<&'static str> {
    let mut words = code
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty());
    words.position(|word| word == "mod")?;
    match words.next()? {
        "tests" => Some("tests"),
        "testing" => Some("testing"),
        _ => None,
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
        let source = Source::rust(text);
        for (line, raw) in &file.removed {
            let Some(name) = source.line(*line).and_then(|(code, _)| test_module(code)) else {
                continue;
            };
            let kept = file
                .added
                .iter()
                .any(|(_, added)| added.trim() == raw.trim());
            let child = child_path(old, name);
            let retired = [old, child.as_str()]
                .iter()
                .any(|path| trailer(&inputs.commits, "Retires", path));
            if !kept && !retired {
                violations.push(format!(
                    "guard: test-module: {old}:{line}: `{}` was removed, which stops its tests \
                     compiling; restore it, or add a `Retires: {child}` trailer to the commit",
                    raw.trim()
                ));
            }
        }
    }
    violations
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
