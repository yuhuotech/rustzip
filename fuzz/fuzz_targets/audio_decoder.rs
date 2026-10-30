#![no_main]

use libfuzzer_sys::fuzz_target;
use rz_audio_preview::{decode_audio_with_limit, MAX_INPUT_BYTES};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

const MAX_DECODED_SAMPLES: usize = 500_000;

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > MAX_INPUT_BYTES {
        return;
    }

    let cancel = AtomicBool::new(false);
    let _ = decode_audio_with_limit(Arc::from(bytes), &cancel, MAX_DECODED_SAMPLES);
});
