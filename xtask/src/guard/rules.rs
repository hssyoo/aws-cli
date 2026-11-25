//! The guard's rules over [`Inputs`]; see the module documentation of
//! `guard` for what each checks.

use std::collections::BTreeMap;

use super::diff::{FileLines, is_test_file, matches};
use super::{Inputs, PROTECTED_LIST};

/// The lines above or below an attribute's first that are read for its
/// closing bracket.
const ATTRIBUTE_LINES_MAX: usize = 32;

pub fn check(inputs: &Inputs) -> Vec<String> {
    let trailers = Trailers::parse(&inputs.messages);
    let mut violations = protected(inputs);
    violations.extend(suppressions(inputs));
    violations.extend(test_counts(&inputs.base_tests, &inputs.head_tests));
    violations.extend(deleted_tests(inputs, &trailers));
    violations.extend(asserts(&inputs.lines, &trailers));
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

/// The `.rs` files at head whose added lines hold a suppression attribute.
pub fn suppression_files(lines: &[FileLines]) -> Vec<String> {
    lines
        .iter()
        .filter(|file| {
            file.added
                .iter()
                .any(|(_, text)| suppression(text).is_some())
        })
        .filter_map(|file| file.new.clone())
        .filter(|path| path.ends_with(".rs"))
        .collect()
}

fn suppressions(inputs: &Inputs) -> Vec<String> {
    let mut violations = Vec::new();
    for file in &inputs.lines {
        let Some(path) = &file.new else { continue };
        let Some(source) = inputs.head_sources.get(path) else {
            continue;
        };
        let source: Vec<_> = source.lines().collect();
        for (line, text) in &file.added {
            let Some(kind) = suppression(text) else {
                continue;
            };
            if !justified(&source, line.saturating_sub(1), kind) {
                violations.push(format!(
                    "guard: suppression: {path}:{line}: `{}` has no justification; say why in \
                     a `//` comment on the same or the previous line, or in the attribute's \
                     `reason = \"..\"` (`#[ignore = \"..\"]` for ignore)",
                    text.trim()
                ));
            }
        }
    }
    violations
}

/// The kind of suppression attribute a line starts, if any: `allow`,
/// `expect` or `ignore`, outer or inner.
pub fn suppression(line: &str) -> Option<&'static str> {
    let line = line.trim_start();
    let rest = line
        .strip_prefix("#![")
        .or_else(|| line.strip_prefix("#["))?;
    let rest = rest.trim_start();
    if rest.starts_with("allow(") {
        Some("allow")
    } else if rest.starts_with("expect(") {
        Some("expect")
    } else if let Some(after) = rest.strip_prefix("ignore") {
        let next = after.trim_start().chars().next();
        matches!(next, None | Some(']' | '=')).then_some("ignore")
    } else {
        None
    }
}

/// Whether the attribute starting at `source[index]` is justified: a `//`
/// comment on its line or the line before, or a non-empty `reason = ".."`
/// (`ignore = ".."` for ignore) in the attribute.
pub fn justified(source: &[&str], index: usize, kind: &str) -> bool {
    let Some(first) = source.get(index) else {
        return false;
    };
    let previous = index
        .checked_sub(1)
        .and_then(|previous| source.get(previous));
    if has_comment(first) || previous.is_some_and(|line| is_comment(line)) {
        return true;
    }
    let attribute = attribute_text(source, index);
    let key = if kind == "ignore" { "ignore" } else { "reason" };
    has_string_value(&attribute, key)
}

/// The text of the attribute starting at `source[index]`, through its
/// closing bracket.
fn attribute_text(source: &[&str], index: usize) -> String {
    let mut text = String::new();
    let mut depth = 0_i32;
    for line in source.iter().skip(index).take(ATTRIBUTE_LINES_MAX) {
        text.push_str(line);
        text.push('\n');
        depth += line.matches('[').count() as i32 - line.matches(']').count() as i32;
        if depth <= 0 {
            break;
        }
    }
    text
}

