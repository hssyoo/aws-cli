//! The bodies of the fuzz targets over the model reader and the validator
//! (`fuzz/fuzz_targets/model_json.rs`, `validate_params.rs`), which the
//! stable replay test (`client::testing::fuzz`'s tests) also runs. No
//! input may panic.
use crate::shapes::Model;
use crate::validate::{Validation, validate_input};
use crate::value::{Map, Value};

/// `Model::from_json` over any bytes: a model or an error. A model it
/// builds renders its projection, and every operation validates an empty
/// input.
pub fn model_json(bytes: &[u8]) {
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return;
    };
    let Ok(model) = Model::from_value(&json) else {
        return;
    };
    assert!(!model.projection().is_empty());
    for operation in model.schema().service().operations() {
        report(validate_input(operation, &Value::Map(Map::new())));
    }
}

/// `ParamValidator().validate` of any JSON document against every
/// operation of `model`; an invalid input's report prints.
pub fn validate(model: &Model, bytes: &[u8]) {
    let Ok(params) = Value::from_json(bytes) else {
        return;
    };
    for operation in model.schema().service().operations() {
        report(validate_input(operation, &params));
    }
}

fn report(validation: Validation) {
    if let Validation::Invalid(report) = validation {
        assert!(!report.to_string().is_empty());
    }
}
