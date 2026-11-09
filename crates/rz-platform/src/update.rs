//! OS operations used by the independent updater.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started: u64,
    pub executable: PathBuf,
}

pub fn process(pid: u32) -> Option<ProcessIdentity> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        ProcessRefreshKind::new().with_exe(UpdateKind::Always),
    );
    let process = system.process(Pid::from_u32(pid))?;
    // Zombies have no executable and must not keep an update waiting forever.
    Some(ProcessIdentity {
        pid,
        started: process.start_time(),
        executable: process.exe()?.to_path_buf(),
    })
}

pub fn open_regular(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_link_or_reparse(&metadata) {
        return Err(io::Error::other("update input is not a regular file"));
    }
    Ok(file)
}

pub fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

pub fn same_process(pid: u32, started: u64) -> bool {
    process(pid).is_some_and(|p| p.started == started)
}

pub fn other_instances(executable: &Path, except: u32) -> bool {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        ProcessRefreshKind::new().with_exe(UpdateKind::Always),
    );
    system.processes().iter().any(|(pid, process)| {
        pid.as_u32() != except
            && process
                .exe()
                .is_some_and(|p| crate::same_file(p, executable))
    })
}

pub fn detached(command: &mut Command) -> io::Result<Child> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00000200);
    }
    command.spawn()
}

pub fn request_exit(pid: u32, started: u64) -> io::Result<()> {
    if !same_process(pid, started) {
        return Ok(());
    }
    #[cfg(windows)]
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .status()?;
    #[cfg(unix)]
    let status = Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("could not request application exit"))
    }
}

pub fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

pub fn durable_rename(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::durable_rename(source, destination)
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, destination)?;
        if let Some(parent) = destination.parent() {
            sync_directory(parent)?;
        }
        if source.parent() != destination.parent() {
            if let Some(parent) = source.parent() {
                sync_directory(parent)?;
            }
        }
        Ok(())
    }
}

pub fn link_directory(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(source, target)
    }
}

pub fn link_file(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(source, target)
    }
}

pub fn elevated() -> bool {
    #[cfg(windows)]
    {
        windows::elevated()
    }
    #[cfg(unix)]
    {
        #[allow(unsafe_code)]
        // SAFETY: geteuid has no arguments or memory preconditions.
        unsafe {
            libc::geteuid() == 0
        }
    }
}

/// Only an installed, protected helper may be used as a privilege entry point.
pub fn authorize(helper: &Path, arguments: &[std::ffi::OsString]) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::authorize(helper, arguments)
    }
    #[cfg(not(windows))]
    {
        let status = Command::new("/usr/bin/pkexec")
            .arg(helper)
            .args(arguments)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "system authorization was cancelled or unavailable",
            ))
        }
    }
}

pub fn protected_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::protected_directory(path)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
}

pub fn private_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::private_directory(path)
    }
    #[cfg(not(windows))]
    {
        protected_directory(path)
    }
}

pub fn installation_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        if elevated() {
            return windows::installation_directory(path);
        }
    }
    fs::create_dir(path)
}

pub fn create_update_file(path: &Path, public_read: bool) -> io::Result<File> {
    #[cfg(windows)]
    {
        if elevated() {
            return windows::create_update_file(path, public_read);
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if public_read { 0o644 } else { 0o600 });
    }
    #[cfg(not(unix))]
    {
        let _ = public_read;
    }
    options.open(path)
}

pub fn copy_update_permissions(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        if elevated() {
            windows::copy_permissions(source, destination)?;
        }
    }
    fs::set_permissions(destination, fs::metadata(source)?.permissions())
}

pub fn validate_protected_installation(root: &Path, parent_pid: u32) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::validate_protected_installation(root, parent_pid)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = parent_pid;
        for path in root.ancestors() {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || metadata.mode() & 0o022 != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "the system installation is not protected",
                ));
            }
        }
        let mut paths = vec![root.to_owned()];
        let mut checked = 0usize;
        while let Some(path) = paths.pop() {
            checked += 1;
            if checked > 100000 || path.components().count() > root.components().count() + 32 {
                return Err(io::Error::other(
                    "system installation exceeds its inspection budget",
                ));
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() && path == root.join("current") {
                let target = fs::read_link(&path)?;
                let parts: Vec<_> = target.components().collect();
                if parts.len() != 2
                    || parts[0].as_os_str() != "releases"
                    || !parts[1].as_os_str().to_str().is_some_and(|id| {
                        id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                {
                    return Err(io::Error::other(
                        "system installation has an invalid current version",
                    ));
                }
                continue;
            }
            if metadata.uid() != 0
                || metadata.mode() & 0o022 != 0
                || (!metadata.is_dir() && !metadata.is_file())
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "system installation contains an unprotected file",
                ));
            }
            if metadata.is_dir() {
                for entry in fs::read_dir(&path)? {
                    if paths.len() + checked > 100000 {
                        return Err(io::Error::other(
                            "system installation exceeds its inspection budget",
                        ));
                    }
                    paths.push(entry?.path());
                }
            }
        }
        Ok(())
    }
}

pub fn validate_system_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for path in directory.ancestors() {
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || metadata.mode() & 0o022 != 0
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "system resource directory is not protected",
                ));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Err(io::Error::other("not a Unix system installation"))
    }
}

pub fn registered_windows_root(machine: bool) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        windows::registered_root(machine)
    }
    #[cfg(not(windows))]
    {
        let _ = machine;
        Err(io::Error::other("not a Windows installation"))
    }
}

pub fn macos_bundle(executable: &Path) -> Option<PathBuf> {
    if std::env::consts::OS != "macos" {
        return None;
    }
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_owned())
}

pub fn externally_managed(executable: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        if ["FLATPAK_ID", "SNAP", "APPIMAGE"]
            .iter()
            .any(|key| std::env::var_os(key).is_some())
        {
            return true;
        }
        for (program, option) in [("/usr/bin/dpkg-query", "-S"), ("/usr/bin/rpm", "-qf")] {
            if Path::new(program).is_file()
                && Command::new(program)
                    .arg(option)
                    .arg(executable)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
            {
                return true;
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = executable;
    }
    false
}

pub fn register_recovery(executable: &Path, journal: &Path, machine: bool) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::register_recovery(executable, journal, machine)
    }
    #[cfg(not(windows))]
    {
        let _ = (executable, journal, machine);
        Ok(())
    }
}

pub fn clear_recovery(machine: bool) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::clear_recovery(machine)
    }
    #[cfg(not(windows))]
    {
        let _ = machine;
        Ok(())
    }
}

pub fn record_windows_version(machine: bool, version: &str) -> io::Result<()> {
    #[cfg(windows)]
    {
        windows::record_version(machine, version)
    }
    #[cfg(not(windows))]
    {
        let _ = (machine, version);
        Ok(())
    }
}
