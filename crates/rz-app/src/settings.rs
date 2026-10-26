//! User settings: a small JSON file that survives upgrades and damage.

use rz_core::{CreateOptions, Error, Result};
use rz_engine::{ExtractOptions, Overwrite};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// What to do when an extracted file already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OnConflict {
    /// Nothing is lost and nothing fails: the new file gets a number (`name (1)`).
    /// Settings saved with the old "ask" mean the same thing now.
    #[default]
    #[serde(alias = "ask")]
    Rename,
    Skip,
    Replace,
}

/// Shared presentation preference for common locations and folder browsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FolderView {
    #[default]
    List,
    Icons,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// BCP 47 tag such as `zh-CN`, or `auto` to follow the system.
    pub language: String,
    pub theme: Theme,
    /// Accent colour: the name of a preset (`graphite`, `teal`...) or `#rrggbb`.
    /// The interface decides what it knows; anything else falls back to the default.
    pub accent: String,
    /// Output format for new archives, as a file extension (`zip`, `7z`, `tar.zst`...).
    pub default_format: String,
    pub default_level: u32,
    /// Worker threads; 0 means one per CPU.
    pub threads: usize,
    pub on_conflict: OnConflict,
    /// Put loose files in a folder named after the archive.
    pub smart_extract: bool,
    pub open_folder_when_done: bool,
    pub task_notifications: bool,
    /// Character set for old archive file names; `auto` guesses.
    pub name_encoding: String,
    pub check_updates: bool,
    /// Anonymous install statistics; see `stats`.
    pub send_install_stats: bool,
    /// The first-launch privacy notice has been shown and acknowledged.
    pub privacy_notice_seen: bool,
    /// Keep passwords that opened an archive in the system keychain. Off unless asked for.
    pub remember_passwords: bool,
    pub max_ratio: u64,
    pub ratio_check_above_mb: u64,
    pub max_total_gb: u64,
    pub recent: Vec<PathBuf>,
    /// User-selected folders; removing a bookmark never removes files.
    pub favorites: Vec<PathBuf>,
    /// Parents of successfully opened archives, most recently used first.
    pub recent_locations: Vec<PathBuf>,
    /// Suppresses a location until an archive from it is opened again.
    pub hidden_locations: Vec<PathBuf>,
    pub folder_view: FolderView,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme: Theme::System,
            accent: "graphite".into(),
            default_format: "zip".into(),
            default_level: 6,
            threads: 0,
            on_conflict: OnConflict::Rename,
            smart_extract: true,
            open_folder_when_done: true,
            task_notifications: false,
            name_encoding: "auto".into(),
            check_updates: true,
            send_install_stats: true,
            privacy_notice_seen: false,
            remember_passwords: false,
            max_ratio: 2000,
            ratio_check_above_mb: 256,
            max_total_gb: 1024,
            recent: Vec::new(),
            favorites: Vec::new(),
            recent_locations: Vec::new(),
            hidden_locations: Vec::new(),
            folder_view: FolderView::List,
        }
    }
}

const MAX_RECENT: usize = 20;

trait SettingsFileOps {
    fn write_and_sync(&self, tmp: &Path, contents: &[u8]) -> std::io::Result<()>;
    fn publish(&self, tmp: &Path, path: &Path) -> std::io::Result<()>;
    fn remove_temp(&self, tmp: &Path);
}

struct SystemSettingsFileOps;

impl SettingsFileOps for SystemSettingsFileOps {
    fn write_and_sync(&self, tmp: &Path, contents: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::File::create(tmp)?;
        file.write_all(contents)?;
        file.sync_all()
    }

    fn publish(&self, tmp: &Path, path: &Path) -> std::io::Result<()> {
        std::fs::rename(tmp, path)
    }

    fn remove_temp(&self, tmp: &Path) {
        let _ = std::fs::remove_file(tmp);
    }
}

