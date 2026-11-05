#![no_main]
mod bounded_archive;

libfuzzer_sys::fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 4096 {
        return;
    }
    let mut image =
        include_bytes!("../../crates/rz-formats/tests/fixtures/dmg_apfs_compressed.dmg").to_vec();
    let start = 40 * 512 + 378 * 4096;
    let fork = &mut image[start..start + 1526];
    let length = fork.len();
    let offset = bytes
        .first()
        .zip(bytes.get(1))
        .map_or(0, |(&a, &b)| u16::from_le_bytes([a, b]) as usize % length);
    for (index, byte) in bytes.iter().skip(2).enumerate() {
        fork[(offset + index) % length] ^= byte;
    }
    bounded_archive::exercise(&image, "dmg");
});
