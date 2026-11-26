//! Attributes, test functions and assert statements in a source, read
//! from its [`lex::mask`] so literals and comments are never code, and from
//! the original text at the same offsets for the values of literals.

use super::{gates, lex};

/// A source and its mask, which share every byte offset.
#[derive(Debug)]
pub struct Source<'a> {
    pub raw: &'a str,
    pub code: String,
    /// The byte offset each line starts at.
    starts: Vec<usize>,
}

impl<'a> Source<'a> {
    pub fn rust(raw: &'a str) -> Self {
        Self::with_code(raw, lex::mask(raw))
    }

    /// A non-Rust source, scanned as it is.
    pub fn plain(raw: &'a str) -> Self {
        Self::with_code(raw, raw.to_owned())
    }

    fn with_code(raw: &'a str, code: String) -> Self {
        let starts = std::iter::once(0)
            .chain(raw.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self { raw, code, starts }
    }

    /// The 1-based line holding a byte offset.
    pub fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&start| start <= offset)
    }

    /// A 1-based line's code and text, without the newline.
    pub fn line(&self, line: usize) -> Option<(&str, &str)> {
        let start = *self.starts.get(line.checked_sub(1)?)?;
        let end = self
            .starts
            .get(line)
            .map_or(self.raw.len(), |next| next - 1);
        Some((self.code.get(start..end)?, self.raw.get(start..end)?))
    }

    /// Whether a line has a `//` comment with text that is not a doc
    /// comment.
    pub fn has_comment(&self, line: usize) -> bool {
        let Some((code, raw)) = self.line(line) else {
            return false;
        };
        code.match_indices("//").any(|(at, _)| {
            let text = raw.get(at + 2..).unwrap_or_default();
            !text.starts_with(['/', '!']) && !text.trim().is_empty()
        })
    }

    /// Whether a line's code is only (the start of) an attribute.
    pub fn is_attribute_line(&self, line: usize) -> bool {
        self.line(line)
            .is_some_and(|(code, _)| code.trim_start().starts_with('#'))
    }
}

/// One suppression-like attribute, or a statement that skips checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// `allow`, `expect`, `ignore`, `should_panic`, `cfg-off` (false in a
    /// test build), `cfg` (not known true), `path`, or from
    /// [`gates::escapes`](super::gates::escapes) `shadow`, `exit` or `return`.
    pub kind: &'static str,
    pub first_line: usize,
    pub last_line: usize,
    /// It carries its own non-empty `reason`, `ignore = ".."` or
    /// `expected = ".."`.
    pub reasoned: bool,
    pub text: String,
    /// The item it is on is a test function, or it lies in one.
    pub test_item: bool,
    /// It is an inner attribute, on the module of its file.
    pub inner: bool,
    /// The module the item declares, and its `#[path]`, if any.
    pub module: Option<(String, Option<String>)>,
}

/// Every attribute's `#` and closing `]` offsets.
pub(super) fn attributes(source: &Source<'_>) -> Vec<(usize, usize)> {
    let code = source.code.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = code[at..].iter().position(|&byte| byte == b'#') {
        let hash = at + offset;
        at = hash + 1;
        let mut open = skip_space(code, hash + 1);
        if code.get(open) == Some(&b'!') {
            open = skip_space(code, open + 1);
        }
        if code.get(open) != Some(&b'[') {
            continue;
        }
        if let Some(close) = closing(code, open) {
            found.push((hash, close));
            at = open + 1;
        }
    }
    found
}

