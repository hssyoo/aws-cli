//! The differential over every case source the card names: botocore's
//! endpoint rule-set tests (the S12 suite's cases, provider and
//! end-to-end), the client's oracle settings (`endpoint/tests.rs`), the
//! parity cases, and parameter combinations generated for every shipped
//! rule set, unset, wrong-typed and undeclared parameters among them.
//! Each source fails on any finding and lists every one; the comparison
//! itself is attacked at the end.
use super::*;
use crate::endpoint::{Builtins, EndpointConfig, EndpointInputs, ParamValue as P, provider_params};
use model::shapes::Model;
use model::source::{DirSource, Kind, Source, load_data_bytes, load_service_bytes};
use model::value::{Map, Value};
use serde_json::Value as Json;
use std::path::PathBuf;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn botocore() -> DirSource {
    DirSource::new(repository().join("aws-cli/awscli/botocore/data"))
}

/// The cases one source resolved, and the findings among them.
#[derive(Default)]
struct Run {
    cases: usize,
    findings: Vec<String>,
}

impl Run {
    fn check(
        &mut self,
        case: impl FnOnce() -> String,
        data: &EndpointData,
        native: &Native,
        params: &EndpointParams,
    ) {
        self.cases += 1;
        self.findings
            .extend(compare(case, data, native, params).map(|finding| finding.to_string()));
    }
    /// Every finding, written out in full and shown in the failure.
    fn finish(self, source: &str, minimum: usize) -> TestResult {
        let path = std::env::temp_dir().join(format!("endpoint-differential-{source}.txt"));
        let mut text = format!(
            "# {source}: {} cases, {} findings\n",
            self.cases,
            self.findings.len()
        );
        for finding in &self.findings {
            text.push_str(finding);
        }
        std::fs::write(&path, &text)?;
        assert!(
            self.cases >= minimum,
            "{source}: {} cases, fewer than {minimum}",
            self.cases
        );
        if !self.findings.is_empty() {
            let shown: String = text.lines().take(60).collect::<Vec<_>>().join("\n");
            return Err(format!(
                "{} findings, all in {}:\n{shown}",
                self.findings.len(),
                path.display()
            )
            .into());
        }
        Ok(())
    }
}

/// A service's model and endpoint data from botocore's data.
fn service(source: &DirSource, name: &str) -> TestResult<(Model, EndpointData)> {
    let model = Model::load(source, name)?;
    let data = EndpointData::load(source, name, model.schema().service())?;
    Ok((model, data))
}

fn params_of(pairs: impl IntoIterator<Item = (String, P)>) -> EndpointParams {
    let mut params = EndpointParams::default();
    for (name, value) in pairs {
        params.set(&name, value);
    }
    params
}

/// A case's JSON parameter; an error for a kind no rule set declares.
fn json_param(name: &str, value: &Json) -> TestResult<(String, P)> {
    let value = match value {
        Json::String(text) => P::String(text.clone()),
        Json::Bool(flag) => P::Boolean(*flag),
        Json::Array(items) => P::StringArray(
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<_>>()
                .ok_or_else(|| format!("{name}: {value}"))?,
        ),
        _ => return Err(format!("{name}: a parameter of an unknown kind, {value}").into()),
    };
    Ok((name.to_owned(), value))
}

fn json_params(object: Option<&Json>) -> TestResult<Vec<(String, P)>> {
    let Some(object) = object.and_then(Json::as_object) else {
        return Ok(Vec::new());
    };
    object
        .iter()
        .map(|(name, value)| json_param(name, value))
        .collect()
}

