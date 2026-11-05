use base64::Engine as _;
use flate2::read::ZlibDecoder;
use quick_xml::events::Event as XmlEvent;
use rz_core::{
    ArchiveReader, CancelToken, Caps, Entry, EntryKind, Error, NoProgress, Progress, ProgressSink,
    Result,
};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const MAX_XML: u64 = 4 * 1024 * 1024;
const MAX_BLOCK: u64 = 8 * 1024 * 1024;
const MAX_RECORDS: usize = 100_000;
const CHUNK: usize = 64 * 1024;
fn corrupt() -> Error {
    Error::Corrupt("DMG block map or HFS catalog is invalid".into())
}
fn unsupported() -> Error {
    Error::Unsupported(
        "DMG: unsupported filesystem, compression, encryption or extent layout".into(),
    )
}
fn bytes<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N]> {
    data.get(at..at.checked_add(N).ok_or_else(corrupt)?)
        .ok_or_else(corrupt)?
        .try_into()
        .map_err(|_| corrupt())
}
fn short(data: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(bytes(data, at)?))
}
fn word(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(bytes(data, at)?))
}
fn long(data: &[u8], at: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(bytes(data, at)?))
}
fn sum(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or_else(corrupt)
}
fn sectors(value: u64) -> Result<u64> {
    value.checked_mul(512).ok_or_else(corrupt)
}
fn checksum(data: &[u8], offset: usize) -> Result<Option<u32>> {
    match (word(data, offset)?, word(data, offset + 4)?) {
        (0, 0) => Ok(None),
        (2, 32) => Ok(Some(word(data, offset + 8)?)),
        _ => Err(unsupported()),
    }
}

#[derive(Clone)]
struct Run {
    start: u64,
    size: u64,
    offset: u64,
    packed: u64,
    kind: u32,
}
struct Partition {
    runs: Vec<Run>,
    checksum: Option<u32>,
}
struct Disk {
    file: File,
    runs: Vec<Run>,
    partitions: Vec<Partition>,
    data_checksum: Option<(u64, u64, u32)>,
    size: u64,
    cancel: CancelToken,
    cache: Option<(usize, Vec<u8>)>,
    max_ratio: u64,
    ratio_above: u64,
}
impl Disk {
    fn new(path: &Path, cancel: CancelToken, max_ratio: u64, ratio_above: u64) -> Result<Self> {
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        if length < 512 {
            return Err(corrupt());
        }
        file.seek(SeekFrom::End(-512))?;
        let mut trailer = [0u8; 512];
        file.read_exact(&mut trailer)?;
        if &trailer[..4] != b"koly" || word(&trailer, 4)? != 4 || word(&trailer, 8)? != 512 {
            return Err(unsupported());
        }
        if word(&trailer, 60)? > 1 {
            return Err(unsupported());
        }
        let xml_offset = long(&trailer, 216)?;
        let xml_length = long(&trailer, 224)?;
        if xml_length > MAX_XML {
            return Err(Error::LimitExceeded("DMG XML metadata".into()));
        }
        if sum(xml_offset, xml_length)? > length - 512 {
            return Err(corrupt());
        }
        let size = sectors(long(&trailer, 492)?)?;
        let fork_offset = long(&trailer, 24)?;
        let fork_length = long(&trailer, 32)?;
        if sum(fork_offset, fork_length)? > length - 512 {
            return Err(corrupt());
        }
        let data_checksum = checksum(&trailer, 80)?.map(|crc| (fork_offset, fork_length, crc));
        let master_checksum = checksum(&trailer, 352)?;
        file.seek(SeekFrom::Start(xml_offset))?;
        let mut xml = Vec::new();
        let mut buffer = [0u8; CHUNK];
        let mut input = (&mut file).take(xml_length);
        loop {
            cancel.check()?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            xml.extend_from_slice(&buffer[..count]);
        }
        if xml.len() as u64 != xml_length {
            return Err(corrupt());
        }
        let mut reader = quick_xml::Reader::from_reader(xml.as_slice());
        let mut depth = 0usize;
        let mut data = None::<Vec<u8>>;
        let mut runs = Vec::new();
        let mut partitions = Vec::new();
        let mut master_hasher = crc32fast::Hasher::new();
        for _ in 0..MAX_RECORDS {
            cancel.check()?;
            match reader.read_event().map_err(|_| corrupt())? {
                XmlEvent::Start(element) => {
                    depth += 1;
                    if depth > 32 {
                        return Err(Error::LimitExceeded("DMG XML depth".into()));
                    }
                    if element.name().as_ref() == b"data" {
                        if data.is_some() {
                            return Err(corrupt());
                        }
                        data = Some(Vec::new());
                    }
                }
                XmlEvent::Text(text) => {
                    if let Some(data) = &mut data {
                        data.extend(
                            text.iter()
                                .copied()
                                .filter(|byte| !byte.is_ascii_whitespace()),
                        );
                    }
                }
                XmlEvent::End(element) => {
                    depth = depth.checked_sub(1).ok_or_else(corrupt)?;
                    if element.name().as_ref() == b"data" {
                        let encoded = data.take().ok_or_else(corrupt)?;
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(encoded)
                            .map_err(|_| corrupt())?;
                        if decoded.starts_with(b"mish") {
                            let partition_checksum = checksum(&decoded, 64)?;
                            master_hasher.update(&bytes::<4>(&decoded, 72)?);
                            let mut partition_runs = Vec::new();
                            let base = sectors(long(&decoded, 8)?)?;
                            let source = sum(fork_offset, long(&decoded, 24)?)?;
                            let count = word(&decoded, 200)? as usize;
                            if count > MAX_RECORDS
                                || 204usize
                                    .checked_add(count.checked_mul(40).ok_or_else(corrupt)?)
                                    .is_none_or(|end| end > decoded.len())
                            {
                                return Err(corrupt());
                            }
                            for index in 0..count {
                                cancel.check()?;
                                let at = 204 + index * 40;
                                let kind = word(&decoded, at)?;
                                if matches!(kind, 0x7fff_fffe | 0xffff_ffff) {
                                    continue;
                                }
                                if !matches!(kind, 0 | 1 | 2 | 0x8000_0005) {
                                    return Err(unsupported());
                                }
                                let run = Run {
                                    start: sum(base, sectors(long(&decoded, at + 8)?)?)?,
                                    size: sectors(long(&decoded, at + 16)?)?,
                                    offset: sum(source, long(&decoded, at + 24)?)?,
                                    packed: long(&decoded, at + 32)?,
                                    kind,
                                };
                                if run.size == 0 {
                                    continue;
                                }
                                if sum(run.start, run.size)? > size
                                    || matches!(kind, 1 | 0x8000_0005)
                                        && sum(run.offset, run.packed)? > xml_offset
                                {
                                    return Err(corrupt());
                                }
                                if kind == 1 && run.packed != run.size {
                                    return Err(corrupt());
                                }
                                if kind == 0x8000_0005
                                    && (run.size > MAX_BLOCK || run.packed > MAX_BLOCK)
                                {
                                    return Err(Error::LimitExceeded(
                                        "DMG compressed block".into(),
                                    ));
                                }
                                if runs.len() >= MAX_RECORDS {
                                    return Err(Error::LimitExceeded("DMG block records".into()));
                                }
                                partition_runs.push(run.clone());
                                runs.push(run);
                            }
                            partitions.push(Partition {
                                runs: partition_runs,
                                checksum: partition_checksum,
                            });
                        }
                    }
                }
                XmlEvent::Eof => {
                    if depth != 0 || data.is_some() {
                        return Err(corrupt());
                    }
                    runs.sort_unstable_by_key(|run| run.start);
                    if runs.is_empty()
                        || runs
                            .windows(2)
                            .any(|pair| pair[0].start + pair[0].size > pair[1].start)
                    {
                        return Err(corrupt());
                    }
                    if master_checksum.is_some_and(|expected| expected != master_hasher.finalize())
                    {
                        return Err(Error::Corrupt("DMG master CRC-32 mismatch".into()));
                    }
                    return Ok(Self {
                        file,
                        runs,
                        partitions,
                        data_checksum,
                        size,
                        cancel,
                        cache: None,
                        max_ratio,
                        ratio_above,
                    });
                }
                _ => {}
            }
        }
        Err(Error::LimitExceeded("DMG XML events".into()))
    }
    fn read_at(&mut self, position: u64, buffer: &mut [u8]) -> Result<()> {
        if sum(position, buffer.len() as u64)? > self.size {
            return Err(corrupt());
        }
        let mut copied = 0;
        while copied < buffer.len() {
            self.cancel.check()?;
            let position = sum(position, copied as u64)?;
            let index = self
                .runs
                .partition_point(|run| run.start <= position)
                .checked_sub(1)
                .ok_or_else(corrupt)?;
            let run = &self.runs[index];
            let within = position - run.start;
            if within >= run.size {
                return Err(corrupt());
            }
            let count = (run.size - within).min((buffer.len() - copied).min(CHUNK) as u64) as usize;
            let target = &mut buffer[copied..copied + count];
            match run.kind {
                0 | 2 => target.fill(0),
                1 => {
                    self.file.seek(SeekFrom::Start(sum(run.offset, within)?))?;
                    self.file.read_exact(target)?;
                }
                _ => {
                    if run.size >= self.ratio_above
                        && run.size > run.packed.saturating_mul(self.max_ratio)
                    {
                        return Err(Error::LimitExceeded("DMG block compression ratio".into()));
                    }
                    if self
                        .cache
                        .as_ref()
                        .is_none_or(|(cached, _)| *cached != index)
                    {
                        self.file.seek(SeekFrom::Start(run.offset))?;
                        let mut input = ZlibDecoder::new((&mut self.file).take(run.packed));
                        let mut decoded = Vec::new();
                        let mut scratch = [0u8; CHUNK];
                        loop {
                            self.cancel.check()?;
                            let count = input.read(&mut scratch).map_err(|_| corrupt())?;
                            if count == 0 {
                                break;
                            }
                            if decoded.len() as u64 + count as u64 > run.size {
                                return Err(corrupt());
                            }
                            decoded.extend_from_slice(&scratch[..count]);
                        }
                        if decoded.len() as u64 != run.size {
                            return Err(corrupt());
                        }
                        self.cache = Some((index, decoded));
                    }
                    let decoded = &self.cache.as_ref().ok_or_else(corrupt)?.1;
                    target.copy_from_slice(
                        decoded
                            .get(within as usize..within as usize + count)
                            .ok_or_else(corrupt)?,
                    );
                }
            }
            copied += count;
        }
        Ok(())
    }

