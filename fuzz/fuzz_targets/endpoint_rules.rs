//! The endpoint rules engine on arbitrary documents: no input panics.
//! An input `P<json>` is a partitions document, looked up for a few
//! regions. Any other input is `<rule set>\xff<params>`, with params one
//! `name=value` per line (`true` and `false` are booleans, `a,b,` a
//! string array, anything else a string), evaluated against the shipped
//! partitions.
#![no_main]
use endpoint_rules::{EvalValue, Library, Params, Partitions, Program};
use std::sync::OnceLock;

const REGIONS: [&str; 5] = ["us-east-1", "aws-global", "cn-north-1", "o-west-2x", ""];

fn shipped() -> &'static Partitions {
    static SHIPPED: OnceLock<Partitions> = OnceLock::new();
    SHIPPED.get_or_init(|| {
        let json = include_bytes!("../../aws-cli/awscli/botocore/data/partitions.json");
        Partitions::decode(json).unwrap_or_else(|_| Partitions::empty())
    })
}

fn value(text: &str) -> EvalValue {
    match text {
        "true" => EvalValue::Bool(true),
        "false" => EvalValue::Bool(false),
        list if list.ends_with(',') => EvalValue::Array(
            list.split(',')
                .map(EvalValue::str)
                .collect::<Vec<_>>()
                .into(),
        ),
        other => EvalValue::str(other),
    }
}

fn partitions(json: &[u8]) {
    let Ok(partitions) = Partitions::decode(json) else {
        return;
    };
    let rules = br#"{"version": "1.0", "parameters": {"R": {"type": "string", "required": true}},
        "rules": [{"type": "endpoint", "endpoint": {"url": "https://{p#name}.{p#dnsSuffix}"},
                   "conditions": [{"fn": "aws.partition", "argv": [{"ref": "R"}],
                                   "assign": "p"}]}]}"#;
    let Ok(program) = Program::decode(rules) else {
        return;
    };
    let Some(slot) = program.param("R").map(|param| param.slot()) else {
        return;
    };
    for region in REGIONS {
        let mut params = Params::new(&program);
        params.set(slot, EvalValue::str(region));
        let _ = program.evaluate(
            &params,
            &Library {
                partitions: &partitions,
            },
        );
    }
}

fn rules(data: &[u8]) {
    let (rules, params_text) = match data.iter().position(|&b| b == 0xff) {
        Some(at) => (&data[..at], &data[at + 1..]),
        None => (data, &[][..]),
    };
    let Ok(program) = Program::decode(rules) else {
        return;
    };
    let mut params = Params::new(&program);
    for line in String::from_utf8_lossy(params_text).lines() {
        if let Some((name, text)) = line.split_once('=')
            && let Some(param) = program.param(name)
        {
            params.set(param.slot(), value(text));
        }
    }
    let _ = program.evaluate(
        &params,
        &Library {
            partitions: shipped(),
        },
    );
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| match data.split_first() {
    Some((b'P', json)) => partitions(json),
    _ => rules(data),
});
