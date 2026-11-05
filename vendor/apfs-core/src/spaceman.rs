//! Space manager (`spaceman_phys_t`, type `OBJECT_TYPE_SPACEMAN 0x5`) and
//! allocation bitmaps.
//!
//! The space manager tracks which container blocks are allocated. It references
//! chunk-info blocks (`chunk_info_block_t`, type `SPACEMAN_CIB 0x7`) and
//! chunk-info-address blocks (`cib_addr_block_t`, type `SPACEMAN_CAB 0x6`) that
//! point at allocation bitmaps (`SPACEMAN_BITMAP 0x8`), plus free queues
//! (`SPACEMAN_FREE_QUEUE 0x9`) of blocks pending release. The forensic value is
//! the "is this physical block currently free?" predicate — input to
//! deleted-extent carve-candidate reasoning (a free bitmap is necessary but
//! **not** sufficient for recoverability; see the analyzer).

use std::io::{Read, Seek, SeekFrom};

use crate::bytes::{le_u32, le_u64};

// `spaceman_phys_t` field offsets (after the 32-byte obj_phys header).
const SM_BLOCKS_PER_CHUNK: usize = 36; // u32 (== 8 * sm_block_size)
const SM_CHUNKS_PER_CIB: usize = 40; // u32
const SM_DEV0: usize = 48; // spaceman_device_t sm_dev[0] (main device)
                           // `spaceman_device_t` field offsets (relative to the device base).
const DEV_BLOCK_COUNT: usize = 0; // u64
const DEV_CHUNK_COUNT: usize = 8; // u64
const DEV_CIB_COUNT: usize = 16; // u32
const DEV_CAB_COUNT: usize = 20; // u32
const DEV_ADDR_OFFSET: usize = 32; // u32 — byte offset within the spaceman block
                                   // `chunk_info_block_t` (CIB) offsets.
const CIB_CHUNK_INFO_COUNT: usize = 36; // u32
const CIB_CHUNK_INFO: usize = 40; // chunk_info_t[]
const CHUNK_INFO_LEN: usize = 32;
// `chunk_info_t` field offsets.
const CI_BITMAP_ADDR: usize = 24; // u64 — SPACEMAN_BITMAP paddr, or 0 if all-free

/// Read a block at `paddr` and verify its Fletcher-64 before trusting it (a
/// checksummed `obj_phys` object: spaceman, CIB, reaper, reap list).
pub(crate) fn read_obj_block<R: Read + Seek>(
    reader: &mut R,
    paddr: u64,
    block_size: usize,
) -> crate::Result<Vec<u8>> {
    let mut buf = vec![0u8; block_size];
    let offset = paddr.saturating_mul(block_size as u64);
    reader.seek(std::io::SeekFrom::Start(offset))?;
    reader.read_exact(&mut buf)?;
    let stored = crate::object::fletcher64_stored(&buf);
    let computed = crate::object::fletcher64_checksum(&buf);
    if stored != computed {
        return Err(crate::ApfsError::ChecksumMismatch {
            block: le_u64(&buf, 8),
            stored,
            computed,
        });
    }
    Ok(buf)
}

