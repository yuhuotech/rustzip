use rz_core::{CancelToken, Error, Progress, ProgressSink, Result};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct MultipartReader {
    files: Vec<File>,
    starts: Vec<u64>,
    sizes: Vec<u64>,
    length: u64,
    position: u64,
    hidden_prefix: u64,
    patches: BTreeMap<u64, u8>,
}

#[derive(Clone)]
struct ZipEntryLocation {
    record_start: u64,
    local_header_start: u64,
    zip64_offset: Option<u64>,
    zip64_disk: Option<u64>,
}

struct ZipLayout {
    length: u64,
    central_start: u64,
    entries_total: u64,
    entries: Vec<ZipEntryLocation>,
    protected: Vec<(u64, u64)>,
    terminal_start: u64,
    eocd: u64,
    zip64_locator: Option<Zip64Locator>,
    zip64_end: Option<Zip64End>,
}

impl MultipartReader {
    pub fn open(paths: &[PathBuf], hidden_prefix: u64, patches: BTreeMap<u64, u8>) -> Result<Self> {
        if paths.is_empty() {
            return Err(Error::Corrupt("archive has no volume files".into()));
        }
        let mut files = Vec::with_capacity(paths.len());
        let mut starts = Vec::with_capacity(paths.len());
        let mut sizes = Vec::with_capacity(paths.len());
        let mut length = 0u64;
        for path in paths {
            let file = File::open(path)?;
            let size = file.metadata()?.len();
            if size == 0 {
                return Err(Error::Corrupt("archive volume is empty".into()));
            }
            starts.push(length);
            sizes.push(size);
            length = length
                .checked_add(size)
                .ok_or_else(|| Error::LimitExceeded("volume set is too large".into()))?;
            files.push(file);
        }
        if hidden_prefix > length {
            return Err(Error::Corrupt(
                "volume set is shorter than its header".into(),
            ));
        }
        Ok(Self {
            files,
            starts,
            sizes,
            length: length - hidden_prefix,
            position: 0,
            hidden_prefix,
            patches,
        })
    }

    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        self.seek(SeekFrom::Start(offset))?;
        self.read(buf)
    }

    pub fn open_zip(paths: &[PathBuf]) -> Result<Self> {
        let mut raw = Self::open(paths, 0, BTreeMap::new())?;
        let patches = normalize_zip_volumes(&mut raw)?;
        Self::open(paths, 4, patches)
    }
}

pub fn split_sevenz<W, F>(
    source_path: &Path,
    volume_size: u64,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
    mut make_part: F,
) -> Result<Vec<W>>
where
    W: Write + Seek,
    F: FnMut() -> Result<W>,
{
    validate_volume_size(volume_size, None)?;
    let mut source = File::open(source_path)?;
    let length = source.metadata()?.len();
    if length <= volume_size {
        return Ok(Vec::new());
    }
    let count = length
        .checked_add(volume_size - 1)
        .ok_or_else(|| Error::LimitExceeded("too many 7z volumes".into()))?
        / volume_size;
    if count > 65_535 {
        return Err(Error::LimitExceeded("7z volume count exceeds 65535".into()));
    }
    let mut parts = Vec::with_capacity(count as usize);
    let mut buffer = vec![0; 256 * 1024];
    let mut copied = 0u64;
    while copied < length {
        cancel.check()?;
        let mut part = make_part()?;
        let target = volume_size.min(length - copied);
        let mut part_written = 0u64;
        while part_written < target {
            cancel.check()?;
            let request = (target - part_written).min(buffer.len() as u64) as usize;
            let n = source.read(&mut buffer[..request])?;
            if n == 0 {
                return Err(Error::Corrupt(
                    "7z archive ended while splitting volumes".into(),
                ));
            }
            part.write_all(&buffer[..n])?;
            copied += n as u64;
            part_written += n as u64;
            progress.update(Progress {
                bytes_done: copied,
                bytes_total: length,
                ..Progress::default()
            });
        }
        part.flush()?;
        parts.push(part);
    }
    Ok(parts)
}

