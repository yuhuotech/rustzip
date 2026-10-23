use rz_core::{Error, Result};
use std::path::{Component, Path, PathBuf};

/// Windows device names that must never be created as files.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Joins an archive path onto `root`, rejecting anything that could escape it:
/// `..`, absolute paths, drive letters, backslash tricks and Windows reserved names.
pub fn safe_join(root: &Path, entry_path: &str) -> Result<PathBuf> {
    let bad = || Error::UnsafePath(entry_path.to_string());
    let normalized = entry_path.replace('\\', "/");
    if normalized.is_empty() || normalized.contains('\0') {
        return Err(bad());
    }
    let mut out = root.to_path_buf();
    let mut depth = 0usize;
    for part in Path::new(&normalized).components() {
        match part {
            Component::Normal(name) => {
                let name = name.to_str().ok_or_else(bad)?;
                if name.contains(':') {
                    return Err(bad());
                }
                let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
                if RESERVED.contains(&stem.as_str()) {
                    return Err(bad());
                }
                out.push(name);
                depth += 1;
            }
            Component::CurDir => {}
            _ => return Err(bad()),
        }
    }
    if depth == 0 {
        return Err(bad());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/out")
    }

    #[test]
    fn accepts_normal_paths() {
        assert_eq!(
            safe_join(&root(), "a/b.txt").ok(),
            Some(PathBuf::from("/out/a/b.txt"))
        );
        assert_eq!(
            safe_join(&root(), "./a").ok(),
            Some(PathBuf::from("/out/a"))
        );
        assert!(safe_join(&root(), "文档/说明.md").is_ok());
    }

    #[test]
    fn rejects_escapes() {
        for p in [
            "../x",
            "a/../../x",
            "/etc/passwd",
            "C:/x",
            "..\\x",
            "a\\..\\..\\x",
            "",
            "NUL",
            "con.txt",
            "a/b:c",
        ] {
            assert!(safe_join(&root(), p).is_err(), "should reject {p:?}");
        }
    }
}
