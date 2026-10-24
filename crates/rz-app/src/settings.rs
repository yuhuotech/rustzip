//! User settings: a small JSON file that survives upgrades and damage.

use rz_core::{CreateOptions, Error, Result};
use rz_engine::{ExtractOptions, Overwrite};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    /// The UI asks; until it answers, the engine refuses to overwrite.
    #[default]
    Ask,
    Skip,
    Replace,
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
            on_conflict: OnConflict::Ask,
            smart_extract: true,
            open_folder_when_done: true,
            name_encoding: "auto".into(),
            check_updates: true,
            send_install_stats: true,
            privacy_notice_seen: false,
            remember_passwords: false,
            max_ratio: 2000,
            ratio_check_above_mb: 256,
            max_total_gb: 1024,
            recent: Vec::new(),
        }
    }
}

const MAX_RECENT: usize = 20;

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
        self
    }

    pub fn extract_options(&self, password: Option<String>) -> ExtractOptions {
        ExtractOptions {
            // "Ask" is a UI concern; the engine's safe default is to refuse.
            overwrite: match self.on_conflict {
                OnConflict::Ask => Overwrite::Error,
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
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
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

    /// Writes atomically: a crash mid-save leaves the old file intact.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(&self.clone().sanitized())
            .map_err(|e| Error::Corrupt(e.to_string()))?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        Ok(())
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
    fn conflict_policy_maps_to_the_engine_and_ask_is_safe() {
        let mut s = Settings::default();
        assert_eq!(s.extract_options(None).overwrite, Overwrite::Error);
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
}
