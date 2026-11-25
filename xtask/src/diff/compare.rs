//! Fingerprints: what differs between two runs of a row, each named by its
//! stage and a differing field, so one fingerprint names one cause however
//! many rows show it. Every check of a stage runs every time, on what both
//! sides share, so knowing one difference never hides another; and when two
//! texts differ but no check names why, the stage's `:format` does, so a
//! differing byte always shows.
//!
//! - `harness:a|b:<kind>` (the capture could not see the run whole) and
//!   `timeout:a|b|both`; no known difference covers these. `exit:<a>-><b>`.
//! - `stdout:` and `stderr:`: when both sides are JSON, each differing path,
//!   array indices as `[]` (`stdout:$.Items[].When`), with `:missing` when B
//!   lacks a member A has and `:extra` for the reverse, `:length` for an
//!   array, `:order` for the order of an object's shared keys, and
//!   `json-format` for the whitespace around shared tokens. Scalars compare
//!   as their text, so `1.0` and `1` differ. Otherwise, each differing line
//!   (after the lines both share at either end), as `text:<A>-><B>` with
//!   digits masked.
//! - `connects:count|host`: the hosts each run asked the proxy for.
//! - `requests:count`, then per request (its index collapsed to `[*]`):
//!   - `request[*].line:method|version|host|path`, `request[*].line:query:<name>`
//!     (a field without `=` differs from one with an empty value),
//!     `line:query-order` and `line:query-format` (a `?` with no query, or
//!     empty fields);
//!   - `request[*].header:<name>:missing|extra|value|count|split|case`, where
//!     `count` is how many times the name appears and `split` the same
//!     values cut differently, and `request[*].header-order`. The User-Agent
//!     compares token by token, `request[*].header:user-agent:<token key>`
//!     (`lang`, `md/fallback`, ...) and `:order`;
//!   - `request[*].body:` the JSON forms as for stdout, a form's fields as
//!     for a query (`-order`, `-format`), each differing XML token's element
//!     path (`/Root/Child`), or `bytes`; and `:complete` and `:framing`
//!     (chunk sizes and trailers).
//!
//! A row yields each fingerprint once, with its first instance.

use serde::Serialize;

use super::json::{self, Document, Element, Kind, Member, Node};
use super::normalize::{self, Fields, Normal};

const REQUEST: &str = "request[*]";
/// The longest excerpt of either side a difference keeps.
const EXCERPT_CHARS: usize = 120;
/// Bytes shown on each side of a body's first difference.
const CONTEXT_BYTES: usize = 40;
/// The characters of a line a text fingerprint keeps of each side.
const LINE_CHARS: usize = 48;
/// The differing lines or XML tokens a stage names before `...`.
const SPAN_MAX: usize = 16;

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
    /// What the capture could not see, each `<kind>: <detail>`.
    pub harness: &'a [String],
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub stdout: &'a [u8],
    pub stderr: &'a [u8],
    pub connects: &'a [String],
    pub requests: Vec<Normal>,
}

#[derive(Debug, Default)]
struct Found {
    diffs: Vec<Diff>,
    request: Option<usize>,
    /// The differences named so far, repeats included.
    noted: usize,
}