/// Query whether a physical block is currently marked **free** in the space
/// manager's allocation bitmaps.
///
/// Resolves `block` to its chunk, follows the main device's inline chunk-info
/// (CIB) address array to the owning `chunk_info_t`, and reads the bit for the
/// block in that chunk's `SPACEMAN_BITMAP`. Per the APFS format, a **set** bit
/// means *allocated*, a **clear** bit means *free* (apfsck:
/// `free = total − popcount(bitmap)`); a `ci_bitmap_addr` of 0 means the whole
/// chunk is free. The bitmap block is raw (no `obj_phys` header / checksum); the
/// spaceman and CIB blocks are checksum-verified before use.
///
/// `spaceman_paddr` is the live space manager's physical block (resolve it from
/// the container's checkpoint map via [`crate::ApfsContainer::spaceman_paddr`]).
///
/// # Errors
/// [`crate::ApfsError::FieldOutOfRange`] if `block` is past the device, or a
/// chunk/CIB index is inconsistent; [`crate::ApfsError::UnsupportedSpacemanCab`]
/// for the multi-TB CAB tier (`sm_cab_count > 0`), not yet implemented;
/// [`crate::ApfsError::ChecksumMismatch`] on a bad spaceman/CIB checksum;
/// [`crate::ApfsError::Io`] on a read/seek failure.
pub fn is_block_free<R: Read + Seek>(
    reader: &mut R,
    spaceman_paddr: u64,
    block: u64,
    block_size: usize,
) -> crate::Result<bool> {
    let sm = read_obj_block(reader, spaceman_paddr, block_size)?;

    let blocks_per_chunk = u64::from(le_u32(&sm, SM_BLOCKS_PER_CHUNK));
    let chunks_per_cib = u64::from(le_u32(&sm, SM_CHUNKS_PER_CIB));
    let block_count = le_u64(&sm, SM_DEV0 + DEV_BLOCK_COUNT);
    let chunk_count = le_u64(&sm, SM_DEV0 + DEV_CHUNK_COUNT);
    let cib_count = u64::from(le_u32(&sm, SM_DEV0 + DEV_CIB_COUNT));
    let cab_count = u64::from(le_u32(&sm, SM_DEV0 + DEV_CAB_COUNT));
    let addr_offset = le_u32(&sm, SM_DEV0 + DEV_ADDR_OFFSET) as usize;

    let range = |field, value, cap| crate::ApfsError::FieldOutOfRange {
        structure: "spaceman_phys",
        field,
        value,
        cap,
    };
    if blocks_per_chunk == 0 || chunks_per_cib == 0 {
        return Err(range("sm_blocks_per_chunk/sm_chunks_per_cib", 0, 1));
    }
    if block >= block_count {
        return Err(range("block", block, block_count));
    }
    // The CAB indirection tier (sm_cab_count > 0) is multi-TB-container only and
    // not yet implemented — fail loud rather than mis-resolve a chunk.
    if cab_count != 0 {
        return Err(crate::ApfsError::UnsupportedSpacemanCab { count: cab_count });
    }

    let chunk_index = block / blocks_per_chunk;
    if chunk_index >= chunk_count {
        return Err(range("chunk_index", chunk_index, chunk_count));
    }
    let cib_idx = chunk_index / chunks_per_cib;
    let within_cib = (chunk_index % chunks_per_cib) as usize;
    if cib_idx >= cib_count {
        return Err(range("cib_index", cib_idx, cib_count));
    }

    // Inline CIB address array (u64 paddrs) at `addr_offset` within the spaceman.
    let cib_paddr = le_u64(&sm, addr_offset + cib_idx as usize * 8);
    let cib = read_obj_block(reader, cib_paddr, block_size)?;

    let ci_count = le_u32(&cib, CIB_CHUNK_INFO_COUNT) as usize;
    if within_cib >= ci_count {
        return Err(range(
            "chunk_info_index",
            within_cib as u64,
            ci_count as u64,
        ));
    }
    let ci_off = CIB_CHUNK_INFO + within_cib * CHUNK_INFO_LEN;
    let bitmap_addr = le_u64(&cib, ci_off + CI_BITMAP_ADDR);

    // A zero bitmap address means the whole chunk is free (no bitmap allocated).
    if bitmap_addr == 0 {
        return Ok(true);
    }

    // The bitmap is a raw block (one bit per block, LSB-first), no obj_phys header.
    let mut bitmap = vec![0u8; block_size];
    let offset = bitmap_addr.saturating_mul(block_size as u64);
    reader.seek(std::io::SeekFrom::Start(offset))?;
    reader.read_exact(&mut bitmap)?;

    let bit = (block % blocks_per_chunk) as usize;
    let allocated = bitmap
        .get(bit / 8)
        .is_some_and(|byte| byte & (1u8 << (bit % 8)) != 0);
    Ok(!allocated)
}

