#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| timestamp::fuzz::parse(data));
