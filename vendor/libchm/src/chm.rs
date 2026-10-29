//! The public entry point: opening an archive and reading entries out of it.

use std::{
    fmt,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use crate::{
    decompress::Decompressor,
    directory::{Directory, Entry, EntrySel},
    error::{ChmError, Result},
    format::{
        ITSF_V2_LEN, ITSF_V3_LEN, ITSP_V1_LEN, ItsfHeader, LZXC_MIN_LEN, LZXC_RESET_TABLE_LEN,
        parse_itsf, parse_itsp, parse_lzxc_control_data, parse_lzxc_reset_table,
    },
};

// Metadata paths for the MSCompressed section
const PATH_RESET_TABLE: &str = "::DataSpace/Storage/MSCompressed/Transform/{7FC28940-9D31-11D0-9B27-00A0C91E9C7C}/InstanceData/ResetTable";
const PATH_CONTROL_DATA: &str = "::DataSpace/Storage/MSCompressed/ControlData";
const PATH_CONTENT: &str = "::DataSpace/Storage/MSCompressed/Content";

/// The uncompressed section, in which an entry's offset is a plain file offset.
const SPACE_UNCOMPRESSED: u8 = 0;
/// The `MSCompressed` section, in which an entry's offset is an LZX stream offset.
const SPACE_COMPRESSED: u8 = 1;

/// Keep compatibility calls that return a `Vec` from allocating an unbounded entry.
const MAX_WHOLE_ENTRY_READ_BYTES: u64 = 64 * 1024 * 1024;

/// A parsed CHM archive that supports entry lookup, enumeration, and reading.
pub struct ChmFile {
    file: File,
    file_len: u64,
    data_offset: u64,
    directory: Directory,
    decompressor: Option<Decompressor>,
}

impl fmt::Debug for ChmFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChmFile")
            .field("file_len", &self.file_len)
            .field("data_offset", &self.data_offset)
            .field("compressed", &self.decompressor.is_some())
            .finish_non_exhaustive()
    }
}

