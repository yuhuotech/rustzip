//! Independent updater orchestration and authenticated local startup receipts.

use crate::update::{Update, UpdateTask};
use crate::update_install::{self as install, InstallKind, Installation, Phase, Transaction};
use rz_core::{CancelToken, Error, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(45);
const PREPARE_TIMEOUT: Duration = Duration::from_secs(180);
static STARTUP_PENDING: AtomicBool = AtomicBool::new(false);
static STARTUP: OnceLock<Mutex<Option<StartupSession>>> = OnceLock::new();
static STARTUP_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Message {
    Hello {
        token: String,
        pid: u32,
        started: u64,
    },
    Ready,
    Armed,
    Published,
    Commit,
    Committed,
    Rollback,
    Deferred,
    Failed {
        detail: String,
    },
}

fn send(stream: &mut TcpStream, message: &Message) -> Result<()> {
    let bytes = serde_json::to_vec(message)
        .map_err(|_| Error::Corrupt("invalid updater message".into()))?;
    if bytes.len() > 65536 {
        return Err(Error::Corrupt("updater message too large".into()));
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    Ok(())
}

fn receive(stream: &mut TcpStream) -> Result<Message> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > 65536 {
        return Err(Error::Corrupt("updater message too large".into()));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Corrupt("invalid updater message".into()))
}

fn listener() -> Result<TcpListener> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn accept(
    listener: &TcpListener,
    token: &str,
    timeout: Duration,
    expected: Option<&Path>,
) -> Result<(TcpStream, u32)> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                stream.set_nonblocking(false)?;
                if !peer.ip().is_loopback() {
                    continue;
                }
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                if let Ok(Message::Hello {
                    token: actual,
                    pid,
                    started,
                }) = receive(&mut stream)
                {
                    let identity_matches = if let Some(executable) = expected {
                        rz_platform::update::process(pid).is_some_and(|p| {
                            p.started == started
                                && rz_platform::same_file(executable, &p.executable)
                        })
                    } else {
                        pid != 0 && started != 0
                    };
                    if actual == token && identity_matches {
                        stream.set_read_timeout(Some(PREPARE_TIMEOUT))?;
                        return Ok((stream, pid));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::Io(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "RustZIP updater did not respond",
    )))
}

fn connect(address: &str, token: &str) -> Result<TcpStream> {
    let token = resolve_token(token)?;
    let address: SocketAddr = address
        .parse()
        .map_err(|_| Error::Corrupt("invalid updater endpoint".into()))?;
    if !address.ip().is_loopback()
        || token.len() != 64
        || !token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::UnsafePath("invalid local updater endpoint".into()));
    }
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(PREPARE_TIMEOUT))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let p = rz_platform::update::process(std::process::id())
        .ok_or_else(|| Error::Corrupt("could not identify update process".into()))?;
    send(
        &mut stream,
        &Message::Hello {
            token,
            pid: p.pid,
            started: p.started,
        },
    )?;
    Ok(stream)
}

fn nonce() -> Result<String> {
    Ok(format!(
        "{}{}",
        install::transaction_id()?,
        install::transaction_id()?
    ))
}

fn token_reference(directory: &Path, name: &str, token: &str) -> Result<String> {
    let path = directory.join(name);
    let mut file = rz_platform::update::create_update_file(&path, false)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    let path = path.to_str().ok_or_else(|| {
        Error::Unsupported("The update temporary directory must have a Unicode path.".into())
    })?;
    Ok(format!("@{path}"))
}

fn resolve_token(reference: &str) -> Result<String> {
    let token = if let Some(path) = reference.strip_prefix('@') {
        let mut bytes = Vec::new();
        rz_platform::update::open_regular(Path::new(path))?
            .take(65)
            .read_to_end(&mut bytes)?;
        String::from_utf8(bytes)
            .map_err(|_| Error::Corrupt("invalid local update capability".into()))?
    } else {
        reference.to_owned()
    };
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Corrupt("invalid local update capability".into()));
    }
    Ok(token)
}

fn expected(message: Message, wanted: &str) -> Result<()> {
    match (message, wanted) {
        (Message::Ready, "ready")
        | (Message::Armed, "armed")
        | (Message::Published, "published")
        | (Message::Committed, "committed") => Ok(()),
        (Message::Failed { detail }, _) => Err(Error::Unsupported(detail)),
        _ => Err(Error::Corrupt("unexpected updater response".into())),
    }
}

