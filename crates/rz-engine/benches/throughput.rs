//! Throughput benchmark: `cargo bench -p rz-engine`.
//!
//! Prints MB/s for compress and extract per format, level and thread count, so a
//! regression shows up in CI logs. Data is text-like: compressible, not trivially so.
//! Set `RZ_BENCH_MB` to change the data size (default 128).

use rz_core::{CancelToken, CreateOptions, NoProgress};
use rz_engine::{compress, extract, ExtractOptions};
use rz_formats::OutputFormat;
use std::fs;
use std::path::Path;
use std::time::Instant;

const FILE_SIZE: usize = 1 << 20;

fn make_data(seed: u64) -> Vec<u8> {
    const WORDS: [&str; 12] = [
        "archive ",
        "rust ",
        "zip ",
        "stream ",
        "buffer ",
        "entry ",
        "header ",
        "block ",
        "compress ",
        "extract ",
        "path ",
        "size ",
    ];
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(FILE_SIZE + 16);
    while out.len() < FILE_SIZE {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(WORDS[(x % 12) as usize].as_bytes());
    }
    out.truncate(FILE_SIZE);
    out
}

fn mbps(bytes: usize, secs: f64) -> f64 {
    bytes as f64 / (1 << 20) as f64 / secs.max(1e-9)
}

fn main() {
    let files: usize = std::env::var("RZ_BENCH_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    let root = std::env::temp_dir().join(format!("rz-bench-{}", std::process::id()));
    let src = root.join("data");
    if fs::create_dir_all(&src).is_err() {
        return;
    }
    for i in 0..files {
        let _ = fs::write(src.join(format!("f{i:04}.txt")), make_data(i as u64 + 1));
    }
    let total = files * FILE_SIZE;
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    println!("{files} files x 1 MiB = {} MiB, {cpus} CPUs", total >> 20);
    println!(
        "{:<9} {:>5} {:>7} {:>11} {:>13} {:>7}",
        "format", "level", "threads", "pack MB/s", "unpack MB/s", "size"
    );

    let cases: [(&str, u32); 8] = [
        ("zip", 6),
        ("tar.gz", 6),
        ("tar.zst", 3),
        ("tar.zst", 9),
        ("tar.xz", 6),
        ("7z", 3),
        ("7z", 6),
        ("tar.bz2", 6),
    ];
    for (ext, level) in cases {
        let Some(format) = OutputFormat::from_path(Path::new(&format!("a.{ext}"))) else {
            continue;
        };
        for threads in [1, cpus] {
            if threads == 1 && cpus == 1 && ext != "zip" {
                continue;
            }
            let archive = root.join(format!("b-{level}-{threads}.{ext}"));
            let opts = CreateOptions {
                level,
                threads,
                ..CreateOptions::default()
            };
            let t = Instant::now();
            let done = compress(
                std::slice::from_ref(&src),
                &archive,
                format,
                &opts,
                &CancelToken::new(),
                &mut NoProgress,
            );
            let pack = t.elapsed().as_secs_f64();
            if let Err(e) = done {
                println!("{ext}: compress failed: {e}");
                continue;
            }
            let packed = fs::metadata(&archive).map(|m| m.len()).unwrap_or(1) as f64;

            let out = root.join(format!("out-{level}-{threads}-{ext}"));
            let ex = ExtractOptions {
                threads,
                ..ExtractOptions::default()
            };
            let t = Instant::now();
            if let Ok(mut r) = rz_formats::open(&archive) {
                let _ = extract(&mut *r, &out, &ex, &CancelToken::new(), &mut NoProgress);
            }
            let unpack = t.elapsed().as_secs_f64();
            println!(
                "{:<9} {:>5} {:>7} {:>11.0} {:>13.0} {:>6.1}%",
                ext,
                level,
                threads,
                mbps(total, pack),
                mbps(total, unpack),
                packed / total as f64 * 100.0
            );
            let _ = fs::remove_dir_all(&out);
            let _ = fs::remove_file(&archive);
        }
    }
    let _ = fs::remove_dir_all(&root);
}