pub fn split_zip<W, F>(
    source_path: &Path,
    volume_size: u64,
    cancel: &CancelToken,
    progress: &mut dyn ProgressSink,
    mut make_part: F,
) -> Result<Vec<W>>
where
    W: Write + Seek,
    F: FnMut() -> Result<W>,
{
    validate_volume_size(volume_size, Some(u64::from(u32::MAX)))?;
    let mut layout_reader =
        MultipartReader::open(&[source_path.to_path_buf()], 0, BTreeMap::new())?;
    let layout = zip_layout(&mut layout_reader, cancel)?;
    if layout
        .length
        .checked_add(4)
        .is_some_and(|length| length <= volume_size)
    {
        return Ok(Vec::new());
    }
    let boundaries = zip_split_boundaries(&layout, volume_size)?;
    let count = boundaries.len().saturating_sub(1);
    if count < 2 {
        return Err(Error::Corrupt(
            "ZIP split plan produced fewer than two volumes".into(),
        ));
    }
    if count > 65_535 {
        return Err(Error::LimitExceeded(
            "ZIP volume count exceeds 65535".into(),
        ));
    }
    let mut source = File::open(source_path)?;
    let mut parts = Vec::with_capacity(count);
    let mut buffer = vec![0; 256 * 1024];
    let mut copied = 0u64;
    for index in 0..count {
        cancel.check()?;
        let mut part = make_part()?;
        if index == 0 {
            part.write_all(&[0x50, 0x4b, 0x07, 0x08])?;
        }
        let mut position = boundaries[index];
        let end = boundaries[index + 1];
        source.seek(SeekFrom::Start(position))?;
        while position < end {
            cancel.check()?;
            let request = (end - position).min(buffer.len() as u64) as usize;
            let n = source.read(&mut buffer[..request])?;
            if n == 0 {
                return Err(Error::Corrupt(
                    "ZIP archive ended while splitting volumes".into(),
                ));
            }
            part.write_all(&buffer[..n])?;
            position += n as u64;
            copied += n as u64;
            progress.update(Progress {
                bytes_done: copied,
                bytes_total: layout.length,
                ..Progress::default()
            });
        }
        part.flush()?;
        parts.push(part);
    }
    patch_split_zip(&mut parts, &boundaries, &layout)?;
    Ok(parts)
}

fn validate_volume_size(size: u64, max: Option<u64>) -> Result<()> {
    if size < 64 * 1024 {
        return Err(Error::Unsupported(
            "volume size must be at least 64 KiB".into(),
        ));
    }
    if max.is_some_and(|max| size > max) {
        return Err(Error::Unsupported(
            "ZIP volume size must be below 4 GiB".into(),
        ));
    }
    Ok(())
}

fn zip_layout(reader: &mut MultipartReader, cancel: &CancelToken) -> Result<ZipLayout> {
    let (eocd, eocd_bytes) = find_eocd(reader)?;
    let eocd_disk = u64::from(read_u16(&eocd_bytes, 4)?);
    let eocd_cd_disk = u64::from(read_u16(&eocd_bytes, 6)?);
    let _eocd_entries_on_disk = u64::from(read_u16(&eocd_bytes, 8)?);
    let eocd_entries_total = u64::from(read_u16(&eocd_bytes, 10)?);
    let eocd_cd_size = u64::from(read_u32(&eocd_bytes, 12)?);
    let eocd_cd_offset = u64::from(read_u32(&eocd_bytes, 16)?);
    let zip64_locator = read_zip64_locator(reader, eocd)?;
    let zip64_end = zip64_locator
        .as_ref()
        .map(|locator| read_zip64_end(reader, locator))
        .transpose()?;
    let (cd_disk, entries_total, cd_size, cd_offset) = match &zip64_end {
        Some(end) => (
            end.cd_disk,
            end.entries_total,
            read_u64_at(reader, end.offset + 40)?,
            end.cd_offset,
        ),
        None => (
            eocd_cd_disk,
            eocd_entries_total,
            eocd_cd_size,
            eocd_cd_offset,
        ),
    };
    let disk_number = zip64_end.as_ref().map_or(eocd_disk, |end| end.disk_number);
    if disk_number != 0 || cd_disk != 0 {
        return Err(Error::Unsupported(
            "cannot split an already multi-disk ZIP archive".into(),
        ));
    }
    let cd_start = cd_offset;
    let cd_end = cd_start
        .checked_add(cd_size)
        .filter(|end| *end <= reader.length)
        .ok_or_else(|| Error::Corrupt("ZIP central directory is out of range".into()))?;
    let mut entries = Vec::new();
    let mut protected = Vec::new();
    let mut position = cd_start;
    for _ in 0..entries_total {
        cancel.check()?;
        let mut header = [0u8; 46];
        read_exact_at(reader, position, &mut header)?;
        if header[..4] != [0x50, 0x4b, 0x01, 0x02] {
            return Err(Error::Corrupt("invalid ZIP central directory entry".into()));
        }
        let name_len = u64::from(read_u16(&header, 28)?);
        let extra_len = u64::from(read_u16(&header, 30)?);
        let comment_len = u64::from(read_u16(&header, 32)?);
        let record_len = 46u64
            .checked_add(name_len)
            .and_then(|n| n.checked_add(extra_len))
            .and_then(|n| n.checked_add(comment_len))
            .ok_or_else(|| Error::Corrupt("ZIP central record is too large".into()))?;
        if position
            .checked_add(record_len)
            .is_none_or(|end| end > cd_end)
        {
            return Err(Error::Corrupt(
                "ZIP central record exceeds the directory".into(),
            ));
        }
        let disk_field = read_u16(&header, 34)?;
        let offset_field = read_u32(&header, 42)?;
        let extra_start = position + 46 + name_len;
        let extra = read_vec_at(reader, extra_start, extra_len)?;
        let extra64 = zip64_extra_offsets(
            &extra,
            offset_field == u32::MAX,
            disk_field == u16::MAX,
            read_u32(&header, 24)? == u32::MAX,
            read_u32(&header, 20)? == u32::MAX,
        )?;
        let local_disk = if disk_field == u16::MAX {
            extra64
                .disk
                .map(|at| read_u32(&extra, at))
                .transpose()?
                .ok_or_else(|| Error::Corrupt("ZIP64 entry has no disk number".into()))?
        } else {
            u32::from(disk_field)
        };
        if local_disk != 0 {
            return Err(Error::Unsupported(
                "cannot split an already multi-disk ZIP archive".into(),
            ));
        }
        let local_header_start = if offset_field == u32::MAX {
            extra64
                .offset
                .map(|at| read_u64(&extra, at))
                .transpose()?
                .ok_or_else(|| Error::Corrupt("ZIP64 entry has no local header offset".into()))?
        } else {
            u64::from(offset_field)
        };
        let mut local = [0u8; 30];
        read_exact_at(reader, local_header_start, &mut local)?;
        if local[..4] != [0x50, 0x4b, 0x03, 0x04] {
            return Err(Error::Corrupt("ZIP local header is missing".into()));
        }
        let local_name = u64::from(read_u16(&local, 26)?);
        let local_extra = u64::from(read_u16(&local, 28)?);
        let local_header_end = local_header_start
            .checked_add(30 + local_name + local_extra)
            .filter(|end| *end <= cd_start)
            .ok_or_else(|| Error::Corrupt("ZIP local header is out of range".into()))?;
        protected.push((local_header_start, local_header_end));
        protected.push((position, position + record_len));
        entries.push(ZipEntryLocation {
            record_start: position,
            local_header_start,
            zip64_offset: extra64.offset.map(|at| extra_start + at as u64),
            zip64_disk: extra64.disk.map(|at| extra_start + at as u64),
        });
        position = position
            .checked_add(record_len)
            .ok_or_else(|| Error::Corrupt("ZIP central directory offset overflow".into()))?;
    }
    if position < cd_end {
        protected.push((position, cd_end));
    }
    let terminal_start = zip64_end.as_ref().map_or(eocd, |record| record.offset);
    protected.push((terminal_start, reader.length));
    protected.sort_unstable_by_key(|range| range.0);
    Ok(ZipLayout {
        length: reader.length,
        central_start: cd_start,
        entries_total,
        entries,
        protected,
        terminal_start,
        eocd,
        zip64_locator,
        zip64_end,
    })
}