/// Runs outside the GUI thread. A successful return means the independent
/// updater has prepared the transaction and obtained required authorization.
pub fn launch_update(update: &Update, archive: &Path, key: &[u8; 32]) -> Result<()> {
    let installation = Installation::locate(&std::env::current_exe()?)?;
    installation.verify_registration()?;
    if installation.machine && rz_platform::update::elevated() {
        return Err(Error::Unsupported(
            "Start RustZIP as a normal user before updating a system installation.".into(),
        ));
    }
    let p = rz_platform::update::process(std::process::id())
        .ok_or_else(|| Error::Corrupt("could not identify RustZIP process".into()))?;
    let task = UpdateTask {
        transaction_id: install::transaction_id()?,
        version: update.version.to_string(),
        asset: update.asset.clone(),
        archive: archive.canonicalize()?,
        installation: installation.root.clone(),
        parent_pid: p.pid,
        parent_start_time: p.started,
    };
    let platform = match installation.kind {
        InstallKind::Windows => "windows-x86_64-zip".to_string(),
        InstallKind::Linux => crate::update::current_platform(),
    };
    task.validate(env!("CARGO_PKG_VERSION"), &platform, key)?;
    let directory = archive
        .parent()
        .ok_or_else(|| Error::UnsafePath("missing download directory".into()))?;
    let task_file = directory.join("task.json");
    install::write_json(&task_file, &task)?;
    let helper = directory.join(if installation.kind == InstallKind::Windows {
        "runner.exe"
    } else {
        "runner"
    });
    fs::copy(installation.helper(), &helper)?;
    let server = listener()?;
    let token = nonce()?;
    let reference = token_reference(directory, "controller.token", &token)?;
    let mut child = rz_platform::update::detached(
        Command::new(&helper)
            .arg("--task")
            .arg(&task_file)
            .arg(server.local_addr()?.to_string())
            .arg(reference)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )?;
    let response =
        accept(&server, &token, PREPARE_TIMEOUT, Some(&helper)).and_then(|(mut stream, pid)| {
            if pid != child.id() {
                return Err(Error::Corrupt("unexpected updater process".into()));
            }
            expected(receive(&mut stream)?, "ready")?;
            send(&mut stream, &Message::Armed)?;
            expected(receive(&mut stream)?, "armed")
        });
    if response.is_err() {
        let _ = child.try_wait();
    }
    response
}

pub fn current_process_started() -> Result<u64> {
    rz_platform::update::process(std::process::id())
        .map(|p| p.started)
        .ok_or_else(|| Error::Corrupt("could not identify RustZIP process".into()))
}

pub fn launch_macos_helper(helper: &Path, arguments: &[OsString]) -> Result<()> {
    let server = listener()?;
    let token = nonce()?;
    let auth =
        crate::update_package::StagingDirectory::create(&std::env::temp_dir(), "RustZIP-auth-")?;
    let reference = token_reference(auth.path(), "controller.token", &token)?;
    let child = rz_platform::update::detached(
        Command::new(helper)
            .args(arguments)
            .arg("--controller")
            .arg(server.local_addr()?.to_string())
            .arg("--token")
            .arg(reference)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )?;
    let (mut stream, pid) = accept(&server, &token, PREPARE_TIMEOUT, Some(helper))?;
    if pid != child.id() {
        return Err(Error::Corrupt("unexpected macOS updater process".into()));
    }
    expected(receive(&mut stream)?, "ready")?;
    send(&mut stream, &Message::Armed)?;
    expected(receive(&mut stream)?, "armed")
}

struct StartupSession {
    stream: TcpStream,
}

/// Called before starting the GUI. Removes the private restart arguments.
pub fn begin_startup(arguments: &mut Vec<OsString>, current_version: &str) -> Result<()> {
    if let Some(index) = arguments.iter().position(|a| a == "--rustzip-update-error") {
        if index + 1 >= arguments.len() {
            return Err(Error::Corrupt("missing update error".into()));
        }
        let detail = arguments
            .remove(index + 1)
            .to_string_lossy()
            .chars()
            .take(1000)
            .collect();
        arguments.remove(index);
        *STARTUP_ERROR
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| Error::Corrupt("startup state unavailable".into()))? = Some(detail);
    }
    let Some(index) = arguments.iter().position(|a| a == "--rustzip-update-start") else {
        return Ok(());
    };
    if arguments.len() != index + 5 {
        return Err(Error::Corrupt("invalid update startup arguments".into()));
    }
    let values: Vec<_> = arguments.drain(index..).collect();
    let text = |i: usize| {
        values[i]
            .to_str()
            .ok_or_else(|| Error::Corrupt("invalid update startup arguments".into()))
    };
    if text(4)? != current_version {
        return Err(Error::Corrupt(
            "restarted program version does not match update".into(),
        ));
    }
    let session = StartupSession {
        stream: connect(text(1)?, text(2)?)?,
    };
    let mut slot = STARTUP
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| Error::Corrupt("startup state unavailable".into()))?;
    *slot = Some(session);
    STARTUP_PENDING.store(true, Ordering::Release);
    Ok(())
}

pub fn take_startup_error() -> Option<String> {
    STARTUP_ERROR
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()?
        .take()
}

