//! botocore's flexible checksums (`botocore/httpchecksum.py`): the request
//! checksum a call computes, the response checksum it validates, and the
//! feature ids they register, for operations with `httpChecksum` or
//! `httpChecksumRequired`.
//!
//! Built but switched off: admission refuses both traits, and the
//! orchestrator runs these phases only for a client whose
//! `http_checksum` switch a test has set, until a later change sets it
//! for every client. The phases, in botocore's order:
//!
//! 1. [`default_validation_mode`], `_handle_request_validation_mode_member`
//!    (`before-parameter-build`, after the idempotency tokens);
//! 2. [`resolve`], `resolve_checksum_context`, after serialization and
//!    before the `before-call` stage, with the feature ids the
//!    `User-Agent` sends;
//! 3. [`CallChecksums::apply`], `apply_request_checksum`, after that stage;
//! 4. [`CallChecksums::validate`], `handle_checksum_body`, on every
//!    response (an error's too) before the `before-parse` stage.
//!
//! What native leaves to Python, as `Unsupported::Trait("httpChecksum")`,
//! before credentials load: the algorithms botocore takes from the CRT or
//! `hashlib` that this module does not compute (SHA-512 and the xxHash
//! family), whether chosen for the request or listed as usable for the
//! response; a checksum sent in an `aws-chunked` trailer (a streaming
//! input over https), which needs the signer's
//! `STREAMING-UNSIGNED-PAYLOAD-TRAILER` and a chunked transport; and a
//! request feature id the `User-Agent` cannot yet send (every algorithm's
//! but CRC-64/NVME's).
use crate::admission::Unsupported;
use crate::error::CallError;
use crate::hooks::HookError;
use crate::http::{Headers, HttpRequest, HttpResponse};
use cli_core::client_config::user_agent::{Feature, Features};
use cli_core::client_config::{ChecksumMode, ClientSettings};
use model::schema::{OperationSchema, ShapeType};
use model::value::{Map, Value};

mod digest;

/// `DEFAULT_CHECKSUM_ALGORITHM`.
pub const DEFAULT_ALGORITHM: &str = "CRC64NVME";

/// `_SUPPORTED_CHECKSUM_ALGORITHMS`: what botocore accepts as a request's
/// algorithm and validates a response with.
const SUPPORTED: [&str; 9] = [
    "crc64nvme",
    "crc32c",
    "crc32",
    "sha1",
    "sha256",
    "sha512",
    "xxhash64",
    "xxhash3",
    "xxhash128",
];

/// `_ALGORITHMS_PRIORITY_LIST`: the order a response's checksums are
/// looked for in.
const PRIORITY: [&str; 9] = [
    "xxhash128",
    "xxhash3",
    "crc64nvme",
    "xxhash64",
    "crc32c",
    "crc32",
    "sha1",
    "sha256",
    "sha512",
];

/// An algorithm of [`SUPPORTED`] this module computes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    Crc64Nvme,
    Crc32c,
    Crc32,
    Sha1,
    Sha256,
}

impl Algorithm {
    pub const ALL: [Self; 5] = [
        Self::Crc64Nvme,
        Self::Crc32c,
        Self::Crc32,
        Self::Sha1,
        Self::Sha256,
    ];
    /// The algorithm of the lowercase name botocore keys `_CHECKSUM_CLS` by.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|algorithm| algorithm.name() == name)
    }
    /// Its lowercase name, as `x-amz-checksum-<name>` spells it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Crc64Nvme => "crc64nvme",
            Self::Crc32c => "crc32c",
            Self::Crc32 => "crc32",
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }
    /// The checksum class's `digest()` of `bytes`: a CRC big-endian.
    pub fn digest(self, bytes: &[u8]) -> Vec<u8> {
        match self {
            Self::Crc64Nvme => digest::crc64nvme(bytes).to_be_bytes().to_vec(),
            Self::Crc32c => digest::crc32c(bytes).to_be_bytes().to_vec(),
            Self::Crc32 => digest::crc32(bytes).to_be_bytes().to_vec(),
            Self::Sha1 => digest::sha1(bytes).to_vec(),
            Self::Sha256 => digest::sha256(bytes),
        }
    }
    /// `b64digest()`.
    pub fn b64digest(self, bytes: &[u8]) -> String {
        pytext::base64::encode(&self.digest(bytes))
    }
    /// `x-amz-checksum-<name>`.
    pub fn header(self) -> String {
        format!("x-amz-checksum-{}", self.name())
    }
}