fn zip_split_boundaries(layout: &ZipLayout, volume_size: u64) -> Result<Vec<u64>> {
    let mut boundaries = vec![0];
    let mut start = 0u64;
    let mut first = true;
    let mut protected_index = 0usize;
    while start < layout.length {
        let capacity = if first { volume_size - 4 } else { volume_size };
        let limit = start
            .checked_add(capacity)
            .ok_or_else(|| Error::LimitExceeded("ZIP volume offset overflow".into()))?;
        let mut end = layout.length.min(limit);
        if end < layout.length && end > layout.terminal_start {
            end = layout.terminal_start;
        }
        loop {
            while protected_index < layout.protected.len()
                && layout.protected[protected_index].1 <= end
            {
                protected_index += 1;
            }
            let crossing = layout
                .protected
                .get(protected_index)
                .filter(|(from, to)| *from < end && end < *to);
            let Some((from, to)) = crossing else { break };
            if *from > start {
                end = *from;
            } else if *to - start <= capacity {
                end = *to;
            } else {
                return Err(Error::Unsupported(
                    "a ZIP record is larger than the requested volume size".into(),
                ));
            }
        }
        if end <= start || end - start > capacity {
            return Err(Error::Unsupported(
                "ZIP records cannot fit within the requested volume size".into(),
            ));
        }
        boundaries.push(end);
        start = end;
        first = false;
    }
    Ok(boundaries)
}

