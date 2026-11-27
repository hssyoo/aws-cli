//! The bodies of the fuzz targets over the model reader and the validator
//! (`fuzz/fuzz_targets/model_json.rs`, `validate_params.rs`), which the
//! stable replay test (`client::testing::fuzz`'s tests) also runs. No
//! input may panic.
use crate::frontend::c2j::arena;
use crate::shapes::Model;
use crate::validate::{Validation, validate_input};
use crate::value::{Map, Value};

/// `Model::from_json` over any bytes: a model or an error. The C2J
/// front-end reads a model the arena reads as the arena does (the same
/// projection) and refuses the rest with the arena's error. Every
/// operation of a model it builds validates an empty input.
pub fn model_json(bytes: &[u8]) {
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return;
    };
    let schema = crate::frontend::c2j::build(&json, &crate::frontend::c2j::namespace("fuzz"));
    let model = match Model::from_value(&json) {
        Ok(model) => model,
        Err(error) => {
            let refused = schema.err().map(|error| error.to_string());
            assert_eq!(refused, Some(error.to_string()));
            return;
        }
    };
    assert_eq!(arena::render(&model), model.projection());
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
