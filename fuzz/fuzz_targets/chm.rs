#![no_main]

use libfuzzer_sys::fuzz_target;
use rz_core::{CancelToken, NoProgress};
use std::io::Read;

const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > MAX_INPUT_BYTES {
        return;
    }
    let Ok(mut file) = tempfile::NamedTempFile::new() else {
        return;
    };
    if std::io::Write::write_all(&mut file, bytes).is_err() {
        return;
    }
    let Ok(mut reader) = rz_formats::open(file.path()) else {
        return;
    };
    if reader.format_name() != "chm" {
        return;
    }

    let cancel = CancelToken::new();
    reader.set_read_control(cancel.clone(), MAX_OUTPUT_BYTES);
    let mut progress = NoProgress;
    let Ok(entries) = reader.entries_with_progress(&cancel, &mut progress) else {
        return;
    };
    let mut total = 0u64;
    for entry in entries {
        if cancel.check().is_err() || entry.kind != rz_core::EntryKind::File {
            continue;
        }
        total = total.saturating_add(entry.size);
        if total > MAX_OUTPUT_BYTES {
            return;
        }
        let Ok(mut stream) = reader.open(&entry) else {
            continue;
        };
        let mut copied = 0u64;
        let mut buffer = [0u8; 16 * 1024];
        loop {
            if cancel.check().is_err() {
                return;
            }
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    copied = copied.saturating_add(count as u64);
                    if copied > entry.size || copied > MAX_OUTPUT_BYTES {
                        return;
                    }
                }
            }
        }
    }
});
