use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::SystemServices::MAXIMUM_ALLOWED;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::other("invalid Windows path"));
    }
    value.push(0);
    Ok(value)
}

pub fn durable_rename(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide(source.as_os_str())?;
    let destination = wide(destination.as_os_str())?;
    // SAFETY: both path buffers are null-terminated and remain valid for the call.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns a non-null kernel handle returned by Windows.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub fn elevated() -> bool {
    let mut token = null_mut();
    // SAFETY: valid output pointer and a pseudo handle for the current process.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let token = Handle(token);
    let mut elevation: TOKEN_ELEVATION = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut length = 0;
    // SAFETY: the buffer matches TOKEN_ELEVATION and its exact size is supplied.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        ) != 0
            && elevation.TokenIsElevated != 0
    }
}

fn quote(value: &OsStr) -> OsString {
    let mut out = vec![b'"' as u16];
    let mut slashes = 0;
    for c in value.encode_wide() {
        if c == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        out.extend(std::iter::repeat_n(
            b'\\' as u16,
            if c == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        out.push(c);
        slashes = 0;
    }
    out.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    out.push(b'"' as u16);
    OsString::from_wide(&out)
}

pub fn authorize(helper: &Path, args: &[OsString]) -> io::Result<()> {
    let file = wide(helper.as_os_str())?;
    let mut joined = OsString::new();
    for argument in args {
        if !joined.is_empty() {
            joined.push(" ");
        }
        joined.push(quote(argument));
    }
    let parameters = wide(&joined)?;
    let verb = wide(OsStr::new("runas"))?;
    // SAFETY: zero is a valid initial value for SHELLEXECUTEINFOW, with cbSize set below.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = SW_HIDE;
    // SAFETY: all UTF-16 strings and the writable struct outlive the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(info.hProcess);
    // SAFETY: ShellExecuteExW with NOCLOSEPROCESS returned this owned process handle.
    if unsafe { WaitForSingleObject(process.0, 180_000) } != WAIT_OBJECT_0 {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "system authorization did not finish",
        ));
    }
    let mut code = 1;
    // SAFETY: valid process handle and output pointer.
    if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 || code != 0 {
        return Err(io::Error::other("authorized update preparation failed"));
    }
    Ok(())
}

pub fn protected_directory(path: &Path) -> io::Result<()> {
    let text = wide(OsStr::new("O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)"))?;
    let path = wide(path.as_os_str())?;
    let mut descriptor = null_mut();
    // SAFETY: valid null-terminated SDDL and output pointer; descriptor is freed below.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: the descriptor remains allocated until CreateDirectoryW returns.
    let ok = unsafe { CreateDirectoryW(path.as_ptr(), &attributes) };
    let error = io::Error::last_os_error();
    // SAFETY: descriptor was allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
    unsafe {
        LocalFree(descriptor);
    }
    if ok == 0 {
        Err(error)
    } else {
        Ok(())
    }
}

pub fn private_directory(path: &Path) -> io::Result<()> {
    if elevated() {
        return protected_directory(path);
    }
    let text = wide(OsStr::new(
        "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)",
    ))?;
    let path = wide(path.as_os_str())?;
    let mut descriptor = null_mut();
    // SAFETY: terminated SDDL and valid descriptor output pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: the path and descriptor remain allocated until CreateDirectoryW returns.
    let ok = unsafe { CreateDirectoryW(path.as_ptr(), &attributes) };
    let error = io::Error::last_os_error();
    // SAFETY: the descriptor was allocated by the SDDL conversion API.
    unsafe {
        LocalFree(descriptor);
    }
    if ok == 0 {
        Err(error)
    } else {
        Ok(())
    }
}

pub fn installation_directory(path: &Path) -> io::Result<()> {
    let text = wide(OsStr::new(
        "O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;GRGX;;;AU)",
    ))?;
    let path = wide(path.as_os_str())?;
    let mut descriptor = null_mut();
    // SAFETY: terminated SDDL and valid descriptor output pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: the descriptor and path outlive the synchronous Windows call.
    let ok = unsafe { CreateDirectoryW(path.as_ptr(), &attributes) };
    let error = io::Error::last_os_error();
    // SAFETY: the conversion API allocated this descriptor with LocalAlloc.
    unsafe {
        LocalFree(descriptor);
    }
    if ok == 0 {
        Err(error)
    } else {
        Ok(())
    }
}

