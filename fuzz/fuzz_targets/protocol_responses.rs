#![no_main]
use model::shapes::Model;
use std::sync::LazyLock;

/// The fake query, json 1.0, json 1.1 and rest-json models the responses are
/// read with.
static MODELS: LazyLock<Vec<Model>> = LazyLock::new(|| {
    [
        &include_bytes!("../../tests/models/zquery/2020-01-01/service-2.json")[..],
        include_bytes!("../../tests/models/zjson/2020-01-01/service-2.json"),
        include_bytes!("../../tests/models/zjson11/2020-01-01/service-2.json"),
        include_bytes!("../../tests/models/zrestjson/2020-01-01/service-2.json"),
    ]
    .into_iter()
    .filter_map(|bytes| Model::from_json(bytes).ok())
    .collect()
});

libfuzzer_sys::fuzz_target!(|data: &[u8]| client::testing::fuzz::responses(&MODELS, data));