impl Found {
    fn add(&mut self, fingerprint: String, a: &str, b: &str) {
        self.noted += 1;
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

    /// Names `<stage>:format` for texts that differ when no check since
    /// `mark` named a difference.
    fn backstop(&mut self, mark: usize, stage: &str, a: &[u8], b: &[u8]) {
        if a != b && self.noted == mark {
            let at = common_prefix(a, b);
            self.add(format!("{stage}:format"), &around(a, at), &around(b, at));
        }
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
    for (side, outcome) in [("a", a), ("b", b)] {
        for error in outcome.harness {
            let kind = error.split(':').next().unwrap_or_default();
            found.add(format!("harness:{side}:{kind}"), error, "");
        }
    }
    status(a, b, &mut found);
    stream("stdout", a.stdout, b.stdout, &mut found);
    stream("stderr", a.stderr, b.stderr, &mut found);
    connects(a.connects, b.connects, &mut found);
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
    let mark = found.noted;
    match (read_json(a), read_json(b)) {
        (Some(x), Some(y)) => documents(stage, &x, &y, found),
        _ => lines(stage, a, b, found),
    }
    found.backstop(mark, stage, a, b);
}

fn read_json(bytes: &[u8]) -> Option<Document<'_>> {
    std::str::from_utf8(bytes).ok().and_then(json::read)
}

/// The differences two JSON documents' walk finds.
#[derive(Debug, Default)]
struct Walk {
    paths: Vec<(String, String, String)>,
    layout: bool,
}

fn documents(stage: &str, x: &Document<'_>, y: &Document<'_>, found: &mut Found) {
    let mut walk = Walk {
        layout: x.before != y.before || x.after != y.after,
        ..Walk::default()
    };
    nodes("$", &x.root, &y.root, &mut walk);
    for (path, l, r) in walk.paths {
        found.add(format!("{stage}:{path}"), &l, &r);
    }
    if walk.layout {
        let (l, r) = (x.root.text.as_bytes(), y.root.text.as_bytes());
        let at = common_prefix(l, r);
        found.add(
            format!("{stage}:json-format"),
            &around(l, at),
            &around(r, at),
        );
    }
}

fn nodes(path: &str, x: &Node<'_>, y: &Node<'_>, walk: &mut Walk) {
    match (&x.kind, &y.kind) {
        (Kind::Object(l, l_close), Kind::Object(r, r_close)) => {
            members(path, l, r, walk);
            walk.layout |= l.is_empty() == r.is_empty() && l_close != r_close;
        }
        (Kind::Array(l, l_close), Kind::Array(r, r_close)) => {
            elements(path, l, r, walk);
            walk.layout |= l.is_empty() == r.is_empty() && l_close != r_close;
        }
        _ if x.text == y.text => {}
        _ => walk
            .paths
            .push((path.to_owned(), x.text.to_owned(), y.text.to_owned())),
    }
}

fn members(path: &str, x: &[Member<'_>], y: &[Member<'_>], walk: &mut Walk) {
    let child = |member: &Member<'_>| {
        let key = member.key;
        format!(
            "{path}.{}",
            key.get(1..key.len().saturating_sub(1)).unwrap_or(key)
        )
    };
    let x_keys = occurrences(x.iter().map(|member| member.key));
    let y_keys = occurrences(y.iter().map(|member| member.key));
    for (member, key) in x.iter().zip(&x_keys) {
        match y_keys
            .iter()
            .position(|other| other == key)
            .map(|at| &y[at])
        {
            Some(other) => {
                walk.layout |= member.layout != other.layout
                    || both_differ(member.before_comma, other.before_comma);
                nodes(&child(member), &member.value, &other.value, walk);
            }
            None => walk.paths.push((
                format!("{}:missing", child(member)),
                member.value.text.to_owned(),
                String::new(),
            )),
        }
    }
    for (member, key) in y.iter().zip(&y_keys) {
        if !x_keys.contains(key) {
            walk.paths.push((
                format!("{}:extra", child(member)),
                String::new(),
                member.value.text.to_owned(),
            ));
        }
    }
    let (l, r) = (shared(&x_keys, &y_keys), shared(&y_keys, &x_keys));
    if l != r {
        let keys = |keys: &[&(&str, usize)]| {
            keys.iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>()
                .join(",")
        };
        walk.paths
            .push((format!("{path}:order"), keys(&l), keys(&r)));
    }
}

fn both_differ(x: Option<&str>, y: Option<&str>) -> bool {
    matches!((x, y), (Some(l), Some(r)) if l != r)
}

fn elements(path: &str, x: &[Element<'_>], y: &[Element<'_>], walk: &mut Walk) {
    if x.len() != y.len() {
        walk.paths.push((
            format!("{path}:length"),
            x.len().to_string(),
            y.len().to_string(),
        ));
    }
    let child = format!("{path}[]");
    for (l, r) in x.iter().zip(y) {
        walk.layout |= l.before != r.before || both_differ(l.before_comma, r.before_comma);
        nodes(&child, &l.value, &r.value, walk);
    }
}

