#![no_main]
use client::testing::facade::{Model, model_from_json};
use std::sync::Arc;
use std::sync::LazyLock;

/// The fake query and json models, whose operations hold every member kind
/// and constraint the validator checks.
static MODELS: LazyLock<Vec<Arc<Model>>> = LazyLock::new(|| {
    [
        &include_bytes!("../../tests/models/zquery/2020-01-01/service-2.json")[..],
        include_bytes!("../../tests/models/zjson/2020-01-01/service-2.json"),
    ]
    .into_iter()
    .filter_map(|bytes| model_from_json(bytes).ok())
    .collect()
});

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    for model in MODELS.iter() {
        client::testing::fuzz::validate(model, data);
    }
});
