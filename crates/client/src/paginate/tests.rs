//! botocore's paginator tests (tests/unit/botocore/test_paginate.py, and
//! tests/functional/botocore/test_paginate.py where it applies), run over
//! a json-protocol client and the in-memory transport: each page is a
//! response body, and each request's body shows the tokens it carried.
//! Tokens and results the Python tests do not pin are the vendored
//! botocore's for the same pages.
use super::*;
use crate::call::{ClientParts, UserAgentContext};
use crate::credentials::{CredentialError, CredentialSource, SigningCredentials};
use crate::endpoint::{EndpointConfig, EndpointData, EndpointInputs};
use crate::http::{Headers, HttpResponse};
use crate::testing::{FixedRuntime, InMemoryTransport};
use cli_core::client_config::ClientSettings;
use cli_core::client_config::user_agent::{Host, Versions};
use model::shapes::Model;
use std::sync::Arc;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SERVICE: &str = r#"{
    "metadata": {"apiVersion": "2020-01-01", "protocol": "json", "jsonVersion": "1.0",
        "targetPrefix": "ZFake", "endpointPrefix": "zfake", "signingName": "zfake",
        "signatureVersion": "v4", "serviceId": "ZFake"},
    "operations": {
        "List": {"name": "List", "http": {"method": "POST", "requestUri": "/"},
            "input": {"shape": "ListIn"}, "output": {"shape": "ListOut"}},
        "Bare": {"name": "Bare", "http": {"method": "POST", "requestUri": "/"},
            "output": {"shape": "ListOut"}}
    },
    "shapes": {
        "ListIn": {"type": "structure", "members": {
            "Filter": {"shape": "S"}, "Marker": {"shape": "S"}, "Marker2": {"shape": "S"},
            "Marker3": {"shape": "S"}, "MaxResults": {"shape": "I"},
            "MaxItems": {"shape": "S"}, "Bin": {"shape": "B"}, "Key": {"shape": "Key"}}},
        "ListOut": {"type": "structure", "members": {
            "Users": {"shape": "Ss"}, "Groups": {"shape": "Ss"}, "Count": {"shape": "I"},
            "Log": {"shape": "S"}, "Marker": {"shape": "S"}, "Marker2": {"shape": "S"},
            "Marker3": {"shape": "S"}, "IsTruncated": {"shape": "Bool"},
            "Inner": {"shape": "Inner"}, "Owner": {"shape": "S"}, "Usage": {"shape": "Usage"},
            "Bin": {"shape": "B"}, "Key": {"shape": "Key"}}},
        "Inner": {"type": "structure", "members": {"Items": {"shape": "Ss"},
            "Marker": {"shape": "S"}, "More": {"shape": "Bool"}, "Note": {"shape": "S"},
            "Deep": {"shape": "Deep"}, "Tags": {"shape": "Tags"}}},
        "Deep": {"type": "structure", "members": {"Names": {"shape": "Ss"}}},
        "Tags": {"type": "list", "member": {"shape": "Tag"}},
        "Tag": {"type": "structure", "members": {"Key": {"shape": "S"},
            "Values": {"shape": "Ss"}}},
        "Key": {"type": "structure", "members": {"key": {"shape": "B"}, "key2": {"shape": "B"}}},
        "Usage": {"type": "map", "key": {"shape": "S"}, "value": {"shape": "Ss"}},
        "Ss": {"type": "list", "member": {"shape": "S"}},
        "S": {"type": "string"}, "I": {"type": "integer"}, "Bool": {"type": "boolean"},
        "B": {"type": "blob"}
    }
}"#;

const RULES: &str = r#"{
    "version": "1.0",
    "parameters": {"Region": {"type": "String", "builtIn": "AWS::Region", "required": false}},
    "rules": [
        {"conditions": [], "endpoint": {"url": "https://zfake.{Region}.example.com"},
         "type": "endpoint"}
    ]
}"#;

const PARTITIONS: &str = r#"{"version": "1.1", "partitions": [{"id": "aws",
    "regionRegex": "^(us|eu)\\-\\w+\\-\\d+$", "regions": {},
    "outputs": {"name": "aws", "dnsSuffix": "example.com", "dualStackDnsSuffix": "example.com",
        "supportsFIPS": true, "supportsDualStack": true, "implicitGlobalRegion": "us-east-1"}}]}"#;

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

struct Fixture {
    transport: Arc<InMemoryTransport>,
    client: Client,
}

fn fixture() -> TestResult<Fixture> {
    let transport = Arc::new(InMemoryTransport::new());
    let now = timestamp::Timestamp::from_unix_seconds(1_440_938_160).ok_or("clock")?;
    let client = Client::new(ClientParts {
        service: "zfake".into(),
        model: Arc::new(Model::from_json(SERVICE.as_bytes())?),
        endpoints: EndpointData::from_documents(
            RULES.as_bytes().to_vec(),
            PARTITIONS.as_bytes().to_vec(),
            None,
        )?,
        endpoint: EndpointConfig::new(&EndpointInputs {
            region: Some("us-west-2"),
            signing: true,
            ..EndpointInputs::default()
        }),
        settings: ClientSettings::new("zfake.list"),
        credentials: Arc::new(Keys),
        transport: transport.clone(),
        runtime: Arc::new(FixedRuntime::new(now, 0.0)),
        customizations: &[],
        parse: crate::protocol::ParseHooks::botocore(),
        user_agent: UserAgentContext {
            host: Host {
                system: "Linux".into(),
                release: "6.1".into(),
                machine: "x86_64".into(),
            },
            distribution: None,
            versions: Versions {
                cli: "2.0.0",
                crt: "0.1.0",
                rust: "1.96.0",
            },
            extras: crate::useragent::SessionExtras {
                installer: "other",
                prompt: "off",
                command: "zfake.list".into(),
            },
        },
    })?;
    Ok(Fixture { transport, client })
}

impl Fixture {
    /// Answers the next requests with `pages`, in order.
    fn respond(&self, pages: &[&str]) {
        for page in pages {
            self.transport.respond(HttpResponse {
                status: 200,
                headers: Headers::new(),
                body: page.as_bytes().to_vec(),
            });
        }
    }
    /// Every request's body, in order.
    fn bodies(&self) -> Vec<String> {
        let sent = self.transport.sent();
        let body = |(request, _): &(crate::http::HttpRequest, _)| {
            String::from_utf8_lossy(&request.body).into_owned()
        };
        sent.iter().map(body).collect()
    }
    fn pages(&self, config: &str, pagination: PaginationConfig) -> TestResult<PageIterator<'_>> {
        let input = Value::Map(Map::new());
        self.pages_of(config, input, pagination)
    }
    fn pages_of(
        &self,
        config: &str,
        input: Value,
        pagination: PaginationConfig,
    ) -> TestResult<PageIterator<'_>> {
        let config = Config::from_value(&json(config)?).map_err(|r| format!("{r:?}"))?;
        Ok(PageIterator::new(
            &self.client,
            "List",
            input,
            config,
            pagination,
        ))
    }
}

/// `value[key]` of a dict.
fn field<'v>(value: &'v Value, key: &str) -> Option<&'v Value> {
    match value {
        Value::Map(map) => map.get(key),
        _ => None,
    }
}

fn json(text: &str) -> TestResult<Value> {
    Ok(Value::from_json(text.as_bytes())?)
}

fn max_items(count: i64) -> PaginationConfig {
    PaginationConfig {
        max_items: Some(Value::Integer(count)),
        ..PaginationConfig::default()
    }
}

fn starting(token: &str) -> PaginationConfig {
    PaginationConfig {
        starting_token: Some(token.into()),
        ..PaginationConfig::default()
    }
}

fn all(pages: PageIterator<'_>) -> Result<Vec<Value>, CallError> {
    pages.collect()
}

