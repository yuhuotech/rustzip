# Video preview samples

- `small-h264.mp4` and `small-h264.mov` are two-frame 64×64 solid-red H.264 clips generated with FFmpeg's `lavfi` color source and `libx264`. They contain no third-party media content and exercise the MP4 and QuickTime atom readers.
- `mp4-box-loop.mp4` is a 1,582-byte malformed MP4 minimized from a local ASan fuzz timeout caused by no-progress nested box parsing. It verifies that the preview reader stops after its bounded I/O operation budget.
