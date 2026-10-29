//! Bounded, callback based access to the optional unRAR reader.

use std::ffi::CString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use unrar_sys as sys;
use zeroize::Zeroize;

#[derive(Debug)]
pub enum Error {
    BadPassword,
    MissingPassword,
    Corrupt(i32),
    UnknownFormat,
    NoMemory,
    Cancelled,
    OutputLimit,
    InvalidInput(&'static str),
    Io(io::Error),
    Other(i32),
}

#[derive(Debug, Clone)]
pub struct EntryInfo {
    pub path: PathBuf,
    pub size: u64,
    pub packed_size: u64,
    pub modified: u32,
    pub encrypted: bool,
    pub is_directory: bool,
}

struct Handle(NonNull<sys::Handle>);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: This wrapper uniquely owns the live handle returned by RAROpenArchiveEx.
        unsafe { sys::RARCloseArchive(self.0.as_ptr()) };
    }
}

pub struct Reader {
    handle: Handle,
    current: bool,
}

impl Reader {
    pub fn open_for_listing(path: &Path, password: Option<&str>) -> Result<Self, Error> {
        Self::open(path, password, sys::RAR_OM_LIST)
    }

    pub fn open_for_processing(path: &Path, password: Option<&str>) -> Result<Self, Error> {
        Self::open(path, password, sys::RAR_OM_EXTRACT)
    }

    fn open(path: &Path, password: Option<&str>, mode: u32) -> Result<Self, Error> {
        #[cfg(any(target_os = "linux", target_os = "netbsd"))]
        let filename = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| Error::InvalidInput("archive path contains a NUL byte"))?;
        #[cfg(not(any(target_os = "linux", target_os = "netbsd")))]
        let filename = widestring::WideCString::from_os_str(path)
            .map_err(|_| Error::InvalidInput("archive path contains a NUL character"))?;

        #[cfg(any(target_os = "linux", target_os = "netbsd"))]
        let mut data = sys::OpenArchiveDataEx::new(filename.as_ptr(), mode);
        #[cfg(not(any(target_os = "linux", target_os = "netbsd")))]
        let mut data = sys::OpenArchiveDataEx::new(filename.as_ptr().cast::<sys::WCHAR>(), mode);
        // SAFETY: `data` and its filename storage remain alive for the synchronous call.
        let data_ptr = std::ptr::from_mut(&mut data).cast_const();
        let raw = unsafe { sys::RAROpenArchiveEx(data_ptr) };
        let code = data.open_result as i32;
        let Some(handle) = NonNull::new(raw as *mut sys::Handle) else {
            return Err(map_code(code));
        };
        let handle = Handle(handle);
        if code != sys::ERAR_SUCCESS {
            return Err(map_code(code));
        }

        if let Some(password) = password {
            let secret = CString::new(password)
                .map_err(|_| Error::InvalidInput("password contains a NUL byte"))?;
            // SAFETY: The handle is live and CString provides a NUL terminated password.
            unsafe { sys::RARSetPassword(handle.0.as_ptr(), secret.as_ptr()) };
            let mut bytes = secret.into_bytes_with_nul();
            bytes.zeroize();
        }

