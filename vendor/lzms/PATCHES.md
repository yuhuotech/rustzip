# RustZIP LZMS decoder

`src/decoder.rs` is from QLIC `rust/qlic-decoder/src/lzms.rs` at
f507ac0343b77c8cbaee1bc1d99920c694c25e4f (Apache-2.0). QLIC states that its
original clean-room LZMS decoder is by Deployment Theory (MIT).
Both license texts and attribution are retained.

RustZIP exposes the existing raw block decoder, applying caller-supplied WIM
chunk size bounds before allocation.
