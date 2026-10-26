use rz_core::{ArchiveReader, Caps, Entry, EntryKind, Error, Result};
use rz_ffi::libarchive::{EntryInfo, Format as NativeFormat, Reader as NativeReader};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct LibarchiveReader {
    path: PathBuf,
    format: NativeFormat,
}

impl LibarchiveReader {
    pub fn new(path: &Path, format: NativeFormat) -> Result<Self> {
        let reader = Self {
            path: path.to_path_buf(),
            format,
        };
        let mut probe = reader.open_native()?;
        let _ = probe.next_entry().map_err(Error::Corrupt)?;
        Ok(reader)
    }

    fn open_native(&self) -> Result<NativeReader> {
        File::open(&self.path)?;
        NativeReader::open(&self.path, &[self.format], None).map_err(Error::Corrupt)
    }

    fn entry(index: usize, info: EntryInfo) -> Entry {
        Entry {
            index,
            path: info.pathname.replace('\\', "/"),
            kind: match info.kind {
                rz_ffi::libarchive::EntryKind::File => EntryKind::File,
                rz_ffi::libarchive::EntryKind::Directory => EntryKind::Dir,
                rz_ffi::libarchive::EntryKind::Symlink => EntryKind::Symlink,
            },
            size: info.size.unwrap_or(0),
            packed_size: None,
            modified: info.modified,
            encrypted: false,
            unix_mode: Some(info.mode & 0o7777),
        }
    }

    fn name(&self) -> &'static str {
        match self.format {
            NativeFormat::Ar => "ar",
            NativeFormat::Cab => "cab",
            NativeFormat::Cpio => "cpio",
            NativeFormat::Iso9660 => "iso9660",
        }
    }
}

impl ArchiveReader for LibarchiveReader {
    fn format_name(&self) -> &'static str {
        self.name()
    }

    fn caps(&self) -> Caps {
        Caps {
            random_access: false,
            solid: false,
            writable: false,
        }
    }

    fn entries(&mut self) -> Result<Vec<Entry>> {
        let mut reader = self.open_native()?;
        let mut entries = Vec::new();
        while let Some(info) = reader.next_entry().map_err(Error::Corrupt)? {
            entries.push(Self::entry(entries.len(), info));
        }
        Ok(entries)
    }

    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        let mut reader = self.open_native()?;
        for index in 0..=entry.index {
            let _found = reader
                .next_entry()
                .map_err(Error::Corrupt)?
                .ok_or_else(|| Error::Corrupt("archive entry index is out of range".into()))?;
            if index < entry.index {
                reader.skip_data().map_err(Error::Corrupt)?;
            } else {
                return Ok(Box::new(EntryReader(reader)));
            }
        }
        Err(Error::Corrupt("archive entry index is out of range".into()))
    }

    fn reopen(&self) -> Option<Result<Box<dyn ArchiveReader>>> {
        Some(Self::new(&self.path, self.format).map(|reader| Box::new(reader) as _))
    }

    fn set_encoding(&mut self, _label: &str) -> Result<()> {
        Ok(())
    }

    fn for_each(
        &mut self,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let mut reader = self.open_native()?;
        let mut index = 0usize;
        while let Some(info) = reader.next_entry().map_err(Error::Corrupt)? {
            let entry = Self::entry(index, info);
            {
                let mut data = BorrowedEntryReader(&mut reader);
                visit(&entry, &mut data)?;
            }
            reader.skip_data().map_err(Error::Corrupt)?;
            index = index
                .checked_add(1)
                .ok_or_else(|| Error::LimitExceeded("archive contains too many entries".into()))?;
        }
        Ok(())
    }
}

struct EntryReader(NativeReader);

impl Read for EntryReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.read_data(buffer).map_err(std::io::Error::other)
    }
}

struct BorrowedEntryReader<'a>(&'a mut NativeReader);

