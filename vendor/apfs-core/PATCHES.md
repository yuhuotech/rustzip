RustZIP uses apfs-core 0.2.6 under Apache-2.0. The local patch bounds
directory, extent and extended-attribute collection before growing their
vectors, because filesystem metadata is untrusted input.
