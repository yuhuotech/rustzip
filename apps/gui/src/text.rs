//! Text resources for desktop surfaces migrated to the shared catalog.

use std::sync::OnceLock;

fn prefers_english(language: &str, system_locale: Option<&str>) -> bool {
    if language.eq_ignore_ascii_case("auto") {
        system_locale
            .map(|locale| locale.to_ascii_lowercase().starts_with("en"))
            .unwrap_or(false)
    } else {
        language.to_ascii_lowercase().starts_with("en")
    }
}

fn system_locale() -> Option<&'static str> {
    static LOCALE: OnceLock<Option<String>> = OnceLock::new();
    LOCALE
        .get_or_init(|| {
            ["LC_ALL", "LC_MESSAGES", "LANG"]
                .into_iter()
                .find_map(|name| {
                    std::env::var(name)
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                })
        })
        .as_deref()
}

/// Resolves the configured language; automatic mode follows the process locale when available.
pub fn tr(language: &str, key: &str) -> &'static str {
    let english = prefers_english(language, system_locale());
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

    #[test]
    fn explicit_language_overrides_system_locale_and_auto_uses_it() {
        assert!(prefers_english("en-GB", Some("zh_CN.UTF-8")));
        assert!(!prefers_english("zh-CN", Some("en_US.UTF-8")));
        assert!(prefers_english("auto", Some("en_US.UTF-8")));
        assert!(!prefers_english("auto", Some("zh_CN.UTF-8")));
        assert!(!prefers_english("auto", None));
    }

    #[test]
    fn resource_keys_are_unique_and_have_both_languages() {
        let mut keys = std::collections::HashSet::new();
        for line in include_str!("../resources/tools.tsv").lines() {
            let columns: Vec<_> = line.split('\t').collect();
            assert_eq!(columns.len(), 3);
            assert!(!columns[0].is_empty());
            assert!(keys.insert(columns[0]));
            assert!(!columns[1].is_empty());
            assert!(!columns[2].is_empty());
        }
    }
}
