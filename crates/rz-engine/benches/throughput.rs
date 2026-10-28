//! Throughput benchmark: `cargo bench -p rz-engine`.
//!
//! Prints MB/s for compress and extract per format, level and thread count, so a
//! regression shows up in CI logs. Data is text-like: compressible, not trivially so.
//! Set `RZ_BENCH_MB` to change the data size (default 128).

use rz_core::{CancelToken, CreateOptions, NoProgress};
use rz_engine::{compress, extract, transform, ExtractOptions, TransformOptions};
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

/// A benchmark result that is only printed is invisible to CI: every failure
/// must exit non-zero, and throughput counts only after the output is verified.
fn die(what: &str, e: impl std::fmt::Display) -> ! {
    eprintln!("bench failed: {what}: {e}");
    std::process::exit(1);
}

/// Confirms an extraction reproduced the source tree: same files, same sizes,
/// and byte-equal content for every file. The archive holds the source
/// folder itself, so the files reappear one level below `out`.
fn verify_extraction(out: &Path, src: &Path, files: usize) {
    let base = out.join(src.file_name().unwrap_or_default());
    for i in 0..files {
        let name = format!("f{i:04}.txt");
        let a = fs::metadata(src.join(&name)).unwrap_or_else(|e| die(&name, e));
        let b = fs::metadata(base.join(&name)).unwrap_or_else(|e| die(&name, e));
        if a.len() != b.len() {
            die(&name, format!("size {} != {}", a.len(), b.len()));
        }
        match (fs::read(src.join(&name)), fs::read(base.join(&name))) {
            (Ok(a), Ok(b)) if a == b => {}
            (Err(e), _) | (_, Err(e)) => die(&name, e),
            _ => die(&name, "content differs from the source"),
        }
    }
}

fn main() {
    let files: usize = std::env::var("RZ_BENCH_MB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    let root = std::env::temp_dir().join(format!("rz-bench-{}", std::process::id()));
    let src = root.join("data");
    fs::create_dir_all(&src).unwrap_or_else(|e| die("create source", e));
    for i in 0..files {
        fs::write(src.join(format!("f{i:04}.txt")), make_data(i as u64 + 1))
            .unwrap_or_else(|e| die("write source", e));
    }
    let total = files * FILE_SIZE;
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    println!("{files} files x 1 MiB = {} MiB, {cpus} CPUs", total >> 20);
    println!(
        "{:<9} {:>5} {:>7} {:>11} {:>13} {:>7}",
        "format", "level", "threads", "pack MB/s", "unpack MB/s", "size"
    );

    let cases: [(&str, &str, u32, Option<u64>); 10] = [
        ("zip", "zip", 6, None),
        ("tar.gz", "tar.gz", 6, None),
        ("tar.zst", "tar.zst", 3, None),
        ("tar.zst", "tar.zst", 9, None),
        ("tar.xz", "tar.xz", 6, None),
        ("7z", "7z", 3, None),
        ("7z", "7z", 6, None),
        ("7z-block", "7z", 3, Some(1 << 20)),
        ("tar.bz2", "tar.bz2", 6, None),
        ("tar.lz4", "tar.lz4", 6, None),
    ];
    for (label, ext, level, solid_block_size) in cases {
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
                solid_block_size,
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
                die("compress", format!("{ext} level {level}: {e}"));
            }
            let packed = fs::metadata(&archive).map(|m| m.len()).unwrap_or(1) as f64;

            let out = root.join(format!("out-{level}-{threads}-{ext}"));
            let ex = ExtractOptions {
                threads,
                ..ExtractOptions::default()
            };
            let t = Instant::now();
            let mut r = rz_formats::open(&archive).unwrap_or_else(|e| die("open", e));
            extract(&mut *r, &out, &ex, &CancelToken::new(), &mut NoProgress)
                .unwrap_or_else(|e| die("extract", format!("{ext} level {level}: {e}")));
            let unpack = t.elapsed().as_secs_f64();
            // Throughput only counts once the output is known to be correct.
            verify_extraction(&out, &src, files);
            println!(
                "{:<9} {:>5} {:>7} {:>11.0} {:>13.0} {:>6.1}%",
                label,
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
    println!("\nZIP / 7z volume creation and verification");
    let volume_size = 64 * 1024;
    for ext in ["zip", "7z"] {
        let archive = root.join(format!("volumes.{ext}"));
        let format = OutputFormat::from_path(&archive).unwrap_or_else(|| die("volume format", ext));
        let opts = CreateOptions {
            volume_size: Some(volume_size),
            threads: cpus,
            ..CreateOptions::default()
        };
        let t = Instant::now();
        compress(
            std::slice::from_ref(&src),
            &archive,
            format,
            &opts,
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap_or_else(|e| die("volume compress", format!("{ext}: {e}")));
        let pack = t.elapsed().as_secs_f64();
        let open_path = if ext == "7z" {
            root.join("volumes.7z.001")
        } else {
            archive.clone()
        };
        let out = root.join(format!("volumes-out-{ext}"));
        let t = Instant::now();
        let mut reader = rz_formats::open(&open_path).unwrap_or_else(|e| die("open volumes", e));
        extract(
            &mut *reader,
            &out,
            &ExtractOptions::default(),
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap_or_else(|e| die("extract volumes", format!("{ext}: {e}")));
        let unpack = t.elapsed().as_secs_f64();
        verify_extraction(&out, &src, files);
        println!(
            "{ext:<9} {:>5} {:>11.0} {:>13.0} MB/s, volume size {} KiB",
            "vol",
            mbps(total, pack),
            mbps(total, unpack),
            volume_size >> 10
        );
    }
    println!("\nStreaming conversion / recovery (includes output verification)");
    let source = root.join("transform-source.zip");
    if let Err(e) = compress(
        std::slice::from_ref(&src),
        &source,
        OutputFormat::Zip,
        &CreateOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    ) {
        die("compress transform-source", e);
    }
    {
        for (ext, recover) in [
            ("zip", false),
            ("7z", false),
            ("tar.zst", false),
            ("zip", true),
        ] {
            let output = root.join(format!("transform-{recover}.{ext}"));
            let Some(format) = OutputFormat::from_path(&output) else {
                continue;
            };
            let opts = TransformOptions {
                format,
                create: CreateOptions::default(),
                limits: ExtractOptions::default(),
                encoding: None,
            };
            let t = Instant::now();
            match transform(
                &source,
                &output,
                &opts,
                recover,
                &CancelToken::new(),
                &mut NoProgress,
            ) {
                Ok(report) => println!(
                    "{} -> {ext}: {:.0} MB/s, {} entries, {} skipped",
                    if recover { "repair" } else { "convert" },
                    mbps(total, t.elapsed().as_secs_f64()),
                    report.files,
                    report.skipped
                ),
                Err(e) => die("transform", format!("{ext} recover={recover}: {e}")),
            }
        }
    }
    let _ = fs::remove_dir_all(&root);
}
