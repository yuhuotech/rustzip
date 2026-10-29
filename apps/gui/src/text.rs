//! Text resources for desktop surfaces migrated to the shared catalog.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LanguageChoice {
    pub tag: &'static str,
    pub short: &'static str,
    pub native_name: &'static str,
}

pub const LANGUAGE_CHOICES: [LanguageChoice; 10] = [
    LanguageChoice {
        tag: "zh-CN",
        short: "中",
        native_name: "简体中文",
    },
    LanguageChoice {
        tag: "en-US",
        short: "EN",
        native_name: "English",
    },
    LanguageChoice {
        tag: "es-ES",
        short: "ES",
        native_name: "Español",
    },
    LanguageChoice {
        tag: "fr-FR",
        short: "FR",
        native_name: "Français",
    },
    LanguageChoice {
        tag: "de-DE",
        short: "DE",
        native_name: "Deutsch",
    },
    LanguageChoice {
        tag: "ja-JP",
        short: "日",
        native_name: "日本語",
    },
    LanguageChoice {
        tag: "ko-KR",
        short: "한",
        native_name: "한국어",
    },
    LanguageChoice {
        tag: "ar",
        short: "ع",
        native_name: "العربية",
    },
    LanguageChoice {
        tag: "pt-BR",
        short: "PT",
        native_name: "Português",
    },
    LanguageChoice {
        tag: "ru-RU",
        short: "RU",
        native_name: "Русский",
    },
];

fn language_choice(locale: &str) -> Option<LanguageChoice> {
    let language = locale
        .trim()
        .split(['-', '_', '.', '@'])
        .next()?
        .to_ascii_lowercase();
    let tag = match language.as_str() {
        "zh" => "zh-CN",
        "en" => "en-US",
        "es" => "es-ES",
        "fr" => "fr-FR",
        "de" => "de-DE",
        "ja" => "ja-JP",
        "ko" => "ko-KR",
        "ar" => "ar",
        "pt" => "pt-BR",
        "ru" => "ru-RU",
        _ => return None,
    };
    LANGUAGE_CHOICES
        .iter()
        .copied()
        .find(|choice| choice.tag == tag)
}

fn system_locales() -> &'static [String] {
    static LOCALES: OnceLock<Vec<String>> = OnceLock::new();
    LOCALES.get_or_init(|| sys_locale::get_locales().collect())
}

pub fn resolve_language(language: &str, preferred_locales: &[String]) -> &'static str {
    if !language.eq_ignore_ascii_case("auto") {
        return language_choice(language).map_or("zh-CN", |choice| choice.tag);
    }
    preferred_locales
        .iter()
        .find_map(|locale| language_choice(locale).map(|choice| choice.tag))
        .unwrap_or("zh-CN")
}

pub fn language_choice_for(language: &str) -> LanguageChoice {
    let resolved = resolve_language(language, system_locales());
    LANGUAGE_CHOICES
        .iter()
        .copied()
        .find(|choice| choice.tag == resolved)
        .unwrap_or(LANGUAGE_CHOICES[0])
}

fn localized_catalog(language: &str) -> Option<&'static str> {
    Some(match language {
        "es-ES" => include_str!("../resources/locales/es-ES.tsv"),
        "fr-FR" => include_str!("../resources/locales/fr-FR.tsv"),
        "de-DE" => include_str!("../resources/locales/de-DE.tsv"),
        "ja-JP" => include_str!("../resources/locales/ja-JP.tsv"),
        "ko-KR" => include_str!("../resources/locales/ko-KR.tsv"),
        "ar" => include_str!("../resources/locales/ar.tsv"),
        "pt-BR" => include_str!("../resources/locales/pt-BR.tsv"),
        "ru-RU" => include_str!("../resources/locales/ru-RU.tsv"),
        _ => return None,
    })
}

fn localized_value(language: &str, key: &str) -> Option<&'static str> {
    let catalog = localized_catalog(language)?;
    catalog.lines().find_map(|line| {
        let (resource_key, value) = line.split_once('\t')?;
        (resource_key == key).then_some(value)
    })
}

/// Resolves the configured language; automatic mode follows the process locale when available.
pub fn tr(language: &str, key: &str) -> &'static str {
    let language = resolve_language(language, system_locales());
    if language != "zh-CN"
        && language != "en-US"
        && let Some(value) = localized_value(language, key)
    {
        return value;
    }
    include_str!("../resources/tools.tsv")
        .lines()
        .find_map(|line| {
            let mut columns = line.split('\t');
            let id = columns.next()?;
            let zh = columns.next()?;
            let en = columns.next()?;
            (id == key).then_some(if language == "zh-CN" { zh } else { en })
        })
        .unwrap_or("?")
}

/// Unsupported archive-like extensions that still deserve a format-specific hint.
const KNOWN_ARCHIVE_EXTENSIONS: [&str; 7] = ["dmg", "wim", "arj", "z", "img", "nsis", "sit"];

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
    fn auto_uses_the_first_supported_system_language_and_explicit_choices_win() {
        let system = vec!["it-IT".into(), "zh-Hans-CN".into(), "en-US".into()];
        assert_eq!(resolve_language("auto", &system), "zh-CN");
        assert_eq!(resolve_language("en-GB", &["zh-CN".into()]), "en-US");
        assert_eq!(resolve_language("es-MX", &[]), "es-ES");
        assert_eq!(resolve_language("auto", &["C.UTF-8".into()]), "zh-CN");
    }

    #[test]
    fn language_list_has_ten_unique_tags_and_native_names() {
        let tags: std::collections::HashSet<_> =
            LANGUAGE_CHOICES.iter().map(|choice| choice.tag).collect();
        assert_eq!(tags.len(), 10);
        assert!(
            LANGUAGE_CHOICES
                .iter()
                .all(|choice| !choice.short.is_empty() && !choice.native_name.is_empty())
        );
    }

    #[test]
    fn every_localized_catalog_covers_all_resources_and_keeps_placeholders() {
        let base: Vec<_> = include_str!("../resources/tools.tsv")
            .lines()
            .map(|line| {
                let mut columns = line.split('\t');
                let key = columns.next().unwrap_or_default();
                let english = columns.nth(1).unwrap_or_default();
                (key, english)
            })
            .collect();
        for language in [
            "es-ES", "fr-FR", "de-DE", "ja-JP", "ko-KR", "ar", "pt-BR", "ru-RU",
        ] {
            let catalog = localized_catalog(language).unwrap_or_default();
            let keys: Vec<_> = catalog
                .lines()
                .filter_map(|line| line.split_once('\t').map(|(key, _)| key))
                .collect();
            assert_eq!(keys.len(), base.len(), "wrong number of {language} entries");
            assert_eq!(
                keys.iter().collect::<std::collections::HashSet<_>>().len(),
                keys.len(),
                "duplicate key in {language}"
            );
            for (key, english) in &base {
                let translated = localized_value(language, key);
                assert!(translated.is_some(), "{language} is missing {key}");
                if let Some(translated) = translated {
                    assert_eq!(
                        placeholders(english),
                        placeholders(translated),
                        "placeholder mismatch for {language}:{key}"
                    );
                }
            }
        }
    }

    fn placeholders(value: &str) -> std::collections::BTreeSet<&str> {
        value
            .split('{')
            .skip(1)
            .filter_map(|part| part.split_once('}').map(|(name, _)| name))
            .collect()
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
