use rz_core::{
    resolve_threads, ArchiveReader, ArchiveWriter, CancelToken, Caps, CreateOptions, Entry,
    EntryKind, Error, FileSource, NewEntry, NoProgress, Progress, ProgressSink, Result,
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

pub(crate) fn map_err(e: sevenz_rust2::Error) -> Error {
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

pub(crate) fn describe(index: usize, f: &ArchiveEntry) -> Entry {
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
    paths: Vec<PathBuf>,
    password: Option<String>,
}

impl SevenReader {
    pub fn new(path: &Path) -> Result<Self> {
        let r = Self {
            paths: vec![path.to_path_buf()],
            password: None,
        };
        // Fail early on a broken signature or header; an encrypted header is
        // reported later, once the caller had a chance to set the password.
        match r.archive() {
            Ok(_) | Err(Error::Password) => Ok(r),
            Err(e) => Err(e),
        }
    }

    pub fn new_volumes(paths: Vec<PathBuf>) -> Result<Self> {
        if paths.len() < 2 {
            return Err(Error::Corrupt(
                "7z volume set has fewer than two parts".into(),
            ));
        }
        let r = Self {
            paths,
            password: None,
        };
        match r.archive() {
            Ok(_) | Err(Error::Password) => Ok(r),
            Err(e) => Err(e),
        }
    }

    fn archive(&self) -> Result<SzReader<crate::volumes::MultipartReader>> {
        let pw = Password::new(self.password.as_deref().unwrap_or(""));
        let source = crate::volumes::MultipartReader::open(&self.paths, 0, Default::default())?;
        SzReader::new(source, pw).map_err(map_err)
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
        self.entries_with_progress(&CancelToken::new(), &mut NoProgress)
    }

    fn entries_with_progress(
        &mut self,
        cancel: &CancelToken,
        progress: &mut dyn ProgressSink,
    ) -> Result<Vec<Entry>> {
        cancel.check()?;
        let r = self.archive()?;
        let files = &r.archive().files;
        let mut entries = Vec::with_capacity(files.len());
        let mut bytes_done = 0u64;
        for (index, file) in files.iter().enumerate() {
            cancel.check()?;
            let entry = describe(index, file);
            bytes_done = bytes_done.saturating_add(entry.size);
            entries.push(entry);
            progress.update(Progress {
                files_done: index as u64 + 1,
                files_total: 0,
                bytes_done,
                bytes_total: 0,
            });
        }
        Ok(entries)
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
    solid_block_size: Option<u64>,
    portable: bool,
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
        let threads = resolve_threads(opts.threads).clamp(1, 256) as u32;
        let level = opts.level.min(9);
        if level == 0 {
            if opts.dictionary_size.is_some() {
                return Err(Error::Unsupported(
                    "a 7z dictionary is not used when compression level is 0".into(),
                ));
            }
            methods.push(EncoderConfiguration::new(EncoderMethod::COPY));
        } else {
            let (lzma, _, _) = lzma_options(opts, level, threads)?;
            methods.push(lzma.into());
        }
        writer.set_content_methods(methods);
        Ok(Self {
            writer,
            solid_block_size: opts.solid_block_size,
            portable: opts.portable,
        })
    }

    fn entry(e: &NewEntry, dir: bool, portable: bool) -> ArchiveEntry {
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
        if !portable {
            a.has_windows_attributes = true;
            a.windows_attributes = if dir { ATTR_DIRECTORY } else { ATTR_ARCHIVE };
            if let Some(mode) = e.unix_mode {
                a.windows_attributes |= ATTR_UNIX_EXTENSION | ((mode & 0o7777) << 16);
            }
        }
        a
    }

    fn flush_solid<R: Read>(
        &mut self,
        entries: &mut Vec<ArchiveEntry>,
        readers: &mut Vec<SourceReader<R>>,
    ) -> Result<()> {
        if !entries.is_empty() {
            self.writer
                .push_archive_entries(std::mem::take(entries), std::mem::take(readers))
                .map_err(map_err)?;
        }
        Ok(())
    }
}

const MIN_DICTIONARY_SIZE: u32 = 4 * 1024;
const MAX_DICTIONARY_SIZE: u32 = 64 << 20;
const DEFAULT_DICTIONARY_SIZE: u32 = 8 << 20;
const DICTIONARY_MEMORY_FACTOR: u64 = 6;
const SINGLE_THREAD_OVERHEAD: u64 = 1 << 20;
const MAX_SOLID_BLOCK_ENTRIES: usize = 4096;

fn lzma_options(
    opts: &CreateOptions,
    level: u32,
    requested_threads: u32,
) -> Result<(Lzma2Options, u32, u32)> {
    let preset = Lzma2Options::from_level(level);
    let preset_config: EncoderConfiguration = preset.into();
    let preset_dictionary = preset_config
        .options
        .as_ref()
        .map_or(DEFAULT_DICTIONARY_SIZE, |o| o.get_lzma_dict_size());
    let mut dictionary = opts.dictionary_size.unwrap_or(preset_dictionary);
    if !(MIN_DICTIONARY_SIZE..=MAX_DICTIONARY_SIZE).contains(&dictionary) {
        return Err(Error::Unsupported(format!(
            "7z dictionary size must be between 4 KiB and 64 MiB, got {dictionary} bytes"
        )));
    }
    let dictionary_was_selected = opts.dictionary_size.is_some();
    let budget = opts.memory_budget;
    let mut selected_threads = None;

    loop {
        for threads in (1..=requested_threads).rev() {
            if lzma_memory_estimate(dictionary, preset_dictionary, threads, budget)
                .is_some_and(|needed| needed <= budget)
            {
                selected_threads = Some(threads);
                break;
            }
        }
        if selected_threads.is_some()
            || dictionary_was_selected
            || dictionary == MIN_DICTIONARY_SIZE
        {
            break;
        }
        dictionary = (dictionary / 2).max(MIN_DICTIONARY_SIZE);
    }

    let Some(threads) = selected_threads else {
        return Err(Error::LimitExceeded(
            "7z compression settings exceed the estimated memory budget; lower the dictionary size or thread count".into(),
        ));
    };
    let lzma = if threads > 1 {
        let chunk_size = lzma_chunk_size(dictionary, preset_dictionary, threads, budget)
            .ok_or_else(|| {
                Error::LimitExceeded(
                    "7z compression settings exceed the estimated memory budget".into(),
                )
            })?;
        Lzma2Options::from_level_mt(level, threads, chunk_size)
    } else {
        Lzma2Options::from_level(level)
    };
    let mut lzma = lzma;
    lzma.set_dictionary_size(dictionary);
    Ok((lzma, threads, dictionary))
}

fn lzma_memory_estimate(
    dictionary: u32,
    preset_dictionary: u32,
    threads: u32,
    budget: u64,
) -> Option<u64> {
    let dict = u64::from(dictionary);
    if threads <= 1 {
        return dict
            .checked_mul(DICTIONARY_MEMORY_FACTOR)?
            .checked_add(SINGLE_THREAD_OVERHEAD);
    }
    let chunk = lzma_chunk_size(dictionary, preset_dictionary, threads, budget)?;
    let thread_count = u64::from(threads);
    let dict_memory = dict.checked_mul(DICTIONARY_MEMORY_FACTOR)?;
    let worker_memory = dict_memory.checked_add(chunk.checked_mul(2)?)?;
    chunk.checked_add(worker_memory.checked_mul(thread_count)?)
}

fn lzma_chunk_size(
    dictionary: u32,
    preset_dictionary: u32,
    threads: u32,
    budget: u64,
) -> Option<u64> {
    let thread_count = u64::from(threads);
    let dict_memory = u64::from(dictionary).checked_mul(DICTIONARY_MEMORY_FACTOR)?;
    let available_for_dictionary = dict_memory.checked_mul(thread_count)?;
    let per_chunk_buffers = thread_count.checked_mul(2)?.checked_add(1)?;
    let available = budget.checked_sub(available_for_dictionary)? / per_chunk_buffers;
    let minimum_chunk = u64::from(dictionary.max(preset_dictionary));
    let chunk = available.min(MT_CHUNK.max(minimum_chunk));
    (chunk >= minimum_chunk).then_some(chunk)
}

impl ArchiveWriter for SevenOut {
    fn add_dir(&mut self, e: &NewEntry) -> Result<()> {
        self.writer
            .push_archive_entry::<&[u8]>(Self::entry(e, true, self.portable), None)
            .map_err(map_err)?;
        Ok(())
    }

    fn add_file(&mut self, e: &NewEntry, src: &mut dyn Read) -> Result<()> {
        self.writer
            .push_archive_entry(Self::entry(e, false, self.portable), Some(src))
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
        let mut block_bytes = 0u64;
        for item in files {
            let (meta, src) = item?;
            if self.solid_block_size == Some(0) {
                self.writer
                    .push_archive_entry(Self::entry(&meta, false, self.portable), Some(src))
                    .map_err(map_err)?;
                continue;
            }
            if let Some(limit) = self.solid_block_size {
                if !entries.is_empty()
                    && (block_bytes.saturating_add(meta.size) > limit
                        || entries.len() >= MAX_SOLID_BLOCK_ENTRIES)
                {
                    self.flush_solid(&mut entries, &mut readers)?;
                    block_bytes = 0;
                }
            }
            entries.push(Self::entry(&meta, false, self.portable));
            readers.push(SourceReader::new(src));
            block_bytes = block_bytes.saturating_add(meta.size);
            if self.solid_block_size.is_some_and(|limit| {
                limit > 0 && (block_bytes >= limit || entries.len() >= MAX_SOLID_BLOCK_ENTRIES)
            }) {
                self.flush_solid(&mut entries, &mut readers)?;
                block_bytes = 0;
            }
        }
        self.flush_solid(&mut entries, &mut readers)
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use std::io::Cursor;

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
        let a = SevenOut::entry(&e, false, false);
        let d = describe(0, &a);
        assert_eq!(d.unix_mode, Some(0o755));
        assert_eq!(d.kind, EntryKind::File);
    }

    #[test]
    fn portable_entries_omit_platform_specific_attributes() {
        let e = NewEntry {
            name: "x".into(),
            size: 0,
            unix_mode: Some(0o755),
            modified: None,
        };
        let a = SevenOut::entry(&e, false, true);
        assert!(!a.has_windows_attributes);
        assert_eq!(a.windows_attributes, 0);
    }

    #[test]
    fn dictionary_size_is_applied_and_thread_count_is_limited_by_memory_budget() {
        let options = CreateOptions {
            level: 6,
            dictionary_size: Some(2 << 20),
            memory_budget: 256 << 20,
            ..CreateOptions::default()
        };
        let (lzma, threads, dictionary) = lzma_options(&options, 6, 16).unwrap();
        let config: EncoderConfiguration = lzma.into();
        assert_eq!(config.options.unwrap().get_lzma_dict_size(), 2 << 20);
        assert!(threads > 0 && threads < 16);
        assert_eq!(dictionary, 2 << 20);
        assert!(
            lzma_memory_estimate(dictionary, 8 << 20, threads, 256 << 20).unwrap() <= 256 << 20
        );
    }

    #[test]
    fn selected_dictionary_that_exceeds_the_memory_budget_is_rejected() {
        let options = CreateOptions {
            level: 9,
            dictionary_size: Some(64 << 20),
            memory_budget: 256 << 20,
            ..CreateOptions::default()
        };
        assert!(matches!(
            lzma_options(&options, 9, 1),
            Err(Error::LimitExceeded(_))
        ));
    }

    fn write_test_archive(path: &Path, block_size: Option<u64>) {
        let options = CreateOptions {
            level: 1,
            threads: 1,
            solid_block_size: block_size,
            ..CreateOptions::default()
        };
        let mut writer = SevenOut::new(File::create(path).unwrap(), &options).unwrap();
        let files = ["a.txt", "b.txt"].map(|name| {
            Ok((
                NewEntry {
                    name: name.into(),
                    size: 2,
                    unix_mode: None,
                    modified: None,
                },
                Box::new(Cursor::new(b"ab".to_vec())) as Box<dyn Read + Send>,
            ))
        });
        let mut files = files.into_iter();
        writer.add_files(&mut files).unwrap();
        Box::new(writer).finish().unwrap();
    }

    #[test]
    fn solid_block_size_splits_7z_blocks_and_zero_disables_solid_mode() {
        let dir = tempfile::tempdir().unwrap();
        let solid = dir.path().join("solid.7z");
        let limited = dir.path().join("limited.7z");
        let non_solid = dir.path().join("non-solid.7z");
        write_test_archive(&solid, None);
        write_test_archive(&limited, Some(2));
        write_test_archive(&non_solid, Some(0));

        let solid = SevenReader::new(&solid).unwrap().archive().unwrap();
        let limited = SevenReader::new(&limited).unwrap().archive().unwrap();
        let non_solid = SevenReader::new(&non_solid).unwrap().archive().unwrap();
        assert!(solid.archive().is_solid);
        assert!(!limited.archive().is_solid);
        assert!(!non_solid.archive().is_solid);
    }
}
