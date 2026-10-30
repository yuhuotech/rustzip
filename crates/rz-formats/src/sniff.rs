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
    Nsis,
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
            Format::Nsis => "nsis",
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
    if sniff_nsis(r)? {
        return Ok(Format::Nsis);
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

const NSIS_FIRST_HEADER_SIZE: usize = 28;
const NSIS_SCAN_LIMIT: u64 = 8 * 1024 * 1024;
const NSIS_SCAN_CHUNK: usize = 64 * 1024;
const MAX_PE_SECTIONS: u16 = 96;

/// Checks a bounded PE overlay for the NSIS FirstHeader signature.
///
/// Only fixed-size headers are read. Declared NSIS lengths are validated as
/// signed values and never used to allocate or seek. The scan limit bounds the
/// work on arbitrary PE files with large overlays.
fn sniff_nsis<R: Read + Seek>(r: &mut R) -> Result<bool> {
    let found = sniff_nsis_inner(r);
    r.seek(SeekFrom::Start(0))?;
    found
}

fn sniff_nsis_inner<R: Read + Seek>(r: &mut R) -> Result<bool> {
    let file_len = r.seek(SeekFrom::End(0))?;
    if file_len < 64 {
        return Ok(false);
    }

    let mut dos = [0; 64];
    if !read_at(r, 0, &mut dos)? || &dos[..2] != b"MZ" {
        return Ok(false);
    }
    let pe_offset = u64::from(u32::from_le_bytes([
        dos[0x3c], dos[0x3d], dos[0x3e], dos[0x3f],
    ]));

    let mut pe_header = [0; 24];
    if pe_offset
        .checked_add(pe_header.len() as u64)
        .is_none_or(|end| end > file_len)
        || !read_at(r, pe_offset, &mut pe_header)?
        || &pe_header[..4] != b"PE\0\0"
    {
        return Ok(false);
    }

    let section_count = u16::from_le_bytes([pe_header[6], pe_header[7]]);
    let optional_size = u64::from(u16::from_le_bytes([pe_header[20], pe_header[21]]));
    if section_count == 0 || section_count > MAX_PE_SECTIONS || optional_size < 2 {
        return Ok(false);
    }

    let optional_start = pe_offset + pe_header.len() as u64;
    let table_start = optional_start
        .checked_add(optional_size)
        .ok_or_else(|| Error::Corrupt("PE section table offset overflow".into()))?;
    let table_len = u64::from(section_count) * 40;
    let table_end = table_start
        .checked_add(table_len)
        .ok_or_else(|| Error::Corrupt("PE section table length overflow".into()))?;
    if table_end > file_len {
        return Ok(false);
    }

    let mut optional_magic = [0; 2];
    if !read_at(r, optional_start, &mut optional_magic)?
        || !matches!(u16::from_le_bytes(optional_magic), 0x10b | 0x20b)
    {
        return Ok(false);
    }

    let mut overlay_end = None::<u64>;
    for index in 0..section_count {
        let offset = table_start + u64::from(index) * 40;
        let mut section = [0; 40];
        if !read_at(r, offset, &mut section)? {
            return Ok(false);
        }
        let raw_size = u64::from(u32::from_le_bytes([
            section[16],
            section[17],
            section[18],
            section[19],
        ]));
        let raw_start = u64::from(u32::from_le_bytes([
            section[20],
            section[21],
            section[22],
            section[23],
        ]));
        if raw_size == 0 || raw_start < table_end {
            continue;
        }
        if let Some(end) = raw_start
            .checked_add(raw_size)
            .filter(|end| *end <= file_len)
        {
            overlay_end = Some(overlay_end.map_or(end, |current| current.max(end)));
        }
    }

    let Some(overlay_start) = overlay_end.map(|end| end.max(table_end)) else {
        return Ok(false);
    };
    if overlay_start >= file_len {
        return Ok(false);
    }

    scan_nsis_overlay(r, overlay_start, file_len - overlay_start)
}

fn scan_nsis_overlay<R: Read + Seek>(r: &mut R, start: u64, length: u64) -> Result<bool> {
    let scan_len = length.min(NSIS_SCAN_LIMIT);
    r.seek(SeekFrom::Start(start))?;

    let mut buffer = [0u8; NSIS_SCAN_CHUNK + NSIS_FIRST_HEADER_SIZE - 1];
    let mut carry = 0usize;
    let mut consumed = 0u64;

    while consumed < scan_len {
        let request = (scan_len - consumed).min(NSIS_SCAN_CHUNK as u64) as usize;
        let read = read_up_to(r, &mut buffer[carry..carry + request])?;
        if read == 0 {
            break;
        }

        let available = carry + read;
        if available >= NSIS_FIRST_HEADER_SIZE
            && buffer[..available]
                .windows(NSIS_FIRST_HEADER_SIZE)
                .any(is_nsis_first_header)
        {
            return Ok(true);
        }

        carry = available.min(NSIS_FIRST_HEADER_SIZE - 1);
        buffer.copy_within(available - carry..available, 0);
        consumed += read as u64;
    }

    Ok(false)
}

fn is_nsis_first_header(header: &[u8]) -> bool {
    if header.len() < NSIS_FIRST_HEADER_SIZE {
        return false;
    }

    let flags = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let signature = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    let magic = [
        u32::from_le_bytes([header[8], header[9], header[10], header[11]]),
        u32::from_le_bytes([header[12], header[13], header[14], header[15]]),
        u32::from_le_bytes([header[16], header[17], header[18], header[19]]),
    ];
    let standard = magic == [0x6c6c_754e, 0x7466_6f73, 0x7473_6e49];
    let legacy = magic == [0x7369_736e, 0x7473_6e69, 0x006c_6c61];
    let header_length = i32::from_le_bytes([header[20], header[21], header[22], header[23]]);
    let data_length = i32::from_le_bytes([header[24], header[25], header[26], header[27]]);

    flags & !0x0f == 0
        && matches!(signature, 0xdead_beef | 0xdead_beed)
        && (standard || legacy)
        && header_length > 0
        && data_length >= NSIS_FIRST_HEADER_SIZE as i32
}

fn read_at<R: Read + Seek>(r: &mut R, offset: u64, buf: &mut [u8]) -> Result<bool> {
    r.seek(SeekFrom::Start(offset))?;
    Ok(read_up_to(r, buf)? == buf.len())
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

    pub(crate) fn minimal_nsis_pe(padding: usize, legacy: bool) -> Vec<u8> {
        const PE_OFFSET: usize = 0x80;
        const OPTIONAL_SIZE: usize = 0xe0;
        const RAW_START: usize = 0x200;
        const RAW_SIZE: usize = 0x200;

        let section_table = PE_OFFSET + 24 + OPTIONAL_SIZE;
        let overlay_start = RAW_START + RAW_SIZE;
        let header_start = overlay_start + padding;
        let mut bytes = vec![0u8; header_start + 512];

        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&(PE_OFFSET as u32).to_le_bytes());
        bytes[PE_OFFSET..PE_OFFSET + 4].copy_from_slice(b"PE\0\0");
        bytes[PE_OFFSET + 4..PE_OFFSET + 6].copy_from_slice(&0x014cu16.to_le_bytes());
        bytes[PE_OFFSET + 6..PE_OFFSET + 8].copy_from_slice(&1u16.to_le_bytes());
        bytes[PE_OFFSET + 20..PE_OFFSET + 22]
            .copy_from_slice(&(OPTIONAL_SIZE as u16).to_le_bytes());

        let optional_start = PE_OFFSET + 24;
        bytes[optional_start..optional_start + 2].copy_from_slice(&0x010bu16.to_le_bytes());
        bytes[section_table + 16..section_table + 20]
            .copy_from_slice(&(RAW_SIZE as u32).to_le_bytes());
        bytes[section_table + 20..section_table + 24]
            .copy_from_slice(&(RAW_START as u32).to_le_bytes());

        let magic: [u32; 3] = if legacy {
            [0x7369_736e, 0x7473_6e69, 0x006c_6c61]
        } else {
            [0x6c6c_754e, 0x7466_6f73, 0x7473_6e49]
        };
        let header = &mut bytes[header_start..header_start + NSIS_FIRST_HEADER_SIZE];
        header[0..4].copy_from_slice(&0u32.to_le_bytes());
        header[4..8].copy_from_slice(&0xdead_beefu32.to_le_bytes());
        for (index, value) in magic.into_iter().enumerate() {
            let start = 8 + index * 4;
            header[start..start + 4].copy_from_slice(&value.to_le_bytes());
        }
        header[20..24].copy_from_slice(&1024i32.to_le_bytes());
        header[24..28].copy_from_slice(&2048i32.to_le_bytes());
        bytes
    }

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

    #[test]
    fn detects_nsis_only_from_a_structural_pe_overlay_signature() {
        for (padding, legacy) in [(0, false), (512, false), (137, false), (0, true)] {
            let mut file = Cursor::new(minimal_nsis_pe(padding, legacy));
            assert_eq!(sniff(&mut file).ok(), Some(Format::Nsis));
            assert_eq!(file.position(), 0);
        }

        let mut ordinary_exe = minimal_nsis_pe(0, false);
        let overlay_start = 0x400;
        ordinary_exe[overlay_start + 8..overlay_start + 20].fill(0);
        assert!(matches!(
            sniff(&mut Cursor::new(ordinary_exe)),
            Err(Error::UnknownFormat)
        ));
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    fn detected_nsis_is_reported_as_known_but_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installer.exe");
        std::fs::write(&path, minimal_nsis_pe(0, false)).unwrap();

        assert!(matches!(
            crate::open(&path),
            Err(Error::Unsupported(message)) if message.contains("NSIS installer detected")
        ));
    }
}
