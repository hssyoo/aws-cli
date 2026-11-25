//! What varies from run to run, replaced alike on both sides, each only
//! when it has the form it should; a value of another form stays as sent.
//! Nothing else is touched: header names, order and case, Content-Length,
//! SignedHeaders, stdout and stderr are compared as captured.
//!
//! - `Authorization`: a 64-digit lowercase hexadecimal `Signature=` becomes
//!   its verdict ([`sigv4`]): `<sig:ok>` when the request's own bytes and
//!   the run's fake secret key give it, `<sig:bad>` when they do not, or
//!   `<sig:unverified>`; and the credential scope's eight-digit date
//!   becomes `<date>`. In a query, `X-Amz-Signature` becomes `<sig>`, and
//!   `X-Amz-Date` and the `X-Amz-Credential` date as in the header.
//! - `X-Amz-Date` of the form `YYYYMMDDTHHMMSSZ` becomes `<amz-date>`.
//! - A version 4 UUID `amz-sdk-invocation-id` becomes `<invocation-id:k>`, k
//!   numbering the run's distinct ids by first appearance, so a retry that
//!   keeps its id still matches only one that does.
//! - `amz-sdk-request`: only a `ttl=` date becomes `<ttl>`; `attempt=` and
//!   `max=` are deterministic and compared.
//! - `User-Agent`, token by token: the host's release in `os/<family>#*`, and
//!   in the `md/<system>#*` after `os/other`, becomes `<release>`, and the
//!   version in `md/distrib#<id>.*` `<version>`; family, system, id and
//!   `md/arch` stay, as both sides run on one host. The Python route's
//!   telemetry session `sid/<hex>` becomes `sid/<sid>`, and the `m/` feature
//!   codes are sorted, as Python joins them from a set.
//! - Version 4 UUIDs in a body or a query value, the idempotency tokens the
//!   CLI generates, become `<uuid:k>` by first appearance, unless the row
//!   itself holds them: its arguments, variables, or replies, which the CLI
//!   may echo.

use std::collections::BTreeSet;

use super::capture::Request;
use super::sigv4::{self, Verdict};

/// A request as compared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Normal {
    pub method: String,
    pub version: String,
    pub host: String,
    /// The target as sent, its query values normalized.
    pub target: String,
    pub path: String,
    /// The query after `?`, normalized; none without a `?`.
    pub query: Option<String>,
    /// In wire order, each name as sent and its value normalized.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// A chunked body's chunk-size lines, then its trailers.
    pub framing: Option<Vec<String>>,
    pub complete: bool,
}

/// A query's or a form's `&`-separated fields: each name with its value,
/// none for a field without `=`, and where empty fields fall.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields {
    pub pairs: Vec<(String, Option<String>)>,
    /// Where each empty field is: after which field (its name and how many
    /// fields of that name came before it), or at the start.
    pub empty: Vec<Option<(String, usize)>>,
}

/// The fields of `text`; the empty text has none.
pub fn fields(text: &str) -> Fields {
    let mut fields = Fields::default();
    if text.is_empty() {
        return fields;
    }
    for field in text.split('&') {
        if field.is_empty() {
            let after = fields.pairs.last().map(|(name, _)| {
                let before = fields
                    .pairs
                    .iter()
                    .filter(|(other, _)| other == name)
                    .count();
                (name.clone(), before - 1)
            });
            fields.empty.push(after);
            continue;
        }
        fields.pairs.push(match field.split_once('=') {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (field.to_owned(), None),
        });
    }
    fields
}

/// The run's distinct values of one kind, numbered by first appearance.
#[derive(Debug, Default)]
struct Numbering {
    seen: Vec<String>,
}

impl Numbering {
    fn label(&mut self, kind: &str, value: &str) -> String {
        let index = match self.seen.iter().position(|seen| seen == value) {
            Some(index) => index,
            None => {
                self.seen.push(value.to_owned());
                self.seen.len() - 1
            }
        };
        format!("<{kind}:{}>", index + 1)
    }
}

/// One run's numberings, the UUIDs its row gave, and its secret key.
#[derive(Debug)]
struct State<'a> {
    ids: Numbering,
    uuids: Numbering,
    exempt: BTreeSet<String>,
    secret: Option<&'a str>,
}

