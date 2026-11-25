//! Fingerprints: what differs between two runs of a row, each named by its
//! stage and first differing field, so one fingerprint names one cause
//! however many rows show it.
//!
//! - `timeout:a|b|both`, and `exit:<a>-><b>`.
//! - `stdout:` and `stderr:`: when both sides are JSON, each differing path,
//!   array indices as `[]` (`stdout:$.Items[].When`), with `:missing` when B
//!   lacks a member A has and `:extra` for the reverse, `:length` for arrays,
//!   or `json-format` when only the text differs; otherwise `text:` and the
//!   first differing line, its digits masked, cut at 48 characters.
//! - `requests:count`, then per request (its index collapsed to `[*]`):
//!   `request[*].line:method|version|host|path`, `request[*].line:query:<name>`
//!   or `query-order`; `request[*].header:<name>:missing|extra|value|case`
//!   (lowercase name), or `request[*].header-order`; the User-Agent token by
//!   token, `request[*].header:user-agent:<token key>` (`lang`, `md/fallback`,
//!   ...) or `:order`; `request[*].body:` and the body's JSON path, form
//!   field, XML element path (`/Root/Child`), or `bytes`, and `:complete`.
//!
//! A row yields each fingerprint once, with its first instance.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use super::normalize::Normal;

const REQUEST: &str = "request[*]";
/// The longest excerpt of either side a difference keeps.
const EXCERPT_CHARS: usize = 120;
/// Bytes shown on each side of a body's first difference.
const CONTEXT_BYTES: usize = 40;
/// The characters of a line a text fingerprint keeps.
const LINE_CHARS: usize = 48;

/// One difference: its fingerprint, the request it is in, and excerpts of
/// both sides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diff {
    pub fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<usize>,
    pub a: String,
    pub b: String,
}

/// What a run did, as compared.
#[derive(Debug)]
pub struct Outcome<'a> {
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub stdout: &'a [u8],
    pub stderr: &'a [u8],
    pub requests: Vec<Normal>,
}

#[derive(Debug, Default)]
struct Found {
    diffs: Vec<Diff>,
    request: Option<usize>,
}

impl Found {
    fn add(&mut self, fingerprint: String, a: &str, b: &str) {
        if self
            .diffs
            .iter()
            .any(|diff| diff.fingerprint == fingerprint)
        {
            return;
        }
        self.diffs.push(Diff {
            fingerprint,
            request: self.request,
            a: excerpt(a),
            b: excerpt(b),
        });
    }
}

fn excerpt(text: &str) -> String {
    if text.chars().count() <= EXCERPT_CHARS {
        return text.to_owned();
    }
    let kept: String = text.chars().take(EXCERPT_CHARS).collect();
    format!("{kept}...")
}

/// The differences between run `a` and run `b` of one row.
pub fn diffs(a: &Outcome<'_>, b: &Outcome<'_>) -> Vec<Diff> {
    let mut found = Found::default();
    status(a, b, &mut found);
    stream("stdout", a.stdout, b.stdout, &mut found);
    stream("stderr", a.stderr, b.stderr, &mut found);
    requests(&a.requests, &b.requests, &mut found);
    found.diffs
}

fn status(a: &Outcome<'_>, b: &Outcome<'_>, found: &mut Found) {
    let timeout = match (a.timed_out, b.timed_out) {
        (true, true) => Some("both"),
        (true, false) => Some("a"),
        (false, true) => Some("b"),
        (false, false) => None,
    };
    if let Some(side) = timeout {
        found.add(format!("timeout:{side}"), "", "");
    }
    let exit = |outcome: &Outcome<'_>| {
        outcome
            .exit
            .map_or("none".to_owned(), |code| code.to_string())
    };
    if a.exit != b.exit {
        let (x, y) = (exit(a), exit(b));
        found.add(format!("exit:{x}->{y}"), &x, &y);
    }
}

fn stream(stage: &str, a: &[u8], b: &[u8], found: &mut Found) {
    if a == b {
        return;
    }
    if let (Some(x), Some(y)) = (json(a), json(b)) {
        return structured(stage, (&x, &y), (a, b), found);
    }
    let (x, y) = first_line_difference(a, b);
    let shown = if x.is_empty() { &y } else { &x };
    found.add(format!("{stage}:text:{}", mask(shown, LINE_CHARS)), &x, &y);
}

fn json(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice(bytes).ok()
}

/// Each differing JSON path of `values`, or `json-format` when only the
/// texts `raw` differ.
fn structured(stage: &str, values: (&Value, &Value), raw: (&[u8], &[u8]), found: &mut Found) {
    let mut paths = Vec::new();
    json_paths("$", values.0, values.1, &mut paths);
    if paths.is_empty() {
        let (x, y) = first_line_difference(raw.0, raw.1);
        found.add(format!("{stage}:json-format"), &x, &y);
    }
    for (path, x, y) in paths {
        found.add(format!("{stage}:{path}"), &x, &y);
    }
}

type Paths = Vec<(String, String, String)>;