/// S12's case and input counts, from `differences.tsv`'s header: the
/// differential must read the very cases the suite reads, the inputs
/// botocore skips included.
fn suite_counts() -> TestResult<(usize, usize)> {
    let tsv = repository().join("crates/customizations/tests/endpoint_suite/differences.tsv");
    let text = std::fs::read_to_string(tsv)?;
    let number = |prefix: &str| -> TestResult<usize> {
        let line = text
            .lines()
            .find_map(|line| line.strip_prefix(prefix))
            .ok_or(prefix.to_owned())?;
        Ok(line.split_whitespace().next().unwrap_or_default().parse()?)
    };
    let skipped: usize = text
        .lines()
        .find_map(|line| line.split("(skipped ").nth(1))
        .and_then(|rest| rest.trim_end_matches(')').parse().ok())
        .ok_or("skipped")?;
    Ok((
        number("# provider cases ")?,
        number("# e2e inputs ")? + skipped,
    ))
}

/// Every case of botocore's `endpoint-tests-1.json` files: its `params`
/// straight to both engines, and each `operationInputs` through the
/// client's provider parameters.
#[test]
#[cfg_attr(miri, ignore = "runs the CRT, which Miri cannot")]
fn the_endpoint_suite_resolves_alike() -> TestResult {
    let source = botocore();
    let dir = repository().join("aws-cli/tests/functional/botocore/endpoint-rules");
    let mut services = source.services(Kind::EndpointRuleSet)?;
    services.sort();
    let (mut run, mut inputs) = (Run::default(), 0);
    for name in &services {
        let file: Json = serde_json::from_slice(&std::fs::read(
            dir.join(name).join("endpoint-tests-1.json"),
        )?)?;
        let (model, data) = service(&source, name)?;
        let native = Native::new(&data);
        for (index, case) in file["testCases"]
            .as_array()
            .ok_or("testCases")?
            .iter()
            .enumerate()
        {
            let params = params_of(json_params(case.get("params"))?);
            run.check(
                || format!("{name} provider {index}"),
                &data,
                &native,
                &params,
            );
            for (k, input) in case["operationInputs"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                inputs += 1;
                let mut builtins = Builtins::default();
                let mut account_id = None;
                for (builtin, value) in json_params(input.get("builtInParams"))? {
                    match (builtin.as_str(), value) {
                        ("AWS::Auth::AccountId", P::String(id)) => account_id = Some(id),
                        (builtin, value) => builtins.set(builtin, value),
                    }
                }
                let operation = input["operationName"].as_str().unwrap_or_default();
                let operation = (model.schema().service().operation(operation))
                    .ok_or_else(|| format!("{name} {index}: no operation {operation}"))?;
                let empty = Json::Object(serde_json::Map::new());
                let op_params = input.get("operationParams").unwrap_or(&empty);
                let value = Value::from_json(op_params.to_string().as_bytes())?;
                let params =
                    provider_params(&data, operation, &value, &builtins, account_id.as_deref());
                run.check(
                    || format!("{name} e2e.{k} {index}"),
                    &data,
                    &native,
                    &params,
                );
            }
        }
    }
    let (cases, expected_inputs) = suite_counts()?;
    assert_eq!(
        (run.cases - inputs, inputs),
        (cases, expected_inputs),
        "S12's counts"
    );
    run.finish("suite", cases)
}

/// One client's endpoint parameters for `operation`, as a call makes them
/// with an empty input; `None` when the client refuses before resolving.
fn client_params(
    model: &Model,
    data: &EndpointData,
    operation: &str,
    inputs: &EndpointInputs<'_>,
    input: &Value,
    account_id: Option<&str>,
) -> TestResult<Option<EndpointParams>> {
    let operation = (model.schema().service().operation(operation)).ok_or(operation.to_owned())?;
    let Ok(builtins) = EndpointConfig::new(inputs).builtins(data) else {
        return Ok(None);
    };
    Ok(Some(provider_params(
        data, operation, input, &builtins, account_id,
    )))
}

