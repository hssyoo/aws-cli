//! Each façade function as a typed `fn` pointer, each struct by an
//! exhaustive destructuring with typed fields, each enum by an exhaustive
//! match. A few behaviours the frozen harnesses lean on are checked too.
use client::http::Method;
use client::testing::facade::{
    self, BlobHandler, CallError, Client, ClientCase, ClientSettings, Credentials, Customization,
    Distribution, EndpointCase, EndpointData, Headers, Host, HttpRequest, HttpResponse,
    InMemoryTransport, Keys, MemberInfo, Model, PaginationCase, ParseError, ParseOptions,
    ProtocolHandle, SerializeError, Serialized, ServiceError, StartingToken, Timestamp,
    TimestampHandler, UserAgentCase, Validated, Versions,
};
use model::source::{DirSource, Source};
use model::value::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

type Result<T> = Out<T, Box<dyn std::error::Error>>;
type Out<T, E> = std::result::Result<T, E>;
type Documents<'a> = Option<(&'a [u8], &'a str)>;
type Pages = Out<Value, CallError>;
type Called = (Value, u32);
type Paginate = fn(&Client, &dyn Source, &str, &str, Value, PaginationCase) -> Out<Pages, String>;

type Refusals = Vec<(String, Option<(String, String)>)>;

