#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_core::{CancelToken, Error, NoProgress};
use std::io::Read;

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nsis_deflate.exe")
}

#[test]
fn lists_and_reads_independently_compiled_nsis_files() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    let entries = reader.entries().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["payload.txt", "empty.txt", "目录/中文.txt"]
    );
    for entry in entries {
        let mut bytes = Vec::new();
        reader
            .open(&entry)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        let expected = if entry.path == "empty.txt" {
            &b""[..]
        } else {
            include_bytes!("fixtures/nsis_sources/payload.txt")
        };
        assert_eq!(bytes, expected);
        assert_eq!(entry.size, bytes.len() as u64);
    }
}

#[test]
fn rejects_cancelled_and_over_budget_nsis_scans() {
    let cancel = CancelToken::new();
    cancel.cancel();
    let mut reader = rz_formats::open(&fixture()).unwrap();
    assert!(matches!(
        reader.entries_with_progress(&cancel, &mut NoProgress),
        Err(Error::Cancelled)
    ));
    reader.set_read_control(CancelToken::new(), 1);
    assert!(matches!(reader.entries(), Err(Error::LimitExceeded(_))));
}

#[test]
fn rejects_corruption_in_the_nsis_payload() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("corrupt.bin");
    for name in ["nsis_deflate.exe", "nsis_bzip2.exe", "nsis_bzip2_solid.exe"] {
        let mut bytes = std::fs::read(fixture().with_file_name(name)).unwrap();
        let index = bytes.len() - 10;
        bytes[index] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();
        assert!(
            rz_formats::open(&path).unwrap().entries().is_err(),
            "{name}"
        );
    }
}

#[test]
fn nsis_stream_observes_cancellation_and_cumulative_output_limits() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    let entries = reader.entries().unwrap();
    let cancel = CancelToken::new();
    reader.set_read_control(cancel.clone(), 1);
    let mut bytes = Vec::new();
    assert!(reader
        .open(&entries[0])
        .unwrap()
        .read_to_end(&mut bytes)
        .is_err());
    reader.set_read_control(cancel.clone(), u64::MAX);
    let mut stream = reader.open(&entries[0]).unwrap();
    cancel.cancel();
    assert!(stream.read(&mut [0u8; 1]).is_err());
}

#[test]
fn reads_non_solid_lzma_header_and_compressed_payload() {
    for name in [
        "nsis_lzma.exe",
        "nsis_deflate_solid.exe",
        "nsis_lzma_solid.exe",
        "nsis_bzip2.exe",
        "nsis_bzip2_solid.exe",
    ] {
        let path = fixture().with_file_name(name);
        let mut reader = rz_formats::open(&path).unwrap();
        let entries = reader.entries().unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.path == "compressed.txt")
            .unwrap();
        if name == "nsis_lzma.exe" {
            assert!(entry.packed_size.unwrap() < entry.size);
        }
        let mut bytes = Vec::new();
        reader.open(entry).unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"RustZIP LZMA streaming.\n".repeat(4096), "{name}");
    }
}
