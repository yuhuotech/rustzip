#![forbid(unsafe_code)]

#[derive(Debug)]
pub enum Error {
    InvalidLzms(&'static str),
    ArithmeticOverflow(&'static str),
    AllocationFailed(&'static str),
    Cancelled,
    LimitExceeded {
        kind: LimitKind,
        limit: u64,
        actual: u64,
    },
}
#[derive(Debug)]
pub enum LimitKind {
    FileBytes,
    PayloadBytes,
}
pub type Result<T> = std::result::Result<T, Error>;

pub struct Limits {
    pub max_file_bytes: u64,
    pub max_payload_bytes: u64,
}
impl Limits {
    fn validate(&self) -> Result<()> {
        Ok(())
    }
}

mod decoder;
pub use decoder::{decompress_lzms, decompress_lzms_into, decompress_raw_with_cancel};