fn has_comment(line: &str) -> bool {
    line.match_indices("//").any(|(at, _)| {
        let comment = &line[at + 2..];
        !comment.starts_with(['/', '!']) && !comment.trim().is_empty()
    })
}

/// A `//` line comment with text, not a doc comment.
fn is_comment(line: &str) -> bool {
    line.trim_start()
        .strip_prefix("//")
        .is_some_and(|comment| !comment.starts_with(['/', '!']) && !comment.trim().is_empty())
}

/// Whether `key = "<something>"` appears in `text`.
fn has_string_value(text: &str, key: &str) -> bool {
    text.match_indices(key).any(|(at, _)| {
        text[at + key.len()..]
            .trim_start()
            .strip_prefix('=')
            .map(str::trim_start)
            .and_then(|value| value.strip_prefix('"'))
            .is_some_and(|value| value.chars().next().is_some_and(|first| first != '"'))
    })
}

fn test_counts(base: &BTreeMap<String, usize>, head: &BTreeMap<String, usize>) -> Vec<String> {
    base.iter()
        .filter_map(|(name, &before)| {
            let after = head.get(name).copied().unwrap_or(0);
            (after < before).then(|| {
                format!(
                    "guard: test-count: {name}: #[test] functions fell from {before} to \
                     {after}; restore the tests, or replace what a retired test covered so \
                     the count holds"
                )
            })
        })
        .collect()
}

fn deleted_tests(inputs: &Inputs, trailers: &Trailers) -> Vec<String> {
    inputs
        .changes
        .iter()
        .filter(|change| change.status == 'D')
        .filter_map(|change| change.old.as_deref())
        .filter(|path| is_test_file(path))
        .filter(|path| {
            !trailers.names(&trailers.retires, path)
                && !inputs
                    .messages
                    .iter()
                    .any(|message| message.contains(*path))
        })
        .map(|path| {
            format!(
                "guard: deleted-test: {path}: a test file was deleted; restore it, or name it \
                 in the commit message or a `Retires: {path}` trailer"
            )
        })
        .collect()
}

fn asserts(lines: &[FileLines], trailers: &Trailers) -> Vec<String> {
    let mut violations = Vec::new();
    for file in lines {
        let (Some(old), Some(new)) = (&file.old, &file.new) else {
            continue;
        };
        if !is_test_file(old)
            || trailers.names_assert_change(old)
            || trailers.names_assert_change(new)
        {
            continue;
        }
        let mut added: Vec<_> = file.added.iter().map(|(_, text)| text.trim()).collect();
        let mut changed = Vec::new();
        for (line, text) in file
            .removed
            .iter()
            .filter(|(_, text)| text.contains("assert"))
        {
            match added.iter().position(|kept| *kept == text.trim()) {
                Some(position) => {
                    added.swap_remove(position);
                }
                None => changed.push(line.to_string()),
            }
        }
        if !changed.is_empty() {
            violations.push(format!(
                "guard: assert: {old}: {} removed or changed assert line(s) (base line {}); \
                 restore them, or say why in an `Asserts-changed: {old}` trailer",
                changed.len(),
                changed.join(", ")
            ));
        }
    }
    violations
}

/// The path trailers of the range's commit messages.
#[derive(Debug, Default)]
pub struct Trailers {
    pub retires: Vec<String>,
    pub asserts_changed: Vec<String>,
}

impl Trailers {
    pub fn parse(messages: &[String]) -> Self {
        let mut trailers = Self::default();
        for line in messages.iter().flat_map(|message| message.lines()) {
            let (list, values) = if let Some(values) = line.strip_prefix("Retires:") {
                (&mut trailers.retires, values)
            } else if let Some(values) = line.strip_prefix("Asserts-changed:") {
                (&mut trailers.asserts_changed, values)
            } else {
                continue;
            };
            list.extend(
                values
                    .split([',', ' ', '\t'])
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
            );
        }
        trailers
    }

    fn names(&self, list: &[String], path: &str) -> bool {
        list.iter().any(|pattern| matches(pattern, path))
    }

    fn names_assert_change(&self, path: &str) -> bool {
        self.names(&self.retires, path) || self.names(&self.asserts_changed, path)
    }
}
