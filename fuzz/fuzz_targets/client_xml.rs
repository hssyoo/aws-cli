#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| client::testing::fuzz::xml(data));
