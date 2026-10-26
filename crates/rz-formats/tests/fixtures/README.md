# Long-tail format fixtures

- `longtail_iso9660.iso.gz` is a gzip-compressed ISO 9660/Joliet image generated with macOS `hdiutil makehybrid` from the text file `HELLO.TXT` containing `RustZIP ISO fixture`. The image is generated test data with no third-party payload.
- `longtail_lha_lh0.lzh` is libarchive's uncompressed LHA header-level 0 test fixture. It contains dummy text, directories, and relative symlinks. The fixture is BSD-2-Clause test data. [Source test](https://github.com/libarchive/libarchive/blob/master/libarchive/test/test_read_format_lha.c), [encoded fixture](https://github.com/libarchive/libarchive/blob/master/libarchive/test/test_read_format_lha_lh0.lzh.uu).