fn patch_split_zip<W: Write + Seek>(
    parts: &mut [W],
    boundaries: &[u64],
    layout: &ZipLayout,
) -> Result<()> {
    let count = parts.len();
    let last_disk = u32::try_from(count - 1)
        .map_err(|_| Error::LimitExceeded("ZIP volume count exceeds format limits".into()))?;
    for entry in &layout.entries {
        let (disk, offset) = split_location(boundaries, entry.local_header_start)?;
        let disk = u16::try_from(disk)
            .map_err(|_| Error::LimitExceeded("ZIP volume count exceeds format limits".into()))?;
        write_at(
            parts,
            boundaries,
            entry.record_start + 34,
            &disk.to_le_bytes(),
        )?;
        if let Some(zip64_offset) = entry.zip64_offset {
            write_at(parts, boundaries, zip64_offset, &offset.to_le_bytes())?;
        } else {
            let offset = u32::try_from(offset)
                .map_err(|_| Error::Unsupported("ZIP64 is required for this volume size".into()))?;
            write_at(
                parts,
                boundaries,
                entry.record_start + 42,
                &offset.to_le_bytes(),
            )?;
        }
        if let Some(zip64_disk) = entry.zip64_disk {
            write_at(
                parts,
                boundaries,
                zip64_disk,
                &u32::from(disk).to_le_bytes(),
            )?;
        }
    }

    let (cd_disk, cd_offset) = split_location(boundaries, layout.central_start)?;
    let cd_entries_here = layout
        .entries
        .iter()
        .filter(|entry| {
            split_location(boundaries, entry.record_start)
                .is_ok_and(|(d, _)| d == last_disk as usize)
        })
        .count() as u64;
    let total_entries = layout.entries_total;
    if let Some(zip64) = &layout.zip64_end {
        write_at(
            parts,
            boundaries,
            zip64.offset + 16,
            &last_disk.to_le_bytes(),
        )?;
        write_at(
            parts,
            boundaries,
            zip64.offset + 20,
            &(cd_disk as u32).to_le_bytes(),
        )?;
        write_at(
            parts,
            boundaries,
            zip64.offset + 24,
            &cd_entries_here.to_le_bytes(),
        )?;
        write_at(
            parts,
            boundaries,
            zip64.offset + 32,
            &total_entries.to_le_bytes(),
        )?;
        write_at(
            parts,
            boundaries,
            zip64.offset + 48,
            &cd_offset.to_le_bytes(),
        )?;
        if let Some(locator) = &layout.zip64_locator {
            let (disk, offset) = split_location(boundaries, zip64.offset)?;
            write_at(
                parts,
                boundaries,
                locator.offset + 4,
                &(disk as u32).to_le_bytes(),
            )?;
            write_at(parts, boundaries, locator.offset + 8, &offset.to_le_bytes())?;
            write_at(
                parts,
                boundaries,
                locator.offset + 16,
                &(count as u32).to_le_bytes(),
            )?;
        }
    }

    write_at(
        parts,
        boundaries,
        layout.eocd + 4,
        &u16::try_from(last_disk).unwrap_or(u16::MAX).to_le_bytes(),
    )?;
    write_at(
        parts,
        boundaries,
        layout.eocd + 6,
        &u16::try_from(cd_disk).unwrap_or(u16::MAX).to_le_bytes(),
    )?;
    write_at(
        parts,
        boundaries,
        layout.eocd + 8,
        &u16::try_from(cd_entries_here)
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    )?;
    write_at(
        parts,
        boundaries,
        layout.eocd + 10,
        &u16::try_from(total_entries)
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    )?;
    write_at(
        parts,
        boundaries,
        layout.eocd + 16,
        &u32::try_from(cd_offset).unwrap_or(u32::MAX).to_le_bytes(),
    )?;
    Ok(())
}

fn split_location(boundaries: &[u64], position: u64) -> Result<(usize, u64)> {
    let index = boundaries
        .partition_point(|start| *start <= position)
        .saturating_sub(1)
        .min(boundaries.len().saturating_sub(2));
    let offset = position
        .checked_sub(boundaries[index])
        .and_then(|n| n.checked_add(u64::from(index == 0) * 4))
        .ok_or_else(|| Error::Corrupt("ZIP split offset overflow".into()))?;
    Ok((index, offset))
}

fn write_at<W: Write + Seek>(
    parts: &mut [W],
    boundaries: &[u64],
    position: u64,
    bytes: &[u8],
) -> Result<()> {
    let (index, offset) = split_location(boundaries, position)?;
    let part = parts
        .get_mut(index)
        .ok_or_else(|| Error::Corrupt("ZIP split record is outside the volume set".into()))?;
    part.seek(SeekFrom::Start(offset))?;
    part.write_all(bytes)?;
    Ok(())
}

