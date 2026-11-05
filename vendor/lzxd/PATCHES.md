# RustZIP WIM LZX patch

Source: `lzxd` 0.2.7 by Lonami Exo, MIT OR Apache-2.0. The original license
files are retained. This library is independent of 7-Zip and wimlib.

RustZIP adds a WIM-specific independent chunk mode, WIM block-size framing,
the WIM E8 translation size, and bounds on decoded lengths and match offsets.
Chunks and resulting SHA-1 are validated by the WIM reader.
