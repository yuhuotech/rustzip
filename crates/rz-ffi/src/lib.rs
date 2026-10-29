//! Small safe wrappers around native parsers used by RustZIP.
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod libarchive;
#[cfg(feature = "rar")]
pub mod unrar;