/// The checksum a request sends in a header (`request_algorithm` with
/// `"in": "header"`), and the algorithm member's header the default
/// algorithm fills (`request_algorithm_header`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestChecksum {
    pub algorithm: Algorithm,
    /// The name of the header the operation's `requestAlgorithmMember`
    /// serializes to, which is set to [`DEFAULT_ALGORITHM`].
    pub algorithm_header: Option<String>,
}

/// A call's `context["checksum"]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallChecksums {
    pub request: Option<RequestChecksum>,
    /// `response_algorithms`: the usable ones, in priority order.
    pub response: Vec<Algorithm>,
}

/// The refusal of a call that needs what this module leaves to Python.
fn refused() -> CallError {
    CallError::Unsupported(Unsupported::Trait("httpChecksum".to_owned()))
}

/// `_handle_request_validation_mode_member`: under `when_supported`
/// response validation, the operation's `requestValidationModeMember`
/// defaults to `ENABLED`.
pub fn default_validation_mode(
    operation: OperationSchema<'_>,
    settings: &ClientSettings,
    input: &mut Value,
) {
    let member = operation
        .http_checksum()
        .and_then(|checksum| checksum.request_validation_mode_member.as_deref());
    let Some(member) = member else {
        return;
    };
    if settings.response_checksum.value != ChecksumMode::WhenSupported {
        return;
    }
    if *input == Value::Null {
        *input = Value::Map(Map::new());
    }
    if let Value::Map(params) = input
        && !params.contains_key(member)
    {
        params.insert(member.to_owned(), Value::String("ENABLED".to_owned()));
    }
}

/// `resolve_checksum_context`: the request's algorithm, the response's
/// usable algorithms and the request feature ids, from the input as the
/// hooks left it, the serialized `headers` and the endpoint `url`.
///
/// # Errors
/// botocore's `FlexibleChecksumError` for an algorithm it does not
/// support, and a refusal of what this module leaves to Python.
pub fn resolve(
    operation: OperationSchema<'_>,
    settings: &ClientSettings,
    input: &Value,
    headers: &Headers,
    url: &str,
) -> Result<(CallChecksums, Features), CallError> {
    let request = request_algorithm(operation, settings, input, headers, url)?;
    let response = response_algorithms(operation, input)?;
    let features = features(headers, request.as_ref())?;
    Ok((CallChecksums { request, response }, features))
}

