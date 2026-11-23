#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| model::fuzz::model_json(data));
