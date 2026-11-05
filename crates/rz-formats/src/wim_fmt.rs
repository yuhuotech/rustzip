use rz_core::{
    ArchiveReader, CancelToken, Caps, Entry, EntryKind, Error, NoProgress, Progress, ProgressSink,
    Result,
};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAX_METADATA: u64 = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const CHUNK: usize = 64 * 1024;
type Hash = [u8; 20];

#[derive(Clone, Copy, PartialEq, Eq)]
enum WimCodec {
    None,
    Xpress,
    Lzx,
    Lzms,
}
impl WimCodec {
    fn chunk_size(self) -> u64 {
        if self == Self::Lzms {
            131_072
        } else {
            32_768
        }
    }
}

fn corrupt() -> Error {
    Error::Corrupt("WIM resource or directory metadata is invalid".into())
}
fn unsupported() -> Error {
    Error::Unsupported(
        "WIM: unsupported compression parameters, split volumes, reparse points or alternate streams"
            .into(),
    )
}
fn number<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N]> {
    data.get(offset..offset.checked_add(N).ok_or_else(corrupt)?)
        .ok_or_else(corrupt)?
        .try_into()
        .map_err(|_| corrupt())
}
fn word(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(number(data, offset)?))
}
fn long(data: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(number(data, offset)?))
}