/// Starts a recovery helper before opening any archive. The caller exits when
/// this returns true so Windows can restore files that would otherwise be busy.
pub fn recover_before_startup() -> Result<bool> {
    if startup_pending() {
        return Ok(false);
    }
    let executable = std::env::current_exe()?;
    if let Some(app) = rz_platform::update::macos_bundle(&executable) {
        let journal = app
            .parent()
            .ok_or_else(|| Error::UnsafePath("missing app parent".into()))?
            .join(format!(
                ".{}.rustzip-update.json",
                app.file_name().unwrap_or_default().to_string_lossy()
            ));
        if journal.exists() {
            rz_platform::update::detached(
                Command::new(app.join("Contents/Helpers/RustZIPUpdateHelper"))
                    .arg("--recover")
                    .arg("--app")
                    .arg(app)
                    .arg("--pid")
                    .arg(std::process::id().to_string())
                    .arg("--pid-start")
                    .arg(current_process_started()?.to_string())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null()),
            )?;
            return Ok(true);
        }
        return Ok(false);
    }
    let installation = match Installation::locate(&executable) {
        Ok(i) => i,
        Err(_) => return Ok(false),
    };
    if !installation.journal().exists() {
        return Ok(false);
    }
    let transaction = Transaction::read(&installation)?;
    if rz_platform::update::same_process(transaction.updater_pid, transaction.updater_started) {
        return Ok(true);
    }
    let directory = crate::update_package::StagingDirectory::create(
        &std::env::temp_dir(),
        "RustZIP-recovery-",
    )?;
    let helper = directory
        .path()
        .join(if installation.kind == InstallKind::Windows {
            "runner.exe"
        } else {
            "runner"
        });
    fs::copy(installation.helper(), &helper)?;
    let server = listener()?;
    let token = nonce()?;
    let reference = token_reference(directory.path(), "controller.token", &token)?;
    rz_platform::update::detached(
        Command::new(&helper)
            .arg("--startup-recover")
            .arg(&installation.root)
            .arg(std::process::id().to_string())
            .arg(current_process_started()?.to_string())
            .arg(server.local_addr()?.to_string())
            .arg(reference)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )?;
    let (mut stream, _) = accept(&server, &token, PREPARE_TIMEOUT, Some(&helper))?;
    expected(receive(&mut stream)?, "ready")?;
    send(&mut stream, &Message::Armed)?;
    expected(receive(&mut stream)?, "armed")?;
    let _ = directory.keep();
    Ok(true)
}

pub fn startup_pending() -> bool {
    STARTUP_PENDING.load(Ordering::Acquire)
}

/// Called on a background thread only after the main window exists.
pub fn complete_startup() -> Result<()> {
    let session = STARTUP
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| Error::Corrupt("startup state unavailable".into()))?
        .take();
    if let Some(mut session) = session {
        send(&mut session.stream, &Message::Ready)?;
        expected(receive(&mut session.stream)?, "committed")?;
    }
    STARTUP_PENDING.store(false, Ordering::Release);
    Ok(())
}

enum RestartResult {
    Confirmed,
    Failed,
    Deferred,
}

fn restart(
    installation: &Installation,
    version: &str,
    id: &str,
    commit: impl FnOnce() -> Result<()>,
) -> Result<RestartResult> {
    let server = listener()?;
    let token = nonce()?;
    let auth =
        crate::update_package::StagingDirectory::create(&std::env::temp_dir(), "RustZIP-auth-")?;
    let reference = token_reference(auth.path(), "startup.token", &token)?;
    let executable = installation.executable();
    let mut child = Command::new(&executable)
        .arg("--rustzip-update-start")
        .arg(server.local_addr()?.to_string())
        .arg(reference)
        .arg(id)
        .arg(version)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut receipt = None;
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(RestartResult::Failed);
        }
        match accept(
            &server,
            &token,
            Duration::from_millis(250),
            Some(&executable),
        ) {
            Ok(value) => {
                receipt = Some(value);
                break;
            }
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error),
        }
    }
    if let Some((mut stream, pid)) = receipt {
        stream.set_read_timeout(Some(STARTUP_TIMEOUT))?;
        if pid == child.id()
            && receive(&mut stream)
                .and_then(|m| expected(m, "ready"))
                .is_ok()
        {
            commit()?;
            send(&mut stream, &Message::Committed)?;
            return Ok(RestartResult::Confirmed);
        }
    }
    if child.try_wait()?.is_some() {
        return Ok(RestartResult::Failed);
    }
    if let Some(identity) = rz_platform::update::process(child.id()) {
        let _ = rz_platform::update::request_exit(identity.pid, identity.started);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(RestartResult::Failed);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(RestartResult::Deferred)
}

fn task(path: &Path, key: &[u8; 32]) -> Result<(UpdateTask, Installation)> {
    let task = UpdateTask::read(rz_platform::update::open_regular(path)?)?;
    let installation = Installation::read(&task.installation)?;
    installation.verify_registration()?;
    let platform = match installation.kind {
        InstallKind::Windows => "windows-x86_64-zip".to_owned(),
        InstallKind::Linux => crate::update::current_platform(),
    };
    task.validate(env!("CARGO_PKG_VERSION"), &platform, key)?;
    let parent = rz_platform::update::process(task.parent_pid)
        .ok_or_else(|| Error::Unsupported("RustZIP exited before update preparation".into()))?;
    if parent.started != task.parent_start_time
        || !rz_platform::same_file(&parent.executable, &installation.executable())
    {
        return Err(Error::UnsafePath(
            "update parent is not the installed RustZIP process".into(),
        ));
    }
    if rz_platform::update::other_instances(&installation.executable(), task.parent_pid) {
        return Err(Error::Unsupported(
            "Close other RustZIP instances before updating.".into(),
        ));
    }
    Ok((task, installation))
}

