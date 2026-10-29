use crate::parallel_deflate::{pack_entry, worker_count as deflate_worker_count};
use crate::zip_fmt::{map_err, unix_to_dos};
use rz_core::{resolve_threads, ArchiveWriter, CreateOptions, Error, FileSource, NewEntry, Result};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Cursor, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter};

/// Entries above this size stop participating in cross-entry worker batches.
/// Eligible unencrypted Deflate entries use the bounded chunked path instead.
const LARGE_FILE: u64 = 32 << 20;

#[derive(Clone)]
struct Settings {
    level: u32,
    password: Option<String>,
    portable: bool,
}

impl Settings {
    fn options(&self, e: &NewEntry) -> SimpleFileOptions {
        let mut o = SimpleFileOptions::default().large_file(e.size >= u64::from(u32::MAX));
        o = if self.level == 0 {
            o.compression_method(CompressionMethod::Stored)
        } else {
            o.compression_method(CompressionMethod::Deflated)
                .compression_level(Some(i64::from(self.level)))
        };
        if !self.portable {
            if let Some(mode) = e.unix_mode {
                o = o.unix_permissions(mode & 0o777);
            }
        }
        if let Some(dt) = e.modified.and_then(unix_to_dos) {
            o = o.last_modified_time(dt);
        }
        o
    }

    fn start<W: Write + std::io::Seek>(&self, w: &mut ZipWriter<W>, e: &NewEntry) -> Result<()> {
        let opts = self.options(e);
        match &self.password {
            Some(pw) => w.start_file(
                e.name.as_str(),
                opts.with_aes_encryption(AesMode::Aes256, pw),
            ),
            None => w.start_file(e.name.as_str(), opts),
        }
        .map_err(map_err)
    }
}

pub struct ZipOut {
    writer: ZipWriter<BufWriter<File>>,
    settings: Settings,
    threads: usize,
    memory_budget: u64,
}

impl ZipOut {
    pub fn new(file: File, opts: &CreateOptions) -> Self {
        Self {
            writer: ZipWriter::new(BufWriter::with_capacity(256 * 1024, file)),
            settings: Settings {
                level: opts.level.min(9),
                password: opts.password.clone().filter(|p| !p.is_empty()),
                portable: opts.portable,
            },
            threads: resolve_threads(opts.threads),
            memory_budget: opts.memory_budget,
        }
    }

    fn merge(&mut self, bytes: Vec<u8>) -> Result<()> {
        let archive = ZipArchive::new(Cursor::new(bytes)).map_err(map_err)?;
        self.writer.merge_archive(archive).map_err(map_err)
    }
}

/// Compresses one small file into an in-memory one-entry ZIP for ordered merging.
fn pack_one(settings: &Settings, e: &NewEntry, src: &mut dyn Read) -> Result<Vec<u8>> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    settings.start(&mut w, e)?;
    std::io::copy(src, &mut w)?;
    Ok(w.finish().map_err(map_err)?.into_inner())
}

struct Job<'a> {
    seq: u64,
    cost: u64,
    entry: NewEntry,
    src: Box<dyn Read + Send + 'a>,
}

type Done = (u64, u64, Result<Vec<u8>>);

fn receive_job<'a>(
    job_rx: &Mutex<mpsc::Receiver<Job<'a>>>,
    producer_done: &AtomicBool,
) -> Option<Job<'a>> {
    loop {
        let received = match job_rx.lock() {
            Ok(rx) => rx.recv_timeout(Duration::from_millis(10)),
            Err(_) => return None,
        };
        match received {
            Ok(job) => return Some(job),
            Err(mpsc::RecvTimeoutError::Timeout) if !producer_done.load(Ordering::Acquire) => {
                continue;
            }
            Err(_) => return None,
        }
    }
}

impl ArchiveWriter for ZipOut {
    fn add_dir(&mut self, e: &NewEntry) -> Result<()> {
        let opts = self.settings.options(e);
        self.writer
            .add_directory(e.name.as_str(), opts)
            .map_err(map_err)
    }

    fn add_file(&mut self, e: &NewEntry, src: &mut dyn Read) -> Result<()> {
        self.settings.start(&mut self.writer, e)?;
        std::io::copy(src, &mut self.writer)?;
        Ok(())
    }

