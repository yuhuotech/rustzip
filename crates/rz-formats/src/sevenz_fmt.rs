use rz_core::{
    resolve_threads, ArchiveReader, ArchiveWriter, Caps, CreateOptions, Entry, EntryKind, Error,
    FileSource, NewEntry, Result,
};
use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
use sevenz_rust2::{
    ArchiveEntry, ArchiveReader as SzReader, ArchiveWriter as SzWriter, EncoderConfiguration,
    EncoderMethod, NtTime, Password, SourceReader,
};
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_ARCHIVE: u32 = 0x20;
const ATTR_REPARSE_POINT: u32 = 0x400;
/// p7zip convention: the high 16 bits hold the Unix mode.
const ATTR_UNIX_EXTENSION: u32 = 0x8000;
/// Independent LZMA2 chunks are what let several threads encode one solid block.
const MT_CHUNK: u64 = 16 << 20;

fn map_err(e: sevenz_rust2::Error) -> Error {
    use sevenz_rust2::Error as E;
    match e {
        E::PasswordRequired | E::MaybeBadPassword(_) => Error::Password,
        E::BadSignature(_) => Error::UnknownFormat,
        E::UnsupportedCompressionMethod(m) => Error::Unsupported(m),
        E::Unsupported(m) => Error::Unsupported(m.into_owned()),
        E::UnsupportedVersion { major, minor } => {
            Error::Unsupported(format!("7z version {major}.{minor}"))
        }
        E::ExternalUnsupported => Error::Unsupported("external 7z data".into()),
        E::MaxMemLimited { max_kb, actaul_kb } => Error::LimitExceeded(format!(
            "decoder needs {actaul_kb} KB, limit is {max_kb} KB"
        )),
        E::Io(e, _) | E::FileOpen(e, _) => match e.kind() {
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof => {
                Error::Corrupt(e.to_string())
            }
            _ => Error::Io(e),
        },
        other => Error::Corrupt(other.to_string()),
    }
}

fn nt_to_unix(t: NtTime) -> Option<i64> {
    let st: SystemTime = t.into();
    match st.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok(),
        Err(e) => i64::try_from(e.duration().as_secs()).ok().map(|s| -s),
    }
}

fn unix_to_nt(t: i64) -> Option<NtTime> {
    let st = if t >= 0 {
        UNIX_EPOCH + Duration::from_secs(t as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(t.unsigned_abs())
    };
    NtTime::try_from(st).ok()
}

fn describe(index: usize, f: &ArchiveEntry) -> Entry {
    let attrs = f.has_windows_attributes.then_some(f.windows_attributes);
    let unix_mode = attrs
        .filter(|a| a & ATTR_UNIX_EXTENSION != 0)
        .map(|a| a >> 16);
    let is_link = unix_mode.is_some_and(|m| m & 0o170000 == 0o120000)
        || attrs.is_some_and(|a| a & ATTR_REPARSE_POINT != 0);
    let kind = if f.is_directory {
        EntryKind::Dir
    } else if is_link {
        EntryKind::Symlink
    } else {
        EntryKind::File
    };
    Entry {
        index,
        path: f.name.replace('\\', "/"),
        kind,
        size: f.size,
        // Solid blocks have no meaningful per-file packed size.
        packed_size: None,
        modified: f
            .has_last_modified_date
            .then(|| nt_to_unix(f.last_modified_date))
            .flatten(),
        encrypted: false,
        unix_mode: unix_mode.map(|m| m & 0o7777),
    }
}

pub struct SevenReader {
    path: PathBuf,
    password: Option<String>,
}

impl SevenReader {
    pub fn new(path: &Path) -> Result<Self> {
        let r = Self {
            path: path.to_path_buf(),
            password: None,
        };
        // Fail early on a broken signature or header; an encrypted header is
        // reported later, once the caller had a chance to set the password.
        match r.archive() {
            Ok(_) | Err(Error::Password) => Ok(r),
            Err(e) => Err(e),
        }
    }

    fn archive(&self) -> Result<SzReader<File>> {
        let pw = Password::new(self.password.as_deref().unwrap_or(""));
        SzReader::open(&self.path, pw).map_err(map_err)
    }
}

impl ArchiveReader for SevenReader {
    fn format_name(&self) -> &'static str {
        "7z"
    }

    fn caps(&self) -> Caps {
        Caps {
            random_access: false,
            solid: true,
            writable: true,
        }
    }

    fn set_password(&mut self, password: &str) -> Result<()> {
        self.password = Some(password.to_string());
        Ok(())
    }

    fn entries(&mut self) -> Result<Vec<Entry>> {
        let r = self.archive()?;
        Ok(r.archive()
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| describe(i, f))
            .collect())
    }

    fn open<'a>(&'a mut self, entry: &Entry) -> Result<Box<dyn Read + 'a>> {
        let mut found = None;
        let wanted = entry.index;
        self.for_each(&mut |e, data| {
            if e.index == wanted {
                let mut buf = Vec::with_capacity(usize::try_from(e.size).unwrap_or(0).min(1 << 28));
                data.read_to_end(&mut buf)?;
                found = Some(buf);
            }
            Ok(())
        })?;
        match found {
            Some(buf) => Ok(Box::new(std::io::Cursor::new(buf))),
            None => Err(Error::Corrupt(format!("entry {wanted} not found"))),
        }
    }

    fn for_each(
        &mut self,
        visit: &mut dyn FnMut(&Entry, &mut dyn Read) -> Result<()>,
    ) -> Result<()> {
        let mut r = self.archive()?;
        r.set_thread_count(resolve_threads(0) as u32);
        // The decoder hands out `&ArchiveEntry` without its index; recover it from the address.
        let base = r.archive().files.as_ptr() as usize;
        let stride = std::mem::size_of::<ArchiveEntry>();
        let has_password = self.password.is_some();
        let mut failure: Option<Error> = None;
        let outcome = r.for_each_entries(|f, data| {
            let index = (f as *const ArchiveEntry as usize - base) / stride;
            match visit(&describe(index, f), data) {
                Ok(()) => {
                    // The decoder is sequential: whatever the visitor left unread must be consumed.
                    std::io::copy(data, &mut std::io::sink())?;
                    Ok(true)
                }
                Err(e) => {
                    failure = Some(e);
                    Ok(false)
                }
            }
        });
        if let Some(e) = failure {
            return Err(match e {
                // With a password set, a decode failure is most likely a wrong password.
                Error::Io(io) if has_password && io.kind() == std::io::ErrorKind::InvalidData => {
                    Error::Password
                }
                other => other,
            });
        }
        outcome.map_err(map_err)
    }
}

