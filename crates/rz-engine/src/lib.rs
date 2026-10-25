//! Compression and extraction engine. Everything read from an archive is untrusted input.

mod compress;
mod extract;
mod naming;
mod progress;
mod safe_path;
mod transform;

pub use compress::{compress, CompressReport};
pub use extract::{extract, ExtractOptions, ExtractReport, Overwrite};
pub use naming::{first_free, numbered_name, unused_path};
pub use safe_path::safe_join;
pub use transform::{transform, TransformOptions, TransformReport};