    fn verify_checksums(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
        max_bytes: u64,
    ) -> Result<()> {
        let partitions = std::mem::take(&mut self.partitions);
        let mut bytes_total = self.data_checksum.map_or(0, |(_, length, _)| length);
        for partition in &partitions {
            if partition.checksum.is_some() {
                for run in partition.runs.iter().filter(|run| run.kind != 2) {
                    bytes_total = sum(bytes_total, run.size)?;
                }
            }
        }
        if bytes_total > max_bytes {
            return Err(Error::LimitExceeded(
                "DMG checksum verification budget".into(),
            ));
        }
        let mut bytes_done = 0u64;
        let mut buffer = [0u8; CHUNK];
        for partition in partitions {
            let Some(expected) = partition.checksum else {
                continue;
            };
            let mut hasher = crc32fast::Hasher::new();
            for run in partition.runs.iter().filter(|run| run.kind != 2) {
                let mut position = 0u64;
                while position < run.size {
                    cancel.check()?;
                    let count = (run.size - position).min(CHUNK as u64) as usize;
                    self.read_at(sum(run.start, position)?, &mut buffer[..count])?;
                    hasher.update(&buffer[..count]);
                    position += count as u64;
                    bytes_done += count as u64;
                    progress.update(Progress {
                        files_done: 0,
                        files_total: 0,
                        bytes_done,
                        bytes_total,
                    });
                }
            }
            if hasher.finalize() != expected {
                return Err(Error::Corrupt("DMG partition CRC-32 mismatch".into()));
            }
        }
        if let Some((offset, length, expected)) = self.data_checksum {
            self.file.seek(SeekFrom::Start(offset))?;
            let mut input = (&mut self.file).take(length);
            let mut hasher = crc32fast::Hasher::new();
            let mut remaining = length;
            while remaining > 0 {
                cancel.check()?;
                self.cancel.check()?;
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    return Err(corrupt());
                }
                hasher.update(&buffer[..count]);
                remaining -= count as u64;
                bytes_done += count as u64;
                progress.update(Progress {
                    files_done: 0,
                    files_total: 0,
                    bytes_done,
                    bytes_total,
                });
            }
            if hasher.finalize() != expected {
                return Err(Error::Corrupt("DMG data fork CRC-32 mismatch".into()));
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Fork {
    size: u64,
    extents: Vec<(u64, u64)>,
}
impl Fork {
    fn parse(data: &[u8], at: usize, block_size: u64, base: u64, disk_size: u64) -> Result<Self> {
        let size = long(data, at)?;
        let mut extents = Vec::new();
        let mut capacity = 0u64;
        for index in 0..8 {
            let start = u64::from(word(data, at + 16 + index * 8)?)
                .checked_mul(block_size)
                .ok_or_else(corrupt)?;
            let length = u64::from(word(data, at + 20 + index * 8)?)
                .checked_mul(block_size)
                .ok_or_else(corrupt)?;
            if length == 0 {
                continue;
            }
            let offset = sum(base, start)?;
            if sum(offset, length)? > disk_size {
                return Err(corrupt());
            }
            capacity = sum(capacity, length)?;
            extents.push((offset, length));
        }
        if capacity < size {
            return Err(unsupported());
        }
        Ok(Self { size, extents })
    }
    fn read_at(&self, disk: &mut Disk, position: u64, target: &mut [u8]) -> Result<()> {
        if sum(position, target.len() as u64)? > self.size {
            return Err(corrupt());
        }
        let mut logical = 0u64;
        let mut copied = 0usize;
        for (offset, length) in &self.extents {
            let position = position + copied as u64;
            if position >= logical + length {
                logical += length;
                continue;
            }
            let within = position.checked_sub(logical).ok_or_else(corrupt)?;
            let count = (length - within).min((target.len() - copied) as u64) as usize;
            if *offset == u64::MAX {
                target[copied..copied + count].fill(0);
            } else {
                disk.read_at(sum(*offset, within)?, &mut target[copied..copied + count])?;
            }
            copied += count;
            logical += length;
            if copied == target.len() {
                return Ok(());
            }
        }
        if target.is_empty() {
            Ok(())
        } else {
            Err(corrupt())
        }
    }
}
struct Record {
    entry: Entry,
    fork: Fork,
    compressed: Option<ApfsCompression>,
}
#[derive(Clone)]
struct ApfsCompression {
    kind: u32,
    inline: Option<Vec<u8>>,
    fork: Option<Fork>,
}
struct Node {
    id: u32,
    parent: u32,
    name: String,
    kind: EntryKind,
    modified: i64,
    mode: u32,
    fork: Fork,
    hidden: bool,
}

struct ApfsSource {
    disk: Disk,
    base: u64,
    size: u64,
    position: u64,
    read_budget: u64,
    operations: usize,
    cancel: CancelToken,
}

impl Read for ApfsSource {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        self.operations += 1;
        if self.operations > 500_000 {
            return Err(io::Error::other("APFS metadata operation budget exceeded"));
        }
        let count = (self.size - self.position).min(output.len().min(CHUNK) as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        self.read_budget = self
            .read_budget
            .checked_sub(count as u64)
            .ok_or_else(|| io::Error::other("APFS metadata read budget exceeded"))?;
        self.disk
            .read_at(self.base + self.position, &mut output[..count])
            .map_err(io::Error::other)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for ApfsSource {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.cancel.check().map_err(io::Error::other)?;
        self.operations += 1;
        if self.operations > 500_000 {
            return Err(io::Error::other("APFS metadata operation budget exceeded"));
        }
        self.position = match position {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => self.size.checked_add_signed(offset),
        }
        .filter(|offset| *offset <= self.size)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "APFS seek outside partition")
        })?;
        Ok(self.position)
    }
}

fn apfs_error(error: apfs_core::ApfsError, cancel: &CancelToken) -> Error {
    if cancel.is_cancelled() {
        Error::Cancelled
    } else if matches!(error, apfs_core::ApfsError::FieldOutOfRange { .. }) {
        Error::LimitExceeded(format!("APFS metadata: {error}"))
    } else {
        Error::Corrupt(format!("APFS metadata: {error}"))
    }
}

fn apfs_fork(
    source: &mut ApfsSource,
    volume: &apfs_core::volume::ApfsVolume,
    oid: u64,
    size: u64,
    block_size: usize,
    cancel: &CancelToken,
) -> Result<Fork> {
    let mapped = apfs_core::extent::list_extents(source, volume, oid, block_size)
        .map_err(|error| apfs_error(error, cancel))?;
    if mapped.len() > MAX_RECORDS {
        return Err(Error::LimitExceeded("APFS file extents".into()));
    }
    let mut extents = Vec::new();
    let mut logical = 0u64;
    for extent in mapped {
        cancel.check()?;
        if extent.logical_offset < logical {
            return Err(corrupt());
        }
        if extent.logical_offset >= size {
            break;
        }
        if extent.logical_offset > logical {
            extents.push((u64::MAX, extent.logical_offset - logical));
            logical = extent.logical_offset;
        }
        let length = extent.len.min(size - logical);
        if length == 0 {
            continue;
        }
        let offset = if extent.phys_block_num == 0 {
            u64::MAX
        } else {
            let offset = extent
                .phys_block_num
                .checked_mul(block_size as u64)
                .ok_or_else(corrupt)?;
            if sum(offset, length)? > source.size {
                return Err(corrupt());
            }
            sum(source.base, offset)?
        };
        extents.push((offset, length));
        logical = sum(logical, length)?;
    }
    if logical < size {
        return Err(corrupt());
    }
    Ok(Fork { size, extents })
}

fn scan_apfs(
    disk: Disk,
    base: u64,
    size: u64,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
    max_total: u64,
) -> Result<Vec<Record>> {
    let mut source = ApfsSource {
        disk,
        base,
        size,
        position: 0,
        read_budget: 256 * 1024 * 1024,
        operations: 0,
        cancel: cancel.clone(),
    };
    let mut container =
        apfs_core::ApfsContainer::open(&mut source).map_err(|error| apfs_error(error, cancel))?;
    let block_size = container.superblock().block_size as usize;
    let volumes = container
        .volume_superblock_addrs()
        .map_err(|error| apfs_error(error, cancel))?;
    drop(container);
    if volumes.len() != 1 {
        return Err(Error::Unsupported("APFS: multiple volumes".into()));
    }
    source.seek(SeekFrom::Start(
        volumes[0]
            .checked_mul(block_size as u64)
            .ok_or_else(corrupt)?,
    ))?;
    let mut block = vec![0u8; block_size];
    source.read_exact(&mut block)?;
    let volume =
        apfs_core::volume::ApfsVolume::parse(&block).map_err(|error| apfs_error(error, cancel))?;
    // The on-disk flags follow the UUID and last-modified timestamp.
    if u64::from_le_bytes(bytes(&block, 264)?) & 1 == 0 {
        return Err(Error::Unsupported("APFS: encrypted volume".into()));
    }
    let mut stack = vec![(apfs_core::dir::ROOT_DIR_INO_NUM, String::new(), 0usize)];
    let mut visited = HashSet::new();
    let mut records = Vec::new();
    let mut cost = 0usize;
    let mut total = 0u64;
    while let Some((directory, parent, depth)) = stack.pop() {
        cancel.check()?;
        if depth > 128 || !visited.insert(directory) {
            return Err(Error::LimitExceeded("APFS directory depth or cycle".into()));
        }
        let entries = apfs_core::dir::list_dir(&mut source, &volume, directory, block_size)
            .map_err(|error| apfs_error(error, cancel))?;
        if entries.len() > MAX_RECORDS.saturating_sub(records.len()) {
            return Err(Error::LimitExceeded("APFS directory records".into()));
        }
        for item in entries {
            cancel.check()?;
            let name = item.name;
            if name.is_empty()
                || name == "."
                || name == ".."
                || name.contains(['/', '\\', ':', '\0'])
            {
                return Err(Error::UnsafePath(name));
            }
            let inode = apfs_core::dir::load_inode(&mut source, &volume, item.file_id, block_size)
                .map_err(|error| apfs_error(error, cancel))?;
            let kind = match inode.mode & 0xf000 {
                0x4000 => EntryKind::Dir,
                0x8000 => EntryKind::File,
                0xa000 => EntryKind::Symlink,
                _ => return Err(Error::Unsupported("APFS: special file".into())),
            };
            let path = if parent.is_empty() {
                name
            } else {
                format!("{parent}/{name}")
            };
            let mut file_size = if kind == EntryKind::File {
                inode.size.unwrap_or(0)
            } else {
                0
            };
            let compressed = if kind == EntryKind::File && inode.bsd_flags & 0x20 != 0 {
                let header =
                    apfs_core::xattr::decmpfs_header(&mut source, &volume, inode.oid, block_size)
                        .map_err(|error| apfs_error(error, cancel))?
                        .ok_or_else(corrupt)?;
                if header.len() < 16 || &header[..4] != b"fpmc" {
                    return Err(corrupt());
                }
                let compression_kind = u32::from_le_bytes(bytes(&header, 4)?);
                file_size = u64::from_le_bytes(bytes(&header, 8)?);
                if !matches!(compression_kind, 1 | 3 | 4 | 7 | 8 | 9 | 10 | 11 | 12) {
                    return Err(Error::Unsupported(format!(
                        "APFS compression type {compression_kind}"
                    )));
                }
                let (inline, fork) = if compression_kind % 2 == 1 {
                    if file_size > CHUNK as u64 {
                        return Err(Error::LimitExceeded("APFS inline compressed output".into()));
                    }
                    (Some(header[16..].to_vec()), None)
                } else {
                    use apfs_core::xattr::{get_xattr, XattrValue, XATTR_NAME_RESOURCE_FORK};
                    let value = get_xattr(
                        &mut source,
                        &volume,
                        inode.oid,
                        XATTR_NAME_RESOURCE_FORK,
                        block_size,
                    )
                    .map_err(|error| apfs_error(error, cancel))?
                    .ok_or_else(corrupt)?;
                    match value {
                        XattrValue::Embedded(value) => (Some(value), None),
                        XattrValue::Stream { dstream_oid, size } => (
                            None,
                            Some(apfs_fork(
                                &mut source,
                                &volume,
                                dstream_oid,
                                size,
                                block_size,
                                cancel,
                            )?),
                        ),
                        _ => return Err(unsupported()),
                    }
                };
                let packed = fork.as_ref().map_or_else(
                    || inline.as_ref().map_or(0, |data| data.len() as u64),
                    |fork| fork.size,
                );
                if file_size > source.disk.ratio_above
                    && packed.saturating_mul(source.disk.max_ratio) < file_size
                {
                    return Err(Error::LimitExceeded("APFS compression ratio".into()));
                }
                Some(ApfsCompression {
                    kind: compression_kind,
                    inline,
                    fork,
                })
            } else {
                None
            };
            let fork = if compressed.is_none() && file_size > 0 {
                apfs_fork(
                    &mut source,
                    &volume,
                    inode.private_id,
                    file_size,
                    block_size,
                    cancel,
                )?
            } else {
                Fork {
                    size: file_size,
                    extents: Vec::new(),
                }
            };
            total = sum(total, file_size)?;
            cost = cost
                .saturating_add(path.len())
                .saturating_add(fork.extents.len() * std::mem::size_of::<(u64, u64)>())
                .saturating_add(compressed.as_ref().map_or(0, |c| {
                    c.inline.as_ref().map_or(0, Vec::len)
                        + c.fork.as_ref().map_or(0, |f| f.extents.len() * 16)
                }))
                .saturating_add(std::mem::size_of::<Record>());
            if total > max_total || cost > 16 * 1024 * 1024 {
                return Err(Error::LimitExceeded("APFS metadata or output".into()));
            }
            if kind == EntryKind::Dir {
                stack.push((item.file_id, path.clone(), depth + 1));
            }
            records.push(Record {
                entry: Entry {
                    index: records.len(),
                    path,
                    kind,
                    size: file_size,
                    packed_size: None,
                    modified: i64::try_from(inode.mod_time / 1_000_000_000).ok(),
                    encrypted: false,
                    unix_mode: Some(u32::from(inode.mode)),
                },
                fork,
                compressed,
            });
            progress.update(Progress {
                files_done: records.len() as u64,
                files_total: 0,
                bytes_done: total,
                bytes_total: 0,
            });
        }
    }
    Ok(records)
}
pub struct DmgReader {
    path: PathBuf,
    records: Option<Vec<Record>>,
    cancel: CancelToken,
    max_total: u64,
    max_ratio: u64,
    ratio_above: u64,
    emitted: Arc<AtomicU64>,
}
impl DmgReader {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            records: None,
            cancel: CancelToken::new(),
            max_total: 1 << 40,
            max_ratio: 1000,
            ratio_above: 1 << 20,
            emitted: Arc::new(AtomicU64::new(0)),
        }
    }
    fn disk(&self) -> Result<Disk> {
        Disk::new(
            &self.path,
            self.cancel.clone(),
            self.max_ratio,
            self.ratio_above,
        )
    }
    fn scan(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<Vec<Entry>> {
        cancel.check()?;
        self.cancel.check()?;
        let mut disk = self.disk()?;
        let mut sector = [0u8; 512];
        disk.read_at(512, &mut sector)?;
        let base = if &sector[..8] == b"EFI PART" {
            let header_size = u32::from_le_bytes(bytes(&sector, 12)?) as usize;
            if !(92..=512).contains(&header_size) {
                return Err(corrupt());
            }
            let expected_crc = u32::from_le_bytes(bytes(&sector, 16)?);
            let mut header = sector[..header_size].to_vec();
            header[16..20].fill(0);
            if crc32fast::hash(&header) != expected_crc {
                return Err(corrupt());
            }
            let table_offset = sectors(u64::from_le_bytes(bytes(&sector, 72)?))?;
            let count = u32::from_le_bytes(bytes(&sector, 80)?) as usize;
            let entry_size = u32::from_le_bytes(bytes(&sector, 84)?) as usize;
            if count == 0 || count > 256 || !(128..=1024).contains(&entry_size) {
                return Err(Error::LimitExceeded("DMG GPT partitions".into()));
            }
            let mut table = vec![0u8; count * entry_size];
            disk.read_at(table_offset, &mut table)?;
            if crc32fast::hash(&table) != u32::from_le_bytes(bytes(&sector, 88)?) {
                return Err(corrupt());
            }
            let hfs_guid = [
                0x00, 0x53, 0x46, 0x48, 0, 0, 0xaa, 0x11, 0xaa, 0x11, 0, 0x30, 0x65, 0x43, 0xec,
                0xac,
            ];
            let apfs_guid = [
                0xef, 0x57, 0x34, 0x7c, 0x00, 0x00, 0xaa, 0x11, 0xaa, 0x11, 0x00, 0x30, 0x65, 0x43,
                0xec, 0xac,
            ];
            let partition = table
                .chunks_exact(entry_size)
                .find(|entry| entry[..16] == hfs_guid || entry[..16] == apfs_guid)
                .ok_or_else(unsupported)?;
            if partition[..16] == apfs_guid {
                let start = u64::from_le_bytes(bytes(partition, 32)?);
                let end = u64::from_le_bytes(bytes(partition, 40)?);
                let base = sectors(start)?;
                let size = sectors(
                    end.checked_sub(start)
                        .and_then(|n| n.checked_add(1))
                        .ok_or_else(corrupt)?,
                )?;
                if sum(base, size)? > disk.size {
                    return Err(corrupt());
                }
                let records = scan_apfs(disk, base, size, cancel, progress, self.max_total)?;
                let entries = records.iter().map(|record| record.entry.clone()).collect();
                self.records = Some(records);
                return Ok(entries);
            }
            sectors(u64::from_le_bytes(bytes(partition, 32)?))?
        } else if &sector[..2] == b"PM" {
            let count = word(&sector, 4)?;
            if count > 256 {
                return Err(Error::LimitExceeded("DMG partitions".into()));
            }
            let mut found = None;
            for index in 1..=count {
                cancel.check()?;
                disk.read_at(u64::from(index) * 512, &mut sector)?;
                if sector
                    .get(48..80)
                    .is_some_and(|value| value.starts_with(b"Apple_HFS\0"))
                {
                    found = Some(sectors(u64::from(word(&sector, 8)?))?);
                    break;
                }
            }
            found.ok_or_else(unsupported)?
        } else {
            0
        };
        disk.read_at(sum(base, 1024)?, &mut sector)?;
        if !matches!(&sector[..2], b"H+" | b"HX") {
            return Err(unsupported());
        }
        let block_size = u64::from(word(&sector, 40)?);
        if !(512..=1024 * 1024).contains(&block_size) || !block_size.is_power_of_two() {
            return Err(corrupt());
        }
        let catalog = Fork::parse(&sector, 272, block_size, base, disk.size)?;
        let mut tree_header = [0u8; 64];
        catalog.read_at(&mut disk, 0, &mut tree_header)?;
        let node_size = usize::from(short(&tree_header, 32)?);
        if !(512..=CHUNK).contains(&node_size) || !node_size.is_power_of_two() {
            return Err(corrupt());
        }
        let mut next = word(&tree_header, 24)?;
        let mut visited = HashSet::new();
        let mut nodes = HashMap::new();
        let mut cost = 0usize;
        while next != 0 {
            cancel.check()?;
            self.cancel.check()?;
            if visited.len() >= MAX_RECORDS || !visited.insert(next) {
                return Err(Error::LimitExceeded(
                    "HFS catalog node count or cycle".into(),
                ));
            }
            let mut data = vec![0u8; node_size];
            catalog.read_at(&mut disk, u64::from(next) * node_size as u64, &mut data)?;
            if data[8] != 0xff {
                return Err(corrupt());
            }
            next = word(&data, 0)?;
            let count = usize::from(short(&data, 10)?);
            if count > (node_size - 14) / 10 {
                return Err(corrupt());
            }
            let offsets_start = node_size.checked_sub((count + 1) * 2).ok_or_else(corrupt)?;
            for index in 0..count {
                cancel.check()?;
                let start = usize::from(short(&data, node_size - 2 * (index + 1))?);
                let end = usize::from(short(&data, node_size - 2 * (index + 2))?);
                if start < 14 || end > offsets_start || end < start {
                    return Err(corrupt());
                }
                let record = data.get(start..end).ok_or_else(corrupt)?;
                let key_length = usize::from(short(record, 0)?);
                let at = key_length.checked_add(2).ok_or_else(corrupt)?;
                let kind = short(record, at)?;
                if kind != 1 && kind != 2 {
                    continue;
                }
                let parent = word(record, 2)?;
                let chars = usize::from(short(record, 6)?);
                if chars > 255 || 8 + chars * 2 > at {
                    return Err(corrupt());
                }
                let name_units: Vec<_> = record[8..8 + chars * 2]
                    .chunks_exact(2)
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                    .collect();
                let name = String::from_utf16(&name_units).map_err(|_| corrupt())?;
                let hidden = kind == 1
                    && parent == 2
                    && matches!(
                        name.as_str(),
                        "\0\0\0\0HFS+ Private Data" | ".HFS+ Private Directory Data\r"
                    );
                if !hidden
                    && (name.is_empty()
                        || name == "."
                        || name == ".."
                        || name.contains(['/', '\\', ':', '\0']))
                {
                    return Err(Error::UnsafePath(name));
                }
                let id = word(record, at + 8)?;
                let mode = u32::from(short(record, at + 42)?);
                if kind == 2 && record.get(at + 48..at + 52) == Some(b"hlnk") {
                    return Err(unsupported());
                }
                let entry_kind = if kind == 1 {
                    EntryKind::Dir
                } else if mode & 0xf000 == 0xa000 {
                    EntryKind::Symlink
                } else {
                    EntryKind::File
                };
                let fork = if kind == 2 {
                    Fork::parse(record, at + 88, block_size, base, disk.size)?
                } else {
                    Fork {
                        size: 0,
                        extents: Vec::new(),
                    }
                };
                cost = cost.saturating_add(std::mem::size_of::<Node>() + name.len());
                if nodes.len() >= MAX_RECORDS || cost > 16 * 1024 * 1024 {
                    return Err(Error::LimitExceeded("HFS catalog metadata".into()));
                }
                if nodes
                    .insert(
                        id,
                        Node {
                            id,
                            parent,
                            name,
                            kind: entry_kind,
                            modified: i64::from(word(record, at + 16)?) - 2_082_844_800,
                            mode,
                            fork,
                            hidden,
                        },
                    )
                    .is_some()
                {
                    return Err(corrupt());
                }
                progress.update(Progress {
                    files_done: nodes.len() as u64,
                    files_total: 0,
                    bytes_done: 0,
                    bytes_total: 0,
                });
            }
        }
        let mut records = Vec::new();
        let mut total = 0u64;
        for node in nodes.values().filter(|node| node.id != 2 && !node.hidden) {
            cancel.check()?;
            let mut components = vec![node.name.as_str()];
            let mut parent = node.parent;
            let mut ancestors = HashSet::new();
            let mut hidden = false;
            while parent != 2 {
                if ancestors.len() >= 128 || !ancestors.insert(parent) {
                    return Err(Error::LimitExceeded("HFS path depth or cycle".into()));
                }
                let folder = nodes
                    .get(&parent)
                    .filter(|node| node.kind == EntryKind::Dir)
                    .ok_or_else(corrupt)?;
                if folder.hidden {
                    hidden = true;
                    break;
                }
                components.push(&folder.name);
                parent = folder.parent;
            }
            if hidden {
                continue;
            }
            components.reverse();
            let path = components.join("/");
            cost = cost.saturating_add(path.len());
            if cost > 16 * 1024 * 1024 {
                return Err(Error::LimitExceeded("HFS paths".into()));
            }
            total = sum(total, node.fork.size)?;
            if total > self.max_total {
                return Err(Error::LimitExceeded("DMG total output".into()));
            }
            records.push(Record {
                entry: Entry {
                    index: 0,
                    path,
                    kind: node.kind,
                    size: node.fork.size,
                    packed_size: None,
                    modified: Some(node.modified),
                    encrypted: false,
                    unix_mode: Some(node.mode),
                },
                fork: node.fork.clone(),
                compressed: None,
            });
        }
        records.sort_unstable_by(|left, right| left.entry.path.cmp(&right.entry.path));
        for (index, record) in records.iter_mut().enumerate() {
            record.entry.index = index;
        }
        let entries = records.iter().map(|record| record.entry.clone()).collect();
        self.records = Some(records);
        Ok(entries)
    }
}
struct FileReader {
    disk: Disk,
    fork: Fork,
    position: u64,
    emitted: Arc<AtomicU64>,
    limit: u64,
}

