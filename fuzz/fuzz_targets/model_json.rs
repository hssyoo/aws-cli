#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| client::testing::fuzz::model_json(data));