fn stage(
    task: &UpdateTask,
    installation: &Installation,
    key: &[u8; 32],
) -> Result<crate::update_package::StagingDirectory> {
    let parent = match installation.kind {
        InstallKind::Linux => installation.root.join("releases"),
        InstallKind::Windows => installation
            .root
            .parent()
            .ok_or_else(|| Error::UnsafePath("missing installation parent".into()))?
            .to_owned(),
    };
    let archive = rz_platform::update::open_regular(&task.archive)?;
    match installation.kind {
        InstallKind::Windows => crate::update_package::stage_windows_update(
            archive,
            &parent,
            &task.version,
            &task.asset,
            key,
            &CancelToken::new(),
            |_, _| {},
        ),
        InstallKind::Linux => {
            let staging = crate::update_package::stage_linux_update(
                archive,
                &parent,
                &task.version,
                &task.asset,
                key,
                &CancelToken::new(),
                |_, _| {},
            )?;
            let current = installation.root.join("current");
            let mut old = std::collections::BTreeSet::new();
            let mut new = std::collections::BTreeSet::new();
            for (directory, files) in [(current.as_path(), &mut old), (staging.path(), &mut new)] {
                for entry in fs::read_dir(directory)? {
                    let entry = entry?;
                    if entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.ends_with(".txt"))
                    {
                        files.insert(entry.file_name());
                    }
                }
            }
            if old != new {
                return Err(Error::Unsupported("This update changes the installation's license or optional component set. Install the official package once to update the desktop integration.".into()));
            }
            Ok(staging)
        }
    }
}

