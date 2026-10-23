use rz_core::{Error, Result};
use std::io::{Read, Seek, SeekFrom};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    Gzip,
    SevenZip,
    Rar,
    Xz,
    Zstd,
    Bzip2,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::Tar => "tar",
            Format::Gzip => "gzip",
            Format::SevenZip => "7z",
            Format::Rar => "rar",
            Format::Xz => "xz",
            Format::Zstd => "zstd",
            Format::Bzip2 => "bzip2",
        }
    }
}

/// Detects the format from magic bytes, never from the file extension.
pub fn sniff<R: Read + Seek>(r: &mut R) -> Result<Format> {
    let mut head = [0u8; 8];
    let n = read_up_to(r, &mut head)?;
    let head = &head[..n];
    let found = if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        Some(Format::Zip)
    } else if head.starts_with(&[0x1f, 0x8b]) {
        Some(Format::Gzip)
    } else if head.starts_with(&[0x37, 0x7a, 0xbc, 0xaf, 0x27, 0x1c]) {
        Some(Format::SevenZip)
    } else if head.starts_with(b"Rar!\x1a\x07") {
        Some(Format::Rar)
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        Some(Format::Xz)
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Some(Format::Zstd)
    } else if head.starts_with(b"BZh") {
        Some(Format::Bzip2)
    } else {
        None
    };
    if let Some(f) = found {
        return Ok(f);
    }
    // tar has its magic at offset 257 ("ustar").
    r.seek(SeekFrom::Start(257))?;
    let mut magic = [0u8; 5];
    let n = read_up_to(r, &mut magic)?;
    r.seek(SeekFrom::Start(0))?;
    if &magic[..n] == b"ustar" {
        Ok(Format::Tar)
    } else {
        Err(Error::UnknownFormat)
    }
}

fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match r.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn detects_by_magic() {
        assert_eq!(
            sniff(&mut Cursor::new(b"PK\x03\x04abcd".to_vec())).ok(),
            Some(Format::Zip)
        );
        assert_eq!(
            sniff(&mut Cursor::new(vec![0x1f, 0x8b, 8, 0])).ok(),
            Some(Format::Gzip)
        );
        assert!(sniff(&mut Cursor::new(b"plain text".to_vec())).is_err());
    }

    #[test]
    fn detects_tar_at_offset() {
        let mut data = vec![0u8; 512];
        data[257..262].copy_from_slice(b"ustar");
        assert_eq!(sniff(&mut Cursor::new(data)).ok(), Some(Format::Tar));
    }
}