/// Each page without the `ResponseMetadata` its parse added, as the CLI's
/// formatters take it out (`Formatter._remove_request_id`).
fn bare(pages: Vec<Value>) -> Vec<Value> {
    let strip = |mut page: Value| {
        if let Value::Map(map) = &mut page {
            map.remove("ResponseMetadata");
        }
        page
    };
    pages.into_iter().map(strip).collect()
}

const MARKER: &str =
    r#"{"input_token": "Marker", "output_token": "Marker", "result_key": "Users"}"#;
const GROUPS: &str =
    r#"{"input_token": "Marker", "output_token": "Marker", "result_key": ["Users", "Groups"]}"#;
const LIMITED: &str = r#"{"input_token": "Marker", "output_token": "Marker",
    "result_key": "Users", "limit_key": "MaxResults"}"#;
const TWO_TOKENS: &str = r#"{"input_token": ["Marker", "Marker2"],
    "output_token": ["Marker", "Marker2"], "result_key": ["Users", "Groups"]}"#;

/// `encode_token({'Marker': 'm1'})`, and kin.
const M1: &str = "eyJNYXJrZXIiOiAibTEifQ==";
const M2: &str = "eyJNYXJrZXIiOiAibTIifQ==";
const M1_AT_1: &str = "eyJNYXJrZXIiOiAibTEiLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAxfQ==";
const NONE_AT_1: &str = "eyJNYXJrZXIiOiBudWxsLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAxfQ==";

#[test]
fn an_entry_reads_as_paginator_reads_it() -> TestResult {
    let read = Config::from_value(&json(
        r#"{"input_token": "Marker", "output_token": "Inner.Marker", "result_key": "Users",
            "limit_key": "MaxResults", "more_results": "Inner.More",
            "non_aggregate_keys": ["Owner", "Inner.Note"]}"#,
    )?);
    let names = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(
        read,
        Ok(Config {
            input_token: names(&["Marker"]),
            output_token: names(&["Inner.Marker"]),
            more_results: Some("Inner.More".into()),
            result_key: names(&["Users"]),
            non_aggregate_keys: names(&["Owner", "Inner.Note"]),
            limit_key: Some("MaxResults".into()),
        })
    );
    let listed = Config::from_value(&json(TWO_TOKENS)?).map_err(|r| format!("{r:?}"))?;
    assert_eq!(listed.input_token, names(&["Marker", "Marker2"]));
    assert_eq!(listed.result_key, names(&["Users", "Groups"]));
    assert_eq!((listed.more_results, listed.limit_key), (None, None));
    Ok(())
}

/// The JMESPath the configurations use beyond names and dotted paths:
/// s3's `NextMarker || Contents[-1].Key`, importexport's `Jobs[-1].JobId`.
#[test]
fn an_expression_beyond_a_dotted_path_is_refused() -> TestResult {
    let refused = |entry: &str, text: &str| -> TestResult<bool> {
        Ok(Config::from_value(&json(entry)?) == Err(Refused::Expression(text.into())))
    };
    assert!(refused(
        r#"{"input_token": "M", "output_token": ["NextMarker || Contents[-1].Key"],
            "result_key": "U"}"#,
        "NextMarker || Contents[-1].Key"
    )?);
    assert!(refused(
        r#"{"input_token": "M", "output_token": "M", "result_key": "Jobs[-1].JobId"}"#,
        "Jobs[-1].JobId"
    )?);
    assert!(refused(
        r#"{"input_token": "M", "output_token": "M", "result_key": "U",
            "more_results": "\"Quoted\""}"#,
        "\"Quoted\""
    )?);
    for key in ["A.b-c", "A.", "1st"] {
        let entry = format!(
            r#"{{"input_token": "M", "output_token": "M", "result_key": "U",
                "non_aggregate_keys": ["{key}"]}}"#
        );
        assert!(refused(&entry, key)?, "{key}");
    }
    Ok(())
}

/// What botocore cannot read, or cannot page with.
#[test]
fn an_entry_botocore_cannot_read_is_malformed() -> TestResult {
    let malformed = |entry: &str| {
        json(entry).map(|entry| matches!(Config::from_value(&entry), Err(Refused::Malformed(_))))
    };
    assert!(malformed(r#"["Marker"]"#)?);
    assert!(malformed(
        r#"{"input_token": "Marker", "result_key": "Users"}"#
    )?);
    assert!(malformed(
        r#"{"input_token": "Marker", "output_token": "Marker"}"#
    )?);
    assert!(malformed(
        r#"{"input_token": "M", "output_token": "M", "result_key": []}"#
    )?);
    assert!(malformed(
        r#"{"input_token": 5, "output_token": "M", "result_key": "U"}"#
    )?);
    assert!(malformed(
        r#"{"input_token": "M", "output_token": "M", "result_key": "U", "limit_key": 5}"#
    )?);
    assert!(malformed(
        r#"{"input_token": "M", "output_token": "M", "result_key": "U",
            "non_aggregate_keys": "Owner"}"#
    )?);
    Ok(())
}

/// `test_no_next_token`.
#[test]
fn a_page_without_a_next_token_is_the_last() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Owner": "foobar"}"#]);
    let pages = all(f.pages(MARKER, PaginationConfig::default())?)?;
    assert_eq!(bare(pages), [json(r#"{"Owner": "foobar"}"#)?]);
    assert_eq!(f.bodies(), ["{}"]);
    Ok(())
}

/// `test_next_token_in_response`, `test_any_passed_in_args_are_unmodified`:
/// each next token is added after the caller's parameters.
#[test]
fn each_next_token_is_sent_after_the_caller_s_parameters() -> TestResult {
    let f = fixture()?;
    let pages = [
        r#"{"Marker": "token1"}"#,
        r#"{"Marker": "token2"}"#,
        r#"{"Owner": "foo"}"#,
    ];
    f.respond(&pages);
    let input = json(r#"{"Filter": "foo"}"#)?;
    let got = all(f.pages_of(MARKER, input, PaginationConfig::default())?)?;
    assert_eq!(got.len(), 3);
    assert_eq!(
        f.bodies(),
        [
            r#"{"Filter": "foo"}"#,
            r#"{"Filter": "foo", "Marker": "token1"}"#,
            r#"{"Filter": "foo", "Marker": "token2"}"#,
        ]
    );
    Ok(())
}

/// `test_exception_raised_if_same_next_token`: botocore's text, after the
/// pages before it.
#[test]
fn the_same_next_token_twice_is_a_pagination_error() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Marker": "t1"}"#,
        r#"{"Marker": "t2"}"#,
        r#"{"Marker": "t2"}"#,
    ]);
    let mut pages = f.pages(MARKER, PaginationConfig::default())?;
    for _ in 0..3 {
        assert!(matches!(pages.next(), Some(Ok(_))));
    }
    let error = pages.next().ok_or("an error")?.err().ok_or("an error")?;
    assert_eq!(
        error.to_string(),
        "Error during pagination: The same next token was received twice: {'Marker': 't2'}"
    );
    assert!(pages.next().is_none());
    assert_eq!(f.bodies().len(), 3);
    Ok(())
}

/// `test_more_tokens`, `test_more_tokens_is_path_expression`.
#[test]
fn a_false_more_results_ends_the_pages() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["1"], "IsTruncated": true, "Marker": "token1"}"#,
        r#"{"Users": ["2"], "IsTruncated": false, "Marker": "token2"}"#,
        r#"{"Inner": {"More": true}, "Marker": "token1"}"#,
        r#"{"Inner": {"More": false}, "Marker": "token2"}"#,
    ]);
    let config = r#"{"more_results": "IsTruncated", "output_token": "Marker",
        "input_token": "Marker", "result_key": "Users"}"#;
    assert_eq!(all(f.pages(config, PaginationConfig::default())?)?.len(), 2);
    let config = r#"{"more_results": "Inner.More", "output_token": "Marker",
        "input_token": "Marker", "result_key": "Users"}"#;
    assert_eq!(all(f.pages(config, PaginationConfig::default())?)?.len(), 2);
    assert_eq!(
        f.bodies(),
        [
            "{}",
            r#"{"Marker": "token1"}"#,
            "{}",
            r#"{"Marker": "token1"}"#
        ]
    );
    Ok(())
}

