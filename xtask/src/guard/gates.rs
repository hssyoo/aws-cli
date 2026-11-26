//! What decides whether checks run: the item an attribute stack is on, the
//! modules a file declares, and the code that skips the checks after it (a
//! shadowed assert macro, an exit, an early return from a test).

use super::attrs::{self, Found, Source, closing, skip_space};

/// The std macros a test's checks go through; a local `macro_rules!` or a
/// `use` of one of these names replaces it for the code below.
const CHECK_MACROS: [&str; 12] = [
    "assert",
    "assert_eq",
    "assert_ne",
    "assert_matches",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "debug_assert_matches",
    "panic",
    "unreachable",
    "unimplemented",
    "todo",
];

/// A stack of attributes with only whitespace between them, and the item
/// after it.
#[derive(Debug)]
pub struct Group {
    pub attributes: Vec<(usize, usize)>,
    /// The offset the item after the stack starts at.
    pub item: usize,
    /// One of the attributes marks a test function.
    pub test: bool,
    /// The item is `mod <name>`, with its `#[path = ".."]` value, if any.
    pub module: Option<(String, Option<String>)>,
}

pub fn groups(source: &Source<'_>) -> Vec<Group> {
    let code = &source.code;
    let mut groups: Vec<Group> = Vec::new();
    for (hash, close) in attrs::attributes(source) {
        match groups.last_mut() {
            Some(group) if group.item == hash => group.attributes.push((hash, close)),
            _ => groups.push(Group {
                attributes: vec![(hash, close)],
                item: 0,
                test: false,
                module: None,
            }),
        }
        if let Some(group) = groups.last_mut() {
            group.item = skip_space(code.as_bytes(), close + 1);
        }
    }
    for group in &mut groups {
        group.test = group
            .attributes
            .iter()
            .any(|&(hash, close)| attrs::is_test_attribute(source, hash, close));
        let path = group
            .attributes
            .iter()
            .find_map(|&(hash, close)| path_value(source, hash, close));
        group.module = module_name(code, group.item).map(|name| (name, path));
    }
    groups
}

/// The value of a `#[path = ".."]` attribute.
fn path_value(source: &Source<'_>, hash: usize, close: usize) -> Option<String> {
    let code = &source.code[hash..close];
    let open = code.find('[')? + 1;
    let rest = code[open..].trim_start().strip_prefix("path")?;
    let equals = rest.trim_start().strip_prefix('=')?;
    let start = close - equals.len();
    let value = source.raw[start..close].trim();
    attrs::non_empty_string(value).then(|| value.trim_matches(['"', '#', 'r']).to_owned())
}

/// The module an item starting at `at` declares (`pub(crate) mod name`).
fn module_name(code: &str, at: usize) -> Option<String> {
    let end = code[at..]
        .find([';', '{', '='])
        .map_or(code.len(), |offset| at + offset);
    let mut words = code[at..end]
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty());
    words.position(|word| word == "mod")?;
    words.next().map(str::to_owned)
}

/// A `mod name;` declaration (a module in its own file).
#[derive(Debug, PartialEq, Eq)]
pub struct Module {
    pub line: usize,
    pub name: String,
    pub path: Option<String>,
}

pub fn modules(source: &Source<'_>) -> Vec<Module> {
    let groups = groups(source);
    let code = source.code.as_bytes();
    words(&source.code, "mod")
        .filter_map(|at| {
            let name_start = skip_space(code, at + 3);
            let name_end = ident_end(code, name_start);
            let after = skip_space(code, name_end);
            (name_end > name_start && code.get(after) == Some(&b';')).then_some(())?;
            let item = item_start(code, at);
            let path = groups
                .iter()
                .find(|group| group.item == item)
                .and_then(|group| group.module.clone())
                .and_then(|(_, path)| path);
            Some(Module {
                line: source.line_of(at),
                name: source.code[name_start..name_end].to_owned(),
                path,
            })
        })
        .collect()
}

/// The start of the item a `mod` keyword at `at` belongs to: before a
/// `pub` or `pub(..)` that precedes it.
fn item_start(code: &[u8], at: usize) -> usize {
    let text = String::from_utf8_lossy(&code[..at]);
    let trimmed = text.trim_end();
    let trimmed = match trimmed.strip_suffix(')') {
        Some(inner) => inner
            .rfind('(')
            .map_or(trimmed, |open| inner[..open].trim_end()),
        None => trimmed,
    };
    trimmed
        .strip_suffix("pub")
        .map_or(at, |before| before.len())
}