/// The run's requests, normalized; `inputs` are the texts the row holds,
/// whose UUIDs stay, and `secret` the run's secret key, if it had one.
pub fn requests<'a>(
    requests: &[Request],
    inputs: impl IntoIterator<Item = &'a str>,
    secret: Option<&str>,
) -> Vec<Normal> {
    let exempt = inputs
        .into_iter()
        .flat_map(|input| {
            uuids(input.as_bytes())
                .into_iter()
                .map(|at| input[at..at + 36].to_owned())
        })
        .collect();
    let mut state = State {
        ids: Numbering::default(),
        uuids: Numbering::default(),
        exempt,
        secret,
    };
    requests
        .iter()
        .map(|request| normal(request, &mut state))
        .collect()
}

fn normal(request: &Request, state: &mut State<'_>) -> Normal {
    let (prefix, host, rest) = match split_absolute(&request.target) {
        Some((prefix, authority, rest)) => (prefix, authority.to_owned(), rest),
        None => {
            let host = header_value(&request.headers, "host").to_owned();
            ("", host, request.target.as_str())
        }
    };
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query_values(query, state))),
        None => (rest, None),
    };
    let verdict = sigv4::verify(request, state.secret);
    let headers = request
        .headers
        .iter()
        .map(|(name, value)| (name.clone(), header(name, value, verdict, &mut state.ids)))
        .collect();
    let query_text = query
        .as_ref()
        .map_or_else(String::new, |query| format!("?{query}"));
    Normal {
        method: request.method.clone(),
        version: request.version.clone(),
        host,
        target: format!("{prefix}{path}{query_text}"),
        path: path.to_owned(),
        query,
        headers,
        body: replace_uuids(&request.body, state),
        framing: request.framing.clone(),
        complete: request.complete,
    }
}

/// An absolute-form target's scheme and authority, its authority, and the
/// rest, from `/` on.
fn split_absolute(target: &str) -> Option<(&str, &str, &str)> {
    let scheme = ["http://", "https://"]
        .into_iter()
        .find(|scheme| target.starts_with(scheme))?;
    let rest = &target[scheme.len()..];
    let end = scheme.len() + rest.find('/').unwrap_or(rest.len());
    Some((&target[..end], &target[scheme.len()..end], &target[end..]))
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map_or("", |(_, value)| value)
}

fn header(name: &str, value: &str, verdict: Verdict, ids: &mut Numbering) -> String {
    match name.to_ascii_lowercase().as_str() {
        "authorization" => scope_date(&signature(value, verdict)),
        "x-amz-date" if is_amz_date(value) => "<amz-date>".to_owned(),
        "amz-sdk-invocation-id" if is_uuid(value.as_bytes()) => ids.label("invocation-id", value),
        "amz-sdk-request" => sdk_request(value),
        "user-agent" => user_agent(value),
        _ => value.to_owned(),
    }
}

/// `YYYYMMDDTHHMMSSZ`.
fn is_amz_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    bytes.len() == 16 && bytes[8] == b'T' && bytes[15] == b'Z' && digits(0..8) && digits(9..15)
}