impl Settings {
    /// Pulls every value back into a range the engine accepts, so a hand-edited
    /// or damaged file can never produce a nonsensical task.
    pub fn sanitized(mut self) -> Self {
        self.default_level = self.default_level.min(9);
        self.threads = self.threads.min(256);
        self.max_ratio = self.max_ratio.clamp(10, 1_000_000);
        self.ratio_check_above_mb = self.ratio_check_above_mb.min(1 << 20);
        self.max_total_gb = self.max_total_gb.clamp(1, 1 << 20);
        if self.accent.trim().is_empty() {
            self.accent = "graphite".into();
        }
        if self.language.trim().is_empty() {
            self.language = "auto".into();
        }
        if self.name_encoding.trim().is_empty() {
            self.name_encoding = "auto".into();
        }
        self.recent.truncate(MAX_RECENT);
        let mut seen = std::collections::HashSet::new();
        self.favorites
            .retain(|p| p.is_absolute() && seen.insert(p.clone()));
        // Old settings only contain recent archives; reuse their parents without
        // resurrecting locations the user explicitly removed.
        for archive in &self.recent {
            if let Some(parent) = archive.parent().filter(|p| p.is_absolute()) {
                if !self.recent_locations.iter().any(|p| p == parent)
                    && !self.hidden_locations.iter().any(|p| p == parent)
                {
                    self.recent_locations.push(parent.to_path_buf());
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        self.recent_locations.retain(|p| {
            p.is_absolute() && seen.insert(p.clone()) && !self.hidden_locations.contains(p)
        });
        self.recent_locations.truncate(MAX_RECENT);
        let mut seen = std::collections::HashSet::new();
        self.hidden_locations
            .retain(|p| p.is_absolute() && seen.insert(p.clone()));
        self
    }

    pub fn extract_options(&self, password: Option<String>) -> ExtractOptions {
        ExtractOptions {
            overwrite: match self.on_conflict {
                OnConflict::Rename => Overwrite::Rename,
                OnConflict::Skip => Overwrite::Skip,
                OnConflict::Replace => Overwrite::Replace,
            },
            max_ratio: self.max_ratio,
            ratio_check_above: self.ratio_check_above_mb.saturating_mul(1 << 20),
            max_total: self.max_total_gb.saturating_mul(1 << 30),
            password,
            threads: self.threads,
            only: None,
        }
    }

    pub fn create_options(&self, password: Option<String>) -> CreateOptions {
        CreateOptions {
            level: self.default_level,
            password,
            threads: self.threads,
            ..CreateOptions::default()
        }
    }

    /// The configured name encoding, or `None` to let readers guess.
    pub fn forced_encoding(&self) -> Option<&str> {
        let e = self.name_encoding.trim();
        (!e.eq_ignore_ascii_case("auto") && !e.is_empty()).then_some(e)
    }

    /// Moves `path` to the front of the recent list.
    pub fn remember(&mut self, path: &Path) {
        if let Some(parent) = path.parent().filter(|p| p.is_absolute()) {
            self.hidden_locations.retain(|p| p != parent);
            self.recent_locations.retain(|p| p != parent);
            self.recent_locations.insert(0, parent.to_path_buf());
            self.recent_locations.truncate(MAX_RECENT);
        }
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    /// Removes a shortcut, preserving the real directory and all its contents.
    pub fn hide_location(&mut self, path: &Path) {
        self.favorites.retain(|p| p != path);
        self.recent_locations.retain(|p| p != path);
        if !self.hidden_locations.iter().any(|p| p == path) {
            self.hidden_locations.push(path.to_path_buf());
        }
    }

    /// Reads the file. A missing file gives the defaults; a damaged one is moved
    /// aside (`settings.json.bad`) and the defaults are used, so the user's
    /// text is never silently destroyed and the app always starts.
    pub fn load(path: &Path) -> Settings {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Settings::default(),
        };
        match serde_json::from_str::<Settings>(&text) {
            Ok(s) => s.sanitized(),
            Err(_) => {
                let mut bad = path.as_os_str().to_owned();
                bad.push(".bad");
                let _ = std::fs::rename(path, bad);
                Settings::default()
            }
        }
    }

    /// Writes atomically: a crash mid-save leaves the old file intact, a failed
    /// save never replaces it, and concurrent savers each use their own
    /// temporary file so their writes never interleave.
    pub fn save(&self, path: &Path) -> Result<()> {
        // Savers run one at a time, so a slower, older snapshot cannot be
        // published after a newer one.
        static SERIALIZE: Mutex<()> = Mutex::new(());
        let _turn = SERIALIZE.lock().unwrap_or_else(|p| p.into_inner());
        self.save_with_io(path, &SystemSettingsFileOps)
    }

    fn save_with_io(&self, path: &Path, ops: &dyn SettingsFileOps) -> Result<()> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&self.clone().sanitized())
            .map_err(|e| Error::Corrupt(e.to_string()))?;
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = path.with_file_name(format!("{name}.rz-{}-{seq}.tmp", std::process::id()));
        let outcome = ops
            .write_and_sync(&tmp, json.as_bytes())
            .and_then(|()| ops.publish(&tmp, path));
        if outcome.is_err() {
            ops.remove_temp(&tmp);
        }
        outcome.map_err(Error::Io)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

/// Where RustZIP keeps its files on each system. The environment is passed in
/// so every platform's rule can be tested on any machine.
pub fn config_dir_for(os: Os, env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let non_empty = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    match os {
        Os::Windows => non_empty("APPDATA").map(|d| d.join("RustZIP")),
        Os::MacOs => non_empty("HOME").map(|h| h.join("Library/Application Support/RustZIP")),
        Os::Linux => non_empty("XDG_CONFIG_HOME")
            .filter(|d| d.is_absolute())
            .or_else(|| non_empty("HOME").map(|h| h.join(".config")))
            .map(|d| d.join("rustzip")),
    }
}

pub fn config_dir() -> Option<PathBuf> {
    config_dir_for(Os::current(), &|k| std::env::var_os(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).map(OsString::from)
    }

    #[test]
    fn config_dir_follows_each_platforms_convention() {
        assert_eq!(
            config_dir_for(
                Os::Windows,
                &env(&[("APPDATA", r"C:\Users\a\AppData\Roaming")])
            ),
            Some(PathBuf::from(r"C:\Users\a\AppData\Roaming").join("RustZIP"))
        );
        assert_eq!(
            config_dir_for(Os::MacOs, &env(&[("HOME", "/Users/a")])),
            Some(PathBuf::from(
                "/Users/a/Library/Application Support/RustZIP"
            ))
        );
        assert_eq!(
            config_dir_for(Os::Linux, &env(&[("HOME", "/home/a")])),
            Some(PathBuf::from("/home/a/.config/rustzip"))
        );
        assert_eq!(
            config_dir_for(
                Os::Linux,
                &env(&[("XDG_CONFIG_HOME", "/x/cfg"), ("HOME", "/home/a")])
            ),
            Some(PathBuf::from("/x/cfg/rustzip"))
        );
        // A relative XDG path is invalid per the spec and must be ignored.
        assert_eq!(
            config_dir_for(
                Os::Linux,
                &env(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/home/a")])
            ),
            Some(PathBuf::from("/home/a/.config/rustzip"))
        );
        assert_eq!(config_dir_for(Os::Linux, &env(&[])), None);
        assert_eq!(config_dir_for(Os::MacOs, &env(&[("HOME", "")])), None);
    }

    #[test]
    fn defaults_round_trip_through_a_file() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        assert!(!Settings::default().task_notifications);
        let path = dir.path().join("sub/settings.json");
        let mut s = Settings {
            theme: Theme::Dark,
            default_level: 9,
            ..Settings::default()
        };
        s.remember(Path::new("/a/b.zip"));
        assert!(s.save(&path).is_ok());
        assert_eq!(Settings::load(&path), s);
        assert!(!dir.path().join("sub/settings.json.tmp").exists());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        assert_eq!(
            Settings::load(&dir.path().join("none.json")),
            Settings::default()
        );
    }

    #[test]
    fn damaged_file_is_kept_aside_and_defaults_are_used() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let path = dir.path().join("settings.json");
        let _ = std::fs::write(&path, "{ this is not json");
        assert_eq!(Settings::load(&path), Settings::default());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.json.bad")).ok(),
            Some("{ this is not json".to_string())
        );
    }

    #[test]
    fn older_and_newer_files_still_load() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let path = dir.path().join("settings.json");
        // Only one known key plus one from a future version.
        let _ = std::fs::write(&path, r#"{"theme":"light","from_the_future":42}"#);
        let s = Settings::load(&path);
        assert_eq!(s.theme, Theme::Light);
        assert_eq!(s.default_level, 6);
    }

    #[test]
    fn out_of_range_values_are_pulled_into_range() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let path = dir.path().join("settings.json");
        let _ = std::fs::write(
            &path,
            r#"{"default_level":99,"threads":100000,"max_ratio":0,"max_total_gb":0,"language":" "}"#,
        );
        let s = Settings::load(&path);
        assert_eq!(s.default_level, 9);
        assert_eq!(s.threads, 256);
        assert!(s.max_ratio >= 10);
        assert!(s.max_total_gb >= 1);
        assert_eq!(s.language, "auto");
    }

