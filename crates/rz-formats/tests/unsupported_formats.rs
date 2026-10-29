#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_core::Error;
use rz_formats::open;

#[test]
fn wim_and_dmg_magic_report_unsupported_by_content() {
    let dir = tempfile::tempdir().unwrap();
    let mut dmg = vec![0u8; 1024];
    dmg[512..516].copy_from_slice(b"koly");
    let samples = [
        ("wim-content.bin", b"MSWIM\0\0\0".to_vec()),
        ("dmg-content.bin", dmg),
    ];

    for (name, bytes) in samples {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(open(&path), Err(Error::Unsupported(_))), "{name}");
    }
}
