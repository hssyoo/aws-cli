//! AWS Signature Version 4, as botocore's `SigV4Auth`, `S3SigV4Auth`,
//! `SigV4QueryAuth` and `S3SigV4QueryAuth` compute it
//! (`awscli/botocore/auth.py`).
//!
//! Two entry points share one canonicalization and one signature: [`presign`]
//! builds a presigned URL (query signing, `UNSIGNED-PAYLOAD`), and [`sign`]
//! takes a request's own headers and body and returns the headers that
//! authenticate it (`SigV4Auth.add_auth`), every one of them computed from
//! the one list it signs. Instants are [`timestamp::Timestamp`]s.
//!
//! ```
//! use sigv4::{Credentials, Scope, Presign, presign};
//! use timestamp::Timestamp;
//!
//! let timestamp = Timestamp::from_unix_seconds(1_471_530_783).ok_or("out of range")?;
//! let url = presign(&Presign {
//!     credentials: Credentials {
//!         access_key_id: "AKIDEXAMPLE",
//!         secret_access_key: "secret",
//!         session_token: None,
//!     },
//!     scope: Scope { region: "us-east-1", service: "s3" },
//!     timestamp,
//!     method: "GET",
//!     origin: "https://bucket.s3.us-east-1.amazonaws.com",
//!     path: "/key",
//!     query: "",
//!     expires: 3600,
//!     path_encoding: sigv4::PathEncoding::Raw,
//! });
//! assert!(url.contains("X-Amz-Date=20160818T143303Z&"));
//! # Ok::<(), &str>(())
//! ```
#![forbid(unsafe_code)]
mod encode;
mod host;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use encode::{quote, quote_bytes};
pub use host::host_header;

use timestamp::Timestamp;

use encode::{canonical_query, normalize_path};

/// The `UNSIGNED-PAYLOAD` hash of a presigned request
/// (`S3SigV4QueryAuth.payload`).
pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
const ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// The header that names a request's payload hash.
const CONTENT_SHA256: &str = "x-amz-content-sha256";

/// Headers botocore never signs (`SIGNED_HEADERS_BLACKLIST`).
const NEVER_SIGNED: [&str; 11] = [
    "connection",
    "expect",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "user-agent",
    "x-amzn-trace-id",
];

/// What a request is signed with (`botocore.credentials.Credentials`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Credentials<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
    pub session_token: Option<&'a str>,
}

/// The region and service a signature is scoped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope<'a> {
    pub region: &'a str,
    pub service: &'a str,
}

/// How the request's path enters the canonical request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathEncoding {
    /// S3: the path as sent (`S3SigV4QueryAuth._normalize_url_path`).
    Raw,
    /// Other services: dot segments removed, then quoted again with `/~`
    /// safe (`SigV4Auth._normalize_url_path`).
    Normalized,
}

/// A presigned URL's inputs (`SigV4QueryAuth`).
#[derive(Debug, Clone, Copy)]
pub struct Presign<'a> {
    pub credentials: Credentials<'a>,
    pub scope: Scope<'a>,
    pub timestamp: Timestamp,
    pub method: &'a str,
    /// `scheme://authority`, as the request's URL spells it.
    pub origin: &'a str,
    /// The URL's path, already percent-encoded.
    pub path: &'a str,
    /// The URL's own query, already encoded; usually empty.
    pub query: &'a str,
    /// `X-Amz-Expires`: the seconds the caller gave, which botocore sends as
    /// given, a negative number among them.
    pub expires: i64,
    pub path_encoding: PathEncoding,
}

/// What a header-signed request's payload contributes to its signature
/// (`SigV4Auth.payload`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload<'a> {
    /// The body as sent, whose SHA-256 is signed; empty for none.
    Body(&'a [u8]),
    /// `UNSIGNED-PAYLOAD`, for a client with payload signing turned off.
    Unsigned,
}

/// A request whose authenticating headers [`sign`] computes
/// (`SigV4Auth.add_auth`).
#[derive(Debug, Clone, Copy)]
pub struct SignedRequest<'a> {
    pub credentials: Credentials<'a>,
    pub scope: Scope<'a>,
    pub timestamp: Timestamp,
    pub method: &'a str,
    /// The Host header the transport sends. botocore signs the host of the
    /// URL and relies on its HTTP client sending the same; a caller passes
    /// the transport's own value so that the two cannot differ.
    pub host: &'a str,
    /// The request's path, already percent-encoded.
    pub path: &'a str,
    /// The URL's query, already encoded.
    pub query: &'a str,
    /// The request's own headers, as it would send them, without Host.
    /// Those botocore never signs, such as `User-Agent`, are left out of
    /// the signature. Those [`sign`] returns, which a request signed before
    /// carries, are replaced by them as botocore deletes them
    /// (`SigV4Auth._modify_request_before_signing`): `Authorization` and
    /// `X-Amz-Date` always, `X-Amz-Security-Token` when the credentials
    /// have a token, and `X-Amz-Content-SHA256` with [`Self::content_sha256`].
    /// Without those, a request's own `X-Amz-Security-Token` is signed and
    /// its own `X-Amz-Content-SHA256` is the payload hash signed, as there.
    /// Not ported: botocore's `Date` case (`_set_necessary_date_headers`),
    /// where a request's own `Date` is rewritten and sent in place of
    /// `X-Amz-Date`; here a `Date` is signed as any other header.
    pub headers: &'a [(&'a str, &'a str)],
    pub payload: Payload<'a>,
    /// Also sends the payload's hash as `X-Amz-Content-SHA256`, as S3
    /// requires (`S3SigV4Auth._modify_request_before_signing`).
    pub content_sha256: bool,
    pub path_encoding: PathEncoding,
}

