use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

const CHUNK: usize = 900_000;
const WORKER_MEMORY: u64 = 16 << 20;
const DECODE_WORKER_MEMORY: u64 = 64 << 20;
const DECODE_FIXED_MEMORY: u64 = 16 << 20;

type Job = (u64, Vec<u8>);
type Done = (u64, io::Result<Vec<u8>>);

#[derive(Clone, Copy)]
enum Codec {
    Bzip2(u32),
    Gzip(u32),
}

impl Codec {
    fn encode(self, input: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::Bzip2(level) => {
                let mut encoder =
                    bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(level));
                encoder.write_all(input)?;
                encoder.finish()
            }
            Self::Gzip(level) => {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(level));
                encoder.write_all(input)?;
                encoder.finish()
            }
        }
    }
}

pub(crate) fn worker_count(threads: usize, budget: u64) -> usize {
    threads.min(usize::try_from(budget / WORKER_MEMORY).unwrap_or(usize::MAX))
}

pub(crate) fn decode_worker_count(threads: usize, budget: u64) -> usize {
    let available = budget
        .saturating_sub(DECODE_FIXED_MEMORY)
        .checked_div(DECODE_WORKER_MEMORY)
        .unwrap_or(0);
    threads.min(usize::try_from(available).unwrap_or(usize::MAX))
}

pub(crate) struct ParallelBzip2Decoder {
    inner: pbz2::ParallelDecoder,
    checked_end: bool,
}

impl ParallelBzip2Decoder {
    pub(crate) fn new(input: impl Read + Send + 'static, threads: usize) -> Self {
        Self {
            inner: pbz2::ParallelDecoder::with_threads(input, threads),
            checked_end: false,
        }
    }
}

impl Read for ParallelBzip2Decoder {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let read = self.inner.read(output)?;
        if read == 0 && !self.checked_end {
            self.checked_end = true;
            if self.inner.has_trailing_bytes() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "trailing bytes after the final bzip2 stream",
                ));
            }
        }
        Ok(read)
    }
}

/// Encodes bounded input chunks independently, then writes their streams in order.
struct ParallelChunks<W: Write> {
    out: Option<W>,
    input: Vec<u8>,
    jobs: Option<mpsc::Sender<Job>>,
    done: mpsc::Receiver<Done>,
    workers: Vec<JoinHandle<()>>,
    pending: BTreeMap<u64, Vec<u8>>,
    sent: u64,
    written: u64,
    outstanding: usize,
    max_jobs: usize,
}

