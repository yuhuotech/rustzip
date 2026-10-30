//! What the window does with the outside world: update checks, the anonymous install
//! report and remembered passwords. Everything that waits on a network or the keychain
//! runs on the background executor; the window only receives the result.

use crate::view::{MainView, Notice, PasswordTarget};
use gpui_kit::{Context, Window};
use rz_app::passwords::{MAX_AUTO_PASSWORD_CANDIDATES, PasswordBook, find_password_with_cancel};
use rz_app::stats::{StatsState, report_if_due};
use rz_app::update::{Fetcher, Update, Updater, current_platform};
use rz_core::CancelToken;
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

enum PasswordSearchFailure {
    Keychain,
    Archive(rz_core::Error),
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
fn open_with_system(path: &Path) -> std::io::Result<()> {
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
    Command::new(program).arg(target).spawn().map(|_| ())
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

fn download_and_open_update<F: Fetcher>(
    updater: &Updater<'_, F>,
    update: &Update,
    dir: &Path,
    open: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let bytes = updater.download(update).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = dir.join(download_name(&update.asset.url));
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    open(&file).map_err(|e| e.to_string())
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
                    let dir = std::env::temp_dir().join(format!("RustZIP-update-{}", job.version));
                    download_and_open_update(
                        &Updater::new(&fetcher, UPDATE_MANIFEST_URL, UPDATE_PUBLIC_KEY),
                        &job,
                        &dir,
                        open_with_system,
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| match saved {
                Ok(()) => {
                    this.update = None;
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
        let (vault, label, lookup_path) = (self.vault.clone(), password_label(&path), path.clone());
        let cancel = CancelToken::new();
        self.password_search_cancel = Some((request_id, cancel.clone()));
        cx.spawn_in(window, async move |this, cx| {
            let search = cancel.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let candidates = match PasswordBook::new(vault).try_candidates_with_cancel(
                        Some(&label),
                        MAX_AUTO_PASSWORD_CANDIDATES,
                        &search,
                    ) {
                        Ok(candidates) => candidates,
                        Err(rz_core::Error::Cancelled) => {
                            return Err(PasswordSearchFailure::Archive(rz_core::Error::Cancelled));
                        }
                        Err(_) => return Err(PasswordSearchFailure::Keychain),
                    };
                    find_password_with_cancel(&lookup_path, &candidates, &search)
                        .map_err(PasswordSearchFailure::Archive)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this
                    .password_search_cancel
                    .as_ref()
                    .is_some_and(|(id, _)| *id == request_id)
                {
                    this.password_search_cancel = None;
                }
                if !this.open_requests.is_active(request_id) {
                    return;
                }
                match result {
                    Ok(Some(password)) => {
                        this.open_path(path, Some(password.to_string()), window, cx);
                    }
                    Ok(None) => {
                        this.open_requests.finish_active(request_id);
                        this.loading = None;
                        this.ask_password(PasswordTarget::Open(path), false, window, cx);
                    }
                    Err(PasswordSearchFailure::Keychain) => {
                        this.open_requests.finish_active(request_id);
                        this.loading = None;
                        this.say(Notice::Error(this.tool_text("vault_error").into()), cx);
                        this.ask_password(PasswordTarget::Open(path), false, window, cx);
                    }
                    Err(PasswordSearchFailure::Archive(rz_core::Error::Cancelled)) => {
                        this.open_requests.finish_active(request_id);
                        this.loading = None;
                        this.ask_password(PasswordTarget::Open(path), false, window, cx);
                    }
                    Err(PasswordSearchFailure::Archive(error)) => {
                        this.open_requests.finish_active(request_id);
                        this.loading = None;
                        let message =
                            crate::text::open_error(&this.settings.language, &path, &error);
                        this.say(Notice::Error(message.into()), cx);
                        this.ask_password(PasswordTarget::Open(path), false, window, cx);
                    }
                }
            });
        })
        .detach();
    }

    /// Tries the password book for a task that has reached an encrypted entry.
    pub(crate) fn try_password_book_for_task(
        &mut self,
        task_id: rz_app::tasks::TaskId,
        archive: PathBuf,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.settings.remember_passwords || self.password_task_searches.contains_key(&task_id) {
            return false;
        }
        let (vault, label, lookup_path) = (
            self.vault.clone(),
            password_label(&archive),
            archive.clone(),
        );
        let cancel = CancelToken::new();
        self.password_task_searches.insert(task_id, cancel.clone());
        cx.spawn(async move |this, cx| {
            let search = cancel.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let candidates = match PasswordBook::new(vault).try_candidates_with_cancel(
                        Some(&label),
                        MAX_AUTO_PASSWORD_CANDIDATES,
                        &search,
                    ) {
                        Ok(candidates) => candidates,
                        Err(rz_core::Error::Cancelled) => {
                            return Err(PasswordSearchFailure::Archive(rz_core::Error::Cancelled));
                        }
                        Err(_) => return Err(PasswordSearchFailure::Keychain),
                    };
                    find_password_with_cancel(&lookup_path, &candidates, &search)
                        .map_err(PasswordSearchFailure::Archive)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(active_search) = this.password_task_searches.remove(&task_id) else {
                    return;
                };
                if active_search.is_cancelled() || !this.settings.remember_passwords {
                    return;
                }
                let still_password_failure = this.tasks.get(task_id).is_some_and(|task| {
                    task.state == rz_app::tasks::TaskState::Failed
                        && task
                            .error
                            .is_some_and(|error| error.code == rz_app::tasks::ErrorCode::Password)
                });
                if !still_password_failure {
                    return;
                }
                match result {
                    Ok(Some(password)) => {
                        if this
                            .retry_task_with_password(task_id, password.as_str(), cx)
                            .is_some()
                        {
                            this.remember_password(&archive, password.as_str(), cx);
                        }
                    }
                    Ok(None) => {
                        let title = this
                            .tasks
                            .get(task_id)
                            .map_or_else(|| password_label(&archive), |task| task.title);
                        this.say(
                            Notice::Error(
                                this.tool_text("task_password_needed")
                                    .replace("{task}", &title),
                            ),
                            cx,
                        );
                    }
                    Err(PasswordSearchFailure::Keychain) => {
                        this.say(Notice::Error(this.tool_text("vault_error").into()), cx);
                    }
                    Err(PasswordSearchFailure::Archive(rz_core::Error::Cancelled)) => {}
                    Err(PasswordSearchFailure::Archive(error)) => {
                        let message =
                            crate::text::open_error(&this.settings.language, &archive, &error);
                        this.say(Notice::Error(message.into()), cx);
                    }
                }
            });
        })
        .detach();
        true
    }