/// The headers that authenticate a request, and the signature among them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    /// To send after the request's own headers, in botocore's order:
    /// `X-Amz-Date`, `X-Amz-Security-Token` when there is a token,
    /// `X-Amz-Content-SHA256` when asked for, then `Authorization`.
    pub headers: Vec<(&'static str, String)>,
    pub signature: String,
}

impl Signed {
    /// Whether these headers replace a request's own header `name`.
    fn replaces(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|(added, _)| name.trim().eq_ignore_ascii_case(added))
    }
    /// The headers to send after Host: the request's own, `base`, without
    /// those these replace, then these. A request sent with exactly them is
    /// the one [`sign`] signed.
    pub fn apply<'s>(&'s self, base: &[(&'s str, &'s str)]) -> Vec<(&'s str, &'s str)> {
        let mut headers: Vec<(&str, &str)> = base
            .iter()
            .filter(|(name, _)| !self.replaces(name))
            .copied()
            .collect();
        headers.extend(
            self.headers
                .iter()
                .map(|(name, value)| (*name, value.as_str())),
        );
        headers
    }
}

/// The lowercase hex SHA-256 of `bytes` (botocore's `sha256(...).hexdigest()`).
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    text
}

/// `SigV4Auth.scope`: access key, date, region, service.
fn credential(credentials: &Credentials<'_>, scope: &Scope<'_>, timestamp: &Timestamp) -> String {
    format!(
        "{}/{}",
        credentials.access_key_id,
        credential_scope(scope, timestamp)
    )
}

/// `SigV4Auth.credential_scope`.
fn credential_scope(scope: &Scope<'_>, timestamp: &Timestamp) -> String {
    format!(
        "{}/{}/{}/aws4_request",
        timestamp.basic_date(),
        scope.region,
        scope.service
    )
}

/// `SigV4Auth.canonical_headers` and `signed_headers`: names lowercased and
/// sorted, values of one name joined with commas, runs of whitespace within
/// a value collapsed.
fn canonical_headers(headers: &[(&str, &str)]) -> (String, String) {
    let mut names: Vec<String> = headers
        .iter()
        .map(|(name, _)| name.trim().to_ascii_lowercase())
        .collect();
    names.sort();
    names.dedup();
    let lines: Vec<String> = names
        .iter()
        .map(|name| {
            let values: Vec<String> = headers
                .iter()
                .filter(|(other, _)| other.trim().eq_ignore_ascii_case(name))
                .map(|(_, value)| value.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect();
            format!("{name}:{}", values.join(","))
        })
        .collect();
    (lines.join("\n"), names.join(";"))
}

/// What `SigV4Auth.canonical_request` reads of a request.
struct CanonicalRequest<'a> {
    method: &'a str,
    path: &'a str,
    path_encoding: PathEncoding,
    query: &'a str,
    /// Every header that is signed, `host` among them.
    headers: &'a [(&'a str, &'a str)],
    payload_hash: &'a str,
}

impl CanonicalRequest<'_> {
    /// The canonical request, and its signed header names.
    fn text(&self) -> (String, String) {
        let (names, signed) = canonical_headers(self.headers);
        let path = match self.path_encoding {
            PathEncoding::Raw => self.path.to_owned(),
            PathEncoding::Normalized => normalize_path(self.path),
        };
        let request = format!(
            "{}\n{path}\n{}\n{names}\n\n{signed}\n{}",
            self.method.to_ascii_uppercase(),
            canonical_query(self.query),
            self.payload_hash,
        );
        (request, signed)
    }
    /// `SigV4Auth.string_to_sign` and `signature`: the canonical request's
    /// signature under the key derived from the secret, and the signed
    /// header names.
    fn signature(
        &self,
        credentials: &Credentials<'_>,
        scope: &Scope<'_>,
        timestamp: &Timestamp,
    ) -> (String, String) {
        let (canonical, signed) = self.text();
        let to_sign = format!(
            "{ALGORITHM}\n{}\n{}\n{}",
            timestamp.iso8601_basic(),
            credential_scope(scope, timestamp),
            sha256_hex(canonical.as_bytes())
        );
        let secret = format!("AWS4{}", credentials.secret_access_key);
        let date = sha256::hmac(secret.as_bytes(), timestamp.basic_date().as_bytes());
        let region = sha256::hmac(&date, scope.region.as_bytes());
        let service = sha256::hmac(&region, scope.service.as_bytes());
        let key = sha256::hmac(&service, b"aws4_request");
        (hex(&sha256::hmac(&key, to_sign.as_bytes())), signed)
    }
}

