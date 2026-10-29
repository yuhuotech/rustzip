#![no_main]

use libfuzzer_sys::fuzz_target;
use rz_core::{CancelToken, EntryKind, ProgressSink};
use std::io;

const MAX_INPUT: usize = 8 * 1024 * 1024;
const MAX_OUTPUT: u64 = 8 * 1024 * 1024;

struct NoProgress;

impl ProgressSink for NoProgress {
    fn update(&mut self, _progress: rz_core::Progress) {}
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT {
        return;
    }
    let Ok(temp) = tempfile::NamedTempFile::new() else {
        return;
    };
    if std::fs::write(temp.path(), data).is_err() {
        return;
    }
    let Ok(mut reader) = rz_formats::open(temp.path()) else {
        return;
    };
    if reader.format_name() != "rar" {
        return;
    }

    let cancel = CancelToken::new();
    reader.set_read_control(cancel.clone(), MAX_OUTPUT);
    let mut progress = NoProgress;
    let _ = reader.entries_with_progress(&cancel, &mut progress);
    let _ = reader.for_each(&mut |entry, stream| {
        if entry.kind == EntryKind::File {
            let mut bytes_left = MAX_OUTPUT.min(entry.size);
            let mut buffer = [0u8; 32 * 1024];
            while bytes_left > 0 {
                let requested = usize::try_from(bytes_left.min(buffer.len() as u64))
                    .map_err(io::Error::other)?;
                let read = stream.read(&mut buffer[..requested])?;
                if read == 0 {
                    break;
                }
                bytes_left -= read as u64;
            }
        }
        Ok(())
    });
});