fn wait_parent(task: &UpdateTask) {
    while rz_platform::update::same_process(task.parent_pid, task.parent_start_time) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn recover(installation: &Installation) -> Result<()> {
    if !installation.journal().exists() {
        return Ok(());
    }
    let transaction = Transaction::read(installation)?;
    if rz_platform::update::same_process(transaction.updater_pid, transaction.updater_started) {
        return Err(Error::Unsupported(
            "Another update is still running.".into(),
        ));
    }
    if transaction.phase == Phase::Confirmed {
        transaction.cleanup(installation)?;
    } else {
        transaction.rollback(installation)?;
    }
    let _ = rz_platform::update::clear_recovery(installation.machine);
    Ok(())
}

fn committed(installation: &Installation, version: &str) {
    if installation.kind == InstallKind::Windows {
        let _ = rz_platform::update::record_windows_version(installation.machine, version);
    }
    let _ = rz_platform::update::clear_recovery(installation.machine);
}

fn worker(
    task: &UpdateTask,
    installation: &Installation,
    key: &[u8; 32],
    controller: &mut TcpStream,
    privileged: bool,
) -> Result<()> {
    let _lock = installation.lock()?;
    recover(installation)?;
    let staged = stage(task, installation, key)?;
    let mut transaction = Transaction::prepare(
        installation,
        staged.path(),
        &task.version,
        &task.transaction_id,
        &CancelToken::new(),
    )?;
    rz_platform::update::register_recovery(
        &std::env::current_exe()?,
        &installation.journal(),
        installation.machine,
    )?;
    send(controller, &Message::Ready)?;
    expected(receive(controller)?, "armed")?;
    send(controller, &Message::Armed)?;
    wait_parent(task);
    if rz_platform::update::other_instances(&installation.executable(), task.parent_pid) {
        transaction.rollback(installation)?;
        return Err(Error::Unsupported(
            "Another RustZIP instance is still using the installation.".into(),
        ));
    }
    if let Err(error) = transaction.publish(installation) {
        let _ = transaction.rollback(installation);
        return Err(error);
    }
    #[cfg(feature = "updater-test-hooks")]
    if std::env::var("RZ_UPDATE_FIXTURE_FAULT").ok().as_deref() == Some("published") {
        std::process::exit(86);
    }
    if privileged {
        send(controller, &Message::Published)?;
        controller.set_read_timeout(Some(Duration::from_secs(120)))?;
        match receive(controller)? {
            Message::Commit => {
                transaction.confirm(installation)?;
                committed(installation, &task.version);
                send(controller, &Message::Committed)?;
                transaction.cleanup(installation)?;
            }
            Message::Rollback => {
                transaction.rollback(installation)?;
                let _ = rz_platform::update::clear_recovery(installation.machine);
                send(controller, &Message::Committed)?;
            }
            Message::Deferred => {}
            _ => return Err(Error::Corrupt("invalid privileged update control".into())),
        }
    } else {
        match restart(installation, &task.version, &task.transaction_id, || { transaction.confirm(installation)?; committed(installation, &task.version); Ok(()) })? {
            RestartResult::Confirmed => { transaction.cleanup(installation)?; }
            RestartResult::Failed => { transaction.rollback(installation)?; let _ = rz_platform::update::clear_recovery(installation.machine); return Err(Error::Unsupported("The updated program did not start. The previous version was restored.".into())); }
            RestartResult::Deferred => return Err(Error::Unsupported("Startup was not confirmed. The backup was retained; close RustZIP and launch it again to recover.".into())),
        }
    }
    Ok(())
}

fn bootstrap(task_path: &Path, address: &str, token: &str, key: &[u8; 32]) -> Result<()> {
    if !rz_platform::update::elevated() {
        return Err(Error::Unsupported(
            "system authorization is required".into(),
        ));
    }
    let (mut task, installation) = task(task_path, key)?;
    if !installation.machine
        || !rz_platform::same_file(&std::env::current_exe()?, &installation.helper())
    {
        return Err(Error::UnsafePath(
            "authorization must enter through the installed helper".into(),
        ));
    }
    rz_platform::update::validate_protected_installation(&installation.root, task.parent_pid)?;
    cleanup_protected_workers(&installation, key);
    let mut input = rz_platform::update::open_regular(&task.archive)?;
    crate::update::verify_asset_reader(&mut input, &task.version, &task.asset, key)?;
    input.seek(SeekFrom::Start(0))?;
    let directory = installation
        .root
        .parent()
        .ok_or_else(|| Error::UnsafePath("missing installation parent".into()))?
        .join(format!(".RustZIP-admin-{}", task.transaction_id));
    rz_platform::update::protected_directory(&directory)?;
    let worker = directory.join(if installation.kind == InstallKind::Windows {
        "rz-updater.exe"
    } else {
        "rz-updater"
    });
    let mut helper_source = rz_platform::update::open_regular(&std::env::current_exe()?)?;
    let mut helper_target = rz_platform::update::create_update_file(&worker, true)?;
    std::io::copy(&mut helper_source, &mut helper_target)?;
    helper_target.sync_all()?;
    drop(helper_target);
    rz_platform::apply_mode(&worker, 0o755)?;
    let archive = directory.join("update.asset");
    let mut output = rz_platform::update::create_update_file(&archive, false)?;
    let length = std::io::copy(
        &mut input.take(task.asset.size.saturating_add(1)),
        &mut output,
    )?;
    if length != task.asset.size {
        return Err(Error::Corrupt(
            "update archive changed during authorization".into(),
        ));
    }
    output.sync_all()?;
    task.archive = archive;
    let file = directory.join("task.json");
    let mut descriptor = rz_platform::update::create_update_file(&file, false)?;
    serde_json::to_writer(&mut descriptor, &task)
        .map_err(|_| Error::Corrupt("could not serialize authorized update task".into()))?;
    descriptor.sync_all()?;
    drop(descriptor);
    let identity = rz_platform::update::process(std::process::id())
        .ok_or_else(|| Error::Corrupt("could not identify authorization process".into()))?;
    rz_platform::update::detached(
        Command::new(worker)
            .arg("--worker")
            .arg(file)
            .arg(address)
            .arg(token)
            .arg(identity.pid.to_string())
            .arg(identity.started.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )?;
    Ok(())
}

fn cleanup_protected_workers(installation: &Installation, key: &[u8; 32]) {
    let Some(parent) = installation.root.parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok).take(4096) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(id) = name.strip_prefix(".RustZIP-admin-") else {
            continue;
        };
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !entry
            .file_type()
            .is_ok_and(|t| t.is_dir() && !t.is_symlink())
            || metadata
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_none_or(|age| age <= Duration::from_secs(60))
        {
            continue;
        }
        let path = entry.path();
        let Ok(file) = rz_platform::update::open_regular(&path.join("task.json")) else {
            continue;
        };
        let Ok(task) = UpdateTask::read(file) else {
            continue;
        };
        if task.installation != installation.root
            || crate::update::verify_asset_metadata(&task.version, &task.asset, key).is_err()
        {
            continue;
        }
        let worker = path.join(if installation.kind == InstallKind::Windows {
            "rz-updater.exe"
        } else {
            "rz-updater"
        });
        if rz_platform::update::other_instances(&worker, 0) {
            continue;
        }
        let _ = fs::remove_dir_all(path);
    }
}

