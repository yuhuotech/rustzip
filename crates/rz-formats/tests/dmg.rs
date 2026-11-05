#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use base64::Engine as _;
use rz_core::{CancelToken, Error, NoProgress};
use std::io::Read;

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dmg_hfs_zlib.dmg")
}

#[test]
fn reads_independently_created_apfs_image() {
    let fixture = fixture().with_file_name("dmg_apfs_zlib.dmg");
    let mut reader = rz_formats::open(&fixture).unwrap();
    let entries = reader.entries().unwrap();
    let entry = entries
        .iter()
        .find(|entry| entry.path == "目录/中文.txt")
        .unwrap();
    let mut bytes = Vec::new();
    reader.open(entry).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, include_bytes!("fixtures/nsis_sources/payload.txt"));
}

#[test]
fn reads_real_transparently_compressed_apfs_file_through_dmg() {
    let path = fixture().with_file_name("dmg_apfs_compressed.dmg");
    let mut reader = rz_formats::open(&path).unwrap();
    let entry = reader
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "compressed.txt")
        .unwrap();
    assert_eq!(entry.size, 180_000);
    let mut output = Vec::new();
    reader
        .open(&entry)
        .unwrap()
        .read_to_end(&mut output)
        .unwrap();
    assert_eq!(
        output,
        b"The quick brown fox jumps over the lazy dog. ".repeat(4000)
    );
}

#[test]
fn rejects_damage_in_real_apfs_compressed_resource_fork() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("damaged.dmg");
    let mut image = std::fs::read(fixture().with_file_name("dmg_apfs_compressed.dmg")).unwrap();
    let start = 40 * 512 + 378 * 4096 + 16;
    image[start..start + 544].fill(0);
    std::fs::write(&path, image).unwrap();
    let mut reader = rz_formats::open(&path).unwrap();
    let entry = reader
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "compressed.txt")
        .unwrap();
    assert!(reader
        .open(&entry)
        .unwrap()
        .read_to_end(&mut Vec::new())
        .is_err());
}

#[test]
fn reads_hdiutil_hfs_image_without_mounting_it() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    let entries = reader.entries().unwrap();
    let entry = entries
        .iter()
        .find(|entry| entry.path == "目录/中文.txt")
        .unwrap();
    let mut bytes = Vec::new();
    reader.open(entry).unwrap().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, include_bytes!("fixtures/nsis_sources/payload.txt"));
    assert!(entries
        .iter()
        .any(|entry| entry.path == "empty.txt" && entry.size == 0));
}

#[test]
fn rejects_oversized_plist_before_allocating_from_its_length() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("oversized.dmg");
    let mut bytes = std::fs::read(fixture()).unwrap();
    let trailer = bytes.len() - 512;
    bytes[trailer + 224..trailer + 232].copy_from_slice(&u64::MAX.to_be_bytes());
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        rz_formats::open(&path).unwrap().entries(),
        Err(Error::LimitExceeded(_))
    ));
}

#[test]
fn cancels_dmg_catalog_and_file_reads_and_limits_total_output() {
    for name in ["dmg_hfs_zlib.dmg", "dmg_apfs_zlib.dmg"] {
        let mut reader = rz_formats::open(&fixture().with_file_name(name)).unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            reader.entries_with_progress(&cancel, &mut NoProgress),
            Err(Error::Cancelled)
        ));
        reader.set_read_control(CancelToken::new(), 1);
        assert!(matches!(reader.entries(), Err(Error::LimitExceeded(_))));
        reader.set_read_control(CancelToken::new(), u64::MAX);
        let entry = reader
            .entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.size > 0)
            .unwrap();
        let cancel = CancelToken::new();
        reader.set_read_control(cancel.clone(), u64::MAX);
        let mut stream = reader.open(&entry).unwrap();
        cancel.cancel();
        assert!(stream.read(&mut [0u8; 1]).is_err());
    }
}

#[test]
fn verifies_partition_checksums_even_when_the_master_checksum_is_valid() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bad-partition.dmg");
    let mut bytes = std::fs::read(fixture()).unwrap();
    let trailer = bytes.len() - 512;
    let xml_offset =
        u64::from_be_bytes(bytes[trailer + 216..trailer + 224].try_into().unwrap()) as usize;
    let xml_length =
        u64::from_be_bytes(bytes[trailer + 224..trailer + 232].try_into().unwrap()) as usize;
    let mut cursor = xml_offset;
    let xml_end = xml_offset + xml_length;
    let mut master = crc32fast::Hasher::new();
    let mut changed = false;
    while let Some(found) = bytes[cursor..xml_end]
        .windows(6)
        .position(|part| part == b"<data>")
    {
        let start = cursor + found + 6;
        let end = start
            + bytes[start..xml_end]
                .windows(7)
                .position(|part| part == b"</data>")
                .unwrap();
        let positions: Vec<_> = (start..end)
            .filter(|index| !bytes[*index].is_ascii_whitespace())
            .collect();
        let encoded: Vec<_> = positions.iter().map(|index| bytes[*index]).collect();
        let mut decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        if decoded.starts_with(b"mish") {
            if !changed {
                decoded[72] ^= 1;
                let encoded = base64::engine::general_purpose::STANDARD.encode(&decoded);
                assert_eq!(encoded.len(), positions.len());
                for (index, value) in positions.into_iter().zip(encoded.bytes()) {
                    bytes[index] = value;
                }
                changed = true;
            }
            master.update(&decoded[72..76]);
        }
        cursor = end + 7;
    }
    assert!(changed);
    bytes[trailer + 360..trailer + 364].copy_from_slice(&master.finalize().to_be_bytes());
    std::fs::write(&path, bytes).unwrap();
    let mut reader = rz_formats::open(&path).unwrap();
    assert!(reader.entries().is_ok());
    assert!(matches!(
        reader.verify_integrity(&CancelToken::new(), &mut NoProgress, 1 << 30),
        Err(Error::Corrupt(message)) if message.contains("partition CRC-32")
    ));
}

#[test]
fn checksum_verification_respects_cancellation_and_budget() {
    let mut reader = rz_formats::open(&fixture()).unwrap();
    reader.entries().unwrap();
    assert!(matches!(
        reader.verify_integrity(&CancelToken::new(), &mut NoProgress, 1),
        Err(Error::LimitExceeded(_))
    ));
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(matches!(
        reader.verify_integrity(&cancel, &mut NoProgress, u64::MAX),
        Err(Error::Cancelled)
    ));
    reader
        .verify_integrity(&CancelToken::new(), &mut NoProgress, 1 << 30)
        .unwrap();
}