impl Read for BorrowedEntryReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.read_data(buffer).map_err(std::io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn ar_file(name: &str, body: &[u8]) -> Vec<u8> {
        let mut archive = b"!<arch>\n".to_vec();
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            format!("{name}/"),
            "0",
            "0",
            "0",
            "100644",
            body.len()
        );
        archive.extend_from_slice(header.as_bytes());
        archive.extend_from_slice(body);
        if !body.len().is_multiple_of(2) {
            archive.push(b'\n');
        }
        archive
    }

    fn cpio_header(name: &[u8], body: &[u8], mode: u32) -> Vec<u8> {
        let values = [
            1,
            mode,
            0,
            0,
            1,
            0,
            body.len() as u32,
            0,
            0,
            0,
            0,
            name.len() as u32 + 1,
            0,
        ];
        let mut out = b"070701".to_vec();
        for value in values {
            out.extend_from_slice(format!("{value:08x}").as_bytes());
        }
        out.extend_from_slice(name);
        out.push(0);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out.extend_from_slice(body);
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out
    }

    fn cab_file(name: &[u8], body: &[u8]) -> Vec<u8> {
        let name_len = name.len() + 1;
        let file_header_len = 16 + name_len;
        let data_offset = 36 + 8 + file_header_len;
        let cabinet_size = data_offset + 8 + body.len();
        let mut out = b"MSCF".to_vec();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(cabinet_size as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&44u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&[3, 1]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(data_offset as u32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x20u16.to_le_bytes());
        out.extend_from_slice(name);
        out.push(0);
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(body.len() as u16).to_le_bytes());
        out.extend_from_slice(&(body.len() as u16).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn reads_ar_members_and_streams_their_contents() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sample.ar");
        std::fs::write(&path, ar_file("hello.txt", b"hello")).unwrap();

        let mut archive = crate::open(&path).unwrap();
        let entries = archive.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "hello.txt");
        assert_eq!(entries[0].size, 5);
        let mut data = Vec::new();
        archive
            .open(&entries[0])
            .unwrap()
            .read_to_end(&mut data)
            .unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn reads_cpio_and_never_registers_the_rar_format() {
        let temp = tempfile::tempdir().unwrap();
        let cpio = temp.path().join("sample.cpio");
        let mut bytes = cpio_header(b"hello.txt", b"hello", 0o100644);
        bytes.extend(cpio_header(b"TRAILER!!!", b"", 0));
        std::fs::write(&cpio, bytes).unwrap();
        let mut archive = crate::open(&cpio).unwrap();
        assert_eq!(archive.entries().unwrap()[0].path, "hello.txt");

        let rar = temp.path().join("misidentified.rar");
        std::fs::write(&rar, b"Rar!\x1a\x07\x00\0").unwrap();
        assert!(crate::open(&rar).is_err());
    }

    #[test]
    fn reads_uncompressed_cab_members() {
        let temp = tempfile::tempdir().unwrap();
        let cab = temp.path().join("sample.cab");
        std::fs::write(&cab, cab_file(b"hello.txt", b"hello")).unwrap();
        let mut archive = crate::open(&cab).unwrap();
        let entries = archive.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "hello.txt");
        let mut data = Vec::new();
        archive
            .open(&entries[0])
            .unwrap()
            .read_to_end(&mut data)
            .unwrap();
        assert_eq!(data, b"hello");
    }

    #[test]
    fn sequential_iteration_skips_unread_data_before_the_next_header() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("two-members.ar");
        let mut bytes = ar_file("first.txt", b"first member");
        let second = ar_file("second.txt", b"second member");
        bytes.extend_from_slice(&second[8..]);
        std::fs::write(&path, bytes).unwrap();

        let mut archive = crate::open(&path).unwrap();
        let mut visited = Vec::new();
        archive
            .for_each(&mut |entry, data| {
                visited.push(entry.path.clone());
                let mut prefix = [0; 1];
                assert_eq!(data.read(&mut prefix)?, 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(visited, ["first.txt", "second.txt"]);
    }

    #[test]
    fn reads_iso9660_from_a_gzip_compressed_test_fixture() {
        let temp = tempfile::tempdir().unwrap();
        let iso = temp.path().join("sample.iso");
        let compressed = include_bytes!("../tests/fixtures/longtail_iso9660.iso.gz");
        let mut decoder = flate2::read::GzDecoder::new(compressed.as_slice());
        let mut bytes = Vec::new();
        decoder.read_to_end(&mut bytes).unwrap();
        std::fs::write(&iso, bytes).unwrap();

        let mut archive = crate::open(&iso).unwrap();
        let entries = archive.entries().unwrap();
        assert!(entries.iter().any(|entry| entry.path == "HELLO.TXT"));
        let hello = entries
            .iter()
            .find(|entry| entry.path == "HELLO.TXT")
            .unwrap();
        let mut data = Vec::new();
        archive.open(hello).unwrap().read_to_end(&mut data).unwrap();
        assert_eq!(data, b"RustZIP ISO fixture\n");
    }

    #[test]
    fn truncated_cab_cpio_and_iso_signatures_fail_cleanly() {
        let temp = tempfile::tempdir().unwrap();
        let mut iso = vec![0u8; 32_774];
        iso[32_769..32_774].copy_from_slice(b"CD001");
        for (name, bytes) in [
            ("broken.cab", b"MSCF\0\0".to_vec()),
            ("broken.cpio", b"07070100".to_vec()),
            ("broken.iso", iso),
        ] {
            let path = temp.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            assert!(crate::open(&path).is_err(), "{name}");
        }
    }
}