fn json_paths(path: &str, a: &Value, b: &Value, out: &mut Paths) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => objects(path, x, y, out),
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push((
                    format!("{path}:length"),
                    x.len().to_string(),
                    y.len().to_string(),
                ));
            }
            let child = format!("{path}[]");
            for (l, r) in x.iter().zip(y) {
                json_paths(&child, l, r, out);
            }
        }
        _ if a != b => out.push((path.to_owned(), a.to_string(), b.to_string())),
        _ => {}
    }
}

type Object = serde_json::Map<String, Value>;

fn objects(path: &str, x: &Object, y: &Object, out: &mut Paths) {
    let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
    for key in keys {
        let child = format!("{path}.{key}");
        match (x.get(key), y.get(key)) {
            (Some(l), Some(r)) => json_paths(&child, l, r, out),
            (Some(l), None) => out.push((format!("{child}:missing"), l.to_string(), String::new())),
            (None, Some(r)) => out.push((format!("{child}:extra"), String::new(), r.to_string())),
            (None, None) => {}
        }
    }
}

/// The first line on which `a` and `b` differ, from each.
fn first_line_difference(a: &[u8], b: &[u8]) -> (String, String) {
    let lines = |text: &[u8]| {
        text.split(|byte| *byte == b'\n')
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>()
    };
    let (x, y) = (lines(a), lines(b));
    let index = (0..x.len().max(y.len()))
        .find(|index| x.get(*index) != y.get(*index))
        .unwrap_or(0);
    let text = |line: Option<&Vec<u8>>| line.map_or_else(String::new, |line| lossy(line));
    (text(x.get(index)), text(y.get(index)))
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `text` with each digit as `#`, cut at `chars` characters.
fn mask(text: &str, chars: usize) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_digit() {
                '#'
            } else {
                character
            }
        })
        .take(chars)
        .collect()
}

fn requests(a: &[Normal], b: &[Normal], found: &mut Found) {
    if a.len() != b.len() {
        found.add(
            "requests:count".to_owned(),
            &a.len().to_string(),
            &b.len().to_string(),
        );
    }
    for (index, (x, y)) in a.iter().zip(b).enumerate() {
        found.request = Some(index);
        line(x, y, found);
        headers(&x.headers, &y.headers, found);
        body(x, y, found);
    }
    found.request = None;
}

fn line(x: &Normal, y: &Normal, found: &mut Found) {
    let fields = [
        ("method", &x.method, &y.method),
        ("version", &x.version, &y.version),
        ("host", &x.host, &y.host),
        ("path", &x.path, &y.path),
    ];
    for (name, l, r) in fields {
        if l != r {
            found.add(format!("{REQUEST}.line:{name}"), l, r);
        }
    }
    pairs(&format!("{REQUEST}.line:query"), &x.query, &y.query, found);
}

/// Name and value pairs, a query's or a form's: each name whose values
/// differ, digits masked, or `-order` when only the order does.
fn pairs(stage: &str, x: &[(String, String)], y: &[(String, String)], found: &mut Found) {
    if x == y {
        return;
    }
    let values = |pairs: &[(String, String)], name: &str| {
        let matching = pairs.iter().filter(|(key, _)| key == name);
        matching.map(|(_, value)| value.clone()).collect::<Vec<_>>()
    };
    let mut differing = false;
    for name in union(
        x.iter().map(|(key, _)| key.clone()),
        y.iter().map(|(key, _)| key.clone()),
    ) {
        let (l, r) = (values(x, &name), values(y, &name));
        if l != r {
            let stage = format!("{stage}:{}", mask(&name, usize::MAX));
            found.add(stage, &l.join(", "), &r.join(", "));
            differing = true;
        }
    }
    if !differing {
        let names = |pairs: &[(String, String)]| {
            pairs
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>()
                .join("&")
        };
        found.add(format!("{stage}-order"), &names(x), &names(y));
    }
}

/// The distinct items of `x` then of `y`, each where it first appears.
fn union(x: impl Iterator<Item = String>, y: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = Vec::new();
    for item in x.chain(y) {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

type Headers = [(String, String)];

fn headers(x: &Headers, y: &Headers, found: &mut Found) {
    let lower = |headers: &Headers| {
        headers
            .iter()
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect::<Vec<_>>()
    };
    let (x_names, y_names) = (lower(x), lower(y));
    for name in union(x_names.iter().cloned(), y_names.iter().cloned()) {
        header(&name, &entries(x, &name), &entries(y, &name), found);
    }
    let sorted = |mut names: Vec<String>| {
        names.sort();
        names
    };
    if x_names != y_names && sorted(x_names.clone()) == sorted(y_names.clone()) {
        found.add(
            format!("{REQUEST}.header-order"),
            &x_names.join(" "),
            &y_names.join(" "),
        );
    }
}

fn entries<'a>(headers: &'a Headers, name: &str) -> Vec<&'a (String, String)> {
    headers
        .iter()
        .filter(|(header, _)| header.eq_ignore_ascii_case(name))
        .collect()
}

type Entries<'a> = [&'a (String, String)];