pub(super) fn skip_space(code: &[u8], mut at: usize) -> usize {
    while code.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// The offset of the bracket closing the one at `open`.
pub(super) fn closing(code: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (offset, byte) in code[open..].iter().enumerate() {
        match byte {
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// The suppression-like attributes of a source, each with what its
/// attribute stack is on.
pub fn suppressions(source: &Source<'_>) -> Vec<Found> {
    let mut found = Vec::new();
    for group in gates::groups(source) {
        for &(hash, close) in &group.attributes {
            let open = source.code[hash..close]
                .find('[')
                .map_or(hash, |at| hash + at);
            let mut kinds = Vec::new();
            classify(source, open + 1, close, &mut kinds);
            for (kind, reasoned) in kinds {
                found.push(Found {
                    kind,
                    first_line: source.line_of(hash),
                    last_line: source.line_of(close),
                    reasoned,
                    text: source.raw[hash..=close]
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                    test_item: group.test,
                    inner: source.code[hash + 1..open].contains('!'),
                    module: group.module.clone(),
                });
            }
        }
    }
    found
}

/// Classifies one meta item, `path`, `path(args)` or `path = value`, in
/// `start..end`, unwrapping `cfg_attr(pred, a, b..)` into its attributes.
fn classify(source: &Source<'_>, start: usize, end: usize, kinds: &mut Vec<(&'static str, bool)>) {
    let code = &source.code[start..end];
    let name_end = code
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':' || c.is_whitespace()))
        .unwrap_or(code.len());
    let path: String = code[..name_end].split_whitespace().collect();
    let rest = start + name_end;
    let args = arguments(source, rest, end);
    let value = || {
        source.raw[rest..end]
            .trim_start()
            .strip_prefix('=')
            .unwrap_or_default()
    };
    match path.as_str() {
        "cfg_attr" => {
            for (from, to) in args.into_iter().skip(1) {
                classify(source, from, to, kinds);
            }
        }
        "allow" => kinds.push(("allow", keyed(source, &args, "reason"))),
        "expect" => kinds.push(("expect", keyed(source, &args, "reason"))),
        "ignore" => kinds.push(("ignore", non_empty_string(value()))),
        "should_panic" => kinds.push(("should_panic", keyed(source, &args, "expected"))),
        "cfg" => {
            let predicate = args.first().map(|&(from, to)| &source.raw[from..to]);
            match predicate.map(|predicate| (evaluate(predicate), evaluate_ci(predicate))) {
                Some((Some(false), _)) => kinds.push(("cfg-off", false)),
                Some((_, Some(true))) | None => {}
                Some(_) => kinds.push(("cfg", false)),
            }
        }
        "path" => kinds.push(("path", false)),
        _ => {}
    }
}

/// The top-level comma-separated argument spans of `(..)` at `start`.
fn arguments(source: &Source<'_>, start: usize, end: usize) -> Vec<(usize, usize)> {
    let code = source.code.as_bytes();
    let open = skip_space(code, start);
    if open >= end || code[open] != b'(' {
        return Vec::new();
    }
    let close = closing(code, open).unwrap_or(end).min(end);
    let mut spans = Vec::new();
    let (mut from, mut depth) = (open + 1, 0_usize);
    for (at, byte) in code.iter().enumerate().take(close).skip(open + 1) {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                spans.push((from, at));
                from = at + 1;
            }
            _ => {}
        }
    }
    if !source.code[from..close].trim().is_empty() {
        spans.push((from, close));
    }
    spans
}

/// Whether an argument `key = "<text>"` is present.
fn keyed(source: &Source<'_>, args: &[(usize, usize)], key: &str) -> bool {
    args.iter().any(|&(from, to)| {
        let code = source.code[from..to].trim_start();
        code.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
            && source.raw[from..to]
                .split_once('=')
                .is_some_and(|(_, value)| non_empty_string(value))
    })
}

/// Whether a literal value is a string with non-blank text.
pub(super) fn non_empty_string(value: &str) -> bool {
    let value = value
        .trim()
        .trim_start_matches(['r', 'b'])
        .trim_matches('#');
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .is_some_and(|text| !text.trim().is_empty())
}

/// A `cfg` predicate's value in any test build: `test`, `true` and
/// `false`, and `all`, `any` and `not` over them. `None` when it depends on
/// anything else.
pub fn evaluate(predicate: &str) -> Option<bool> {
    evaluate_with(predicate, |atom| match atom {
        "test" | "true" => Some(true),
        "false" => Some(false),
        _ => None,
    })
}

/// A `cfg` predicate's value in the Linux test build the gates run: also
/// `unix`, `debug_assertions`, `target_os = "linux"` and
/// `target_family = "unix"` true, and `windows`, `miri` and other targets
/// false. Features and the rest stay unknown.
pub fn evaluate_ci(predicate: &str) -> Option<bool> {
    evaluate_with(predicate, |atom| {
        let value = |key: &str| {
            let rest = atom.strip_prefix(key)?.trim_start().strip_prefix('=')?;
            Some(rest.trim().trim_matches('"').to_owned())
        };
        match atom {
            "test" | "true" | "unix" | "debug_assertions" => Some(true),
            "false" | "windows" | "miri" => Some(false),
            _ => value("target_os")
                .map(|os| os == "linux")
                .or_else(|| value("target_family").map(|family| family == "unix")),
        }
    })
}

