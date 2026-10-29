//! Best-effort reading without changing the damaged source.

use crate::zip_fmt::map_err;
use rz_core::{CancelToken, Entry, EntryKind, Error, Result};
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Recovers each independent 7z block even when an earlier block cannot be decoded.
pub fn sevenz_entries(
    path: &Path,
    password: Option<&str>,
    threads: usize,
    cancel: &CancelToken,
    visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    issue: &mut dyn FnMut(String, bool),
) -> Result<()> {
    use crate::sevenz_fmt::{describe, map_err};
    use sevenz_rust2::{Archive, BlockDecoder, Password};
    let password = Password::new(password.unwrap_or(""));
    let mut input = Controlled {
        inner: BufReader::new(File::open(path)?),
        cancel,
        until_check: 0,
    };
    let archive = Archive::read(&mut input, &password).map_err(map_err)?;
    let base = archive.files.as_ptr() as usize;
    let stride = std::mem::size_of::<sevenz_rust2::ArchiveEntry>();
    let mut delivered = std::collections::HashSet::new();
    for block_index in 0..archive.blocks.len() {
        cancel.check()?;
        let decoder = BlockDecoder::new(
            rz_core::resolve_threads(threads).min(256) as u32,
            block_index,
            &archive,
            &password,
            &mut input,
        );
        let indexes: Vec<usize> = decoder
            .entries()
            .iter()
            .map(|f| (f as *const _ as usize - base) / stride)
            .collect();
        let mut failure = None;
        let result = decoder.for_each_entries(&mut |f, data| {
            let index = (f as *const _ as usize - base) / stride;
            match visit(&describe(index, f), data) {
                Ok(()) => {
                    delivered.insert(index);
                    Ok(true)
                }
                Err(e) => {
                    failure = Some(e);
                    Ok(false)
                }
            }
        });
        if let Some(e) = failure {
            return Err(e);
        }
        cancel.check()?;
        if let Err(e) = result {
            let e = map_err(e);
            if matches!(e, Error::Password) {
                return Err(e);
            }
            issue(format!("7z block {block_index}: {e}"), false);
            for index in indexes.into_iter().filter(|i| !delivered.contains(i)) {
                issue(
                    format!("{}: damaged solid block", archive.files[index].name),
                    true,
                );
            }
        }
    }
    for (index, file) in archive.files.iter().enumerate() {
        cancel.check()?;
        if archive.stream_map.file_block_index[index].is_none() {
            visit(&describe(index, file), &mut std::io::empty())?;
        }
    }
    Ok(())
}

/// Visits independently readable ZIP entries, or scans local headers when the directory is lost.
/// `visit` must consume and validate each stream before accepting it. Decode errors inside
/// a stream are handled by the visitor; structural errors are sent to `issue`.
pub fn zip_entries(
    path: &Path,
    password: Option<&str>,
    encoding: Option<&str>,
    cancel: &CancelToken,
    visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    issue: &mut dyn FnMut(String, bool),
) -> Result<()> {
    cancel.check()?;
    if let Ok(mut archive) = zip::ZipArchive::new(File::open(path)?) {
        for index in 0..archive.len() {
            cancel.check()?;
            let file = match password {
                Some(pw) => archive.by_index_decrypt(index, pw.as_bytes()),
                None => archive.by_index(index),
            };
            match file {
                Ok(mut file) => {
                    let entry = describe(index, &file, encoding)?;
                    visit(&entry, &mut file)?;
                }
                Err(e) => {
                    let e = map_err(e);
                    if matches!(e, Error::Password) {
                        return Err(e);
                    }
                    issue(format!("#{index}: {e}"), true);
                }
            }
        }
        return Ok(());
    }
    issue(
        "ZIP central directory unavailable; local headers used, attributes may be lost".into(),
        false,
    );
    let len = std::fs::metadata(path)?.len();
    let mut input = Controlled {
        inner: BufReader::new(File::open(path)?),
        cancel,
        until_check: 0,
    };
    let mut window = [0; 4];
    let mut index = 0;
    while input.read(&mut window[3..])? != 0 {
        if window != *b"PK\x03\x04" {
            window.rotate_left(1);
            continue;
        }
        cancel.check()?;
        let start = input.stream_position()?.saturating_sub(4);
        let mut header = [0; 26];
        if input.read_exact(&mut header).is_err() {
            issue(format!("offset {start}: truncated local header"), true);
            break;
        }
        let flags = u16::from_le_bytes([header[2], header[3]]);
        let packed = u32::from_le_bytes(
            header[14..18]
                .try_into()
                .map_err(|_| Error::UnknownFormat)?,
        ) as u64;
        let name_len = u16::from_le_bytes([header[22], header[23]]) as u64;
        let extra_len = u16::from_le_bytes([header[24], header[25]]) as u64;
        let end = start
            .saturating_add(30 + name_len + extra_len)
            .saturating_add(packed);
        // Streaming ZIPs need a directory to resolve descriptors and ZIP64 sizes.
        if flags & 9 != 0 || packed == u32::MAX as u64 || end > len {
            issue(format!("offset {start}: encrypted, descriptor, ZIP64 or truncated entry cannot be recovered without directory"), true);
            input.seek(SeekFrom::Start(start + 4))?;
        } else {
            input.seek(SeekFrom::Start(start))?;
            match zip::read::read_zipfile_from_stream(&mut input) {
                Ok(Some(mut file)) => {
                    let entry = describe(index, &file, encoding)?;
                    visit(&entry, &mut file)?;
                }
                Ok(None) => {}
                Err(e) => issue(format!("offset {start}: {e}"), true),
            };
            cancel.check()?;
            input.seek(SeekFrom::Start(end))?;
        }
        index += 1;
        if index > 1_000_000 {
            return Err(Error::LimitExceeded("too many local ZIP headers".into()));
        }
        window = [0; 4];
    }
    cancel.check()
}

fn describe<R: Read>(
    index: usize,
    file: &zip::read::ZipFile<'_, R>,
    encoding: Option<&str>,
) -> Result<Entry> {
    let path = if let Some(label) = encoding {
        let enc = encoding_rs::Encoding::for_label(label.as_bytes())
            .ok_or_else(|| Error::Unsupported("unknown file name encoding".into()))?;
        enc.decode_without_bom_handling(file.name_raw())
            .0
            .into_owned()
    } else {
        file.name().map_err(map_err)?.to_string()
    };
    let mode = file.unix_mode();
    Ok(Entry {
        index,
        path,
        kind: if file.is_dir() {
            EntryKind::Dir
        } else if mode.is_some_and(|m| m & 0o170000 == 0o120000) {
            EntryKind::Symlink
        } else {
            EntryKind::File
        },
        size: file.size(),
        packed_size: Some(file.compressed_size()),
        modified: None,
        encrypted: file.encrypted(),
        unix_mode: mode,
    })
}

struct Controlled<'a> {
    inner: BufReader<File>,
    cancel: &'a CancelToken,
    until_check: usize,
}
impl Read for Controlled<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.until_check == 0 {
            self.cancel.check().map_err(std::io::Error::other)?;
            self.until_check = 65536;
        }
        let n = self.inner.read(buf)?;
        self.until_check = self.until_check.saturating_sub(n);
        Ok(n)
    }
}
impl Seek for Controlled<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}
