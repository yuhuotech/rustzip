use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use rz_core::{
    ArchiveReader, ArchiveWriter, Caps, CreateOptions, Entry, EntryKind, Error, NewEntry, Result,
};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// tar is a sequential format, so each `open` rescans from the start.
/// Good enough for single entries; bulk extraction will use one streaming pass.
pub struct TarReader {
    path: PathBuf,
    gzip: bool,
}

impl TarReader {
    pub fn new(path: &Path, gzip: bool) -> Self {
        Self {
            path: path.to_path_buf(),
            gzip,
        }
    }

    fn stream(&self) -> Result<Box<dyn Read>> {
        let file = BufReader::with_capacity(256 * 1024, File::open(&self.path)?);
        if self.gzip {
            Ok(Box::new(GzDecoder::new(file)))
        } else {
            Ok(Box::new(file))
        }
    }
}

fn corrupt(e: std::io::Error) -> Error {
    Error::Corrupt(e.to_string())
}

impl ArchiveReader for TarReader {
    fn format_name(&self) -> &'static str {
        if self.gzip {
            "tar.gz"
        } else {
            "tar"
        }
    }

    fn caps(&self) -> Caps {
        Caps {
            random_access: false,
            solid: true,
            writable: true,
        }
    }

    fn entries(&mut self) -> Result<Vec<Entry>> {
        let mut archive = tar::Archive::new(self.stream()?);
        let mut out = Vec::new();
        for (index, item) in archive.entries().map_err(corrupt)?.enumerate() {
            let item = item.map_err(corrupt)?;
            let header = item.header();
            let kind = match header.entry_type() {
                t if t.is_dir() => EntryKind::Dir,
                t if t.is_symlink() || t.is_hard_link() => EntryKind::Symlink,
                _ => EntryKind::File,
            };
            let path = item
                .path()
                .map_err(corrupt)?
                .to_string_lossy()
                .replace('\\', "/");
            out.push(Entry {
                index,
                path,
                kind,
                size: header.size().map_err(corrupt)?,
                packed_size: None,
                modified: header.mtime().ok().and_then(|t| i64::try_from(t).ok()),
                encrypted: false,
                unix_mode: header.mode().ok(),
            });
        }
        Ok(out)
    }

    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        let mut archive = tar::Archive::new(self.stream()?);
        let mut data = Vec::new();
        for (index, item) in archive.entries().map_err(corrupt)?.enumerate() {
            let mut item = item.map_err(corrupt)?;
            if index == entry.index {
                item.read_to_end(&mut data).map_err(corrupt)?;
                return Ok(Box::new(std::io::Cursor::new(data)));
            }
        }
        Err(Error::Corrupt(format!("entry {} not found", entry.index)))
    }
}

enum Sink {
    Plain(BufWriter<File>),
    Gzip(GzEncoder<BufWriter<File>>),
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Sink::Plain(w) => w.write(buf),
            Sink::Gzip(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Sink::Plain(w) => w.flush(),
            Sink::Gzip(w) => w.flush(),
        }
    }
}

pub struct TarOut {
    builder: tar::Builder<Sink>,
}

impl TarOut {
    pub fn new(file: File, gzip: bool, opts: &CreateOptions) -> Result<Self> {
        if opts.password.as_deref().is_some_and(|p| !p.is_empty()) {
            return Err(Error::Unsupported(
                "tar archives cannot be encrypted; use zip or 7z".into(),
            ));
        }
        let buffered = BufWriter::with_capacity(256 * 1024, file);
        let sink = if gzip {
            Sink::Gzip(GzEncoder::new(
                buffered,
                Compression::new(opts.level.min(9)),
            ))
        } else {
            Sink::Plain(buffered)
        };
        Ok(Self {
            builder: tar::Builder::new(sink),
        })
    }

    fn header(e: &NewEntry, dir: bool) -> tar::Header {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(if dir {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        });
        h.set_size(if dir { 0 } else { e.size });
        h.set_mode(e.unix_mode.unwrap_or(if dir { 0o755 } else { 0o644 }) & 0o777);
        h.set_mtime(e.modified.and_then(|t| u64::try_from(t).ok()).unwrap_or(0));
        h
    }
}

impl ArchiveWriter for TarOut {
    fn add_dir(&mut self, e: &NewEntry) -> Result<()> {
        let mut h = Self::header(e, true);
        let name = format!("{}/", e.name.trim_end_matches('/'));
        self.builder
            .append_data(&mut h, name, std::io::empty())
            .map_err(Error::Io)
    }

    fn add_file(&mut self, e: &NewEntry, src: &mut dyn Read) -> Result<()> {
        let mut h = Self::header(e, false);
        self.builder
            .append_data(&mut h, &e.name, src)
            .map_err(Error::Io)
    }

    fn finish(self: Box<Self>) -> Result<()> {
        let sink = self.builder.into_inner()?;
        let mut buffered = match sink {
            Sink::Plain(w) => w,
            Sink::Gzip(w) => w.finish()?,
        };
        buffered.flush()?;
        buffered.get_ref().sync_all()?;
        Ok(())
    }
}
