#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > MAX_INPUT_BYTES {
        return;
    }
    let _ = rz_formats::sniff(&mut Cursor::new(bytes));
});
