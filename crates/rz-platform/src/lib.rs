//! Platform differences live here so the core crates stay platform-independent.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

/// Applies the permission bits of an archive entry.
///
/// Only the lower nine bits are applied: setuid, setgid and sticky bits from an
/// untrusted archive are never restored. A no-op on platforms without Unix modes.
pub fn apply_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// Unix permission bits of a file, when the platform has them.
pub fn mode_of(meta: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// Sets the modification time from seconds since the Unix epoch.
pub fn set_mtime(path: &Path, unix_secs: i64) -> io::Result<()> {
    let time = if unix_secs >= 0 {
        UNIX_EPOCH + Duration::from_secs(unix_secs as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(unix_secs.unsigned_abs())
    };
    let file: File = OpenOptions::new().write(true).open(path)?;
    file.set_modified(time)
}

/// Modification time of a file as seconds since the Unix epoch.
pub fn mtime_of(meta: &std::fs::Metadata) -> Option<i64> {
    let t = meta.modified().ok()?;
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

/// True when `a` and `b` are the same existing file.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mtime_round_trips() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let p = dir.path().join("f");
        let _ = std::fs::write(&p, b"x");
        assert!(set_mtime(&p, 1_700_000_000).is_ok());
        let meta = std::fs::metadata(&p).unwrap_or_else(|_| unreachable!());
        assert_eq!(mtime_of(&meta), Some(1_700_000_000));
    }

    #[cfg(unix)]
    #[test]
    fn special_mode_bits_are_never_restored() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let p = dir.path().join("f");
        let _ = std::fs::write(&p, b"x");
        assert!(apply_mode(&p, 0o4755).is_ok());
        let mode = std::fs::metadata(&p)
            .map(|m| m.permissions().mode())
            .unwrap_or(0);
        assert_eq!(mode & 0o7777, 0o755);
    }

    #[test]
    fn same_file_detects_identity() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let p = dir.path().join("f");
        let _ = std::fs::write(&p, b"x");
        assert!(same_file(&p, &dir.path().join(".").join("f")));
        assert!(!same_file(&p, &dir.path().join("missing")));
    }
}