fn normal(task_path: &Path, address: &str, token: &str, key: &[u8; 32]) -> Result<()> {
    let mut gui = connect(address, token)?;
    if let Some(directory) = task_path.parent() {
        install::write_json(
            &directory.join("runner.json"),
            &(std::process::id(), current_process_started()?),
        )?;
    }
    let result = (|| {
        let (task, installation) = task(task_path, key)?;
        if installation.machine && !rz_platform::update::elevated() {
            let server = listener()?;
            let secret = nonce()?;
            let reference = token_reference(
                task_path
                    .parent()
                    .ok_or_else(|| Error::UnsafePath("missing task parent".into()))?,
                "worker.token",
                &secret,
            )?;
            let arguments = vec![
                OsString::from("--bootstrap"),
                task_path.as_os_str().to_owned(),
                server.local_addr()?.to_string().into(),
                reference.into(),
            ];
            rz_platform::update::authorize(&installation.helper(), &arguments)?;
            let (mut elevated, _) = accept(&server, &secret, PREPARE_TIMEOUT, None)?;
            expected(receive(&mut elevated)?, "ready")?;
            send(&mut gui, &Message::Ready)?;
            expected(receive(&mut gui)?, "armed")?;
            send(&mut elevated, &Message::Armed)?;
            expected(receive(&mut elevated)?, "armed")?;
            send(&mut gui, &Message::Armed)?;
            elevated.set_read_timeout(None)?;
            expected(receive(&mut elevated)?, "published")?;
            let result = restart(&installation, &task.version, &task.transaction_id, || {
                send(&mut elevated, &Message::Commit)?;
                expected(receive(&mut elevated)?, "committed")
            })?;
            match result {
                RestartResult::Confirmed => Ok(()),
                RestartResult::Failed => {
                    send(&mut elevated, &Message::Rollback)?;
                    expected(receive(&mut elevated)?, "committed")?;
                    Err(Error::Unsupported(
                        "The update could not start. The previous version was restored.".into(),
                    ))
                }
                RestartResult::Deferred => {
                    send(&mut elevated, &Message::Deferred)?;
                    Err(Error::Unsupported("Startup was not confirmed. The previous version was retained for recovery.".into()))
                }
            }
        } else {
            worker(&task, &installation, key, &mut gui, false)
        }
    })();
    if let Err(error) = &result {
        let _ = send(
            &mut gui,
            &Message::Failed {
                detail: error.to_string(),
            },
        );
        if let Ok(task) = UpdateTask::read(File::open(task_path)?) {
            if !rz_platform::update::same_process(task.parent_pid, task.parent_start_time) {
                if let Ok(installation) = Installation::read(&task.installation) {
                    if !rz_platform::update::other_instances(&installation.executable(), 0) {
                        let _ = Command::new(installation.executable())
                            .arg("--rustzip-update-error")
                            .arg(error.to_string())
                            .spawn();
                    }
                }
            }
        }
    } else if std::env::consts::OS != "windows" {
        cleanup_download(task_path);
    }
    result
}

fn cleanup_download(task_path: &Path) {
    if let Some(directory) = task_path.parent() {
        if directory
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("RustZIP-update-"))
        {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

pub fn cleanup_previous_downloads(key: &[u8; 32]) {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok).take(4096) {
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with("RustZIP-update-"))
        {
            continue;
        }
        let directory = entry.path();
        if !entry
            .file_type()
            .is_ok_and(|t| t.is_dir() && !t.is_symlink())
        {
            continue;
        }
        let Ok(file) = File::open(directory.join("task.json")) else {
            continue;
        };
        let Ok(task) = UpdateTask::read(file) else {
            continue;
        };
        if crate::update::verify_asset_metadata(&task.version, &task.asset, key).is_err()
            || task.archive.parent() != Some(directory.as_path())
            || rz_platform::update::same_process(task.parent_pid, task.parent_start_time)
        {
            continue;
        }
        if let Ok(identity) = install::read_json::<(u32, u64)>(&directory.join("runner.json")) {
            if rz_platform::update::same_process(identity.0, identity.1) {
                continue;
            }
        }
        let _ = fs::remove_dir_all(directory);
    }
}

fn startup_recovery(root: &Path, pid: u32, started: u64, address: &str, token: &str) -> Result<()> {
    let mut gui = connect(address, token)?;
    let result = (|| {
        let installation = Installation::read(root)?;
        let rolled_back = Transaction::read(&installation)?.phase != Phase::Confirmed;
        installation.verify_registration()?;
        let parent = rz_platform::update::process(pid)
            .ok_or_else(|| Error::Corrupt("recovery parent is not running".into()))?;
        if parent.started != started
            || !rz_platform::same_file(&parent.executable, &installation.executable())
        {
            return Err(Error::UnsafePath("invalid recovery parent".into()));
        }
        if installation.machine && !rz_platform::update::elevated() {
            let server = listener()?;
            let secret = nonce()?;
            let private = std::env::current_exe()?
                .parent()
                .ok_or_else(|| Error::UnsafePath("missing recovery helper parent".into()))?
                .to_owned();
            let reference = token_reference(&private, "worker.token", &secret)?;
            rz_platform::update::authorize(
                &installation.helper(),
                &[
                    "--recover-bootstrap".into(),
                    root.as_os_str().to_owned(),
                    pid.to_string().into(),
                    started.to_string().into(),
                    server.local_addr()?.to_string().into(),
                    reference.into(),
                ],
            )?;
            let (mut controller, _) = accept(&server, &secret, PREPARE_TIMEOUT, None)?;
            expected(receive(&mut controller)?, "ready")?;
            send(&mut gui, &Message::Ready)?;
            expected(receive(&mut gui)?, "armed")?;
            send(&mut controller, &Message::Armed)?;
            expected(receive(&mut controller)?, "armed")?;
            send(&mut gui, &Message::Armed)?;
            expected(receive(&mut controller)?, "committed")?;
        } else {
            let _lock = installation.lock()?;
            send(&mut gui, &Message::Ready)?;
            expected(receive(&mut gui)?, "armed")?;
            send(&mut gui, &Message::Armed)?;
            while rz_platform::update::same_process(pid, started) {
                std::thread::sleep(Duration::from_millis(100));
            }
            recover(&installation)?;
        }
        let mut command = Command::new(installation.executable());
        if rolled_back {
            command
                .arg("--rustzip-update-error")
                .arg("An interrupted update was rolled back. You can try updating again.");
        }
        command.spawn()?;
        Ok(())
    })();
    if let Err(error) = &result {
        let _ = send(
            &mut gui,
            &Message::Failed {
                detail: error.to_string(),
            },
        );
    }
    result
}

