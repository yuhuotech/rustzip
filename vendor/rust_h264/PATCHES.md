# rust_h264 local safety patch

- Upstream: `rust_h264` 0.5.0, commit `c18b35ec85f3023e85e6cee6d56fc399fc19c499`.
- License: MIT OR Apache-2.0. Upstream license texts remain in this directory and are shipped with RustZIP.
- Local patch: reject intra 4x4, 8x8 and 16x16 luma block coordinates outside the coded frame and return `DecodeError::InvalidSyntax` instead of indexing the frame plane unchecked. Regression inputs `apps/video-renderer/tests/fixtures/intra8x8-out-of-frame.mov` and `intra16x16-out-of-frame.mov` were minimized by the nightly video decoder fuzz target.
- The patch does not change valid H.264 output or the supported profiles.
- 2026-11-01: `decode_mvd_comp` now returns `Result`, rejects an excessive CABAC exponential prefix before shifting or adding, and validates the signed 16-bit MVD domain. All CABAC callers propagate `InvalidSyntax` instead of truncating invalid motion vectors. The original scheduled-Fuzz artifact `crash-8d6777a5ec00a74498153da79c8c65c9421bd8d1` is preserved as `cabac-mvd-overflow.mp4` in renderer/helper regression tests and Fuzz seeds.
- Validation: the upstream library suite passed 205 tests and 6 doctests, with 2 upstream slow corpus tests ignored. Its 53 MiB test corpus was fetched temporarily from the pinned upstream revision for verification and is not included in the RustZIP source or packages.
