#![no_main]

use libfuzzer_sys::fuzz_target;

const CRASH_SENTINEL: &[u8] = b"rustzip-crash-artifact-smoke-v1";

fuzz_target!(|data: &[u8]| {
    if data == CRASH_SENTINEL {
        panic!("intentional crash artifact smoke sentinel");
    }
});