#[test]
fn every_function_keeps_its_signature() {
    let _: fn(&dyn Source, &str) -> Out<Arc<Model>, String> = facade::load_model;
    let _: fn(&[u8]) -> Out<Arc<Model>, String> = facade::model_from_json;
    let _: fn(&Model) -> Vec<String> = facade::operation_names;
    let _: fn(&Model) -> Option<String> = facade::endpoint_prefix;
    let _: fn(&Model, &str) -> Option<String> = facade::error_shape_name;
    let _: fn(&Model, &str) -> Vec<MemberInfo> = facade::input_members;
    let _: fn(&Model, &str, &Value) -> Out<Validated, String> = facade::validate_case;
    let _: fn(&str) -> Option<ProtocolHandle> = facade::protocol_named;
    let _: fn(&Model) -> Option<ProtocolHandle> = facade::protocol_for_model;
    let _: fn(ProtocolHandle, &Model, &str, &Value) -> Out<Serialized, SerializeError> =
        facade::serialize_case;
    let _: fn(
        ProtocolHandle,
        &Model,
        &str,
        &HttpResponse,
        &ParseOptions<'_>,
    ) -> Out<Value, ParseError> = facade::parse_case;
    let _: fn(
        ProtocolHandle,
        &Model,
        &str,
        &HttpResponse,
        &ParseOptions<'_>,
    ) -> Out<ServiceError, ParseError> = facade::parse_error_case;
    let _: fn(&dyn Source, &str, &Model) -> Out<EndpointData, String> = facade::endpoint_data;
    let _: fn(Vec<u8>, Vec<u8>, Documents<'_>) -> Out<EndpointData, String> =
        facade::endpoint_data_from_documents;
    let _: fn(&dyn Source, &str, &[&'static Customization]) -> Out<Refusals, String> =
        facade::refusals;
    let _: fn(ClientCase) -> Out<Client, CallError> = facade::client_case;
    let _: fn(&Client, &str, Value) -> Out<Called, CallError> = facade::call_case;
    let _: Paginate = facade::paginate_case;
    let _: fn(&[u8]) -> bool = facade::xml_parses;
    let _: fn(&str, &[String]) -> Out<StartingToken, CallError> = facade::parse_starting_token;
    let _: fn(&Map, &[String]) -> Out<String, String> = facade::resume_token;
    let _: &str = facade::TRUNCATE_AMOUNT;
    let _: fn(&[Arc<Model>], &[u8]) = client::testing::fuzz::responses;
    let _: fn(&[u8]) = client::testing::fuzz::xml;
    let _: fn(&[u8]) = client::testing::fuzz::starting_token;
    let _: fn(&[u8]) = client::testing::fuzz::model_json;
    let _: fn(&Model, &[u8]) = client::testing::fuzz::validate;
    let _: fn(TimestampHandler<'static>, BlobHandler<'static>) -> ParseOptions<'static> =
        facade::parse_options;
    let _: fn() -> ParseOptions<'static> = facade::botocore_options;
    let _: fn(&Value) -> Out<Value, ParseError> = facade::botocore_timestamp;
    let _: fn(&Value) -> Out<Value, ParseError> = facade::botocore_blob;
}

fn client_case_fields(case: ClientCase) {
    let ClientCase {
        service,
        model,
        endpoints,
        endpoint,
        settings,
        credentials,
        transport,
        now,
        random,
        customizations,
        user_agent,
    } = case;
    let _: (String, Arc<Model>, EndpointData, EndpointCase) = (service, model, endpoints, endpoint);
    let _: (ClientSettings, Credentials, Arc<InMemoryTransport>) =
        (settings, credentials, transport);
    let _: (
        Timestamp,
        f64,
        &'static [&'static Customization],
        UserAgentCase,
    ) = (now, random, customizations, user_agent);
}

fn other_fields(
    endpoint: EndpointCase,
    agent: UserAgentCase,
    member: MemberInfo,
    pagination: PaginationCase,
    token: StartingToken,
    keys: Keys,
) {
    let EndpointCase {
        region,
        endpoint_url,
        use_fips,
        use_dual_stack,
    } = endpoint;
    let _: (Option<String>, Option<String>, Option<bool>, Option<bool>) =
        (region, endpoint_url, use_fips, use_dual_stack);
    let UserAgentCase {
        host,
        distribution,
        versions,
        installer,
        prompt,
        command,
    } = agent;
    let _: (Host, Option<Distribution>, Versions<'static>) = (host, distribution, versions);
    let _: (&'static str, &'static str, String) = (installer, prompt, command);
    let MemberInfo {
        name,
        location,
        location_name,
        type_name,
        item_type_name,
    } = member;
    let _: (String, Option<String>, Option<String>) = (name, location, location_name);
    let _: (String, Option<String>) = (type_name, item_type_name);
    let PaginationCase {
        max_items,
        page_size,
        starting_token,
    } = pagination;
    let _: (Option<Value>, Option<Value>, Option<String>) = (max_items, page_size, starting_token);
    let StartingToken { token, index } = token;
    let _: (Map, Value) = (token, index);
    let Keys {
        access_key_id,
        secret_access_key,
        session_token,
        account_id,
    } = keys;
    let _: (String, String) = (access_key_id, secret_access_key);
    let _: (Option<String>, Option<String>) = (session_token, account_id);
}

fn enum_variants(validated: &Validated, credentials: &Credentials) -> usize {
    let first = match validated {
        Validated::Valid => 0,
        Validated::Invalid { message, report } => message.len() + report.len(),
        Validated::Undecided(reason) => reason.len(),
    };
    let second = match credentials {
        Credentials::Static { keys, method } => {
            keys.access_key_id.len() + method.map_or(0, str::len)
        }
        Credentials::Absent => 0,
        Credentials::Failing(text) => text.len(),
        Credentials::Counting { keys, loads } => {
            keys.access_key_id.len() + loads.load(std::sync::atomic::Ordering::SeqCst)
        }
    };
    first + second
}

#[test]
fn every_type_keeps_its_fields_and_variants() {
    let _: fn(ClientCase) = client_case_fields;
    let _: fn(EndpointCase, UserAgentCase, MemberInfo, PaginationCase, StartingToken, Keys) =
        other_fields;
    let _: fn(&Validated, &Credentials) -> usize = enum_variants;
    let _: (Headers, Option<HttpRequest>, Option<AtomicUsize>) = (Headers::default(), None, None);
}

fn fake(service: &str) -> Result<Arc<Model>> {
    let models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/models");
    Ok(facade::load_model(&DirSource::new(models), service)?)
}

#[test]
fn the_protocols_are_named_as_models_name_them() -> Result<()> {
    for name in ["query", "json", "rest-json"] {
        assert!(facade::protocol_named(name).is_some(), "{name}");
    }
    assert!(facade::protocol_named("rest-xml").is_none());
    let model = fake("zquery")?;
    assert!(facade::protocol_for_model(&model).is_some());
    Ok(())
}

#[test]
fn input_members_name_their_types_and_traits() -> Result<()> {
    let model = fake("zrestjson")?;
    let mut types = Vec::new();
    for operation in facade::operation_names(&model) {
        for member in facade::input_members(&model, &operation) {
            types.push(member.type_name.clone());
            if member.type_name == "list" {
                assert!(member.item_type_name.is_some(), "{}", member.name);
            } else {
                assert_eq!(member.item_type_name, None, "{}", member.name);
            }
        }
    }
    assert!(types.iter().any(|kind| kind == "list"));
    assert!(types.iter().any(|kind| kind == "string"));
    assert!(facade::input_members(&model, "NoSuchOperation").is_empty());
    Ok(())
}

#[test]
fn validation_reports_carry_both_texts() -> Result<()> {
    let model = fake("zjson")?;
    let mut invalid = 0;
    for operation in facade::operation_names(&model) {
        let input = Value::Map(Map::new());
        if let Validated::Invalid { message, report } =
            facade::validate_case(&model, &operation, &input)?
        {
            assert_eq!(message, format!("Parameter validation failed:\n{report}"));
            invalid += 1;
        }
    }
    assert!(invalid > 0);
    assert!(facade::validate_case(&model, "NoSuchOperation", &Value::Null).is_err());
    Ok(())
}

#[test]
fn a_starting_token_round_trips_through_the_resume_token() -> Result<()> {
    let names = vec!["NextToken".to_owned()];
    let mut value = Map::new();
    value.insert("NextToken".into(), Value::String("abc".into()));
    let encoded = facade::resume_token(&value, &names)?;
    let parsed = facade::parse_starting_token(&encoded, &names).map_err(|e| format!("{e:?}"))?;
    assert_eq!(parsed.token, value);
    assert_eq!(parsed.index, Value::Integer(0));
    assert!(facade::xml_parses(b"<a/>"));
    assert!(!facade::xml_parses(b"<a>"));
    Ok(())
}

fn copy<T: Copy>() {}

/// The fields of the product types the frozen harnesses read or build.
fn read_fields(serialized: &Serialized, error: &ServiceError) {
    let _: (&Method, &str, &str) = (&serialized.method, &serialized.path, &serialized.query);
    let _: (&Headers, &Vec<u8>, &Option<String>) = (
        &serialized.headers,
        &serialized.body,
        &serialized.host_prefix,
    );
    let _: &str = serialized.method.as_str();
    let _: (&Map, &Map, &Option<String>) = (&error.error, &error.members, &error.request_id);
    let _: String = error.to_string();
}

#[test]
fn the_types_harnesses_read_keep_their_fields() {
    copy::<ProtocolHandle>();
    copy::<ParseOptions<'static>>();
    let _: fn(&Serialized, &ServiceError) = read_fields;
    let HttpResponse {
        status,
        headers,
        body,
    } = HttpResponse {
        status: 200,
        headers: Headers::default(),
        body: Vec::new(),
    };
    let _: (u16, Headers, Vec<u8>) = (status, headers, body);
}

/// One header member per shape type, and a timestamp list, as an inline
/// model: the type text `input_members` gives is what the frozen
/// `list_headers` compares.
const EVERY_TYPE: &str = r#"{
  "metadata": {"apiVersion": "2020-01-01", "endpointPrefix": "ztypes", "protocol": "rest-json",
    "protocols": ["rest-json"], "serviceFullName": "Types", "serviceId": "Ztypes",
    "signatureVersion": "v4", "uid": "ztypes-2020-01-01"},
  "operations": {"Put": {"name": "Put", "http": {"method": "PUT", "requestUri": "/"},
    "input": {"shape": "PutRequest"}}},
  "shapes": {
    "PutRequest": {"type": "structure", "members": {
      "Stamps": {"shape": "Stamps", "location": "header", "locationName": "x-stamps"},
      "Names": {"shape": "Names", "location": "header", "locationName": "x-names"},
      "Nested": {"shape": "Nested"},
      "Tags": {"shape": "Tags", "location": "headers", "locationName": "x-tag-"},
      "Text": {"shape": "Text", "location": "querystring", "locationName": "text"},
      "Int": {"shape": "Int", "location": "header", "locationName": "x-int"},
      "Big": {"shape": "Big"},
      "Flag": {"shape": "Flag"},
      "Ratio": {"shape": "Ratio"},
      "Precise": {"shape": "Precise"},
      "At": {"shape": "At", "location": "header", "locationName": "x-at"},
      "Data": {"shape": "Data", "location": "uri", "locationName": "data"},
      "Odd": {"shape": "Odd"}}},
    "Stamps": {"type": "list", "member": {"shape": "At"}},
    "Names": {"type": "list", "member": {"shape": "Text"}},
    "Nested": {"type": "structure", "members": {}},
    "Tags": {"type": "map", "key": {"shape": "Text"}, "value": {"shape": "Text"}},
    "Text": {"type": "string"},
    "Int": {"type": "integer"},
    "Big": {"type": "long"},
    "Flag": {"type": "boolean"},
    "Ratio": {"type": "float"},
    "Precise": {"type": "double"},
    "At": {"type": "timestamp"},
    "Data": {"type": "blob"},
    "Odd": {"type": "fancy"}}
}"#;

#[test]
fn input_members_give_botocore_s_type_names_and_locations() -> Result<()> {
    let model = facade::model_from_json(EVERY_TYPE.as_bytes())?;
    let text = |value: &str| Some(value.to_owned());
    let member =
        |name: &str, kind: &str, item: Option<&str>, location: &str, location_name: &str| {
            MemberInfo {
                name: name.to_owned(),
                location: (!location.is_empty()).then(|| location.to_owned()),
                location_name: (!location_name.is_empty()).then(|| location_name.to_owned()),
                type_name: kind.to_owned(),
                item_type_name: item.and_then(text),
            }
        };
    let expected = vec![
        member("Stamps", "list", Some("timestamp"), "header", "x-stamps"),
        member("Names", "list", Some("string"), "header", "x-names"),
        member("Nested", "structure", None, "", ""),
        member("Tags", "map", None, "headers", "x-tag-"),
        member("Text", "string", None, "querystring", "text"),
        member("Int", "integer", None, "header", "x-int"),
        member("Big", "long", None, "", ""),
        member("Flag", "boolean", None, "", ""),
        member("Ratio", "float", None, "", ""),
        member("Precise", "double", None, "", ""),
        member("At", "timestamp", None, "header", "x-at"),
        member("Data", "blob", None, "uri", "data"),
        member("Odd", "fancy", None, "", ""),
    ];
    assert_eq!(facade::input_members(&model, "Put"), expected);
    Ok(())
}