/// Each item with the count of the same item before it, so a repeated
/// name pairs with its own repeat.
fn occurrences<'a, T: PartialEq + Copy + 'a>(items: impl Iterator<Item = T>) -> Vec<(T, usize)> {
    let mut seen: Vec<T> = Vec::new();
    items
        .map(|item| {
            let count = seen.iter().filter(|other| **other == item).count();
            seen.push(item);
            (item, count)
        })
        .collect()
}

/// The items of `x` that `y` holds too, in `x`'s order.
fn shared<'a, T: PartialEq>(x: &'a [T], y: &[T]) -> Vec<&'a T> {
    x.iter().filter(|item| y.contains(item)).collect()
}

/// Each differing line, after the lines both share at either end.
fn lines(stage: &str, a: &[u8], b: &[u8], found: &mut Found) {
    let split = |text: &'_ [u8]| {
        text.split(|byte| *byte == b'\n')
            .map(lossy)
            .collect::<Vec<_>>()
    };
    let (x, y) = (split(a), split(b));
    for (index, (p, q)) in differing(&x, &y).into_iter().enumerate() {
        if index == SPAN_MAX {
            found.add(format!("{stage}:text:..."), "", "");
            break;
        }
        let (p, q) = (
            p.cloned().unwrap_or_default(),
            q.cloned().unwrap_or_default(),
        );
        let fingerprint = format!(
            "{stage}:text:{}->{}",
            mask(&p, LINE_CHARS),
            mask(&q, LINE_CHARS)
        );
        found.add(fingerprint, &p, &q);
    }
}

/// The items at each place between what `x` and `y` share at the start
/// and the end that differ, either absent past its side's end.
fn differing<'s, T: PartialEq>(x: &'s [T], y: &'s [T]) -> Vec<(Option<&'s T>, Option<&'s T>)> {
    let (l, r) = span(x, y);
    let pairs = (0..l.len().max(r.len())).map(|index| (l.get(index), r.get(index)));
    pairs.filter(|(p, q)| p != q).collect()
}

/// What lies between the items `x` and `y` share at the start and the end.
fn span<'s, T: PartialEq>(x: &'s [T], y: &'s [T]) -> (&'s [T], &'s [T]) {
    let prefix = x.iter().zip(y).take_while(|(l, r)| l == r).count();
    let (x, y) = (&x[prefix..], &y[prefix..]);
    let suffix = x
        .iter()
        .rev()
        .zip(y.iter().rev())
        .take_while(|(l, r)| l == r)
        .count();
    (&x[..x.len() - suffix], &y[..y.len() - suffix])
}

fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(l, r)| l == r).count()
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

fn connects(a: &[String], b: &[String], found: &mut Found) {
    if a.len() != b.len() {
        found.add(
            "connects:count".to_owned(),
            &a.len().to_string(),
            &b.len().to_string(),
        );
    }
    if a.iter().zip(b).any(|(l, r)| l != r) {
        found.add("connects:host".to_owned(), &a.join(" "), &b.join(" "));
    }
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
    let mark = found.noted;
    let parts = [
        ("method", &x.method, &y.method),
        ("version", &x.version, &y.version),
        ("host", &x.host, &y.host),
        ("path", &x.path, &y.path),
    ];
    for (name, l, r) in parts {
        if l != r {
            found.add(format!("{REQUEST}.line:{name}"), l, r);
        }
    }
    let query = |normal: &Normal| normalize::fields(normal.query.as_deref().unwrap_or_default());
    fields(
        &format!("{REQUEST}.line:query"),
        &query(x),
        &query(y),
        found,
    );
    if x.query.is_some() != y.query.is_some() {
        found.add(format!("{REQUEST}.line:query-format"), &x.target, &y.target);
    }
    let stage = format!("{REQUEST}.line:target");
    found.backstop(mark, &stage, x.target.as_bytes(), y.target.as_bytes());
}