fn normalize_zip_volumes(reader: &mut MultipartReader) -> Result<BTreeMap<u64, u8>> {
    if reader.read_at(0, &mut [0; 4])? != 4 {
        return Err(Error::Corrupt(
            "split ZIP is missing its spanning marker".into(),
        ));
    }
    let mut marker = [0u8; 4];
    reader.read_at(0, &mut marker)?;
    if marker != [0x50, 0x4b, 0x07, 0x08] {
        return Err(Error::Corrupt(
            "split ZIP is missing its spanning marker".into(),
        ));
    }
    let (eocd, eocd_bytes) = find_eocd(reader)?;
    let disk_number = u64::from(u16::from_le_bytes([eocd_bytes[4], eocd_bytes[5]]));
    let mut cd_disk = u64::from(u16::from_le_bytes([eocd_bytes[6], eocd_bytes[7]]));
    let mut entries_on_disk = u64::from(u16::from_le_bytes([eocd_bytes[8], eocd_bytes[9]]));
    let mut entries_total = u64::from(u16::from_le_bytes([eocd_bytes[10], eocd_bytes[11]]));
    let mut last_disk = disk_number;
    let mut cd_offset = u64::from(u32::from_le_bytes([
        eocd_bytes[16],
        eocd_bytes[17],
        eocd_bytes[18],
        eocd_bytes[19],
    ]));
    let zip64 = read_zip64_locator(reader, eocd)?;
    if let Some(locator) = &zip64 {
        let record = read_zip64_end(reader, locator)?;
        last_disk = record.disk_number;
        cd_disk = record.cd_disk;
        entries_on_disk = record.entries_on_disk;
        entries_total = record.entries_total;
        cd_offset = record.cd_offset;
    }
    if last_disk + 1 != reader.files.len() as u64
        || zip64
            .as_ref()
            .is_some_and(|locator| locator.total_disks as usize != reader.files.len())
    {
        return Err(Error::Corrupt(
            "split ZIP volume count does not match its end record".into(),
        ));
    }
    if cd_disk >= reader.files.len() as u64 {
        return Err(Error::Corrupt(
            "split ZIP central directory volume is missing".into(),
        ));
    }
    let cd_start = reader.starts[cd_disk as usize]
        .checked_add(cd_offset)
        .ok_or_else(|| Error::Corrupt("split ZIP central directory offset overflow".into()))?;
    let mut patches = BTreeMap::new();
    let mut record_offset = cd_start;
    for _ in 0..entries_total {
        let mut header = [0u8; 46];
        read_exact_at(reader, record_offset, &mut header)?;
        if header[..4] != [0x50, 0x4b, 0x01, 0x02] {
            return Err(Error::Corrupt(
                "invalid split ZIP central directory entry".into(),
            ));
        }
        let name_len = u64::from(u16::from_le_bytes([header[28], header[29]]));
        let extra_len = u64::from(u16::from_le_bytes([header[30], header[31]]));
        let comment_len = u64::from(u16::from_le_bytes([header[32], header[33]]));
        let record_len = 46u64
            .checked_add(name_len)
            .and_then(|n| n.checked_add(extra_len))
            .and_then(|n| n.checked_add(comment_len))
            .ok_or_else(|| Error::Corrupt("split ZIP directory entry is too large".into()))?;
        let disk_field = u16::from_le_bytes([header[34], header[35]]);
        let offset_field = u32::from_le_bytes([header[42], header[43], header[44], header[45]]);
        let extra_start = record_offset + 46 + name_len;
        let extra = read_vec_at(reader, extra_start, extra_len)?;
        let extra64 = zip64_extra_offsets(
            &extra,
            offset_field == u32::MAX,
            disk_field == u16::MAX,
            u32::from_le_bytes([header[24], header[25], header[26], header[27]]) == u32::MAX,
            u32::from_le_bytes([header[20], header[21], header[22], header[23]]) == u32::MAX,
        )?;
        let local_disk = if disk_field == u16::MAX {
            extra64
                .disk
                .map(|at| read_u32(&extra, at))
                .transpose()?
                .ok_or_else(|| Error::Corrupt("split ZIP entry has no ZIP64 disk number".into()))?
                as u64
        } else {
            u64::from(disk_field)
        };
        let local_offset = if offset_field == u32::MAX {
            extra64
                .offset
                .map(|at| read_u64(&extra, at))
                .transpose()?
                .ok_or_else(|| Error::Corrupt("split ZIP entry has no ZIP64 local offset".into()))?
        } else {
            u64::from(offset_field)
        };
        if local_disk >= reader.files.len() as u64 {
            return Err(Error::Corrupt(
                "split ZIP entry starts on a missing volume".into(),
            ));
        }
        let local_physical = reader.starts[local_disk as usize]
            .checked_add(local_offset)
            .ok_or_else(|| Error::Corrupt("split ZIP local header offset overflow".into()))?;
        let local_normalized = local_physical.checked_sub(4).ok_or_else(|| {
            Error::Corrupt("split ZIP local header points before its marker".into())
        })?;
        patch_u16(&mut patches, record_offset + 34, 0)?;
        if offset_field == u32::MAX {
            let at = extra64.offset.ok_or_else(|| {
                Error::Corrupt("split ZIP entry has no ZIP64 local offset".into())
            })?;
            patch_u64(&mut patches, extra_start + at as u64, local_normalized)?;
        } else {
            let normalized = u32::try_from(local_normalized)
                .map_err(|_| Error::Unsupported("ZIP64 split offsets are required".into()))?;
            patch_u32(&mut patches, record_offset + 42, normalized)?;
        }
        if disk_field == u16::MAX {
            let at = extra64
                .disk
                .ok_or_else(|| Error::Corrupt("split ZIP entry has no ZIP64 disk number".into()))?;
            patch_u32(&mut patches, extra_start + at as u64, 0)?;
        }
        record_offset = record_offset
            .checked_add(record_len)
            .ok_or_else(|| Error::Corrupt("split ZIP directory offset overflow".into()))?;
    }
    let cd_logical = cd_start
        .checked_sub(4)
        .ok_or_else(|| Error::Corrupt("split ZIP central directory precedes its marker".into()))?;
    patch_u16(&mut patches, eocd + 4, 0)?;
    patch_u16(&mut patches, eocd + 6, 0)?;
    patch_u16(
        &mut patches,
        eocd + 8,
        u16::try_from(entries_total).unwrap_or(u16::MAX),
    )?;
    if zip64.is_some() {
        patch_u32(&mut patches, eocd + 16, u32::MAX)?;
    } else {
        patch_u32(
            &mut patches,
            eocd + 16,
            u32::try_from(cd_logical)
                .map_err(|_| Error::Unsupported("ZIP64 split offsets are required".into()))?,
        )?;
    }
    if let Some(locator) = zip64 {
        let record = read_zip64_end(reader, &locator)?;
        let offset = record.offset;
        patch_u32(&mut patches, offset + 16, 0)?;
        patch_u32(&mut patches, offset + 20, 0)?;
        patch_u64(&mut patches, offset + 24, entries_total)?;
        patch_u64(&mut patches, offset + 48, cd_logical)?;
        patch_u32(&mut patches, locator.offset + 4, 0)?;
        patch_u64(&mut patches, locator.offset + 8, offset - 4)?;
        patch_u32(&mut patches, locator.offset + 16, 1)?;
    }
    if entries_on_disk > entries_total {
        return Err(Error::Corrupt(
            "split ZIP entry count is inconsistent".into(),
        ));
    }
    Ok(patches)
}

