use crate::zip_fmt::{map_err, unix_to_dos};
use crc32fast::Hasher;
use flate2::{Compress, Compression, FlushCompress, Status};
use rz_core::{Error, NewEntry, Result};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread;
use zip::ZipArchive;

const CHUNK_SIZE: usize = 900_000;
const MAX_IN_FLIGHT_PER_WORKER: usize = 2;

type Job = (u64, Vec<u8>);
type Done = (u64, io::Result<Vec<u8>>);

/// Builds a one-entry ZIP containing an independently reset Deflate stream.
/// The archive writer then merges its compressed bytes without recompressing them.
pub(crate) fn pack_entry(
    entry: &NewEntry,
    src: &mut dyn Read,
    level: u32,
    workers: usize,
    portable: bool,
) -> Result<ZipArchive<File>> {
    if workers < 2 {
        return Err(Error::Unsupported(
            "parallel Deflate needs at least two workers".into(),
        ));
    }
    let name = entry.name.as_bytes();
    let name_len = u16::try_from(name.len())
        .map_err(|_| Error::LimitExceeded("ZIP entry name exceeds 65535 bytes".into()))?;
    let mut archive = tempfile::tempfile()?;
    let (time, date) = entry
        .modified
        .and_then(unix_to_dos)
        .map(|dt| (dt.timepart(), dt.datepart()))
        .unwrap_or((0, 0x21));

    write_local_header(&mut archive, name, name_len, time, date)?;
    let data_offset = archive.stream_position()?;
    let (uncompressed_size, crc32) = compress_chunks(src, &mut archive, level, workers)?;
    let compressed_size = archive.stream_position()?.saturating_sub(data_offset);

    let end_of_data = archive.stream_position()?;
    archive.seek(SeekFrom::Start(14))?;
    archive.write_all(&crc32.to_le_bytes())?;
    archive.seek(SeekFrom::Start(30 + u64::from(name_len) + 4))?;
    archive.write_all(&uncompressed_size.to_le_bytes())?;
    archive.write_all(&compressed_size.to_le_bytes())?;
    archive.seek(SeekFrom::Start(end_of_data))?;

    write_central_directory(
        &mut archive,
        name,
        name_len,
        time,
        date,
        crc32,
        uncompressed_size,
        compressed_size,
        unix_mode(entry, portable),
    )?;
    archive.flush()?;
    archive.seek(SeekFrom::Start(0))?;
    ZipArchive::new(archive).map_err(map_err)
}

fn unix_mode(entry: &NewEntry, portable: bool) -> u32 {
    if portable {
        return 0o100644 << 16;
    }
    entry
        .unix_mode
        .map(|mode| (0o100000 | (mode & 0o777)) << 16)
        .unwrap_or(0)
}

fn write_local_header(
    output: &mut File,
    name: &[u8],
    name_len: u16,
    time: u16,
    date: u16,
) -> io::Result<()> {
    output.write_all(&0x0403_4b50u32.to_le_bytes())?;
    output.write_all(&45u16.to_le_bytes())?;
    output.write_all(&0x0800u16.to_le_bytes())?;
    output.write_all(&8u16.to_le_bytes())?;
    output.write_all(&time.to_le_bytes())?;
    output.write_all(&date.to_le_bytes())?;
    output.write_all(&0u32.to_le_bytes())?;
    output.write_all(&u32::MAX.to_le_bytes())?;
    output.write_all(&u32::MAX.to_le_bytes())?;
    output.write_all(&name_len.to_le_bytes())?;
    output.write_all(&20u16.to_le_bytes())?;
    output.write_all(name)?;
    output.write_all(&1u16.to_le_bytes())?;
    output.write_all(&16u16.to_le_bytes())?;
    output.write_all(&0u64.to_le_bytes())?;
    output.write_all(&0u64.to_le_bytes())
}

