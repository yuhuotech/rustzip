//! Streaming file hashes shared by the command line and graphical interfaces.

use rz_core::{CancelToken, Error, Progress, ProgressSink, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HashAlgorithm {
    Sha256,
    Blake3,
}

impl HashAlgorithm {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sha256 => "SHA-256",
            Self::Blake3 => "BLAKE3",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashResult {
    pub algorithm: HashAlgorithm,
    pub hex: String,
}

/// Computes selected hashes in one bounded-memory pass over a regular file.
pub fn hash_file(
    path: &Path,
    algorithms: &[HashAlgorithm],
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
) -> Result<Vec<HashResult>> {
    cancel.check()?;
    let algorithms: BTreeSet<_> = algorithms.iter().copied().collect();
    if algorithms.is_empty() {
        return Err(Error::Unsupported(
            "select at least one hash algorithm".into(),
        ));
    }
    if !std::fs::metadata(path)?.is_file() {
        return Err(Error::Unsupported("hashing requires a regular file".into()));
    }
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    let mut sha256 = algorithms
        .contains(&HashAlgorithm::Sha256)
        .then(Sha256::new);
    let mut blake3 = algorithms
        .contains(&HashAlgorithm::Blake3)
        .then(blake3::Hasher::new);
    let mut done = 0u64;
    let mut buffer = vec![0; 256 * 1024];
    loop {
        cancel.check()?;
        let count = file.read(&mut buffer).map_err(Error::Io)?;
        if count == 0 {
            break;
        }
        let chunk = &buffer[..count];
        if let Some(hasher) = &mut sha256 {
            hasher.update(chunk);
        }
        if let Some(hasher) = &mut blake3 {
            hasher.update(chunk);
        }
        done = done.saturating_add(count as u64);
        progress.update(Progress {
            files_done: 0,
            files_total: 1,
            bytes_done: done,
            bytes_total: total,
        });
    }
    cancel.check()?;
    progress.update(Progress {
        files_done: 1,
        files_total: 1,
        bytes_done: done,
        bytes_total: total,
    });
    let mut results = Vec::with_capacity(algorithms.len());
    if let Some(hasher) = sha256 {
        let digest = hasher.finalize();
        results.push(HashResult {
            algorithm: HashAlgorithm::Sha256,
            hex: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        });
    }
    if let Some(hasher) = blake3 {
        results.push(HashResult {
            algorithm: HashAlgorithm::Blake3,
            hex: hasher.finalize().to_hex().to_string(),
        });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_core::NoProgress;
    use std::fs;

    #[test]
    fn known_empty_file_hashes_match_standard_vectors() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let results = hash_file(
            tmp.path(),
            &[HashAlgorithm::Sha256, HashAlgorithm::Blake3],
            &CancelToken::new(),
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(results[0].algorithm, HashAlgorithm::Sha256);
        assert_eq!(
            results[0].hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(results[1].algorithm, HashAlgorithm::Blake3);
        assert_eq!(
            results[1].hex,
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn hash_reads_large_files_in_chunks_and_reports_the_final_byte_count() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        fs::write(tmp.path(), vec![7u8; 800_000]).unwrap();
        let mut updates = Vec::new();
        let mut progress = |p| updates.push(p);
        hash_file(
            tmp.path(),
            &[HashAlgorithm::Blake3],
            &CancelToken::new(),
            &mut progress,
        )
        .unwrap();
        assert!(updates.len() >= 4);
        assert_eq!(updates.last().unwrap().bytes_done, 800_000);
        assert_eq!(updates.last().unwrap().files_done, 1);
    }

    #[test]
    fn no_algorithm_and_cancel_are_reported() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        assert!(hash_file(tmp.path(), &[], &CancelToken::new(), &mut NoProgress).is_err());
        let token = CancelToken::new();
        token.cancel();
        assert!(matches!(
            hash_file(
                tmp.path(),
                &[HashAlgorithm::Sha256],
                &token,
                &mut NoProgress
            ),
            Err(Error::Cancelled)
        ));
    }

    #[test]
    fn directories_are_rejected_before_opening() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            hash_file(
                tmp.path(),
                &[HashAlgorithm::Sha256],
                &CancelToken::new(),
                &mut NoProgress
            ),
            Err(Error::Unsupported(_))
        ));
    }
}