/// The oracle's regions, and more of each partition.
const ORACLE_REGIONS: [Option<&str>; 20] = [
    None,
    Some("us-east-1"),
    Some("us-west-2"),
    Some("eu-west-1"),
    Some("cn-north-1"),
    Some("us-gov-west-1"),
    Some("aws-global"),
    Some("fips-us-east-1"),
    Some("us-east-1-fips"),
    Some("us-east-9"),
    Some("mars-1"),
    Some("us-iso-east-1"),
    Some("eu-isoe-west-1"),
    Some("us-isob-east-1"),
    Some("us-isof-south-1"),
    Some("eusc-de-east-1"),
    Some("fips-us-west-2"),
    Some("fips-eu-isoe-west-1"),
    Some("us-gov-east-1"),
    Some("cn-northwest-1"),
];
/// The oracle's endpoint URLs.
const ORACLE_URLS: [Option<&str>; 4] = [
    None,
    Some("https://sts.example.com/base"),
    Some("http://localhost:4566"),
    Some("http://localhost:8000"),
];

/// The settings of `endpoint/tests.rs`'s botocore oracle, crossed: its
/// services and operations, every region it names, FIPS and dual-stack
/// unset or on, and its endpoint URLs.
#[test]
#[cfg_attr(miri, ignore = "runs the CRT, which Miri cannot")]
fn the_client_oracle_settings_resolve_alike() -> TestResult {
    let source = botocore();
    let mut run = Run::default();
    for (name, operation) in [
        ("sts", "GetCallerIdentity"),
        ("dynamodb", "ListTables"),
        ("iam", "ListUsers"),
        ("b2bi", "ListCapabilities"),
    ] {
        let (model, data) = service(&source, name)?;
        let native = Native::new(&data);
        for region in ORACLE_REGIONS {
            for (use_fips, use_dual_stack) in [
                (None, None),
                (Some(true), None),
                (None, Some(true)),
                (Some(true), Some(true)),
            ] {
                for endpoint_url in ORACLE_URLS {
                    let inputs = EndpointInputs {
                        region,
                        use_fips,
                        use_dual_stack,
                        endpoint_url,
                        signing: true,
                    };
                    let input = Value::Map(Map::new());
                    for account_id in [None, Some("123456789012")] {
                        let Some(params) =
                            client_params(&model, &data, operation, &inputs, &input, account_id)?
                        else {
                            continue;
                        };
                        run.check(
                            || format!("{name} {operation} {inputs:?} {account_id:?}"),
                            &data,
                            &native,
                            &params,
                        );
                    }
                }
            }
        }
    }
    run.finish("oracle", 4 * 19 * 4 * 4 * 2)
}

