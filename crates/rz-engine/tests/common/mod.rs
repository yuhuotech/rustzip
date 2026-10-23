#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_core::{CancelToken, CreateOptions, NoProgress, Result};
use rz_engine::{compress, extract, CompressReport, ExtractOptions, ExtractReport};
use rz_formats::OutputFormat;
use std::fs;
use std::path::{Path, PathBuf};

/// Even second, so zip's two-second timestamps compare equal.
pub const MTIME: i64 = 1_700_000_000;

/// Builds `<root>/proj` with nested dirs, CJK names, an empty file, an empty dir
/// and (on Unix) an executable script.
pub fn make_tree(root: &Path) -> PathBuf {
    let p = root.join("proj");
    fs::create_dir_all(p.join("docs/深层")).unwrap();
    fs::create_dir_all(p.join("empty-dir")).unwrap();
    fs::write(p.join("readme.txt"), "hello rustzip").unwrap();
    fs::write(p.join("docs/说明.md"), "# 中文文件名\n").unwrap();
    fs::write(p.join("docs/深层/日本語.txt"), "テスト").unwrap();
    fs::write(p.join("empty.bin"), "").unwrap();
    fs::write(p.join("big.dat"), pseudo_random(300_000, 7)).unwrap();
    fs::write(p.join("run.sh"), "#!/bin/sh\necho ok\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    for f in walk_files(&p) {
        rz_platform::set_mtime(&f, MTIME).unwrap();
    }
    p
}

pub fn pseudo_random(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

pub fn pack(src: &Path, out: &Path, opts: &CreateOptions) -> Result<CompressReport> {
    let format = OutputFormat::from_path(out).unwrap();
    compress(
        &[src.to_path_buf()],
        out,
        format,
        opts,
        &CancelToken::new(),
        &mut NoProgress,
    )
}

pub fn unpack(archive: &Path, out: &Path, opts: &ExtractOptions) -> Result<ExtractReport> {
    let mut r = rz_formats::open(archive)?;
    extract(&mut *r, out, opts, &CancelToken::new(), &mut NoProgress)
}

pub fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn count_entries(root: &Path) -> usize {
    if !root.exists() {
        return 0;
    }
    walk_files(root).len()
}

pub fn assert_same_tree(expected: &Path, actual: &Path, label: &str) {
    let a = walk_files(expected);
    let b = walk_files(actual);
    let rel = |base: &Path, v: &[PathBuf]| -> Vec<PathBuf> {
        v.iter()
            .map(|p| p.strip_prefix(base).unwrap().to_path_buf())
            .collect()
    };
    assert_eq!(
        rel(expected, &a),
        rel(actual, &b),
        "{label}: file lists differ"
    );
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(fs::read(x).unwrap(), fs::read(y).unwrap(), "{label}: {x:?}");
        let mx = rz_platform::mtime_of(&fs::metadata(x).unwrap());
        let my = rz_platform::mtime_of(&fs::metadata(y).unwrap());
        assert_eq!(mx, my, "{label}: mtime of {x:?}");
    }
    if expected.join("empty-dir").is_dir() {
        assert!(actual.join("empty-dir").is_dir(), "{label}: empty dir lost");
    }
}