/// `test_page_size`: the limit key, set before the first request, comes
/// before the tokens that follow it.
#[test]
fn the_page_size_is_sent_as_the_limit_key() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User1"], "Marker": "m1"}"#,
        r#"{"Users": ["User2"], "Marker": "m2"}"#,
        r#"{"Users": ["User3"]}"#,
    ]);
    let pagination = PaginationConfig {
        page_size: Some(Value::Integer(1)),
        ..PaginationConfig::default()
    };
    all(f.pages(LIMITED, pagination)?)?;
    assert_eq!(
        f.bodies(),
        [
            r#"{"MaxResults": 1}"#,
            r#"{"MaxResults": 1, "Marker": "m1"}"#,
            r#"{"MaxResults": 1, "Marker": "m2"}"#,
        ]
    );
    Ok(())
}

/// `test_page_size_incorrectly_provided`: before any request.
#[test]
fn a_page_size_without_a_limit_key_is_a_pagination_error() -> TestResult {
    let f = fixture()?;
    let pagination = PaginationConfig {
        page_size: Some(Value::Integer(5)),
        ..PaginationConfig::default()
    };
    let error = all(f.pages(MARKER, pagination)?).err().ok_or("an error")?;
    assert_eq!(
        error.to_string(),
        "Error during pagination: PageSize parameter is not supported for the pagination \
         interface for this operation."
    );
    assert!(f.bodies().is_empty());
    Ok(())
}

/// `TestStringPageSize`: a string limit key takes `str()` of the size.
#[test]
fn a_string_limit_key_takes_the_page_size_as_a_string() -> TestResult {
    let f = fixture()?;
    f.respond(&["{}", "{}"]);
    let config = r#"{"input_token": "Marker", "output_token": "Marker", "result_key": "Users",
        "limit_key": "MaxItems"}"#;
    for size in [Value::Integer(1), Value::String("1".into())] {
        let pagination = PaginationConfig {
            page_size: Some(size),
            ..PaginationConfig::default()
        };
        all(f.pages(config, pagination)?)?;
    }
    assert_eq!(f.bodies(), [r#"{"MaxItems": "1"}"#, r#"{"MaxItems": "1"}"#]);
    Ok(())
}

/// `test_with_empty_markers`: an empty token is none.
#[test]
fn an_empty_next_token_ends_the_pages() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Users": ["User1"], "Marker": ""}"#]);
    assert_eq!(all(f.pages(MARKER, PaginationConfig::default())?)?.len(), 1);
    assert_eq!(f.bodies(), ["{}"]);
    Ok(())
}

/// `TestMultipleTokens`: dotted output tokens, each sent as its input
/// token.
#[test]
fn several_tokens_are_sent_together() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["1"], "Inner": {"Marker": "key1"}, "Marker2": "up1"}"#,
        r#"{"Users": ["2"], "Inner": {"Marker": "key2"}, "Marker2": "up2"}"#,
        "{}",
    ]);
    let config = r#"{"output_token": ["Inner.Marker", "Marker2"],
        "input_token": ["Marker", "Marker2"], "result_key": "Users"}"#;
    all(f.pages(config, PaginationConfig::default())?)?;
    assert_eq!(
        f.bodies(),
        [
            "{}",
            r#"{"Marker": "key1", "Marker2": "up1"}"#,
            r#"{"Marker": "key2", "Marker2": "up2"}"#,
        ]
    );
    Ok(())
}

/// `TestOptionalTokens`: a token a page lacks is taken out of the next
/// request.
#[test]
fn a_token_a_page_lacks_is_not_sent() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["1"], "Marker": "aaa", "Marker2": "A", "Marker3": "id"}"#,
        r#"{"Users": ["2"], "Marker": "bbb", "Marker2": "A"}"#,
        r#"{"Users": ["3"]}"#,
    ]);
    let config = r#"{"output_token": ["Marker", "Marker2", "Marker3"],
        "input_token": ["Marker", "Marker2", "Marker3"], "result_key": "Users"}"#;
    all(f.pages(config, PaginationConfig::default())?)?;
    assert_eq!(
        f.bodies(),
        [
            "{}",
            r#"{"Marker": "aaa", "Marker2": "A", "Marker3": "id"}"#,
            r#"{"Marker": "bbb", "Marker2": "A"}"#,
        ]
    );
    Ok(())
}

const USERS_1_2_3: [&str; 3] = [
    r#"{"Users": ["User1"], "Marker": "m1"}"#,
    r#"{"Users": ["User2"], "Marker": "m2"}"#,
    r#"{"Users": ["User3"]}"#,
];

