//! The data behind the archive browser: a folder tree built from a flat entry
//! list, with sorting, search and display formatting. It knows nothing about any
//! UI toolkit, so every front end shows the same thing.

use rz_core::{clean_path, Entry, EntryKind};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Dir,
    File,
    Link,
}

/// What a file looks like in the list; each kind gets its own badge colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    Folder,
    Image,
    Document,
    Code,
    Archive,
    Media,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Last path component.
    pub name: String,
    /// Full cleaned path inside the archive; the identity of the row.
    pub path: String,
    pub kind: RowKind,
    /// File size; for folders, the total of everything inside.
    pub size: u64,
    pub packed: Option<u64>,
    pub modified: Option<i64>,
    pub encrypted: bool,
    /// For folders: number of files inside (recursively). Zero for files.
    pub items: usize,
    /// Index into the entry list, or `None` for folders the archive never lists itself.
    pub entry: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Kind,
}

#[derive(Debug, Clone)]
struct Node {
    kind: RowKind,
    entry: Option<usize>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Totals {
    size: u64,
    packed: u64,
    files: usize,
}

pub struct ArchiveTree {
    entries: Vec<Entry>,
    /// folder path -> child name -> node. The root folder is `""`.
    dirs: BTreeMap<String, BTreeMap<String, Node>>,
    /// folder path -> totals of everything below it.
    totals: HashMap<String, Totals>,
    all: Totals,
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn split_parent(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

impl ArchiveTree {
    pub fn build(entries: Vec<Entry>) -> Self {
        let mut dirs: BTreeMap<String, BTreeMap<String, Node>> = BTreeMap::new();
        dirs.entry(String::new()).or_default();
        for (i, e) in entries.iter().enumerate() {
            let path = clean_path(&e.path);
            if path.is_empty() {
                continue;
            }
            let kind = match e.kind {
                EntryKind::Dir => RowKind::Dir,
                EntryKind::File => RowKind::File,
                EntryKind::Symlink => RowKind::Link,
            };
            // Make sure every parent folder exists, even if the archive never lists it.
            let mut parent = String::new();
            let parts: Vec<&str> = path.split('/').collect();
            for part in &parts[..parts.len() - 1] {
                dirs.entry(parent.clone())
                    .or_default()
                    .entry((*part).to_string())
                    .or_insert(Node {
                        kind: RowKind::Dir,
                        entry: None,
                    });
                parent = join(&parent, part);
                dirs.entry(parent.clone()).or_default();
            }
            let (dir, name) = split_parent(&path);
            if kind == RowKind::Dir {
                dirs.entry(path.clone()).or_default();
            }
            dirs.entry(dir.to_string()).or_default().insert(
                name.to_string(),
                Node {
                    kind,
                    entry: Some(i),
                },
            );
        }

        let mut totals: HashMap<String, Totals> = HashMap::new();
        let mut all = Totals::default();
        for (i, e) in entries.iter().enumerate() {
            let _ = i;
            if e.kind != EntryKind::File {
                continue;
            }
            let path = clean_path(&e.path);
            all.size += e.size;
            all.packed += e.packed_size.unwrap_or(0);
            all.files += 1;
            let mut dir = split_parent(&path).0;
            loop {
                let t = totals.entry(dir.to_string()).or_default();
                t.size += e.size;
                t.packed += e.packed_size.unwrap_or(0);
                t.files += 1;
                if dir.is_empty() {
                    break;
                }
                dir = split_parent(dir).0;
            }
        }
        Self {
            entries,
            dirs,
            totals,
            all,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn total_files(&self) -> usize {
        self.all.files
    }

    pub fn total_size(&self) -> u64 {
        self.all.size
    }

    /// Packed size of all files, when the format reports it.
    pub fn total_packed(&self) -> Option<u64> {
        (self.all.packed > 0).then_some(self.all.packed)
    }

    pub fn is_dir(&self, path: &str) -> bool {
        self.dirs.contains_key(&clean_path(path))
    }

    fn row(&self, dir: &str, name: &str, node: &Node) -> Row {
        let path = join(dir, name);
        let e = node.entry.map(|i| &self.entries[i]);
        let (size, items) = match node.kind {
            RowKind::Dir => {
                let t = self.totals.get(&path).copied().unwrap_or_default();
                (t.size, t.files)
            }
            _ => (e.map_or(0, |e| e.size), 0),
        };
        Row {
            name: name.to_string(),
            path,
            kind: node.kind,
            size,
            packed: e.and_then(|e| e.packed_size),
            modified: e.and_then(|e| e.modified),
            encrypted: e.is_some_and(|e| e.encrypted),
            items,
            entry: node.entry,
        }
    }

    /// Contents of `dir`, or with a non-empty `filter`, every item below `dir`
    /// whose name contains it (case-insensitive). Folders come first.
    pub fn list(&self, dir: &str, sort: SortKey, ascending: bool, filter: &str) -> Vec<Row> {
        let dir = clean_path(dir);
        let filter = filter.trim().to_lowercase();
        let mut rows = Vec::new();
        if filter.is_empty() {
            if let Some(children) = self.dirs.get(&dir) {
                rows.extend(children.iter().map(|(n, node)| self.row(&dir, n, node)));
            }
        } else {
            self.collect_matches(&dir, &filter, &mut rows);
        }
        sort_rows(&mut rows, sort, ascending);
        rows
    }

    fn collect_matches(&self, dir: &str, filter: &str, out: &mut Vec<Row>) {
        let Some(children) = self.dirs.get(dir) else {
            return;
        };
        for (name, node) in children {
            if name.to_lowercase().contains(filter) {
                out.push(self.row(dir, name, node));
            }
            if node.kind == RowKind::Dir {
                self.collect_matches(&join(dir, name), filter, out);
            }
        }
    }

    pub fn find(&self, path: &str) -> Option<Row> {
        let path = clean_path(path);
        let (dir, name) = split_parent(&path);
        let node = self.dirs.get(dir)?.get(name)?;
        Some(self.row(dir, name, node))
    }

    /// `(label, path)` from the root down to `dir`; the root's label is empty.
    pub fn breadcrumbs(&self, dir: &str) -> Vec<(String, String)> {
        let dir = clean_path(dir);
        let mut out = vec![(String::new(), String::new())];
        let mut acc = String::new();
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            acc = join(&acc, part);
            out.push((part.to_string(), acc.clone()));
        }
        out
    }
}

pub fn parent_dir(dir: &str) -> String {
    split_parent(&clean_path(dir)).0.to_string()
}

fn kind_rank(r: &Row) -> u8 {
    match r.kind {
        RowKind::Dir => 0,
        _ => 1,
    }
}

fn sort_rows(rows: &mut [Row], key: SortKey, ascending: bool) {
    rows.sort_by(|a, b| {
        // Folders stay on top whichever way the list is sorted.
        let by_kind = kind_rank(a).cmp(&kind_rank(b));
        if by_kind != Ordering::Equal {
            return by_kind;
        }
        let ord = match key {
            SortKey::Name => natural_cmp(&a.name, &b.name),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Kind => extension(&a.name).cmp(&extension(&b.name)),
        }
        .then_with(|| natural_cmp(&a.name, &b.name));
        if ascending {
            ord
        } else {
            ord.reverse()
        }
    });
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

/// Case-insensitive comparison where digit runs compare as numbers:
/// `file2` sorts before `file10`.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let num = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (num(&mut ai), num(&mut bi));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let (lx, ly) = (x.to_lowercase().next(), y.to_lowercase().next());
                if lx != ly {
                    return lx.cmp(&ly);
                }
                ai.next();
                bi.next();
            }
        }
    }
}

pub fn badge(name: &str, kind: RowKind) -> Badge {
    if kind == RowKind::Dir {
        return Badge::Folder;
    }
    match extension(name).as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "heic" | "tiff" => {
            Badge::Image
        }
        "txt" | "md" | "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "rtf" | "csv"
        | "odt" => Badge::Document,
        "rs" | "js" | "ts" | "py" | "go" | "c" | "h" | "cpp" | "java" | "json" | "toml"
        | "yaml" | "yml" | "html" | "css" | "sh" | "xml" | "sql" => Badge::Code,
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "zst" | "bz2" | "tgz" => Badge::Archive,
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "mp4" | "mov" | "mkv" | "avi" | "webm" => {
            Badge::Media
        }
        _ => Badge::Other,
    }
}

