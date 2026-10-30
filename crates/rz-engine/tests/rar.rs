#![cfg(feature = "rar")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use rz_core::{CancelToken, EntryKind, Error, NoProgress};
use rz_engine::{extract, ExtractOptions};
use std::fs;
use std::path::PathBuf;

/// A 87-byte RAR 4 archive from the Homebrew test suite (BSD-2-Clause):
/// one file `container` holding "#!/bin/sh\nexit 0\n".
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rz-formats/tests/fixtures/container.rar")
}

fn duplicate_container_entry() -> Vec<u8> {
    let mut archive = fs::read(fixture()).unwrap();
    let entry_start = 7 + 13;
    assert_eq!(archive[entry_start + 2], 0x74);
    let header_size = u16::from_le_bytes(
        archive[entry_start + 5..entry_start + 7]
            .try_into()
            .unwrap(),
    ) as usize;
    let packed_size = u32::from_le_bytes(
        archive[entry_start + 7..entry_start + 11]
            .try_into()
            .unwrap(),
    ) as usize;
    let entry_end = entry_start + header_size + packed_size;
    assert!(entry_end < archive.len());
    let duplicate = archive[entry_start..entry_end].to_vec();
    archive.splice(entry_end..entry_end, duplicate);
    archive
}

#[test]
fn rar_is_listed_read_and_extracted() {
    let mut r = rz_formats::open(&fixture()).unwrap();
    assert_eq!(r.format_name(), "rar");
    assert!(!r.caps().writable, "RAR must stay read-only");
    let entries = r.entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "container");
    assert_eq!(entries[0].kind, EntryKind::File);
    assert_eq!(entries[0].size, 17);
    assert!(entries[0].modified.is_some());

    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let rep = extract(
        &mut *r,
        &out,
        &ExtractOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    assert_eq!(rep.files, 1);
    assert_eq!(
        fs::read_to_string(out.join("container")).unwrap(),
        "#!/bin/sh\nexit 0\n"
    );
}

#[test]
fn rar_entry_can_be_opened_directly() {
    let mut r = rz_formats::open(&fixture()).unwrap();
    let entry = r.entries().unwrap().remove(0);
    let mut text = String::new();
    std::io::Read::read_to_string(&mut r.open(&entry).unwrap(), &mut text).unwrap();
    assert_eq!(text, "#!/bin/sh\nexit 0\n");
}

#[test]
fn opening_a_rar_entry_honors_output_limits_and_cancellation() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    let entry = reader.entries().unwrap().remove(0);

    reader.set_read_control(CancelToken::new(), 1);
    assert!(matches!(reader.open(&entry), Err(Error::LimitExceeded(_))));

    let cancel = CancelToken::new();
    cancel.cancel();
    reader.set_read_control(cancel, u64::MAX);
    assert!(matches!(reader.open(&entry), Err(Error::Cancelled)));
}

#[test]
fn rar_streaming_output_stops_at_the_configured_limit() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    reader.set_read_control(CancelToken::new(), 1);
    let result = reader.for_each(&mut |_, data| {
        std::io::copy(data, &mut std::io::sink())?;
        Ok(())
    });
    assert!(matches!(result, Err(Error::LimitExceeded(_))));
}

#[test]
fn selected_rar_entries_share_the_total_output_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("two-entries.rar");
    fs::write(&path, duplicate_container_entry()).unwrap();
    let mut reader = rz_formats::open(&path).unwrap();
    assert_eq!(reader.entries().unwrap().len(), 2);

    reader.set_read_control(CancelToken::new(), 20);
    let mut visited = 0;
    let result = reader.for_each_selected(Some(&[0, 1]), &mut |_, data| {
        std::io::copy(data, &mut std::io::sink())?;
        visited += 1;
        Ok(())
    });

    assert!(matches!(result, Err(Error::LimitExceeded(_))));
    assert_eq!(visited, 1, "the second entry must not be extracted");
}

#[test]
fn selecting_no_rar_entries_does_not_write_unselected_content() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    reader.set_read_control(CancelToken::new(), 1);
    let mut visited = 0;

    reader
        .for_each_selected(Some(&[]), &mut |_, _| {
            visited += 1;
            Ok(())
        })
        .unwrap();

    assert_eq!(visited, 0);
}

#[test]
fn rar_compression_ratio_is_checked_before_native_output() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    reader.set_ratio_limit(0, 0);
    let result = reader.for_each(&mut |_, data| {
        std::io::copy(data, &mut std::io::sink())?;
        Ok(())
    });
    assert!(
        matches!(result, Err(Error::LimitExceeded(reason)) if reason.contains("compression ratio"))
    );
}

#[test]
fn damaged_rar_files_are_errors_not_crashes() {
    let original = fs::read(fixture()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bad.rar");

    // Truncated at every length.
    for len in 0..original.len() {
        fs::write(&path, &original[..len]).unwrap();
        let _ = rz_formats::open(&path).and_then(|mut r| {
            r.entries()?;
            r.for_each(&mut |_, d| {
                std::io::copy(d, &mut std::io::sink())?;
                Ok(())
            })
        });
    }

    // Every single-byte change.
    for at in 0..original.len() {
        let mut data = original.clone();
        data[at] ^= 0xff;
        fs::write(&path, &data).unwrap();
        let _ = rz_formats::open(&path).and_then(|mut r| {
            r.entries()?;
            let out = tmp.path().join("o");
            let _ = fs::remove_dir_all(&out);
            extract(
                &mut *r,
                &out,
                &ExtractOptions::default(),
                &CancelToken::new(),
                &mut NoProgress,
            )
            .map(|_| ())
        });
    }
}

#[test]
fn a_rar_signature_with_junk_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("junk.rar");
    fs::write(&path, b"Rar!\x1a\x07\x00 this is not a real archive").unwrap();
    let res = rz_formats::open(&path);
    assert!(
        matches!(
            res.err(),
            Some(Error::Corrupt(_) | Error::UnknownFormat | Error::Io(_))
        ),
        "junk after a RAR signature must be an error"
    );
}