fn evaluate_with(predicate: &str, atom: fn(&str) -> Option<bool>) -> Option<bool> {
    let predicate = predicate.trim();
    let call = |name: &str| {
        predicate
            .strip_prefix(name)
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('('))
            .and_then(|rest| rest.strip_suffix(')'))
    };
    if let Some(inner) = call("not") {
        return evaluate_with(inner, atom).map(|value| !value);
    }
    let (inner, all) = match (call("all"), call("any")) {
        (Some(inner), _) => (inner, true),
        (_, Some(inner)) => (inner, false),
        _ => return atom(predicate),
    };
    let source = Source::plain(inner);
    let values: Vec<_> = split_top(&source)
        .into_iter()
        .map(|part| evaluate_with(part, atom))
        .collect();
    if values.contains(&Some(!all)) {
        Some(!all)
    } else if values.iter().all(Option::is_some) {
        Some(all)
    } else {
        None
    }
}

fn split_top<'a>(source: &Source<'a>) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let (mut from, mut depth) = (0, 0_usize);
    for (at, byte) in source.raw.bytes().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(&source.raw[from..at]);
                from = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&source.raw[from..]);
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect()
}

/// The number of test functions: attributes whose path ends in `test`,
/// alone or with arguments (`#[tokio::test(..)]`), or is `test_case` or
/// `rstest`.
pub fn test_count(source: &Source<'_>) -> usize {
    attributes(source)
        .into_iter()
        .filter(|&(hash, close)| is_test_attribute(source, hash, close))
        .count()
}

pub(super) fn is_test_attribute(source: &Source<'_>, hash: usize, close: usize) -> bool {
    let code = &source.code[hash..close];
    let inner = code.split_once('[').map_or("", |(_, inner)| inner);
    let path: String = inner
        .split(['(', '=', ']'])
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let last = path.rsplit("::").next().unwrap_or_default();
    matches!(last, "test" | "test_case" | "rstest")
}

/// An assert statement: an identifier containing `assert`, then (after an
/// optional `!`) its bracketed arguments to the balanced close, or else the
/// rest of its line (a Python `assert x`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub first_line: usize,
    pub last_line: usize,
    /// The statement's text without code whitespace or trailing commas,
    /// so reflowing it changes nothing.
    pub text: String,
}

pub fn statements(source: &Source<'_>) -> Vec<Statement> {
    let code = source.code.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = source.code[at..].find("assert") {
        let hit = at + offset;
        let start = code[..hit]
            .iter()
            .rposition(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
            .map_or(0, |before| before + 1);
        let mut after = hit;
        while code
            .get(after)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            after += 1;
        }
        let mut open = skip_space(code, after);
        if code.get(open) == Some(&b'!') {
            open = skip_space(code, open + 1);
        }
        let end = match code.get(open) {
            Some(b'(' | b'[' | b'{') => closing(code, open).map_or(code.len(), |close| close + 1),
            _ => code[after..]
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(code.len(), |offset| after + offset),
        };
        found.push(Statement {
            first_line: source.line_of(start),
            last_line: source.line_of(end.saturating_sub(1).max(start)),
            text: normalized(source, start, end),
        });
        at = end.max(after);
    }
    found
}

fn normalized(source: &Source<'_>, start: usize, end: usize) -> String {
    let code = &source.code.as_bytes()[start..end];
    let raw = &source.raw.as_bytes()[start..end];
    let mut text: Vec<u8> = Vec::with_capacity(raw.len());
    let mut comma = false;
    for (&code, &byte) in code.iter().zip(raw) {
        if code.is_ascii_whitespace() {
            continue;
        }
        if comma && matches!(code, b')' | b']' | b'}') {
            text.pop();
        }
        comma = code == b',';
        text.push(byte);
    }
    String::from_utf8_lossy(&text).into_owned()
}