struct CompressedReader {
    disk: Disk,
    compression: ApfsCompression,
    size: u64,
    position: u64,
    chunk_index: u64,
    chunk: Vec<u8>,
    emitted: Arc<AtomicU64>,
    limit: u64,
}

impl CompressedReader {
    fn fork_read(&mut self, offset: u64, output: &mut [u8]) -> Result<()> {
        if let Some(fork) = &self.compression.fork {
            fork.read_at(&mut self.disk, offset, output)
        } else {
            let data = self.compression.inline.as_ref().ok_or_else(corrupt)?;
            let start = usize::try_from(offset).map_err(|_| corrupt())?;
            let end = start.checked_add(output.len()).ok_or_else(corrupt)?;
            output.copy_from_slice(data.get(start..end).ok_or_else(corrupt)?);
            Ok(())
        }
    }

    fn fork_len(&self) -> u64 {
        self.compression.fork.as_ref().map_or_else(
            || {
                self.compression
                    .inline
                    .as_ref()
                    .map_or(0, |v| v.len() as u64)
            },
            |fork| fork.size,
        )
    }

    fn load_chunk(&mut self, index: u64) -> Result<()> {
        self.disk.cancel.check()?;
        let expected = (self.size - index * CHUNK as u64).min(CHUNK as u64) as usize;
        let kind = self.compression.kind;
        let input = if kind % 2 == 1 {
            self.compression.inline.clone().ok_or_else(corrupt)?
        } else {
            let mut word = [0u8; 4];
            self.fork_read(0, &mut word)?;
            let header = if kind == 4 {
                u32::from_be_bytes(word)
            } else {
                u32::from_le_bytes(word)
            } as u64;
            let (start, length) = if kind == 4 {
                let table = sum(header, 4)?;
                self.fork_read(table, &mut word)?;
                let count = u32::from_le_bytes(word) as u64;
                let required = self.size.div_ceil(CHUNK as u64);
                if count != required || index >= count {
                    return Err(corrupt());
                }
                let mut pair = [0u8; 8];
                self.fork_read(
                    sum(sum(table, 4)?, index.checked_mul(8).ok_or_else(corrupt)?)?,
                    &mut pair,
                )?;
                let offset = u32::from_le_bytes(bytes(&pair, 0)?) as u64;
                let length = u32::from_le_bytes(bytes(&pair, 4)?) as u64;
                let start = sum(table, offset)?;
                if start < sum(sum(table, 4)?, count.checked_mul(8).ok_or_else(corrupt)?)? {
                    return Err(corrupt());
                }
                (start, length)
            } else {
                let required = self.size.div_ceil(CHUNK as u64);
                if header < (required + 1).checked_mul(4).ok_or_else(corrupt)?
                    || !header.is_multiple_of(4)
                {
                    return Err(corrupt());
                }
                let start = if index == 0 {
                    header
                } else {
                    self.fork_read(index.checked_mul(4).ok_or_else(corrupt)?, &mut word)?;
                    u32::from_le_bytes(word) as u64
                };
                self.fork_read((index + 1).checked_mul(4).ok_or_else(corrupt)?, &mut word)?;
                let end = u32::from_le_bytes(word) as u64;
                if start < header {
                    return Err(corrupt());
                }
                (start, end.checked_sub(start).ok_or_else(corrupt)?)
            };
            if length == 0 || length > MAX_BLOCK || sum(start, length)? > self.fork_len() {
                return Err(Error::LimitExceeded("APFS compressed chunk".into()));
            }
            let mut data = vec![0; length as usize];
            self.fork_read(start, &mut data)?;
            data
        };
        if expected as u64 > self.disk.ratio_above
            && (input.len() as u64).saturating_mul(self.disk.max_ratio) < expected as u64
        {
            return Err(Error::LimitExceeded("APFS compression ratio".into()));
        }
        let mut decoded = Vec::with_capacity(expected);
        match kind {
            1 | 10 => decoded.extend_from_slice(&input),
            9 => decoded.extend_from_slice(input.get(1..).ok_or_else(corrupt)?),
            3 | 4 if input.first() == Some(&0xff) => decoded.extend_from_slice(&input[1..]),
            3 | 4 => {
                ZlibDecoder::new(input.as_slice())
                    .take(expected as u64 + 1)
                    .read_to_end(&mut decoded)?;
            }
            7 | 8 if input.first() == Some(&0x06) => decoded.extend_from_slice(&input[1..]),
            7 | 8 => {
                decoded.resize(expected, 0);
                let written = lzvn::decode_into(&input, &mut decoded).map_err(|_| corrupt())?;
                decoded.truncate(written);
            }
            11 | 12 => {
                let mut decoder = lzfse_rust::LzfseRingDecoder::default();
                decoder
                    .reader(input.as_slice())
                    .take(expected as u64 + 1)
                    .read_to_end(&mut decoded)?;
            }
            _ => return Err(unsupported()),
        }
        if decoded.len() != expected {
            return Err(corrupt());
        }
        self.chunk = decoded;
        self.chunk_index = index;
        Ok(())
    }
}