/// A query's or a form's fields: each name whose fields differ, digits
/// masked, then `-order` for the shared fields' order and `-format` for
/// empty fields.
fn fields(stage: &str, x: &Fields, y: &Fields, found: &mut Found) {
    let render = |fields: &Fields, name: &str| {
        let matching = fields.pairs.iter().filter(|(key, _)| key == name);
        let shown = matching
            .map(|(key, value)| value.as_ref().map_or(key.clone(), |v| format!("{key}={v}")));
        shown.collect::<Vec<_>>().join("&")
    };
    let names = |fields: &Fields| {
        fields
            .pairs
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>()
    };
    for name in union(names(x), names(y)) {
        let (l, r) = (render(x, &name), render(y, &name));
        if l != r {
            found.add(format!("{stage}:{}", mask(&name, usize::MAX)), &l, &r);
        }
    }
    let x_keys = occurrences(x.pairs.iter().map(|(key, _)| key.as_str()));
    let y_keys = occurrences(y.pairs.iter().map(|(key, _)| key.as_str()));
    let (l, r) = (shared(&x_keys, &y_keys), shared(&y_keys, &x_keys));
    if l != r {
        let keys = |keys: &[&(&str, usize)]| {
            keys.iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>()
                .join("&")
        };
        found.add(format!("{stage}-order"), &keys(&l), &keys(&r));
    }
    if x.empty != y.empty {
        found.add(
            format!("{stage}-format"),
            &format!("{:?}", x.empty),
            &format!("{:?}", y.empty),
        );
    }
}

/// The distinct items of `x` then of `y`, each where it first appears.
fn union(x: Vec<String>, y: Vec<String>) -> Vec<String> {
    let mut seen = Vec::new();
    for item in x.into_iter().chain(y) {
        if !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

type Headers = [(String, String)];

fn headers(x: &Headers, y: &Headers, found: &mut Found) {
    let mark = found.noted;
    let lower = |headers: &Headers| {
        headers
            .iter()
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect::<Vec<_>>()
    };
    let (x_names, y_names) = (lower(x), lower(y));
    for name in union(x_names.clone(), y_names.clone()) {
        header(&name, &entries(x, &name), &entries(y, &name), found);
    }
    let x_order = occurrences(x_names.iter().map(String::as_str));
    let y_order = occurrences(y_names.iter().map(String::as_str));
    let (l, r) = (shared(&x_order, &y_order), shared(&y_order, &x_order));
    if l != r {
        let names = |names: &[&(&str, usize)]| {
            names
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(" ")
        };
        found.add(format!("{REQUEST}.header-order"), &names(&l), &names(&r));
    }
    let flat = |headers: &Headers| {
        headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\n"))
            .collect::<String>()
    };
    found.backstop(
        mark,
        &format!("{REQUEST}.header"),
        flat(x).as_bytes(),
        flat(y).as_bytes(),
    );
}

fn entries<'a>(headers: &'a Headers, name: &str) -> Vec<&'a (String, String)> {
    headers
        .iter()
        .filter(|(header, _)| header.eq_ignore_ascii_case(name))
        .collect()
}

type Entries<'a> = [&'a (String, String)];

/// One header name's entries on each side: absent on one, or its values,
/// how many times it appears and how its values are cut, and the case of
/// its name.
fn header(name: &str, x: &Entries<'_>, y: &Entries<'_>, found: &mut Found) {
    fn values<'a>(entries: &Entries<'a>) -> Vec<&'a str> {
        entries.iter().map(|(_, value)| value.as_str()).collect()
    }
    let stage = format!("{REQUEST}.header:{name}");
    let (l, r) = (values(x).join(", "), values(y).join(", "));
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
    if x.len() != y.len() {
        found.add(
            format!("{stage}:count"),
            &x.len().to_string(),
            &y.len().to_string(),
        );
    } else if l == r && values(x) != values(y) {
        found.add(
            format!("{stage}:split"),
            &values(x).join(" | "),
            &values(y).join(" | "),
        );
    }
    if x.iter().zip(y).any(|((l, _), (r, _))| l != r) {
        let names = |entries: &Entries<'_>| {
            entries
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        found.add(format!("{stage}:case"), &names(x), &names(y));
    }
}

