//! The "recent archives" page: what is known about each file, and how it is sorted.

use rz_app::browse::natural_cmp;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentItem {
    pub path: PathBuf,
    pub name: String,
    /// The folder the file is in.
    pub folder: String,
    /// Seconds since the Unix epoch; `None` when the system does not record it.
    pub created: Option<i64>,
    pub modified: Option<i64>,
    pub size: Option<u64>,
    /// The file is still where it was.
    pub exists: bool,
    /// A folder (only in the folder listing; recent archives are files).
    pub is_dir: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentSort {
    Name,
    Folder,
    Size,
    Created,
    Modified,
}

#[derive(Debug)]
pub struct RecentList {
    pub items: Vec<RecentItem>,
    pub sort: RecentSort,
    pub ascending: bool,
    pub selected: Option<PathBuf>,
    /// Folders before files, whatever the order (the folder listing).
    pub dirs_first: bool,
}

impl Default for RecentList {
    fn default() -> Self {
        // Newest first is what "recent" means.
        Self {
            items: Vec::new(),
            sort: RecentSort::Modified,
            ascending: false,
            selected: None,
            dirs_first: false,
        }
    }
}

impl RecentList {
    /// A list for browsing a folder: by name, folders first.
    pub fn for_folder() -> Self {
        Self {
            sort: RecentSort::Name,
            ascending: true,
            dirs_first: true,
            ..Self::default()
        }
    }

    pub fn set_items(&mut self, items: Vec<RecentItem>) {
        self.items = items;
        if let Some(sel) = &self.selected
            && !self.items.iter().any(|i| &i.path == sel)
        {
            self.selected = None;
        }
        self.resort();
    }

    /// A click on a column title: sort by it, or reverse the order if it already is.
    pub fn sort_by(&mut self, key: RecentSort) {
        if self.sort == key {
            self.ascending = !self.ascending;
        } else {
            self.sort = key;
            // Text reads best from A, dates and sizes from the largest.
            self.ascending = matches!(key, RecentSort::Name | RecentSort::Folder);
        }
        self.resort();
    }

    pub fn selected_item(&self) -> Option<&RecentItem> {
        let sel = self.selected.as_ref()?;
        self.items.iter().find(|i| &i.path == sel)
    }

    fn resort(&mut self) {
        let (key, asc, dirs_first) = (self.sort, self.ascending, self.dirs_first);
        self.items.sort_by(|a, b| {
            if dirs_first && a.is_dir != b.is_dir {
                return b.is_dir.cmp(&a.is_dir);
            }
            let ord = match key {
                RecentSort::Name => directed(natural_cmp(&a.name, &b.name), asc),
                RecentSort::Folder => directed(natural_cmp(&a.folder, &b.folder), asc),
                RecentSort::Size => cmp_known(a.size, b.size, asc),
                RecentSort::Created => cmp_known(a.created, b.created, asc),
                RecentSort::Modified => cmp_known(a.modified, b.modified, asc),
            };
            // Equal keys keep a stable, readable order.
            ord.then_with(|| natural_cmp(&a.name, &b.name))
        });
    }
}

fn directed(ord: Ordering, ascending: bool) -> Ordering {
    if ascending { ord } else { ord.reverse() }
}

/// Unknown values always come last, whichever way the list is sorted.
fn cmp_known<T: Ord>(a: Option<T>, b: Option<T>, ascending: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => directed(a.cmp(&b), ascending),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

pub(crate) fn unix(t: std::io::Result<SystemTime>) -> Option<i64> {
    let d = t.ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_secs()).ok()
}

/// Looks at each file on disk. Runs off the UI thread.
pub fn scan(paths: &[PathBuf]) -> Vec<RecentItem> {
    paths
        .iter()
        .map(|path| {
            let meta = std::fs::metadata(path).ok().filter(|m| m.is_file());
            RecentItem {
                path: path.clone(),
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                folder: path
                    .parent()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                created: meta.as_ref().and_then(|m| unix(m.created())),
                modified: meta.as_ref().and_then(|m| unix(m.modified())),
                size: meta.as_ref().map(|m| m.len()),
                exists: meta.is_some(),
                is_dir: false,
            }
        })
        .collect()
}

/// Lists a folder for the file browser. Runs off the UI thread. Hidden files are left
/// out, like a file manager does by default; unreadable entries are skipped.
pub fn list_dir(dir: &Path) -> std::io::Result<Vec<RecentItem>> {
    let mut items = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // `metadata` follows links, so a link to a folder behaves as the folder.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        items.push(RecentItem {
            folder: dir.to_string_lossy().into_owned(),
            created: unix(meta.created()),
            modified: unix(meta.modified()),
            size: (!meta.is_dir()).then_some(meta.len()),
            exists: true,
            is_dir: meta.is_dir(),
            path,
            name,
        });
    }
    Ok(items)
}

/// Whether a file name looks like one of the archives this program opens.
pub fn looks_like_archive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let standard = [
        ".zip", ".7z", ".rar", ".tar", ".tgz", ".txz", ".tzst", ".tbz2", ".tbz", ".gz", ".xz",
        ".zst", ".zstd", ".bz2", ".lz4", ".jar", ".apk", ".cbz",
    ]
    .iter()
    .any(|e| lower.ends_with(e));
    standard
        || lower.rfind(".z").is_some_and(|at| {
            lower[at + 2..].len() == 2 && lower[at + 2..].bytes().all(|b| b.is_ascii_digit())
        })
        || lower.rfind(".7z.").is_some_and(|at| {
            let suffix = &lower[at + 4..];
            suffix.len() == 3 && suffix.bytes().all(|b| b.is_ascii_digit())
        })
}

