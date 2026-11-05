#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_core::{CancelToken, EntryKind, Error, NoProgress};
use std::io::Read;

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wim_stored.wim")
}

#[test]
fn reads_independently_created_wim_with_unicode_and_empty_files() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    let entries = reader.entries().unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries
        .iter()
        .any(|entry| entry.path == "目录" && entry.kind == EntryKind::Dir));
    for entry in entries.iter().filter(|entry| entry.kind == EntryKind::File) {
        let mut bytes = Vec::new();
        reader.open(entry).unwrap().read_to_end(&mut bytes).unwrap();
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
fn refuses_cancelled_and_over_budget_wim_operations() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(matches!(
        reader.entries_with_progress(&cancel, &mut NoProgress),
        Err(Error::Cancelled)
    ));
    reader.set_read_control(CancelToken::new(), 1);
    assert!(matches!(reader.entries(), Err(Error::LimitExceeded(_))));
}

#[test]
fn verifies_wim_file_hashes_and_rejects_corrupt_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("corrupt.wim");
    let mut bytes = std::fs::read(fixture()).unwrap();
    bytes[208] ^= 0x80;
    std::fs::write(&path, &bytes).unwrap();
    let mut reader = rz_formats::open(&path).unwrap();
    let entry = reader
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.size > 0)
        .unwrap();
    assert!(reader
        .open(&entry)
        .unwrap()
        .read_to_end(&mut Vec::new())
        .is_err());
    bytes[208] ^= 0x80;
    bytes[250] ^= 0x80;
    std::fs::write(&path, bytes).unwrap();
    assert!(rz_formats::open(&path).unwrap().entries().is_err());
}

#[test]
fn wim_file_reads_observe_cancellation_and_total_budget() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    let entry = reader
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.size > 0)
        .unwrap();
    let cancel = CancelToken::new();
    reader.set_read_control(cancel.clone(), 1);
    assert!(reader
        .open(&entry)
        .unwrap()
        .read_to_end(&mut Vec::new())
        .is_err());
    reader.set_read_control(cancel.clone(), u64::MAX);
    let mut stream = reader.open(&entry).unwrap();
    cancel.cancel();
    assert!(stream.read(&mut [0u8; 1]).is_err());
}

#[test]
fn reads_xpress_compressed_resource_across_chunk_boundaries() {
    let path = fixture().with_file_name("wim_xpress.wim");
    let mut reader = rz_formats::open(&path).unwrap();
    let entries = reader.entries().unwrap();
    let entry = entries
        .iter()
        .find(|entry| entry.path == "large.txt")
        .unwrap();
    assert!(entry.packed_size.unwrap() < entry.size);
    let mut bytes = Vec::new();
    reader.open(entry).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"RustZIP compressed chunk fixture.\n".repeat(4096));
}

#[test]
fn reads_lzx_compressed_resource_across_chunk_boundaries() {
    let path = fixture().with_file_name("wim_lzx.wim");
    let mut reader = rz_formats::open(&path).unwrap();
    let entries = reader.entries().unwrap();
    let entry = entries
        .iter()
        .find(|entry| entry.path == "large.txt")
        .unwrap();
    assert!(entry.packed_size.unwrap() < entry.size);
    let mut bytes = Vec::new();
    reader.open(entry).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"RustZIP compressed chunk fixture.\n".repeat(4096));
}

#[test]
fn reads_lzms_compressed_resource_across_chunk_boundaries() {
    for name in ["wim_lzms.wim", "wim_lzms_solid.esd"] {
        let path = fixture().with_file_name(name);
        let mut reader = rz_formats::open(&path).unwrap();
        let entries = reader.entries().unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.path == "large.txt")
            .unwrap();
        let mut bytes = Vec::new();
        reader.open(entry).unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(
            bytes,
            b"RustZIP compressed chunk fixture.\n".repeat(4096),
            "{name}"
        );
    }
}

#[test]
fn compressed_wim_reads_observe_cancellation_and_output_budget() {
    for name in ["wim_lzx.wim", "wim_lzms.wim", "wim_lzms_solid.esd"] {
        let mut reader = rz_formats::open(&fixture().with_file_name(name)).unwrap();
        let entry = reader
            .entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.path == "large.txt")
            .unwrap();
        let cancel = CancelToken::new();
        reader.set_read_control(cancel.clone(), 1);
        assert!(
            reader
                .open(&entry)
                .unwrap()
                .read_to_end(&mut Vec::new())
                .is_err(),
            "{name}"
        );
        reader.set_read_control(cancel.clone(), u64::MAX);
        let mut stream = reader.open(&entry).unwrap();
        cancel.cancel();
        assert!(stream.read(&mut [0u8; 1]).is_err(), "{name}");
    }
}

#[test]
fn solid_wim_enforces_compression_ratio_on_the_shared_stream() {
    let mut reader = rz_formats::open(&fixture().with_file_name("wim_lzms_solid.esd")).unwrap();
    reader.set_ratio_limit(1, 1);
    assert!(matches!(reader.entries(), Err(Error::LimitExceeded(_))));
}