/// Return maximal runs of the **free** bit value within a single chunk's
/// allocation bitmap as `(first_bit, run_len)` pairs (ascending).
///
/// `blocks` caps the number of valid bits — the bitmap block is block-sized
/// (one bit per block) so padding past `blocks` is ignored. `free_bit_is_one`
/// selects the polarity: APFS marks a **set** bit *allocated* and a **clear**
/// bit *free*, so the space-manager traversal passes `false`; the parameter
/// keeps the core reusable for the inverse (NTFS/ext4-style) convention. A byte
/// past the end of `bitmap` is treated as fully **allocated**, so free space
/// that cannot be read is never fabricated. Pure and panic-free.
#[must_use]
pub fn free_runs(bitmap: &[u8], blocks: u64, free_bit_is_one: bool) -> Vec<(u64, u64)> {
    // A missing byte reads as "all allocated": 0x00 when a set bit means free,
    // 0xFF when a clear bit means free (APFS).
    let allocated_fill: u8 = if free_bit_is_one { 0x00 } else { 0xFF };
    let mut runs = Vec::new();
    let mut start: Option<u64> = None;
    for i in 0..blocks {
        let byte = bitmap
            .get((i / 8) as usize)
            .copied()
            .unwrap_or(allocated_fill);
        let bit_set = byte & (1u8 << (i % 8) as u32) != 0;
        let is_free = bit_set == free_bit_is_one;
        match (is_free, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                runs.push((s, i - s));
                start = None;
            }
            (true, Some(_)) | (false, None) => {}
        }
    }
    if let Some(s) = start {
        runs.push((s, blocks.saturating_sub(s)));
    }
    runs
}

/// Append a free run, coalescing with the previous run when physically
/// contiguous so the enumeration stays maximal across chunk boundaries.
fn push_free_run(runs: &mut Vec<(u64, u64)>, first: u64, len: u64) {
    if len == 0 {
        return; // cov:unreachable: free_runs and whole-chunk pushes are always len>=1
    }
    if let Some(last) = runs.last_mut() {
        if last.0.saturating_add(last.1) == first {
            last.1 = last.1.saturating_add(len);
            return;
        }
    }
    runs.push((first, len));
}