/// The headers that authenticate `request` (`SigV4Auth.add_auth`, and
/// `S3SigV4Auth`'s with [`SignedRequest::content_sha256`]).
///
/// The signature covers the request's own headers but those botocore never
/// signs and those replaced, its Host, and every header returned but
/// `Authorization`; the request is authenticated when it sends exactly
/// [`Signed::apply`] of its own headers after Host.
pub fn sign(request: &SignedRequest<'_>) -> Signed {
    debug_assert!(
        request
            .headers
            .iter()
            .all(|(name, _)| !name.trim().eq_ignore_ascii_case("host")),
        "the request's own headers include Host"
    );
    let mut signed = Signed {
        headers: vec![("X-Amz-Date", request.timestamp.iso8601_basic())],
        signature: String::new(),
    };
    if let Some(token) = request.credentials.session_token {
        signed
            .headers
            .push(("X-Amz-Security-Token", token.to_owned()));
    }
    let payload_hash = payload_hash(request);
    if request.content_sha256 {
        signed
            .headers
            .push(("X-Amz-Content-SHA256", payload_hash.clone()));
    }
    let mut signed_headers: Vec<(&str, &str)> = vec![("host", request.host)];
    signed_headers.extend(request.headers.iter().copied().filter(|(name, _)| {
        let lower = name.trim().to_ascii_lowercase();
        !NEVER_SIGNED.contains(&lower.as_str())
            && !signed.replaces(&lower)
            && lower != "authorization"
    }));
    signed_headers.extend(
        signed
            .headers
            .iter()
            .map(|(name, value)| (*name, value.as_str())),
    );
    let canonical = CanonicalRequest {
        method: request.method,
        path: request.path,
        path_encoding: request.path_encoding,
        query: request.query,
        headers: &signed_headers,
        payload_hash: &payload_hash,
    };
    let (signature, names) =
        canonical.signature(&request.credentials, &request.scope, &request.timestamp);
    let authorization = format!(
        "{ALGORITHM} Credential={}, SignedHeaders={names}, Signature={signature}",
        credential(&request.credentials, &request.scope, &request.timestamp),
    );
    signed.headers.push(("Authorization", authorization));
    signed.signature = signature;
    signed
}

/// The payload hash a request is signed with (`SigV4Auth.payload`), or the
/// request's own `X-Amz-Content-SHA256` where the signer sets none, which
/// `SigV4Auth.canonical_request` signs in its place.
fn payload_hash(request: &SignedRequest<'_>) -> String {
    let own = request
        .headers
        .iter()
        .find(|(name, _)| name.trim().eq_ignore_ascii_case(CONTENT_SHA256));
    match (request.content_sha256, own, request.payload) {
        (false, Some((_, value)), _) => (*value).to_owned(),
        (_, _, Payload::Body(body)) => sha256_hex(body),
        (_, _, Payload::Unsigned) => UNSIGNED_PAYLOAD.to_owned(),
    }
}

/// The presigned URL of `request`, signing the `host` header only
/// (`SigV4QueryAuth.add_auth` with `S3SigV4QueryAuth`'s payload).
///
/// The URL's query is the request's own, then `X-Amz-Algorithm`,
/// `X-Amz-Credential`, `X-Amz-Date`, `X-Amz-Expires`, `X-Amz-SignedHeaders`,
/// `X-Amz-Security-Token` when there is a token, and `X-Amz-Signature`. The
/// canonical query sorts the same parameters by name instead.
pub fn presign(request: &Presign<'_>) -> String {
    let (scheme, authority) = request
        .origin
        .split_once("://")
        .unwrap_or(("", request.origin));
    let host = host_header(scheme, authority);
    let mut auth: Vec<(&str, String)> = vec![
        ("X-Amz-Algorithm", ALGORITHM.to_owned()),
        (
            "X-Amz-Credential",
            credential(&request.credentials, &request.scope, &request.timestamp),
        ),
        ("X-Amz-Date", request.timestamp.iso8601_basic()),
        ("X-Amz-Expires", request.expires.to_string()),
        ("X-Amz-SignedHeaders", "host".to_owned()),
    ];
    if let Some(token) = request.credentials.session_token {
        auth.push(("X-Amz-Security-Token", token.to_owned()));
    }
    let auth_query = auth
        .iter()
        .map(|(name, value)| format!("{name}={}", quote(value, "-_.~")))
        .collect::<Vec<_>>()
        .join("&");
    let query = if request.query.is_empty() {
        auth_query
    } else {
        format!("{}&{auth_query}", request.query)
    };
    let canonical = CanonicalRequest {
        method: request.method,
        path: request.path,
        path_encoding: request.path_encoding,
        query: &query,
        headers: &[("host", host.as_str())],
        payload_hash: UNSIGNED_PAYLOAD,
    };
    let (signature, _) =
        canonical.signature(&request.credentials, &request.scope, &request.timestamp);
    format!(
        "{}{}?{query}&X-Amz-Signature={signature}",
        request.origin, request.path
    )
}

#[cfg(test)]
mod tests;
