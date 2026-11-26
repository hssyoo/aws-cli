//! The stable replay of the parser fuzz targets: each target's curated
//! inputs under `fuzz/corpus/<target>`, every prefix of each, then a seeded
//! sweep, through the same bodies the targets run.
use super::{model_json, responses, starting_token, validate, xml};
use crate::testing::facade::{Model, model_from_json};
use std::path::Path;
use std::sync::Arc;

const QUERY: &[u8] = include_bytes!("../../../../../tests/models/zquery/2020-01-01/service-2.json");
const JSON: &[u8] = include_bytes!("../../../../../tests/models/zjson/2020-01-01/service-2.json");
const JSON11: &[u8] =
    include_bytes!("../../../../../tests/models/zjson11/2020-01-01/service-2.json");
const REST_JSON: &[u8] =
    include_bytes!("../../../../../tests/models/zrestjson/2020-01-01/service-2.json");

/// The fake models the response and validation targets read with.
fn models() -> Vec<Arc<Model>> {
    [QUERY, JSON, JSON11, REST_JSON]
        .into_iter()
        .filter_map(|bytes| model_from_json(bytes).ok())
        .collect()
}

/// Every curated input of `target`, sorted by name.
fn curated(target: &str) -> std::io::Result<Vec<Vec<u8>>> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(target);
    let mut paths: Vec<_> = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    paths.sort();
    paths.into_iter().map(std::fs::read).collect()
}

/// `count` inputs of up to 96 bytes from SplitMix64, one seed per input.
fn sweep(count: u64, body: impl Fn(&[u8])) {
    for seed in 0..count {
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut mixed = state;
            mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            mixed ^ (mixed >> 31)
        };
        let length = next() % 97;
        let bytes: Vec<u8> = (0..length).map(|_| next().to_le_bytes()[0]).collect();
        body(&bytes);
    }
}

/// Each curated input of `target` and every prefix of it, then the sweep.
fn replay(target: &str, body: impl Fn(&[u8])) -> std::io::Result<()> {
    let inputs = curated(target)?;
    assert!(!inputs.is_empty(), "{target} has no curated inputs");
    for input in &inputs {
        for end in 0..=input.len() {
            body(&input[..end]);
        }
    }
    sweep(256, body);
    Ok(())
}

#[test]
fn the_xml_reader_target_replays() -> std::io::Result<()> {
    replay("client_xml", xml)
}

#[test]
fn the_response_target_replays() -> std::io::Result<()> {
    let models = models();
    assert_eq!(models.len(), 4);
    replay("protocol_responses", |bytes| responses(&models, bytes))
}

#[test]
fn the_starting_token_target_replays() -> std::io::Result<()> {
    replay("paginator_token", starting_token)
}

#[test]
fn the_model_targets_replay() -> std::io::Result<()> {
    let models = models();
    replay("model_json", model_json)?;
    replay("model_validate", |bytes| {
        for model in &models {
            validate(model, bytes);
        }
    })
}

#[test]
fn the_timestamp_and_conversion_targets_replay() -> std::io::Result<()> {
    replay("timestamp_text", timestamp::fuzz::parse)?;
    replay("argparse_convert", awscli_argparse::fuzz::convert_both)
}