    /// Removes every remembered password, for the settings page.
    pub fn forget_all_passwords(&mut self, cx: &mut Context<Self>) {
        self.cancel_password_search();
        self.cancel_all_password_task_searches();
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
    fn update_handoff_verifies_before_opening_and_propagates_launch_failure() {
        use ed25519_dalek::{Signer, SigningKey};
        use rz_app::update::{Asset, hex, signed_message};
        use std::cell::Cell;

        struct Bytes(Vec<u8>);
        impl Fetcher for Bytes {
            fn get(&self, _: &str, _: u64) -> rz_core::Result<Vec<u8>> {
                Ok(self.0.clone())
            }
        }
        let key = SigningKey::from_bytes(&[7; 32]);
        let version = "0.2.0";
        let platform = "macos-aarch64";
        let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let signature = key.sign(&signed_message(version, platform, digest));
        let update = Update {
            version: version.parse().unwrap(),
            notes: String::new(),
            asset: Asset {
                platform: platform.into(),
                url: "https://example.com/installer.dmg".into(),
                size: 3,
                sha256: digest.into(),
                signature: hex(&signature.to_bytes()),
            },
        };
        let dir = tempfile::tempdir().unwrap();
        let fetcher = Bytes(b"abc".to_vec());
        let updater = Updater::new(
            &fetcher,
            "https://example.com/update.json",
            key.verifying_key().to_bytes(),
        );
        let called = Cell::new(false);
        let result = download_and_open_update(&updater, &update, dir.path(), |path| {
            called.set(true);
            assert_eq!(std::fs::read(path).unwrap(), b"abc");
            Err(std::io::Error::other("launch failed"))
        });
        assert!(called.get());
        assert!(result.unwrap_err().contains("launch failed"));

        called.set(false);
        assert!(
            download_and_open_update(&updater, &update, dir.path(), |path| {
                called.set(true);
                assert_eq!(std::fs::read(path).unwrap(), b"abc");
                Ok(())
            })
            .is_ok()
        );
        assert!(called.get());

        let bad = Bytes(b"bad".to_vec());
        let updater = Updater::new(
            &bad,
            "https://example.com/update.json",
            key.verifying_key().to_bytes(),
        );
        called.set(false);
        assert!(
            download_and_open_update(&updater, &update, dir.path(), |_| {
                called.set(true);
                Ok(())
            })
            .is_err()
        );
        assert!(!called.get());
    }

    #[test]
    fn a_password_is_filed_under_the_archive_name_only() {
        assert_eq!(
            password_label(Path::new("/home/me/secret/Taxes 2025.7z")),
            "Taxes 2025.7z"
        );
    }
}
