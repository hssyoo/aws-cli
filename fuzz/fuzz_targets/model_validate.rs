#![no_main]
use model::shapes::Model;
use std::sync::LazyLock;

/// The fake query and json models, whose operations hold every member kind
/// and constraint the validator checks.
static MODELS: LazyLock<Vec<Model>> = LazyLock::new(|| {
    [
        &include_bytes!("../../tests/models/zquery/2020-01-01/service-2.json")[..],
        include_bytes!("../../tests/models/zjson/2020-01-01/service-2.json"),
    ]
    .into_iter()
    .filter_map(|bytes| Model::from_json(bytes).ok())
    .collect()
});

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    for model in MODELS.iter() {
        model::fuzz::validate(model, data);
    }
});
