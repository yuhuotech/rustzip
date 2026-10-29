#![cfg(feature = "rar")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_ffi::unrar::{Error, Reader};
use std::fs;

fn archive() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("sample.rar"),
        include_bytes!("../../rz-formats/tests/fixtures/container.rar"),
    )
    .unwrap();
    dir
}

#[test]
fn callback_refuses_a_chunk_that_would_cross_the_output_limit() {
    let dir = archive();
    let mut reader = Reader::open_for_processing(&dir.path().join("sample.rar"), None).unwrap();
    assert!(reader.next_entry().unwrap().is_some());
    let mut output = Vec::new();

    let result = reader.extract_current_to(&mut output, 1, &|| false);

    assert!(matches!(result, Err(Error::OutputLimit)));
    assert!(output.len() <= 1);
}

#[test]
fn callback_can_cancel_native_decompression() {
    let dir = archive();
    let mut reader = Reader::open_for_processing(&dir.path().join("sample.rar"), None).unwrap();
    assert!(reader.next_entry().unwrap().is_some());
    let mut output = Vec::new();

    let result = reader.extract_current_to(&mut output, 1024, &|| true);

    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(output.is_empty());
}