pub struct SevenOut {
    writer: SzWriter<BufWriter<File>>,
}

impl SevenOut {
    pub fn new(file: File, opts: &CreateOptions) -> Result<Self> {
        let mut writer =
            SzWriter::new(BufWriter::with_capacity(256 * 1024, file)).map_err(map_err)?;
        let mut methods: Vec<EncoderConfiguration> = Vec::new();
        let password = opts.password.as_deref().filter(|p| !p.is_empty());
        if let Some(pw) = password {
            methods.push(AesEncoderOptions::new(Password::new(pw)).into());
            // Hides file names too, like `7z a -mhe`.
            writer.set_encrypt_header(true);
        }
        let threads = resolve_threads(opts.threads) as u32;
        let level = opts.level.min(9);
        methods.push(if level == 0 {
            EncoderConfiguration::new(EncoderMethod::COPY)
        } else if threads > 1 {
            Lzma2Options::from_level_mt(level, threads, MT_CHUNK).into()
        } else {
            Lzma2Options::from_level(level).into()
        });
        writer.set_content_methods(methods);
        Ok(Self { writer })
    }

    fn entry(e: &NewEntry, dir: bool) -> ArchiveEntry {
        let name = e.name.trim_end_matches('/');
        let mut a = if dir {
            ArchiveEntry::new_directory(name)
        } else {
            ArchiveEntry::new_file(name)
        };
        if let Some(nt) = e.modified.and_then(unix_to_nt) {
            a.has_last_modified_date = true;
            a.last_modified_date = nt;
        }
        a.has_windows_attributes = true;
        a.windows_attributes = if dir { ATTR_DIRECTORY } else { ATTR_ARCHIVE };
        if let Some(mode) = e.unix_mode {
            a.windows_attributes |= ATTR_UNIX_EXTENSION | ((mode & 0o7777) << 16);
        }
        a
    }
}

impl ArchiveWriter for SevenOut {
    fn add_dir(&mut self, e: &NewEntry) -> Result<()> {
        self.writer
            .push_archive_entry::<&[u8]>(Self::entry(e, true), None)
            .map_err(map_err)?;
        Ok(())
    }

    fn add_file(&mut self, e: &NewEntry, src: &mut dyn Read) -> Result<()> {
        self.writer
            .push_archive_entry(Self::entry(e, false), Some(src))
            .map_err(map_err)?;
        Ok(())
    }

    /// One solid block: far better ratio for many small files than one block per file.
    fn add_files<'a>(
        &mut self,
        files: &mut dyn Iterator<Item = Result<FileSource<'a>>>,
    ) -> Result<()> {
        let mut entries = Vec::new();
        let mut readers = Vec::new();
        for item in files {
            let (meta, src) = item?;
            entries.push(Self::entry(&meta, false));
            readers.push(SourceReader::new(src));
        }
        if entries.is_empty() {
            return Ok(());
        }
        self.writer
            .push_archive_entries(entries, readers)
            .map_err(map_err)?;
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<()> {
        let mut out = self.writer.finish()?;
        out.flush()?;
        out.get_ref().sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nt_time_round_trips_unix_seconds() {
        for t in [0, 1_700_000_000, 951_782_400] {
            assert_eq!(unix_to_nt(t).and_then(nt_to_unix), Some(t));
        }
    }

    #[test]
    fn unix_mode_lives_in_high_attribute_bits() {
        let e = NewEntry {
            name: "x".into(),
            size: 0,
            unix_mode: Some(0o755),
            modified: None,
        };
        let a = SevenOut::entry(&e, false);
        let d = describe(0, &a);
        assert_eq!(d.unix_mode, Some(0o755));
        assert_eq!(d.kind, EntryKind::File);
    }
}
