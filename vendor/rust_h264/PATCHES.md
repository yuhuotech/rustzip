# rust_h264 local safety patch

- Upstream: `rust_h264` 0.5.0, commit `c18b35ec85f3023e85e6cee6d56fc399fc19c499`.
- License: MIT OR Apache-2.0. Upstream license texts remain in this directory and are shipped with RustZIP.
- Local patch: reject intra 4x4 luma block coordinates outside the coded frame and return `DecodeError::InvalidSyntax` instead of indexing the frame plane unchecked. Regression input `apps/video-renderer/tests/fixtures/intra4x4-out-of-frame.mov` was minimized by the nightly video decoder fuzz target.
- The patch does not change valid H.264 output or the supported profiles.
