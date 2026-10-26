use crate::codec::{Compression, Sink};
use rz_core::{
    ArchiveReader, ArchiveWriter, Caps, CreateOptions, Entry, EntryKind, Error, NewEntry, Result,
};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// tar is a sequential format. `entries` and `for_each` each make one pass over
/// the file; `open` rescans, so bulk work should go through `for_each`.
pub struct TarReader {
    path: PathBuf,
    comp: Compression,
}

impl TarReader {
    pub fn new(path: &Path, comp: Compression) -> Self {
        Self {
            path: path.to_path_buf(),
            comp,
        }
    }

    fn stream(&self) -> Result<Box<dyn Read + Send>> {
        self.comp.reader(File::open(&self.path)?)
    }
}

fn corrupt(e: std::io::Error) -> Error {
    Error::Corrupt(e.to_string())
}

fn describe<R: Read>(index: usize, item: &tar::Entry<'_, R>) -> Result<Entry> {
    let header = item.header();
    let kind = match header.entry_type() {
        t if t.is_dir() => EntryKind::Dir,
        t if t.is_symlink() || t.is_hard_link() => EntryKind::Symlink,
        t if t.is_file() || t.is_contiguous() => EntryKind::File,
        // Devices, FIFOs and metadata records are never extracted.
        _ => EntryKind::Symlink,
    };
    let path = item
        .path()
        .map_err(corrupt)?
        .to_string_lossy()
        .replace('\\', "/");
    Ok(Entry {
        index,
        path,
        kind,
        size: item.size(),
        packed_size: None,
        modified: header.mtime().ok().and_then(|t| i64::try_from(t).ok()),
        encrypted: false,
        unix_mode: header.mode().ok(),
    })
}

impl ArchiveReader for TarReader {
    fn format_name(&self) -> &'static str {
        match self.comp {
            Compression::None => "tar",
            Compression::Gzip => "tar.gz",
            Compression::Xz => "tar.xz",
            Compression::Zstd => "tar.zst",
            Compression::Bzip2 => "tar.bz2",
            Compression::Lz4 => "tar.lz4",
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
            out.push(describe(index, &item.map_err(corrupt)?)?);
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

    fn for_each(
        &mut self,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let mut archive = tar::Archive::new(self.stream()?);
        for (index, item) in archive.entries().map_err(corrupt)?.enumerate() {
            let mut item = item.map_err(corrupt)?;
            let entry = describe(index, &item)?;
            visit(&entry, &mut item)?;
        }
        Ok(())
    }
}

pub struct TarOut {
    builder: tar::Builder<Sink>,
}

impl TarOut {
    pub fn new(file: File, comp: Compression, opts: &CreateOptions) -> Result<Self> {
        if opts.password.as_deref().is_some_and(|p| !p.is_empty()) {
            return Err(Error::Unsupported(
                "tar archives cannot be encrypted; use zip or 7z".into(),
            ));
        }
        let sink = comp.writer(file, opts.level, opts.threads)?;
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
        self.builder.into_inner()?.finish()
    }
}
