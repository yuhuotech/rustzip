//! What the program was started to do: the command line, and on macOS the `file://`
//! addresses Finder hands over for "Open With" and double clicks.

use std::ffi::OsString;
use std::path::PathBuf;
use url::Url;

const MAX_ACTION_PATHS: usize = 4096;
const MAX_SYSTEM_URL_BYTES: usize = 1_048_576;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Launch {
    /// Archives to browse.
    pub open: Vec<PathBuf>,
    /// Archives to extract next to themselves, without opening a window of choices
    /// (the file manager's "Extract here").
    pub extract_here: Vec<PathBuf>,
    /// Files and folders to put into a new archive (the file manager's "Compress").
    pub compress: Vec<PathBuf>,
}

impl Launch {
    pub fn is_empty(&self) -> bool {
        self.open.is_empty() && self.extract_here.is_empty() && self.compress.is_empty()
    }

    pub fn extend(&mut self, other: Self) {
        self.open.extend(other.open);
        self.extract_here.extend(other.extract_here);
        self.compress.extend(other.compress);
    }
}

/// `rustzip [paths]`, `rustzip --extract-here [archives]`, `rustzip --compress [paths]`.
/// Paths that do not exist are dropped: a stale shortcut should not open an error.
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Launch {
    #[derive(Clone, Copy)]
    enum Mode {
        Open,
        Extract,
        Compress,
    }
    let mut launch = Launch::default();
    let mut mode = Mode::Open;
    for arg in args {
        match arg.to_str() {
            Some("--extract-here") => mode = Mode::Extract,
            Some("--compress") => mode = Mode::Compress,
            _ => {
                let path = PathBuf::from(arg);
                match mode {
                    Mode::Open if path.is_file() => launch.open.push(path),
                    Mode::Extract if path.is_file() => launch.extract_here.push(path),
                    Mode::Compress if path.exists() => launch.compress.push(path),
                    _ => {}
                }
            }
        }
    }
    launch
}

/// `file:///Users/me/a%20b.zip` -> `/Users/me/a b.zip`. Anything that is not a plain
/// local `file` URL gives `None`.
pub fn file_url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // Only the local machine: an empty host or `localhost`.
    let path = match rest.strip_prefix("localhost") {
        Some("") => "/",
        Some(p) if p.starts_with('/') => p,
        Some(_) => return None,
        None if rest.starts_with('/') => rest,
        None => return None,
    };
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = path.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(out).ok()?;
    // `/C:/dir/file` is how Windows paths appear in file URLs.
    let text = match text.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => text[1..].to_string(),
        _ => text,
    };
    Some(PathBuf::from(text))
}

/// Parses a local file-open URL or a Finder action dispatched through the app URL scheme.
pub fn parse_system_url(raw: &str) -> Option<Launch> {
    if raw.len() > MAX_SYSTEM_URL_BYTES {
        return None;
    }
    if let Some(path) = file_url_to_path(raw) {
        return Some(Launch {
            open: vec![path],
            ..Default::default()
        });
    }
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "rustzip"
        || url.username() != ""
        || url.password().is_some()
        || url.port().is_some()
        || url.path() != ""
        || url.fragment().is_some()
    {
        return None;
    }
    let action = url.host_str()?;
    if !matches!(action, "compress" | "extract-here") {
        return None;
    }
    let mut paths = Vec::new();
    for (key, value) in url.query_pairs() {
        if key != "path" || paths.len() >= MAX_ACTION_PATHS {
            return None;
        }
        let path = file_url_to_path(&value)?;
        let usable = match action {
            "compress" => path.exists(),
            "extract-here" => path.is_file(),
            _ => false,
        };
        if !usable {
            return None;
        }
        paths.push(path);
    }
    if paths.is_empty() {
        return None;
    }
    Some(match action {
        "compress" => Launch {
            compress: paths,
            ..Default::default()
        },
        "extract-here" => Launch {
            extract_here: paths,
            ..Default::default()
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn urls_become_paths() {
        assert_eq!(
            file_url_to_path("file:///Users/me/a%20b%E4%B8%AD.zip"),
            Some(PathBuf::from("/Users/me/a b\u{4e2d}.zip"))
        );
        assert_eq!(
            file_url_to_path("file://localhost/tmp/x.7z"),
            Some(PathBuf::from("/tmp/x.7z"))
        );
        assert_eq!(
            file_url_to_path("file:///C:/Users/me/x.zip"),
            Some(PathBuf::from("C:/Users/me/x.zip"))
        );
        // Not local files, or broken escapes.
        assert_eq!(file_url_to_path("https://example.com/a.zip"), None);
        assert_eq!(file_url_to_path("file://server/share/a.zip"), None);
        assert_eq!(file_url_to_path("file://localhost.evil/tmp/a.zip"), None);
        assert_eq!(file_url_to_path("file:///a%2"), None);
        assert_eq!(file_url_to_path("file:///a%ZZ"), None);
        assert_eq!(file_url_to_path("file:///a%FF.zip"), None);
    }

    #[test]
    fn arguments_pick_the_mode_and_drop_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.zip");
        let b = dir.path().join("b.zip");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        let args = |v: Vec<OsString>| parse_args(v);
        let s = |p: &std::path::Path| p.as_os_str().to_owned();

        let l = args(vec![
            s(&a),
            "--extract-here".into(),
            s(&b),
            "/no/such".into(),
        ]);
        assert_eq!(l.open, vec![a.clone()]);
        assert_eq!(l.extract_here, vec![b.clone()]);
        assert!(l.compress.is_empty());

        // Folders are fine to compress, not to open.
        let l = args(vec!["--compress".into(), s(dir.path()), s(&a)]);
        assert_eq!(l.compress, vec![dir.path().to_path_buf(), a]);
        assert!(l.open.is_empty() && !l.is_empty());
        assert!(args(vec![]).is_empty());
    }

    #[test]
    fn finder_action_urls_preserve_selected_paths_and_mode() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("归档 & 文件.zip");
        let folder = dir.path().join("要压缩的文件夹");
        std::fs::write(&archive, b"zip").unwrap();
        std::fs::create_dir(&folder).unwrap();

        let action_url = |action: &str, paths: &[PathBuf]| {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            for path in paths {
                let file_url = Url::from_file_path(path).unwrap();
                query.append_pair("path", file_url.as_str());
            }
            format!("rustzip://{action}?{}", query.finish())
        };

        let open =
            parse_system_url(&action_url("compress", &[archive.clone(), folder.clone()])).unwrap();
        assert_eq!(open.compress, vec![archive, folder]);
        assert!(open.open.is_empty() && open.extract_here.is_empty());

        let extract = parse_system_url(&action_url(
            "extract-here",
            &[dir.path().join("归档 & 文件.zip")],
        ))
        .unwrap();
        assert_eq!(extract.extract_here.len(), 1);
        assert!(extract.compress.is_empty() && extract.open.is_empty());
    }

    #[test]
    fn finder_action_urls_reject_remote_and_unknown_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        std::fs::write(&archive, b"zip").unwrap();
        let file_url = Url::from_file_path(&archive).unwrap();
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("path", file_url.as_str())
            .finish();

        assert!(parse_system_url(&format!("rustzip://delete?{query}")).is_none());
        assert!(parse_system_url(&format!("rustzip://compress@remote?{query}")).is_none());
        assert!(
            parse_system_url("rustzip://compress?path=file%3A%2F%2Fserver%2Fshare%2Fa.zip")
                .is_none()
        );
        assert!(parse_system_url("rustzip://compress?path=file%3A%2F%2F%2Fmissing.zip").is_none());
    }
}
