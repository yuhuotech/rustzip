//! Throughput benchmark: `cargo bench -p rz-engine`.
//! Prints MB/s for compress and extract so regressions are visible in CI logs.

use rz_core::{CancelToken, CreateOptions, NoProgress};
use rz_engine::{compress, extract, ExtractOptions};
use rz_formats::OutputFormat;
use std::fs;
use std::path::Path;
use std::time::Instant;

const FILES: usize = 64;
const FILE_SIZE: usize = 1 << 20;

/// Text-like data: compressible, but not trivially so.
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
    bytes as f64 / (1 << 20) as f64 / secs
}

fn main() {
    let Ok(tmp) = std::env::temp_dir().canonicalize() else {
        return;
    };
    let root = tmp.join(format!("rz-bench-{}", std::process::id()));
    let src = root.join("data");
    if fs::create_dir_all(&src).is_err() {
        return;
    }
    for i in 0..FILES {
        let _ = fs::write(src.join(format!("f{i:03}.txt")), make_data(i as u64 + 1));
    }
    let total = FILES * FILE_SIZE;
    println!(
        "{FILES} files x {} MiB = {} MiB",
        FILE_SIZE >> 20,
        total >> 20
    );
    println!(
        "{:<10} {:>6} {:>12} {:>12} {:>8}",
        "format", "level", "pack MB/s", "unpack MB/s", "ratio"
    );

    for (name, format) in [("zip", OutputFormat::Zip), ("tar.gz", OutputFormat::TarGz)] {
        for level in [1, 6] {
            let archive = root.join(format!("b-{level}.{name}"));
            let opts = CreateOptions {
                level,
                password: None,
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
            if done.is_err() {
                println!("{name}: compress failed");
                continue;
            }
            let packed = fs::metadata(&archive).map(|m| m.len()).unwrap_or(1) as f64;

            let out = root.join(format!("out-{level}-{name}"));
            let t = Instant::now();
            if let Ok(mut r) = rz_formats::open(&archive) {
                let _ = extract(
                    &mut *r,
                    &out,
                    &ExtractOptions::default(),
                    &CancelToken::new(),
                    &mut NoProgress,
                );
            }
            let unpack = t.elapsed().as_secs_f64();
            println!(
                "{:<10} {:>6} {:>12.0} {:>12.0} {:>7.1}%",
                name,
                level,
                mbps(total, pack),
                mbps(total, unpack),
                packed / total as f64 * 100.0
            );
        }
    }
    cleanup(&root);
}

fn cleanup(root: &Path) {
    let _ = fs::remove_dir_all(root);
}