/// One header name's entries on each side: absent on one, or its values,
/// or the case of its name.
fn header(name: &str, x: &Entries<'_>, y: &Entries<'_>, found: &mut Found) {
    let values = |entries: &Entries<'_>| {
        entries
            .iter()
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let names = |entries: &Entries<'_>| {
        entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let stage = format!("{REQUEST}.header:{name}");
    let (l, r) = (values(x), values(y));
    match (x.is_empty(), y.is_empty()) {
        (false, true) => return found.add(format!("{stage}:missing"), &l, ""),
        (true, false) => return found.add(format!("{stage}:extra"), "", &r),
        _ => {}
    }
    if name == "user-agent" {
        user_agent(&l, &r, found);
    } else if l != r {
        found.add(format!("{stage}:value"), &l, &r);
    }
    let (l, r) = (names(x), names(y));
    if l != r {
        found.add(format!("{stage}:case"), &l, &r);
    }
}

fn user_agent(a: &str, b: &str, found: &mut Found) {
    if a == b {
        return;
    }
    let tokens = |value: &str| value.split(' ').map(str::to_owned).collect::<Vec<_>>();
    let (x, y) = (tokens(a), tokens(b));
    let keyed = |tokens: &[String], key: &str| {
        let matching = tokens.iter().filter(|token| token_key(token) == key);
        matching.cloned().collect::<Vec<_>>().join(" ")
    };
    let keys = union(
        x.iter().map(|token| token_key(token).to_owned()),
        y.iter().map(|token| token_key(token).to_owned()),
    );
    let mut differing = false;
    for key in keys {
        let (l, r) = (keyed(&x, &key), keyed(&y, &key));
        if l != r {
            found.add(format!("{REQUEST}.header:user-agent:{key}"), &l, &r);
            differing = true;
        }
    }
    if !differing {
        found.add(format!("{REQUEST}.header:user-agent:order"), a, b);
    }
}

/// A User-Agent token's key: the prefix, and for `md/` and `cfg/` the name
/// too (`md/arch`), without the value after `#`.
fn token_key(token: &str) -> &str {
    match token.split_once('/') {
        Some((prefix @ ("md" | "cfg"), rest)) => {
            &token[..prefix.len() + 1 + rest.find('#').unwrap_or(rest.len())]
        }
        Some((prefix, _)) => prefix,
        None => token,
    }
}

fn body(x: &Normal, y: &Normal, found: &mut Found) {
    let stage = format!("{REQUEST}.body");
    if x.complete != y.complete {
        found.add(
            format!("{stage}:complete"),
            &x.complete.to_string(),
            &y.complete.to_string(),
        );
    }
    if x.body == y.body {
        return;
    }
    if let (Some(l), Some(r)) = (json(&x.body), json(&y.body)) {
        return structured(&stage, (&l, &r), (&x.body, &y.body), found);
    }
    if is_form(x) && is_form(y) {
        return pairs(&stage, &form(&x.body), &form(&y.body), found);
    }
    let at = x
        .body
        .iter()
        .zip(&y.body)
        .take_while(|(l, r)| l == r)
        .count();
    let xml = |body: &[u8]| body.trim_ascii_start().starts_with(b"<");
    let field = if xml(&x.body) && xml(&y.body) {
        xml_path(&x.body, at)
    } else {
        "bytes".to_owned()
    };
    found.add(
        format!("{stage}:{field}"),
        &around(&x.body, at),
        &around(&y.body, at),
    );
}

fn is_form(request: &Normal) -> bool {
    entries(&request.headers, "content-type")
        .iter()
        .any(|(_, value)| {
            value
                .to_ascii_lowercase()
                .contains("application/x-www-form-urlencoded")
        })
}

fn form(body: &[u8]) -> Vec<(String, String)> {
    let text = lossy(body);
    let pairs = text.split('&').filter(|pair| !pair.is_empty());
    pairs
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (name.to_owned(), value.to_owned())
        })
        .collect()
}

/// The elements open at byte `offset` of an XML document, outermost first.
fn xml_path(body: &[u8], offset: usize) -> String {
    let mut open: Vec<String> = Vec::new();
    let mut at = 0;
    while let Some(start) = body[at..offset]
        .iter()
        .position(|byte| *byte == b'<')
        .map(|i| at + i)
    {
        let end = body[start..]
            .iter()
            .position(|byte| *byte == b'>')
            .map_or(body.len(), |i| start + i);
        let tag = &body[start + 1..end];
        if tag.starts_with(b"/") {
            open.pop();
        } else if !(tag.starts_with(b"?") || tag.starts_with(b"!") || tag.ends_with(b"/")) {
            let name = tag
                .split(u8::is_ascii_whitespace)
                .next()
                .unwrap_or_default();
            open.push(lossy(name));
        }
        at = (end + 1).min(offset);
    }
    format!("/{}", open.join("/"))
}

/// The bytes around `at`, as text.
fn around(body: &[u8], at: usize) -> String {
    let start = at.saturating_sub(CONTEXT_BYTES);
    let end = (at + CONTEXT_BYTES).min(body.len());
    lossy(body.get(start..end).unwrap_or_default())
}

#[cfg(test)]
mod tests;
