#![no_main]

use libfuzzer_sys::fuzz_target;
use rz_core::{CancelToken, Error};
use std::io::{Read, Write};

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 1 << 20 {
        return;
    }
    let Ok(mut file) = tempfile::NamedTempFile::new() else {
        return;
    };
    if file.write_all(bytes).is_err() {
        return;
    }
    let mut total = 0u64;
    let _ = rz_formats::recovery::zip_entries(
        file.path(),
        None,
        None,
        &CancelToken::new(),
        &mut |entry, data| {
            total = total.saturating_add(entry.size);
            if total > 8 << 20 {
                return Err(Error::LimitExceeded("fuzz data budget".into()));
            }
            std::io::copy(
                &mut data.take(entry.size.saturating_add(1)),
                &mut std::io::sink(),
            )?;
            Ok(())
        },
        &mut |_, _| {},
    );
});
