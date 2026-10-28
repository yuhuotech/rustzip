//! Stream compressors used by tar and by single-file formats (.gz .xz .zst .bz2).

use crate::parallel_bzip2::{worker_count, ParallelBzip2};
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use lz4_flex::frame::{FrameDecoder, FrameEncoder};
use rz_core::{resolve_threads, Error, Result};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compression {
    None,
    Gzip,
    Xz,
    Zstd,
    Bzip2,
    Lz4,
}

impl Compression {
    /// File extension without the dot, e.g. for naming the single file inside `.gz`.
    pub fn extension(self) -> &'static str {
        match self {
            Compression::None => "",
            Compression::Gzip => "gz",
            Compression::Xz => "xz",
            Compression::Zstd => "zst",
            Compression::Bzip2 => "bz2",
            Compression::Lz4 => "lz4",
        }
    }

    /// Wraps a file in the matching decompressor.
    pub fn reader(self, file: File) -> Result<Box<dyn Read + Send>> {
        let buffered = BufReader::with_capacity(256 * 1024, file);
        Ok(match self {
            Compression::None => Box::new(buffered),
            Compression::Gzip => Box::new(MultiGzDecoder::new(buffered)),
            Compression::Xz => Box::new(liblzma::read::XzDecoder::new_parallel(buffered)),
            Compression::Zstd => Box::new(zstd::stream::read::Decoder::new(buffered)?),
            Compression::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(buffered)),
            Compression::Lz4 => Box::new(FrameDecoder::new(buffered)),
        })
    }

    /// Wraps a file in the matching compressor. `level` is 0-9 on our scale.
    pub fn writer(
        self,
        file: File,
        level: u32,
        threads: usize,
        memory_budget: u64,
    ) -> Result<Sink> {
        let level = level.min(9);
        let threads = resolve_threads(threads);
        let out = BufWriter::with_capacity(256 * 1024, file);
        Ok(match self {
            Compression::None => Sink::Plain(out),
            Compression::Gzip => Sink::Gzip(GzEncoder::new(out, flate2::Compression::new(level))),
            Compression::Xz => {
                let mut b = liblzma::stream::MtStreamBuilder::new();
                b.threads(threads as u32)
                    .preset(level)
                    .check(liblzma::stream::Check::Crc64);
                let stream = b
                    .encoder()
                    .map_err(|e| Error::Unsupported(format!("xz encoder: {e}")))?;
                Sink::Xz(liblzma::write::XzEncoder::new_stream(out, stream))
            }
            Compression::Zstd => {
                let mut enc = zstd::stream::write::Encoder::new(out, zstd_level(level))?;
                if threads > 1 {
                    enc.multithread(threads as u32)?;
                }
                Sink::Zstd(Box::new(enc))
            }
            Compression::Bzip2 => {
                let workers = worker_count(threads, memory_budget);
                if workers > 1 {
                    Sink::ParallelBzip2(ParallelBzip2::new(out, level.clamp(1, 9), workers)?)
                } else {
                    Sink::Bzip2(bzip2::write::BzEncoder::new(
                        out,
                        bzip2::Compression::new(level.clamp(1, 9)),
                    ))
                }
            }
            Compression::Lz4 => Sink::Lz4(FrameEncoder::new(out)),
        })
    }
}

/// Maps our 0-9 scale onto zstd's 1-19 range.
pub fn zstd_level(level: u32) -> i32 {
    const MAP: [i32; 10] = [1, 1, 2, 3, 5, 7, 9, 12, 16, 19];
    MAP[level.min(9) as usize]
}

type Out = BufWriter<File>;

pub enum Sink {
    Plain(Out),
    Gzip(GzEncoder<Out>),
    Xz(liblzma::write::XzEncoder<Out>),
    Zstd(Box<zstd::stream::write::Encoder<'static, Out>>),
    Bzip2(bzip2::write::BzEncoder<Out>),
    ParallelBzip2(ParallelBzip2<Out>),
    Lz4(FrameEncoder<Out>),
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Sink::Plain(w) => w.write(buf),
            Sink::Gzip(w) => w.write(buf),
            Sink::Xz(w) => w.write(buf),
            Sink::Zstd(w) => w.write(buf),
            Sink::Bzip2(w) => w.write(buf),
            Sink::ParallelBzip2(w) => w.write(buf),
            Sink::Lz4(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Sink::Plain(w) => w.flush(),
            Sink::Gzip(w) => w.flush(),
            Sink::Xz(w) => w.flush(),
            Sink::Zstd(w) => w.flush(),
            Sink::Bzip2(w) => w.flush(),
            Sink::ParallelBzip2(w) => w.flush(),
            Sink::Lz4(w) => w.flush(),
        }
    }
}

impl Sink {
    /// Writes the stream trailer and makes sure everything reached the disk.
    pub fn finish(self) -> Result<()> {
        let mut out = match self {
            Sink::Plain(w) => w,
            Sink::Gzip(w) => w.finish()?,
            Sink::Xz(w) => w.finish()?,
            Sink::Zstd(w) => w.finish()?,
            Sink::Bzip2(w) => w.finish()?,
            Sink::ParallelBzip2(w) => w.finish()?,
            Sink::Lz4(w) => w.finish().map_err(std::io::Error::from)?,
        };
        out.flush()?;
        out.get_ref().sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_levels_are_monotonic_and_in_range() {
        let levels: Vec<i32> = (0..=9).map(zstd_level).collect();
        assert!(levels.windows(2).all(|w| w[0] <= w[1]));
        assert!(levels.iter().all(|l| (1..=19).contains(l)));
        assert_eq!(zstd_level(100), 19);
    }

    #[test]
    fn every_codec_round_trips_through_a_file() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        for c in [
            Compression::None,
            Compression::Gzip,
            Compression::Xz,
            Compression::Zstd,
            Compression::Bzip2,
        ] {
            let path = dir.path().join(format!("f.{}", c.extension()));
            let file = File::create(&path).unwrap_or_else(|_| unreachable!());
            let mut sink = c
                .writer(file, 5, 2, 256 << 20)
                .unwrap_or_else(|_| unreachable!());
            assert!(sink.write_all(&data).is_ok());
            assert!(sink.finish().is_ok());
            let mut back = Vec::new();
            let mut r = c
                .reader(File::open(&path).unwrap_or_else(|_| unreachable!()))
                .unwrap_or_else(|_| unreachable!());
            assert!(r.read_to_end(&mut back).is_ok());
            assert_eq!(back, data, "{c:?}");
        }
    }
}