/// `test_build_full_result_with_single_key`.
#[test]
fn the_full_result_joins_every_page_s_list() -> TestResult {
    let f = fixture()?;
    f.respond(&USERS_1_2_3);
    let full = f
        .pages(LIMITED, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(full, json(r#"{"Users": ["User1", "User2", "User3"]}"#)?);
    Ok(())
}

/// `test_max_items_can_be_specified`, `test_max_items_as_strings`,
/// `test_next_token_on_page_boundary`, `test_next_token_is_string`: at a
/// page boundary the resume token is the next page's token.
#[test]
fn max_items_on_a_page_boundary_resumes_from_the_next_token() -> TestResult {
    for (max, expected) in [
        (
            Value::Integer(1),
            format!(r#"{{"Users": ["User1"], "NextToken": "{M1}"}}"#),
        ),
        (
            Value::String("1".into()),
            format!(r#"{{"Users": ["User1"], "NextToken": "{M1}"}}"#),
        ),
        (
            Value::Integer(2),
            format!(r#"{{"Users": ["User1", "User2"], "NextToken": "{M2}"}}"#),
        ),
    ] {
        let f = fixture()?;
        f.respond(&USERS_1_2_3);
        let pagination = PaginationConfig {
            max_items: Some(max),
            ..PaginationConfig::default()
        };
        let mut pages = f.pages(MARKER, pagination)?;
        assert_eq!(pages.build_full_result()?, json(&expected)?);
        assert_eq!(
            f.bodies().len(),
            if expected.contains("User2") { 2 } else { 1 }
        );
    }
    Ok(())
}

/// `test_max_items_can_be_specified_truncates_response`,
/// `test_resume_next_marker_mid_page`: the token of the request that got
/// the page, and how much of it was taken.
#[test]
fn max_items_mid_page_truncates_it_and_resumes_into_it() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User1", "User2", "User3"], "Marker": "m1"}"#,
        r#"{"Users": ["User4", "User5", "User6"], "Marker": "m2"}"#,
    ]);
    let full = f.pages(MARKER, max_items(4))?.build_full_result()?;
    let expected =
        format!(r#"{{"Users": ["User1", "User2", "User3", "User4"], "NextToken": "{M1_AT_1}"}}"#);
    assert_eq!(full, json(&expected)?);
    f.respond(&[
        r#"{"Users": ["User4", "User5", "User6"], "Marker": "m2"}"#,
        r#"{"Users": ["User7"]}"#,
    ]);
    let full = f.pages(MARKER, starting(M1_AT_1))?.build_full_result()?;
    assert_eq!(full, json(r#"{"Users": ["User5", "User6", "User7"]}"#)?);
    assert_eq!(
        f.bodies(),
        [
            "{}",
            r#"{"Marker": "m1"}"#,
            r#"{"Marker": "m1"}"#,
            r#"{"Marker": "m2"}"#
        ]
    );
    Ok(())
}

/// `test_build_multiple_results`: the resume token counts from the page's
/// start, its own left truncation included.
#[test]
fn a_resumed_truncation_counts_from_the_page_s_start() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User1", "User2"], "Marker": "m1"}"#,
        r#"{"Users": ["User3", "User4"], "Marker": "m2"}"#,
        r#"{"Users": ["User3", "User4"], "Marker": "m2"}"#,
        r#"{"Users": ["User5", "User6", "User7"], "Marker": "m3"}"#,
    ]);
    let pagination = |starting_token| PaginationConfig {
        max_items: Some(Value::Integer(3)),
        page_size: Some(Value::Integer(2)),
        starting_token,
    };
    let first = f.pages(LIMITED, pagination(None))?.build_full_result()?;
    let Some(Value::String(token)) = field(&first, "NextToken") else {
        return Err("a NextToken".into());
    };
    let second = f
        .pages(LIMITED, pagination(Some(token.clone())))?
        .build_full_result()?;
    let expected = "eyJNYXJrZXIiOiAibTIiLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAyfQ==";
    assert_eq!(
        second,
        json(&format!(
            r#"{{"Users": ["User4", "User5", "User6"], "NextToken": "{expected}"}}"#
        ))?
    );
    assert_eq!(
        f.bodies()[2..],
        [
            r#"{"Marker": "m1", "MaxResults": 2}"#,
            r#"{"Marker": "m2", "MaxResults": 2}"#
        ]
    );
    Ok(())
}

/// `test_max_items_exceeds_actual_amount`.
#[test]
fn max_items_beyond_every_page_gives_them_all() -> TestResult {
    let f = fixture()?;
    f.respond(&USERS_1_2_3);
    let full = f.pages(MARKER, max_items(10))?.build_full_result()?;
    assert_eq!(full, json(r#"{"Users": ["User1", "User2", "User3"]}"#)?);
    Ok(())
}

/// `test_bad_input_tokens`: before any request.
#[test]
fn an_old_token_with_too_many_parts_is_a_value_error() -> TestResult {
    let f = fixture()?;
    let error = all(f.pages(MARKER, starting("does___not___work"))?)
        .err()
        .ok_or("an error")?;
    assert_eq!(
        error,
        CallError::Pagination(PaginationError::Value(
            "Bad starting token: does___not___work".into()
        ))
    );
    assert!(f.bodies().is_empty());
    Ok(())
}

/// `TestMultipleResultKeys`: each key joins its own pages, an empty list
/// kept.
#[test]
fn several_result_keys_are_joined_each() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User1"], "Groups": ["Group1"], "Marker": "m1"}"#,
        r#"{"Users": [], "Groups": ["Group2"], "Marker": "m2"}"#,
        r#"{"Users": [], "Groups": ["Group3"]}"#,
    ]);
    let full = f
        .pages(GROUPS, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(
        full,
        json(r#"{"Users": ["User1"], "Groups": ["Group1", "Group2", "Group3"]}"#)?
    );
    f.respond(&[r#"{"Users": [], "Groups": ["Group1"]}"#]);
    let full = f
        .pages(GROUPS, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(full, json(r#"{"Users": [], "Groups": ["Group1"]}"#)?);
    Ok(())
}

/// `test_build_result_with_secondary_keys`: only the primary key counts
/// toward and is cut to `MaxItems`.
#[test]
fn max_items_cuts_only_the_primary_result_key() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User1", "User2"], "Groups": ["Group1", "Group2"], "Marker": "m1"}"#,
    ]);
    let full = f.pages(GROUPS, max_items(1))?.build_full_result()?;
    let expected = format!(
        r#"{{"Users": ["User1"], "Groups": ["Group1", "Group2"], "NextToken": "{NONE_AT_1}"}}"#
    );
    assert_eq!(full, json(&expected)?);
    Ok(())
}

/// `test_resume_with_secondary_keys`, `..._as_string`,
/// `..._does_not_reset_integer`: on the page resumed into, the secondary
/// keys were already given whole.
#[test]
fn resuming_into_a_page_empties_its_secondary_keys() -> TestResult {
    let f = fixture()?;
    let pagination = PaginationConfig {
        starting_token: Some(NONE_AT_1.into()),
        ..max_items(1)
    };
    f.respond(&[
        r#"{"Users": ["User1", "User2"], "Groups": ["Group1", "Group2"], "Marker": "m1"}"#,
    ]);
    let full = f.pages(GROUPS, pagination.clone())?.build_full_result()?;
    assert_eq!(
        full,
        json(&format!(
            r#"{{"Users": ["User2"], "Groups": [], "NextToken": "{M1}"}}"#
        ))?
    );
    let kinds = r#"{"input_token": "Marker", "output_token": "Marker",
        "result_key": ["Users", "Log", "Count"]}"#;
    f.respond(&[r#"{"Users": ["User1", "User2"], "Log": "a", "Count": 123}"#]);
    let full = f.pages(kinds, pagination)?.build_full_result()?;
    assert_eq!(
        full,
        json(r#"{"Users": ["User2"], "Log": "", "Count": 123}"#)?
    );
    assert_eq!(f.bodies(), ["{}", "{}"]);
    Ok(())
}

/// `TestMultipleInputKeys`.
#[test]
fn several_input_tokens_resume_together() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User1", "User2"], "Groups": ["Group1"], "Marker": "m1", "Marker2": "m2"}"#,
        r#"{"Users": ["User3", "User4"], "Groups": ["Group2"], "Marker": "m3", "Marker2": "m4"}"#,
    ]);
    let full = f.pages(TWO_TOKENS, max_items(3))?.build_full_result()?;
    let token = "eyJNYXJrZXIiOiAibTEiLCAiTWFya2VyMiI6ICJtMiIsICJib3RvX3RydW5jYXRlX2Ftb3VudCI6IDF9";
    let expected = format!(
        r#"{{"Users": ["User1", "User2", "User3"], "Groups": ["Group1", "Group2"],
            "NextToken": "{token}"}}"#
    );
    assert_eq!(full, json(&expected)?);
    f.respond(&[
        r#"{"Users": ["User3", "User4"], "Groups": ["Group2"], "Marker": "m3", "Marker2": "m4"}"#,
    ]);
    let pagination = PaginationConfig {
        starting_token: Some(token.into()),
        ..max_items(1)
    };
    let full = f.pages(TWO_TOKENS, pagination)?.build_full_result()?;
    let next = "eyJNYXJrZXIiOiAibTMiLCAiTWFya2VyMiI6ICJtNCJ9";
    assert_eq!(
        full,
        json(&format!(
            r#"{{"Users": ["User4"], "Groups": [], "NextToken": "{next}"}}"#
        ))?
    );
    assert_eq!(f.bodies()[2], r#"{"Marker": "m1", "Marker2": "m2"}"#);
    Ok(())
}

/// `test_resume_encounters_an_empty_payload`: the result keys are set to
/// `None` on the page resumed into, which the full result skips.
#[test]
fn resuming_into_a_page_without_result_keys_sets_them_none() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Marker": "m1"}"#, r#"{"Users": ["u"]}"#]);
    let pages = all(f.pages(GROUPS, starting(NONE_AT_1))?)?;
    assert_eq!(
        bare(pages),
        [
            json(r#"{"Marker": "m1", "Users": null, "Groups": null}"#)?,
            json(r#"{"Users": ["u"]}"#)?
        ]
    );
    f.respond(&[r#"{"Owner": "it happens in some service"}"#]);
    let full = f
        .pages(TWO_TOKENS, starting(NONE_AT_1))?
        .build_full_result()?;
    assert_eq!(full, json("{}")?);
    Ok(())
}

/// `TestExpressionKeyIterators`: a dotted result key is joined in place.
#[test]
fn a_dotted_result_key_is_joined_where_it_is() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Inner": {"Items": ["One", "Two"]}, "Marker": "m1"}"#,
        r#"{"Inner": {"Items": ["Three", "Four"]}, "Marker": "m2"}"#,
        r#"{"Inner": {"Items": ["Five"]}}"#,
    ]);
    let config = r#"{"input_token": "Marker", "output_token": "Marker",
        "limit_key": "MaxResults", "result_key": "Inner.Items"}"#;
    let full = f
        .pages(config, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(
        full,
        json(r#"{"Inner": {"Items": ["One", "Two", "Three", "Four", "Five"]}}"#)?
    );
    Ok(())
}

/// `TestIncludeResultKeys`: numbers add and strings join; a page lacking
/// a key adds nothing.
#[test]
fn numbers_add_and_strings_join_across_pages() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["a"], "Count": 1, "Log": "x", "Marker": "a"}"#,
        r#"{"Owner": "this page will be ignored", "Marker": "_"}"#,
        r#"{"Users": ["b", "c"], "Count": 2, "Log": "y", "Marker": "b"}"#,
        r#"{"Users": ["d", "e", "f"], "Count": 3, "Log": "z"}"#,
        r#"{"Owner": "ignored", "Marker": "_"}"#,
        r#"{"Owner": "ignored, too"}"#,
    ]);
    let config = r#"{"input_token": "Marker", "output_token": "Marker",
        "result_key": ["Users", "Count", "Log"]}"#;
    let full = f
        .pages(config, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(
        full,
        json(r#"{"Users": ["a", "b", "c", "d", "e", "f"], "Count": 6, "Log": "xyz"}"#)?
    );
    let full = f
        .pages(config, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(full, json("{}")?);
    Ok(())
}

/// `TestIncludeNonResultKeys`: the first page's, nested where dotted, and
/// `None` where the page lacks one.
#[test]
fn the_non_aggregate_keys_are_the_first_page_s() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Inner": {"Items": ["foo"], "Note": "v1"}, "Owner": "v2", "Marker": "t1"}"#,
        r#"{"Inner": {"Items": ["bar", "baz"], "Note": "v3"}, "Owner": "v4", "Marker": "t2"}"#,
        r#"{"Inner": {"Items": ["qux"], "Note": "v5"}, "Owner": "v6"}"#,
    ]);
    let config = r#"{"input_token": "Marker", "output_token": "Marker",
        "result_key": "Inner.Items", "non_aggregate_keys": ["Owner", "Inner.Note"]}"#;
    let mut pages = f.pages(config, PaginationConfig::default())?;
    let full = pages.build_full_result()?;
    assert_eq!(
        Value::Map(pages.non_aggregate_part().clone()),
        json(r#"{"Owner": "v2", "Inner": {"Note": "v1"}}"#)?
    );
    let expected =
        r#"{"Inner": {"Items": ["foo", "bar", "baz", "qux"], "Note": "v1"}, "Owner": "v2"}"#;
    assert_eq!(full, json(expected)?);
    f.respond(&[
        r#"{"Users": ["a"], "Marker": "m1"}"#,
        r#"{"Users": ["b"], "Owner": "o"}"#,
    ]);
    let config = r#"{"input_token": "Marker", "output_token": "Marker",
        "result_key": "Users", "non_aggregate_keys": ["Owner", "Inner.Note"]}"#;
    let full = f
        .pages(config, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(
        full,
        json(r#"{"Users": ["a", "b"], "Owner": null, "Inner": {"Note": null}}"#)?
    );
    Ok(())
}

/// `TestDeprecatedStartingToken`: an old token resumes, its offset into
/// the first page, and a truncation then gives a token of the new form.
#[test]
fn an_old_token_resumes_and_gives_a_new_one() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User1", "User2"], "Groups": ["Group1"], "Marker": "m1", "Marker2": "m2"}"#,
        r#"{"Users": ["User3", "User4"], "Groups": ["Group2"], "Marker": "m3", "Marker2": "m4"}"#,
        r#"{"Users": ["User5"], "Groups": ["Group3"]}"#,
    ]);
    let full = f
        .pages(TWO_TOKENS, starting("m0___m0___1"))?
        .build_full_result()?;
    let expected =
        r#"{"Users": ["User2", "User3", "User4", "User5"], "Groups": ["Group2", "Group3"]}"#;
    assert_eq!(full, json(expected)?);
    f.respond(&[
        r#"{"Users": ["User1"], "Marker": "m2"}"#,
        r#"{"Users": ["User2"], "Marker": "m3"}"#,
        r#"{"Users": ["User3"], "Marker": "m4"}"#,
    ]);
    let pagination = PaginationConfig {
        starting_token: Some("m1___0".into()),
        ..max_items(3)
    };
    let full = f.pages(MARKER, pagination)?.build_full_result()?;
    let m4 = "eyJNYXJrZXIiOiAibTQifQ==";
    assert_eq!(
        full,
        json(&format!(
            r#"{{"Users": ["User1", "User2", "User3"], "NextToken": "{m4}"}}"#
        ))?
    );
    assert_eq!(f.bodies()[0], r#"{"Marker": "m0", "Marker2": "m0"}"#);
    assert_eq!(f.bodies()[3], r#"{"Marker": "m1"}"#);
    Ok(())
}

/// `test_deprecated_starting_token_without_all_input_set_to_none`: the
/// tokens it lacks are not sent.
#[test]
fn an_old_token_short_of_the_input_tokens_sends_what_it_has() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User1"], "Marker": "m1", "Marker2": "m2"}"#,
        r#"{"Users": ["User2"]}"#,
    ]);
    let full = f.pages(TWO_TOKENS, starting("m0"))?.build_full_result()?;
    assert_eq!(full, json(r#"{"Users": ["User1", "User2"]}"#)?);
    assert_eq!(
        f.bodies(),
        [
            r#"{"Marker": "m0"}"#,
            r#"{"Marker": "m1", "Marker2": "m2"}"#
        ]
    );
    Ok(())
}

/// The functional `test_can_specify_zero_marker` (rds
/// `DownloadDBLogFilePortion`): `'0'` is an old token, and a str result
/// key counts its characters.
#[test]
fn a_zero_marker_starts_and_a_str_result_key_counts_characters() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Log": "foo", "Marker": "2", "IsTruncated": true}"#]);
    let config = r#"{"input_token": "Marker", "output_token": "Marker", "result_key": "Log",
        "more_results": "IsTruncated", "limit_key": "MaxResults"}"#;
    let pagination = PaginationConfig {
        starting_token: Some("0".into()),
        ..max_items(3)
    };
    let full = f.pages(config, pagination)?.build_full_result()?;
    assert_eq!(
        full,
        json(r#"{"Log": "foo", "NextToken": "eyJNYXJrZXIiOiAiMiJ9"}"#)?
    );
    assert_eq!(f.bodies(), [r#"{"Marker": "0"}"#]);
    Ok(())
}

/// The functional `test_token_with_triple_underscores` (logs
/// `FilterLogEvents`): no `int` after the `___`, so the service's own.
#[test]
fn a_service_token_with_triple_underscores_is_sent_whole() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Users": ["e"]}"#]);
    let pagination = PaginationConfig {
        starting_token: Some("foo___bar".into()),
        ..max_items(1)
    };
    let full = f.pages(MARKER, pagination)?.build_full_result()?;
    assert_eq!(full, json(r#"{"Users": ["e"]}"#)?);
    assert_eq!(f.bodies(), [r#"{"Marker": "foo___bar"}"#]);
    Ok(())
}

/// The functional `test_repeated_build_full_results` (autoscaling): each
/// resume token picks up where the last stopped, and none repeats.
#[test]
fn resuming_again_and_again_gives_every_item_once() -> TestResult {
    let f = fixture()?;
    let page = |items: &str, marker: &str| format!(r#"{{"Users": [{items}]{marker}}}"#);
    let (first, second) = (r#""a", "b", "c", "d""#, r#""e", "f", "g", "h""#);
    for body in [
        page(first, r#", "Marker": "p2""#),
        page(first, r#", "Marker": "p2""#),
        page(second, ""),
        page(second, ""),
    ] {
        f.respond(&[&body]);
    }
    let (mut items, mut tokens) = (Vec::new(), Vec::<String>::new());
    let mut pagination = max_items(2);
    for _ in 0..4 {
        let full = f.pages(MARKER, pagination.clone())?.build_full_result()?;
        if let Some(Value::List(users)) = field(&full, "Users") {
            items.extend(users.iter().cloned());
        }
        let Some(Value::String(token)) = field(&full, "NextToken") else {
            break;
        };
        assert!(!tokens.contains(token));
        tokens.push(token.clone());
        pagination.starting_token = Some(token.clone());
    }
    let expected: Vec<Value> = "abcdefgh"
        .chars()
        .map(|c| Value::String(c.into()))
        .collect();
    assert_eq!(items, expected);
    Ok(())
}

const BIN: &str = r#"{"input_token": "Bin", "output_token": "Bin", "result_key": "Users"}"#;
/// `encode_token({'Bin': b'\xff', 'boto_truncate_amount': 1})`.
const BIN_AT_1: &str = "eyJCaW4iOiAiL3c9PSIsICJib3RvX3RydW5jYXRlX2Ftb3VudCI6IDEsICJib3RvX2VuY29kZWR\
    fa2V5cyI6IFtbIkJpbiJdXX0=";

/// `TestBinaryTokens`: a bytes token is base64 in the resume token, and
/// bytes again when resumed from.
#[test]
fn a_bytes_token_resumes_as_bytes() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User1", "User2"], "Bin": "/w=="}"#,
        r#"{"Users": ["User3", "User4"], "Bin": "/g=="}"#,
    ]);
    let full = f.pages(BIN, max_items(3))?.build_full_result()?;
    let expected =
        format!(r#"{{"Users": ["User1", "User2", "User3"], "NextToken": "{BIN_AT_1}"}}"#);
    assert_eq!(full, json(&expected)?);
    f.respond(&[
        r#"{"Users": ["User3", "User4"], "Bin": "/g=="}"#,
        r#"{"Users": ["User5"]}"#,
    ]);
    let full = f.pages(BIN, starting(BIN_AT_1))?.build_full_result()?;
    assert_eq!(full, json(r#"{"Users": ["User4", "User5"]}"#)?);
    assert_eq!(
        f.bodies(),
        [
            "{}",
            r#"{"Bin": "/w=="}"#,
            r#"{"Bin": "/w=="}"#,
            r#"{"Bin": "/g=="}"#
        ]
    );
    Ok(())
}

/// `test_resume_with_multiple_bytes_values`: nested bytes.
#[test]
fn nested_bytes_tokens_resume_as_bytes() -> TestResult {
    let f = fixture()?;
    f.respond(&[
        r#"{"Users": ["User3", "User4"], "Key": {"key": "/g==", "key2": "7g=="}}"#,
        "{}",
    ]);
    let token = "eyJLZXkiOiB7ImtleSI6ICIvdz09IiwgImtleTIiOiAiN3c9PSJ9LCAiYm90b190cnVuY2F0ZV9hbW91bn\
        QiOiAx\
                 LCAiYm90b19lbmNvZGVkX2tleXMiOiBbWyJLZXkiLCAia2V5Il0sIFsiS2V5IiwgImtleTIiXV19";
    let config = r#"{"input_token": "Key", "output_token": "Key", "result_key": "Users"}"#;
    let full = f.pages(config, starting(token))?.build_full_result()?;
    assert_eq!(full, json(r#"{"Users": ["User4"]}"#)?);
    assert_eq!(
        f.bodies(),
        [
            r#"{"Key": {"key": "/w==", "key2": "7w=="}}"#,
            r#"{"Key": {"key": "/g==", "key2": "7g=="}}"#,
        ]
    );
    Ok(())
}

/// `@with_current_context(partial(register_feature_id, 'PAGINATOR'))`:
/// every page's call names `PAGINATOR` (`C`); `Client::call` does not.
#[test]
fn each_page_s_user_agent_names_the_paginator() -> TestResult {
    let f = fixture()?;
    f.respond(&["{}", r#"{"Marker": "m1"}"#, "{}"]);
    f.client.call("List", Value::Map(Map::new()))?;
    all(f.pages(MARKER, PaginationConfig::default())?)?;
    let agents: Vec<String> = f
        .transport
        .sent()
        .iter()
        .map(|(request, _)| {
            request
                .headers
                .get("User-Agent")
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    let features = |agent: &str| {
        let list = agent.split(' ').find_map(|part| part.strip_prefix("m/"));
        list.unwrap_or_default()
            .split(',')
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert!(
        !features(&agents[0]).contains(&"C".to_owned()),
        "{}",
        agents[0]
    );
    let mut paged = features(&agents[0]);
    paged.retain(|code| !code.is_empty());
    paged.insert(0, "C".into());
    assert_eq!(features(&agents[1]), paged);
    assert_eq!(agents[1], agents[2]);
    assert_eq!(
        agents[1].replace("m/C,", "m/").replace(" m/C ", " "),
        agents[0]
    );
    Ok(())
}

/// A page's call failure ends the pages with it; those before it stay
/// returned.
#[test]
fn a_page_s_call_failure_ends_the_pages() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Users": ["a"], "Marker": "m1"}"#]);
    f.transport.respond(HttpResponse {
        status: 400,
        headers: Headers::new(),
        body: br#"{"__type": "Boom", "message": "no"}"#.to_vec(),
    });
    let mut pages = f.pages(MARKER, PaginationConfig::default())?;
    assert!(matches!(pages.next(), Some(Ok(_))));
    let error = pages.next().ok_or("an error")?.err().ok_or("an error")?;
    let code = |error: &CallError| match error {
        CallError::Service(service) => service.code().map(str::to_owned),
        _ => None,
    };
    assert_eq!(code(&error).as_deref(), Some("Boom"), "{error:?}");
    assert!(pages.next().is_none());
    f.respond(&[r#"{"Users": ["a"], "Marker": "m1"}"#]);
    let failed = f
        .pages(MARKER, PaginationConfig::default())?
        .build_full_result();
    assert!(
        matches!(failed, Err(CallError::Transport { .. })),
        "{failed:?}"
    );
    Ok(())
}

/// The resume token at a page boundary is set once the page after it is
/// asked for, as the generator sets it on resuming.
#[test]
fn the_boundary_s_resume_token_waits_for_the_next_page() -> TestResult {
    let f = fixture()?;
    f.respond(&USERS_1_2_3);
    let mut pages = f.pages(MARKER, max_items(1))?;
    assert!(matches!(pages.next(), Some(Ok(_))));
    assert_eq!(pages.resume_token(), None);
    assert!(pages.next().is_none());
    assert_eq!(pages.resume_token(), Some(M1));
    Ok(())
}

/// A map result key has a `len`, but no slice: `dict[:1]` looks the slice
/// up, a `KeyError` printed as `slice(None, 1, None)` (apigateway
/// `GetUsage`'s `items`, on the Python route). The page is not returned.
#[test]
fn max_items_into_a_map_result_key_is_python_s_key_error() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Usage": {"a": ["1"], "b": ["2"]}, "Marker": "m1"}"#]);
    let config = r#"{"input_token": "Marker", "output_token": "Marker", "result_key": "Usage"}"#;
    let mut pages = f.pages(config, max_items(1))?;
    let error = pages.next().ok_or("an error")?.err().ok_or("an error")?;
    assert_eq!(error.to_string(), "slice(None, 1, None)");
    assert!(pages.next().is_none());
    Ok(())
}

fn b64(json: &str) -> String {
    pytext::base64::encode(json.as_bytes())
}

/// What the decoder raises reaches the caller; a token the decoder fails
/// on with a `ValueError` or `TypeError` is an old one.
#[test]
fn a_starting_token_fails_or_falls_back_as_python_s_does() -> TestResult {
    let f = fixture()?;
    let error = all(f.pages(MARKER, starting(&b64(r#""abc""#)))?)
        .err()
        .ok_or("an error")?;
    assert_eq!(error.to_string(), "'str' object has no attribute 'pop'");
    assert!(f.bodies().is_empty());
    f.respond(&[r#"{"Users": ["a"]}"#]);
    all(f.pages(MARKER, starting(&b64(r#"["x"]"#)))?)?;
    assert_eq!(f.bodies(), [r#"{"Marker": "WyJ4Il0="}"#]);
    Ok(())
}

/// A starting token's keys beyond the input tokens are sent too, and the
/// resume token refuses them (dynamodb `ListTables`, on the Python route).
#[test]
fn a_starting_token_s_other_keys_are_sent_and_then_refused() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Users": ["User1", "User2", "User3"], "Marker": "m2"}"#]);
    let token = b64(r#"{"Marker": "m1", "Filter": "x", "boto_truncate_amount": 1}"#);
    let pagination = PaginationConfig {
        starting_token: Some(token),
        ..max_items(1)
    };
    let pages = f.pages_of(MARKER, json(r#"{"Filter": "y"}"#)?, pagination)?;
    let error = all(pages).err().ok_or("an error")?;
    assert_eq!(
        error,
        CallError::Pagination(PaginationError::Value(
            "Bad starting token: {'Marker': 'm1', 'Filter': 'x', 'boto_truncate_amount': 2}".into()
        ))
    );
    assert_eq!(f.bodies(), [r#"{"Filter": "x", "Marker": "m1"}"#]);
    Ok(())
}

/// `MaxItems` of 0 keeps nothing of the first page, and a negative one
/// slices from the end, as Python's slices do.
#[test]
fn max_items_of_zero_or_less_slices_as_python_does() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Users": ["a", "b"], "Marker": "m1"}"#]);
    let full = f.pages(MARKER, max_items(0))?.build_full_result()?;
    let at_0 = "eyJNYXJrZXIiOiBudWxsLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAwfQ==";
    assert_eq!(
        full,
        json(&format!(r#"{{"Users": [], "NextToken": "{at_0}"}}"#))?
    );
    f.respond(&[r#"{"Users": ["a", "b", "c"], "Marker": "m1"}"#]);
    let full = f.pages(MARKER, max_items(-1))?.build_full_result()?;
    let at_minus_1 = "eyJNYXJrZXIiOiBudWxsLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAtMX0=";
    assert_eq!(
        full,
        json(&format!(
            r#"{{"Users": ["a", "b"], "NextToken": "{at_minus_1}"}}"#
        ))?
    );
    Ok(())
}

/// A starting token's offset of `null` cannot be added to.
#[test]
fn a_null_offset_is_python_s_type_error_on_truncation() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Marker": "m1"}"#]);
    let pagination = PaginationConfig {
        starting_token: Some(b64(r#"{"Marker": "m0", "boto_truncate_amount": null}"#)),
        ..max_items(-1)
    };
    let error = all(f.pages(MARKER, pagination)?).err().ok_or("an error")?;
    assert_eq!(
        error.to_string(),
        "unsupported operand type(s) for +: 'int' and 'NoneType'"
    );
    Ok(())
}

/// `int(max_items)` and kin, before any request.
#[test]
fn a_max_items_that_is_no_int_is_int_s_value_error() -> TestResult {
    let f = fixture()?;
    let pagination = PaginationConfig {
        max_items: Some(Value::String("abc".into())),
        ..PaginationConfig::default()
    };
    let error = all(f.pages(MARKER, pagination)?).err().ok_or("an error")?;
    assert_eq!(
        error,
        CallError::Pagination(PaginationError::Value(
            "invalid literal for int() with base 10: 'abc'".into()
        ))
    );
    let config = Config::from_value(&json(LIMITED)?).map_err(|r| format!("{r:?}"))?;
    let pagination = PaginationConfig {
        page_size: Some(Value::Integer(1)),
        ..PaginationConfig::default()
    };
    let bare = PageIterator::new(&f.client, "Bare", Value::Null, config, pagination);
    let error = all(bare).err().ok_or("an error")?;
    assert_eq!(
        error.to_string(),
        "'NoneType' object has no attribute 'members'"
    );
    assert!(f.bodies().is_empty());
    Ok(())
}

/// `first_input`: the first request's input, before and after it is sent,
/// with what `_inject_starting_params` put in; or the error the first page
/// fails with before its request. It sends nothing itself.
#[test]
fn the_first_input_is_the_first_request_s() -> TestResult {
    let f = fixture()?;
    let pagination = PaginationConfig {
        page_size: Some(Value::Integer(2)),
        ..starting(M1_AT_1)
    };
    let pages = f.pages_of(LIMITED, json(r#"{"Filter": "foo"}"#)?, pagination)?;
    let expected = json(r#"{"Filter": "foo", "Marker": "m1", "MaxResults": 2}"#)?;
    assert_eq!(pages.first_input()?, expected);
    assert!(f.bodies().is_empty());
    f.respond(&[
        r#"{"Users": ["a", "b"], "Marker": "m2"}"#,
        r#"{"Users": ["c"]}"#,
    ]);
    let mut pages = pages;
    all_of(&mut pages)?;
    assert_eq!(pages.first_input()?, expected);
    assert_eq!(
        f.bodies()[0],
        r#"{"Filter": "foo", "Marker": "m1", "MaxResults": 2}"#
    );
    let plain = f.pages(MARKER, PaginationConfig::default())?;
    assert_eq!(plain.first_input()?, json("{}")?);
    let bad = f.pages(MARKER, starting("does___not___work"))?;
    assert_eq!(
        bad.first_input(),
        Err(CallError::Pagination(PaginationError::Value(
            "Bad starting token: does___not___work".into()
        )))
    );
    assert_eq!(f.bodies().len(), 2);
    Ok(())
}

fn all_of(pages: &mut PageIterator<'_>) -> Result<Vec<Value>, CallError> {
    pages.collect()
}

/// devops-guru `GetResourceCollection`'s shape: result keys within the
/// non-aggregate key.
const NESTED: &str = r#"{"input_token": "Marker", "output_token": "Marker",
    "result_key": ["Inner.Deep.Names", "Inner.Tags"], "non_aggregate_keys": ["Inner"]}"#;

const NESTED_PAGES: [&str; 3] = [
    r#"{"Inner": {"Deep": {"Names": ["s1", "s2"]}, "Tags": [{"Key": "k", "Values": ["t"]}]},
        "Marker": "n1"}"#,
    r#"{"Inner": {"Deep": {"Names": ["s3"]}}, "Marker": "n2"}"#,
    r#"{"Inner": {"Deep": {"Names": ["s4"]}, "Tags": [{"Key": "k", "Values": ["u"]}]}}"#,
];

/// botocore's non-aggregate part holds the first page's own objects, and
/// the full result extends that page's lists in place: merging the part
/// back gives the joined lists, not the first page's.
#[test]
fn a_result_key_within_a_non_aggregate_key_keeps_every_page() -> TestResult {
    let f = fixture()?;
    f.respond(&NESTED_PAGES);
    let mut pages = f.pages(NESTED, PaginationConfig::default())?;
    let full = pages.build_full_result()?;
    let joined = r#"{"Inner": {"Deep": {"Names": ["s1", "s2", "s3", "s4"]},
        "Tags": [{"Key": "k", "Values": ["t"]}, {"Key": "k", "Values": ["u"]}]}}"#;
    assert_eq!(full, json(joined)?);
    assert_eq!(
        Value::Map(pages.non_aggregate_part().clone()),
        json(joined)?
    );
    f.respond(&NESTED_PAGES);
    let full = f.pages(NESTED, max_items(3))?.build_full_result()?;
    let n2 = "eyJNYXJrZXIiOiAibjIifQ==";
    let expected = format!(
        r#"{{"Inner": {{"Deep": {{"Names": ["s1", "s2", "s3"]}},
            "Tags": [{{"Key": "k", "Values": ["t"]}}]}}, "NextToken": "{n2}"}}"#
    );
    assert_eq!(full, json(&expected)?);
    Ok(())
}

/// `_truncate_response` sets the cut list in the first page's own dict,
/// which the non-aggregate part shares: both the full result and the part
/// the text output reads after the first page see it cut.
#[test]
fn a_truncated_first_page_is_cut_within_the_non_aggregate_part() -> TestResult {
    let f = fixture()?;
    f.respond(&NESTED_PAGES);
    let mut pages = f.pages(NESTED, max_items(1))?;
    assert!(matches!(pages.next(), Some(Ok(_))));
    let cut = r#"{"Inner": {"Deep": {"Names": ["s1"]}, "Tags": [{"Key": "k", "Values": ["t"]}]}}"#;
    assert_eq!(Value::Map(pages.non_aggregate_part().clone()), json(cut)?);
    let f = fixture()?;
    f.respond(&NESTED_PAGES);
    let full = f.pages(NESTED, max_items(1))?.build_full_result()?;
    let expected = format!(
        r#"{{"Inner": {{"Deep": {{"Names": ["s1"]}}, "Tags": [{{"Key": "k", "Values": ["t"]}}]}},
            "NextToken": "{NONE_AT_1}"}}"#
    );
    assert_eq!(full, json(&expected)?);
    Ok(())
}

/// A starting token's offset slices the first page's list in its own dict,
/// and the secondary key emptied there is the list later pages extend.
#[test]
fn a_resumed_first_page_is_sliced_within_the_non_aggregate_part() -> TestResult {
    let f = fixture()?;
    let n0_at_1 = "eyJNYXJrZXIiOiAibjAiLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAxfQ==";
    f.respond(&NESTED_PAGES);
    let mut pages = f.pages(NESTED, starting(n0_at_1))?;
    assert!(matches!(pages.next(), Some(Ok(_))));
    let sliced = r#"{"Inner": {"Deep": {"Names": ["s2"]}, "Tags": []}}"#;
    assert_eq!(
        Value::Map(pages.non_aggregate_part().clone()),
        json(sliced)?
    );
    let f = fixture()?;
    f.respond(&NESTED_PAGES);
    let full = f.pages(NESTED, starting(n0_at_1))?.build_full_result()?;
    let expected = r#"{"Inner": {"Deep": {"Names": ["s2", "s3", "s4"]},
        "Tags": [{"Key": "k", "Values": ["u"]}]}}"#;
    assert_eq!(full, json(expected)?);
    // The first page lacks `Tags`: `_handle_first_request` sets it `None`
    // in the page's dict, which the merge then writes over page 3's.
    f.respond(&NESTED_PAGES[1..]);
    let n1_at_1 = "eyJNYXJrZXIiOiAibjEiLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAxfQ==";
    let full = f.pages(NESTED, starting(n1_at_1))?.build_full_result()?;
    assert_eq!(
        full,
        json(r#"{"Inner": {"Deep": {"Names": ["s4"]}, "Tags": null}}"#)?
    );
    Ok(())
}

/// A non-aggregate key that is a result key holds the first page's list:
/// joined in place, unless truncation replaced it in the page, which
/// leaves the part the list as it came.
#[test]
fn a_non_aggregate_key_that_is_a_result_key_is_the_first_page_s_list() -> TestResult {
    let config = r#"{"input_token": "Marker", "output_token": "Marker",
        "result_key": ["Users", "Groups"], "non_aggregate_keys": ["Users", "Groups"]}"#;
    let pages = [
        r#"{"Users": ["a", "b"], "Groups": ["g1"], "Marker": "m1"}"#,
        r#"{"Users": ["c"], "Groups": ["g2"]}"#,
    ];
    let f = fixture()?;
    f.respond(&pages);
    let full = f
        .pages(config, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(
        full,
        json(r#"{"Users": ["a", "b", "c"], "Groups": ["g1", "g2"]}"#)?
    );
    f.respond(&pages);
    let full = f.pages(config, max_items(1))?.build_full_result()?;
    let expected =
        format!(r#"{{"Users": ["a", "b"], "Groups": ["g1"], "NextToken": "{NONE_AT_1}"}}"#);
    assert_eq!(full, json(&expected)?);
    f.respond(&pages[1..]);
    let m1_at_0 = "eyJNYXJrZXIiOiAibTEiLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAwfQ==";
    let full = f.pages(config, starting(m1_at_0))?.build_full_result()?;
    assert_eq!(full, json(r#"{"Users": ["c"], "Groups": []}"#)?);
    Ok(())
}

/// A scalar result key within a non-aggregate key is replaced in the full
/// result, not in the page, so the merge writes the first page's back.
#[test]
fn a_scalar_within_a_non_aggregate_key_is_the_first_page_s() -> TestResult {
    let config = r#"{"input_token": "Marker", "output_token": "Marker",
        "result_key": ["Inner.Items", "Inner.Note"], "non_aggregate_keys": ["Inner"]}"#;
    let f = fixture()?;
    f.respond(&[
        r#"{"Inner": {"Items": ["a"], "Note": "x"}, "Marker": "m1"}"#,
        r#"{"Inner": {"Items": ["b"], "Note": "y"}}"#,
    ]);
    let full = f
        .pages(config, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(
        full,
        json(r#"{"Inner": {"Items": ["a", "b"], "Note": "x"}}"#)?
    );
    f.respond(&[
        r#"{"Inner": {"Note": "x"}, "Marker": "m1"}"#,
        r#"{"Inner": {"Items": ["b"], "Note": "y"}}"#,
    ]);
    let full = f
        .pages(config, PaginationConfig::default())?
        .build_full_result()?;
    assert_eq!(full, json(r#"{"Inner": {"Note": "x", "Items": ["b"]}}"#)?);
    f.respond(&[r#"{"Inner": {"Note": "x"}, "Marker": "m1"}"#]);
    let mut pages = f.pages(config, max_items(-1))?;
    let full = pages.build_full_result()?;
    let at_minus_1 = "eyJNYXJrZXIiOiBudWxsLCAiYm90b190cnVuY2F0ZV9hbW91bnQiOiAtMX0=";
    let expected =
        format!(r#"{{"Inner": {{"Items": [], "Note": "x"}}, "NextToken": "{at_minus_1}"}}"#);
    assert_eq!(full, json(&expected)?);
    assert_eq!(
        Value::Map(pages.non_aggregate_part().clone()),
        json(r#"{"Inner": {"Note": "x", "Items": []}}"#)?
    );
    Ok(())
}

/// A `NaN` offset is `json.loads`' float, which the first page cannot be
/// sliced from (kms `ListAliases`, on the Python route); a token only
/// Python can read is unsupported before any request.
#[test]
fn a_token_python_reads_differently_is_read_as_python_does() -> TestResult {
    let f = fixture()?;
    f.respond(&[r#"{"Users": ["a", "b"], "Marker": "m2"}"#]);
    let nan = b64(r#"{"Marker": "m1", "boto_truncate_amount": NaN}"#);
    let error = all(f.pages(MARKER, starting(&nan))?)
        .err()
        .ok_or("an error")?;
    assert_eq!(
        error.to_string(),
        "slice indices must be integers or None or have an __index__ method"
    );
    assert_eq!(f.bodies(), [r#"{"Marker": "m1"}"#]);
    let lone = f.pages(MARKER, starting(&b64(r#"{"Marker": "\ud800"}"#)))?;
    assert!(matches!(
        lone.first_input(),
        Err(CallError::Unsupported(
            crate::admission::Unsupported::StartingToken(_)
        ))
    ));
    assert_eq!(f.bodies().len(), 1);
    Ok(())
}

/// `int()` as `_extract_paging_params` applies it to `MaxItems` and
/// `PageSize`: text with any Unicode decimal digits and spaces, as
/// CPython's `PyLong_FromUnicodeObject` reads it, and a float of any size
/// truncated exactly.
#[test]
fn paging_params_are_read_as_python_s_int_reads_them() {
    use super::py::int;
    assert_eq!(
        int(&Value::String("\u{661}\u{662}".into())),
        Ok(Value::Integer(12))
    );
    assert_eq!(
        int(&Value::String("\u{a0}7 ".into())),
        Ok(Value::Integer(7))
    );
    assert_eq!(
        int(&Value::Float(-2.5e38)),
        Ok(Value::BigInteger(
            "-249999999999999984927291592900794646528".into()
        ))
    );
}
