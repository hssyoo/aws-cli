//! The checksums against known answers, botocore's own unit-test data, and
//! the Python route's captures of the `zchecksum` fake rows of
//! `tests/diff/corpus/fakes.jsonl` (`cargo xtask diff --a candidate:python
//! --b candidate:python --filter zchecksum`): each native call below sends
//! what that route sent for its row, in order, but `Host`,
//! `Accept-Encoding` and the signature, and fails or returns as it did.
use super::*;
use crate::call::{Client, ClientParts, Output};
use crate::credentials::{CredentialError, CredentialSource, SigningCredentials};
use crate::endpoint::{EndpointConfig, EndpointData, EndpointInputs};
use crate::testing::{FixedRuntime, InMemoryTransport};
use crate::useragent::{SessionExtras, UserAgentContext};
use cli_core::client_config::user_agent::{Host, Versions};
use model::schema::OperationSchema;
use model::shapes::Model;
use std::path::PathBuf;
use std::sync::Arc;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The fake rows' request body.
const BODY: &[u8] = br#"{"Data": "hello world"}"#;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn shas_match_known_answers() {
    let sha1 = [
        (&b""[..], "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
        (b"abc", "a9993e364706816aba3e25717850c26c9cd0d89d"),
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
        ),
    ];
    for (input, digest) in sha1 {
        assert_eq!(hex(&Algorithm::Sha1.digest(input)), digest);
    }
    let million = vec![b'a'; 1_000_000];
    assert_eq!(
        hex(&Algorithm::Sha1.digest(&million)),
        "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
    );
    assert_eq!(
        hex(&Algorithm::Sha256.digest(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn crcs_match_known_answers() {
    let check = b"123456789";
    assert_eq!(hex(&Algorithm::Crc32.digest(check)), "cbf43926");
    assert_eq!(hex(&Algorithm::Crc32c.digest(check)), "e3069283");
    assert_eq!(hex(&Algorithm::Crc64Nvme.digest(check)), "ae8b14860a799888");
    // Every block-boundary length agrees between the CRCs' tables and a
    // bitwise reference.
    let bitwise = |poly: u64, width: u32, bytes: &[u8]| {
        let mask = if width == 64 {
            u64::MAX
        } else {
            (1 << width) - 1
        };
        let mut crc = mask;
        for byte in bytes {
            crc ^= u64::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ poly
                } else {
                    crc >> 1
                };
            }
        }
        !crc & mask
    };
    let bytes: Vec<u8> = (0..=255).collect();
    for len in [0, 1, 55, 56, 63, 64, 65, 256] {
        let part = &bytes[..len];
        assert_eq!(
            u64::from(digest::crc32(part)),
            bitwise(0xEDB8_8320, 32, part)
        );
        assert_eq!(
            u64::from(digest::crc32c(part)),
            bitwise(0x82F6_3B78, 32, part)
        );
        assert_eq!(
            digest::crc64nvme(part),
            bitwise(0x9A6C_9329_AC4B_C9B5, 64, part)
        );
    }
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `_CHECKSUM_DIGEST_CASES` of botocore's `test_httpchecksum.py`, read as
/// data: each class this module stands in for gives its digest of
/// `hello world`.
#[test]
fn digests_match_botocore_s_cases() -> TestResult {
    let path = repo().join("aws-cli/tests/unit/botocore/test_httpchecksum.py");
    let source = std::fs::read_to_string(path)?;
    let start = source
        .find("_CHECKSUM_DIGEST_CASES = [")
        .ok_or("no cases")?;
    let cases = &source[start..source[start..].find("\n]").ok_or("no end")? + start];
    let classes = [
        ("Crc32Checksum", Algorithm::Crc32),
        ("CrtCrc32Checksum", Algorithm::Crc32),
        ("CrtCrc32cChecksum", Algorithm::Crc32c),
        ("CrtCrc64NvmeChecksum", Algorithm::Crc64Nvme),
        ("Sha1Checksum", Algorithm::Sha1),
        ("Sha256Checksum", Algorithm::Sha256),
    ];
    for (class, algorithm) in classes {
        let line = (cases.lines())
            .find(|line| line.trim_start().starts_with(&format!("({class}, ")))
            .ok_or(class)?;
        let expected = line.split('"').nth(1).ok_or(class)?;
        assert_eq!(algorithm.b64digest(b"hello world"), expected, "{class}");
    }
    Ok(())
}

/// The digests the Python route sent for the rows' bodies, for every
/// algorithm, whether or not native sends that row.
#[test]
fn digests_match_the_python_route_s_headers() {
    let gone = br#"{"message": "gone"}"#;
    let sent: [(Algorithm, &[u8], &str); 8] = [
        (Algorithm::Crc64Nvme, BODY, "eTiKPoBNMWQ="),
        (Algorithm::Crc32, BODY, "IT5H/w=="),
        (Algorithm::Crc32c, BODY, "ZP46yg=="),
        (Algorithm::Sha1, BODY, "6ihXUlsIkDgN9Ti8ZGjnFaoCNBM="),
        (
            Algorithm::Sha256,
            BODY,
            "+uXLgx7wbi4En5HP7xy2ykphZlz/D0P+CjXolMw75DI=",
        ),
        (Algorithm::Crc32, gone, "HnDQQA=="),
        (
            Algorithm::Sha1,
            br#"{"algo": "SHA1"}"#,
            "R1P1zgftRvXCk7m+LVZ7QYu0NqE=",
        ),
        (Algorithm::Crc32, b"{}", "o6a/Qw=="),
    ];
    for (algorithm, body, digest) in sent {
        assert_eq!(algorithm.b64digest(body), digest, "{algorithm:?}");
    }
}

#[test]
fn the_header_pattern_is_botocore_s() {
    for name in [
        "x-amz-checksum-crc32",
        "X-AMZ-CHECKSUM-SHA1",
        "x-amz-checksum-",
        "x-Amz-checksum-mode9",
    ] {
        assert!(checksum_header(name).is_some(), "{name}");
    }
    for name in [
        "x-amz-checksum",
        "x-amz-checksum-crc-32",
        "x-amz-sdk-checksum-algorithm",
        "x-amz-checksum-é",
    ] {
        assert!(checksum_header(name).is_none(), "{name}");
    }
}

#[test]
fn feature_ids_follow_the_headers_else_the_algorithm() -> TestResult {
    let crc64 = Features::from_iter([Feature::FlexibleChecksumsReqCrc64]);
    let default = RequestChecksum {
        algorithm: Algorithm::Crc64Nvme,
        algorithm_header: None,
    };
    assert_eq!(features(&Headers::new(), Some(&default))?, crc64);
    assert_eq!(features(&Headers::new(), None)?, Features::default());
    let headers = |name| Headers::from_pairs([(name, "v")]);
    assert_eq!(features(&headers("X-Amz-Checksum-CRC64"), None)?, crc64);
    // A mode, type or algorithm header, or an unknown name, registers
    // nothing, and stops the fallback to the algorithm.
    for name in [
        "x-amz-checksum-mode",
        "x-amz-checksum-type",
        "x-amz-Checksum-Algorithm",
        "x-amz-checksum-foo",
    ] {
        assert_eq!(
            features(&headers(name), Some(&default))?,
            Features::default(),
            "{name}"
        );
    }
    for name in [
        "x-amz-checksum-crc32",
        "x-amz-checksum-sha256",
        "x-amz-checksum-xxhash3",
    ] {
        assert!(
            matches!(
                features(&headers(name), None),
                Err(CallError::Unsupported(_))
            ),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn validation_mode_defaults_only_when_supported() -> TestResult {
    let model = zchecksum()?;
    let operation = op(&model, "GetValidated")?;
    let mut input = Value::Null;
    default_validation_mode(operation, &settings(), &mut input);
    assert_eq!(
        input.get("ChecksumMode").and_then(Value::as_str),
        Some("ENABLED")
    );
    let mut given = params(&[("ChecksumMode", "OTHER")]);
    default_validation_mode(operation, &settings(), &mut given);
    assert_eq!(
        given.get("ChecksumMode").and_then(Value::as_str),
        Some("OTHER")
    );
    let mut required = settings();
    required.response_checksum.value = ChecksumMode::WhenRequired;
    let mut untouched = params(&[]);
    default_validation_mode(operation, &required, &mut untouched);
    assert_eq!(untouched, params(&[]));
    let put = op(&model, "PutRequired")?;
    default_validation_mode(put, &settings(), &mut untouched);
    assert_eq!(untouched, params(&[]));
    Ok(())
}

/// `test_request_checksum_algorithm_model_default_streaming`'s rule: a
/// streaming input over https would take a trailer, which native leaves to
/// Python; over http it is a header.
#[test]
fn a_trailer_is_refused_and_http_keeps_the_header() -> TestResult {
    let mut doc: serde_json::Value = serde_json::from_slice(&std::fs::read(model_path())?)?;
    doc["shapes"]["UploadRequest"]["members"]["Body"] = serde_json::json!({"shape": "Blob"});
    doc["shapes"]["UploadRequest"]["payload"] = "Body".into();
    doc["shapes"]["Blob"] = serde_json::json!({"type": "blob"});
    let model = Model::from_json(&serde_json::to_vec(&doc)?)?;
    let operation = op(&model, "PutRequired")?;
    let input = params(&[("Id", "a")]);
    let resolved = |url| resolve(operation, &settings(), &input, &Headers::new(), url);
    assert!(matches!(
        resolved("https://h/"),
        Err(CallError::Unsupported(_))
    ));
    let (checksums, _) = resolved("http://h/")?;
    assert_eq!(
        checksums.request.map(|r| r.algorithm),
        Some(Algorithm::Crc64Nvme)
    );
    Ok(())
}

#[test]
fn response_algorithms_go_in_priority_order() -> TestResult {
    let model = zchecksum()?;
    let operation = op(&model, "GetValidated")?;
    let (checksums, _) = resolve(
        operation,
        &settings(),
        &params(&[("ChecksumMode", "ENABLED")]),
        &Headers::new(),
        "https://h/",
    )?;
    use Algorithm::*;
    // MD5 is listed but not supported, so it is not usable.
    assert_eq!(checksums.response, [Crc64Nvme, Crc32c, Crc32, Sha1, Sha256]);
    let (none, _) = resolve(
        operation,
        &settings(),
        &params(&[]),
        &Headers::new(),
        "https://h/",
    )?;
    assert!(none.response.is_empty());
    let wide = op(&model, "GetWide")?;
    let refused = resolve(
        wide,
        &settings(),
        &params(&[("ChecksumMode", "E")]),
        &Headers::new(),
        "https://h/",
    );
    assert!(matches!(refused, Err(CallError::Unsupported(_))));
    Ok(())
}

// The call-level rows.

fn op<'m>(model: &'m Model, name: &str) -> TestResult<OperationSchema<'m>> {
    Ok(model.schema().service().operation(name).ok_or(name)?)
}

fn model_path() -> PathBuf {
    repo().join("tests/models/zchecksum/2020-01-01/service-2.json")
}

fn zchecksum() -> TestResult<Model> {
    Ok(Model::from_json(&std::fs::read(model_path())?)?)
}

fn settings() -> ClientSettings {
    ClientSettings::new("zchecksum.op")
}

fn params(pairs: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (name, value) in pairs {
        map.insert((*name).to_owned(), Value::String((*value).to_owned()));
    }
    Value::Map(map)
}

#[derive(Debug)]
struct Keys;

impl CredentialSource for Keys {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        Ok(Some(SigningCredentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "SECRET".into(),
            session_token: None,
            account_id: None,
        }))
    }
}

/// A zchecksum client in us-west-2 with checksums switched on as `on`
/// says, and its transport.
fn client(settings: ClientSettings, on: bool) -> TestResult<(Client, Arc<InMemoryTransport>)> {
    let data = |path: &str| std::fs::read(repo().join(path));
    let endpoints = EndpointData::from_documents(
        data("tests/models/zchecksum/2020-01-01/endpoint-rule-set-1.json")?,
        data("aws-cli/awscli/botocore/data/partitions.json")?,
        None,
    )?;
    let transport = Arc::new(InMemoryTransport::new());
    let now = timestamp::Timestamp::from_unix_seconds(1_440_938_160).ok_or("clock")?;
    let mut client = Client::with_protocol(
        ClientParts {
            service: "zchecksum".into(),
            model: Arc::new(zchecksum()?),
            endpoints,
            endpoint: EndpointConfig::new(&EndpointInputs {
                region: Some("us-west-2"),
                signing: true,
                ..EndpointInputs::default()
            }),
            settings,
            credentials: Arc::new(Keys),
            transport: transport.clone(),
            runtime: Arc::new(FixedRuntime::new(now, 0.0)),
            customizations: &[],
            parse: crate::protocol::ParseHooks::botocore(),
            user_agent: UserAgentContext {
                host: Host::default(),
                distribution: None,
                versions: Versions {
                    cli: "2.0.0",
                    crt: "0.1.0",
                    rust: "1.96.0",
                },
                extras: SessionExtras {
                    installer: "other",
                    prompt: "off",
                    command: "zchecksum.op".into(),
                },
            },
        },
        &crate::protocol::rest_json::REST_JSON_1,
    )?;
    client.http_checksum = on;
    Ok((client, transport))
}

/// What a request sent: its headers but the signer's and the unsigned
/// pair, `User-Agent` as its `m/` codes in declaration order, without the
/// Python route's credential provider's (`g`).
fn sent(request: &HttpRequest) -> Vec<(String, String)> {
    let skipped = [
        "X-Amz-Date",
        "Authorization",
        "amz-sdk-invocation-id",
        "amz-sdk-request",
    ];
    (request.headers.iter())
        .filter(|(name, _)| !skipped.contains(name))
        .map(|(name, value)| {
            let value = match name {
                "User-Agent" => value.split(' ').find_map(|part| part.strip_prefix("m/")),
                _ => Some(value),
            };
            (name.to_owned(), value.unwrap_or_default().to_owned())
        })
        .collect()
}

const CT: (&str, &str) = ("Content-Type", "application/json");
const CRC64: (&str, &str) = ("x-amz-checksum-crc64nvme", "eTiKPoBNMWQ=");
const ALGORITHM: (&str, &str) = ("x-amz-sdk-checksum-algorithm", "CRC64NVME");
const LENGTH: (&str, &str) = ("Content-Length", "23");
const MODE: (&str, &str) = ("x-amz-checksum-mode", "ENABLED");
const UA_MODE: (&str, &str) = ("User-Agent", "E,Z,b");
const MISMATCH: &str = "Expected checksum AAAAAA== did not match calculated checksum: IT5H/w==";

type Sent = (
    &'static str,
    &'static [(&'static str, &'static str)],
    Option<&'static str>,
);

/// The rows native sends: each one's headers as the Python route sent
/// them, and its error's text.
const SENT: &[Sent] = &[
    (
        "put-required",
        &[CT, ("User-Agent", "E,W,Z,b"), CRC64, ALGORITHM, LENGTH],
        None,
    ),
    (
        "put-required-crc64nvme",
        &[ALGORITHM, CT, ("User-Agent", "E,W,Z,b"), CRC64, LENGTH],
        None,
    ),
    (
        "put-required-when-required",
        &[CT, ("User-Agent", "E,W,a,b"), CRC64, ALGORITHM, LENGTH],
        None,
    ),
    (
        "put-optional",
        &[CT, ("User-Agent", "E,W,Z,b"), CRC64, ALGORITHM, LENGTH],
        None,
    ),
    (
        "put-optional-when-required",
        &[CT, ("User-Agent", "E,a,b"), LENGTH],
        None,
    ),
    (
        "put-legacy",
        &[CT, ("User-Agent", "E,W,Z,b"), CRC64, LENGTH],
        None,
    ),
    // The body member's `locationName` names the header the default fills.
    (
        "put-body-algorithm",
        &[
            CT,
            ("User-Agent", "E,W,Z,b"),
            ("x-amz-checksum-crc64nvme", "wlxTeeB65/4="),
            ("algo", "CRC64NVME"),
            ("Content-Length", "13"),
        ],
        None,
    ),
    ("get-validated", &[MODE, UA_MODE], None),
    ("get-validated-mismatch", &[MODE, UA_MODE], Some(MISMATCH)),
    ("get-validated-priority", &[MODE, UA_MODE], Some(MISMATCH)),
    ("get-validated-composite", &[MODE, UA_MODE], None),
    (
        "get-validated-bad-base64",
        &[MODE, UA_MODE],
        Some(
            "Invalid base64-encoded string: number of data characters (5) cannot be 1 more \
             than a multiple of 4",
        ),
    ),
    ("get-validated-none", &[MODE, UA_MODE], None),
    // An error response is validated before it is parsed.
    (
        "get-validated-error",
        &[MODE, UA_MODE],
        Some("Expected checksum AAAAAA== did not match calculated checksum: HnDQQA=="),
    ),
    (
        "get-validated-when-required",
        &[("User-Agent", "E,Z,c")],
        None,
    ),
    (
        "get-validated-explicit-mode",
        &[MODE, ("User-Agent", "E,Z,c")],
        Some(MISMATCH),
    ),
];

/// The rows native leaves to Python before a request: a feature id the
/// `User-Agent` lacks, or an algorithm this module does not compute.
const REFUSED: &[&str] = &[
    "put-required-crc32",
    "put-required-crc32c",
    "put-required-sha1",
    "put-required-sha256",
    "put-required-lowercase",
    "put-required-own-checksum",
    "put-required-own-sha256",
    "put-optional-chosen-when-required",
    "put-legacy-own-checksum",
    "put-body-algorithm-chosen",
    "get-wide",
];

/// The zchecksum rows of the corpus, by id without the prefix.
fn rows() -> TestResult<Vec<(String, serde_json::Value)>> {
    let corpus = std::fs::read_to_string(repo().join("tests/diff/corpus/fakes.jsonl"))?;
    let mut rows = Vec::new();
    for line in corpus.lines() {
        let row: serde_json::Value = serde_json::from_str(line)?;
        let id = row["id"].as_str().unwrap_or_default();
        if let Some(id) = id.strip_prefix("fakes-zchecksum-") {
            rows.push((id.to_owned(), row));
        }
    }
    Ok(rows)
}

/// A row's operation, input, settings and first reply, as the CLI builds
/// them from its arguments and the harness gives them.
fn call_of(row: &serde_json::Value) -> TestResult<(String, Value, ClientSettings, HttpResponse)> {
    let source = row["source"].as_str().unwrap_or_default();
    let operation = source.rsplit('#').next().unwrap_or_default().to_owned();
    let members = [
        ("--id", "Id"),
        ("--data", "Data"),
        ("--checksum-algorithm", "ChecksumAlgorithm"),
        ("--checksum-crc32", "ChecksumCRC32"),
        ("--checksum-sha256", "ChecksumSHA256"),
        ("--checksum-mode", "ChecksumMode"),
        ("--algorithm", "Algorithm"),
    ];
    let argv: Vec<&str> = (row["argv"].as_array().ok_or("argv")?.iter())
        .filter_map(serde_json::Value::as_str)
        .collect();
    let mut input = Map::new();
    for pair in argv[2..].chunks(2) {
        let (flag, value) = (pair[0], pair.get(1).ok_or("a flag's value")?);
        let member = members.iter().find(|(f, _)| *f == flag).ok_or(flag)?.1;
        input.insert(member.to_owned(), Value::String((*value).to_owned()));
    }
    let mut settings = settings();
    let env = |name: &str| row["env"][name].as_str();
    if env("AWS_REQUEST_CHECKSUM_CALCULATION") == Some("when_required") {
        settings.request_checksum.value = ChecksumMode::WhenRequired;
    }
    if env("AWS_RESPONSE_CHECKSUM_VALIDATION") == Some("when_required") {
        settings.response_checksum.value = ChecksumMode::WhenRequired;
    }
    let reply = &row["responses"][0];
    let headers = (reply["headers"].as_array().ok_or("headers")?.iter())
        .filter_map(|pair| Some((pair[0].as_str()?, pair[1].as_str()?)));
    let response = HttpResponse {
        status: u16::try_from(reply["status"].as_u64().unwrap_or(200))?,
        headers: Headers::from_pairs(headers),
        body: reply["body"]
            .as_str()
            .unwrap_or_default()
            .as_bytes()
            .to_vec(),
    };
    Ok((operation, Value::Map(input), settings, response))
}

/// Whether `outcome` is admission's refusal of the trait `key`.
fn refused_as<T: std::fmt::Debug>(outcome: &Result<T, CallError>, key: &str) -> bool {
    matches!(outcome, Err(CallError::Unsupported(Unsupported::Trait(found))) if found == key)
}

/// The outcome of row `id`, which sent `requests`, as the Python route's.
fn check(id: &str, requests: &[HttpRequest], outcome: Result<Output, CallError>) {
    let text = |outcome: &Result<Output, CallError>| match outcome {
        Err(error) => Some(error.to_string()),
        Ok(_) => None,
    };
    let sent: Vec<_> = requests.iter().map(sent).collect();
    if let Some((_, expected, error)) = SENT.iter().find(|sent| sent.0 == id) {
        let expected: Vec<(String, String)> = (expected.iter())
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        assert_eq!(
            (sent, text(&outcome).as_deref()),
            (vec![expected], *error),
            "{id}"
        );
    } else if REFUSED.contains(&id) {
        assert!(
            sent.is_empty() && refused_as(&outcome, "httpChecksum"),
            "{id}: {outcome:?}"
        );
    } else {
        let unsupported = Some("Unsupported checksum algorithm: md5");
        assert_eq!(
            (id, sent.len(), text(&outcome).as_deref()),
            ("put-required-unsupported", 0, unsupported)
        );
    }
}

/// Every zchecksum row, through a client with checksums switched on: each
/// row native sends sends what the Python route sent and returns as it
/// did, each it refuses sends nothing, and the one botocore fails before
/// a request fails with its text. Every row is one of the three.
#[test]
fn every_row_does_what_the_python_route_did() -> TestResult {
    let rows = rows()?;
    let classified = SENT.len() + REFUSED.len() + 1;
    assert_eq!(rows.len(), classified, "a row is unclassified");
    for (id, row) in &rows {
        let (operation, input, settings, response) = call_of(row)?;
        let (client, transport) = client(settings, true)?;
        transport.respond(response);
        let outcome = client.call(&operation, input);
        let requests: Vec<HttpRequest> = (transport.sent().into_iter())
            .map(|(request, _)| request)
            .collect();
        check(id, &requests, outcome);
    }
    Ok(())
}

/// Switched off, as every production client is, admission refuses both
/// traits before a request.
#[test]
fn switched_off_every_checksum_operation_is_refused() -> TestResult {
    let (client, transport) = client(settings(), false)?;
    let operations = [
        ("PutRequired", "httpChecksum"),
        ("GetValidated", "httpChecksum"),
        ("PutLegacy", "httpChecksumRequired"),
    ];
    for (operation, key) in operations {
        let outcome = client.prepare(operation, params(&[("Id", "a")]));
        assert!(refused_as(&outcome, key), "{operation}: {outcome:?}");
    }
    assert!(transport.sent().is_empty());
    Ok(())
}
