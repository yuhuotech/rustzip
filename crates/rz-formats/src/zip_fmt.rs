use crate::timeutil::{civil_to_unix, unix_to_civil};
use rz_core::{ArchiveReader, Caps, Entry, EntryKind, Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use zip::result::ZipError;
use zip::{DateTime, ZipArchive};

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
    archive: ZipArchive<ZipSource>,
    path: PathBuf,
    volume_paths: Option<Vec<PathBuf>>,
    password: Option<Vec<u8>>,
    encoding: Option<&'static encoding_rs::Encoding>,
}

enum ZipSource {
    File(File),
    Volumes(crate::volumes::MultipartReader),
}

impl Read for ZipSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File(file) => file.read(buf),
            Self::Volumes(reader) => reader.read(buf),
        }
    }
}

impl Seek for ZipSource {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::File(file) => file.seek(from),
            Self::Volumes(reader) => reader.seek(from),
        }
    }
}

/// Picks the character set for entry names that are not valid UTF-8.
///
/// Old Chinese, Japanese and Korean tools write names in the local code page
/// without marking it, so the whole set of such names is sampled together:
/// more text makes the guess far more reliable than one name at a time.
fn guess_encoding(raw_names: &[Vec<u8>]) -> Option<&'static encoding_rs::Encoding> {
    // ISO-2022-JP uses escape bytes that never appear in zip names; UTF-8 is ruled
    // out because only names that failed UTF-8 validation are sampled.
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    let mut any = false;
    for raw in raw_names.iter().filter(|r| std::str::from_utf8(r).is_err()) {
        detector.feed(raw, false);
        any = true;
    }
    any.then(|| {
        detector.feed(&[], true);
        detector.guess(None, chardetng::Utf8Detection::Deny)
    })
}

fn decode_name(raw: &[u8], enc: Option<&'static encoding_rs::Encoding>, forced: bool) -> String {
    match (std::str::from_utf8(raw), enc) {
        (Ok(s), _) if !forced => s.to_string(),
        (_, Some(enc)) => enc.decode_without_bom_handling(raw).0.into_owned(),
        _ => String::from_utf8_lossy(raw).into_owned(),
    }
}

impl ZipReader {
    pub fn new(file: File, path: &Path) -> Result<Self> {
        let archive = ZipArchive::new(ZipSource::File(file)).map_err(map_err)?;
        Ok(Self {
            archive,
            path: path.to_path_buf(),
            volume_paths: None,
            password: None,
            encoding: None,
        })
    }

    pub fn new_volumes(paths: Vec<PathBuf>, path: &Path) -> Result<Self> {
        let source = crate::volumes::MultipartReader::open_zip(&paths)?;
        let archive = ZipArchive::new(ZipSource::Volumes(source)).map_err(map_err)?;
        Ok(Self {
            archive,
            path: path.to_path_buf(),
            volume_paths: Some(paths),
            password: None,
            encoding: None,
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

pub(crate) fn unix_to_dos(t: i64) -> Option<DateTime> {
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
        let mut raw_names = Vec::with_capacity(self.archive.len());
        for index in 0..self.archive.len() {
            let f = self.archive.by_index_raw(index).map_err(map_err)?;
            raw_names.push(f.name_raw().to_vec());
        }
        let forced = self.encoding.is_some();
        let enc = self.encoding.or_else(|| guess_encoding(&raw_names));
        let mut out = Vec::with_capacity(self.archive.len());
        for (index, raw) in raw_names.iter().enumerate() {
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
                path: decode_name(raw, enc, forced),
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

    fn set_encoding(&mut self, label: &str) -> Result<()> {
        let enc = encoding_rs::Encoding::for_label(label.as_bytes())
            .ok_or_else(|| Error::Unsupported(format!("unknown character set {label:?}")))?;
        self.encoding = Some(enc);
        Ok(())
    }

    fn reopen(&self) -> Option<Result<Box<dyn ArchiveReader>>> {
        let open = || -> Result<Box<dyn ArchiveReader>> {
            let mut r = match &self.volume_paths {
                Some(paths) => ZipReader::new_volumes(paths.clone(), &self.path)?,
                None => ZipReader::new(File::open(&self.path)?, &self.path)?,
            };
            r.password.clone_from(&self.password);
            r.encoding = self.encoding;
            Ok(Box::new(r))
        };
        Some(open())
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
