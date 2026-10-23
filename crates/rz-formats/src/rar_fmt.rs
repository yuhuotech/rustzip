//! Read-only RAR support (feature `rar`), built on the unRAR library.
//!
//! The library decodes into files, so each entry goes through a private temporary
//! directory. RAR is a legacy format here: it is read, never written.

use crate::timeutil::civil_to_unix;
use rz_core::{ArchiveReader, Caps, Entry, EntryKind, Error, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use unrar::error::{Code, UnrarError};
use unrar::{Archive, FileHeader};

fn map_err(e: UnrarError) -> Error {
    match e.code {
        Code::BadPassword | Code::MissingPassword => Error::Password,
        Code::BadData | Code::BadArchive => Error::Corrupt(e.to_string()),
        Code::UnknownFormat => Error::UnknownFormat,
        Code::NoMemory => Error::LimitExceeded("out of memory".into()),
        other => Error::Io(std::io::Error::other(format!("unrar: {other:?}"))),
    }
}

/// RAR stores DOS date and time: date in the high half, time in the low half.
fn dos_to_unix(t: u32) -> Option<i64> {
    let (date, time) = (t >> 16, t & 0xffff);
    let (day, month, year) = (date & 31, (date >> 5) & 15, 1980 + (date >> 9));
    let (sec, min, hour) = ((time & 31) * 2, (time >> 5) & 63, time >> 11);
    if day == 0 || month == 0 || month > 12 || hour > 23 {
        return None;
    }
    Some(civil_to_unix(
        i64::from(year),
        i64::from(month),
        i64::from(day),
        i64::from(hour),
        i64::from(min),
        i64::from(sec),
    ))
}

fn describe(index: usize, h: &FileHeader) -> Entry {
    Entry {
        index,
        path: h.filename.to_string_lossy().replace('\\', "/"),
        kind: if h.is_directory() {
            EntryKind::Dir
        } else {
            EntryKind::File
        },
        size: h.unpacked_size,
        packed_size: None,
        modified: dos_to_unix(h.file_time),
        encrypted: h.is_encrypted(),
        unix_mode: None,
    }
}

pub struct RarReader {
    path: PathBuf,
    password: Option<String>,
}

impl RarReader {
    pub fn new(path: &Path) -> Result<Self> {
        let r = Self {
            path: path.to_path_buf(),
            password: None,
        };
        // Walk all headers now, so a damaged archive fails at open and not half way
        // through an extraction. Passwords are only checked when data is read.
        for header in r.archive().open_for_listing().map_err(map_err)? {
            header.map_err(map_err)?;
        }
        Ok(r)
    }

    fn archive(&self) -> Archive<'_> {
        match &self.password {
            Some(pw) => Archive::with_password(&self.path, pw),
            None => Archive::new(&self.path),
        }
    }
}

impl ArchiveReader for RarReader {
    fn format_name(&self) -> &'static str {
        "rar"
    }

    fn caps(&self) -> Caps {
        Caps {
            random_access: false,
            solid: true,
            writable: false,
        }
    }

    fn set_password(&mut self, password: &str) -> Result<()> {
        self.password = Some(password.to_string());
        Ok(())
    }

    fn entries(&mut self) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        for (index, header) in self
            .archive()
            .open_for_listing()
            .map_err(map_err)?
            .enumerate()
        {
            out.push(describe(index, &header.map_err(map_err)?));
        }
        Ok(out)
    }

    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        let wanted = entry.index;
        let mut found = None;
        self.for_each(&mut |e, data| {
            if e.index == wanted {
                let mut buf = Vec::new();
                data.read_to_end(&mut buf)?;
                found = Some(buf);
            }
            Ok(())
        })?;
        match found {
            Some(buf) => Ok(Box::new(std::io::Cursor::new(buf))),
            None => Err(Error::Corrupt(format!("entry {wanted} not found"))),
        }
    }

    fn for_each(
        &mut self,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let tmp = scratch.path().join("entry");
        let mut cursor = self.archive().open_for_processing().map_err(map_err)?;
        let mut index = 0usize;
        while let Some(header) = cursor.read_header().map_err(map_err)? {
            let entry = describe(index, header.entry());
            index += 1;
            if entry.kind == EntryKind::Dir {
                visit(&entry, &mut std::io::empty())?;
                cursor = header.skip().map_err(map_err)?;
                continue;
            }
            cursor = header.extract_to(&tmp).map_err(map_err)?;
            let mut file = std::fs::File::open(&tmp)?;
            let result = visit(&entry, &mut file);
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            result?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dos_time_decodes() {
        // 2016-08-20 12:34:56 as DOS date/time.
        let date = ((2016 - 1980) << 9) | (8 << 5) | 20;
        let time = (12 << 11) | (34 << 5) | (56 / 2);
        assert_eq!(
            dos_to_unix((date << 16) | time),
            Some(civil_to_unix(2016, 8, 20, 12, 34, 56))
        );
        assert_eq!(dos_to_unix(0), None);
    }
}
