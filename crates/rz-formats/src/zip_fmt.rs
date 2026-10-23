use crate::timeutil::{civil_to_unix, unix_to_civil};
use rz_core::{
    ArchiveReader, ArchiveWriter, Caps, CreateOptions, Entry, EntryKind, Error, NewEntry, Result,
};
use std::fs::File;
use std::io::{BufWriter, Read};
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, DateTime, ZipArchive, ZipWriter};

pub(crate) fn map_err(e: ZipError) -> Error {
    match e {
        ZipError::Io(e) => Error::Io(e),
        ZipError::InvalidPassword => Error::Password,
        ZipError::UnsupportedArchive(m) if m.contains("assword") => Error::Password,
        ZipError::UnsupportedArchive(m) => Error::Unsupported(m.to_string()),
        other => Error::Corrupt(other.to_string()),
    }
}

pub struct ZipReader {
    archive: ZipArchive<File>,
    password: Option<Vec<u8>>,
}

impl ZipReader {
    pub fn new(file: File) -> Result<Self> {
        let archive = ZipArchive::new(file).map_err(map_err)?;
        Ok(Self {
            archive,
            password: None,
        })
    }
}

fn dos_to_unix(dt: DateTime) -> i64 {
    civil_to_unix(
        i64::from(dt.year()),
        i64::from(dt.month()),
        i64::from(dt.day()),
        i64::from(dt.hour()),
        i64::from(dt.minute()),
        i64::from(dt.second()),
    )
}

fn unix_to_dos(t: i64) -> Option<DateTime> {
    let (y, mo, d, h, mi, s) = unix_to_civil(t);
    // DOS time only covers 1980..=2107 with two-second resolution.
    let y = u16::try_from(y)
        .ok()
        .filter(|y| (1980..=2107).contains(y))?;
    DateTime::from_date_and_time(y, mo as u8, d as u8, h as u8, mi as u8, (s & !1) as u8).ok()
}

impl ArchiveReader for ZipReader {
    fn format_name(&self) -> &'static str {
        "zip"
    }

    fn caps(&self) -> Caps {
        Caps {
            random_access: true,
            solid: false,
            writable: true,
        }
    }

    fn entries(&mut self) -> Result<Vec<Entry>> {
        let mut out = Vec::with_capacity(self.archive.len());
        for index in 0..self.archive.len() {
            let f = self.archive.by_index_raw(index).map_err(map_err)?;
            let mode = f.unix_mode();
            let kind = if f.is_dir() {
                EntryKind::Dir
            } else if mode.is_some_and(|m| m & 0o170000 == 0o120000) {
                EntryKind::Symlink
            } else {
                EntryKind::File
            };
            out.push(Entry {
                index,
                path: f.name().to_string(),
                kind,
                size: f.size(),
                packed_size: Some(f.compressed_size()),
                modified: f.last_modified().map(dos_to_unix),
                encrypted: f.encrypted(),
                unix_mode: mode,
            });
        }
        Ok(out)
    }

    fn set_password(&mut self, password: &str) -> Result<()> {
        self.password = Some(password.as_bytes().to_vec());
        Ok(())
    }

    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        let f = match (&self.password, entry.encrypted) {
            (Some(pw), true) => self.archive.by_index_decrypt(entry.index, pw),
            _ => self.archive.by_index(entry.index),
        }
        .map_err(map_err)?;
        Ok(Box::new(f))
    }
}

pub struct ZipOut {
    writer: ZipWriter<BufWriter<File>>,
    level: u32,
    password: Option<String>,
}

impl ZipOut {
    pub fn new(file: File, opts: &CreateOptions) -> Self {
        Self {
            writer: ZipWriter::new(BufWriter::with_capacity(256 * 1024, file)),
            level: opts.level.min(9),
            password: opts.password.clone().filter(|p| !p.is_empty()),
        }
    }

    fn base_options(&self, e: &NewEntry) -> SimpleFileOptions {
        let mut o = SimpleFileOptions::default().large_file(e.size >= u64::from(u32::MAX));
        o = if self.level == 0 {
            o.compression_method(CompressionMethod::Stored)
        } else {
            o.compression_method(CompressionMethod::Deflated)
                .compression_level(Some(i64::from(self.level)))
        };
        if let Some(mode) = e.unix_mode {
            o = o.unix_permissions(mode & 0o777);
        }
        if let Some(dt) = e.modified.and_then(unix_to_dos) {
            o = o.last_modified_time(dt);
        }
        o
    }
}

impl ArchiveWriter for ZipOut {
    fn add_dir(&mut self, e: &NewEntry) -> Result<()> {
        let opts = self.base_options(e);
        self.writer
            .add_directory(e.name.as_str(), opts)
            .map_err(map_err)
    }

    fn add_file(&mut self, e: &NewEntry, src: &mut dyn Read) -> Result<()> {
        let opts = self.base_options(e);
        match &self.password {
            Some(pw) => self.writer.start_file(
                e.name.as_str(),
                opts.with_aes_encryption(AesMode::Aes256, pw),
            ),
            None => self.writer.start_file(e.name.as_str(), opts),
        }
        .map_err(map_err)?;
        std::io::copy(src, &mut self.writer)?;
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<()> {
        let mut inner = self.writer.finish().map_err(map_err)?;
        std::io::Write::flush(&mut inner)?;
        inner.get_ref().sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_time_round_trips_with_two_second_resolution() {
        let t = civil_to_unix(2024, 5, 17, 9, 30, 41);
        let dt = unix_to_dos(t);
        assert_eq!(dt.map(dos_to_unix), Some(t - 1));
    }

    #[test]
    fn dates_outside_dos_range_are_dropped() {
        assert!(unix_to_dos(0).is_none());
        assert!(unix_to_dos(civil_to_unix(2200, 1, 1, 0, 0, 0)).is_none());
    }
}
