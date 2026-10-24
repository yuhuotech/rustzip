//! The parts of RustZIP that reach outside the process: HTTP and the system keychain.
//!
//! `rz-app` defines what an update check, a statistics report and a password store
//! *do* and tests them against fakes. This crate supplies the real transports, so the
//! front ends need no networking code of their own.

#![forbid(unsafe_code)]

mod http;
mod keychain;
mod sysinfo;

pub use http::{HttpFetcher, HttpSender};
pub use keychain::KeychainStore;
pub use sysinfo::environment;

/// Where the update manifest lives: a file attached to the latest GitHub release.
pub const UPDATE_MANIFEST_URL: &str =
    "https://github.com/yuhuotech/rustzip/releases/latest/download/update.json";

/// The release page, for platforms where the user installs the update by hand.
pub const RELEASES_URL: &str = "https://github.com/yuhuotech/rustzip/releases/latest";

/// Public half of the release signing key. The private half never enters the repository:
/// it is a secret of the release workflow (see `docs/release.md`).
pub const UPDATE_PUBLIC_KEY: [u8; 32] = include!("update_key.in");

/// Where anonymous install statistics are posted, if anywhere. Set `RZ_STATS_URL` when
/// building a release; an empty value means nothing is ever sent. The environment
/// variable of the same name overrides it at run time.
pub fn stats_url() -> Option<String> {
    std::env::var("RZ_STATS_URL")
        .ok()
        .or_else(|| option_env!("RZ_STATS_URL").map(str::to_string))
        .filter(|u| u.starts_with("https://"))
}
