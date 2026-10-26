//! What the window does with the outside world: update checks, the anonymous install
//! report and remembered passwords. Everything that waits on a network or the keychain
//! runs on the background executor; the window only receives the result.

use crate::view::{MainView, Notice, PasswordTarget};
use gpui_kit::{Context, Window};
use rz_app::stats::{StatsState, report_if_due};
use rz_app::update::{Update, Updater, current_platform};
use rz_services::{
    HttpFetcher, HttpSender, UPDATE_MANIFEST_URL, UPDATE_PUBLIC_KEY, environment, stats_url,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub enum UpdateState {
    Available(Update),
    Downloading(Update),
}

/// The name an archive is remembered under: its file name, never its location.
pub fn password_label(archive: &Path) -> String {
    archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn today() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs() / 86_400).unwrap_or(0))
}

/// Hands a downloaded installer to the operating system.
fn open_with_system(path: &Path) {
    let (program, target): (&str, PathBuf) = match std::env::consts::OS {
        "macos" => ("open", path.to_path_buf()),
        "windows" => ("explorer", path.to_path_buf()),
        // A tarball is not an installer: show the folder it was saved in.
        _ => (
            "xdg-open",
            path.parent()
                .map_or_else(|| path.to_path_buf(), Path::to_path_buf),
        ),
    };
    let _ = Command::new(program).arg(target).spawn();
}

/// The largest file unpacked just to be opened for a look.
pub const MAX_PREVIEW_BYTES: u64 = 256 * 1024 * 1024;

/// Opens a file with the program the system uses for that kind of file.
pub fn open_file_with_system(path: &Path) {
    let program = match std::env::consts::OS {
        "macos" => "open",
        "windows" => "explorer",
        _ => "xdg-open",
    };
    let _ = Command::new(program).arg(path).spawn();
}

/// Files that run when opened. A preview must never start a program that came out of an
/// archive, so these are refused and the user is told to extract them deliberately.
pub fn is_runnable(name: &str) -> bool {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "exe"
            | "com"
            | "scr"
            | "bat"
            | "cmd"
            | "msi"
            | "msp"
            | "ps1"
            | "psm1"
            | "vbs"
            | "vbe"
            | "js"
            | "jse"
            | "wsf"
            | "wsh"
            | "hta"
            | "cpl"
            | "lnk"
            | "reg"
            | "jar"
            | "dll"
            | "sh"
            | "bash"
            | "zsh"
            | "command"
            | "app"
            | "pkg"
            | "dmg"
            | "action"
            | "workflow"
            | "appimage"
            | "desktop"
            | "run"
            | "bin"
            | "pif"
            | "gadget"
            | "inf"
            | "url"
            | "webloc"
            | "terminal"
            | "scpt"
            | "scptd"
            | "py"
            | "pl"
            | "rb"
    ) || name.is_empty()
}

/// A file name that is safe to create inside the scratch folder.
pub fn scratch_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_matches('.').to_string();
    if clean.is_empty() {
        "preview".into()
    } else {
        clean
    }
}

/// Writes preview data into this run's scratch folder and returns the new file's path.
/// Each preview gets its own numbered folder so equal names never clash.
pub fn write_scratch(name: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir()
        .join(format!("RustZIP-preview-{}", std::process::id()))
        .join(NEXT.fetch_add(1, Ordering::Relaxed).to_string());
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(scratch_name(name));
    std::fs::write(&path, data)?;
    Ok(path)
}

/// The file name a download is saved under, taken from its URL and made harmless.
fn download_name(url: &str) -> String {
    let last = url.rsplit('/').next().unwrap_or("");
    let last = last.split(['?', '#']).next().unwrap_or("");
    let clean: String = last
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if clean.is_empty() || clean.starts_with('.') {
        "RustZIP-update".into()
    } else {
        clean
    }
}

impl MainView {
    /// Called once after the window is up. Neither check may delay or break startup.
    pub fn start_background_services(&mut self, cx: &mut Context<Self>) {
        self.check_for_update(cx);
        self.report_install(cx);
    }