fn recovery_bootstrap(
    root: &Path,
    pid: u32,
    started: u64,
    address: &str,
    token: &str,
) -> Result<()> {
    if !rz_platform::update::elevated() {
        return Err(Error::Unsupported("system authorization required".into()));
    }
    let installation = Installation::read(root)?;
    installation.verify_registration()?;
    if !installation.machine
        || !rz_platform::same_file(&std::env::current_exe()?, &installation.helper())
    {
        return Err(Error::UnsafePath("untrusted recovery entry point".into()));
    }
    rz_platform::update::validate_protected_installation(root, pid)?;
    let secret = resolve_token(token)?;
    let directory = root
        .parent()
        .ok_or_else(|| Error::UnsafePath("missing installation parent".into()))?
        .join(format!(".RustZIP-recover-{}", &secret[..32]));
    rz_platform::update::protected_directory(&directory)?;
    let executable = directory.join(if installation.kind == InstallKind::Windows {
        "rz-updater.exe"
    } else {
        "rz-updater"
    });
    let mut input = rz_platform::update::open_regular(&std::env::current_exe()?)?;
    let mut output = rz_platform::update::create_update_file(&executable, false)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    rz_platform::apply_mode(&executable, 0o755)?;
    let own_started = current_process_started()?;
    rz_platform::update::detached(
        Command::new(executable)
            .args([
                OsString::from("--recover-worker"),
                root.as_os_str().to_owned(),
                pid.to_string().into(),
                started.to_string().into(),
                address.into(),
                token.into(),
                std::process::id().to_string().into(),
                own_started.to_string().into(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )?;
    Ok(())
}

pub fn helper_main(arguments: impl IntoIterator<Item = OsString>, key: &[u8; 32]) -> Result<()> {
    let args: Vec<_> = arguments.into_iter().collect();
    let text = |i: usize| {
        args.get(i)
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::Corrupt("invalid updater arguments".into()))
    };
    match text(0)? {
        "--task" if args.len() == 4 => normal(Path::new(&args[1]), text(2)?, text(3)?, key),
        "--bootstrap" if args.len() == 4 => bootstrap(Path::new(&args[1]), text(2)?, text(3)?, key),
        "--worker" if args.len() == 6 => {
            if !rz_platform::update::elevated() {
                return Err(Error::Unsupported(
                    "authorized worker must be privileged".into(),
                ));
            }
            let bootstrap_pid: u32 = text(4)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid bootstrap identity".into()))?;
            let bootstrap_started: u64 = text(5)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid bootstrap identity".into()))?;
            while rz_platform::update::same_process(bootstrap_pid, bootstrap_started) {
                std::thread::sleep(Duration::from_millis(50));
            }
            let (task, installation) = task(Path::new(&args[1]), key)?;
            let own = std::env::current_exe()?.canonicalize()?;
            let expected_directory = installation
                .root
                .parent()
                .ok_or_else(|| Error::UnsafePath("missing installation parent".into()))?
                .join(format!(".RustZIP-admin-{}", task.transaction_id));
            if own.parent() != Some(expected_directory.as_path()) {
                return Err(Error::UnsafePath(
                    "authorized worker is not in its protected directory".into(),
                ));
            }
            rz_platform::update::validate_protected_installation(
                &installation.root,
                task.parent_pid,
            )?;
            let mut controller = connect(text(2)?, text(3)?)?;
            let result = worker(&task, &installation, key, &mut controller, true);
            if let Err(error) = &result {
                let _ = send(
                    &mut controller,
                    &Message::Failed {
                        detail: error.to_string(),
                    },
                );
            }
            result
        }
        "--install" if args.len() == 4 || args.len() == 5 => install::install_linux(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            args.get(4).is_some_and(|s| s == "--uninstall"),
        ),
        "--startup-recover" if args.len() == 6 => startup_recovery(
            Path::new(&args[1]),
            text(2)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid recovery process".into()))?,
            text(3)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid recovery process".into()))?,
            text(4)?,
            text(5)?,
        ),
        "--recover-bootstrap" if args.len() == 6 => recovery_bootstrap(
            Path::new(&args[1]),
            text(2)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid recovery process".into()))?,
            text(3)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid recovery process".into()))?,
            text(4)?,
            text(5)?,
        ),
        "--recover-worker" if args.len() == 8 => {
            if !rz_platform::update::elevated() {
                return Err(Error::Unsupported("system authorization required".into()));
            }
            let bootstrap_pid = text(6)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid recovery bootstrap".into()))?;
            let bootstrap_started = text(7)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid recovery bootstrap".into()))?;
            while rz_platform::update::same_process(bootstrap_pid, bootstrap_started) {
                std::thread::sleep(Duration::from_millis(50));
            }
            let installation = Installation::read(Path::new(&args[1]))?;
            installation.verify_registration()?;
            let pid = text(2)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid recovery process".into()))?;
            let started = text(3)?
                .parse()
                .map_err(|_| Error::Corrupt("invalid recovery process".into()))?;
            rz_platform::update::validate_protected_installation(&installation.root, pid)?;
            let own = std::env::current_exe()?.canonicalize()?;
            let directory = own
                .parent()
                .ok_or_else(|| Error::UnsafePath("missing recovery directory".into()))?;
            if directory.parent() != installation.root.parent()
                || !directory
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".RustZIP-recover-"))
            {
                return Err(Error::UnsafePath("untrusted recovery worker".into()));
            }
            let mut controller = connect(text(4)?, text(5)?)?;
            let _lock = installation.lock()?;
            send(&mut controller, &Message::Ready)?;
            expected(receive(&mut controller)?, "armed")?;
            send(&mut controller, &Message::Armed)?;
            while rz_platform::update::same_process(pid, started) {
                std::thread::sleep(Duration::from_millis(100));
            }
            recover(&installation)?;
            send(&mut controller, &Message::Committed)
        }
        "--recover" if args.len() == 2 => recover_journal(Path::new(&args[1])),
        _ => Err(Error::Unsupported("invalid updater command".into())),
    }
}

