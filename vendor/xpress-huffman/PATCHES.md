# RustZIP XPRESS-Huffman patch

Source: crates.io xpress-huffman 0.1.3, Apache-2.0. Original license and upstream
regression vectors are preserved. This code is independent of wimlib and 7-Zip.

RustZIP continues decoding symbols still buffered after the byte cursor reaches
EOF, tracks real buffered bits to reject truncation, and refuses backreferences
that would exceed the declared output or 64 KiB block. This fixes short WIM
metadata output and bounds allocations before copying an extended match.