struct Zip64Locator {
    offset: u64,
    disk: u32,
    relative_offset: u64,
    total_disks: u32,
}

struct Zip64End {
    offset: u64,
    disk_number: u64,
    cd_disk: u64,
    entries_on_disk: u64,
    entries_total: u64,
    cd_offset: u64,
}

fn read_zip64_locator(reader: &mut MultipartReader, eocd: u64) -> Result<Option<Zip64Locator>> {
    if eocd < 20 {
        return Ok(None);
    }
    let mut bytes = [0u8; 20];
    read_exact_at(reader, eocd - 20, &mut bytes)?;
    if bytes[..4] != [0x50, 0x4b, 0x06, 0x07] {
        return Ok(None);
    }
    Ok(Some(Zip64Locator {
        offset: eocd - 20,
        disk: read_u32(&bytes, 4)?,
        relative_offset: read_u64(&bytes, 8)?,
        total_disks: read_u32(&bytes, 16)?,
    }))
}

fn read_zip64_end(reader: &mut MultipartReader, locator: &Zip64Locator) -> Result<Zip64End> {
    let disk = locator.disk as usize;
    if disk >= reader.files.len() {
        return Err(Error::Corrupt(
            "split ZIP64 record volume is missing".into(),
        ));
    }
    let offset = reader.starts[disk]
        .checked_add(locator.relative_offset)
        .ok_or_else(|| Error::Corrupt("split ZIP64 record offset overflow".into()))?;
    let mut bytes = [0u8; 56];
    read_exact_at(reader, offset, &mut bytes)?;
    if bytes[..4] != [0x50, 0x4b, 0x06, 0x06] {
        return Err(Error::Corrupt("split ZIP64 end record is missing".into()));
    }
    Ok(Zip64End {
        offset,
        disk_number: u64::from(read_u32(&bytes, 16)?),
        cd_disk: u64::from(read_u32(&bytes, 20)?),
        entries_on_disk: read_u64(&bytes, 24)?,
        entries_total: read_u64(&bytes, 32)?,
        cd_offset: read_u64(&bytes, 48)?,
    })
}

struct Zip64ExtraOffsets {
    offset: Option<usize>,
    disk: Option<usize>,
}

fn zip64_extra_offsets(
    extra: &[u8],
    has_offset: bool,
    has_disk: bool,
    has_uncompressed: bool,
    has_compressed: bool,
) -> Result<Zip64ExtraOffsets> {
    let mut offset = 0usize;
    while offset + 4 <= extra.len() {
        let id = read_u16(extra, offset)?;
        let size = usize::from(read_u16(extra, offset + 2)?);
        let data = offset + 4;
        let end = data
            .checked_add(size)
            .filter(|end| *end <= extra.len())
            .ok_or_else(|| Error::Corrupt("truncated ZIP extra field".into()))?;
        if id == 1 {
            let mut cursor = data;
            if has_uncompressed {
                cursor += 8;
            }
            if has_compressed {
                cursor += 8;
            }
            let local_offset = has_offset.then_some(cursor);
            if has_offset {
                cursor += 8;
            }
            let disk_number = has_disk.then_some(cursor);
            if has_disk {
                cursor += 4;
            }
            if cursor > end {
                return Err(Error::Corrupt("truncated ZIP64 extra field".into()));
            }
            return Ok(Zip64ExtraOffsets {
                offset: local_offset,
                disk: disk_number,
            });
        }
        offset = end;
    }
    Ok(Zip64ExtraOffsets {
        offset: None,
        disk: None,
    })
}

fn find_eocd(reader: &mut MultipartReader) -> Result<(u64, [u8; 22])> {
    let tail_len = reader.length.min(65_557);
    let tail_start = reader.length - tail_len;
    let mut tail = vec![0; tail_len as usize];
    read_exact_at(reader, tail_start, &mut tail)?;
    for offset in (0..=tail.len().saturating_sub(22)).rev() {
        if tail.get(offset..offset + 4) != Some(&[0x50, 0x4b, 0x05, 0x06]) {
            continue;
        }
        let comment = usize::from(read_u16(&tail, offset + 20)?);
        if offset + 22 + comment != tail.len() {
            continue;
        }
        let record: [u8; 22] = tail[offset..offset + 22]
            .try_into()
            .map_err(|_| Error::Corrupt("truncated ZIP end record".into()))?;
        return Ok((tail_start + offset as u64, record));
    }
    Err(Error::Corrupt("ZIP end record is missing".into()))
}

