use flate2::read::DeflateDecoder;
use rz_core::{
    ArchiveReader, CancelToken, Caps, Entry, EntryKind, Error, NoProgress, Progress, ProgressSink,
    Result,
};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAX_METADATA: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const CHUNK: usize = 64 * 1024;

#[derive(Clone)]
struct Record {
    entry: Entry,
    offset: u64,
    packed: u64,
    compressed: bool,
    solid: bool,
}

pub struct NsisReader {
    path: PathBuf,
    records: Option<Vec<Record>>,
    cancel: CancelToken,
    max_total: u64,
    max_ratio: u64,
    ratio_check_above: u64,
    emitted: Arc<AtomicU64>,
    solid: Option<File>,
}

fn corrupt() -> Error {
    Error::Corrupt("NSIS header or data range is invalid".into())
}

fn unsupported() -> Error {
    Error::Unsupported(
        "NSIS: only modern stored / Deflate / standard LZMA installers are supported".into(),
    )
}

fn word(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset.checked_add(4).ok_or_else(corrupt)?)
        .ok_or_else(corrupt)?;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| corrupt())?))
}

struct Controlled<R> {
    reader: R,
    cancel: CancelToken,
    total: Arc<AtomicU64>,
    limit: u64,
}

impl<R: Read> Read for Controlled<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        let request = buffer.len().min(CHUNK);
        let count = self.reader.read(&mut buffer[..request])?;
        let old = self.total.fetch_add(count as u64, Ordering::Relaxed);
        if old
            .checked_add(count as u64)
            .is_none_or(|total| total > self.limit)
        {
            return Err(io::Error::other("NSIS output limit exceeded"));
        }
        Ok(count)
    }
}

