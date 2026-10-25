//! Names that never collide: `report.txt` becomes `report (1).txt`, `photos` becomes `photos (1)`.

use std::path::{Path, PathBuf};

/// Where the number goes: before the extension of a file, at the end of a folder name.
/// `.tar.gz` and friends count as one extension.
fn split_name(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    // A leading dot is part of the name (`.bashrc`), not an extension.
    let Some(dot) = name.rfind('.').filter(|d| *d > 0) else {
        return (name, "");
    };
    let stem = &name[..dot];
    let dot = match stem.to_ascii_lowercase().strip_suffix(".tar") {
        Some(_) => stem.len() - 4,
        None => dot,
    };
    (&name[..dot], &name[dot..])
}

pub fn numbered_name(name: &str, n: u32, is_dir: bool) -> String {
    let (stem, ext) = split_name(name, is_dir);
    format!("{stem} ({n}){ext}")
}

/// The first of `name`, `name (1)`, `name (2)`... that `taken` does not claim.
pub fn first_free(name: &str, is_dir: bool, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    (1..)
        .map(|n| numbered_name(name, n, is_dir))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_string())
}

/// `path` itself when nothing is there, otherwise the same name with a number.
pub fn unused_path(path: &Path) -> PathBuf {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return path.to_path_buf();
    };
    let is_dir = path.is_dir();
    let free = first_free(&name, is_dir, |candidate| {
        path.with_file_name(candidate).symlink_metadata().is_ok()
    });
    path.with_file_name(free)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_number_goes_before_the_extension_of_a_file() {
        assert_eq!(numbered_name("report.txt", 1, false), "report (1).txt");
        assert_eq!(numbered_name("data.tar.gz", 2, false), "data (2).tar.gz");
        assert_eq!(numbered_name("Makefile", 1, false), "Makefile (1)");
        assert_eq!(numbered_name(".bashrc", 1, false), ".bashrc (1)");
        assert_eq!(numbered_name("v1.2", 1, true), "v1.2 (1)");
        assert_eq!(numbered_name("大型.zip", 3, false), "大型 (3).zip");
    }

    #[test]
    fn the_first_unclaimed_number_is_used() {
        let taken = ["a.zip", "a (1).zip", "a (2).zip"];
        let free = first_free("a.zip", false, |n| taken.contains(&n));
        assert_eq!(free, "a (3).zip");
        assert_eq!(first_free("b.zip", false, |n| taken.contains(&n)), "b.zip");
    }

    #[test]
    fn an_existing_file_on_disk_pushes_the_name_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.zip");
        assert_eq!(unused_path(&path), path);
        std::fs::write(&path, "1").unwrap();
        assert_eq!(unused_path(&path), dir.path().join("x (1).zip"));
        std::fs::write(dir.path().join("x (1).zip"), "1").unwrap();
        assert_eq!(unused_path(&path), dir.path().join("x (2).zip"));
    }
}
