//! The parts of RustZIP that reach outside the process: HTTP and the system keychain.
//!
//! `rz-app` defines what an update check, a statistics report and a password store
//! *do* and tests them against fakes. This crate supplies the real transports, so the
//! front ends need no networking code of their own.

#![forbid(unsafe_code)]

mod http;
mod keychain;
mod recycle;
mod sysinfo;

pub use http::{HttpFetcher, HttpSender};
pub use keychain::KeychainStore;
pub use recycle::move_to_trash;
pub use sysinfo::environment;

/// Where the update manifest lives: a file attached to the latest GitHub release.
pub const UPDATE_MANIFEST_URL: &str =
    "https://github.com/yuhuotech/rustzip/releases/latest/download/update.json";

/// The release page, for platforms where the user installs the update by hand.
pub const RELEASES_URL: &str = "https://github.com/yuhuotech/rustzip/releases/latest";

/// Public half of the release signing key. The private half never enters the repository:
/// it is a secret of the release workflow (see `docs/release.md`).
pub const UPDATE_PUBLIC_KEY: [u8; 32] = include!("update_key.in");

const BUILT_STATS_URL: &str = env!("RZ_STATS_URL");

/// Where anonymous install statistics are posted, if anywhere. Set `RZ_STATS_URL` when
/// building a release; an empty value means nothing is ever sent. The environment
/// variable of the same name overrides it at run time.
pub fn stats_url() -> Option<String> {
    std::env::var("RZ_STATS_URL")
        .ok()
        .or_else(|| (!BUILT_STATS_URL.is_empty()).then(|| BUILT_STATS_URL.to_string()))
        .filter(|u| u.starts_with("https://"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{stats_url, BUILT_STATS_URL};
    use std::process::Command;

    #[test]
    fn build_time_stats_url_is_used_when_runtime_override_is_absent() {
        if BUILT_STATS_URL.is_empty() {
            return;
        }
        if std::env::var_os("RZ_STATS_URL_BUILD_TEST_CHILD").is_some() {
            assert_eq!(stats_url().as_deref(), Some(BUILT_STATS_URL));
            return;
        }

        let output = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("tests::build_time_stats_url_is_used_when_runtime_override_is_absent")
            .arg("--nocapture")
            .env_remove("RZ_STATS_URL")
            .env("RZ_STATS_URL_BUILD_TEST_CHILD", "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "child test failed: {}{}",
            stdout,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed; 0 failed"),
            "child test did not run: {stdout}"
        );
    }

    #[test]
    fn unset_build_and_runtime_urls_disable_statistics() {
        if BUILT_STATS_URL.is_empty() && std::env::var_os("RZ_STATS_URL").is_none() {
            assert_eq!(stats_url(), None);
        }
    }
}
