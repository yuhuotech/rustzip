use std::io::{Read, Write};

pub fn exercise(bytes: &[u8], format: &str) {
    if bytes.len() > 16 * 1024 * 1024 {
        return;
    }
    let Ok(mut file) = tempfile::NamedTempFile::new() else {
        return;
    };
    if file.write_all(bytes).is_err() {
        return;
    }
    let Ok(mut reader) = rz_formats::open(file.path()) else {
        return;
    };
    if reader.format_name() != format {
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
    let _ = reader.verify_integrity(
        &rz_core::CancelToken::new(),
        &mut rz_core::NoProgress,
        16 * 1024 * 1024,
    );
}
