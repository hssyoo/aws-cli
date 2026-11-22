//! A server's check of a header-signed request, for tests whose loopback
//! servers stand in for AWS: the signature is computed again from what
//! arrived, so a header the client signed but sent differently, or did not
//! send, fails it.
use crate::{Credentials, PathEncoding, Payload, Scope, SignedRequest, sign};
use timestamp::Timestamp;

/// Checks the `Authorization` of a request as it arrived: `head` is its
/// request line and headers, up to the blank line, and `body` its body.
/// The access key comes from the request; `secret_access_key` is the
/// server's.
///
/// # Errors
/// What fails to verify, in words.
pub fn verify(
    head: &str,
    body: &[u8],
    secret_access_key: &str,
    path_encoding: PathEncoding,
) -> Result<(), String> {
    let received = Received::parse(head)?;
    let one = |name: &str| received.one(name);
    let authorization = one("authorization")?;
    let (access_key_id, region, service, signed) = authorization_fields(authorization)?;
    let date = one("x-amz-date")?;
    let timestamp =
        Timestamp::parse_iso8601_basic(date).ok_or(format!("a malformed X-Amz-Date {date}"))?;
    let session_token = match signed.contains(&"x-amz-security-token") {
        true => Some(one("x-amz-security-token")?),
        false => None,
    };
    let content_sha256 = signed.contains(&"x-amz-content-sha256");
    let payload = match content_sha256 && one("x-amz-content-sha256")? == crate::UNSIGNED_PAYLOAD {
        true => Payload::Unsigned,
        false => Payload::Body(body),
    };
    let set = [
        "host",
        "x-amz-date",
        "x-amz-security-token",
        "x-amz-content-sha256",
    ];
    let others: Vec<(&str, &str)> = received
        .headers
        .iter()
        .filter(|(name, _)| signed.contains(&name.as_str()) && !set.contains(&name.as_str()))
        .map(|(name, value)| (name.as_str(), *value))
        .collect();
    let expected = sign(&SignedRequest {
        credentials: Credentials {
            access_key_id,
            secret_access_key,
            session_token,
        },
        scope: Scope { region, service },
        timestamp,
        method: received.method,
        host: one("host")?,
        path: received.path,
        query: received.query,
        headers: &others,
        payload,
        content_sha256,
        path_encoding,
    });
    let computed = expected
        .headers
        .iter()
        .find(|(name, _)| *name == "Authorization")
        .map_or("", |(_, value)| value.as_str());
    match computed == authorization {
        true => Ok(()),
        false => Err(format!(
            "{authorization} does not verify: expected {computed}"
        )),
    }
}

/// The access key, region, service and signed header names an
/// `Authorization` value names.
fn authorization_fields(authorization: &str) -> Result<(&str, &str, &str, Vec<&str>), String> {
    let field = |key: &str| {
        authorization
            .split([' ', ','])
            .find_map(|part| part.strip_prefix(key))
            .ok_or(format!("no {key} in {authorization}"))
    };
    let credential: Vec<&str> = field("Credential=")?.split('/').collect();
    let [access_key_id, _, region, service, _] = credential[..] else {
        return Err(format!("a malformed credential in {authorization}"));
    };
    let signed = field("SignedHeaders=")?.split(';').collect();
    Ok((access_key_id, region, service, signed))
}

/// A request's head as it arrived.
struct Received<'a> {
    method: &'a str,
    path: &'a str,
    query: &'a str,
    /// Names lowercased, values trimmed, in order.
    headers: Vec<(String, &'a str)>,
}

impl<'a> Received<'a> {
    fn parse(head: &'a str) -> Result<Self, String> {
        let mut lines = head.lines();
        let mut request_line = lines.next().ok_or("no request line")?.split(' ');
        let method = request_line.next().unwrap_or("");
        let target = request_line.next().unwrap_or("");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim()))
            .collect();
        Ok(Self {
            method,
            path,
            query,
            headers,
        })
    }
    /// The value of the one header named `name`.
    fn one(&self, name: &str) -> Result<&'a str, String> {
        let mut found = self.headers.iter().filter(|(other, _)| other == name);
        match (found.next(), found.next()) {
            (Some((_, value)), None) => Ok(value),
            _ => Err(format!("not exactly one {name} header")),
        }
    }
}
