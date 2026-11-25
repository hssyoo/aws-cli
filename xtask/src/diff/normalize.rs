//! What varies from run to run, replaced alike on both sides. Nothing else
//! is touched: header names, order and case, Content-Length, SignedHeaders,
//! stdout and stderr are compared as captured.
//!
//! - `Authorization`: the `Signature=` value becomes `<sig>` and the
//!   credential scope's date `<date>`; in a query, `X-Amz-Signature`,
//!   `X-Amz-Date` and the `X-Amz-Credential` date alike.
//! - `X-Amz-Date` becomes `<amz-date>`.
//! - `amz-sdk-invocation-id` becomes `<invocation-id:k>`, k numbering the
//!   run's distinct ids by first appearance, so a retry that keeps its id
//!   still matches only one that does.
//! - `amz-sdk-request`: only `ttl=` becomes `<ttl>`; `attempt=` and `max=`
//!   are deterministic and compared.
//! - `User-Agent`, token by token: the host's tokens `os/<family>#<release>`,
//!   and `os/other` with the `md/<system>#<release>` after it, become
//!   `os/<os>`, `md/arch#*` `md/arch#<arch>`, `md/distrib#*`
//!   `md/distrib#<distrib>`, and the CRT's `platform/*` `platform/<platform>`;
//!   the Python route's telemetry session `sid/*` becomes `sid/<sid>`, and
//!   the `m/` feature codes are sorted, as Python joins them from a set.
//! - Version 4 UUIDs in a body or a query value, the idempotency tokens the
//!   CLI generates, become `<uuid:k>` by first appearance, unless the row's
//!   own arguments or variables hold them.

use std::collections::BTreeSet;

use super::capture::Request;

/// A request as compared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Normal {
    pub method: String,
    pub version: String,
    pub host: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    /// In wire order, each name as sent and its value normalized.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub complete: bool,
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

/// One run's numberings, and the UUIDs its row gave.
#[derive(Debug)]
struct State {
    ids: Numbering,
    uuids: Numbering,
    exempt: BTreeSet<String>,
}

/// The run's requests, normalized; `inputs` are the row's arguments and
/// variables, whose UUIDs stay.
pub fn requests<'a>(
    requests: &[Request],
    inputs: impl IntoIterator<Item = &'a str>,
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
    };
    requests
        .iter()
        .map(|request| normal(request, &mut state))
        .collect()
}

fn normal(request: &Request, state: &mut State) -> Normal {
    let (host, rest) = match split_absolute(&request.target) {
        Some((authority, rest)) => (authority.to_owned(), rest),
        None => (
            header_value(&request.headers, "host").to_owned(),
            request.target.as_str(),
        ),
    };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let headers = request
        .headers
        .iter()
        .map(|(name, value)| (name.clone(), header(name, value, &mut state.ids)))
        .collect();
    Normal {
        method: request.method.clone(),
        version: request.version.clone(),
        host,
        path: path.to_owned(),
        query: query_pairs(query, state),
        headers,
        body: replace_uuids(&request.body, state),
        complete: request.complete,
    }
}

/// An absolute-form target's authority and the rest, from `/` on.
fn split_absolute(target: &str) -> Option<(&str, &str)> {
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))?;
    Some(rest.find('/').map_or((rest, "/"), |at| rest.split_at(at)))
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> &'a str {
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map_or("", |(_, value)| value)
}

fn header(name: &str, value: &str, ids: &mut Numbering) -> String {
    match name.to_ascii_lowercase().as_str() {
        "authorization" => scope_date(&after_marker(value, "Signature=", "<sig>")),
        "x-amz-date" => "<amz-date>".to_owned(),
        "amz-sdk-invocation-id" => ids.label("invocation-id", value),
        "amz-sdk-request" => sdk_request(value),
        "user-agent" => user_agent(value),
        _ => value.to_owned(),
    }
}

/// `value` with the hexadecimal run after `marker` replaced.
fn after_marker(value: &str, marker: &str, placeholder: &str) -> String {
    let Some(start) = value.find(marker).map(|at| at + marker.len()) else {
        return value.to_owned();
    };
    let length = value[start..]
        .bytes()
        .take_while(u8::is_ascii_hexdigit)
        .count();
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
            if trimmed.starts_with("ttl=") {
                format!("{}ttl=<ttl>", &part[..part.len() - trimmed.len()])
            } else {
                part.to_owned()
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
        if system {
            continue;
        }
        tokens.push(user_agent_token(token));
    }
    tokens.join(" ")
}

fn user_agent_token(token: &str) -> String {
    let placeholder = |prefix: &str, name: &str| {
        token
            .starts_with(prefix)
            .then(|| format!("{prefix}<{name}>"))
    };
    if let Some(codes) = token.strip_prefix("m/") {
        let mut codes: Vec<&str> = codes.split(',').collect();
        codes.sort_unstable();
        return format!("m/{}", codes.join(","));
    }
    placeholder("os/", "os")
        .or_else(|| placeholder("md/arch#", "arch"))
        .or_else(|| placeholder("md/distrib#", "distrib"))
        .or_else(|| placeholder("platform/", "platform"))
        .or_else(|| placeholder("sid/", "sid"))
        .unwrap_or_else(|| token.to_owned())
}

fn query_pairs(query: &str, state: &mut State) -> Vec<(String, String)> {
    if query.is_empty() {
        return Vec::new();
    }
    query
        .split('&')
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = match name {
                "X-Amz-Signature" => "<sig>".to_owned(),
                "X-Amz-Date" => "<amz-date>".to_owned(),
                "X-Amz-Credential" => scope_date(value),
                _ => String::from_utf8_lossy(&replace_uuids(value.as_bytes(), state)).into_owned(),
            };
            (name.to_owned(), value)
        })
        .collect()
}

fn replace_uuids(text: &[u8], state: &mut State) -> Vec<u8> {
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
    candidate
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