    /// Small files are compressed across entries; large unencrypted Deflate entries
    /// are compressed in bounded chunks and merged in input order.
    fn add_files<'a>(
        &mut self,
        files: &mut dyn Iterator<Item = Result<FileSource<'a>>>,
    ) -> Result<()> {
        if self.threads <= 1 {
            for item in files {
                let (entry, mut src) = item?;
                self.add_file(&entry, &mut src)?;
            }
            return Ok(());
        }
        let threads = self.threads;
        let large = LARGE_FILE.min((self.memory_budget / threads as u64).max(1 << 20));
        let budget = self.memory_budget;
        let settings = self.settings.clone();

        let (job_tx, job_rx) = mpsc::sync_channel::<Job<'a>>(threads * 2);
        let job_rx = Mutex::new(job_rx);
        let producer_done = Arc::new(AtomicBool::new(false));
        let (done_tx, done_rx) = mpsc::channel::<Done>();

        std::thread::scope(|scope| {
            for _ in 0..threads {
                let (job_rx, done_tx, settings, producer_done) =
                    (&job_rx, done_tx.clone(), &settings, producer_done.clone());
                scope.spawn(move || loop {
                    let Some(mut job) = receive_job(job_rx, &producer_done) else {
                        return;
                    };
                    let packed = pack_one(settings, &job.entry, &mut job.src);
                    if done_tx.send((job.seq, job.cost, packed)).is_err() {
                        return;
                    }
                });
            }
            drop(done_tx);

            let mut next_write = 0u64;
            let mut pending: BTreeMap<u64, (u64, Vec<u8>)> = BTreeMap::new();
            let mut outstanding = 0usize;
            let mut in_flight = 0u64;

            // Takes one finished job, then writes everything that is now in order.
            let receive = |this: &mut Self,
                           pending: &mut BTreeMap<u64, (u64, Vec<u8>)>,
                           outstanding: &mut usize,
                           in_flight: &mut u64,
                           next_write: &mut u64|
             -> Result<()> {
                let (seq, cost, packed) = done_rx
                    .recv()
                    .map_err(|_| Error::Corrupt("compression worker stopped".into()))?;
                *outstanding -= 1;
                pending.insert(seq, (cost, packed?));
                while let Some((cost, bytes)) = pending.remove(next_write) {
                    this.merge(bytes)?;
                    *in_flight -= cost;
                    *next_write += 1;
                }
                Ok(())
            };

            let enqueue_result: Result<()> = (|| {
                for (my_seq, item) in (0u64..).zip(files) {
                    let (entry, mut src) = item?;
                    if entry.size > large {
                        // Keep archive order: everything before this file lands first.
                        while next_write < my_seq {
                            receive(
                                self,
                                &mut pending,
                                &mut outstanding,
                                &mut in_flight,
                                &mut next_write,
                            )?;
                        }
                        let deflate_workers = deflate_worker_count(threads, budget);
                        if settings.password.is_none() && settings.level > 0 && deflate_workers > 1
                        {
                            let packed = pack_entry(
                                &entry,
                                &mut src,
                                settings.level,
                                deflate_workers,
                                settings.portable,
                            )?;
                            self.writer.merge_archive(packed).map_err(map_err)?;
                        } else {
                            self.add_file(&entry, &mut src)?;
                        }
                        next_write = my_seq + 1;
                        continue;
                    }
                    let cost = entry.size.max(1);
                    // Wait for memory, but never when nothing is in flight (a file bigger than
                    // the whole budget must still be able to run alone).
                    while in_flight + cost > budget && outstanding > 0 {
                        receive(
                            self,
                            &mut pending,
                            &mut outstanding,
                            &mut in_flight,
                            &mut next_write,
                        )?;
                    }
                    in_flight += cost;
                    outstanding += 1;
                    job_tx
                        .send(Job {
                            seq: my_seq,
                            cost,
                            entry,
                            src,
                        })
                        .map_err(|_| Error::Corrupt("compression workers stopped".into()))?;
                }
                Ok(())
            })();
            // The explicit flag bounds shutdown even if an error path retains a sender.
            producer_done.store(true, Ordering::Release);
            drop(job_tx);
            enqueue_result?;
            while outstanding > 0 {
                receive(
                    self,
                    &mut pending,
                    &mut outstanding,
                    &mut in_flight,
                    &mut next_write,
                )?;
            }
            Ok(())
        })
    }

    fn finish(self: Box<Self>) -> Result<()> {
        let mut inner = self.writer.finish().map_err(map_err)?;
        inner.flush()?;
        inner.get_ref().sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn producer_shutdown_releases_an_idle_worker_with_a_live_sender() {
        let (job_tx, job_rx) = mpsc::sync_channel::<Job<'static>>(1);
        let job_rx = Mutex::new(job_rx);
        let producer_done = AtomicBool::new(true);
        let (finished_tx, finished_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            assert!(receive_job(&job_rx, &producer_done).is_none());
            let _ = finished_tx.send(());
        });

        assert!(
            finished_rx.recv_timeout(Duration::from_secs(1)).is_ok(),
            "idle worker did not observe producer shutdown"
        );
        worker.join().unwrap();
        drop(job_tx);
    }

    #[test]
    fn portable_zip_uses_default_permissions_instead_of_source_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("portable.zip");
        let mut writer = ZipOut::new(
            File::create(&path).unwrap(),
            &CreateOptions {
                threads: 1,
                portable: true,
                ..CreateOptions::default()
            },
        );
        let entry = NewEntry {
            name: "run.sh".into(),
            size: 4,
            unix_mode: Some(0o755),
            modified: None,
        };
        writer.add_file(&entry, &mut Cursor::new(b"test")).unwrap();
        Box::new(writer).finish().unwrap();

        let mut reader = crate::open(&path).unwrap();
        assert_eq!(reader.entries().unwrap()[0].unix_mode, Some(0o100644));
    }
}
