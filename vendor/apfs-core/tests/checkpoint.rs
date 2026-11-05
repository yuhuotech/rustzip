//! Checkpoint-ring walk to the live container superblock, validated against the
//! REAL self-minted descriptor area (Tier 2). The carved head starts at NXSB
//! block 0, so physical block addresses map directly to byte offsets in the
//! fixture (`paddr * block_size`). See `tests/data/README.md`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Cursor;

use apfs_core::checkpoint::resolve_live_checkpoint;
use apfs_core::container::NxSuperblock;
use apfs_core::ApfsError;

const HEAD: &[u8] = include_bytes!("../../tests/data/apfs_nxsb_head.bin");
const BLOCK_SIZE: usize = 4096;

fn block0() -> &'static [u8] {
    &HEAD[0..BLOCK_SIZE]
}

#[test]
fn resolve_picks_highest_valid_xid_superblock() {
    // Block 0 (a copy) has xid 2. The descriptor area (blocks 1..8) holds the
    // checkpoint maps + NXSB copies; the live superblock is the highest-xid,
    // cksum-valid NXSB in that ring. Verified independently: block 4 @ xid 2
    // (omap_oid 343, fs_oid 1026) is the live one; block 2 @ xid 1 is older.
    let bootstrap = NxSuperblock::parse(block0()).expect("bootstrap NXSB");
    let mut reader = Cursor::new(HEAD);
    let live = resolve_live_checkpoint(&mut reader, &bootstrap).expect("resolve checkpoint");

    assert_eq!(live.xid, 2);
    // The chosen superblock sits at descriptor block 4 (paddr 4).
    assert_eq!(live.superblock_paddr, 4);
}

#[test]
fn resolved_superblock_reparses_consistently() {
    // Re-reading the chosen superblock block must yield the same live state the
    // block-0 copy described (Apple: block 0 is typically a copy of the latest).
    let bootstrap = NxSuperblock::parse(block0()).expect("bootstrap NXSB");
    let mut reader = Cursor::new(HEAD);
    let live = resolve_live_checkpoint(&mut reader, &bootstrap).expect("resolve");

    let off = live.superblock_paddr as usize * BLOCK_SIZE;
    let chosen = NxSuperblock::parse(&HEAD[off..off + BLOCK_SIZE]).expect("reparse chosen");
    assert_eq!(chosen.xid, live.xid);
    assert_eq!(chosen.omap_oid, 343);
    assert_eq!(chosen.fs_oids, vec![1026]);
    assert_eq!(chosen.block_size, 4096);
}

#[test]
fn empty_descriptor_ring_fails_loud_not_empty_ok() {
    // Bootstrap that points the descriptor area at all-zero blocks: no
    // cksum-valid NXSB exists, so resolution must be a loud NoValidSuperblock
    // (fleet fail-loud-on-bootstrap), never Ok with an empty/garbage result.
    let mut blk = block0().to_vec();
    // Repoint the descriptor area into the zero-filled tail of the fixture
    // (blocks 5..8 are zero in this carve) and shrink it so every slot is zero.
    // Simpler: build a bootstrap whose desc area is one zero block.
    // We mutate the raw bytes then re-checksum so parse() accepts it.
    // desc_base @112 -> 15 (a zero block in the head), desc_blocks @104 -> 1.
    blk[104..108].copy_from_slice(&1u32.to_le_bytes());
    blk[112..120].copy_from_slice(&15u64.to_le_bytes());
    // recompute Fletcher-64 so the mutated bootstrap still parses
    let cks = apfs_core::object::fletcher64_checksum(&blk);
    blk[0..8].copy_from_slice(&cks.to_le_bytes());
    let bootstrap = NxSuperblock::parse(&blk).expect("mutated bootstrap parses");

    let mut reader = Cursor::new(HEAD);
    match resolve_live_checkpoint(&mut reader, &bootstrap) {
        Err(ApfsError::NoValidSuperblock { checked, .. }) => assert!(checked >= 1),
        other => panic!("expected NoValidSuperblock, got {other:?}"),
    }
}

#[test]
fn tree_backed_descriptor_area_is_rejected_loudly() {
    // A descriptor area stored as a B-tree (high bit of nx_xp_desc_blocks) needs
    // B-tree resolution (phase P2). P1 must reject it loudly, not silently
    // mis-read a tree oid as a contiguous base.
    let mut blk = block0().to_vec();
    // set the tree flag on nx_xp_desc_blocks
    let raw = u32::from_le_bytes(blk[104..108].try_into().unwrap()) | 0x8000_0000;
    blk[104..108].copy_from_slice(&raw.to_le_bytes());
    let cks = apfs_core::object::fletcher64_checksum(&blk);
    blk[0..8].copy_from_slice(&cks.to_le_bytes());
    let bootstrap = NxSuperblock::parse(&blk).expect("tree-flagged bootstrap parses");
    assert!(bootstrap.xp_desc_is_tree());

    let mut reader = Cursor::new(HEAD);
    assert!(resolve_live_checkpoint(&mut reader, &bootstrap).is_err());
}

#[test]
fn descriptor_blocks_past_the_image_end_are_skipped_not_fatal() {
    // Point the descriptor ring at blocks beyond the reader's end: each read hits
    // EOF, which read_block turns into a per-slot miss (skip), so the walk finds
    // no valid NXSB and fails loud with NoValidSuperblock — never an Io panic.
    let mut blk = block0().to_vec();
    // desc_base @112 -> block 10_000 (far past this ~17-block head), 2 slots.
    blk[104..108].copy_from_slice(&2u32.to_le_bytes());
    blk[112..120].copy_from_slice(&10_000u64.to_le_bytes());
    let cks = apfs_core::object::fletcher64_checksum(&blk);
    blk[0..8].copy_from_slice(&cks.to_le_bytes());
    let bootstrap = NxSuperblock::parse(&blk).expect("mutated bootstrap parses");

    let mut reader = Cursor::new(HEAD);
    assert!(matches!(
        resolve_live_checkpoint(&mut reader, &bootstrap),
        Err(ApfsError::NoValidSuperblock { .. })
    ));
}

#[test]
fn out_of_range_block_size_is_rejected_loudly() {
    // A corrupt nx_block_size outside the spec range (4096..=65536) would drive
    // every checkpoint seek to the wrong place; resolution must reject it loud
    // with the offending value, never trust it.
    let mut blk = block0().to_vec();
    blk[36..40].copy_from_slice(&131_072u32.to_le_bytes()); // nx_block_size (2x max)
    let cks = apfs_core::object::fletcher64_checksum(&blk);
    blk[0..8].copy_from_slice(&cks.to_le_bytes());
    let bootstrap = NxSuperblock::parse(&blk).expect("mutated bootstrap parses");
    assert_eq!(bootstrap.block_size, 131_072);

    let mut reader = Cursor::new(HEAD);
    match resolve_live_checkpoint(&mut reader, &bootstrap) {
        Err(ApfsError::FieldOutOfRange { field, value, .. }) => {
            assert_eq!(field, "nx_block_size");
            assert_eq!(value, 131_072);
        }
        other => panic!("expected FieldOutOfRange(nx_block_size), got {other:?}"),
    }
}
