#![no_main]

use libfuzzer_sys::fuzz_target;
use rz_core::{CancelToken, NoProgress};
use std::io::{self, Read};

fuzz_target!(|data: &[u8]| {
    if data.len() > 8 * 1024 * 1024 {
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
    reader.set_decode_config(2, 256 << 20);
    let Ok(entries) = reader.entries() else {
        return;
    };
    let _ = reader.for_each(&mut |_, stream| {
        let mut limited = (&mut *stream).take(8 * 1024 * 1024);
        io::copy(&mut limited, &mut io::sink())?;
        Ok(())
    });
    let _ = reader.for_each_selected(Some(&[]), &mut |_, _| Ok(()));
    let _ = reader.entries_with_progress(&CancelToken::new(), &mut NoProgress);
    drop(entries);
});
