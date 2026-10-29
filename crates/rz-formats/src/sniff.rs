use rz_core::{Error, Result};
use std::io::{Read, Seek, SeekFrom};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    Tar,
    Gzip,
    SevenZip,
    Rar,
    Rpm,
    Dmg,
    Chm,
    Wim,
    Iso9660,
    Cab,
    Cpio,
    Ar,
    Lha,
    Xar,
    Xz,
    Zstd,
    Bzip2,
    Lz4,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::Tar => "tar",
            Format::Gzip => "gzip",
            Format::SevenZip => "7z",
            Format::Rar => "rar",
            Format::Rpm => "rpm",
            Format::Dmg => "dmg",
            Format::Chm => "chm",
            Format::Wim => "wim",
            Format::Iso9660 => "iso",
            Format::Cab => "cab",
            Format::Cpio => "cpio",
            Format::Ar => "ar",
            Format::Lha => "lha",
            Format::Xar => "xar",
            Format::Xz => "xz",
            Format::Zstd => "zstd",
            Format::Bzip2 => "bzip2",
            Format::Lz4 => "lz4",
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
    } else if head.starts_with(&[0xed, 0xab, 0xee, 0xdb])
        && head.get(4).is_some_and(|version| matches!(*version, 3 | 4))
        && head.get(6) == Some(&0)
        && head.get(7).is_some_and(|kind| matches!(*kind, 0 | 1))
    {
        Some(Format::Rpm)
    } else if head.starts_with(b"MSWIM\0\0\0") {
        Some(Format::Wim)
    } else if head.starts_with(b"ITSF") {
        Some(Format::Chm)
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        Some(Format::Xz)
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Some(Format::Zstd)
    } else if head.starts_with(b"BZh") {
        Some(Format::Bzip2)
    } else if head.starts_with(&[0x04, 0x22, 0x4d, 0x18]) {
        Some(Format::Lz4)
    } else if head.starts_with(b"MSCF") {
        Some(Format::Cab)
    } else if head.starts_with(b"!<arch>\n") {
        Some(Format::Ar)
    } else if head.starts_with(b"xar!") {
        Some(Format::Xar)
    } else if head.len() >= 7
        && head[2] == b'-'
        && matches!(head[3], b'l')
        && matches!(head[4], b'h' | b'z')
        && head[6] == b'-'
    {
        Some(Format::Lha)
    } else if head.starts_with(b"070701")
        || head.starts_with(b"070702")
        || head.starts_with(b"070707")
        || head.starts_with(&[0x71, 0xc7])
        || head.starts_with(&[0xc7, 0x71])
    {
        Some(Format::Cpio)
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
        return Ok(Format::Tar);
    }
    r.seek(SeekFrom::Start(32_769))?;
    let mut iso_magic = [0u8; 5];
    let iso_n = read_up_to(r, &mut iso_magic)?;
    r.seek(SeekFrom::Start(0))?;
    if iso_n == 5 && &iso_magic == b"CD001" {
        return Ok(Format::Iso9660);
    }

    // UDIF stores its `koly` signature at the start of the 512-byte trailer.
    if r.seek(SeekFrom::End(0))? >= 512 {
        r.seek(SeekFrom::End(-512))?;
        let mut dmg_magic = [0u8; 4];
        let n = read_up_to(r, &mut dmg_magic)?;
        r.seek(SeekFrom::Start(0))?;
        if n == dmg_magic.len() && &dmg_magic == b"koly" {
            return Ok(Format::Dmg);
        }
    } else {
        r.seek(SeekFrom::Start(0))?;
    }

    Err(Error::UnknownFormat)
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

    #[test]
    fn detects_supported_libarchive_signatures_by_content() {
        assert_eq!(
            sniff(&mut Cursor::new(b"!<arch>\n".to_vec())).ok(),
            Some(Format::Ar)
        );
        assert_eq!(
            sniff(&mut Cursor::new(b"MSCF\0\0\0\0".to_vec())).ok(),
            Some(Format::Cab)
        );
        assert_eq!(
            sniff(&mut Cursor::new(b"0707010000000000".to_vec())).ok(),
            Some(Format::Cpio)
        );
        assert_eq!(
            sniff(&mut Cursor::new(vec![0x71, 0xc7, 0, 0, 0, 0, 0, 0])).ok(),
            Some(Format::Cpio)
        );
        assert_eq!(
            sniff(&mut Cursor::new(
                include_bytes!("../tests/fixtures/longtail_lha_lh0.lzh").to_vec()
            ))
            .ok(),
            Some(Format::Lha)
        );
        assert_eq!(
            sniff(&mut Cursor::new(b"xar!\0\x1c\0\x01".to_vec())).ok(),
            Some(Format::Xar)
        );
        assert_eq!(
            sniff(&mut Cursor::new(vec![0xed, 0xab, 0xee, 0xdb, 3, 0, 0, 0])).ok(),
            Some(Format::Rpm)
        );
        assert_eq!(
            sniff(&mut Cursor::new(vec![0xed, 0xab, 0xee, 0xdb, 0, 0, 0, 0])).ok(),
            None
        );
        let mut iso = vec![0u8; 32_774];
        iso[32_769..32_774].copy_from_slice(b"CD001");
        assert_eq!(sniff(&mut Cursor::new(iso)).ok(), Some(Format::Iso9660));
    }

    #[test]
    fn detects_known_unimplemented_formats_by_magic() {
        assert_eq!(
            sniff(&mut Cursor::new(b"MSWIM\0\0\0".to_vec())).ok(),
            Some(Format::Wim)
        );
        assert_eq!(
            sniff(&mut Cursor::new(b"ITSF\0\0\0\0".to_vec())).ok(),
            Some(Format::Chm)
        );

        let mut dmg = vec![0u8; 1024];
        dmg[512..516].copy_from_slice(b"koly");
        assert_eq!(sniff(&mut Cursor::new(dmg)).ok(), Some(Format::Dmg));
    }

    #[test]
    fn does_not_classify_truncated_format_signatures() {
        assert!(matches!(
            sniff(&mut Cursor::new(b"MSWIM\0\0x".to_vec())),
            Err(Error::UnknownFormat)
        ));
        assert!(matches!(
            sniff(&mut Cursor::new(b"ITS".to_vec())),
            Err(Error::UnknownFormat)
        ));
        assert!(matches!(
            sniff(&mut Cursor::new(vec![0u8; 511])),
            Err(Error::UnknownFormat)
        ));
    }
}
