//! Restricted libarchive reader. RAR support is deliberately not registered here.

use libarchive2_sys as sys;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr::NonNull;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ar,
    Cab,
    Cpio,
    Iso9660,
    Lha,
    Rpm,
    Xar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    pub pathname: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub mode: u32,
    pub modified: Option<i64>,
}

/// One archive reader owns one libarchive handle and must not be shared concurrently.
pub struct Reader {
    archive: NonNull<sys::archive>,
}

// SAFETY: the pointer is exclusively owned and the wrapper is movable but not Sync.
unsafe impl Send for Reader {}

impl Reader {
    pub fn open(path: &Path, formats: &[Format], password: Option<&str>) -> Result<Self, String> {
        // SAFETY: This allocates an independent handle without dereferencing caller data.
        let pointer = unsafe { sys::archive_read_new() };
        let archive =
            NonNull::new(pointer).ok_or_else(|| "libarchive allocation failed".to_string())?;
        let mut reader = Self { archive };
        // No decompressor is enabled by default. The RPM path below adds only explicit
        // in-process filters, so untrusted input cannot invoke an external command.
        // SAFETY: `reader` uniquely owns a newly allocated libarchive handle.
        reader.check(unsafe { sys::archive_read_support_filter_none(reader.as_ptr()) })?;
        for format in formats {
            if *format == Format::Rpm {
                // RPM wraps a compressed CPIO payload. Only in-process filters are enabled;
                // the command filter remains disabled for untrusted archives.
                // SAFETY: `reader` uniquely owns a live handle in the unconfigured state.
                unsafe {
                    reader.check(sys::archive_read_support_filter_rpm(reader.as_ptr()))?;
                    reader.check(sys::archive_read_support_filter_gzip(reader.as_ptr()))?;
                    reader.check(sys::archive_read_support_filter_bzip2(reader.as_ptr()))?;
                    reader.check(sys::archive_read_support_filter_xz(reader.as_ptr()))?;
                    reader.check(sys::archive_read_support_filter_zstd(reader.as_ptr()))?;
                    reader.check(sys::archive_read_support_format_cpio(reader.as_ptr()))?;
                }
                continue;
            }
            // SAFETY: `reader` uniquely owns a live handle in the unconfigured state.
            let code = unsafe {
                match format {
                    Format::Ar => sys::archive_read_support_format_ar(reader.as_ptr()),
                    Format::Cab => sys::archive_read_support_format_cab(reader.as_ptr()),
                    Format::Cpio => sys::archive_read_support_format_cpio(reader.as_ptr()),
                    Format::Iso9660 => sys::archive_read_support_format_iso9660(reader.as_ptr()),
                    Format::Lha => sys::archive_read_support_format_lha(reader.as_ptr()),
                    Format::Rpm => sys::archive_read_support_format_cpio(reader.as_ptr()),
                    Format::Xar => sys::archive_read_support_format_xar(reader.as_ptr()),
                }
            };
            reader.check(code)?;
        }
        if let Some(password) = password {
            let secret =
                CString::new(password).map_err(|_| "password contains a NUL byte".to_string())?;
            // SAFETY: The CString and the owned archive handle remain valid for this call.
            reader.check(unsafe {
                sys::archive_read_add_passphrase(reader.as_ptr(), secret.as_ptr())
            })?;
        }
        reader.open_path(path)?;
        Ok(reader)
    }

    pub fn next_entry(&mut self) -> Result<Option<EntryInfo>, String> {
        let mut entry = std::ptr::null_mut();
        // SAFETY: The reader owns a live handle and the output pointer is writable.
        let code = unsafe { sys::archive_read_next_header(self.as_ptr(), &mut entry) };
        if code == sys::ARCHIVE_EOF as i32 {
            return Ok(None);
        }
        self.check(code)?;
        if entry.is_null() {
            return Err("libarchive returned an empty entry header".into());
        }

        // SAFETY: `entry` is the current valid header and has not been advanced.
        let pathname =
            entry_string(entry, true).ok_or_else(|| "archive entry has no pathname".to_string())?;
        // SAFETY: The header remains valid until the next call that advances this reader.
        let filetype = unsafe { sys::archive_entry_filetype(entry) as u32 };
        let kind = match filetype & 0o170000 {
            0o040000 => EntryKind::Directory,
            0o100000 => EntryKind::File,
            _ => EntryKind::Symlink,
        };
        // SAFETY: The header remains valid until the next call that advances this reader.
        let raw_size = unsafe { sys::archive_entry_size(entry) };
        let size = u64::try_from(raw_size).ok();
        if kind == EntryKind::File && size.is_none() {
            return Err("archive file has an invalid length".into());
        }
        // SAFETY: The header remains valid until the next call that advances this reader.
        let mode = unsafe { sys::archive_entry_perm(entry) as u32 };
        // SAFETY: The header remains valid until the next call that advances this reader.
        #[allow(clippy::useless_conversion)] // `time_t` is platform-sized.
        let modified = if unsafe { sys::archive_entry_mtime_is_set(entry) } != 0 {
            // SAFETY: The header remains valid until the next call that advances this reader.
            i64::try_from(unsafe { sys::archive_entry_mtime(entry) }).ok()
        } else {
            None
        };
        Ok(Some(EntryInfo {
            pathname,
            kind,
            size,
            mode,
            modified,
        }))
    }

