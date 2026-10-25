//! A single compressed stream that is not a tar: `notes.txt.gz`, `disk.img.zst`, ...

use crate::codec::{Compression, Sink};
use rz_core::{
    ArchiveReader, ArchiveWriter, CancelToken, Caps, CreateOptions, Entry, EntryKind, Error,
    NewEntry, Result,
};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Counting the unpacked size means decoding once; stop at this size.
const COUNT_LIMIT: u64 = 1 << 40;

pub struct SingleReader {
    path: PathBuf,
    comp: Compression,
    size: Option<u64>,
    control: Option<(CancelToken, u64)>,
}

impl SingleReader {
    pub fn new(path: &Path, comp: Compression) -> Self {
        Self {
            path: path.to_path_buf(),
            comp,
            size: None,
            control: None,
        }
    }

    /// Name of the file inside: the archive name without its compression extension.
    fn inner_name(&self) -> String {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "data".into());
        let ext = format!(".{}", self.comp.extension());
        if name.len() > ext.len() && name.to_ascii_lowercase().ends_with(&ext) {
            name[..name.len() - ext.len()].to_string()
        } else {
            format!("{name}.out")
        }
    }

    fn unpacked_size(&mut self) -> Result<u64> {
        if let Some(s) = self.size {
            return Ok(s);
        }
        let mut r = self.comp.reader(File::open(&self.path)?)?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut total = 0u64;
        loop {
            if let Some((cancel, _)) = &self.control {
                cancel.check()?;
            }
            let n = r
                .read(&mut buf)
                .map_err(|e| Error::Corrupt(e.to_string()))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            let limit = self
                .control
                .as_ref()
                .map_or(COUNT_LIMIT, |(_, limit)| (*limit).min(COUNT_LIMIT));
            if total > limit {
                return Err(Error::LimitExceeded(
                    "stream exceeds unpacked size limit".into(),
                ));
            }
        }
        self.size = Some(total);
        Ok(total)
    }
}

impl ArchiveReader for SingleReader {
    fn set_read_control(&mut self, cancel: CancelToken, max_total: u64) {
        self.control = Some((cancel, max_total));
    }
    fn format_name(&self) -> &'static str {
        match self.comp {
            Compression::Gzip => "gzip",
            Compression::Xz => "xz",
            Compression::Zstd => "zstd",
            Compression::Bzip2 => "bzip2",
            Compression::None => "raw",
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
        let size = self.unpacked_size()?;
        let meta = std::fs::metadata(&self.path)?;
        Ok(vec![Entry {
            index: 0,
            path: self.inner_name(),
            kind: EntryKind::File,
            size,
            packed_size: Some(meta.len()),
            modified: rz_platform::mtime_of(&meta),
            encrypted: false,
            unix_mode: None,
        }])
    }

    fn open<'a>(&'a mut self, _entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        Ok(Box::new(self.comp.reader(File::open(&self.path)?)?))
    }
}

pub struct SingleOut {
    sink: Sink,
    added: bool,
}

impl SingleOut {
    pub fn new(file: File, comp: Compression, opts: &CreateOptions) -> Result<Self> {
        if opts.password.as_deref().is_some_and(|p| !p.is_empty()) {
            return Err(Error::Unsupported(
                "this format cannot be encrypted; use zip or 7z".into(),
            ));
        }
        Ok(Self {
            sink: comp.writer(file, opts.level, opts.threads)?,
            added: false,
        })
    }
}

impl ArchiveWriter for SingleOut {
    fn add_dir(&mut self, _: &NewEntry) -> Result<()> {
        Err(Error::Unsupported(
            "this format holds one file, not a folder; use .tar.gz, .tar.xz or .tar.zst".into(),
        ))
    }

    fn add_file(&mut self, _: &NewEntry, src: &mut dyn Read) -> Result<()> {
        if self.added {
            return Err(Error::Unsupported(
                "this format holds exactly one file; use .zip, .7z or .tar.* for several".into(),
            ));
        }
        self.added = true;
        std::io::copy(src, &mut self.sink)?;
        self.sink.flush()?;
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<()> {
        self.sink.finish()
    }
}