    fn check_for_update(&mut self, cx: &mut Context<Self>) {
        if !self.settings.check_updates {
            return;
        }
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async {
                    let fetcher = HttpFetcher::new();
                    Updater::new(&fetcher, UPDATE_MANIFEST_URL, UPDATE_PUBLIC_KEY)
                        .check(env!("CARGO_PKG_VERSION"), &current_platform())
                })
                .await;
            // Offline, a missing manifest and a bad one all look the same to the user: no update.
            if let Ok(Some(update)) = found {
                let _ = this.update(cx, |this, cx| {
                    this.update = Some(UpdateState::Available(update));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Downloads, verifies and opens the new version. Nothing is installed by us.
    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some(UpdateState::Available(update)) = self.update.clone() else {
            return;
        };
        self.update = Some(UpdateState::Downloading(update.clone()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let job = update.clone();
            let saved = cx
                .background_executor()
                .spawn(async move {
                    let fetcher = HttpFetcher::new();
                    let bytes = Updater::new(&fetcher, UPDATE_MANIFEST_URL, UPDATE_PUBLIC_KEY)
                        .download(&job)
                        .map_err(|e| e.to_string())?;
                    let dir = std::env::temp_dir().join(format!("RustZIP-update-{}", job.version));
                    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                    let file = dir.join(download_name(&job.asset.url));
                    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
                    Ok::<_, String>(file)
                })
                .await;
            let _ = this.update(cx, |this, cx| match saved {
                Ok(file) => {
                    this.update = None;
                    open_with_system(&file);
                    this.say(
                        Notice::Info(
                            this.tool_text("update_installed")
                                .replace("{version}", &update.version.to_string()),
                        ),
                        cx,
                    );
                }
                Err(e) => {
                    this.update = Some(UpdateState::Available(update));
                    this.say(
                        Notice::Error(this.tool_text("update_failed").replace("{error}", &e)),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// Sends the anonymous activation or daily report when one is due.
    pub fn report_install(&mut self, cx: &mut Context<Self>) {
        // Nothing is sent before the user has seen the notice, and never against their answer.
        if !self.settings.send_install_stats || !self.settings.privacy_notice_seen {
            return;
        }
        let (Some(url), Some(dir)) = (
            stats_url(),
            self.settings_path
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf),
        ) else {
            return;
        };
        cx.background_executor()
            .spawn(async move {
                let path = dir.join("stats.json");
                let mut state = StatsState::load(&path);
                let sender = HttpSender::new(&url);
                if report_if_due(&mut state, true, today(), &environment(), &sender).is_some() {
                    let _ = state.save(&path);
                }
            })
            .detach();
    }

    // ----- remembered passwords --------------------------------------------------------

    pub(crate) fn remember_password(&self, archive: &Path, secret: &str, cx: &mut Context<Self>) {
        let (vault, label, secret) = (
            self.vault.clone(),
            password_label(archive),
            secret.to_string(),
        );
        cx.background_executor()
            .spawn(async move {
                let _ = vault.set(&label, &secret);
            })
            .detach();
    }

    pub(crate) fn open_with_saved_password(
        &mut self,
        path: PathBuf,
        request_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (vault, label) = (self.vault.clone(), password_label(&path));
        cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move { vault.get(&label) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match saved {
                Some(pw) if this.open_requests.is_active(request_id) => {
                    this.open_path(path, Some(pw), window, cx);
                }
                None if this.open_requests.is_active(request_id) => {
                    this.open_requests.finish_active(request_id);
                    this.loading = None;
                    this.ask_password(PasswordTarget::Open(path), false, window, cx);
                }
                _ => {}
            });
        })
        .detach();
    }

    /// Removes every remembered password, for the settings page.
    pub fn forget_all_passwords(&mut self, cx: &mut Context<Self>) {
        let vault = self.vault.clone();
        cx.background_executor()
            .spawn(async move {
                for label in vault.labels() {
                    let _ = vault.delete(&label);
                }
            })
            .detach();
        self.say(Notice::Info(self.tool_text("passwords_cleared").into()), cx);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn programs_and_scripts_are_never_previewed() {
        for name in ["setup.EXE", "run.sh", "x.command", "a.app", "m.js", "k.lnk"] {
            assert!(is_runnable(name), "{name}");
        }
        for name in ["photo.png", "notes.txt", "report.pdf", "data.csv", "README"] {
            assert!(!is_runnable(name), "{name}");
        }
    }

    #[test]
    fn scratch_files_get_harmless_names_in_separate_folders() {
        assert_eq!(scratch_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(scratch_name("..."), "preview");
        assert_eq!(scratch_name("报告 终稿.pdf"), "报告 终稿.pdf");
        let a = write_scratch("x.txt", b"1").unwrap();
        let b = write_scratch("x.txt", b"2").unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&b).unwrap(), b"2");
        let _ = std::fs::remove_dir_all(a.parent().and_then(Path::parent).unwrap());
    }

    #[test]
    fn download_names_cannot_escape_the_folder() {
        assert_eq!(
            download_name("https://x/y/RustZIP-0.2.0.dmg"),
            "RustZIP-0.2.0.dmg"
        );
        assert_eq!(download_name("https://x/y/a.dmg?token=1#f"), "a.dmg");
        assert_eq!(download_name("https://x/y/"), "RustZIP-update");
        assert_eq!(download_name("https://x/..%2f..%2fevil"), "RustZIP-update");
        assert!(!download_name("https://x/a b/../../etc").contains('/'));
    }

    #[test]
    fn a_password_is_filed_under_the_archive_name_only() {
        assert_eq!(
            password_label(Path::new("/home/me/secret/Taxes 2025.7z")),
            "Taxes 2025.7z"
        );
    }
}