        Ok(Self {
            handle,
            current: false,
        })
    }

    pub fn next_entry(&mut self) -> Result<Option<EntryInfo>, Error> {
        if self.current {
            return Err(Error::Other(sys::ERAR_UNKNOWN));
        }
        let mut header = sys::HeaderDataEx::default();
        // SAFETY: UnRAR may request a volume change while reading the header; the callback
        // remains installed only for this synchronous call and uses no process-data context.
        let code = unsafe {
            sys::RARSetCallback(self.handle.0.as_ptr(), Some(process_callback), 0);
            // The C API writes `header` although this binding declares the pointer as const.
            let header_ptr = std::ptr::from_mut(&mut header).cast_const();
            let code = sys::RARReadHeaderEx(self.handle.0.as_ptr(), header_ptr);
            sys::RARSetCallback(self.handle.0.as_ptr(), None, 0);
            code
        };
        match code {
            sys::ERAR_END_ARCHIVE => Ok(None),
            sys::ERAR_SUCCESS => {
                self.current = true;
                #[cfg(windows)]
                let path = unsafe {
                    // SAFETY: UnRAR fills this fixed-size array with a terminated wide path.
                    widestring::WideCString::from_ptr_truncate(
                        header.filename_w.as_ptr(),
                        header.filename_w.len(),
                    )
                };
                #[cfg(not(windows))]
                let path = unsafe {
                    // SAFETY: UnRAR fills this fixed-size array with a terminated wide path;
                    // both native wchar_t and Rust wide units are 32-bit on these targets.
                    widestring::WideCString::from_ptr_truncate(
                        header.filename_w.as_ptr().cast(),
                        header.filename_w.len(),
                    )
                };
                Ok(Some(EntryInfo {
                    path: PathBuf::from(path.to_os_string()),
                    size: combine(header.unp_size, header.unp_size_high),
                    packed_size: combine(header.pack_size, header.pack_size_high),
                    modified: header.file_time,
                    encrypted: header.flags & sys::RHDF_ENCRYPTED != 0,
                    is_directory: header.flags & sys::RHDF_DIRECTORY != 0,
                }))
            }
            other => Err(map_code(other)),
        }
    }

    pub fn skip_current(&mut self) -> Result<(), Error> {
        self.process_current(sys::RAR_SKIP, None, u64::MAX, &|| false)
            .map(|_| ())
    }

    pub fn test_current(
        &mut self,
        max_output: u64,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<u64, Error> {
        self.process_current(sys::RAR_TEST, None, max_output, cancelled)
    }

    pub fn extract_current_to(
        &mut self,
        output: &mut dyn Write,
        max_output: u64,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<u64, Error> {
        self.process_current(sys::RAR_TEST, Some(output), max_output, cancelled)
    }

    fn process_current<'a>(
        &mut self,
        operation: i32,
        output: Option<&'a mut dyn Write>,
        max_output: u64,
        cancelled: &'a dyn Fn() -> bool,
    ) -> Result<u64, Error> {
        if !self.current {
            return Err(Error::Other(sys::ERAR_UNKNOWN));
        }

        let mut context = ProcessContext {
            output,
            max_output,
            written: 0,
            cancelled,
            failure: None,
        };
        let user_data = (&mut context as *mut ProcessContext<'_>).cast::<()>() as sys::LPARAM;
        // SAFETY: `context` stays alive and uniquely borrowed until RARProcessFile returns.
        // UnRAR invokes this callback synchronously while processing this one current entry.
        let code = unsafe {
            sys::RARSetCallback(self.handle.0.as_ptr(), Some(process_callback), user_data);
            let code = sys::RARProcessFile(
                self.handle.0.as_ptr(),
                operation,
                std::ptr::null(),
                std::ptr::null(),
            );
            sys::RARSetCallback(self.handle.0.as_ptr(), None, 0);
            code
        };
        self.current = false;

        if let Some(error) = context.failure {
            return Err(error);
        }
        if code != sys::ERAR_SUCCESS {
            return Err(map_code(code));
        }
        Ok(context.written)
    }
}

struct ProcessContext<'a> {
    output: Option<&'a mut dyn Write>,
    max_output: u64,
    written: u64,
    cancelled: &'a dyn Fn() -> bool,
    failure: Option<Error>,
}

extern "C" fn process_callback(
    message: sys::UINT,
    user_data: sys::LPARAM,
    data_ptr: sys::LPARAM,
    data_len: sys::LPARAM,
) -> std::os::raw::c_int {
    if message == sys::UCM_CHANGEVOLUMEW {
        return if data_len == sys::RAR_VOL_ASK { -1 } else { 0 };
    }
    if message != sys::UCM_PROCESSDATA || user_data == 0 {
        return 0;
    }

    // SAFETY: `user_data` points to the stack context held across the synchronous native call.
    let context = unsafe { &mut *(user_data as *mut ProcessContext<'static>) };
    if (context.cancelled)() {
        context.failure = Some(Error::Cancelled);
        return -1;
    }
    let Ok(length) = usize::try_from(data_len) else {
        context.failure = Some(Error::Corrupt(sys::ERAR_BAD_DATA));
        return -1;
    };
    if length == 0 {
        return 0;
    }
    if data_ptr == 0 {
        context.failure = Some(Error::Corrupt(sys::ERAR_BAD_DATA));
        return -1;
    }
    let Some(next_written) = context.written.checked_add(length as u64) else {
        context.failure = Some(Error::OutputLimit);
        return -1;
    };
    if next_written > context.max_output {
        context.failure = Some(Error::OutputLimit);
        return -1;
    }

    // SAFETY: UnRAR supplies a valid `data_len`-byte buffer for the callback duration.
    let bytes = unsafe { std::slice::from_raw_parts(data_ptr as *const u8, length) };
    if let Some(output) = context.output.as_deref_mut() {
        if let Err(error) = output.write_all(bytes) {
            context.failure = Some(Error::Io(error));
            return -1;
        }
    }
    context.written = next_written;
    0
}

fn combine(low: u32, high: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn map_code(code: i32) -> Error {
    match code {
        sys::ERAR_MISSING_PASSWORD => Error::MissingPassword,
        sys::ERAR_BAD_PASSWORD => Error::BadPassword,
        sys::ERAR_BAD_DATA | sys::ERAR_BAD_ARCHIVE => Error::Corrupt(code),
        sys::ERAR_UNKNOWN_FORMAT => Error::UnknownFormat,
        sys::ERAR_NO_MEMORY => Error::NoMemory,
        sys::ERAR_SUCCESS => Error::Other(sys::ERAR_UNKNOWN),
        other => Error::Other(other),
    }
}