pub fn create_update_file(path: &Path, public_read: bool) -> io::Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let text = wide(OsStr::new(if public_read {
        "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;GRGX;;;AU)"
    } else {
        "O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)"
    }))?;
    let path = wide(path.as_os_str())?;
    let mut descriptor = null_mut();
    // SAFETY: valid SDDL and writable output pointer.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: path and explicit security descriptor remain live; CREATE_NEW refuses existing objects.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    let error = io::Error::last_os_error();
    // SAFETY: descriptor was allocated by the conversion API.
    unsafe {
        LocalFree(descriptor);
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(error);
    }
    // SAFETY: CreateFileW returned a newly owned file handle; File takes sole ownership.
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
}

pub fn copy_permissions(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide(source.as_os_str())?;
    let destination = wide(destination.as_os_str())?;
    let mut descriptor = null_mut();
    // SAFETY: valid path and descriptor output pointer; only the DACL is requested.
    let status = unsafe {
        GetNamedSecurityInfoW(
            source.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    // SAFETY: descriptor contains the source DACL and remains allocated through the call.
    let ok = unsafe {
        SetFileSecurityW(
            destination.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        )
    };
    let error = io::Error::last_os_error();
    // SAFETY: GetNamedSecurityInfoW allocated this descriptor with LocalAlloc.
    unsafe {
        LocalFree(descriptor);
    }
    if ok == 0 {
        Err(error)
    } else {
        Ok(())
    }
}

pub fn registered_root(machine: bool) -> io::Result<PathBuf> {
    let key = wide(OsStr::new("Software\\Yuhuo\\RustZIP"))?;
    let value = wide(OsStr::new("InstallDir"))?;
    let mut data = vec![0u16; 32768];
    let mut length = (data.len() * 2) as u32;
    // SAFETY: the byte size describes the writable UTF-16 buffer; key names are terminated.
    let status = unsafe {
        RegGetValueW(
            if machine {
                HKEY_LOCAL_MACHINE
            } else {
                HKEY_CURRENT_USER
            },
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            null_mut(),
            data.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    data.truncate(length as usize / 2);
    while data.last() == Some(&0) {
        data.pop();
    }
    Ok(PathBuf::from(OsString::from_wide(&data)))
}

pub fn validate_protected_installation(root: &Path, parent_pid: u32) -> io::Result<()> {
    if !crate::same_file(root, &registered_root(true)?) {
        return Err(io::Error::other(
            "machine installation registration does not match",
        ));
    }
    // SAFETY: OpenProcess has no borrowed pointers; the handle is wrapped immediately.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, parent_pid) };
    if process.is_null() {
        return Err(io::Error::last_os_error());
    }
    let process = Handle(process);
    let mut token = null_mut();
    // SAFETY: valid handle and output pointer.
    if unsafe { OpenProcessToken(process.0, TOKEN_QUERY | TOKEN_DUPLICATE, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    let mut impersonation = null_mut();
    // SAFETY: a live token handle and valid output pointer are supplied.
    if unsafe { DuplicateToken(token.0, SecurityImpersonation, &mut impersonation) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let impersonation = Handle(impersonation);
    let mut paths: Vec<_> = root
        .ancestors()
        .map(|path| (path.to_owned(), path == root))
        .collect();
    let mut checked = 0usize;
    while let Some((path, check_contents)) = paths.pop() {
        checked += 1;
        if checked > 100000 || path.components().count() > root.components().count() + 32 {
            return Err(io::Error::other(
                "protected installation exceeds its inspection limit",
            ));
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if super::is_link_or_reparse(&metadata) {
            return Err(io::Error::other("protected installation contains a link"));
        }
        let name = wide(path.as_os_str())?;
        let mut descriptor = null_mut();
        // SAFETY: valid path and output pointer; allocated security descriptor is freed below.
        let status = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };
        let mut privileges = [0u64; 128];
        let mut length = std::mem::size_of_val(&privileges) as u32;
        let mut granted = 0;
        let mut allowed = 0;
        // SAFETY: aligned privilege buffer has the supplied size; all other inputs are live Windows structures.
        let ok = unsafe {
            AccessCheck(
                descriptor,
                impersonation.0,
                MAXIMUM_ALLOWED,
                &mapping,
                privileges.as_mut_ptr().cast(),
                &mut length,
                &mut granted,
                &mut allowed,
            )
        };
        let error = io::Error::last_os_error();
        // SAFETY: GetNamedSecurityInfoW allocated this descriptor with LocalAlloc.
        unsafe {
            LocalFree(descriptor);
        }
        if ok == 0 {
            return Err(error);
        }
        let mut forbidden = WRITE_DAC | WRITE_OWNER | FILE_DELETE_CHILD | DELETE;
        if check_contents {
            forbidden |=
                FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_ATTRIBUTES | FILE_WRITE_EA | DELETE;
        }
        if allowed != 0 && granted & forbidden != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "installation can be modified without administrator authorization",
            ));
        }
        if check_contents && metadata.is_dir() {
            for entry in std::fs::read_dir(&path)? {
                if paths.len() + checked > 100000 {
                    return Err(io::Error::other(
                        "protected installation exceeds its inspection limit",
                    ));
                }
                paths.push((entry?.path(), true));
            }
        }
    }
    Ok(())
}

fn recovery_key(machine: bool, create: bool) -> io::Result<HKEY> {
    let name = wide(OsStr::new(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
    ))?;
    let hive = if machine {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };
    let mut key = null_mut();
    // SAFETY: terminated path and writable handle output pointer; no security attributes are supplied.
    let status = unsafe {
        if create {
            RegCreateKeyExW(
                hive,
                name.as_ptr(),
                0,
                null(),
                0,
                KEY_SET_VALUE | KEY_WOW64_64KEY,
                null(),
                &mut key,
                null_mut(),
            )
        } else {
            RegOpenKeyExW(
                hive,
                name.as_ptr(),
                0,
                KEY_SET_VALUE | KEY_WOW64_64KEY,
                &mut key,
            )
        }
    };
    if status != ERROR_SUCCESS {
        Err(io::Error::from_raw_os_error(status as i32))
    } else {
        Ok(key)
    }
}

pub fn register_recovery(executable: &Path, journal: &Path, machine: bool) -> io::Result<()> {
    let key = recovery_key(machine, true)?;
    let mut command = quote(executable.as_os_str());
    command.push(" --recover ");
    command.push(quote(journal.as_os_str()));
    let data = wide(&command)?;
    let name = wide(OsStr::new("RustZIPUpdateRecovery"))?;
    // SAFETY: registry key and UTF-16 byte buffer are live and correctly sized.
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    // SAFETY: recovery_key returned this owned registry handle.
    unsafe {
        RegCloseKey(key);
    }
    if status != ERROR_SUCCESS {
        Err(io::Error::from_raw_os_error(status as i32))
    } else {
        Ok(())
    }
}

pub fn clear_recovery(machine: bool) -> io::Result<()> {
    let key = match recovery_key(machine, false) {
        Ok(key) => key,
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => return Ok(()),
        Err(error) => return Err(error),
    };
    let name = wide(OsStr::new("RustZIPUpdateRecovery"))?;
    // SAFETY: the handle and terminated value name are valid.
    let status = unsafe { RegDeleteValueW(key, name.as_ptr()) };
    // SAFETY: recovery_key returned this owned registry handle.
    unsafe {
        RegCloseKey(key);
    }
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

pub fn record_version(machine: bool, version: &str) -> io::Result<()> {
    let path = wide(OsStr::new("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{6F2A5B1E-3C0D-4E0B-9B8A-5D1F7A9C2E41}_is1"))?;
    let mut key = null_mut();
    // SAFETY: the terminated path and output handle pointer are valid.
    let status = unsafe {
        RegOpenKeyExW(
            if machine {
                HKEY_LOCAL_MACHINE
            } else {
                HKEY_CURRENT_USER
            },
            path.as_ptr(),
            0,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let name = wide(OsStr::new("DisplayVersion"))?;
    let value = wide(OsStr::new(version))?;
    // SAFETY: the UTF-16 value remains valid for the supplied byte length.
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    // SAFETY: RegOpenKeyExW returned this owned registry handle.
    unsafe {
        RegCloseKey(key);
    }
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}
