#![no_main]

use libfuzzer_sys::fuzz_target;
use rz_video_renderer::{render_first_frame, MAX_INPUT_BYTES};

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > MAX_INPUT_BYTES {
        return;
    }
    let _ = render_first_frame(bytes);
});
