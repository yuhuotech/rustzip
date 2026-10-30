#![no_main]

use libfuzzer_sys::fuzz_target;
use rodio::{Decoder, Source};
use std::io::Cursor;
use std::time::Duration;

const MAX_AUDIO_INPUT: usize = 8 * 1024 * 1024;
const MAX_DECODED_SAMPLES: usize = 500_000;

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > MAX_AUDIO_INPUT {
        return;
    }

    if let Ok(decoder) = Decoder::new(Cursor::new(bytes.to_vec())) {
        let _ = decoder
            .take_duration(Duration::from_secs(1))
            .take(MAX_DECODED_SAMPLES)
            .count();
    }
});