#[allow(clippy::too_many_arguments)]
fn write_central_directory(
    output: &mut File,
    name: &[u8],
    name_len: u16,
    time: u16,
    date: u16,
    crc32: u32,
    uncompressed_size: u64,
    compressed_size: u64,
    external_attributes: u32,
) -> io::Result<()> {
    let central_offset = output.stream_position()?;
    output.write_all(&0x0201_4b50u32.to_le_bytes())?;
    output.write_all(&0x032du16.to_le_bytes())?;
    output.write_all(&45u16.to_le_bytes())?;
    output.write_all(&0x0800u16.to_le_bytes())?;
    output.write_all(&8u16.to_le_bytes())?;
    output.write_all(&time.to_le_bytes())?;
    output.write_all(&date.to_le_bytes())?;
    output.write_all(&crc32.to_le_bytes())?;
    output.write_all(&u32::MAX.to_le_bytes())?;
    output.write_all(&u32::MAX.to_le_bytes())?;
    output.write_all(&name_len.to_le_bytes())?;
    output.write_all(&28u16.to_le_bytes())?;
    output.write_all(&0u16.to_le_bytes())?;
    output.write_all(&0u16.to_le_bytes())?;
    output.write_all(&0u16.to_le_bytes())?;
    output.write_all(&external_attributes.to_le_bytes())?;
    output.write_all(&u32::MAX.to_le_bytes())?;
    output.write_all(name)?;
    output.write_all(&1u16.to_le_bytes())?;
    output.write_all(&24u16.to_le_bytes())?;
    output.write_all(&uncompressed_size.to_le_bytes())?;
    output.write_all(&compressed_size.to_le_bytes())?;
    output.write_all(&0u64.to_le_bytes())?;
    let central_size = output.stream_position()?.saturating_sub(central_offset);

    let zip64_end_offset = output.stream_position()?;
    output.write_all(&0x0606_4b50u32.to_le_bytes())?;
    output.write_all(&44u64.to_le_bytes())?;
    output.write_all(&45u16.to_le_bytes())?;
    output.write_all(&45u16.to_le_bytes())?;
    output.write_all(&0u32.to_le_bytes())?;
    output.write_all(&0u32.to_le_bytes())?;
    output.write_all(&1u64.to_le_bytes())?;
    output.write_all(&1u64.to_le_bytes())?;
    output.write_all(&central_size.to_le_bytes())?;
    output.write_all(&central_offset.to_le_bytes())?;
    output.write_all(&0x0706_4b50u32.to_le_bytes())?;
    output.write_all(&0u32.to_le_bytes())?;
    output.write_all(&zip64_end_offset.to_le_bytes())?;
    output.write_all(&1u32.to_le_bytes())?;
    output.write_all(&0x0605_4b50u32.to_le_bytes())?;
    output.write_all(&0u16.to_le_bytes())?;
    output.write_all(&0u16.to_le_bytes())?;
    output.write_all(&u16::MAX.to_le_bytes())?;
    output.write_all(&u16::MAX.to_le_bytes())?;
    output.write_all(&u32::MAX.to_le_bytes())?;
    output.write_all(&u32::MAX.to_le_bytes())?;
    output.write_all(&0u16.to_le_bytes())
}

fn compress_chunks(
    src: &mut dyn Read,
    output: &mut File,
    level: u32,
    workers: usize,
) -> Result<(u64, u32)> {
    let max_jobs = workers.saturating_mul(MAX_IN_FLIGHT_PER_WORKER).max(1);
    let (job_tx, job_rx) = mpsc::sync_channel::<Job>(max_jobs);
    let job_rx = Mutex::new(job_rx);
    let (done_tx, done_rx) = mpsc::channel::<Done>();
    let mut pending = BTreeMap::<u64, Vec<u8>>::new();
    let mut sent = 0u64;
    let mut written = 0u64;
    let mut outstanding = 0usize;
    let mut input = vec![0u8; CHUNK_SIZE];
    let mut total = 0u64;
    let mut crc = Hasher::new();

    let result = thread::scope(|scope| -> Result<()> {
        for _ in 0..workers {
            let receiver = &job_rx;
            let sender = done_tx.clone();
            scope.spawn(move || loop {
                let job = match receiver.lock() {
                    Ok(rx) => rx.recv(),
                    Err(_) => return,
                };
                let Ok((index, chunk)) = job else {
                    return;
                };
                let compressed = catch_unwind(AssertUnwindSafe(|| encode_chunk(&chunk, level)))
                    .unwrap_or_else(|_| Err(io::Error::other("Deflate worker failed")));
                if sender.send((index, compressed)).is_err() {
                    return;
                }
            });
        }
        drop(done_tx);

        let receive = |output: &mut File,
                       pending: &mut BTreeMap<u64, Vec<u8>>,
                       outstanding: &mut usize,
                       written: &mut u64|
         -> Result<()> {
            let (index, compressed) = done_rx
                .recv()
                .map_err(|_| Error::Corrupt("Deflate worker stopped".into()))?;
            *outstanding = outstanding.saturating_sub(1);
            pending.insert(index, compressed?);
            while let Some(chunk) = pending.remove(&*written) {
                output.write_all(&chunk)?;
                *written += 1;
            }
            Ok(())
        };

        let producer_result = (|| -> Result<()> {
            loop {
                let read = read_chunk(src, &mut input)?;
                if read == 0 {
                    break;
                }
                crc.update(&input[..read]);
                total = total
                    .checked_add(read as u64)
                    .ok_or_else(|| Error::LimitExceeded("ZIP entry size overflow".into()))?;
                while sent.saturating_sub(written) >= max_jobs as u64 {
                    receive(output, &mut pending, &mut outstanding, &mut written)?;
                }
                input.truncate(read);
                let chunk = std::mem::replace(&mut input, vec![0u8; CHUNK_SIZE]);
                job_tx
                    .send((sent, chunk))
                    .map_err(|_| Error::Corrupt("Deflate workers stopped".into()))?;
                sent += 1;
                outstanding += 1;
            }
            while written < sent {
                receive(output, &mut pending, &mut outstanding, &mut written)?;
            }
            output.write_all(&[0x03, 0x00])?;
            Ok(())
        })();
        drop(job_tx);
        producer_result
    });
    result?;
    Ok((total, crc.finalize()))
}

