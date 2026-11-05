#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::Cursor;
use std::io::{Read, Write};

const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;

fn exercise(bytes: &[u8]) {
    let Ok(mut file) = tempfile::NamedTempFile::new() else {
        return;
    };
    if file.write_all(bytes).is_err() {
        return;
    }
    let Ok(mut reader) = rz_formats::open(file.path()) else {
        return;
    };
    if reader.format_name() != "nsis" {
        return;
    }
    reader.set_read_control(rz_core::CancelToken::new(), 16 * 1024 * 1024);
    let Ok(entries) = reader.entries() else {
        return;
    };
    let mut buffer = [0u8; 16 * 1024];
    for entry in entries {
        let Ok(mut stream) = reader.open(&entry) else {
            continue;
        };
        while let Ok(count) = stream.read(&mut buffer) {
            if count == 0 {
                break;
            }
        }
    }
}

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > MAX_INPUT_BYTES {
        return;
    }
    let _ = rz_formats::sniff(&mut Cursor::new(bytes));
    if !bytes.starts_with(b"MZ") {
        return;
    }
    exercise(bytes);

    // Recompute valid NSIS CRCs so mutations can reach the decoders as well as
    // exercising the checksum rejection path above.
    let Some(at) = bytes.windows(16).position(|window| {
        window[..4] == [0xef, 0xbe, 0xad, 0xde] && window[4..] == *b"NullsoftInst"
    }) else {
        return;
    };
    let Some(header) = at.checked_sub(4) else {
        return;
    };
    let Some(raw) = bytes.get(header..header.saturating_add(28)) else {
        return;
    };
    let flags = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let length = u32::from_le_bytes([raw[24], raw[25], raw[26], raw[27]]) as usize;
    let Some(end) = header.checked_add(length).filter(|end| *end <= bytes.len()) else {
        return;
    };
    if flags & 4 != 0 || end < 516 {
        return;
    }
    let checksum = crc32fast::hash(&bytes[512..end - 4]);
    let mut fixed = bytes.to_vec();
    fixed[end - 4..end].copy_from_slice(&checksum.to_le_bytes());
    exercise(&fixed);
});
