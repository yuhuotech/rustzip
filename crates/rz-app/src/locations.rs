//! Common locations: system folders followed by recent archive parents and old bookmarks.

use crate::settings::Settings;
pub use rz_platform::UserDirectory;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub system: Option<UserDirectory>,
}

fn identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Runs on a background executor. Missing default folders are omitted; recently
/// used missing folders remain visible so they can be removed or reconnected.
pub fn collect(settings: &Settings) -> Vec<Location> {
    collect_with_defaults(settings, rz_platform::user_directories())
}

/// Explicit defaults make ordering, deduplication and redirected paths testable.
pub fn collect_with_defaults(
    settings: &Settings,
    defaults: Vec<(UserDirectory, PathBuf)>,
) -> Vec<Location> {
    let hidden: HashSet<_> = settings
        .hidden_locations
        .iter()
        .map(|p| identity(p))
        .collect();
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    let candidates = defaults
        .into_iter()
        .filter(|(_, p)| p.is_dir())
        .map(|(k, p)| (p, Some(k)))
        .chain(
            settings
                .recent_locations
                .iter()
                .chain(&settings.favorites)
                .cloned()
                .map(|p| (p, None)),
        );
    for (path, system) in candidates {
        let key = identity(&path);
        if path.is_absolute() && !hidden.contains(&key) && seen.insert(key) {
            result.push(Location { path, system });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_first_recent_parents_are_deduplicated_and_hidden_paths_stay_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let download = dir.path().join("redirected-downloads");
        let documents = dir.path().join("documents");
        let recent = dir.path().join("project");
        for p in [&download, &documents, &recent] {
            std::fs::create_dir(p).unwrap();
        }
        let mut settings = Settings {
            favorites: vec![recent.clone()],
            ..Settings::default()
        };
        settings.remember(&recent.join("a.zip"));
        settings.remember(&download.join("b.zip"));
        let defaults = vec![
            (UserDirectory::Downloads, download.clone()),
            (UserDirectory::Documents, documents.clone()),
            (UserDirectory::Pictures, dir.path().join("missing")),
        ];
        let items = collect_with_defaults(&settings, defaults.clone());
        assert_eq!(
            items.iter().map(|l| &l.path).collect::<Vec<_>>(),
            vec![&download, &documents, &recent]
        );
        settings.hide_location(&download);
        assert!(!collect_with_defaults(&settings, defaults.clone())
            .iter()
            .any(|l| l.path == download));
        settings.remember(&download.join("c.zip"));
        assert_eq!(collect_with_defaults(&settings, defaults)[0].path, download);
        assert!(documents.is_dir());
    }
}