impl Read for CompressedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.disk.cancel.check().map_err(io::Error::other)?;
        if self.position == self.size || buffer.is_empty() {
            return Ok(0);
        }
        let index = self.position / CHUNK as u64;
        if self.chunk.is_empty() || self.chunk_index != index {
            self.load_chunk(index).map_err(io::Error::other)?;
        }
        let within = (self.position % CHUNK as u64) as usize;
        let count = buffer.len().min(self.chunk.len() - within);
        let old = self.emitted.fetch_add(count as u64, Ordering::Relaxed);
        if old
            .checked_add(count as u64)
            .is_none_or(|total| total > self.limit)
        {
            return Err(io::Error::other("DMG output limit exceeded"));
        }
        buffer[..count].copy_from_slice(&self.chunk[within..within + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Read for FileReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.disk.cancel.check().map_err(io::Error::other)?;
        let count = (self.fork.size - self.position).min(buffer.len().min(CHUNK) as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        self.fork
            .read_at(&mut self.disk, self.position, &mut buffer[..count])
            .map_err(io::Error::other)?;
        let old = self.emitted.fetch_add(count as u64, Ordering::Relaxed);
        if old
            .checked_add(count as u64)
            .is_none_or(|total| total > self.limit)
        {
            return Err(io::Error::other("DMG output limit exceeded"));
        }
        self.position += count as u64;
        Ok(count)
    }
}
impl ArchiveReader for DmgReader {
    fn format_name(&self) -> &'static str {
        "dmg"
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
    fn verify_integrity(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
        max_bytes: u64,
    ) -> Result<()> {
        cancel.check()?;
        self.disk()?.verify_checksums(cancel, progress, max_bytes)
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
        if let Some(compression) = &record.compressed {
            return Ok(Box::new(CompressedReader {
                disk: self.disk()?,
                compression: compression.clone(),
                size: record.entry.size,
                position: 0,
                chunk_index: 0,
                chunk: Vec::new(),
                emitted: self.emitted.clone(),
                limit: self.max_total,
            }));
        }
        Ok(Box::new(FileReader {
            disk: self.disk()?,
            fork: record.fork.clone(),
            position: 0,
            emitted: self.emitted.clone(),
            limit: self.max_total,
        }))
    }
}

#[cfg(test)]
mod compression_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::io::Write;

    fn reader(kind: u32, payload: Vec<u8>, size: usize) -> CompressedReader {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dmg_apfs_zlib.dmg");
        CompressedReader {
            disk: Disk::new(&path, CancelToken::new(), 1000, 1 << 20).unwrap(),
            compression: ApfsCompression {
                kind,
                inline: Some(payload),
                fork: None,
            },
            size: size as u64,
            position: 0,
            chunk_index: 0,
            chunk: Vec::new(),
            emitted: Arc::new(AtomicU64::new(0)),
            limit: u64::MAX,
        }
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn inline_zlib_and_lzfse_are_bounded_and_exact() {
        let payload = b"transparent compression";
        let mut stream = reader(3, zlib(payload), payload.len());
        let mut output = Vec::new();
        stream.read_to_end(&mut output).unwrap();
        assert_eq!(output, payload);
        let mut stream = reader(3, zlib(payload), payload.len() - 1);
        assert!(stream.read_to_end(&mut Vec::new()).is_err());
        let lzfse = b"bvx-\x04\0\0\0testbvx$".to_vec();
        let mut stream = reader(11, lzfse, 4);
        output.clear();
        stream.read_to_end(&mut output).unwrap();
        assert_eq!(output, b"test");
    }

    #[test]
    fn inline_stored_variants_and_lzvn_markers() {
        for (kind, payload) in [
            (1, b"test".to_vec()),
            (3, b"\xfftest".to_vec()),
            (7, b"\x06test".to_vec()),
            (9, b"\xcctest".to_vec()),
        ] {
            let mut output = Vec::new();
            reader(kind, payload, 4).read_to_end(&mut output).unwrap();
            assert_eq!(output, b"test");
        }
        let mut fork = Vec::new();
        fork.extend_from_slice(&8u32.to_le_bytes());
        fork.extend_from_slice(&13u32.to_le_bytes());
        fork.extend_from_slice(b"\x06test");
        let mut output = Vec::new();
        reader(8, fork, 4).read_to_end(&mut output).unwrap();
        assert_eq!(output, b"test");
    }

    #[test]
    fn zlib_resource_fork_streams_multiple_chunks_and_rejects_damage() {
        let source = vec![b'x'; CHUNK + 17];
        let chunks: Vec<_> = source.chunks(CHUNK).map(zlib).collect();
        let mut fork = vec![0; 16 + 4 + 4 + 16];
        fork[..4].copy_from_slice(&16u32.to_be_bytes());
        fork[20..24].copy_from_slice(&2u32.to_le_bytes());
        for (index, chunk) in chunks.iter().enumerate() {
            let offset = (fork.len() - 20) as u32;
            fork[24 + index * 8..28 + index * 8].copy_from_slice(&offset.to_le_bytes());
            fork[28 + index * 8..32 + index * 8]
                .copy_from_slice(&(chunk.len() as u32).to_le_bytes());
            fork.extend_from_slice(chunk);
        }
        let mut stream = reader(4, fork.clone(), source.len());
        let mut output = Vec::new();
        stream.read_to_end(&mut output).unwrap();
        assert_eq!(output, source);
        fork[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(reader(4, fork, source.len())
            .read_to_end(&mut Vec::new())
            .is_err());
    }

    #[test]
    fn chunked_fork_respects_cancel_and_output_limit() {
        let mut fork = Vec::new();
        fork.extend_from_slice(&8u32.to_le_bytes());
        fork.extend_from_slice(&12u32.to_le_bytes());
        fork.extend_from_slice(b"test");
        let mut stream = reader(10, fork.clone(), 4);
        stream.limit = 3;
        assert!(stream.read_to_end(&mut Vec::new()).is_err());
        let mut stream = reader(10, fork, 4);
        stream.disk.cancel.cancel();
        assert!(stream.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn resource_fork_rejects_invalid_chunk_table_and_ratio() {
        let mut fork = Vec::new();
        fork.extend_from_slice(&8u32.to_le_bytes());
        fork.extend_from_slice(&7u32.to_le_bytes());
        assert!(reader(8, fork, 4).read_to_end(&mut Vec::new()).is_err());

        let mut stream = reader(3, zlib(&vec![0; CHUNK]), CHUNK);
        stream.disk.max_ratio = 2;
        stream.disk.ratio_above = 1;
        assert!(stream.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn lzfse_resource_fork_decodes_multiple_chunks() {
        let source: Vec<u8> = (0..CHUNK + 2700).map(|n| (n % 239) as u8).collect();
        let chunks: Vec<Vec<u8>> = source
            .chunks(CHUNK)
            .map(|part| {
                let mut encoded = Vec::new();
                lzfse_rust::encode_bytes(part, &mut encoded).unwrap();
                encoded
            })
            .collect();
        let mut fork = vec![0u8; 4 * (chunks.len() + 1)];
        let header = fork.len() as u32;
        fork[..4].copy_from_slice(&header.to_le_bytes());
        for (index, chunk) in chunks.iter().enumerate() {
            fork.extend_from_slice(chunk);
            let end = fork.len() as u32;
            fork[4 + index * 4..8 + index * 4].copy_from_slice(&end.to_le_bytes());
        }
        let mut output = Vec::new();
        reader(12, fork, source.len())
            .read_to_end(&mut output)
            .unwrap();
        assert_eq!(output, source);
    }

    #[test]
    fn scans_and_reads_real_macos_lzvn_resource_fork() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/apfs_compressed.bin");
        let file = File::open(path).unwrap();
        let size = file.metadata().unwrap().len();
        let disk = Disk {
            file,
            runs: vec![Run {
                start: 0,
                size,
                offset: 0,
                packed: size,
                kind: 1,
            }],
            partitions: Vec::new(),
            data_checksum: None,
            size,
            cancel: CancelToken::new(),
            cache: None,
            max_ratio: 1000,
            ratio_above: 1 << 20,
        };
        let cancel = CancelToken::new();
        let records = scan_apfs(disk, 0, size, &cancel, &mut NoProgress, u64::MAX).unwrap();
        let record = records
            .iter()
            .find(|r| r.entry.path == "compressed.txt")
            .unwrap();
        assert_eq!(record.entry.size, 180_000);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/apfs_compressed.bin");
        let disk = Disk {
            file: File::open(path).unwrap(),
            runs: vec![Run {
                start: 0,
                size,
                offset: 0,
                packed: size,
                kind: 1,
            }],
            partitions: Vec::new(),
            data_checksum: None,
            size,
            cancel,
            cache: None,
            max_ratio: 1000,
            ratio_above: 1 << 20,
        };
        let mut stream = CompressedReader {
            disk,
            compression: record.compressed.clone().unwrap(),
            size: record.entry.size,
            position: 0,
            chunk_index: 0,
            chunk: Vec::new(),
            emitted: Arc::new(AtomicU64::new(0)),
            limit: u64::MAX,
        };
        let mut decoded = Vec::new();
        stream.read_to_end(&mut decoded).unwrap();
        assert_eq!(
            decoded,
            b"The quick brown fox jumps over the lazy dog. ".repeat(4000)
        );
    }
}
