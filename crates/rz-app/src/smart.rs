//! Smart extraction: never scatter loose files into the destination.

use rz_core::Entry;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Suffixes stripped from an archive name to get the folder name.
const SUFFIXES: [&str; 15] = [
    ".tar.gz",
    ".tar.xz",
    ".tar.zst",
    ".tar.zstd",
    ".tar.bz2",
    ".tar.lz4",
    ".tgz",
    ".txz",
    ".tzst",
    ".tbz2",
    ".tbz",
    ".zip",
    ".7z",
    ".rar",
    ".tar",
];

/// `Photos.2024.zip` -> `Photos.2024`; `data.tar.gz` -> `data`.
pub fn archive_stem(archive: &Path) -> String {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    let stem = SUFFIXES
        .iter()
        .chain([".gz", ".xz", ".zst", ".bz2", ".lz4"].iter())
        .find(|s| lower.ends_with(*s) && name.len() > s.len())
        .map_or(name.as_str(), |s| &name[..name.len() - s.len()]);
    let stem = stem.trim().trim_matches('.');
    if stem.is_empty() {
        "extracted".into()
    } else {
        stem.to_string()
    }
}

/// Names that cannot be used as a folder on some system.
fn folder_safe(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    cleaned.trim_end_matches([' ', '.']).to_string()
}

/// Number of distinct first path components in the archive.
pub fn top_level_count(entries: &[Entry]) -> usize {
    entries
        .iter()
        .filter_map(|e| {
            e.path
                .replace('\\', "/")
                .split('/')
                .find(|p| !p.is_empty() && *p != ".")
                .map(str::to_string)
        })
        .collect::<BTreeSet<_>>()
        .len()
}

/// Decides where to extract: straight into `dest` when the archive has at most
/// one top-level item (it already has its own wrapper), otherwise into a new
/// folder named after the archive. An existing folder is never reused, so
/// `photos (2)` is picked instead of mixing two archives together.
pub fn extraction_dir(entries: &[Entry], archive: &Path, dest: &Path) -> PathBuf {
    if top_level_count(entries) <= 1 {
        return dest.to_path_buf();
    }
    let base = folder_safe(&archive_stem(archive));
    let base = if base.is_empty() {
        "extracted".into()
    } else {
        base
    };
    let first = dest.join(&base);
    if !first.exists() {
        return first;
    }
    (2..10_000)
        .map(|n| dest.join(format!("{base} ({n})")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_core::EntryKind;

    fn entry(path: &str) -> Entry {
        Entry {
            index: 0,
            path: path.into(),
            kind: if path.ends_with('/') {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
            size: 0,
            packed_size: None,
            modified: None,
            encrypted: false,
            unix_mode: None,
        }
    }

    fn entries(paths: &[&str]) -> Vec<Entry> {
        paths.iter().map(|p| entry(p)).collect()
    }

    #[test]
    fn stems_strip_known_suffixes() {
        let s = |n: &str| archive_stem(Path::new(n));
        assert_eq!(s("photos.zip"), "photos");
        assert_eq!(s("data.tar.gz"), "data");
        assert_eq!(s("Data.TAR.GZ"), "Data");
        assert_eq!(s("a.b.c.7z"), "a.b.c");
        assert_eq!(s("notes.txt.gz"), "notes.txt");
        assert_eq!(s("项目备份.zip"), "项目备份");
        assert_eq!(s(".zip"), ".zip".trim_matches('.'));
        assert_eq!(s("noext"), "noext");
    }

    #[test]
    fn counts_distinct_top_level_items() {
        assert_eq!(top_level_count(&entries(&[])), 0);
        assert_eq!(
            top_level_count(&entries(&["a/", "a/b.txt", "a/c/d.txt"])),
            1
        );
        assert_eq!(top_level_count(&entries(&["a/x", "b/y"])), 2);
        assert_eq!(top_level_count(&entries(&["./a/x", "a/y", "/a/z"])), 1);
        assert_eq!(top_level_count(&entries(&["a\\x", "b\\y"])), 2);
    }

    #[test]
    fn one_wrapper_folder_extracts_in_place() {
        let dest = Path::new("/tmp/out");
        let e = entries(&["proj/", "proj/a.txt", "proj/src/b.rs"]);
        assert_eq!(extraction_dir(&e, Path::new("/x/proj.zip"), dest), dest);
    }

    #[test]
    fn a_single_loose_file_or_empty_archive_extracts_in_place() {
        let dest = Path::new("/tmp/out");
        assert_eq!(
            extraction_dir(&entries(&["notes.txt"]), Path::new("notes.txt.gz"), dest),
            dest
        );
        assert_eq!(extraction_dir(&[], Path::new("empty.zip"), dest), dest);
    }

    #[test]
    fn loose_files_get_a_folder_named_after_the_archive() {
        let tmp = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let e = entries(&["a.txt", "b.txt", "sub/c.txt"]);
        assert_eq!(
            extraction_dir(&e, Path::new("/x/My Photos.zip"), tmp.path()),
            tmp.path().join("My Photos")
        );
    }

    #[test]
    fn existing_folders_are_never_reused() {
        let tmp = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let e = entries(&["a.txt", "b.txt"]);
        let _ = std::fs::create_dir(tmp.path().join("pack"));
        assert_eq!(
            extraction_dir(&e, Path::new("pack.zip"), tmp.path()),
            tmp.path().join("pack (2)")
        );
        let _ = std::fs::create_dir(tmp.path().join("pack (2)"));
        assert_eq!(
            extraction_dir(&e, Path::new("pack.zip"), tmp.path()),
            tmp.path().join("pack (3)")
        );
    }

    #[test]
    fn archive_names_cannot_escape_or_break_the_folder_name() {
        let tmp = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let e = entries(&["a", "b"]);
        let got = extraction_dir(&e, Path::new("we:ird*na?me.zip"), tmp.path());
        assert_eq!(got, tmp.path().join("we_ird_na_me"));
        let got = extraction_dir(&e, Path::new("trailing. .zip"), tmp.path());
        assert!(got.starts_with(tmp.path()));
        assert!(!got.to_string_lossy().ends_with(' '));
    }
}