/// Enumerate every currently-free physical block on the main device as maximal
/// `(first_block, block_count)` runs — ascending and merged across chunk
/// boundaries.
///
/// Walks the main device's inline chunk-info (CIB) address array; for each
/// chunk it reads the `SPACEMAN_BITMAP` (or treats a `ci_bitmap_addr` of 0 as a
/// wholly-free chunk) and collects the blocks whose bit is **clear** (APFS marks
/// a set bit *allocated*). The spaceman and CIB blocks are checksum-verified
/// before trust; the bitmap block is raw (no `obj_phys` header). The last chunk
/// is clamped to `sm_dev.block_count`, so trailing padding bits never appear as
/// free. This is the inverse predicate of [`is_block_free`], across the whole
/// device in one pass.
///
/// `spaceman_paddr` is the live space manager's physical block (resolve it from
/// the container's checkpoint map via [`crate::ApfsContainer::spaceman_paddr`]).
///
/// # Errors
/// [`crate::ApfsError::UnsupportedSpacemanCab`] for the multi-TB CAB tier
/// (`sm_cab_count > 0`), not yet implemented; [`crate::ApfsError::FieldOutOfRange`]
/// on inconsistent geometry; [`crate::ApfsError::ChecksumMismatch`] on a bad
/// spaceman/CIB checksum; [`crate::ApfsError::Io`] on a read/seek failure.
pub fn free_block_runs<R: Read + Seek>(
    reader: &mut R,
    spaceman_paddr: u64,
    block_size: usize,
) -> crate::Result<Vec<(u64, u64)>> {
    let sm = read_obj_block(reader, spaceman_paddr, block_size)?;

    let blocks_per_chunk = u64::from(le_u32(&sm, SM_BLOCKS_PER_CHUNK));
    let chunks_per_cib = u64::from(le_u32(&sm, SM_CHUNKS_PER_CIB));
    let block_count = le_u64(&sm, SM_DEV0 + DEV_BLOCK_COUNT);
    let chunk_count = le_u64(&sm, SM_DEV0 + DEV_CHUNK_COUNT);
    let cib_count = u64::from(le_u32(&sm, SM_DEV0 + DEV_CIB_COUNT));
    let cab_count = u64::from(le_u32(&sm, SM_DEV0 + DEV_CAB_COUNT));
    let addr_offset = le_u32(&sm, SM_DEV0 + DEV_ADDR_OFFSET) as usize;

    let range = |field, value, cap| crate::ApfsError::FieldOutOfRange {
        structure: "spaceman_phys",
        field,
        value,
        cap,
    };
    if blocks_per_chunk == 0 || chunks_per_cib == 0 {
        return Err(range("sm_blocks_per_chunk/sm_chunks_per_cib", 0, 1));
    }
    // The CAB indirection tier is multi-TB-container only and not yet
    // implemented — fail loud rather than mis-resolve a chunk.
    if cab_count != 0 {
        return Err(crate::ApfsError::UnsupportedSpacemanCab { count: cab_count });
    }

    let mut runs: Vec<(u64, u64)> = Vec::new();
    for cib_idx in 0..cib_count {
        let cib_paddr = le_u64(&sm, addr_offset + cib_idx as usize * 8);
        let cib = read_obj_block(reader, cib_paddr, block_size)?;
        let ci_count = le_u32(&cib, CIB_CHUNK_INFO_COUNT) as usize;

        for within in 0..ci_count {
            let chunk_index = cib_idx.saturating_mul(chunks_per_cib) + within as u64;
            if chunk_index >= chunk_count {
                break;
            }
            let chunk_base = chunk_index.saturating_mul(blocks_per_chunk);
            if chunk_base >= block_count {
                break;
            }
            let blocks_in_chunk = blocks_per_chunk.min(block_count - chunk_base);
            let ci_off = CIB_CHUNK_INFO + within * CHUNK_INFO_LEN;
            let bitmap_addr = le_u64(&cib, ci_off + CI_BITMAP_ADDR);

            // A zero bitmap address means the whole chunk is free.
            if bitmap_addr == 0 {
                push_free_run(&mut runs, chunk_base, blocks_in_chunk);
                continue;
            }

            let mut bitmap = vec![0u8; block_size];
            let offset = bitmap_addr.saturating_mul(block_size as u64);
            reader.seek(SeekFrom::Start(offset))?;
            reader.read_exact(&mut bitmap)?;
            for (start, len) in free_runs(&bitmap, blocks_in_chunk, false) {
                push_free_run(&mut runs, chunk_base + start, len);
            }
        }
    }
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const BS: usize = 4096;

    /// Build a checksum-valid `spaceman_phys` block at block 0 of a synthetic
    /// image, with the main-device geometry fields set. `cab_count` selects the
    /// (unsupported) CAB tier; `addr_offset` positions the inline CIB address
    /// array; `cib0` is the paddr written at CIB slot 0.
    #[allow(clippy::too_many_arguments)]
    fn spaceman_image(
        blocks_per_chunk: u32,
        chunks_per_cib: u32,
        block_count: u64,
        chunk_count: u64,
        cib_count: u32,
        cab_count: u32,
        addr_offset: u32,
        cib0: u64,
    ) -> Vec<u8> {
        // A 4-block image: [0]=spaceman, and room for cib/bitmap at 1..4.
        let mut img = vec![0u8; BS * 4];
        let sm = &mut img[0..BS];
        sm[24..28].copy_from_slice(&5u32.to_le_bytes()); // o_type = SPACEMAN (informational)
        sm[SM_BLOCKS_PER_CHUNK..SM_BLOCKS_PER_CHUNK + 4]
            .copy_from_slice(&blocks_per_chunk.to_le_bytes());
        sm[SM_CHUNKS_PER_CIB..SM_CHUNKS_PER_CIB + 4].copy_from_slice(&chunks_per_cib.to_le_bytes());
        let d = SM_DEV0;
        sm[d + DEV_BLOCK_COUNT..d + DEV_BLOCK_COUNT + 8]
            .copy_from_slice(&block_count.to_le_bytes());
        sm[d + DEV_CHUNK_COUNT..d + DEV_CHUNK_COUNT + 8]
            .copy_from_slice(&chunk_count.to_le_bytes());
        sm[d + DEV_CIB_COUNT..d + DEV_CIB_COUNT + 4].copy_from_slice(&cib_count.to_le_bytes());
        sm[d + DEV_CAB_COUNT..d + DEV_CAB_COUNT + 4].copy_from_slice(&cab_count.to_le_bytes());
        sm[d + DEV_ADDR_OFFSET..d + DEV_ADDR_OFFSET + 4]
            .copy_from_slice(&addr_offset.to_le_bytes());
        let ao = addr_offset as usize;
        sm[ao..ao + 8].copy_from_slice(&cib0.to_le_bytes());
        // Fletcher-64 over the spaceman block (stored in bytes 0..8).
        let cks = crate::object::fletcher64_checksum(&img[0..BS]);
        img[0..8].copy_from_slice(&cks.to_le_bytes());
        img
    }

    #[test]
    fn read_obj_block_rejects_a_bad_checksum() {
        // An all-zero block has a zero stored checksum but a non-zero computed one
        // (the header oid bytes differ) → ChecksumMismatch, never trusted.
        let mut img = vec![0u8; BS];
        img[8..16].copy_from_slice(&0x1234u64.to_le_bytes()); // perturb → cksum != 0
        let mut r = Cursor::new(img);
        let got = read_obj_block(&mut r, 0, BS);
        let Err(crate::ApfsError::ChecksumMismatch { block, .. }) = got else {
            unreachable!("bad checksum must be ChecksumMismatch, got {got:?}") // cov:unreachable
        };
        assert_eq!(block, 0x1234);
    }

    #[test]
    fn zero_geometry_is_out_of_range() {
        let img = spaceman_image(0, 0, 100, 10, 1, 0, 256, 0);
        let mut r = Cursor::new(img);
        assert!(matches!(
            is_block_free(&mut r, 0, 0, BS),
            Err(crate::ApfsError::FieldOutOfRange { .. })
        ));
    }

    #[test]
    fn block_past_device_is_out_of_range() {
        let img = spaceman_image(8, 4, 100, 10, 1, 0, 256, 0);
        let mut r = Cursor::new(img);
        let got = is_block_free(&mut r, 0, 999, BS);
        let Err(crate::ApfsError::FieldOutOfRange { field, value, .. }) = got else {
            unreachable!("block OOR ⇒ FieldOutOfRange: {got:?}") // cov:unreachable
        };
        assert_eq!(field, "block");
        assert_eq!(value, 999);
    }

    #[test]
    fn cab_tier_is_unsupported_and_loud() {
        let img = spaceman_image(8, 4, 100, 10, 1, 3, 256, 0);
        let mut r = Cursor::new(img);
        let got = is_block_free(&mut r, 0, 0, BS);
        let Err(crate::ApfsError::UnsupportedSpacemanCab { count }) = got else {
            unreachable!("CAB tier ⇒ UnsupportedSpacemanCab: {got:?}") // cov:unreachable
        };
        assert_eq!(count, 3);
    }

    #[test]
    fn chunk_index_past_chunk_count_is_out_of_range() {
        // block 64 with blocks_per_chunk 8 → chunk 8, but chunk_count is 2.
        let img = spaceman_image(8, 4, 1000, 2, 1, 0, 256, 0);
        let mut r = Cursor::new(img);
        let got = is_block_free(&mut r, 0, 64, BS);
        let Err(crate::ApfsError::FieldOutOfRange { field, .. }) = got else {
            unreachable!("chunk_index OOR ⇒ FieldOutOfRange: {got:?}") // cov:unreachable
        };
        assert_eq!(field, "chunk_index");
    }

    #[test]
    fn cib_index_past_cib_count_is_out_of_range() {
        // chunk 4 with chunks_per_cib 4 → cib 1, but cib_count is 1 (only cib 0).
        let img = spaceman_image(8, 4, 1000, 100, 1, 0, 256, 0);
        let mut r = Cursor::new(img);
        let got = is_block_free(&mut r, 0, 32, BS);
        let Err(crate::ApfsError::FieldOutOfRange { field, .. }) = got else {
            unreachable!("bad cib_index must be FieldOutOfRange, got {got:?}") // cov:unreachable
        };
        assert_eq!(field, "cib_index");
    }

    #[test]
    fn zero_bitmap_addr_means_whole_chunk_free() {
        // A valid path to a CIB whose chunk_info has ci_bitmap_addr == 0 → free.
        // Build the CIB at block 1 with one chunk_info (count 1), bitmap_addr 0.
        let mut img = spaceman_image(8, 4, 1000, 10, 1, 0, 256, 1);
        {
            let cib = &mut img[BS..2 * BS];
            cib[CIB_CHUNK_INFO_COUNT..CIB_CHUNK_INFO_COUNT + 4]
                .copy_from_slice(&1u32.to_le_bytes());
            // ci_bitmap_addr @ CIB_CHUNK_INFO + 24 left zero.
            let cks = crate::object::fletcher64_checksum(cib);
            cib[0..8].copy_from_slice(&cks.to_le_bytes());
        }
        let mut r = Cursor::new(img);
        assert!(
            is_block_free(&mut r, 0, 0, BS).expect("is_block_free"),
            "a zero ci_bitmap_addr marks the whole chunk free"
        );
    }

    #[test]
    fn chunk_info_index_past_count_is_out_of_range() {
        // A CIB whose chunk_info_count is 0, so within_cib (0) is not < 0.
        let mut img = spaceman_image(8, 4, 1000, 10, 1, 0, 256, 1);
        {
            let cib = &mut img[BS..2 * BS];
            // CIB_CHUNK_INFO_COUNT left 0.
            let cks = crate::object::fletcher64_checksum(cib);
            cib[0..8].copy_from_slice(&cks.to_le_bytes());
        }
        let mut r = Cursor::new(img);
        let got = is_block_free(&mut r, 0, 0, BS);
        let Err(crate::ApfsError::FieldOutOfRange { field, .. }) = got else {
            unreachable!("chunk_info_index OOR ⇒ FieldOutOfRange: {got:?}") // cov:unreachable
        };
        assert_eq!(field, "chunk_info_index");
    }

    // ---- free_runs (pure bitmap → free-block runs) --------------------------

    #[test]
    fn free_runs_apfs_polarity_clear_bit_is_free() {
        // APFS: a SET bit is allocated, a CLEAR bit is free (free_bit_is_one=false).
        // byte 0x1E = 0b0001_1110: bit0 clear(free), bits1..4 set(alloc),
        // bits5..7 clear(free) → free runs (0,1) and (5,3).
        let runs = free_runs(&[0x1E], 8, false);
        assert_eq!(runs, vec![(0, 1), (5, 3)]);
    }

    #[test]
    fn free_runs_inverse_polarity_set_bit_is_free() {
        // The inverse convention (set bit == free): the same byte flips meaning,
        // so the free runs are the complement: (1,4).
        let runs = free_runs(&[0x1E], 8, true);
        assert_eq!(runs, vec![(1, 4)]);
    }

    #[test]
    fn free_runs_honors_blocks_cap_and_ignores_padding() {
        // Only the first `blocks` bits are valid; padding past it is ignored.
        // 0xFF (all set = all allocated under APFS polarity) → no free runs even
        // though later bytes are all-free (0x00) padding.
        let runs = free_runs(&[0x00, 0x00], 3, false);
        // First 3 bits of 0x00 are clear → free → one run (0,3), not 16.
        assert_eq!(runs, vec![(0, 3)]);
    }

    #[test]
    fn free_runs_missing_bytes_are_allocated_never_fabricated() {
        // `blocks` exceeds the supplied bitmap: the missing byte must read as
        // fully ALLOCATED (never fabricate free space we cannot see). With APFS
        // polarity the fill is 0xFF, so blocks 8..16 are allocated → no run there.
        // Bitmap byte 0 = 0x00 → blocks 0..8 free.
        let runs = free_runs(&[0x00], 16, false);
        assert_eq!(runs, vec![(0, 8)]);
        // Inverse polarity: missing byte fills 0x00 (allocated when free==1) → same.
        let runs = free_runs(&[0xFF], 16, true);
        assert_eq!(runs, vec![(0, 8)]);
    }

    #[test]
    fn free_runs_empty_when_all_allocated() {
        assert!(free_runs(&[0xFF, 0xFF], 16, false).is_empty());
    }

    // ---- free_block_runs (full main-device traversal) -----------------------

    /// Build a 4-block image (0=spaceman, 1=CIB, 2=bitmap, 3=spare) with two
    /// chunks in one CIB: chunk 0 uses the bitmap at block 2, chunk 1 is
    /// wholly-free (`ci_bitmap_addr == 0`). Geometry: `blocks_per_chunk=8`,
    /// `chunks_per_cib=4`, `block_count=16`, `chunk_count=2`.
    fn two_chunk_image(chunk0_bitmap_byte: u8) -> Vec<u8> {
        let mut img = spaceman_image(8, 4, 16, 2, 1, 0, 256, 1);
        {
            let cib = &mut img[BS..2 * BS];
            cib[CIB_CHUNK_INFO_COUNT..CIB_CHUNK_INFO_COUNT + 4]
                .copy_from_slice(&2u32.to_le_bytes());
            // chunk_info[0].ci_bitmap_addr = block 2.
            let ci0 = CIB_CHUNK_INFO + CI_BITMAP_ADDR;
            cib[ci0..ci0 + 8].copy_from_slice(&2u64.to_le_bytes());
            // chunk_info[1].ci_bitmap_addr = 0 (whole chunk free).
            let cks = crate::object::fletcher64_checksum(cib);
            cib[0..8].copy_from_slice(&cks.to_le_bytes());
        }
        // Bitmap block (raw, no obj_phys header) for chunk 0.
        img[2 * BS] = chunk0_bitmap_byte;
        img
    }

    #[test]
    fn free_block_runs_merges_across_chunk_boundary() {
        // chunk 0 bitmap 0x1E → free blocks (0,1) and (5,3); chunk 1 wholly free
        // → (8,8). The tail of chunk 0 (5..8) is contiguous with all of chunk 1
        // (8..16), so they coalesce into (5,11). Result: [(0,1),(5,11)].
        let mut r = Cursor::new(two_chunk_image(0x1E));
        let runs = free_block_runs(&mut r, 0, BS).expect("free_block_runs");
        assert_eq!(runs, vec![(0, 1), (5, 11)]);
    }

    #[test]
    fn free_block_runs_all_allocated_chunk0_keeps_chunk1_free() {
        // chunk 0 fully allocated (0xFF) → no runs there; chunk 1 wholly free.
        let mut r = Cursor::new(two_chunk_image(0xFF));
        let runs = free_block_runs(&mut r, 0, BS).expect("free_block_runs");
        assert_eq!(runs, vec![(8, 8)]);
    }

    #[test]
    fn free_block_runs_rejects_cab_tier_loud() {
        // sm_cab_count > 0 (multi-TB CAB indirection) is unsupported — fail loud.
        let img = spaceman_image(8, 4, 16, 2, 1, 3, 256, 1);
        let mut r = Cursor::new(img);
        let got = free_block_runs(&mut r, 0, BS);
        let Err(crate::ApfsError::UnsupportedSpacemanCab { count }) = got else {
            unreachable!("CAB tier ⇒ UnsupportedSpacemanCab: {got:?}") // cov:unreachable
        };
        assert_eq!(count, 3);
    }

    #[test]
    fn free_block_runs_rejects_zero_geometry_loud() {
        let img = spaceman_image(0, 0, 16, 2, 1, 0, 256, 1);
        let mut r = Cursor::new(img);
        assert!(matches!(
            free_block_runs(&mut r, 0, BS),
            Err(crate::ApfsError::FieldOutOfRange { .. })
        ));
    }

    /// A one-chunk image (`block_count=8`, `chunk_count=1`) whose CIB claims
    /// `chunk_info_count` chunk-infos, chunk 0's bitmap wholly free at block 2.
    fn overcounted_cib_image(block_count: u64, chunk_count: u64, chunk_info_count: u32) -> Vec<u8> {
        let mut img = spaceman_image(8, 4, block_count, chunk_count, 1, 0, 256, 1);
        {
            let cib = &mut img[BS..2 * BS];
            cib[CIB_CHUNK_INFO_COUNT..CIB_CHUNK_INFO_COUNT + 4]
                .copy_from_slice(&chunk_info_count.to_le_bytes());
            let ci0 = CIB_CHUNK_INFO + CI_BITMAP_ADDR;
            cib[ci0..ci0 + 8].copy_from_slice(&2u64.to_le_bytes());
            let cks = crate::object::fletcher64_checksum(cib);
            cib[0..8].copy_from_slice(&cks.to_le_bytes());
        }
        img[2 * BS] = 0x00; // chunk 0 all free
        img
    }

    #[test]
    fn free_block_runs_stops_when_cib_overcounts_chunks() {
        // Adversarial: the CIB claims 2 chunk-infos but the device has 1 chunk.
        // The within=1 iteration's chunk_index (1) reaches chunk_count (1) and
        // the walk stops — only chunk 0's free blocks are reported.
        let mut r = Cursor::new(overcounted_cib_image(8, 1, 2));
        let runs = free_block_runs(&mut r, 0, BS).expect("free_block_runs");
        assert_eq!(runs, vec![(0, 8)]);
    }

    #[test]
    fn free_block_runs_stops_when_chunk_base_exceeds_block_count() {
        // Adversarial: chunk_count claims 2 but block_count is one chunk (8).
        // Chunk 1 would begin at block 8 == block_count, so the walk stops before
        // it — chunk 0's free blocks are still reported.
        let mut r = Cursor::new(overcounted_cib_image(8, 2, 2));
        let runs = free_block_runs(&mut r, 0, BS).expect("free_block_runs");
        assert_eq!(runs, vec![(0, 8)]);
    }
}