fn user_agent(a: &str, b: &str, found: &mut Found) {
    if a == b {
        return;
    }
    let (x, y): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
    let keyed = |tokens: &[&str], key: &str| {
        let matching = tokens.iter().filter(|token| token_key(token) == key);
        matching.copied().collect::<Vec<_>>().join(" ")
    };
    let keys = |tokens: &[&str]| {
        tokens
            .iter()
            .map(|token| token_key(token).to_owned())
            .collect::<Vec<_>>()
    };
    for key in union(keys(&x), keys(&y)) {
        let (l, r) = (keyed(&x, &key), keyed(&y, &key));
        if l != r {
            found.add(format!("{REQUEST}.header:user-agent:{key}"), &l, &r);
        }
    }
    let x_keys = occurrences(x.iter().map(|token| token_key(token)));
    let y_keys = occurrences(y.iter().map(|token| token_key(token)));
    if shared(&x_keys, &y_keys) != shared(&y_keys, &x_keys) {
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
    if x.framing != y.framing {
        found.add(
            format!("{stage}:framing"),
            &format!("{:?}", x.framing),
            &format!("{:?}", y.framing),
        );
    }
    if x.body == y.body {
        return;
    }
    let mark = found.noted;
    if let (Some(l), Some(r)) = (read_json(&x.body), read_json(&y.body)) {
        documents(&stage, &l, &r, found);
    } else if is_form(x) && is_form(y) {
        let form = |normal: &Normal| normalize::fields(&lossy(&normal.body));
        fields(&stage, &form(x), &form(y), found);
    } else if is_xml(&x.body) && is_xml(&y.body) {
        xml(&stage, &x.body, &y.body, found);
    } else {
        let at = common_prefix(&x.body, &y.body);
        found.add(
            format!("{stage}:bytes"),
            &around(&x.body, at),
            &around(&y.body, at),
        );
    }
    found.backstop(mark, &stage, &x.body, &y.body);
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

fn is_xml(body: &[u8]) -> bool {
    body.trim_ascii_start().starts_with(b"<")
}

/// Each differing XML token, a tag or the text between tags, named by the
/// path of the elements open at it.
fn xml(stage: &str, a: &[u8], b: &[u8], found: &mut Found) {
    let (x, y) = (xml_tokens(a), xml_tokens(b));
    for (index, (p, q)) in differing(&x, &y).into_iter().enumerate() {
        if index == SPAN_MAX {
            found.add(format!("{stage}:..."), "", "");
            break;
        }
        let path = p.or(q).map_or("/", |(path, _)| path.as_str());
        let text = |token: Option<&(String, &[u8])>| {
            token.map_or_else(String::new, |(_, bytes)| lossy(bytes))
        };
        found.add(format!("{stage}:{path}"), &text(p), &text(q));
    }
}

fn xml_tokens(body: &[u8]) -> Vec<(String, &[u8])> {
    let mut open: Vec<String> = Vec::new();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < body.len() {
        let end = if body[at] == b'<' {
            body[at..]
                .iter()
                .position(|byte| *byte == b'>')
                .map_or(body.len(), |i| at + i + 1)
        } else {
            body[at..]
                .iter()
                .position(|byte| *byte == b'<')
                .map_or(body.len(), |i| at + i)
        };
        let token = &body[at..end];
        let closing = token.starts_with(b"</");
        let opening = token.starts_with(b"<")
            && !closing
            && !token.starts_with(b"<?")
            && !token.starts_with(b"<!");
        if opening {
            let name =
                token[1..].split(|byte| byte.is_ascii_whitespace() || matches!(byte, b'>' | b'/'));
            open.push(lossy(name.into_iter().next().unwrap_or_default()));
        }
        tokens.push((format!("/{}", open.join("/")), token));
        if closing || (opening && token.ends_with(b"/>")) {
            open.pop();
        }
        at = end;
    }
    tokens
}

/// The bytes around `at`, as text.
fn around(body: &[u8], at: usize) -> String {
    let start = at.saturating_sub(CONTEXT_BYTES);
    let end = (at + CONTEXT_BYTES).min(body.len());
    lossy(body.get(start..end).unwrap_or_default())
}

#[cfg(test)]
mod tests;