fn ident_end(code: &[u8], mut at: usize) -> usize {
    while code
        .get(at)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        at += 1;
    }
    at
}

/// The offsets of `word` as a whole word in masked code.
fn words<'a>(code: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
    let bytes = code.as_bytes();
    let part =
        |at: Option<&u8>| at.is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
    code.match_indices(word)
        .map(|(at, _)| at)
        .filter(move |&at| {
            !part(at.checked_sub(1).and_then(|before| bytes.get(before)))
                && !part(bytes.get(at + word.len()))
        })
}

/// The byte spans of test function bodies.
fn test_bodies(source: &Source<'_>) -> Vec<(usize, usize)> {
    let code = source.code.as_bytes();
    groups(source)
        .into_iter()
        .filter(|group| group.test)
        .filter_map(|group| {
            let open = group.item + source.code[group.item..].find('{')?;
            Some((open, closing(code, open)?))
        })
        .collect()
}

/// The code that keeps checks from running: a `macro_rules!` or `use` of
/// a check macro's name (or one of `names`, the macros the file's asserts
/// used at base), an `exit`/`abort` call, and a `return` in a test body
/// with an assert statement after it. `test_item` marks one in a test body.
pub fn escapes(source: &Source<'_>, names: &[String]) -> Vec<Found> {
    let bodies = test_bodies(source);
    let in_body = |at: usize| {
        bodies
            .iter()
            .find(|&&(open, close)| open < at && at < close)
            .copied()
    };
    let mut spans = shadows(&source.code, names);
    for (at, end) in exits(&source.code) {
        spans.push(("exit", at, end, in_body(at).is_some()));
    }
    let statements = attrs::statements(source);
    for at in words(&source.code, "return") {
        let Some((_, close)) = in_body(at) else {
            continue;
        };
        // `return Err(..)` fails the test; a bare or `Ok` return passes it.
        let value = source.code[at + 6..].trim_start();
        let passes =
            value.starts_with([';', '}']) || value.starts_with("Ok") || value.starts_with("()");
        let line = source.line_of(at);
        let checks_after = statements.iter().any(|statement| {
            statement.first_line > line && source.line_of(close) >= statement.first_line
        });
        if passes && checks_after {
            spans.push(("return", at, at + 6, true));
        }
    }
    spans
        .into_iter()
        .map(|(kind, start, end, test_item)| Found {
            kind,
            first_line: source.line_of(start),
            last_line: source.line_of(end),
            text: source.raw[start..end]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            test_item,
            ..Found::default()
        })
        .collect()
}

/// A `macro_rules!` or `use` of a check macro's name or one of `names`.
fn shadows(code: &str, names: &[String]) -> Vec<(&'static str, usize, usize, bool)> {
    let shadowed =
        |name: &str| CHECK_MACROS.contains(&name) || names.iter().any(|known| known == name);
    let bytes = code.as_bytes();
    let mut spans = Vec::new();
    for at in words(code, "macro_rules") {
        let name_start = skip_space(bytes, skip_space(bytes, at + 11) + 1);
        let name_end = ident_end(bytes, name_start);
        if shadowed(&code[name_start..name_end]) {
            spans.push(("shadow", at, name_end, false));
        }
    }
    for at in words(code, "use") {
        let end = code[at..]
            .find(';')
            .map_or(code.len(), |offset| at + offset);
        let mut idents = code[at..end].split(|c: char| !(c.is_alphanumeric() || c == '_'));
        if idents.any(shadowed) {
            spans.push(("shadow", at, end, false));
        }
    }
    spans
}

/// Calls of `exit`, `abort` or `_exit` (not methods or definitions).
fn exits(code: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for word in ["exit", "abort", "_exit"] {
        for at in words(code, word) {
            let after = skip_space(code.as_bytes(), at + word.len());
            let before = code[..at].trim_end();
            let call = code.as_bytes().get(after) == Some(&b'(')
                && !before.ends_with('.')
                && !before.ends_with("fn");
            if call {
                spans.push((at, after));
            }
        }
    }
    spans
}

/// The macro names the asserts of a source use (`assert_eq` in
/// `assert_eq!(..)`).
pub fn assert_macros(source: &Source<'_>) -> Vec<String> {
    let mut names: Vec<String> = attrs::statements(source)
        .into_iter()
        .filter_map(|statement| Some(statement.text.split_once('!')?.0.to_owned()))
        .filter(|name| name.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .collect();
    names.sort();
    names.dedup();
    names
}