fn read_chunk(src: &mut dyn Read, buffer: &mut Vec<u8>) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match src.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    buffer.truncate(filled);
    Ok(filled)
}

fn encode_chunk(chunk: &[u8], level: u32) -> io::Result<Vec<u8>> {
    let mut encoder = Compress::new(Compression::new(level), false);
    let mut compressed = Vec::with_capacity(chunk.len() + 64 * 1024);
    loop {
        let input_offset = usize::try_from(encoder.total_in())
            .map_err(|_| io::Error::other("Deflate input offset overflow"))?;
        let previous_out = encoder.total_out();
        compressed.reserve(64 * 1024);
        let flush = if input_offset == chunk.len() {
            FlushCompress::Full
        } else {
            FlushCompress::None
        };
        let status = encoder
            .compress_vec(&chunk[input_offset..], &mut compressed, flush)
            .map_err(io::Error::other)?;
        let next_offset = usize::try_from(encoder.total_in())
            .map_err(|_| io::Error::other("Deflate input offset overflow"))?;
        let produced = encoder.total_out() - previous_out;
        if flush == FlushCompress::Full && next_offset == chunk.len() && produced == 0 {
            return match status {
                Status::Ok | Status::BufError => Ok(compressed),
                _ => Err(io::Error::other("unexpected Deflate flush status")),
            };
        }
        if input_offset == next_offset && produced == 0 {
            return Err(io::Error::other(format!(
                "Deflate encoder made no progress ({status:?})"
            )));
        }
    }
}

pub(crate) fn worker_count(threads: usize, memory_budget: u64) -> usize {
    threads.min(usize::try_from(memory_budget / (16 << 20)).unwrap_or(usize::MAX))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use flate2::read::DeflateDecoder;

    #[test]
    fn parallel_chunks_decode_to_the_original_bytes() {
        let mut state = 0x5eedu64.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let block: Vec<u8> = (0..1024 * 1024usize)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect();
        let source: Vec<u8> = block
            .iter()
            .copied()
            .cycle()
            .take(4 * 1024 * 1024)
            .collect();
        let mut compressed = tempfile::tempfile().unwrap();
        compress_chunks(&mut source.as_slice(), &mut compressed, 6, 4).unwrap();
        compressed.seek(SeekFrom::Start(0)).unwrap();
        let mut decoded = Vec::new();
        DeflateDecoder::new(compressed)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(
            decoded.len(),
            source.len(),
            "decoded length differs; decoded hash {:x}, source hash {:x}",
            crc32fast::hash(&decoded),
            crc32fast::hash(&source)
        );
        let mismatch = decoded
            .iter()
            .zip(&source)
            .position(|(decoded, source)| decoded != source);
        assert!(
            mismatch.is_none(),
            "first differing byte at {:?}; decoded CRC {:08x}, source CRC {:08x}",
            mismatch,
            crc32fast::hash(&decoded),
            crc32fast::hash(&source)
        );
    }
}
