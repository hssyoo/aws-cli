//! SigV4 verification: whether a captured request's `Authorization`
//! signature is the one its own bytes and the run's fake secret key give,
//! computed as botocore's `SigV4Auth` does (`botocore/auth.py`).
//!
//! The canonical request is the method; the path, encoded again except
//! for S3 (`quote(path, safe='/~')`); the query's `&`-separated pairs sorted
//! by name then value; each signed header, lowercase, with its values'
//! whitespace collapsed and repeats joined by `,`; the signed header list;
//! and `X-Amz-Content-SHA256`, or the body's SHA-256. The string to sign
//! takes the request's `X-Amz-Date` and the credential scope, and the key
//! is derived from the secret, the scope's date, region and service.
//!
//! Only `AWS4-HMAC-SHA256` header signatures are verified. Another scheme
//! (SigV4a), a query signature or a run without a secret key is
//! unverified.

use sha2::{Digest, Sha256};

use super::capture::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Bad,
    Unverified,
}

impl Verdict {
    pub fn placeholder(self) -> &'static str {
        match self {
            Self::Ok => "<sig:ok>",
            Self::Bad => "<sig:bad>",
            Self::Unverified => "<sig:unverified>",
        }
    }
}

/// The parts of an `Authorization` header a verification reads.
#[derive(Debug, PartialEq, Eq)]
struct Authorization<'a> {
    /// The scope after the access key: date, region, service, terminator.
    scope: &'a str,
    date: &'a str,
    region: &'a str,
    service: &'a str,
    signed: &'a str,
    signature: &'a str,
}

fn authorization(value: &str) -> Option<Authorization<'_>> {
    let rest = value.strip_prefix("AWS4-HMAC-SHA256 ")?;
    let field = |name: &str| {
        rest.split(',')
            .map(str::trim)
            .find_map(|part| part.strip_prefix(name)?.strip_prefix('='))
    };
    let credential = field("Credential")?;
    let scope = &credential[credential.find('/')? + 1..];
    let mut parts = scope.split('/');
    let (date, region, service) = (parts.next()?, parts.next()?, parts.next()?);
    (parts.next()? == "aws4_request" && parts.next().is_none()).then_some(())?;
    Some(Authorization {
        scope,
        date,
        region,
        service,
        signed: field("SignedHeaders")?,
        signature: field("Signature")?,
    })
}

/// Whether `request`'s signature is the one `secret` gives.
pub fn verify(request: &Request, secret: Option<&str>) -> Verdict {
    let header = |name: &str| {
        let matching = request
            .headers
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name));
        matching
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
    };
    let authorizations = header("authorization");
    let (Some(secret), [value]) = (secret, authorizations.as_slice()) else {
        return Verdict::Unverified;
    };
    let Some(auth) = authorization(value) else {
        return Verdict::Unverified;
    };
    let Some(canonical) = canonical_request(request, &auth, &header) else {
        return Verdict::Bad;
    };
    let Some(&amz_date) = header("x-amz-date").first() else {
        return Verdict::Bad;
    };
    let digest = hex(&Sha256::digest(canonical.as_bytes()));
    let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{}\n{digest}", auth.scope);
    let mut key = hmac(format!("AWS4{secret}").as_bytes(), auth.date.as_bytes());
    for part in [auth.region, auth.service, "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    if hex(&hmac(&key, to_sign.as_bytes())) == auth.signature {
        Verdict::Ok
    } else {
        Verdict::Bad
    }
}

/// The canonical request, or none when a signed header is absent.
fn canonical_request<'a>(
    request: &Request,
    auth: &Authorization<'_>,
    header: &dyn Fn(&str) -> Vec<&'a str>,
) -> Option<String> {
    let target = request.target.as_str();
    let origin = ["http://", "https://"]
        .iter()
        .find_map(|scheme| target.strip_prefix(scheme))
        .map_or(target, |rest| rest.find('/').map_or("", |at| &rest[at..]));
    let (path, query) = origin.split_once('?').unwrap_or((origin, ""));
    let path = if path.is_empty() { "/" } else { path };
    let path = if auth.service == "s3" {
        path.to_owned()
    } else {
        encode(path)
    };
    let mut lines = vec![
        request.method.to_ascii_uppercase(),
        path,
        canonical_query(query),
    ];
    for name in auth.signed.split(';') {
        let values = header(name);
        if values.is_empty() {
            return None;
        }
        let trimmed: Vec<String> = values
            .iter()
            .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        lines.push(format!("{name}:{}", trimmed.join(",")));
    }
    lines.push(String::new());
    lines.push(auth.signed.to_owned());
    let payload = header("x-amz-content-sha256").first().map_or_else(
        || hex(&Sha256::digest(&request.body)),
        |value| (*value).to_owned(),
    );
    lines.push(payload);
    Some(lines.join("\n"))
}

/// `quote(path, safe='/~')`: every byte but the unreserved ones and `/`
/// percent-encoded, `%` included.
fn encode(path: &str) -> String {
    let mut encoded = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn canonical_query(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    let mut pairs: Vec<(&str, &str)> = query
        .split('&')
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
        .collect();
    pairs.sort_unstable();
    let joined: Vec<String> = pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    joined.join("&")
}

/// HMAC-SHA256 (RFC 2104).
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.map(|key_byte| key_byte ^ byte);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
