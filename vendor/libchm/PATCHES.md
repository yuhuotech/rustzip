# Vendored `libchm`

- Upstream: [`trypsynth/libchm`](https://github.com/trypsynth/libchm), MIT
- Upstream revision: `927cec11c4cf8165d28731fe1b3e07b40f8ddf5e`
- Upstream crate version: `0.3.0`
- Local changes: bounded ranged reads into caller buffers, cancellation checkpoints during LZX block replay, cancellable directory enumeration, and checked limits for reset tables (8 MiB), compressed blocks (4 MiB), directory metadata (16 MiB / 100,000 entries), and whole-entry compatibility reads (64 MiB).

Keep the upstream MIT license in `LICENSE`. Review upstream changes before updating this copy; RustZIP relies on the local methods to keep entry output and parsing work bounded.