/// `utils.CHECKSUM_HEADER_PATTERN`: `^X-Amz-Checksum-([a-z0-9]*)$`, any
/// case; the suffix when `name` matches.
fn checksum_header(name: &str) -> Option<&str> {
    const PREFIX: &str = "x-amz-checksum-";
    let head = name.get(..PREFIX.len())?;
    let rest = &name[PREFIX.len()..];
    (head.eq_ignore_ascii_case(PREFIX) && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then_some(rest)
}

/// `resolve_request_checksum_algorithm`.
fn request_algorithm(
    operation: OperationSchema<'_>,
    settings: &ClientSettings,
    input: &Value,
    headers: &Headers,
    url: &str,
) -> Result<Option<RequestChecksum>, CallError> {
    // A checksum header the input set skips the calculation.
    if headers
        .iter()
        .any(|(name, _)| checksum_header(name).is_some())
    {
        return Ok(None);
    }
    let http_checksum = operation.http_checksum();
    let required = operation.http_checksum_required()
        || http_checksum.is_some_and(|checksum| checksum.request_checksum_required);
    let member = http_checksum.and_then(|checksum| checksum.request_algorithm_member.as_deref());
    let chosen = member.and_then(|member| input.get(member));
    let request = if let Some(chosen) = chosen {
        // `params[algorithm_member].lower()`.
        let name = chosen.as_str().ok_or_else(refused)?.to_lowercase();
        if !SUPPORTED.contains(&name.as_str()) {
            return Err(CallError::Hook(HookError::Failed(format!(
                "Unsupported checksum algorithm: {name}"
            ))));
        }
        RequestChecksum {
            algorithm: Algorithm::named(&name).ok_or_else(refused)?,
            algorithm_header: None,
        }
    } else if required
        || (member.is_some() && settings.request_checksum.value == ChecksumMode::WhenSupported)
    {
        RequestChecksum {
            algorithm: Algorithm::Crc64Nvme,
            algorithm_header: member.and_then(|member| algorithm_member_header(operation, member)),
        }
    } else {
        return Ok(None);
    };
    // The trailer botocore uses for a streaming input over https.
    let https = url
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"));
    if https && streaming_input(operation) {
        return Err(refused());
    }
    Ok(Some(request))
}

/// `_get_request_algorithm_member_header`: the member's serialized name
/// (its `locationName`), wherever the member is bound.
fn algorithm_member_header(operation: OperationSchema<'_>, member: &str) -> Option<String> {
    let input = operation.input()?;
    if input.shape_type() != ShapeType::Structure {
        return None;
    }
    input.member(member)?.location_name().map(str::to_owned)
}

/// `OperationModel.has_streaming_input`: a blob payload.
fn streaming_input(operation: OperationSchema<'_>) -> bool {
    operation.input().is_some_and(|input| {
        input
            .payload()
            .and_then(|payload| input.member(payload))
            .is_some_and(|member| member.shape_type() == ShapeType::Blob)
    })
}

/// `resolve_response_checksum_algorithms`: when the input sets the
/// validation mode member, the model's `responseAlgorithms` botocore
/// supports, in priority order.
fn response_algorithms(
    operation: OperationSchema<'_>,
    input: &Value,
) -> Result<Vec<Algorithm>, CallError> {
    let Some(http_checksum) = operation.http_checksum() else {
        return Ok(Vec::new());
    };
    let mode = http_checksum.request_validation_mode_member.as_deref();
    if mode.is_none_or(|member| input.get(member).is_none()) {
        return Ok(Vec::new());
    }
    let listed: Vec<String> = (http_checksum.response_algorithms.iter())
        .map(|name| name.to_lowercase())
        .collect();
    let usable = PRIORITY
        .into_iter()
        .filter(|name| listed.iter().any(|listed| listed == name));
    usable
        .map(|name| Algorithm::named(name).ok_or_else(refused))
        .collect()
}

/// `_register_checksum_feature_ids`: each checksum header's algorithm,
/// else the resolved request algorithm's.
fn features(headers: &Headers, request: Option<&RequestChecksum>) -> Result<Features, CallError> {
    let mut features = Features::default();
    let mut found = false;
    for (name, _) in headers.iter() {
        let Some(suffix) = checksum_header(name) else {
            continue;
        };
        found = true;
        let suffix = suffix.to_ascii_uppercase();
        if !matches!(suffix.as_str(), "ALGORITHM" | "MODE" | "TYPE") {
            register(&mut features, &suffix)?;
        }
    }
    if let (false, Some(request)) = (found, request) {
        register(
            &mut features,
            &request.algorithm.name().to_ascii_uppercase(),
        )?;
    }
    Ok(features)
}

/// `_register_checksum_algorithm_feature_id`: `FLEXIBLE_CHECKSUMS_REQ_`
/// and the name (`CRC64NVME` as `CRC64`), which registers only a name
/// `_USERAGENT_FEATURE_MAPPINGS` has.
fn register(features: &mut Features, name: &str) -> Result<(), CallError> {
    match name {
        "CRC64NVME" | "CRC64" => features.insert(Feature::FlexibleChecksumsReqCrc64),
        // Feature ids the `User-Agent` does not have yet.
        "CRC32" | "CRC32C" | "SHA1" | "SHA256" | "MD5" | "SHA512" | "XXHASH3" | "XXHASH64"
        | "XXHASH128" => return Err(refused()),
        _ => {}
    }
    Ok(())
}

impl CallChecksums {
    /// `apply_request_checksum` for a header checksum: the body's digest,
    /// unless the header is there, then the algorithm member's header.
    pub fn apply(&self, request: &mut HttpRequest) {
        let Some(checksum) = &self.request else {
            return;
        };
        let name = checksum.algorithm.header();
        if !request.headers.contains(&name) {
            let digest = checksum.algorithm.b64digest(&request.body);
            request.headers.set(&name, digest);
        }
        if let Some(header) = &checksum.algorithm_header {
            request.headers.set(header, DEFAULT_ALGORITHM);
        }
    }

    /// `handle_checksum_body` for a body read whole: the first usable
    /// algorithm whose header the response has, a composite (`-`) value
    /// skipped, must match the body's digest.
    ///
    /// # Errors
    /// `FlexibleChecksumError` on a mismatch, and `binascii.Error` for a
    /// header `b64decode` cannot read, each as its text.
    pub fn validate(&self, response: &HttpResponse) -> Result<(), HookError> {
        for algorithm in &self.response {
            let Some(expected) = response.headers.joined(&algorithm.header()) else {
                continue;
            };
            if expected.contains('-') {
                continue;
            }
            let decoded = pytext::base64::decode(&expected).map_err(HookError::Failed)?;
            if algorithm.digest(&response.body) != decoded {
                return Err(HookError::Failed(format!(
                    "Expected checksum {expected} did not match calculated checksum: {}",
                    algorithm.b64digest(&response.body)
                )));
            }
            return Ok(());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
