//! Text resources for the sidebar tools; existing screens migrate separately.

/// Resolves the configured language; `auto` keeps the application's established Chinese default.
pub fn tr(language: &str, key: &str) -> &'static str {
    let english = language.to_ascii_lowercase().starts_with("en");
    include_str!("../resources/tools.tsv")
        .lines()
        .find_map(|line| {
            let mut columns = line.split('\t');
            let id = columns.next()?;
            let zh = columns.next()?;
            let en = columns.next()?;
            (id == key).then_some(if english { en } else { zh })
        })
        .unwrap_or("?")
}

/// Archive types people know by name but that no backend reads yet.
const KNOWN_ARCHIVE_EXTENSIONS: [&str; 12] = [
    "dmg", "wim", "arj", "lzh", "rpm", "xar", "z", "img", "chm", "nsis", "sit", "lha",
];

/// Turns a failure to open `path` into a sentence a non-technical user can act on.
pub fn open_error(language: &str, path: &std::path::Path, error: &rz_core::Error) -> &'static str {
    use rz_core::Error;
    use std::io::ErrorKind;
    let known_type = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| KNOWN_ARCHIVE_EXTENSIONS.contains(&e.as_str()));
    let key = match error {
        // A folder fails with a raw OS error on some platforms and not at all on others.
        _ if path.is_dir() => "open_not_archive",
        Error::UnknownFormat if known_type => "open_unsupported",
        Error::UnknownFormat => "open_not_archive",
        Error::Unsupported(_) => "open_unsupported",
        Error::Corrupt(_) => "open_corrupt",
        Error::LimitExceeded(_) => "open_limit",
        Error::Io(e) => match e.kind() {
            ErrorKind::NotFound => "open_missing",
            ErrorKind::PermissionDenied => "open_denied",
            _ => "open_failed",
        },
        _ => "open_failed",
    };
    tr(language, key)
}

/// Keeps a filesystem conflict actionable without exposing the host destination path.
pub fn task_error(language: &str, error: &rz_app::tasks::TaskError) -> String {
    if error.code == rz_app::tasks::ErrorCode::AlreadyExists {
        return tr(language, "destination_exists").into();
    }
    if error.code == rz_app::tasks::ErrorCode::LimitExceeded {
        return tr(language, "task_resource_limit").into();
    }
    if let Some((first, second)) = &error.conflicting_paths {
        tr(language, "case_conflict")
            .replace("{first}", first)
            .replace("{second}", second)
    } else {
        error.message.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rz_core::Error;
    use std::path::Path;

    #[test]
    fn open_failures_are_explained_in_plain_words() {
        let dir = std::env::temp_dir();
        let io = Error::Io(std::io::Error::from_raw_os_error(21));
        assert_eq!(open_error("zh", &dir, &io), tr("zh", "open_not_archive"));
        assert_eq!(
            open_error("zh", Path::new("notes.txt"), &Error::UnknownFormat),
            tr("zh", "open_not_archive")
        );
        assert_eq!(
            open_error("zh", Path::new("disk.WIM"), &Error::UnknownFormat),
            tr("zh", "open_unsupported")
        );
        assert_eq!(
            open_error("en", Path::new("a.zip"), &Error::Corrupt("x".into())),
            tr("en", "open_corrupt")
        );
    }
}