/// The folder name for "extract to a folder of the same name": the file name without its
/// archive extension, `data.tar.gz` giving `data`.
pub fn archive_stem(path: &Path) -> String {
    archive_stem_for_language(path, "zh-CN")
}

pub fn archive_stem_for_language(path: &Path, language: &str) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    if let Some(at) = lower.rfind(".7z.") {
        let suffix = &lower[at + 4..];
        if suffix.len() == 3 && suffix.bytes().all(|b| b.is_ascii_digit()) {
            return name[..at].to_string();
        }
    }
    if let Some(at) = lower.rfind(".z") {
        let suffix = &lower[at + 2..];
        if suffix.len() == 2 && suffix.bytes().all(|b| b.is_ascii_digit()) {
            return name[..at].to_string();
        }
    }
    for double in [
        ".tar.gz",
        ".tar.xz",
        ".tar.zst",
        ".tar.zstd",
        ".tar.bz2",
        ".tar.lz4",
    ] {
        if lower.ends_with(double) {
            return name[..name.len() - double.len()].to_string();
        }
    }
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => format!("{name}-{}", crate::text::tr(language, "extract_suffix")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn item(name: &str, folder: &str, modified: Option<i64>, size: Option<u64>) -> RecentItem {
        RecentItem {
            path: PathBuf::from(folder).join(name),
            name: name.into(),
            folder: folder.into(),
            created: modified.map(|m| m - 10),
            modified,
            size,
            exists: modified.is_some(),
            is_dir: false,
        }
    }

    #[test]
    fn a_folder_listing_puts_folders_first_and_skips_hidden_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("zeta")).unwrap();
        std::fs::write(tmp.path().join("alpha.zip"), b"x").unwrap();
        std::fs::write(tmp.path().join(".hidden"), b"x").unwrap();
        let mut l = RecentList::for_folder();
        l.set_items(list_dir(tmp.path()).unwrap());
        assert_eq!(names(&l), ["zeta", "alpha.zip"]);
        assert!(l.items[0].is_dir && l.items[0].size.is_none());
        // Folders stay first when the order is reversed.
        l.sort_by(RecentSort::Name);
        assert_eq!(names(&l), ["zeta", "alpha.zip"]);
        assert!(looks_like_archive("Backup.TAR.GZ") && !looks_like_archive("notes.txt"));
        assert!(list_dir(&tmp.path().join("missing")).is_err());
    }

    fn names(l: &RecentList) -> Vec<&str> {
        l.items.iter().map(|i| i.name.as_str()).collect()
    }

    #[test]
    fn newest_first_by_default_and_a_second_click_reverses() {
        let mut l = RecentList::default();
        l.set_items(vec![
            item("a.zip", "/x", Some(10), Some(5)),
            item("b.zip", "/y", Some(30), Some(1)),
            item("c.zip", "/x", None, None),
        ]);
        // Unknown dates go last whichever way the list runs.
        assert_eq!(names(&l), ["b.zip", "a.zip", "c.zip"]);
        l.sort_by(RecentSort::Modified);
        assert_eq!(names(&l), ["a.zip", "b.zip", "c.zip"]);
        l.sort_by(RecentSort::Name);
        assert_eq!(names(&l), ["a.zip", "b.zip", "c.zip"]);
        l.sort_by(RecentSort::Name);
        assert_eq!(names(&l), ["c.zip", "b.zip", "a.zip"]);
        l.sort_by(RecentSort::Folder);
        assert_eq!(names(&l), ["a.zip", "c.zip", "b.zip"]);
        l.sort_by(RecentSort::Size);
        assert_eq!(names(&l), ["a.zip", "b.zip", "c.zip"]);
    }

    #[test]
    fn a_selection_that_disappears_is_dropped() {
        let mut l = RecentList::default();
        l.set_items(vec![item("a.zip", "/x", Some(1), Some(1))]);
        l.selected = Some(PathBuf::from("/x/a.zip"));
        assert!(l.selected_item().is_some());
        l.set_items(vec![item("b.zip", "/x", Some(1), Some(1))]);
        assert!(l.selected.is_none());
    }

    #[test]
    fn scanning_reports_existing_and_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.zip");
        std::fs::write(&real, b"12345").unwrap();
        let items = scan(&[real.clone(), tmp.path().join("gone.zip")]);
        assert!(items[0].exists && items[0].size == Some(5) && items[0].modified.is_some());
        assert_eq!(items[0].name, "real.zip");
        assert!(!items[1].exists && items[1].size.is_none() && items[1].modified.is_none());
    }

    #[test]
    fn folder_names_drop_the_archive_extension() {
        assert_eq!(archive_stem(Path::new("/a/data.tar.gz")), "data");
        assert_eq!(archive_stem(Path::new("/a/项目文档.zip")), "项目文档");
        assert_eq!(archive_stem(Path::new("/a/v1.2.7z")), "v1.2");
        assert_eq!(archive_stem(Path::new("/a/photos.7z.001")), "photos");
        assert_eq!(archive_stem(Path::new("/a/backup.z01")), "backup");
        assert_eq!(archive_stem(Path::new("/a/noext")), "noext-解压");
        assert_eq!(
            archive_stem_for_language(Path::new("/a/noext"), "en-US"),
            "noext-extract"
        );
        assert!(looks_like_archive("photos.7z.001"));
        assert!(looks_like_archive("backup.z01"));
    }
}
