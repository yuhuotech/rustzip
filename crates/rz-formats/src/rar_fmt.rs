//! Read-only RAR support (feature `rar`), built on the unRAR library.
//!
//! Decoded chunks are written through a bounded callback to a private temporary
//! file. RAR is a legacy format here: it is read, never written.

use crate::timeutil::civil_to_unix;
use rz_core::{
    ArchiveReader, CancelToken, Caps, Entry, EntryKind, Error, NoProgress, Progress, ProgressSink,
    Result,
};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

const MAX_COMPAT_ENTRY: u64 = 64 << 20;
const DEFAULT_MAX_TOTAL: u64 = 1 << 40;

fn map_err(e: rz_ffi::unrar::Error) -> Error {
    match e {
        rz_ffi::unrar::Error::BadPassword | rz_ffi::unrar::Error::MissingPassword => {
            Error::Password
        }
        rz_ffi::unrar::Error::Corrupt(code) => Error::Corrupt(format!("unRAR error {code}")),
        rz_ffi::unrar::Error::UnknownFormat => Error::UnknownFormat,
        rz_ffi::unrar::Error::NoMemory => {
            Error::LimitExceeded("unRAR could not allocate decoder memory".into())
        }
        rz_ffi::unrar::Error::Cancelled => Error::Cancelled,
        rz_ffi::unrar::Error::OutputLimit => {
            Error::LimitExceeded("RAR output exceeded the declared or configured size limit".into())
        }
        rz_ffi::unrar::Error::InvalidInput(message) => Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            message,
        )),
        rz_ffi::unrar::Error::Io(error) => Error::Io(error),
        rz_ffi::unrar::Error::Other(code) => {
            Error::Io(std::io::Error::other(format!("unRAR error {code}")))
        }
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

fn describe(index: usize, h: rz_ffi::unrar::EntryInfo) -> Entry {
    Entry {
        index,
        path: h.path.to_string_lossy().replace('\\', "/"),
        kind: if h.is_directory {
            EntryKind::Dir
        } else {
            EntryKind::File
        },
        size: h.size,
        packed_size: Some(h.packed_size),
        modified: dos_to_unix(h.modified),
        encrypted: h.encrypted,
        unix_mode: None,
    }
}

pub struct RarReader {
    path: PathBuf,
    password: Option<Zeroizing<String>>,
    cancel: CancelToken,
    max_total: u64,
    max_ratio: u64,
    ratio_check_above: u64,
}

impl RarReader {
    pub fn new(path: &Path) -> Result<Self> {
        let r = Self {
            path: path.to_path_buf(),
            password: None,
            cancel: CancelToken::new(),
            max_total: DEFAULT_MAX_TOTAL,
            max_ratio: 2000,
            ratio_check_above: 256 << 20,
        };
        // Walk all headers now, so a damaged archive fails at open and not half way
        // through an extraction. Passwords are only checked when data is read.
        let mut archive = r.listing_archive()?;
        while archive.next_entry().map_err(map_err)?.is_some() {
            archive.skip_current().map_err(map_err)?;
        }
        Ok(r)
    }

    fn listing_archive(&self) -> Result<rz_ffi::unrar::Reader> {
        rz_ffi::unrar::Reader::open_for_listing(
            &self.path,
            self.password.as_deref().map(String::as_str),
        )
        .map_err(map_err)
    }

    fn processing_archive(&self) -> Result<rz_ffi::unrar::Reader> {
        rz_ffi::unrar::Reader::open_for_processing(
            &self.path,
            self.password.as_deref().map(String::as_str),
        )
        .map_err(map_err)
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
        self.password = Some(Zeroizing::new(password.to_string()));
        Ok(())
    }

    fn set_read_control(&mut self, cancel: CancelToken, max_total: u64) {
        self.cancel = cancel;
        self.max_total = max_total;
    }

    fn set_ratio_limit(&mut self, max_ratio: u64, check_above: u64) {
        self.max_ratio = max_ratio;
        self.ratio_check_above = check_above;
    }

    fn entries(&mut self) -> Result<Vec<Entry>> {
        self.entries_with_progress(&CancelToken::new(), &mut NoProgress)
    }

    fn entries_with_progress(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<Vec<Entry>> {
        cancel.check()?;
        let mut archive = self.listing_archive()?;
        let mut out = Vec::new();
        let mut bytes_done = 0u64;
        while let Some(header) = archive.next_entry().map_err(map_err)? {
            cancel.check()?;
            archive.skip_current().map_err(map_err)?;
            let entry = describe(out.len(), header);
            bytes_done = bytes_done.saturating_add(entry.size);
            out.push(entry);
            progress.update(Progress {
                files_done: out.len() as u64,
                files_total: 0,
                bytes_done,
                bytes_total: 0,
            });
        }
        Ok(out)
    }

    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        if entry.size > MAX_COMPAT_ENTRY {
            return Err(Error::LimitExceeded(format!(
                "RAR entry is larger than the {MAX_COMPAT_ENTRY}-byte compatibility read limit"
            )));
        }
        let wanted = entry.index;
        let selected = [wanted];
        let mut found = None;
        self.for_each_selected(Some(&selected), &mut |e, data| {
            if e.index == wanted {
                let mut buf = Vec::with_capacity(e.size as usize);
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
        self.for_each_selected(None, visit)
    }

    fn for_each_selected(
        &mut self,
        selected: Option<&[usize]>,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let selected = selected.map(|indexes| indexes.iter().copied().collect::<HashSet<_>>());
        let mut archive = self.processing_archive()?;
        let scratch = tempfile::tempdir()?;
        let tmp = scratch.path().join("entry");
        let mut index = 0usize;
        let mut bytes_written = 0u64;

        while let Some(header) = archive.next_entry().map_err(map_err)? {
            self.cancel.check()?;
            let entry = describe(index, header);
            index += 1;
            let chosen = selected
                .as_ref()
                .is_none_or(|indexes| indexes.contains(&entry.index));

            if entry.size > self.ratio_check_above {
                let allowed = entry
                    .packed_size
                    .unwrap_or(0)
                    .max(1)
                    .saturating_mul(self.max_ratio);
                if entry.size > allowed {
                    return Err(Error::LimitExceeded(format!(
                        "{}: compression ratio too high",
                        entry.path
                    )));
                }
            }

            if entry.kind == EntryKind::Dir {
                archive.skip_current().map_err(map_err)?;
                if chosen {
                    visit(&entry, &mut std::io::empty())?;
                }
                continue;
            }

            let cancelled = || self.cancel.check().is_err();
            if !chosen {
                archive
                    .test_current(entry.size, &cancelled)
                    .map_err(map_err)?;
                continue;
            }

            let remaining = self.max_total.saturating_sub(bytes_written);
            if entry.size > remaining {
                return Err(Error::LimitExceeded(format!(
                    "RAR selected output exceeds the configured {}-byte total limit",
                    self.max_total
                )));
            }

            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)?;
            let decoded = archive
                .extract_current_to(&mut file, remaining.min(entry.size), &cancelled)
                .map_err(map_err)?;
            if decoded != entry.size {
                return Err(Error::Corrupt(format!(
                    "{}: RAR entry size does not match its header",
                    entry.path
                )));
            }
            file.flush()?;
            file.seek(SeekFrom::Start(0))?;
            let result = visit(&entry, &mut file);
            drop(file);
            std::fs::remove_file(&tmp)?;
            result?;
            bytes_written = bytes_written.saturating_add(decoded);
        }
        Ok(())
    }

    fn set_encoding(&mut self, _label: &str) -> Result<()> {
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
