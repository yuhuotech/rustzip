use crate::zip_fmt::{map_err, unix_to_dos};
use rz_core::{resolve_threads, ArchiveWriter, CreateOptions, Error, FileSource, NewEntry, Result};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Cursor, Read, Write};
use std::sync::mpsc;
use std::sync::Mutex;
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter};

/// Files above this size are compressed on the writing thread: holding them in
/// memory to compress elsewhere would cost more than the parallelism gains.
const LARGE_FILE: u64 = 32 << 20;

#[derive(Clone)]
struct Settings {
    level: u32,
    password: Option<String>,
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
        if let Some(mode) = e.unix_mode {
            o = o.unix_permissions(mode & 0o777);
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

/// Compresses one file into a complete one-entry zip held in memory.
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

    /// Small files are compressed on worker threads and merged in input order,
    /// so the result is the same archive a single thread would write.
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
        let (done_tx, done_rx) = mpsc::channel::<Done>();

        std::thread::scope(|scope| {
            for _ in 0..threads {
                let (job_rx, done_tx, settings) = (&job_rx, done_tx.clone(), &settings);
                scope.spawn(move || loop {
                    let job = match job_rx.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => return,
                    };
                    let Ok(mut job) = job else { return };
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
                    self.add_file(&entry, &mut src)?;
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
                if job_tx
                    .send(Job {
                        seq: my_seq,
                        cost,
                        entry,
                        src,
                    })
                    .is_err()
                {
                    return Err(Error::Corrupt("compression workers stopped".into()));
                }
            }
            drop(job_tx);
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
