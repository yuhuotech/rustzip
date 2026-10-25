//! Standard user folders resolved by the OS conventions, including redirection.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserDirectory {
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
}

impl UserDirectory {
    /// Stable resource key; presentation supplies the localized display name.
    pub fn key(self) -> &'static str {
        match self {
            Self::Desktop => "dir_desktop",
            Self::Downloads => "dir_downloads",
            Self::Documents => "dir_documents",
            Self::Pictures => "dir_pictures",
            Self::Music => "dir_music",
            Self::Videos => "dir_videos",
        }
    }
}

/// Uses Windows Known Folders, Linux XDG user-dirs, and macOS standard folders.
/// Calls may read configuration; callers must run this off the GUI thread.
pub fn user_directories() -> Vec<(UserDirectory, PathBuf)> {
    [
        (UserDirectory::Desktop, dirs::desktop_dir()),
        (UserDirectory::Downloads, dirs::download_dir()),
        (UserDirectory::Documents, dirs::document_dir()),
        (UserDirectory::Pictures, dirs::picture_dir()),
        (UserDirectory::Music, dirs::audio_dir()),
        (UserDirectory::Videos, dirs::video_dir()),
    ]
    .into_iter()
    .filter_map(|(kind, path)| path.map(|p| (kind, p)))
    .collect()
}