/// Short label inside the badge: the extension, or `DIR`.
pub fn badge_label(name: &str, kind: RowKind) -> String {
    if kind == RowKind::Dir {
        return "DIR".into();
    }
    let ext = extension(name);
    if ext.is_empty() {
        "FILE".into()
    } else {
        ext.chars().take(4).collect::<String>().to_uppercase()
    }
}

/// `1.5 MB`, `812 KB`, `0 B`. Binary units, one decimal for values below 10.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// `2026-10-24 18:04`, from seconds since the Unix epoch (UTC).
pub fn format_time(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// Space saved as a percentage of the original, e.g. `66%`; `None` without data.
pub fn compression_ratio(unpacked: u64, packed: Option<u64>) -> Option<String> {
    let packed = packed?;
    (unpacked > 0 && packed > 0).then(|| {
        let saved = 100.0 * (1.0 - packed as f64 / unpacked as f64);
        format!("{:.0}%", saved.max(0.0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64, kind: EntryKind) -> Entry {
        Entry {
            index: 0,
            path: path.into(),
            kind,
            size,
            packed_size: Some(size / 2),
            modified: Some(1_700_000_000),
            encrypted: false,
            unix_mode: None,
        }
    }

    fn file(path: &str, size: u64) -> Entry {
        entry(path, size, EntryKind::File)
    }

    fn tree() -> ArchiveTree {
        ArchiveTree::build(vec![
            entry("proj/", 0, EntryKind::Dir),
            file("proj/readme.txt", 100),
            file("proj/src/main.rs", 2000),
            file("proj/src/lib.rs", 500),
            file("proj/docs/说明.md", 300),
            file("top.bin", 9000),
        ])
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn root_lists_top_level_items_folders_first() {
        let t = tree();
        let rows = t.list("", SortKey::Name, true, "");
        assert_eq!(names(&rows), ["proj", "top.bin"]);
        assert_eq!(rows[0].kind, RowKind::Dir);
    }

    #[test]
    fn folders_the_archive_never_lists_are_created_from_file_paths() {
        let t = tree();
        let rows = t.list("proj", SortKey::Name, true, "");
        assert_eq!(names(&rows), ["docs", "src", "readme.txt"]);
        // "docs" and "src" have no entry of their own.
        assert_eq!(rows[0].entry, None);
        assert_eq!(rows[2].entry, Some(1));
        assert!(t.is_dir("proj/src") && t.is_dir("proj/docs/") && !t.is_dir("proj/readme.txt"));
    }

    #[test]
    fn folder_rows_carry_recursive_size_and_file_count() {
        let t = tree();
        let proj = t.find("proj").unwrap_or_else(|| unreachable!());
        assert_eq!((proj.size, proj.items), (100 + 2000 + 500 + 300, 4));
        let src = t.find("proj/src").unwrap_or_else(|| unreachable!());
        assert_eq!((src.size, src.items), (2500, 2));
        assert_eq!(
            (t.total_files(), t.total_size()),
            (5, 100 + 2000 + 500 + 300 + 9000)
        );
        // Each test entry packs to half its size: 50 + 1000 + 250 + 150 + 4500.
        assert_eq!(t.total_packed(), Some(5950));
    }

    #[test]
    fn sorting_keeps_folders_on_top_in_both_directions() {
        let t = tree();
        let asc = t.list("proj", SortKey::Size, true, "");
        assert_eq!(names(&asc), ["docs", "src", "readme.txt"]);
        let desc = t.list("proj/src", SortKey::Size, false, "");
        assert_eq!(names(&desc), ["main.rs", "lib.rs"]);
        let desc = t.list("", SortKey::Size, false, "");
        assert_eq!(names(&desc), ["proj", "top.bin"]);
    }

    #[test]
    fn sort_by_kind_groups_extensions() {
        let t = ArchiveTree::build(vec![
            file("b.txt", 1),
            file("a.rs", 1),
            file("c.txt", 1),
            file("d.RS", 1),
        ]);
        let rows = t.list("", SortKey::Kind, true, "");
        assert_eq!(names(&rows), ["a.rs", "d.RS", "b.txt", "c.txt"]);
    }

    #[test]
    fn natural_order_compares_numbers_as_numbers() {
        let mut v = vec!["file10", "file2", "File1", "file01", "file100", "a", "B"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            ["a", "B", "File1", "file01", "file2", "file10", "file100"]
        );
        assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("a1", "a1b"), Ordering::Less);
        assert_eq!(
            natural_cmp("99999999999999999999", "100000000000000000000"),
            Ordering::Less
        );
    }

    #[test]
    fn search_looks_through_subfolders_and_ignores_case() {
        let t = tree();
        let rows = t.list("", SortKey::Name, true, "  MAIN ");
        assert_eq!(names(&rows), ["main.rs"]);
        assert_eq!(rows[0].path, "proj/src/main.rs");
        let rows = t.list("", SortKey::Name, true, "说明");
        assert_eq!(names(&rows), ["说明.md"]);
        // Search is limited to the current folder and below.
        assert!(t.list("proj/src", SortKey::Name, true, "readme").is_empty());
        assert!(t.list("", SortKey::Name, true, "zzz").is_empty());
        // A matching folder is returned together with matching files inside it.
        let rows = t.list("", SortKey::Name, true, "src");
        assert_eq!(names(&rows), ["src"]);
    }

    #[test]
    fn find_and_breadcrumbs_use_cleaned_paths() {
        let t = tree();
        assert_eq!(t.find("./proj//readme.txt").map(|r| r.size), Some(100));
        assert!(t.find("nope").is_none());
        assert_eq!(
            t.breadcrumbs("proj/src/"),
            [
                (String::new(), String::new()),
                ("proj".into(), "proj".into()),
                ("src".into(), "proj/src".into())
            ]
        );
        assert_eq!(t.breadcrumbs("").len(), 1);
        assert_eq!(parent_dir("proj/src"), "proj");
        assert_eq!(parent_dir("proj"), "");
        assert_eq!(parent_dir(""), "");
    }

    #[test]
    fn odd_paths_do_not_break_the_tree() {
        let t = ArchiveTree::build(vec![
            file("", 1),
            file("./", 1),
            file("a\\b\\c.txt", 5),
            file("/abs/x", 3),
            file("dup", 1),
            file("dup", 2),
        ]);
        assert_eq!(names(&t.list("a/b", SortKey::Name, true, "")), ["c.txt"]);
        assert_eq!(names(&t.list("abs", SortKey::Name, true, "")), ["x"]);
        // The later duplicate wins.
        assert_eq!(t.find("dup").map(|r| r.size), Some(2));
        assert!(ArchiveTree::build(vec![])
            .list("", SortKey::Name, true, "")
            .is_empty());
    }

    #[test]
    fn a_file_and_folder_with_clashing_names_stay_apart_by_path() {
        let t = ArchiveTree::build(vec![file("a", 1), file("a/b", 2)]);
        // The folder created for "a/b" replaces the file node named "a"; nothing panics.
        let rows = t.list("", SortKey::Name, true, "");
        assert_eq!(rows.len(), 1);
        assert_eq!(t.find("a/b").map(|r| r.size), Some(2));
    }

    #[test]
    fn badges_follow_the_file_type() {
        assert_eq!(badge("x.PNG", RowKind::File), Badge::Image);
        assert_eq!(badge("说明.md", RowKind::File), Badge::Document);
        assert_eq!(badge("main.rs", RowKind::File), Badge::Code);
        assert_eq!(badge("a.tar", RowKind::File), Badge::Archive);
        assert_eq!(badge("song.flac", RowKind::File), Badge::Media);
        assert_eq!(badge("noext", RowKind::File), Badge::Other);
        assert_eq!(badge("anything.png", RowKind::Dir), Badge::Folder);
        assert_eq!(badge_label("main.rs", RowKind::File), "RS");
        assert_eq!(badge_label("archive.verylong", RowKind::File), "VERY");
        assert_eq!(badge_label("noext", RowKind::File), "FILE");
        assert_eq!(badge_label("x", RowKind::Dir), "DIR");
    }

    #[test]
    fn sizes_use_binary_units_with_sensible_precision() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(812 * 1024), "812 KB");
        assert_eq!(format_size(2_516_582), "2.4 MB");
        assert_eq!(format_size(3 << 30), "3.0 GB");
        assert_eq!(format_size(u64::MAX), "16777216 TB");
    }

    #[test]
    fn times_format_in_utc_and_handle_edges() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(format_time(-1), "1969-12-31 23:59");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00");
    }

    #[test]
    fn ratio_is_space_saved() {
        assert_eq!(compression_ratio(1000, Some(340)).as_deref(), Some("66%"));
        assert_eq!(compression_ratio(1000, Some(1200)).as_deref(), Some("0%"));
        assert_eq!(compression_ratio(0, Some(10)), None);
        assert_eq!(compression_ratio(10, None), None);
        assert_eq!(compression_ratio(10, Some(0)), None);
    }
}
