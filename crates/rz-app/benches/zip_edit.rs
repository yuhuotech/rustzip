//! Compare raw-copy ZIP editing with the existing extract-and-repack path.

use rz_app::modify::{edit_archive, Edit};
use rz_core::{CancelToken, CreateOptions, EntryKind, NoProgress};
use rz_engine::{compress, ExtractOptions};
use rz_formats::OutputFormat;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Instant;

fn content(path: &Path) -> Result<BTreeMap<String, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut archive = rz_formats::open(path)?;
    let entries = archive.entries()?;
    let mut hashes = BTreeMap::new();
    let mut buf = vec![0; 256 * 1024];
    for entry in entries
        .into_iter()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        let mut reader = archive.open(&entry)?;
        let mut hash = Sha256::new();
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        hashes.insert(entry.path, hash.finalize().to_vec());
    }
    Ok(hashes)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let size_mb = std::env::var("RZ_BENCH_MB")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(32)
        .max(1);
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source");
    std::fs::create_dir(&source)?;
    let mut block = vec![0u8; 1 << 20];
    let mut state = 7u64;
    for byte in &mut block {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state >> 24) as u8;
    }
    for name in ["keep.bin", "delete.bin"] {
        let mut file = std::fs::File::create(source.join(name))?;
        for _ in 0..size_mb {
            file.write_all(&block)?;
        }
    }
    let original = dir.path().join("original.zip");
    compress(
        &[source],
        &original,
        OutputFormat::Zip,
        &CreateOptions::default(),
        &CancelToken::new(),
        &mut NoProgress,
    )?;
    let mut expected = None;
    for (label, encoding) in [("raw-copy", None), ("repack", Some("utf-8"))] {
        let archive = dir.path().join(format!("{label}.zip"));
        std::fs::copy(&original, &archive)?;
        let start = Instant::now();
        edit_archive(
            &archive,
            &Edit::Delete(vec!["source/delete.bin".into()]),
            &ExtractOptions::default(),
            6,
            encoding,
            &CancelToken::new(),
            &mut NoProgress,
        )?;
        let elapsed = start.elapsed().as_secs_f64();
        let hashes = content(&archive)?;
        if let Some(reference) = &expected {
            if reference != &hashes {
                return Err("edited archives contain different data".into());
            }
        } else {
            expected = Some(hashes);
        }
        println!("{label}: {elapsed:.3} s ({size_mb} MiB retained)");
    }
    Ok(())
}