impl NsisReader {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            records: None,
            cancel: CancelToken::new(),
            max_total: 1 << 40,
            max_ratio: 1000,
            ratio_check_above: 1 << 20,
            emitted: Arc::new(AtomicU64::new(0)),
            solid: None,
        }
    }

    fn stream(&self, record: &Record) -> Result<Box<dyn Read + Send>> {
        let mut file = if record.solid {
            self.solid.as_ref().ok_or_else(corrupt)?.try_clone()?
        } else {
            File::open(&self.path)?
        };
        file.seek(SeekFrom::Start(record.offset))?;
        let mut input = Controlled {
            reader: file.take(record.packed),
            cancel: self.cancel.clone(),
            total: Arc::new(AtomicU64::new(0)),
            limit: record.packed,
        };
        if record.compressed {
            let mut prefix = [0u8; 5];
            input.read_exact(&mut prefix)?;
            if prefix[0] == 0x5d {
                let dictionary = u32::from_le_bytes([prefix[1], prefix[2], prefix[3], prefix[4]]);
                if dictionary > 16 * 1024 * 1024 {
                    return Err(Error::LimitExceeded("NSIS LZMA dictionary".into()));
                }
                let size = if record.entry.size == 0 {
                    u64::MAX
                } else {
                    record.entry.size
                };
                let decoder = lzma_rust2::LzmaReader::new_with_props(
                    input, size, prefix[0], dictionary, None,
                )
                .map_err(|_| corrupt())?;
                Ok(Box::new(decoder))
            } else if prefix[0] == 0x31 {
                Ok(Box::new(rustzip_nsis_bzip2::Decoder::new(
                    std::io::Cursor::new(prefix).chain(input),
                    self.cancel.clone(),
                )))
            } else {
                Ok(Box::new(DeflateDecoder::new(
                    std::io::Cursor::new(prefix).chain(input),
                )))
            }
        } else {
            Ok(Box::new(input))
        }
    }

    fn header(&self, record: &Record, expected: usize, cancel: &CancelToken) -> Result<Vec<u8>> {
        let mut input = self.stream(record)?;
        let mut header = Vec::new();
        let mut buffer = [0u8; CHUNK];
        loop {
            cancel.check()?;
            let count = input.read(&mut buffer).map_err(|_| unsupported())?;
            if count == 0 {
                break;
            }
            if header.len() + count > expected {
                return Err(unsupported());
            }
            header.extend_from_slice(&buffer[..count]);
        }
        if header.len() != expected {
            return Err(unsupported());
        }
        Ok(header)
    }

    fn solid_header(
        &self,
        mut source: Record,
        expected: usize,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<(Vec<u8>, File, u64)> {
        source.entry.size = 0;
        source.compressed = true;
        let mut input = self.stream(&source)?;
        let mut staging = tempfile::tempfile()?;
        let limit = self
            .max_total
            .saturating_add(MAX_METADATA as u64 + 4 * MAX_ENTRIES as u64)
            .min(512 * 1024 * 1024);
        let mut prefix = [0u8; 4];
        input.read_exact(&mut prefix).map_err(|_| unsupported())?;
        if u32::from_le_bytes(prefix) as usize != expected {
            return Err(unsupported());
        }
        staging.write_all(&prefix)?;
        let mut total = 4u64;
        let mut buffer = [0u8; CHUNK];
        loop {
            cancel.check()?;
            self.cancel.check()?;
            let count = input.read(&mut buffer).map_err(|_| unsupported())?;
            if count == 0 {
                break;
            }
            total = total.checked_add(count as u64).ok_or_else(corrupt)?;
            if total > limit
                || total >= self.ratio_check_above
                    && total > source.packed.saturating_mul(self.max_ratio)
            {
                return Err(Error::LimitExceeded(
                    "NSIS solid output or temporary disk budget".into(),
                ));
            }
            staging.write_all(&buffer[..count])?;
            progress.update(Progress {
                files_done: 0,
                files_total: 0,
                bytes_done: total,
                bytes_total: 0,
            });
        }
        if total < 4 + expected as u64 {
            return Err(corrupt());
        }
        staging.seek(SeekFrom::Start(4))?;
        let mut header = vec![0u8; expected];
        staging.read_exact(&mut header)?;
        Ok((header, staging, total))
    }

    fn scan(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<Vec<Entry>> {
        self.solid = None;
        cancel.check()?;
        self.cancel.check()?;
        let mut file = File::open(&self.path)?;
        let file_size = file.metadata()?.len();
        // Bound the search so a large executable stub cannot force unbounded allocation.
        let mut prefix = Vec::new();
        let mut buffer = [0u8; CHUNK];
        let header_offset = loop {
            cancel.check()?;
            self.cancel.check()?;
            let start = prefix.len().saturating_sub(27);
            let count = file.read(&mut buffer)?;
            if count == 0 || prefix.len() + count > 8 * 1024 * 1024 {
                return Err(unsupported());
            }
            prefix.extend_from_slice(&buffer[..count]);
            if let Some(index) = prefix[start..]
                .windows(28)
                .position(super::sniff::is_nsis_first_header)
            {
                break start + index;
            }
        };
        let expected = word(&prefix, header_offset + 20)? as usize;
        let flags = word(&prefix, header_offset)?;
        let nsis_end = (header_offset as u64)
            .checked_add(u64::from(word(&prefix, header_offset + 24)?))
            .ok_or_else(corrupt)?;
        if nsis_end > file_size {
            return Err(corrupt());
        }
        let mut payload_end = if flags & 4 == 0 {
            nsis_end.checked_sub(4).ok_or_else(corrupt)?
        } else {
            nsis_end
        };
        if expected > MAX_METADATA {
            return Err(Error::LimitExceeded("NSIS header metadata".into()));
        }
        if prefix.get(header_offset + 8..header_offset + 20) != Some(b"NullsoftInst") {
            return Err(unsupported());
        }
        drop(prefix);
        if flags & 4 == 0 {
            file.seek(SeekFrom::Start(payload_end))?;
            let mut expected_crc = [0u8; 4];
            file.read_exact(&mut expected_crc)?;
            file.seek(SeekFrom::Start(512))?;
            let mut source = (&mut file).take(payload_end.checked_sub(512).ok_or_else(corrupt)?);
            let mut hasher = crc32fast::Hasher::new();
            loop {
                cancel.check()?;
                self.cancel.check()?;
                let count = source.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            if hasher.finalize() != u32::from_le_bytes(expected_crc) {
                return Err(corrupt());
            }
        }
        let frame_offset = header_offset as u64 + 28;
        file.seek(SeekFrom::Start(frame_offset))?;
        let mut length = [0u8; 4];
        file.read_exact(&mut length)?;
        let frame = u32::from_le_bytes(length);
        let packed = u64::from(frame & 0x7fff_ffff);
        let compressed = frame & 0x8000_0000 != 0;
        let metadata_record = Record {
            entry: Entry {
                index: 0,
                path: String::new(),
                kind: EntryKind::File,
                size: expected as u64,
                packed_size: Some(packed),
                modified: None,
                encrypted: false,
                unix_mode: None,
            },
            offset: frame_offset + 4,
            packed,
            compressed,
            solid: false,
        };
        let non_solid = if packed <= MAX_METADATA as u64 && frame_offset + 4 + packed <= payload_end
        {
            self.header(&metadata_record, expected, cancel).ok()
        } else {
            None
        };
        cancel.check()?;
        self.cancel.check()?;
        let (header, data_offset) = if let Some(header) = non_solid {
            (header, frame_offset + 4 + packed)
        } else {
            let mut source = metadata_record;
            source.offset = frame_offset;
            source.packed = payload_end.checked_sub(frame_offset).ok_or_else(corrupt)?;
            let (header, staging, total) = self.solid_header(source, expected, cancel, progress)?;
            file = staging.try_clone()?;
            self.solid = Some(staging);
            payload_end = total;
            (header, 4 + expected as u64)
        };
        let entries_offset = word(&header, 20)? as usize;
        let entries_count = word(&header, 24)? as usize;
        let strings_offset = word(&header, 28)? as usize;
        let strings_end = word(&header, 36)? as usize;
        if entries_count > MAX_ENTRIES || entries_offset < 68 || strings_offset < entries_offset {
            return Err(corrupt());
        }
        let end = entries_count
            .checked_mul(28)
            .and_then(|size| entries_offset.checked_add(size))
            .ok_or_else(corrupt)?;
        if end > strings_offset {
            return Err(corrupt());
        }
        let strings = header
            .get(strings_offset..strings_end)
            .ok_or_else(corrupt)?;
        let unicode = strings.starts_with(&[0, 0]);
        let ansi3 = !unicode
            && strings
                .windows(3)
                .any(|bytes| bytes[0] == 0 && bytes[1] == 3 && bytes[2] & 0x80 != 0);
        let mut directory = String::new();
        let mut records = Vec::new();
        let mut total = 0u64;
        let mut metadata_bytes = header.len();
        for index in 0..entries_count {
            cancel.check()?;
            self.cancel.check()?;
            let at = entries_offset + index * 28;
            let opcode = word(&header, at)?;
            if opcode == 11 && word(&header, at + 8)? != 0 {
                directory =
                    decode_name(strings, word(&header, at + 4)?, unicode, ansi3, &directory)?;
            }
            if opcode != 20 {
                continue;
            }
            let name = decode_name(strings, word(&header, at + 8)?, unicode, ansi3, &directory)?;
            let path = if name.starts_with('$') || directory.is_empty() {
                name
            } else {
                format!("{directory}/{name}")
            };
            let path = path.replace('\\', "/");
            let path = path.strip_prefix("$INSTDIR/").unwrap_or(&path).to_owned();
            if path.is_empty()
                || path.starts_with('/')
                || path.contains(':')
                || path.split('/').any(|part| part == "..")
            {
                return Err(Error::UnsafePath(path));
            }
            metadata_bytes =
                metadata_bytes.saturating_add(path.len() + std::mem::size_of::<Record>());
            if metadata_bytes > MAX_METADATA {
                return Err(Error::LimitExceeded("NSIS directory metadata".into()));
            }
            let offset = data_offset
                .checked_add(u64::from(word(&header, at + 12)?))
                .ok_or_else(corrupt)?;
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut length)?;
            let frame = u32::from_le_bytes(length);
            let packed = u64::from(frame & 0x7fff_ffff);
            if offset
                .checked_add(4)
                .and_then(|offset| offset.checked_add(packed))
                .is_none_or(|end| end > payload_end)
            {
                return Err(corrupt());
            }
            let mut record = Record {
                entry: Entry {
                    index: records.len(),
                    path,
                    kind: EntryKind::File,
                    size: 0,
                    packed_size: Some(packed),
                    modified: None,
                    encrypted: false,
                    unix_mode: None,
                },
                offset: offset + 4,
                packed,
                compressed: frame & 0x8000_0000 != 0,
                solid: self.solid.is_some(),
            };
            let mut input = self.stream(&record)?;
            loop {
                cancel.check()?;
                let count = input.read(&mut buffer).map_err(|_| corrupt())?;
                if count == 0 {
                    break;
                }
                record.entry.size = record
                    .entry
                    .size
                    .checked_add(count as u64)
                    .ok_or_else(corrupt)?;
                total = total.checked_add(count as u64).ok_or_else(corrupt)?;
                if total > self.max_total
                    || (record.entry.size >= self.ratio_check_above
                        && record.entry.size > packed.saturating_mul(self.max_ratio))
                {
                    return Err(Error::LimitExceeded(
                        "NSIS decoded size or compression ratio".into(),
                    ));
                }
                progress.update(Progress {
                    files_done: records.len() as u64,
                    files_total: 0,
                    bytes_done: total,
                    bytes_total: 0,
                });
            }
            records.push(record);
        }
        let visible = records.iter().map(|record| record.entry.clone()).collect();
        self.records = Some(records);
        Ok(visible)
    }
}

fn decode_name(
    table: &[u8],
    index: u32,
    unicode: bool,
    ansi3: bool,
    directory: &str,
) -> Result<String> {
    let width = if unicode { 2 } else { 1 };
    let start = (index as usize).checked_mul(width).ok_or_else(corrupt)?;
    let mut offset = start;
    let mut units = Vec::new();
    loop {
        if offset.saturating_sub(start) > 8192 {
            return Err(Error::LimitExceeded("NSIS filename".into()));
        }
        let unit = if unicode {
            let pair = table.get(offset..offset + 2).ok_or_else(corrupt)?;
            u16::from_le_bytes([pair[0], pair[1]])
        } else {
            u16::from(*table.get(offset).ok_or_else(corrupt)?)
        };
        offset += width;
        if unit == 0 {
            break;
        }
        let variable = if unicode || ansi3 { 3 } else { 253 };
        let escape = if unicode || ansi3 { 4 } else { 252 };
        if unit == escape {
            let literal = if unicode {
                let bytes = table.get(offset..offset + 2).ok_or_else(corrupt)?;
                u16::from_le_bytes([bytes[0], bytes[1]])
            } else {
                u16::from(*table.get(offset).ok_or_else(corrupt)?)
            };
            units.push(literal);
            offset += width;
        } else if unit == variable {
            let argument = table.get(offset..offset + 2).ok_or_else(corrupt)?;
            let variable = u16::from(argument[0] & 0x7f) | (u16::from(argument[1] & 0x7f) << 7);
            let name = match variable {
                21 => "$INSTDIR",
                22 => directory,
                26 => "$PLUGINSDIR",
                _ => return Err(unsupported()),
            };
            units.extend(name.encode_utf16());
            offset += 2;
        } else if (unicode || ansi3) && unit < 5 || !unicode && !ansi3 && unit >= 254 {
            return Err(unsupported());
        } else {
            units.push(unit);
        }
    }
    if unicode {
        String::from_utf16(&units).map_err(|_| corrupt())
    } else {
        let bytes: Vec<_> = units.into_iter().map(|unit| unit as u8).collect();
        let (text, _, malformed) = encoding_rs::WINDOWS_1252.decode(&bytes);
        if malformed {
            return Err(corrupt());
        }
        Ok(text.into_owned())
    }
}

impl ArchiveReader for NsisReader {
    fn format_name(&self) -> &'static str {
        "nsis"
    }
    fn caps(&self) -> Caps {
        Caps {
            random_access: true,
            ..Caps::default()
        }
    }
    fn set_read_control(&mut self, cancel: CancelToken, max_total: u64) {
        self.cancel = cancel;
        self.max_total = max_total;
        self.emitted.store(0, Ordering::Relaxed);
    }
    fn set_ratio_limit(&mut self, max_ratio: u64, check_above: u64) {
        self.max_ratio = max_ratio;
        self.ratio_check_above = check_above;
    }
    fn entries(&mut self) -> Result<Vec<Entry>> {
        self.entries_with_progress(&self.cancel.clone(), &mut NoProgress)
    }
    fn entries_with_progress(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<Vec<Entry>> {
        cancel.check()?;
        if let Some(records) = &self.records {
            return Ok(records.iter().map(|record| record.entry.clone()).collect());
        }
        self.scan(cancel, progress)
    }
    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        self.cancel.check()?;
        let record = self
            .records
            .as_ref()
            .and_then(|records| records.get(entry.index))
            .filter(|record| record.entry.path == entry.path)
            .ok_or_else(corrupt)?;
        Ok(Box::new(Controlled {
            reader: self.stream(record)?,
            cancel: self.cancel.clone(),
            total: self.emitted.clone(),
            limit: self.max_total,
        }))
    }
}