fn read_exact_at(reader: &mut MultipartReader, offset: u64, buf: &mut [u8]) -> Result<()> {
    reader
        .read_at(offset, buf)
        .and_then(|n| {
            if n == buf.len() {
                Ok(())
            } else {
                Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
            }
        })
        .map_err(Error::Io)
}

fn read_vec_at(reader: &mut MultipartReader, offset: u64, len: u64) -> Result<Vec<u8>> {
    let len = usize::try_from(len)
        .map_err(|_| Error::LimitExceeded("ZIP metadata is too large".into()))?;
    let mut bytes = vec![0; len];
    read_exact_at(reader, offset, &mut bytes)?;
    Ok(bytes)
}

fn patch_bytes(patches: &mut BTreeMap<u64, u8>, physical: u64, bytes: &[u8]) -> Result<()> {
    let logical = physical
        .checked_sub(4)
        .ok_or_else(|| Error::Corrupt("ZIP patch precedes spanning marker".into()))?;
    for (index, byte) in bytes.iter().enumerate() {
        let at = logical
            .checked_add(index as u64)
            .ok_or_else(|| Error::Corrupt("ZIP patch offset overflow".into()))?;
        patches.insert(at, *byte);
    }
    Ok(())
}

fn patch_u16(patches: &mut BTreeMap<u64, u8>, at: u64, value: u16) -> Result<()> {
    patch_bytes(patches, at, &value.to_le_bytes())
}

fn patch_u32(patches: &mut BTreeMap<u64, u8>, at: u64, value: u32) -> Result<()> {
    patch_bytes(patches, at, &value.to_le_bytes())
}

fn patch_u64(patches: &mut BTreeMap<u64, u8>, at: u64, value: u64) -> Result<()> {
    patch_bytes(patches, at, &value.to_le_bytes())
}

impl Read for MultipartReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.position >= self.length {
            return Ok(0);
        }
        let wanted = (self.length - self.position).min(buf.len() as u64) as usize;
        let mut copied = 0usize;
        while copied < wanted {
            let physical = self
                .position
                .checked_add(self.hidden_prefix)
                .ok_or_else(|| std::io::Error::other("volume offset overflow"))?;
            let index = self
                .starts
                .partition_point(|start| *start <= physical)
                .saturating_sub(1);
            let part_start = self.starts[index];
            let part_size = self.sizes[index];
            let within = physical - part_start;
            let available = part_size.saturating_sub(within);
            if available == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
            }
            let count = available.min((wanted - copied) as u64) as usize;
            self.files[index].seek(SeekFrom::Start(within))?;
            let n = self.files[index].read(&mut buf[copied..copied + count])?;
            if n == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
            }
            copied += n;
            self.position += n as u64;
        }
        let end = self.position;
        for (at, value) in self.patches.range((end - copied as u64)..end) {
            let index = (*at - (end - copied as u64)) as usize;
            if index < copied {
                buf[index] = *value;
            }
        }
        Ok(copied)
    }
}

impl Seek for MultipartReader {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        let next = match from {
            SeekFrom::Start(at) => i128::from(at),
            SeekFrom::Current(by) => i128::from(self.position) + i128::from(by),
            SeekFrom::End(by) => i128::from(self.length) + i128::from(by),
        };
        if next < 0 || next > i128::from(u64::MAX) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek outside the volume set",
            ));
        }
        self.position = next as u64;
        Ok(self.position)
    }
}

pub fn zip_volume_paths(path: &Path) -> Result<Option<Vec<PathBuf>>> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let lower = name.to_ascii_lowercase();
    let final_path = if lower.ends_with(".zip") {
        path.to_path_buf()
    } else if is_zip_part_extension(path.extension().and_then(|e| e.to_str())) {
        path.with_extension("zip")
    } else {
        return Ok(None);
    };
    let first = with_zip_part(&final_path, 1)?;
    if !first.is_file() {
        if final_path != path && path.is_file() {
            return Err(Error::Corrupt("ZIP volume 1 (.z01) is missing".into()));
        }
        return match zip_volume_count(&final_path) {
            Ok(count) if count > 1 => Err(Error::Corrupt("ZIP volume 1 (.z01) is missing".into())),
            _ => Ok(None),
        };
    }
    if !final_path.is_file() {
        return Err(Error::Corrupt("last ZIP volume (.zip) is missing".into()));
    }
    let count = zip_volume_count(&final_path)?;
    if count < 2 {
        if final_path != path && path.is_file() {
            return Err(Error::Corrupt(
                "split ZIP has an invalid volume count".into(),
            ));
        }
        return Ok(None);
    }
    let mut paths = Vec::with_capacity(count);
    for index in 1..count {
        let part = with_zip_part(&final_path, index)?;
        if !part.is_file() {
            return Err(Error::Corrupt(format!("ZIP volume {index:02} is missing")));
        }
        paths.push(part);
    }
    paths.push(final_path);
    Ok(Some(paths))
}

