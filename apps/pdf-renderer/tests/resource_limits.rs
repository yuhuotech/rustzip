#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn pdf_with_content(content: &[u8], compressed: bool) -> Vec<u8> {
    pdf_with_page_size(content, compressed, 200, 200)
}

fn pdf_with_page_size(
    content: &[u8],
    compressed: bool,
    page_width: u32,
    page_height: u32,
) -> Vec<u8> {
    let content_header = if compressed {
        format!(
            "<< /Length {} /Filter /FlateDecode >>\nstream\n",
            content.len()
        )
    } else {
        format!("<< /Length {} >>\nstream\n", content.len())
    };
    let mut content_object = content_header.into_bytes();
    content_object.extend_from_slice(content);
    content_object.extend_from_slice(b"\nendstream");
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_width} {page_height}] /Resources << >> /Contents 4 0 R >>"
        )
        .into_bytes(),
        content_object,
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn compression_bomb_pdf(decoded_bytes: usize) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    let zeros = [0_u8; 64 * 1024];
    for _ in 0..(decoded_bytes / zeros.len()) {
        encoder.write_all(&zeros).unwrap();
    }
    pdf_with_content(&encoder.finish().unwrap(), true)
}

fn run_helper(pdf: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rz-pdf-renderer"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(pdf).unwrap();
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

fn run_bomb_in_isolated_helper(pdf: &[u8]) -> std::process::ExitStatus {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rz-pdf-renderer"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(pdf).unwrap();
    drop(child.stdin.take());

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("resource-limited PDF helper did not stop the bomb before timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn helper_renders_a_small_pdf_to_bounded_png() {
    let pdf = pdf_with_content(b"0 0 1 rg\n10 10 180 180 re\nf\n", false);
    let output = run_helper(&pdf);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(output.stdout.len() <= 8 * 1024 * 1024);
}

#[test]
fn helper_stops_waiting_when_its_input_pipe_stays_open() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rz-pdf-renderer"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _open_input = child.stdin.take().unwrap();

    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(!status.success());
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("PDF helper did not enforce its own process timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn helper_scales_huge_page_boxes_before_rasterization() {
    let pdf = pdf_with_page_size(b"", false, 1_000_000, 1_000_000);
    let output = run_helper(&pdf);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stdout.starts_with(b"\x89PNG\r\n\x1a\n"));
    let width = u32::from_be_bytes(output.stdout[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(output.stdout[20..24].try_into().unwrap());
    assert!(u64::from(width) * u64::from(height) <= 16_777_216);
    assert!(width <= 8192 && height <= 8192);
}

#[test]
fn compressed_stream_bomb_is_stopped_in_the_helper_process() {
    let pdf = compression_bomb_pdf(300 * 1024 * 1024);
    let status = run_bomb_in_isolated_helper(&pdf);
    assert!(!status.success());
}