/// Every case of `tests/parity/cases`, with its settings as the parity
/// harness gives them to the client (region `us-west-2` unless set), with
/// and without an account id.
#[test]
#[cfg_attr(miri, ignore = "runs the CRT, which Miri cannot")]
fn the_parity_cases_resolve_alike() -> TestResult {
    let (real, fake) = (
        botocore(),
        DirSource::new(repository().join("tests/models")),
    );
    let partitions = load_data_bytes(&real, "partitions")?;
    let endpoints = load_data_bytes(&real, "endpoints")?;
    let mut run = Run::default();
    let mut files: Vec<PathBuf> = std::fs::read_dir(repository().join("tests/parity/cases"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    files.sort();
    for file in files {
        let cases: Json = serde_json::from_slice(&std::fs::read(&file)?)?;
        for case in cases.as_array().ok_or("cases")? {
            let name = case["service"].as_str().ok_or("service")?;
            let (model, data) = if case["models"] == "fake" {
                let model = Model::load(&fake, name)?;
                let (_, rules) = load_service_bytes(&fake, name, Kind::EndpointRuleSet)?;
                let prefix = model
                    .schema()
                    .service()
                    .endpoint_prefix()
                    .unwrap_or(name)
                    .to_owned();
                let data = EndpointData::from_documents(
                    rules,
                    partitions.clone(),
                    Some((&endpoints, &prefix)),
                )?;
                (model, data)
            } else {
                service(&real, name)?
            };
            let native = Native::new(&data);
            let settings = &case["settings"];
            let inputs = EndpointInputs {
                region: Some(settings["region"].as_str().unwrap_or("us-west-2")),
                use_fips: Some(settings["fips"].as_bool().unwrap_or(false)),
                use_dual_stack: Some(settings["dualstack"].as_bool().unwrap_or(false)),
                endpoint_url: settings["endpoint_url"].as_str(),
                signing: true,
            };
            let input = Value::from_json(case["input"].to_string().as_bytes())?;
            let operation = case["operation"].as_str().ok_or("operation")?;
            for account_id in [None, Some("123456789012")] {
                let params = client_params(&model, &data, operation, &inputs, &input, account_id)?
                    .ok_or_else(|| format!("{}: refused", case["name"]))?;
                run.check(
                    || format!("{} {account_id:?}", case["name"]),
                    &data,
                    &native,
                    &params,
                );
            }
        }
    }
    run.finish("parity", 2 * 80)
}

const REGIONS: [&str; 17] = [
    "us-east-1",
    "us-west-2",
    "cn-north-1",
    "us-gov-west-1",
    "us-iso-east-1",
    "us-isob-east-1",
    "eu-isoe-west-1",
    "us-isof-south-1",
    "eusc-de-east-1",
    "aws-global",
    "mars-1",
    "",
    "Invalid_Region",
    "us-east-1.amazonaws.com",
    "fips-us-east-1",
    "a",
    "us-east-1 ",
];

const STRINGS: [&str; 14] = [
    "",
    "https://example.com",
    "http://localhost:4566/path?x=1",
    "https://[::1]:443/a%2Fb",
    "not a url",
    "arn:aws:s3:us-west-2:123456789012:accesspoint:ap",
    "arn:aws:s3-outposts:us-west-2:123456789012:outpost:op-01234567890123456:accesspoint:ap",
    "arn:aws-cn:kinesis:cn-north-1:123456789012:stream/s",
    "bucket.with.dots",
    "UPPER--Case",
    "123456789012",
    "%2F/~ +é",
    "a--x-s3",
    "arn:aws:s3:::bucket",
];

/// The wrong-typed values for a declared type.
fn wrong(ty: endpoint_rules::ParamType) -> [P; 2] {
    use endpoint_rules::ParamType::{Boolean, String, StringArray};
    match ty {
        String => [P::Boolean(true), P::StringArray(vec!["us-east-1".into()])],
        Boolean => [P::String("true".into()), P::StringArray(vec![])],
        StringArray => [P::String("a".into()), P::Boolean(false)],
    }
}

/// The values to try for a parameter: regions for `AWS::Region`, the
/// string pool for any other string, both flags, a few arrays.
fn pool(def: &endpoint_rules::ParameterDef) -> Vec<P> {
    use endpoint_rules::ParamType::{Boolean, String, StringArray};
    let text = |items: &[&str]| {
        items
            .iter()
            .map(|item| P::String((*item).to_owned()))
            .collect()
    };
    match def.ty() {
        String if def.builtin() == Some("AWS::Region") => text(&REGIONS),
        String => text(&STRINGS),
        Boolean => vec![P::Boolean(true), P::Boolean(false)],
        StringArray => vec![
            P::StringArray(vec![]),
            P::StringArray(vec!["a".into()]),
            P::StringArray(vec!["a".into(), "arn:aws:s3:::b".into()]),
        ],
    }
}

/// Cases, each with its label.
type Labelled = Vec<(String, EndpointParams)>;

/// A service's generated cases, labelled: nothing set; each region with
/// FIPS and dual-stack in every pairing; then, from a base of
/// `us-east-1`, each parameter through its pool and unset; and an
/// undeclared name. The second list sets each parameter to a value of the
/// wrong type, which the CRT may crash on, so it runs apart.
fn generated(program: &Program) -> (Labelled, Labelled) {
    let defs = program.params();
    let by_builtin = |builtin: &str| {
        defs.iter()
            .find(|def| def.builtin() == Some(builtin))
            .map(|def| def.name().to_owned())
    };
    let (region, fips, dual) = (
        by_builtin("AWS::Region"),
        by_builtin("AWS::UseFIPS"),
        by_builtin("AWS::UseDualStack"),
    );
    let (mut typed, mut wrong_typed) = (
        vec![("unset".to_owned(), EndpointParams::default())],
        Vec::new(),
    );
    let mut base = EndpointParams::default();
    for text in REGIONS {
        for (use_fips, use_dual) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut params = EndpointParams::default();
            for (name, value) in [
                (&region, P::String(text.into())),
                (&fips, P::Boolean(use_fips)),
                (&dual, P::Boolean(use_dual)),
            ] {
                if let Some(name) = name {
                    params.set(name, value);
                }
            }
            if text == "us-east-1" && !use_fips && !use_dual {
                base = params.clone();
            }
            typed.push((
                format!("region {text:?} fips {use_fips} dual {use_dual}"),
                params,
            ));
        }
    }
    let with = |name: &str, value: P| {
        let mut params = base.clone();
        params.set(name, value);
        params
    };
    for def in defs {
        for value in pool(def) {
            typed.push((
                format!("{} = {value:?}", def.name()),
                with(def.name(), value),
            ));
        }
        for value in wrong(def.ty()) {
            wrong_typed.push((
                format!("{} = {value:?} (wrong type)", def.name()),
                with(def.name(), value),
            ));
        }
        let mut params = base.clone();
        params.remove(def.name());
        typed.push((format!("{} unset", def.name()), params));
    }
    typed.push((
        "undeclared".to_owned(),
        with("NotDeclared", P::String("x".into())),
    ));
    (typed, wrong_typed)
}

/// A shipped service's endpoint data and its decoded program.
/// Resolution reads only the rule set and the partitions, so neither the
/// model nor `endpoints.json` is loaded.
fn shipped(source: &DirSource, name: &str) -> TestResult<(EndpointData, Native, Program)> {
    let (_, rules) = load_service_bytes(source, name, Kind::EndpointRuleSet)?;
    let data = EndpointData::from_documents(rules, load_data_bytes(source, "partitions")?, None)?;
    let native = Native::new(&data);
    let program =
        Program::decode(&data.rules).map_err(|error| format!("{name}: {}", error.detail()))?;
    Ok((data, native, program))
}

fn shipped_services(source: &DirSource) -> TestResult<Vec<String>> {
    let mut services = source.services(Kind::EndpointRuleSet)?;
    services.sort();
    Ok(services)
}

/// The generated well-typed cases of every shipped rule set.
#[test]
#[cfg_attr(miri, ignore = "runs the CRT, which Miri cannot")]
fn generated_parameters_resolve_alike() -> TestResult {
    let source = botocore();
    let mut run = Run::default();
    for name in shipped_services(&source)? {
        let (data, native, program) = shipped(&source, &name)?;
        for (label, params) in generated(&program).0 {
            run.check(|| format!("{name} {label}"), &data, &native, &params);
        }
    }
    run.finish("generated", 436 * 80)
}

/// The index of the first case a [`wrong_typed_child`] resolves.
const CHILD: &str = "ENDPOINT_DIFFERENTIAL_WRONG_TYPED";
const CHILD_TEST: &str = "endpoint::differential::tests::wrong_typed_child";

/// Every shipped service, and each wrong-typed case, in one order, by its
/// service's index.
type WrongTyped = (
    Vec<(String, EndpointData, Native)>,
    Vec<(usize, String, EndpointParams)>,
);

fn wrong_typed_cases() -> TestResult<WrongTyped> {
    let source = botocore();
    let (mut services, mut cases) = (Vec::new(), Vec::new());
    for name in shipped_services(&source)? {
        let (data, native, program) = shipped(&source, &name)?;
        let service = services.len();
        cases.extend(
            generated(&program)
                .1
                .into_iter()
                .map(|(label, params)| (service, label, params)),
        );
        services.push((name, data, native));
    }
    Ok((services, cases))
}

/// In a child process only: resolves the wrong-typed cases from the given
/// index on, one `CASE <index> <finding or ->` line each, so the parent
/// knows which case a crash ended on.
#[test]
fn wrong_typed_child() -> TestResult {
    let Some(start) = std::env::var_os(CHILD) else {
        return Ok(());
    };
    let start: usize = start.into_string().map_err(|_| "start")?.parse()?;
    let (services, cases) = wrong_typed_cases()?;
    let mut out = std::io::stdout().lock();
    for (index, (service, label, params)) in cases.iter().enumerate().skip(start) {
        let (name, data, native) = &services[*service];
        let finding = compare(|| format!("{name} {label}"), data, native, params);
        let text = finding.map_or_else(
            || "-".to_owned(),
            |finding| finding.to_string().replace('\n', " | "),
        );
        std::io::Write::write_all(&mut out, format!("CASE {index} {text}\n").as_bytes())?;
        std::io::Write::flush(&mut out)?;
    }
    Ok(())
}

/// The generated wrong-typed cases of every shipped rule set, in a child
/// process: a case the CRT crashes on is a finding, with the native
/// engine's result, and a new child resumes after it.
#[test]
#[cfg_attr(miri, ignore = "runs the CRT in child processes, which Miri cannot")]
fn wrong_typed_parameters_resolve_alike() -> TestResult {
    let (services, cases) = wrong_typed_cases()?;
    let exe = std::env::current_exe()?;
    let (mut run, mut crashes) = (Run::default(), String::new());
    let mut start = 0;
    while start < cases.len() {
        // No core file: a crash of a child that holds every rule set
        // otherwise takes minutes to dump.
        let output = std::process::Command::new("sh")
            .args(["-c", "ulimit -c 0 && exec \"$0\" \"$@\""])
            .arg(&exe)
            .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads", "1"])
            .env(CHILD, start.to_string())
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut next = start;
        // The harness prints `test <name> ... ` before the first line.
        let lines = stdout
            .lines()
            .filter_map(|line| line.split_once("CASE ").map(|(_, rest)| rest));
        for line in lines {
            let (index, text) = line.split_once(' ').ok_or("case line")?;
            assert_eq!(index.parse::<usize>()?, next, "the child skipped a case");
            run.cases += 1;
            if text != "-" {
                run.findings.push(format!("{text}\n"));
            }
            next += 1;
        }
        if output.status.success() {
            assert_eq!(next, cases.len(), "the child stopped early");
            break;
        }
        let (service, label, params) = cases.get(next).ok_or_else(|| {
            format!(
                "the child failed after its last case: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        })?;
        let (name, _, native) = &services[*service];
        run.cases += 1;
        crashes.push_str(&format!("{name}\t{label}\t{:?}\n", native.resolve(params)));
        start = next + 1;
    }
    let recorded = std::fs::read_to_string(crashes_path())?;
    if recorded != crashes {
        run.findings.push(format!(
            "the CRT's crashes differ from {}; this run's:\n{crashes}",
            crashes_path().display()
        ));
    }
    run.finish("wrong-typed", 1734 * 2)
}

/// The wrong-typed cases the CRT crashes on (SIGSEGV), with the native
/// engine's resolution of each: service, case, native result. A finding
/// recorded exactly, not accepted: a crash gained or lost, or a native
/// result changed, fails the run. The client never gives a rule set a
/// value of the wrong type (`param_value`, the built-ins), so none of
/// these is reachable from a call.
fn crashes_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/endpoint/differential/crt-crashes.tsv")
}

// ------------------------------------------------ the comparison, attacked

const RULES: &str = r#"{"version": "1.0",
    "parameters": {
        "Region": {"type": "String", "builtIn": "AWS::Region", "required": true},
        "Flag": {"type": "Boolean", "required": true, "default": false},
        "Url": {"type": "String", "required": false}
    },
    "rules": [
        {"type": "endpoint", "conditions": [{"fn": "isSet", "argv": [{"ref": "Url"}]}],
         "endpoint": {"url": "{Url}", "properties": {"b": 1, "a": [2, 1]},
                      "headers": {"h": ["v"]}}},
        {"type": "error", "conditions": [{"fn": "booleanEquals", "argv": [{"ref": "Flag"}, true]}],
         "error": "Invalid region in ARN: `{Region}` (invalid DNS name)"},
        {"type": "endpoint", "conditions": [], "endpoint": {"url": "https://x.{Region}.example.com",
         "properties": {"authSchemes": [{"name": "sigv4", "signingRegion": "{Region}"}]}}}
    ]}"#;

fn tiny() -> TestResult<(EndpointData, Native)> {
    let partitions = load_data_bytes(&botocore(), "partitions")?;
    let data = EndpointData::from_documents(RULES.as_bytes().to_vec(), partitions, None)?;
    let native = Native::new(&data);
    assert!(native.decode_error().is_none(), "the fixture decodes");
    Ok((data, native))
}

fn named(pairs: &[(&str, P)]) -> EndpointParams {
    params_of(
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone())),
    )
}

