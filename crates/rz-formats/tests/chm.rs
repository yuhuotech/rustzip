#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use libchm::{ChmError, ChmFile};
use rz_core::{CancelToken, EntryKind, Error, Progress};
use rz_formats::open;
use std::io::Read;

const CHM: &[u8] = include_bytes!("fixtures/chm_basic.chm");

fn fixture_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("content.bin");
    std::fs::write(&path, CHM).unwrap();
    path
}

#[test]
fn lists_and_streams_lzx_entries_from_an_independent_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_path(&dir);
    let mut archive = open(&path).unwrap();
    assert_eq!(archive.format_name(), "chm");
    assert!(archive.caps().solid);
    assert!(!archive.caps().writable);

    let entries = archive.entries().unwrap();
    let mut paths = entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "a.htm",
            "b.htm",
            "empty.txt",
            "large.bin",
            "style.css",
            "项目.htm",
        ]
    );
    assert!(entries.iter().all(|entry| entry.kind == EntryKind::File));

    for (path, expected) in [
        (
            "a.htm",
            include_bytes!("fixtures/chm_basic_sources/a.htm").as_slice(),
        ),
        (
            "b.htm",
            include_bytes!("fixtures/chm_basic_sources/b.htm").as_slice(),
        ),
        (
            "style.css",
            include_bytes!("fixtures/chm_basic_sources/style.css").as_slice(),
        ),
        (
            "empty.txt",
            include_bytes!("fixtures/chm_basic_sources/empty.txt").as_slice(),
        ),
        (
            "large.bin",
            include_bytes!("fixtures/chm_basic_sources/large.bin").as_slice(),
        ),
        (
            "项目.htm",
            include_bytes!("fixtures/chm_basic_sources/项目.htm").as_slice(),
        ),
    ] {
        let entry = entries.iter().find(|entry| entry.path == path).unwrap();
        let mut reader = archive.open(entry).unwrap();
        let mut actual = Vec::new();
        reader.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, expected, "{path}");
    }
}

#[test]
fn cancellation_checkpoint_interrupts_a_multi_block_lzx_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_path(&dir);
    let mut archive = ChmFile::open(&path).unwrap();
    let reset_table = archive
        .find("::DataSpace/Storage/MSCompressed/Transform/{7FC28940-9D31-11D0-9B27-00A0C91E9C7C}/InstanceData/ResetTable")
        .unwrap();
    let reset_bytes = archive.read(&reset_table).unwrap();
    let block_count = u32::from_le_bytes(reset_bytes[4..8].try_into().unwrap());
    assert!(block_count > 1);

    let entry = archive.find("/large.bin").unwrap();
    let mut output = vec![0u8; 128 * 1024];
    let mut checkpoints = 0;
    let result = archive.read_range_with(&entry, 0, &mut output, || {
        checkpoints += 1;
        checkpoints < 8
    });
    assert!(matches!(result, Err(ChmError::Cancelled)));
    assert_eq!(checkpoints, 8);
}

#[test]
fn metadata_scan_reports_progress_and_stops_when_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_path(&dir);
    let mut archive = open(&path).unwrap();
    let cancel = CancelToken::new();
    let stop = cancel.clone();
    let mut progress = move |update: Progress| {
        if update.files_done == 1 {
            stop.cancel();
        }
    };

    assert!(matches!(
        archive.entries_with_progress(&cancel, &mut progress),
        Err(Error::Cancelled)
    ));
}

#[test]
fn entry_stream_checks_cancellation_and_output_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_path(&dir);
    let mut archive = open(&path).unwrap();
    let entries = archive.entries().unwrap();
    let entry = entries.iter().find(|entry| entry.path == "a.htm").unwrap();

    let cancel = CancelToken::new();
    archive.set_read_control(cancel.clone(), u64::MAX);
    let mut reader = archive.open(entry).unwrap();
    cancel.cancel();
    let error = reader.read(&mut [0u8; 16]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    drop(reader);

    archive.set_read_control(CancelToken::new(), 8);
    let mut reader = archive.open(entry).unwrap();
    let mut output = Vec::new();
    let error = reader.read_to_end(&mut output).unwrap_err();
    assert_eq!(output.len(), 8);
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    drop(reader);

    archive.set_read_control(CancelToken::new(), 223);
    let mut first = archive
        .open(entries.iter().find(|entry| entry.path == "a.htm").unwrap())
        .unwrap();
    let mut first_bytes = Vec::new();
    first.read_to_end(&mut first_bytes).unwrap();
    assert_eq!(first_bytes.len(), 213);
    drop(first);

    let mut second = archive
        .open(entries.iter().find(|entry| entry.path == "b.htm").unwrap())
        .unwrap();
    let mut second_bytes = Vec::new();
    let error = second.read_to_end(&mut second_bytes).unwrap_err();
    assert_eq!(second_bytes.len(), 10);
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn whole_entry_compatibility_read_has_a_hard_allocation_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture_path(&dir);
    let mut archive = ChmFile::open(&path).unwrap();
    let mut entry = archive.find("/large.bin").unwrap();
    entry.length = (64 * 1024 * 1024 + 1) as u64;
    assert!(matches!(
        archive.read(&entry),
        Err(ChmError::LimitExceeded("whole-entry read"))
    ));
}

#[test]
fn truncated_chm_signature_is_not_reported_as_an_unknown_format() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.chm");
    std::fs::write(&path, &CHM[..96]).unwrap();
    assert!(matches!(open(&path), Err(Error::Io(_) | Error::Corrupt(_))));
}