#[derive(Clone, Copy)]
struct Resource {
    offset: u64,
    size: u64,
    packed: u64,
    flags: u8,
    hash: Hash,
    solid: Option<Solid>,
}
#[derive(Clone, Copy)]
struct Solid {
    offset: u64,
    packed: u64,
    size: u64,
    chunk_size: u64,
    file_offset: u64,
}
fn resource(data: &[u8], offset: usize, file_size: u64) -> Result<Resource> {
    let encoded = long(data, offset)?;
    let flags = (encoded >> 56) as u8;
    let packed = encoded & 0x00ff_ffff_ffff_ffff;
    let position = long(data, offset + 8)?;
    let size = long(data, offset + 16)?;
    if flags & !6 != 0 || flags & 4 == 0 && packed != size {
        return Err(unsupported());
    }
    if position
        .checked_add(packed)
        .is_none_or(|end| end > file_size)
    {
        return Err(corrupt());
    }
    Ok(Resource {
        offset: position,
        size,
        packed,
        flags,
        hash: [0; 20],
        solid: None,
    })
}
struct Record {
    entry: Entry,
    resource: Resource,
}
pub struct WimReader {
    path: PathBuf,
    records: Option<Vec<Record>>,
    cancel: CancelToken,
    max_total: u64,
    emitted: Arc<AtomicU64>,
    codec: WimCodec,
    max_ratio: u64,
    ratio_above: u64,
}
impl WimReader {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            records: None,
            cancel: CancelToken::new(),
            max_total: 1 << 40,
            emitted: Arc::new(AtomicU64::new(0)),
            codec: WimCodec::None,
            max_ratio: 1000,
            ratio_above: 1 << 20,
        }
    }
    fn metadata(
        &self,
        file: &mut File,
        resource: Resource,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>> {
        if resource.size > MAX_METADATA {
            return Err(Error::LimitExceeded("WIM metadata".into()));
        }
        let mut bytes = Vec::new();
        let mut buffer = [0u8; CHUNK];
        let mut input = ResourceReader::new(
            file.try_clone()?,
            resource,
            self.codec,
            cancel.clone(),
            Arc::new(AtomicU64::new(0)),
            MAX_METADATA,
        )?;
        loop {
            cancel.check()?;
            self.cancel.check()?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        if bytes.len() as u64 != resource.size {
            return Err(corrupt());
        }
        if resource.hash != [0; 20] && Sha1::digest(&bytes).as_slice() != resource.hash {
            return Err(corrupt());
        }
        Ok(bytes)
    }
    fn scan(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<Vec<Entry>> {
        cancel.check()?;
        self.cancel.check()?;
        let mut file = File::open(&self.path)?;
        let file_size = file.metadata()?.len();
        let mut header = [0u8; 208];
        file.read_exact(&mut header)?;
        if &header[..8] != b"MSWIM\0\0\0"
            || word(&header, 8)? != 208
            || !matches!(word(&header, 12)?, 0x10d00 | 0xe00)
        {
            return Err(unsupported());
        }
        if u16::from_le_bytes(number(&header, 40)?) != 1
            || u16::from_le_bytes(number(&header, 42)?) != 1
        {
            return Err(unsupported());
        }
        let flags = word(&header, 16)?;
        self.codec = match flags & 0x000e_0000 {
            0x0002_0000 => WimCodec::Xpress,
            0x0004_0000 => WimCodec::Lzx,
            0x0008_0000 => WimCodec::Lzms,
            _ => WimCodec::None,
        };
        if self.codec != WimCodec::None && u64::from(word(&header, 20)?) != self.codec.chunk_size()
        {
            return Err(unsupported());
        }
        let image_count = word(&header, 44)? as usize;
        if image_count == 0 || image_count > 256 {
            return Err(Error::LimitExceeded("WIM image count".into()));
        }
        let table = self.metadata(&mut file, resource(&header, 48, file_size)?, cancel)?;
        if table.len() % 50 != 0 || table.len() / 50 > MAX_ENTRIES {
            return Err(corrupt());
        }
        let mut blobs = HashMap::new();
        let mut images = Vec::new();
        let mut solid = None::<Solid>;
        for row in table.chunks_exact(50) {
            cancel.check()?;
            self.cancel.check()?;
            let encoded = long(row, 0)?;
            let flags = (encoded >> 56) as u8;
            let row_size = encoded & 0x00ff_ffff_ffff_ffff;
            let row_offset = long(row, 8)?;
            let row_original = long(row, 16)?;
            if flags == 0x10 {
                if row_original == 0x1_0000_0000 {
                    if row_size < 20
                        || row_offset
                            .checked_add(row_size)
                            .is_none_or(|end| end > file_size)
                    {
                        return Err(corrupt());
                    }
                    file.seek(SeekFrom::Start(row_offset))?;
                    let mut header = [0u8; 16];
                    file.read_exact(&mut header)?;
                    let stream_size = long(&header, 0)?;
                    let chunk_size = u64::from(word(&header, 8)?);
                    if chunk_size == 0
                        || chunk_size > 64 * 1024 * 1024
                        || !chunk_size.is_power_of_two()
                        || word(&header, 12)? != 3
                    {
                        return Err(unsupported());
                    }
                    if stream_size >= self.ratio_above
                        && stream_size > (row_size - 16).saturating_mul(self.max_ratio)
                    {
                        return Err(Error::LimitExceeded("WIM solid compression ratio".into()));
                    }
                    solid = Some(Solid {
                        offset: row_offset + 16,
                        packed: row_size - 16,
                        size: stream_size,
                        chunk_size,
                        file_offset: 0,
                    });
                    continue;
                }
                let mut stream = solid.ok_or_else(corrupt)?;
                if row_offset
                    .checked_add(row_size)
                    .is_none_or(|end| end > stream.size)
                {
                    return Err(corrupt());
                }
                stream.file_offset = row_offset;
                let hash = number(row, 30)?;
                let item = Resource {
                    offset: stream.offset,
                    size: row_size,
                    packed: stream.packed,
                    flags: 4,
                    hash,
                    solid: Some(stream),
                };
                if blobs.insert(hash, item).is_some() {
                    return Err(corrupt());
                }
                continue;
            }
            let mut item = resource(row, 0, file_size)?;
            if u16::from_le_bytes(number(row, 24)?) != 1 {
                return Err(unsupported());
            }
            item.hash = number(row, 30)?;
            if item.flags & 2 != 0 {
                images.push(item);
            } else if blobs.insert(item.hash, item).is_some() {
                return Err(corrupt());
            }
        }
        if images.len() != image_count {
            return Err(corrupt());
        }
        let mut records = Vec::new();
        let mut metadata_cost = table.len();
        let mut total_size = 0u64;
        for (index, image) in images.into_iter().enumerate() {
            let metadata = self.metadata(&mut file, image, cancel)?;
            let security_size = word(&metadata, 0)? as usize;
            let root = security_size
                .checked_add(7)
                .map(|value| value & !7)
                .ok_or_else(corrupt)?;
            if security_size < 8 {
                return Err(corrupt());
            }
            let root_length = usize::try_from(long(&metadata, root)?).map_err(|_| corrupt())?;
            if root_length < 104
                || root
                    .checked_add(root_length)
                    .is_none_or(|end| end > metadata.len())
            {
                return Err(corrupt());
            }
            let children = long(&metadata, root + 16)?;
            let prefix = if image_count > 1 {
                (index + 1).to_string()
            } else {
                String::new()
            };
            let mut queue = vec![(children, prefix, 0usize)];
            let mut visited = HashSet::new();
            while let Some((offset, parent, depth)) = queue.pop() {
                cancel.check()?;
                self.cancel.check()?;
                if offset == 0 {
                    continue;
                }
                if depth > 128 || !visited.insert(offset) {
                    return Err(Error::LimitExceeded("WIM directory depth or cycle".into()));
                }
                let mut at = usize::try_from(offset).map_err(|_| corrupt())?;
                loop {
                    cancel.check()?;
                    self.cancel.check()?;
                    let length = usize::try_from(long(&metadata, at)?).map_err(|_| corrupt())?;
                    if length == 0 {
                        break;
                    }
                    if length < 104 || length % 8 != 0 || at % 8 != 0 {
                        return Err(corrupt());
                    }
                    let item = metadata
                        .get(at..at.checked_add(length).ok_or_else(corrupt)?)
                        .ok_or_else(corrupt)?;
                    let attributes = word(item, 8)?;
                    if attributes & 0x400 != 0 || u16::from_le_bytes(number(item, 96)?) != 0 {
                        return Err(unsupported());
                    }
                    let name_size = usize::from(u16::from_le_bytes(number(item, 100)?));
                    let raw = item
                        .get(102..102usize.checked_add(name_size).ok_or_else(corrupt)?)
                        .ok_or_else(corrupt)?;
                    if name_size == 0 || name_size % 2 != 0 || name_size > 8192 {
                        return Err(corrupt());
                    }
                    let units: Vec<_> = raw
                        .chunks_exact(2)
                        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                        .collect();
                    let name = String::from_utf16(&units).map_err(|_| corrupt())?;
                    if name == "." || name == ".." || name.contains(['/', '\\', ':', '\0']) {
                        return Err(Error::UnsafePath(name));
                    }
                    let path = if parent.is_empty() {
                        name
                    } else {
                        format!("{parent}/{name}")
                    };
                    metadata_cost =
                        metadata_cost.saturating_add(path.len() + std::mem::size_of::<Record>());
                    if records.len() >= MAX_ENTRIES || metadata_cost > MAX_METADATA as usize {
                        return Err(Error::LimitExceeded("WIM directory records".into()));
                    }
                    let kind = if attributes & 0x10 != 0 {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    };
                    let hash: Hash = number(item, 64)?;
                    let resource = if hash == [0; 20] {
                        Resource {
                            offset: 0,
                            size: 0,
                            packed: 0,
                            flags: 0,
                            hash,
                            solid: None,
                        }
                    } else {
                        *blobs.get(&hash).ok_or_else(corrupt)?
                    };
                    if kind == EntryKind::Dir && resource.size != 0 {
                        return Err(corrupt());
                    }
                    total_size = total_size.checked_add(resource.size).ok_or_else(corrupt)?;
                    if resource.flags & 4 != 0 && self.codec == WimCodec::None {
                        return Err(unsupported());
                    }
                    if resource.size >= self.ratio_above
                        && resource.size > resource.packed.saturating_mul(self.max_ratio)
                    {
                        return Err(Error::LimitExceeded("WIM compression ratio".into()));
                    }
                    if total_size > self.max_total {
                        return Err(Error::LimitExceeded("WIM total output".into()));
                    }
                    let filetime = long(item, 56)?;
                    let modified = if filetime == 0 {
                        None
                    } else {
                        i64::try_from(filetime / 10_000_000)
                            .ok()
                            .and_then(|seconds| seconds.checked_sub(11_644_473_600))
                    };
                    if kind == EntryKind::Dir {
                        queue.push((long(item, 16)?, path.clone(), depth + 1));
                    }
                    records.push(Record {
                        entry: Entry {
                            index: records.len(),
                            path,
                            kind,
                            size: resource.size,
                            packed_size: Some(resource.packed),
                            modified,
                            encrypted: false,
                            unix_mode: None,
                        },
                        resource,
                    });
                    progress.update(Progress {
                        files_done: records.len() as u64,
                        files_total: 0,
                        bytes_done: total_size,
                        bytes_total: 0,
                    });
                    at += length;
                }
            }
        }
        let entries = records.iter().map(|record| record.entry.clone()).collect();
        self.records = Some(records);
        Ok(entries)
    }
}

struct ResourceReader {
    file: File,
    resource: Resource,
    codec: WimCodec,
    position: u64,
    cache: Vec<u8>,
    cache_index: Option<u64>,
    digest: Sha1,
    expected: Hash,
    remaining: u64,
    cancel: CancelToken,
    emitted: Arc<AtomicU64>,
    max_total: u64,
}
impl ResourceReader {
    fn new(
        file: File,
        resource: Resource,
        codec: WimCodec,
        cancel: CancelToken,
        emitted: Arc<AtomicU64>,
        max_total: u64,
    ) -> Result<Self> {
        if resource.flags & 4 != 0 && codec == WimCodec::None {
            return Err(unsupported());
        }
        Ok(Self {
            file,
            resource,
            codec,
            position: 0,
            cache: Vec::new(),
            cache_index: None,
            digest: Sha1::new(),
            expected: resource.hash,
            remaining: resource.size,
            cancel,
            emitted,
            max_total,
        })
    }
    fn offset(&mut self, index: u64, width: u64) -> io::Result<u64> {
        let position = index
            .checked_mul(width)
            .and_then(|value| self.resource.offset.checked_add(value))
            .ok_or_else(|| io::Error::other("WIM chunk offset overflow"))?;
        self.file.seek(SeekFrom::Start(position))?;
        let mut bytes = [0u8; 8];
        self.file.read_exact(&mut bytes[..width as usize])?;
        Ok(u64::from_le_bytes(bytes))
    }
    fn chunk(&mut self, index: u64) -> io::Result<()> {
        self.cancel.check().map_err(io::Error::other)?;
        let chunk_size = self
            .resource
            .solid
            .map_or_else(|| self.codec.chunk_size(), |solid| solid.chunk_size);
        let stream_size = self
            .resource
            .solid
            .map_or(self.resource.size, |solid| solid.size);
        let chunks = stream_size.div_ceil(chunk_size);
        if let Some(solid) = self.resource.solid {
            let table_size = chunks
                .checked_mul(4)
                .filter(|size| *size <= MAX_METADATA && *size <= solid.packed)
                .ok_or_else(|| io::Error::other("WIM solid chunk table exceeds limit"))?;
            let mut start = 0u64;
            for entry in 0..=index {
                self.cancel.check().map_err(io::Error::other)?;
                let size = self.offset(entry, 4)?;
                if entry == index {
                    let expected = (stream_size - index * chunk_size).min(chunk_size) as usize;
                    if size > chunk_size
                        || start
                            .checked_add(size)
                            .is_none_or(|end| end > solid.packed - table_size)
                    {
                        return Err(io::Error::other("WIM solid chunk range is invalid"));
                    }
                    self.file
                        .seek(SeekFrom::Start(solid.offset + table_size + start))?;
                    let mut encoded = vec![0; size as usize];
                    self.file.read_exact(&mut encoded)?;
                    let mut output = vec![0; expected];
                    if encoded.len() == expected {
                        output.copy_from_slice(&encoded);
                    } else {
                        rustzip_lzms::decompress_raw_with_cancel(&encoded, &mut output, &|| {
                            self.cancel.is_cancelled()
                        })
                        .map_err(|_| io::Error::other("WIM solid LZMS chunk is corrupt"))?;
                    }
                    self.cache = output;
                    self.cache_index = Some(index);
                    return Ok(());
                }
                start = start
                    .checked_add(size)
                    .ok_or_else(|| io::Error::other("WIM solid chunk overflow"))?;
            }
            return Err(io::Error::other("WIM solid chunk is missing"));
        }
        let width = if self.resource.size > u64::from(u32::MAX) {
            8
        } else {
            4
        };
        let table_size = chunks
            .checked_sub(1)
            .and_then(|value| value.checked_mul(width))
            .filter(|size| *size <= self.resource.packed)
            .ok_or_else(|| io::Error::other("WIM chunk table is invalid"))?;
        let start = if index == 0 {
            0
        } else {
            self.offset(index - 1, width)?
        };
        let end = if index + 1 == chunks {
            self.resource.packed - table_size
        } else {
            self.offset(index, width)?
        };
        let count = end
            .checked_sub(start)
            .filter(|count| *count <= chunk_size.saturating_mul(2))
            .ok_or_else(|| io::Error::other("WIM compressed chunk limit exceeded"))?;
        if end > self.resource.packed - table_size {
            return Err(io::Error::other("WIM chunk extends past resource"));
        }
        let position = self
            .resource
            .offset
            .checked_add(table_size)
            .and_then(|value| value.checked_add(start))
            .ok_or_else(|| io::Error::other("WIM chunk range overflow"))?;
        let expected = (self.resource.size - index * chunk_size).min(chunk_size) as usize;
        self.file.seek(SeekFrom::Start(position))?;
        let mut encoded = vec![0u8; count as usize];
        self.file.read_exact(&mut encoded)?;
        let decoded = if encoded.len() == expected {
            encoded
        } else {
            match self.codec {
                WimCodec::Xpress => xpress_huffman::decompress(&encoded, expected)
                    .map_err(|_| io::Error::other("WIM XPRESS chunk is corrupt"))?,
                WimCodec::Lzx => lzxd::Lzxd::new_wim()
                    .decompress_next(&encoded, expected)
                    .map_err(|_| io::Error::other("WIM LZX chunk is corrupt"))?
                    .to_vec(),
                WimCodec::Lzms => {
                    let mut output = vec![0; expected];
                    rustzip_lzms::decompress_raw_with_cancel(&encoded, &mut output, &|| {
                        self.cancel.is_cancelled()
                    })
                    .map_err(|_| io::Error::other("WIM LZMS chunk is corrupt"))?;
                    output
                }
                WimCodec::None => return Err(io::Error::other("WIM compression is unsupported")),
            }
        };
        self.cancel.check().map_err(io::Error::other)?;
        if decoded.len() != expected {
            return Err(io::Error::other("WIM XPRESS chunk size mismatch"));
        }
        self.cache = decoded;
        self.cache_index = Some(index);
        Ok(())
    }
}
impl Read for ResourceReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        if buffer.is_empty() {
            return Ok(0);
        }
        let request = buffer.len().min(CHUNK).min(self.remaining as usize);
        let count = if self.remaining == 0 {
            0
        } else if self.resource.flags & 4 != 0 {
            let offset = self.position + self.resource.solid.map_or(0, |solid| solid.file_offset);
            let chunk_size = self
                .resource
                .solid
                .map_or_else(|| self.codec.chunk_size(), |solid| solid.chunk_size);
            let index = offset / chunk_size;
            if self.cache_index != Some(index) {
                self.chunk(index)?;
            }
            let within = (offset % chunk_size) as usize;
            let count = request.min(self.cache.len() - within);
            buffer[..count].copy_from_slice(&self.cache[within..within + count]);
            count
        } else {
            self.file
                .seek(SeekFrom::Start(self.resource.offset + self.position))?;
            self.file.read(&mut buffer[..request])?
        };
        if count == 0 && self.remaining != 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "WIM resource truncated",
            ));
        }
        self.remaining -= count as u64;
        self.position += count as u64;
        self.digest.update(&buffer[..count]);
        let old = self.emitted.fetch_add(count as u64, Ordering::Relaxed);
        if old
            .checked_add(count as u64)
            .is_none_or(|value| value > self.max_total)
        {
            return Err(io::Error::other("WIM output limit exceeded"));
        }
        if self.remaining == 0
            && self.expected != [0; 20]
            && self.digest.clone().finalize().as_slice() != self.expected
        {
            return Err(io::Error::other("WIM resource SHA-1 mismatch"));
        }
        Ok(count)
    }
}
impl ArchiveReader for WimReader {
    fn format_name(&self) -> &'static str {
        "wim"
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
        self.ratio_above = check_above;
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
        self.cancel.check()?;
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
        Ok(Box::new(ResourceReader::new(
            File::open(&self.path)?,
            record.resource,
            self.codec,
            self.cancel.clone(),
            self.emitted.clone(),
            self.max_total,
        )?))
    }
}
