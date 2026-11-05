# RustZIP NSIS bzip2 adapter

Original module: `nsis` 0.6.0 `src/decompress/bzip2.rs`, Apache-2.0, from
https://github.com/ATRAPSLLC/nsis-rs at
f20d03c407674dd670787ebcb2fa9e5f0d65a0ad. The upstream module states that it
ports NSIS's bzip2 decompressor, which has the bzip2 license. Both licenses are
included in the distributed format-reader notice.

RustZIP adapts the input to a cancellable stream and decodes one bounded block
at a time. The outer NSIS archive provides CRC verification.
