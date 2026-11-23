#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| awscli_argparse::fuzz::convert_both(data));
