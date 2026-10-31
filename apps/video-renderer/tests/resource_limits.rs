#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use image::GenericImageView as _;
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const VIDEO: &[u8] = include_bytes!("../tests/fixtures/small-h264.mp4");
const OUT_OF_FRAME: &[u8] = include_bytes!("../tests/fixtures/intra4x4-out-of-frame.mov");

#[test]
fn helper_renders_one_frame_to_png() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rz-video-renderer"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(VIDEO).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.starts_with(b"\x89PNG\r\n\x1a\n"));
    let image = image::load_from_memory(&output.stdout).unwrap();
    assert_eq!(image.dimensions(), (64, 64));
}

#[test]
fn helper_rejects_h264_blocks_outside_the_coded_frame() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rz-video-renderer"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(OUT_OF_FRAME).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn helper_rejects_input_over_the_byte_limit() {
    let input = vec![0u8; rz_video_renderer::MAX_INPUT_BYTES + 1];
    let mut child = Command::new(env!("CARGO_BIN_EXE_rz-video-renderer"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn helper_stops_waiting_when_its_input_pipe_stays_open() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rz-video-renderer"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    // The helper deadline is 10 seconds; leave room for a busy CI runner to
    // schedule the watchdog thread and reap the child process.
    let exit_bound = Duration::from_secs(16);
    while started.elapsed() < exit_bound {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("video helper stayed alive after its watchdog deadline");
}