/// A rule, a rule error with its host-label mapping, headers, an unset
/// required parameter, a wrong type and an undeclared name agree.
#[test]
#[cfg_attr(miri, ignore = "runs the CRT, which Miri cannot")]
fn the_tiny_rule_set_resolves_alike_every_way() -> TestResult {
    let (data, native) = tiny()?;
    let region = ("Region", P::String("us-east-1".into()));
    for params in [
        named(std::slice::from_ref(&region)),
        named(&[region.clone(), ("Flag", P::Boolean(true))]),
        named(&[region.clone(), ("Url", P::String("https://u".into()))]),
        named(&[]),
        named(&[("Region", P::Boolean(true))]),
        named(&[region.clone(), ("Other", P::String("x".into()))]),
    ] {
        let finding = compare(String::new, &data, &native, &params);
        assert!(
            finding.is_none(),
            "{}",
            finding.map(|f| f.to_string()).unwrap_or_default()
        );
    }
    Ok(())
}

fn endpoint(url: &str, properties: &str, headers: usize) -> Resolution {
    Resolution::Endpoint {
        url: url.into(),
        properties: canonical(properties),
        headers,
    }
}

fn failed(text: &str, detail: &str) -> Resolution {
    Resolution::Failed {
        text: text.into(),
        detail: detail.into(),
    }
}