pub fn sevenz_volume_paths(path: &Path) -> Result<Option<Vec<PathBuf>>> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let lower = name.to_ascii_lowercase();
    let (base, requested_index) = if lower.ends_with(".7z") {
        (path.to_path_buf(), None)
    } else if let Some(marker) = lower.rfind(".7z.") {
        let suffix = &lower[marker + 4..];
        if suffix.len() != 3 || !suffix.bytes().all(|b| b.is_ascii_digit()) {
            return Ok(None);
        }
        let index = suffix
            .parse::<u32>()
            .map_err(|_| Error::Corrupt("invalid 7z volume number".into()))?;
        (path.with_file_name(&name[..name.len() - 4]), Some(index))
    } else {
        return Ok(None);
    };
    let first = with_sevenz_part(&base, 1)?;
    if !first.is_file() {
        if requested_index.is_some_and(|index| index != 1) {
            return Err(Error::Corrupt("7z volume 1 (.001) is missing".into()));
        }
        return Ok(None);
    }
    let mut paths = Vec::new();
    for index in 1u32..=65_535 {
        let part = with_sevenz_part(&base, index)?;
        if !part.is_file() {
            let later = with_sevenz_part(&base, index + 1)?;
            if later.is_file() {
                return Err(Error::Corrupt(format!("7z volume {index:03} is missing")));
            }
            break;
        }
        paths.push(part);
    }
    Ok(Some(paths))
}

fn with_sevenz_part(base: &Path, index: u32) -> Result<PathBuf> {
    let mut name = base
        .file_name()
        .ok_or_else(|| Error::Corrupt("invalid 7z volume name".into()))?
        .to_os_string();
    name.push(format!(".{index:03}"));
    Ok(base.with_file_name(name))
}

fn is_zip_part_extension(extension: Option<&str>) -> bool {
    extension.is_some_and(|e| {
        let e = e.to_ascii_lowercase();
        e.strip_prefix('z')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn with_zip_part(final_path: &Path, index: usize) -> Result<PathBuf> {
    let stem = final_path
        .file_stem()
        .ok_or_else(|| Error::Corrupt("invalid ZIP volume name".into()))?;
    let mut name = stem.to_os_string();
    name.push(format!(".z{index:02}"));
    Ok(final_path.with_file_name(name))
}

fn zip_volume_count(final_path: &Path) -> Result<usize> {
    let mut file = File::open(final_path)?;
    let length = file.metadata()?.len();
    let tail_len = length.min(65_557) as usize;
    file.seek(SeekFrom::End(-(tail_len as i64)))?;
    let mut tail = vec![0u8; tail_len];
    file.read_exact(&mut tail)?;
    let eocd = find_eocd_in_tail(&tail)?;
    let disk = read_u16(&tail, eocd + 4)?;
    if disk != u16::MAX {
        return Ok(usize::from(disk) + 1);
    }
    let locator = eocd
        .checked_sub(20)
        .ok_or_else(|| Error::Corrupt("ZIP64 locator is missing".into()))?;
    if tail.get(locator..locator + 4) != Some(&[0x50, 0x4b, 0x06, 0x07]) {
        return Err(Error::Corrupt("ZIP64 locator is missing".into()));
    }
    Ok(read_u32(&tail, locator + 16)? as usize)
}

fn find_eocd_in_tail(tail: &[u8]) -> Result<usize> {
    if tail.len() < 22 {
        return Err(Error::Corrupt("ZIP end record is missing".into()));
    }
    for offset in (0..=tail.len() - 22).rev() {
        if tail.get(offset..offset + 4) != Some(&[0x50, 0x4b, 0x05, 0x06]) {
            continue;
        }
        let comment_len = usize::from(read_u16(tail, offset + 20)?);
        if offset + 22 + comment_len == tail.len() {
            return Ok(offset);
        }
    }
    Err(Error::Corrupt("ZIP end record is missing".into()))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let raw: [u8; 2] = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| Error::Corrupt("truncated ZIP volume record".into()))?
        .try_into()
        .map_err(|_| Error::Corrupt("truncated ZIP volume record".into()))?;
    Ok(u16::from_le_bytes(raw))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let raw: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| Error::Corrupt("truncated ZIP volume record".into()))?
        .try_into()
        .map_err(|_| Error::Corrupt("truncated ZIP volume record".into()))?;
    Ok(u32::from_le_bytes(raw))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64> {
    let raw: [u8; 8] = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| Error::Corrupt("truncated ZIP volume record".into()))?
        .try_into()
        .map_err(|_| Error::Corrupt("truncated ZIP volume record".into()))?;
    Ok(u64::from_le_bytes(raw))
}

fn read_u64_at(reader: &mut MultipartReader, offset: u64) -> Result<u64> {
    let mut bytes = [0u8; 8];
    read_exact_at(reader, offset, &mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
