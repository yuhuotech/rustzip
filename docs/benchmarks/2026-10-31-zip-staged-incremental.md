# ZIP staged incremental edit benchmark

## Environment and method

- Host: macOS 27.0.1, arm64; Rust 1.96.0.
- The benchmark generated two ZIP entries from the same 1 MiB deterministic pseudorandom block repeated to the selected size. Each operation deleted `delete.bin` and retained `keep.bin`.
- `staged-incremental` used the normal ZIP edit path. Its reflink capability probe succeeded on the temporary-file volume. `full-repack` forced the existing unpack / repack path by specifying UTF-8 encoding.
- Each row is the median of three edit operations. The benchmark checked that both results produced the same entry names and SHA-256 content hashes; those checks run after the timed operation.

## Results

| Retained file | Staged incremental | Full repack | Ratio |
|---:|---:|---:|---:|
| 8 MiB | 0.013 s | 0.120 s | 9.2× |
| 40 MiB | 0.032 s | 0.242 s | 7.6× |
| 128 MiB | 0.088 s | 0.785 s | 8.9× |

These measurements cover one macOS filesystem and one repeated-data shape. They confirm the expected path on a volume where reflink succeeded; they do not establish Windows / Linux performance, and the repeated block is more compressible than typical user data. Cross-platform external-tool checks remain part of acceptance.
