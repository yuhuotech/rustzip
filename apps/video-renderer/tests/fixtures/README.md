# Video preview samples

- `small-h264.mp4` and `small-h264.mov` are two-frame 64×64 solid-red H.264 clips generated with FFmpeg's `lavfi` color source and `libx264`. They contain no third-party media content and exercise the MP4 and QuickTime atom readers.
- `mp4-box-loop.mp4` is a 1,582-byte malformed MP4 minimized from a local ASan fuzz timeout caused by no-progress nested box parsing. It verifies that the preview reader stops after its bounded I/O operation budget.
- `zero-samples-per-chunk.mp4` is a 1,614-byte minimized fuzz crash with an `stsc` entry that declares zero samples per chunk. It reproduces a divide-by-zero panic in `mp4` 0.14.0's sample-offset calculation; RustZIP rejects the invalid table before reading a sample.
- `missing-stts-entry.mp4` is a 1,561-byte minimized fuzz crash with a keyframe missing from its `stts` time table. It reproduces an unchecked `unwrap()` in `mp4` 0.14.0's sample-time calculation; RustZIP rejects the incomplete table before reading a sample.
- `extended-box-size.mp4` is a 920-byte minimized fuzz OOM with a nested `hdlr` box that declares a 64-bit size beyond its parent. The bounded box-tree preflight rejects extended-size boxes before the `mp4` parser can allocate from that declaration.