    #[test]
    fn conflict_policy_maps_to_the_engine_and_the_default_loses_nothing() {
        let mut s = Settings::default();
        assert_eq!(s.extract_options(None).overwrite, Overwrite::Rename);
        // Files saved when "ask" existed keep working.
        let old: Settings = serde_json::from_str(r#"{"on_conflict":"ask"}"#).unwrap();
        assert_eq!(old.on_conflict, OnConflict::Rename);
        s.on_conflict = OnConflict::Skip;
        assert_eq!(s.extract_options(None).overwrite, Overwrite::Skip);
        s.on_conflict = OnConflict::Replace;
        assert_eq!(s.extract_options(None).overwrite, Overwrite::Replace);
        let o = s.extract_options(Some("pw".into()));
        assert_eq!(o.password.as_deref(), Some("pw"));
        assert_eq!(o.ratio_check_above, 256 << 20);
    }

    #[test]
    fn recent_list_is_deduplicated_ordered_and_bounded() {
        let mut s = Settings::default();
        for i in 0..30 {
            s.remember(Path::new(&format!("/a/{i}.zip")));
        }
        s.remember(Path::new("/a/5.zip"));
        assert_eq!(s.recent.len(), 20);
        assert_eq!(s.recent[0], PathBuf::from("/a/5.zip"));
        assert_eq!(s.recent.iter().filter(|p| p.ends_with("5.zip")).count(), 1);
    }

    #[test]
    fn auto_encoding_means_no_forced_charset() {
        let mut s = Settings::default();
        assert_eq!(s.forced_encoding(), None);
        s.name_encoding = "GBK".into();
        assert_eq!(s.forced_encoding(), Some("GBK"));
    }

    #[test]
    fn favorite_folders_round_trip_and_remove_relative_or_duplicate_paths() {
        let dir = tempfile::tempdir().unwrap();
        let absolute = dir.path().to_path_buf();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            favorites: vec![
                absolute.clone(),
                PathBuf::from("relative"),
                absolute.clone(),
            ],
            ..Settings::default()
        }
        .sanitized();
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).favorites, vec![absolute]);
    }

    #[test]
    fn recent_locations_are_bounded_reordered_and_backfilled_without_resurrecting_hidden_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        for ix in 0..25 {
            settings.remember(&dir.path().join(format!("project-{ix}/archive.zip")));
        }
        assert_eq!(settings.recent_locations.len(), 20);
        let chosen = dir.path().join("project-10");
        settings.remember(&chosen.join("other.zip"));
        assert_eq!(settings.recent_locations[0], chosen);
        settings.hide_location(&chosen);
        settings.folder_view = FolderView::Icons;
        let path = dir.path().join("settings.json");
        settings.save(&path).unwrap();
        let restored = Settings::load(&path);
        assert_eq!(restored.folder_view, FolderView::Icons);
        assert!(!restored.recent_locations.contains(&chosen));
        assert!(restored.hidden_locations.contains(&chosen));
        let older = Settings {
            recent: vec![
                chosen.join("a.zip"),
                chosen.join("b.zip"),
                dir.path().join("elsewhere/c.zip"),
            ],
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(
            older.recent_locations,
            vec![chosen.clone(), dir.path().join("elsewhere")]
        );
        let mut restored = restored;
        restored.remember(&chosen.join("reopened.zip"));
        assert_eq!(restored.recent_locations[0], chosen);
        assert!(!restored.hidden_locations.contains(&chosen));
    }

    #[test]
    fn partial_write_sync_and_publish_failures_preserve_the_last_good_settings() {
        #[derive(Clone, Copy)]
        enum Failure {
            Write,
            Sync,
            Publish,
        }
        struct Fault(Failure);
        impl SettingsFileOps for Fault {
            fn write_and_sync(&self, tmp: &Path, contents: &[u8]) -> std::io::Result<()> {
                match self.0 {
                    Failure::Write => {
                        use std::io::Write;
                        let mut file = std::fs::File::create(tmp)?;
                        file.write_all(&contents[..contents.len().min(12)])?;
                        Err(std::io::Error::other("injected write failure"))
                    }
                    Failure::Sync => {
                        std::fs::write(tmp, contents)?;
                        Err(std::io::Error::other("injected sync failure"))
                    }
                    Failure::Publish => SystemSettingsFileOps.write_and_sync(tmp, contents),
                }
            }
            fn publish(&self, tmp: &Path, path: &Path) -> std::io::Result<()> {
                if matches!(self.0, Failure::Publish) {
                    Err(std::io::Error::other("injected publish failure"))
                } else {
                    SystemSettingsFileOps.publish(tmp, path)
                }
            }
            fn remove_temp(&self, tmp: &Path) {
                SystemSettingsFileOps.remove_temp(tmp);
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let good = Settings {
            theme: Theme::Dark,
            accent: "last-good".into(),
            ..Settings::default()
        };
        good.save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let update = Settings {
            theme: Theme::Light,
            accent: "new".into(),
            ..good.clone()
        };
        for failure in [Failure::Write, Failure::Sync, Failure::Publish] {
            assert!(update.save_with_io(&path, &Fault(failure)).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), original);
            assert_eq!(Settings::load(&path), good);
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                1,
                "failed save left a temp file"
            );
        }
    }
}
