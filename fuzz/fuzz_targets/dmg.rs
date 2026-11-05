#![no_main]
mod bounded_archive;
libfuzzer_sys::fuzz_target!(|bytes: &[u8]| bounded_archive::exercise(bytes, "dmg"));