    pub fn read_data(&mut self, buffer: &mut [u8]) -> Result<usize, String> {
        // SAFETY: `buffer` is writable for its full length and this reader owns a live handle.
        let count = unsafe {
            sys::archive_read_data(self.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len())
        };
        if count < 0 {
            return Err(self.error_message());
        }
        usize::try_from(count).map_err(|_| "libarchive returned an invalid byte count".into())
    }

    pub fn skip_data(&mut self) -> Result<(), String> {
        // SAFETY: The reader owns a live handle and has an active entry.
        let code = unsafe { sys::archive_read_data_skip(self.as_ptr()) };
        self.check(code)
    }

    fn open_path(&mut self, path: &Path) -> Result<(), String> {
        #[cfg(unix)]
        let code = {
            use std::os::unix::ffi::OsStrExt;
            let filename = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| "archive path contains a NUL byte".to_string())?;
            // SAFETY: The NUL-terminated filename and owned archive handle remain valid for this call.
            unsafe { sys::archive_read_open_filename(self.as_ptr(), filename.as_ptr(), 64 * 1024) }
        };
        #[cfg(windows)]
        let code = {
            use std::os::windows::ffi::OsStrExt;
            let mut filename: Vec<sys::wchar_t> = path
                .as_os_str()
                .encode_wide()
                .map(|unit| unit as sys::wchar_t)
                .collect();
            filename.push(0 as sys::wchar_t);
            // SAFETY: The terminated UTF-16 path and owned archive handle remain valid for this call.
            unsafe {
                sys::archive_read_open_filename_w(self.as_ptr(), filename.as_ptr(), 64 * 1024)
            }
        };
        #[cfg(not(any(unix, windows)))]
        let code = {
            let filename = CString::new(
                path.to_str()
                    .ok_or_else(|| "archive path is not valid UTF-8".to_string())?,
            )
            .map_err(|_| "archive path contains a NUL byte".to_string())?;
            // SAFETY: The NUL-terminated filename and owned archive handle remain valid for this call.
            unsafe { sys::archive_read_open_filename(self.as_ptr(), filename.as_ptr(), 64 * 1024) }
        };
        self.check(code)
    }

    fn as_ptr(&self) -> *mut sys::archive {
        self.archive.as_ptr()
    }

    fn check(&self, code: i32) -> Result<(), String> {
        if code < 0 {
            Err(self.error_message())
        } else {
            Ok(())
        }
    }

    fn error_message(&self) -> String {
        // SAFETY: The live archive handle returns a borrowed NUL-terminated error string.
        let message = unsafe { sys::archive_error_string(self.as_ptr()) };
        if message.is_null() {
            return "libarchive could not read this archive".into();
        }
        // SAFETY: libarchive documents the error string as a valid NUL-terminated string.
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: This object uniquely owns the live archive handle until drop.
        unsafe {
            sys::archive_read_close(self.as_ptr());
            sys::archive_read_free(self.as_ptr());
        }
    }
}

fn entry_string(entry: *mut sys::archive_entry, utf8: bool) -> Option<String> {
    // SAFETY: The caller passes the valid current header returned by archive_read_next_header.
    let pathname = unsafe {
        if utf8 {
            sys::archive_entry_pathname_utf8(entry)
        } else {
            sys::archive_entry_pathname(entry)
        }
    };
    if pathname.is_null() {
        None
    } else {
        // SAFETY: The current archive header owns a NUL-terminated pathname.
        Some(
            unsafe { CStr::from_ptr(pathname) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}