/// Sixty-four lowercase hexadecimal digits.
fn is_signature(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// `value` with the signature after `Signature=` replaced by its verdict.
fn signature(value: &str, verdict: Verdict) -> String {
    let marker = "Signature=";
    let Some(start) = value.find(marker).map(|at| at + marker.len()) else {
        return value.to_owned();
    };
    let length = value[start..]
        .bytes()
        .take_while(u8::is_ascii_alphanumeric)
        .count();
    if !is_signature(&value[start..start + length]) {
        return value.to_owned();
    }
    let placeholder = verdict.placeholder();
    format!(
        "{}{placeholder}{}",
        &value[..start],
        &value[start + length..]
    )
}

/// `value` with its first credential scope date, eight digits between
/// slashes (`/` or `%2F`), replaced.
fn scope_date(value: &str) -> String {
    let bytes = value.as_bytes();
    for (at, _) in value.match_indices('/').chain(value.match_indices("%2F")) {
        let separator = if bytes[at] == b'/' { 1 } else { 3 };
        let start = at + separator;
        let digits = bytes.get(start..start + 8);
        if !digits.is_some_and(|digits| digits.iter().all(u8::is_ascii_digit)) {
            continue;
        }
        let after = &value[start + 8..];
        if after.starts_with('/') || after.starts_with("%2F") {
            return format!("{}<date>{after}", &value[..start]);
        }
    }
    value.to_owned()
}

fn sdk_request(value: &str) -> String {
    let parts: Vec<String> = value
        .split(';')
        .map(|part| {
            let trimmed = part.trim_start();
            match trimmed.strip_prefix("ttl=") {
                Some(date) if is_amz_date(date) => {
                    format!("{}ttl=<ttl>", &part[..part.len() - trimmed.len()])
                }
                _ => part.to_owned(),
            }
        })
        .collect();
    parts.join(";")
}

fn user_agent(value: &str) -> String {
    let mut tokens: Vec<String> = Vec::new();
    let mut previous_other = false;
    for token in value.split(' ') {
        let system = previous_other && token.starts_with("md/") && !token.starts_with("md/arch");
        previous_other = token == "os/other";
        tokens.push(if system {
            release(token)
        } else {
            user_agent_token(token)
        });
    }
    tokens.join(" ")
}

/// `token` with the value after its `#` as `<release>`.
fn release(token: &str) -> String {
    match token.split_once('#') {
        Some((name, value)) if !value.is_empty() => format!("{name}#<release>"),
        _ => token.to_owned(),
    }
}

fn user_agent_token(token: &str) -> String {
    if let Some(codes) = token.strip_prefix("m/") {
        let mut codes: Vec<&str> = codes.split(',').collect();
        codes.sort_unstable();
        return format!("m/{}", codes.join(","));
    }
    if token.starts_with("os/") {
        return release(token);
    }
    let distribution = token
        .strip_prefix("md/distrib#")
        .and_then(|rest| rest.rsplit_once('.'));
    if let Some((id, _)) = distribution.filter(|(_, version)| !version.is_empty()) {
        return format!("md/distrib#{id}.<version>");
    }
    match token.strip_prefix("sid/") {
        Some(hex) if !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            "sid/<sid>".to_owned()
        }
        _ => token.to_owned(),
    }
}

/// `query` with each field's value normalized, its fields and framing as
/// sent.
fn query_values(query: &str, state: &mut State<'_>) -> String {
    let fields: Vec<String> = query
        .split('&')
        .map(|field| match field.split_once('=') {
            Some((name, value)) => format!("{name}={}", query_value(name, value, state)),
            None => field.to_owned(),
        })
        .collect();
    fields.join("&")
}

fn query_value(name: &str, value: &str, state: &mut State<'_>) -> String {
    match name {
        "X-Amz-Signature" if is_signature(value) => "<sig>".to_owned(),
        "X-Amz-Date" if is_amz_date(value) => "<amz-date>".to_owned(),
        "X-Amz-Credential" => scope_date(value),
        _ => String::from_utf8_lossy(&replace_uuids(value.as_bytes(), state)).into_owned(),
    }
}

fn replace_uuids(text: &[u8], state: &mut State<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut copied = 0;
    for at in uuids(text) {
        let uuid = String::from_utf8_lossy(&text[at..at + 36]).into_owned();
        if state.exempt.contains(&uuid) {
            continue;
        }
        out.extend_from_slice(&text[copied..at]);
        out.extend_from_slice(state.uuids.label("uuid", &uuid).as_bytes());
        copied = at + 36;
    }
    out.extend_from_slice(&text[copied..]);
    out
}

/// The offsets of the version 4 UUIDs in canonical form in `text`, not
/// inside a longer run of hexadecimal digits and hyphens.
fn uuids(text: &[u8]) -> Vec<usize> {
    let part = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'-';
    let mut found = Vec::new();
    let mut at = 0;
    while at + 36 <= text.len() {
        let bounded =
            (at == 0 || !part(&text[at - 1])) && text.get(at + 36).is_none_or(|b| !part(b));
        if bounded && is_uuid(&text[at..at + 36]) {
            found.push(at);
            at += 36;
        } else {
            at += 1;
        }
    }
    found
}

fn is_uuid(candidate: &[u8]) -> bool {
    candidate.len() == 36
        && candidate
            .iter()
            .enumerate()
            .all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => *byte == b'-',
                14 => *byte == b'4',
                19 => matches!(byte, b'8' | b'9' | b'a' | b'b' | b'A' | b'B'),
                _ => byte.is_ascii_hexdigit(),
            })
}

#[cfg(test)]
mod tests;