impl<W: Write> ParallelChunks<W> {
    fn new(out: W, codec: Codec, workers: usize) -> io::Result<Self> {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done_rx) = mpsc::channel::<Done>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let jobs = Arc::clone(&job_rx);
            let done = done_tx.clone();
            let spawned = thread::Builder::new().spawn(move || loop {
                let job = match jobs.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => break,
                };
                let Ok((index, input)) = job else {
                    break;
                };
                let result = std::panic::catch_unwind(|| codec.encode(&input))
                    .unwrap_or_else(|_| Err(io::Error::other("compression worker failed")));
                if done.send((index, result)).is_err() {
                    break;
                }
            });
            match spawned {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    drop(job_tx);
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(error);
                }
            }
        }
        drop(done_tx);
        Ok(Self {
            out: Some(out),
            input: Vec::with_capacity(CHUNK),
            jobs: Some(job_tx),
            done: done_rx,
            workers: handles,
            pending: BTreeMap::new(),
            sent: 0,
            written: 0,
            outstanding: 0,
            max_jobs: workers.saturating_mul(2),
        })
    }

    fn receive(&mut self) -> io::Result<()> {
        let (index, result) = self
            .done
            .recv()
            .map_err(|_| io::Error::other("compression worker stopped"))?;
        self.outstanding -= 1;
        self.pending.insert(index, result?);
        while let Some(data) = self.pending.remove(&self.written) {
            self.out
                .as_mut()
                .ok_or_else(|| io::Error::other("compression writer is closed"))?
                .write_all(&data)?;
            self.written += 1;
        }
        Ok(())
    }

    fn submit(&mut self) -> io::Result<()> {
        while self.outstanding + self.pending.len() >= self.max_jobs {
            self.receive()?;
        }
        let input = std::mem::replace(&mut self.input, Vec::with_capacity(CHUNK));
        self.jobs
            .as_ref()
            .ok_or_else(|| io::Error::other("compression writer is closed"))?
            .send((self.sent, input))
            .map_err(|_| io::Error::other("compression workers stopped"))?;
        self.sent += 1;
        self.outstanding += 1;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        if !self.input.is_empty() || self.sent == 0 {
            self.submit()?;
        }
        while self.outstanding > 0 {
            self.receive()?;
        }
        self.stop_workers();
        self.out
            .take()
            .ok_or_else(|| io::Error::other("compression writer is closed"))
    }

    fn stop_workers(&mut self) {
        self.jobs.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl<W: Write> Write for ParallelChunks<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.input.len() == CHUNK {
            self.submit()?;
        }
        let count = data.len().min(CHUNK - self.input.len());
        self.input.extend_from_slice(&data[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.input.is_empty() {
            self.submit()?;
        }
        while self.outstanding > 0 {
            self.receive()?;
        }
        self.out
            .as_mut()
            .ok_or_else(|| io::Error::other("compression writer is closed"))?
            .flush()
    }
}

impl<W: Write> Drop for ParallelChunks<W> {
    fn drop(&mut self) {
        self.stop_workers();
    }
}

pub struct ParallelBzip2<W: Write>(ParallelChunks<W>);

impl<W: Write> ParallelBzip2<W> {
    pub fn new(out: W, level: u32, workers: usize) -> io::Result<Self> {
        ParallelChunks::new(out, Codec::Bzip2(level), workers).map(Self)
    }

    pub fn finish(self) -> io::Result<W> {
        self.0.finish()
    }
}

impl<W: Write> Write for ParallelBzip2<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.write(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

pub struct ParallelGzip<W: Write>(ParallelChunks<W>);

impl<W: Write> ParallelGzip<W> {
    pub fn new(out: W, level: u32, workers: usize) -> io::Result<Self> {
        ParallelChunks::new(out, Codec::Gzip(level), workers).map(Self)
    }

    pub fn finish(self) -> io::Result<W> {
        self.0.finish()
    }
}

impl<W: Write> Write for ParallelGzip<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.write(data)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("injected output failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn output_failure_stops_workers_without_deadlock() {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = ParallelBzip2::new(FailingWriter, 1, 2).and_then(|mut writer| {
                writer.write_all(&vec![42; CHUNK * 5])?;
                writer.finish().map(|_| ())
            });
            let _ = tx.send(result.is_err());
        });
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(10)),
            Ok(true)
        );
    }

    #[test]
    fn empty_and_flushed_streams_decode_in_original_order() -> io::Result<()> {
        for chunks in [
            vec![Vec::new()],
            vec![vec![1; CHUNK + 17], vec![2; CHUNK + 31], vec![3; 20]],
        ] {
            let mut writer = ParallelBzip2::new(Vec::new(), 1, 3)?;
            let mut expected = Vec::new();
            for chunk in chunks {
                writer.write_all(&chunk)?;
                writer.flush()?;
                expected.extend_from_slice(&chunk);
            }
            let bytes = writer.finish()?;
            let mut decoded = Vec::new();
            bzip2::read::MultiBzDecoder::new(bytes.as_slice()).read_to_end(&mut decoded)?;
            assert_eq!(decoded, expected);
        }
        Ok(())
    }

    #[test]
    fn gzip_chunks_decode_as_a_standard_multistream() -> io::Result<()> {
        for chunks in [
            vec![Vec::new()],
            vec![vec![1; CHUNK + 17], vec![2; CHUNK + 31], vec![3; 20]],
        ] {
            let mut writer = ParallelGzip::new(Vec::new(), 6, 3)?;
            let mut expected = Vec::new();
            for chunk in chunks {
                writer.write_all(&chunk)?;
                writer.flush()?;
                expected.extend_from_slice(&chunk);
            }
            let bytes = writer.finish()?;
            let mut decoded = Vec::new();
            flate2::read::MultiGzDecoder::new(bytes.as_slice()).read_to_end(&mut decoded)?;
            assert_eq!(decoded, expected);
        }
        Ok(())
    }
}