/// Each evasion the comparison could let through is a difference: a url
/// off by a slash or a letter's case, reordered arrays, a number written
/// otherwise, a header count, a rule error dressed as a failure, failure
/// texts, properties that are not JSON. Only key order and a failure's
/// detail are not.
#[test]
fn every_kind_of_difference_is_a_difference() {
    let base = endpoint("https://a", r#"{"b": [1, 2], "a": {"y": 1, "x": "s"}}"#, 0);
    assert!(base.same(&endpoint(
        "https://a",
        r#"{"a": {"x": "s", "y": 1}, "b": [1, 2]}"#,
        0
    )));
    for other in [
        endpoint("https://a/", r#"{"b": [1, 2], "a": {"y": 1, "x": "s"}}"#, 0),
        endpoint("https://A", r#"{"b": [1, 2], "a": {"y": 1, "x": "s"}}"#, 0),
        endpoint("https://a", r#"{"b": [2, 1], "a": {"y": 1, "x": "s"}}"#, 0),
        endpoint(
            "https://a",
            r#"{"b": [1, 2], "a": {"y": 1.0, "x": "s"}}"#,
            0,
        ),
        endpoint(
            "https://a",
            r#"{"b": [1, 2], "a": {"y": 1, "x": "s"}, "c": null}"#,
            0,
        ),
        endpoint("https://a", r#"{"b": [1, 2], "a": {"y": 1, "x": "s"}}"#, 1),
        endpoint("https://a", "not json", 0),
        Resolution::RuleError("https://a".into()),
    ] {
        assert!(!base.same(&other) && !other.same(&base), "{other:?}");
    }
    assert!(!endpoint("https://a", "{}", 0).same(&endpoint("https://a", "not json", 0)));
    let rule = Resolution::RuleError("endpoint resolution failed".into());
    assert!(!rule.same(&failed("endpoint resolution failed", "")));
    assert!(
        failed("endpoint resolution failed", "").same(&failed("endpoint resolution failed", "why"))
    );
    assert!(
        !failed("invalid endpoint ruleset", "").same(&failed("endpoint resolution failed", ""))
    );
}

/// The client-level results decide on their own: two engines that agree
/// on the resolution but not on the client's result are a finding, and
/// the client mapping refuses headers and maps the ARN region error.
#[test]
#[cfg_attr(miri, ignore = "runs the CRT, which Miri cannot")]
fn the_client_mapping_matches_resolve() -> TestResult {
    let (data, native) = tiny()?;
    let headers = named(&[
        ("Region", P::String("r".into())),
        ("Url", P::String("https://u".into())),
    ]);
    assert_eq!(
        native.client_resolve(&headers),
        Err(CallError::Unsupported(Unsupported::EndpointHeaders))
    );
    let arn = named(&[
        ("Region", P::String("r".into())),
        ("Flag", P::Boolean(true)),
    ]);
    let Err(CallError::Endpoint(text)) = native.client_resolve(&arn) else {
        return Err("no endpoint error".into());
    };
    assert!(text.starts_with("Invalid host label"), "{text}");
    let plain = named(&[("Region", P::String("r".into()))]);
    let resolved = native
        .client_resolve(&plain)
        .map_err(|error| error.to_string())?;
    assert_eq!(
        resolved.auth_schemes[0].signing_region.as_deref(),
        Some("r")
    );
    let finding = Finding {
        case: "c".into(),
        params: plain.clone(),
        crt: crt_resolve(&data, &plain),
        native: native.resolve(&plain),
        crt_client: Err(CallError::NoRegion),
        native_client: Ok(resolved),
    };
    let text = finding.to_string();
    assert!(
        text.contains("client crt") && !text.contains("  crt     "),
        "{text}"
    );
    Ok(())
}

/// A rule set or partitions the native engine cannot decode fails every
/// case with the CRT's text for that document, which the CRT gives too.
#[test]
#[cfg_attr(miri, ignore = "runs the CRT, which Miri cannot")]
fn an_undecodable_document_fails_alike() -> TestResult {
    let bad_rules = r#"{"version": "1.0", "parameters": {}, "rules": [{"type": "nope"}]}"#;
    let partitions = load_data_bytes(&botocore(), "partitions")?;
    let data = EndpointData::from_documents(bad_rules.as_bytes().to_vec(), partitions, None)?;
    let native = Native::new(&data);
    assert!(native.decode_error().is_some());
    let params = EndpointParams::default();
    assert!(compare(String::new, &data, &native, &params).is_none());
    let (data, _) = tiny()?;
    let bad = EndpointData::from_documents(data.rules.clone(), b"[]".to_vec(), None)?;
    let native = Native::new(&bad);
    let Resolution::Failed { text, .. } = native.resolve(&params) else {
        return Err("partitions that decode".into());
    };
    assert_eq!(text, "invalid endpoint partitions");
    assert!(compare(String::new, &bad, &native, &params).is_none());
    Ok(())
}