impl ChmFile {
    /// Open a CHM archive at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened, has an invalid CHM header, or the directory structure is malformed.
    // `itsf` and `itsp` are the format's own names for the two headers.
    #[allow(clippy::similar_names)]
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut file = File::open(path)?;
        let file_len = file.metadata()?.len();
        if file_len < ITSF_V2_LEN as u64 {
            return Err(ChmError::BadItsf);
        }
        // V2 headers are shorter than V3, so read whichever the file can supply and let
        // the parser decide which layout it is looking at.
        let header_len = ITSF_V3_LEN.min(usize::try_from(file_len).unwrap_or(usize::MAX));
        let header_bytes = read_at(&mut file, 0, header_len)?;
        let itsf = parse_itsf(&header_bytes)?;
        if itsf.data_offset > file_len {
            return Err(ChmError::BadItsf);
        }
        let itsp_offset_end = itsf
            .dir_offset
            .checked_add(ITSP_V1_LEN as u64)
            .ok_or(ChmError::Overflow)?;
        if itsp_offset_end > file_len {
            return Err(ChmError::BadItsp);
        }
        let dir_bytes = read_at(&mut file, itsf.dir_offset, ITSP_V1_LEN)?;
        let itsp = parse_itsp(&dir_bytes)?;
        let first_block = itsf
            .dir_offset
            .checked_add(u64::from(itsp.header_len))
            .ok_or(ChmError::Overflow)?;
        let first_block_end = first_block
            .checked_add(u64::from(itsp.block_len))
            .ok_or(ChmError::Overflow)?;
        if first_block_end > file_len {
            return Err(ChmError::BadItsp);
        }
        let directory = Directory::new(itsf.dir_offset, &itsp);
        let decompressor = Self::load_decompressor(&mut file, &itsf, &directory)?;
        Ok(Self {
            file,
            file_len,
            data_offset: itsf.data_offset,
            directory,
            decompressor,
        })
    }

    /// Try to load the `MSCompressed` decompression machinery. Returns `Ok(None)` if the file has no compressed section.
    fn load_decompressor(
        file: &mut File,
        itsf: &ItsfHeader,
        dir: &Directory,
    ) -> Result<Option<Decompressor>> {
        // The three control entries are themselves stored uncompressed; anything else
        // means the file does not describe a section we can decode.
        let (Some(rt_entry), Some(cn_entry), Some(lzxc_entry)) = (
            find_uncompressed(file, dir, PATH_RESET_TABLE)?,
            find_uncompressed(file, dir, PATH_CONTENT)?,
            find_uncompressed(file, dir, PATH_CONTROL_DATA)?,
        ) else {
            return Ok(None);
        };
        // Both control structures have a fixed layout, so read exactly what the parsers
        // need rather than trusting the stored entry length as an allocation size.
        if rt_entry.length < LZXC_RESET_TABLE_LEN as u64 || lzxc_entry.length < LZXC_MIN_LEN as u64
        {
            return Ok(None);
        }
        let reset_offset = itsf
            .data_offset
            .checked_add(rt_entry.start)
            .ok_or(ChmError::Overflow)?;
        let rt_buf = read_at(file, reset_offset, LZXC_RESET_TABLE_LEN)?;
        let Ok(reset_table) = parse_lzxc_reset_table(&rt_buf) else {
            return Ok(None);
        };
        let control_offset = itsf
            .data_offset
            .checked_add(lzxc_entry.start)
            .ok_or(ChmError::Overflow)?;
        let lzxc_buf = read_at(file, control_offset, LZXC_MIN_LEN)?;
        let Ok(ctl) = parse_lzxc_control_data(&lzxc_buf) else {
            return Ok(None);
        };
        let decomp = Decompressor::new(
            file,
            itsf.data_offset,
            cn_entry.start,
            rt_entry.start,
            rt_entry.length,
            &reset_table,
            &ctl,
        )?;
        Ok(Some(decomp))
    }

    /// Find an entry by path (case-insensitive).
    ///
    /// # Errors
    ///
    /// Returns [`ChmError::NotFound`] if no entry with that path exists.
    pub fn find(&mut self, path: &str) -> Result<Entry> {
        self.directory.find(&mut self.file, path)
    }

    /// Read an entire entry into memory.
    ///
    /// # Errors
    ///
    /// Returns an error if the entry is compressed and compression is unavailable, or if an I/O or decompression error occurs.
    pub fn read(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        if entry.length > MAX_WHOLE_ENTRY_READ_BYTES {
            return Err(ChmError::LimitExceeded("whole-entry read"));
        }
        if entry.length == 0 {
            return Ok(Vec::new());
        }
        match entry.space {
            SPACE_UNCOMPRESSED => {
                let offset = self
                    .data_offset
                    .checked_add(entry.start)
                    .ok_or(ChmError::Overflow)?;
                let end = offset.checked_add(entry.length).ok_or(ChmError::Overflow)?;
                // Check before allocating, so a corrupt length cannot request a huge buffer
                // that the read would only then fail to fill.
                if end > self.file_len {
                    return Err(ChmError::BadPmgl);
                }
                let len = usize::try_from(entry.length).map_err(|_| ChmError::Overflow)?;
                read_at(&mut self.file, offset, len)
            }
            SPACE_COMPRESSED => {
                let decomp = self.decompressor.as_mut().ok_or(ChmError::NoCompression)?;
                decomp.read(&mut self.file, entry.start, entry.length)
            }
            _ => Err(ChmError::NoCompression),
        }
    }

    /// Read a bounded range from an entry into caller-owned memory.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed entry ranges, unavailable compression, or I/O failures.
    pub fn read_range(&mut self, entry: &Entry, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        self.read_range_with(entry, offset, buffer, || true)
    }

    /// Read a bounded range, checking `checkpoint` between LZX blocks.
    ///
    /// # Errors
    ///
    /// Returns [`ChmError::Cancelled`] when `checkpoint` returns `false`, or an error for
    /// malformed entry ranges, unavailable compression, or I/O failures.
    pub fn read_range_with<F>(
        &mut self,
        entry: &Entry,
        offset: u64,
        buffer: &mut [u8],
        mut checkpoint: F,
    ) -> Result<usize>
    where
        F: FnMut() -> bool,
    {
        let request_len = u64::try_from(buffer.len()).map_err(|_| ChmError::Overflow)?;
        let len = usize::try_from(request_len.min(entry.length.saturating_sub(offset)))
            .map_err(|_| ChmError::Overflow)?;
        if len == 0 {
            return Ok(0);
        }
        if !checkpoint() {
            return Err(ChmError::Cancelled);
        }

        match entry.space {
            SPACE_UNCOMPRESSED => {
                let start = self
                    .data_offset
                    .checked_add(entry.start)
                    .and_then(|start| start.checked_add(offset))
                    .ok_or(ChmError::Overflow)?;
                let end = start
                    .checked_add(u64::try_from(len).map_err(|_| ChmError::Overflow)?)
                    .ok_or(ChmError::Overflow)?;
                if end > self.file_len {
                    return Err(ChmError::BadPmgl);
                }
                self.file.seek(SeekFrom::Start(start))?;
                self.file.read_exact(&mut buffer[..len])?;
                Ok(len)
            }
            SPACE_COMPRESSED => {
                let start = entry.start.checked_add(offset).ok_or(ChmError::Overflow)?;
                let decompressor = self.decompressor.as_mut().ok_or(ChmError::NoCompression)?;
                decompressor.read_into(&mut self.file, start, &mut buffer[..len], &mut checkpoint)
            }
            _ => Err(ChmError::NoCompression),
        }
    }

    /// Look up `path` and read its contents in one step.
    ///
    /// # Errors
    ///
    /// Returns [`ChmError::NotFound`] if no entry with that path exists, or any error
    /// [`ChmFile::read`] would return.
    pub fn read_path(&mut self, path: &str) -> Result<Vec<u8>> {
        let entry = self.find(path)?;
        self.read(&entry)
    }

    /// Enumerate all entries matching `sel`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory structure cannot be read.
    pub fn entries(&mut self, sel: EntrySel) -> Result<Vec<Entry>> {
        self.directory.enumerate(&mut self.file, None, sel)
    }

    /// Visit entries without collecting the complete directory in memory.
    ///
    /// Return `false` from the callback to stop enumeration. The boolean result is `true`
    /// only when every matching entry was visited.
    ///
    /// # Errors
    ///
    /// Returns [`ChmError::Cancelled`] when `checkpoint` returns `false`, or an error when
    /// a directory block is malformed or cannot be read.
    pub fn for_each_entry<F, C>(&mut self, sel: EntrySel, checkpoint: C, visit: F) -> Result<bool>
    where
        F: FnMut(Entry) -> bool,
        C: FnMut() -> bool,
    {
        self.directory
            .for_each(&mut self.file, None, sel, checkpoint, visit)
    }

    /// Enumerate entries whose path starts with `prefix`, matching `sel`.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory structure cannot be read.
    pub fn entries_in(&mut self, prefix: &str, sel: EntrySel) -> Result<Vec<Entry>> {
        self.directory.enumerate(&mut self.file, Some(prefix), sel)
    }

    /// Whether the archive has a usable `MSCompressed` section.
    ///
    /// When this is `false`, reading an entry stored in that section fails with
    /// [`ChmError::NoCompression`].
    #[must_use]
    pub const fn has_compression(&self) -> bool {
        self.decompressor.is_some()
    }
}

/// Look up `path`, treating both a missing entry and a compressed one as "absent".
///
/// The `MSCompressed` control entries must be stored uncompressed, since they are what
/// tells us how to decompress everything else.
fn find_uncompressed(file: &mut File, dir: &Directory, path: &str) -> Result<Option<Entry>> {
    match dir.find(file, path) {
        Ok(entry) if entry.is_compressed() => Ok(None),
        Ok(entry) => Ok(Some(entry)),
        Err(ChmError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn read_at(file: &mut File, offset: u64, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut buf)?;
    Ok(buf)
}