fn recover_journal(journal: &Path) -> Result<()> {
    // Recovery only accepts the journal of a registered installation, never an arbitrary target.
    let installation = if cfg!(windows) {
        let candidate = |root: std::path::PathBuf, machine| Installation {
            schema: 1,
            kind: InstallKind::Windows,
            root,
            machine,
            prefix: None,
            data_home: None,
        };
        let user = rz_platform::update::registered_windows_root(false)
            .ok()
            .map(|root| candidate(root, false))
            .filter(|i| i.journal() == journal);
        let machine = rz_platform::update::registered_windows_root(true)
            .ok()
            .map(|root| candidate(root, true))
            .filter(|i| i.journal() == journal);
        let chosen = user
            .or(machine)
            .ok_or_else(|| Error::UnsafePath("unrecognized Windows recovery journal".into()))?;
        if chosen.machine && !rz_platform::update::elevated() {
            let executable = std::env::current_exe()?.canonicalize()?;
            let parent = executable
                .parent()
                .ok_or_else(|| Error::UnsafePath("missing recovery parent".into()))?;
            if parent.parent() != chosen.root.parent()
                || !parent
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with(".RustZIP-admin-"))
            {
                return Err(Error::UnsafePath("untrusted system recovery helper".into()));
            }
            rz_platform::update::authorize(
                &executable,
                &["--recover".into(), journal.as_os_str().to_owned()],
            )?;
            return Ok(());
        }
        chosen
    } else {
        Installation::read(
            journal
                .parent()
                .ok_or_else(|| Error::UnsafePath("missing journal parent".into()))?,
        )?
    };
    if installation.journal() != journal {
        return Err(Error::UnsafePath("unrecognized recovery journal".into()));
    }
    if !journal.exists() {
        let _ = rz_platform::update::clear_recovery(installation.machine);
        return Ok(());
    }
    let _lock = installation.lock()?;
    if rz_platform::update::other_instances(&installation.executable(), 0) {
        return Err(Error::Unsupported(
            "Close RustZIP before recovering the update.".into(),
        ));
    }
    recover(&installation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_channel_rejects_a_wrong_token_before_accepting_the_helper() {
        let server = listener().unwrap();
        let endpoint = server.local_addr().unwrap().to_string();
        let token = nonce().unwrap();
        let valid = token.clone();
        let executable = std::env::current_exe().unwrap();
        let client = std::thread::spawn(move || {
            let bad = connect(&endpoint, &"0".repeat(64)).unwrap();
            drop(bad);
            let mut good = connect(&endpoint, &valid).unwrap();
            expected(receive(&mut good).unwrap(), "ready").unwrap();
            send(&mut good, &Message::Armed).unwrap();
        });
        let (mut peer, pid) =
            accept(&server, &token, Duration::from_secs(5), Some(&executable)).unwrap();
        assert_eq!(pid, std::process::id());
        send(&mut peer, &Message::Ready).unwrap();
        expected(receive(&mut peer).unwrap(), "armed").unwrap();
        client.join().unwrap();
    }

    #[test]
    fn local_capability_files_are_bounded_and_external_endpoints_are_rejected() {
        let directory =
            crate::update_package::StagingDirectory::create(&std::env::temp_dir(), "RustZIP-test-")
                .unwrap();
        let token = nonce().unwrap();
        let reference = token_reference(directory.path(), "token", &token).unwrap();
        assert_eq!(resolve_token(&reference).unwrap(), token);
        assert!(connect("192.0.2.1:80", &token).is_err());
        fs::write(directory.path().join("token"), vec![b'0'; 65]).unwrap();
        assert!(resolve_token(&reference).is_err());
    }

    #[test]
    fn messages_do_not_allow_unsigned_payload_expansion() {
        let server = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = server.local_addr().unwrap();
        let writer = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(endpoint).unwrap();
            stream.write_all(&65537u32.to_be_bytes()).unwrap();
        });
        let (mut stream, _) = server.accept().unwrap();
        assert!(receive(&mut stream).is_err());
        writer.join().unwrap();
    }
}
