//! The task queue: runs compress, extract and test jobs on a small worker pool
//! and reports what happens as events a UI can render.

use crate::modify::{edit_archive, Edit};
use crate::smart::extraction_dir;
use rz_core::{CancelToken, CreateOptions, EntryKind, Error, Progress, ProgressSink};
use rz_engine::{compress, extract, ExtractOptions};
use rz_formats::OutputFormat;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub type TaskId = u64;

#[derive(Debug, Clone)]
pub enum JobSpec {
    Transform {
        archive: PathBuf,
        output: PathBuf,
        opts: rz_engine::TransformOptions,
        recover: bool,
    },
    Compress {
        sources: Vec<PathBuf>,
        output: PathBuf,
        format: OutputFormat,
        opts: CreateOptions,
    },
    Extract {
        archive: PathBuf,
        dest: PathBuf,
        opts: ExtractOptions,
        /// Use a folder named after the archive when it has several top-level items.
        smart: bool,
        encoding: Option<String>,
    },
    Test {
        archive: PathBuf,
        password: Option<String>,
        encoding: Option<String>,
    },
    /// Add files to, or delete entries from, an existing archive.
    Edit {
        archive: PathBuf,
        edit: Edit,
        /// How the archive is unpacked first; carries the password.
        opts: ExtractOptions,
        level: u32,
        encoding: Option<String>,
    },
    Hash {
        file: PathBuf,
        algorithms: Vec<crate::hash::HashAlgorithm>,
    },
    ScanDuplicates {
        sources: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Convert,
    Repair,
    Compress,
    Extract,
    Test,
    Edit,
    Hash,
    DuplicateScan,
}

impl JobSpec {
    pub fn kind(&self) -> JobKind {
        match self {
            JobSpec::Transform { recover, .. } => {
                if *recover {
                    JobKind::Repair
                } else {
                    JobKind::Convert
                }
            }
            JobSpec::Compress { .. } => JobKind::Compress,
            JobSpec::Extract { .. } => JobKind::Extract,
            JobSpec::Test { .. } => JobKind::Test,
            JobSpec::Edit { .. } => JobKind::Edit,
            JobSpec::Hash { .. } => JobKind::Hash,
            JobSpec::ScanDuplicates { .. } => JobKind::DuplicateScan,
        }
    }

    /// A short name for the task list: the archive's file name.
    pub fn title(&self) -> String {
        let path: &std::path::Path = match self {
            JobSpec::Transform { archive, .. } => archive,
            JobSpec::Compress { output, .. } => output,
            JobSpec::Extract { archive, .. }
            | JobSpec::Test { archive, .. }
            | JobSpec::Edit { archive, .. } => archive,
            JobSpec::Hash { file, .. } => file,
            JobSpec::ScanDuplicates { sources } => sources
                .first()
                .map_or(std::path::Path::new("files"), PathBuf::as_path),
        };
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    }

    fn with_password(&self, password: String) -> Option<JobSpec> {
        match self {
            JobSpec::Transform { .. } => {
                let mut spec = self.clone();
                if let JobSpec::Transform { opts, .. } = &mut spec {
                    opts.limits.password = Some(password);
                }
                Some(spec)
            }
            JobSpec::Extract { opts, .. } => {
                let mut spec = self.clone();
                if let JobSpec::Extract { opts: o, .. } = &mut spec {
                    *o = ExtractOptions {
                        password: Some(password),
                        ..opts.clone()
                    };
                }
                Some(spec)
            }
            JobSpec::Test { .. } => {
                let mut spec = self.clone();
                if let JobSpec::Test { password: p, .. } = &mut spec {
                    *p = Some(password);
                }
                Some(spec)
            }
            JobSpec::Edit { opts, .. } => {
                let mut spec = self.clone();
                if let JobSpec::Edit { opts: o, .. } = &mut spec {
                    *o = ExtractOptions {
                        password: Some(password),
                        ..opts.clone()
                    };
                }
                Some(spec)
            }
            JobSpec::Compress { .. } => None,
            JobSpec::Hash { .. } => None,
            JobSpec::ScanDuplicates { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// A stable code for the UI to branch on (for example, ask for a password).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NameConflict,
    AlreadyExists,
    Password,
    UnsafePath,
    LimitExceeded,
    Unsupported,
    UnknownFormat,
    Corrupt,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskError {
    pub code: ErrorCode,
    pub message: String,
    /// Archive-relative names only, for a localized conflict explanation.
    pub conflicting_paths: Option<(String, String)>,
}

impl From<&Error> for TaskError {
    fn from(e: &Error) -> Self {
        let code = match e {
            Error::NameConflict { .. } => ErrorCode::NameConflict,
            Error::Password => ErrorCode::Password,
            Error::UnsafePath(_) => ErrorCode::UnsafePath,
            Error::LimitExceeded(_) => ErrorCode::LimitExceeded,
            Error::Unsupported(_) => ErrorCode::Unsupported,
            Error::UnknownFormat => ErrorCode::UnknownFormat,
            Error::Corrupt(_) => ErrorCode::Corrupt,
            Error::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists => {
                ErrorCode::AlreadyExists
            }
            Error::Io(_) | Error::Cancelled => ErrorCode::Io,
        };
        Self {
            code,
            message: e.to_string(),
            conflicting_paths: match e {
                Error::NameConflict { first, second } => Some((first.clone(), second.clone())),
                _ => None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Summary {
    /// Recovery details remain inspectable after the completion toast disappears.
    pub recovery: Option<rz_engine::TransformReport>,
    pub hashes: Vec<crate::hash::HashResult>,
    /// Duplicate groups found by the optional pre-compression scan.
    pub duplicates: Vec<crate::duplicates::DuplicateGroup>,
    pub files: u64,
    pub bytes: u64,
    /// The archive that was created, or the folder files were extracted to.
    pub output: PathBuf,
    /// What a file manager should highlight when the job is done: the item the
    /// user will look for, inside the folder that gets opened.
    pub reveal: PathBuf,
    pub elapsed: Duration,
}

/// Picks the extracted item to highlight. A wrapper folder made by smart
/// extraction is itself the result; otherwise the first extracted item.
fn reveal_target(
    entries: &[rz_core::Entry],
    only: Option<&[String]>,
    dest: &std::path::Path,
    target: &std::path::Path,
) -> PathBuf {
    if target != dest {
        return target.to_path_buf();
    }
    let top = |p: &str| {
        p.replace('\\', "/")
            .split('/')
            .find(|c| !c.is_empty() && *c != ".")
            .map(str::to_string)
    };
    let mut names: Vec<String> = match only {
        Some(paths) => paths.iter().filter_map(|p| top(p)).collect(),
        None => entries.iter().filter_map(|e| top(&e.path)).collect(),
    };
    names.dedup();
    names
        .into_iter()
        .map(|n| target.join(n))
        .find(|p| p.symlink_metadata().is_ok())
        .unwrap_or_else(|| target.to_path_buf())
}

#[derive(Debug, Clone)]
pub struct TaskInfo {
    pub id: TaskId,
    pub kind: JobKind,
    pub title: String,
    pub state: TaskState,
    pub progress: Progress,
    pub speed_bps: u64,
    pub eta_secs: Option<u64>,
    pub error: Option<TaskError>,
    pub summary: Option<Summary>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Added(TaskInfo),
    Started(TaskId),
    Progress {
        id: TaskId,
        progress: Progress,
        speed_bps: u64,
        eta_secs: Option<u64>,
    },
    Paused(TaskId),
    Resumed(TaskId),
    Finished {
        id: TaskId,
        summary: Summary,
    },
    Failed {
        id: TaskId,
        error: TaskError,
    },
    Cancelled(TaskId),
}

/// Smooths bytes-per-second over time so the displayed speed and ETA do not jump.
#[derive(Debug, Default)]
pub struct SpeedMeter {
    last: Option<(Instant, u64)>,
    ewma: f64,
}

const SAMPLE_EVERY: Duration = Duration::from_millis(250);

impl SpeedMeter {
    /// Feeds the byte counter; returns the smoothed speed once a sample is due.
    pub fn sample(&mut self, now: Instant, bytes: u64) -> Option<u64> {
        let Some((t0, b0)) = self.last else {
            self.last = Some((now, bytes));
            return None;
        };
        let dt = now.saturating_duration_since(t0);
        if dt < SAMPLE_EVERY {
            return None;
        }
        let instant = bytes.saturating_sub(b0) as f64 / dt.as_secs_f64();
        self.ewma = if self.ewma == 0.0 {
            instant
        } else {
            0.7 * self.ewma + 0.3 * instant
        };
        self.last = Some((now, bytes));
        Some(self.ewma as u64)
    }

    /// Forget the previous sample, for example after a pause, so idle time is not counted.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

pub fn eta_secs(progress: &Progress, speed_bps: u64) -> Option<u64> {
    if speed_bps == 0 || progress.bytes_total < progress.bytes_done {
        return None;
    }
    Some((progress.bytes_total - progress.bytes_done).div_ceil(speed_bps))
}

struct Entry {
    info: TaskInfo,
    spec: JobSpec,
    token: CancelToken,
}

struct Inner {
    next_id: TaskId,
    queue: VecDeque<TaskId>,
    tasks: HashMap<TaskId, Entry>,
    shutdown: bool,
}

struct Shared {
    inner: Mutex<Inner>,
    /// Workers wait here for queued work.
    work: Condvar,
    /// Callers of `wait_for` wait here for state changes.
    changed: Condvar,
    subscribers: Mutex<Vec<mpsc::Sender<Event>>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panicking worker must not take the whole queue down with it.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn emit(&self, event: Event) {
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.retain(|s| s.send(event.clone()).is_ok());
        }
        self.changed.notify_all();
    }
}

pub struct TaskManager {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl TaskManager {
    /// `max_concurrent` tasks run at once (1 to 16); the rest wait in order.
    pub fn new(max_concurrent: usize) -> Self {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                next_id: 1,
                queue: VecDeque::new(),
                tasks: HashMap::new(),
                shutdown: false,
            }),
            work: Condvar::new(),
            changed: Condvar::new(),
            subscribers: Mutex::new(Vec::new()),
        });
        let workers = (0..max_concurrent.clamp(1, 16))
            .map(|_| {
                let shared = Arc::clone(&shared);
                std::thread::spawn(move || worker(&shared))
            })
            .collect();
        Self { shared, workers }
    }

    pub fn subscribe(&self) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel();
        if let Ok(mut subs) = self.shared.subscribers.lock() {
            subs.push(tx);
        }
        rx
    }

    pub fn submit(&self, spec: JobSpec) -> TaskId {
        self.add(spec, false)
    }

    /// Queues a task that waits until [`resume`](Self::resume) is called.
    pub fn submit_paused(&self, spec: JobSpec) -> TaskId {
        self.add(spec, true)
    }

    fn add(&self, spec: JobSpec, paused: bool) -> TaskId {
        let token = CancelToken::new();
        if paused {
            token.pause();
        }
        let info;
        let id;
        {
            let mut inner = self.shared.lock();
            id = inner.next_id;
            inner.next_id += 1;
            info = TaskInfo {
                id,
                kind: spec.kind(),
                title: spec.title(),
                state: if paused {
                    TaskState::Paused
                } else {
                    TaskState::Queued
                },
                progress: Progress::default(),
                speed_bps: 0,
                eta_secs: None,
                error: None,
                summary: None,
            };
            inner.tasks.insert(
                id,
                Entry {
                    info: info.clone(),
                    spec,
                    token,
                },
            );
            // Publish creation before a worker can pop this task from the queue.
            self.shared.emit(Event::Added(info));
            inner.queue.push_back(id);
        }
        self.shared.work.notify_one();
        id
    }

    /// Cancels a queued or running task. Returns false if it is unknown or already finished.
    pub fn cancel(&self, id: TaskId) -> bool {
        let queued_cancel;
        {
            let mut inner = self.shared.lock();
            let Some(entry) = inner.tasks.get_mut(&id) else {
                return false;
            };
            if entry.info.state.is_finished() {
                return false;
            }
            entry.token.cancel();
            let was_queued = inner.queue.contains(&id);
            if was_queued {
                inner.queue.retain(|q| *q != id);
                if let Some(entry) = inner.tasks.get_mut(&id) {
                    entry.info.state = TaskState::Cancelled;
                }
            }
            queued_cancel = was_queued;
        }
        // A running task reports Cancelled itself when it notices the token.
        if queued_cancel {
            self.shared.emit(Event::Cancelled(id));
        }
        true
    }

    pub fn pause(&self, id: TaskId) -> bool {
        self.set_paused(id, true)
    }

    pub fn resume(&self, id: TaskId) -> bool {
        self.set_paused(id, false)
    }

    fn set_paused(&self, id: TaskId, pause: bool) -> bool {
        let changed;
        {
            let mut inner = self.shared.lock();
            let waiting = inner.queue.contains(&id);
            let Some(entry) = inner.tasks.get_mut(&id) else {
                return false;
            };
            if entry.info.state.is_finished() {
                return false;
            }
            if pause {
                entry.token.pause();
            } else {
                entry.token.resume();
            }
            let was = entry.info.state;
            entry.info.state = match (pause, waiting) {
                (true, _) => TaskState::Paused,
                // Not started yet: it goes back to waiting its turn.
                (false, true) => TaskState::Queued,
                (false, false) => TaskState::Running,
            };
            changed = entry.info.state != was;
        }
        if changed {
            self.shared.emit(if pause {
                Event::Paused(id)
            } else {
                Event::Resumed(id)
            });
        }
        true
    }

    /// Starts a new task from a failed one that needed a password.
    pub fn retry_with_password(&self, id: TaskId, password: &str) -> Option<TaskId> {
        let spec = {
            let inner = self.shared.lock();
            let e = inner.tasks.get(&id)?;
            let needs = e
                .info
                .error
                .as_ref()
                .is_some_and(|er| er.code == ErrorCode::Password);
            if e.info.state != TaskState::Failed || !needs {
                return None;
            }
            e.spec.with_password(password.to_string())?
        };
        Some(self.submit(spec))
    }

    pub fn get(&self, id: TaskId) -> Option<TaskInfo> {
        self.shared.lock().tasks.get(&id).map(|e| e.info.clone())
    }

    /// All tasks in the order they were added.
    pub fn snapshot(&self) -> Vec<TaskInfo> {
        let inner = self.shared.lock();
        let mut all: Vec<TaskInfo> = inner.tasks.values().map(|e| e.info.clone()).collect();
        all.sort_by_key(|i| i.id);
        all
    }

    /// Drops finished tasks from the list.
    pub fn clear_finished(&self) {
        self.shared
            .lock()
            .tasks
            .retain(|_, e| !e.info.state.is_finished());
    }

    /// Blocks until the task has finished (done, failed or cancelled) or the timeout passes.
    pub fn wait_for(&self, id: TaskId, timeout: Duration) -> Option<TaskInfo> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.shared.lock();
        loop {
            let info = inner.tasks.get(&id)?.info.clone();
            if info.state.is_finished() {
                return Some(info);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            inner = match self.shared.changed.wait_timeout(inner, left) {
                Ok((g, _)) => g,
                Err(p) => p.into_inner().0,
            };
        }
    }
}

impl Drop for TaskManager {
    fn drop(&mut self) {
        {
            let mut inner = self.shared.lock();
            inner.shutdown = true;
            // Every task must notice: running ones through their token, queued ones never start.
            for e in inner.tasks.values() {
                e.token.cancel();
            }
            inner.queue.clear();
        }
        self.shared.work.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

fn worker(shared: &Arc<Shared>) {
    loop {
        let (id, spec, token) = {
            let mut inner = shared.lock();
            let id = loop {
                if let Some(id) = inner.queue.pop_front() {
                    break id;
                }
                if inner.shutdown {
                    return;
                }
                inner = shared.work.wait(inner).unwrap_or_else(|p| p.into_inner());
            };
            let Some(entry) = inner.tasks.get_mut(&id) else {
                continue;
            };
            if !entry.token.is_paused() {
                entry.info.state = TaskState::Running;
            }
            (id, entry.spec.clone(), entry.token.clone())
        };
        shared.emit(Event::Started(id));

        let started = Instant::now();
        let result = run(shared, id, &spec, &token, started);

        let event = {
            let mut inner = shared.lock();
            let Some(entry) = inner.tasks.get_mut(&id) else {
                continue;
            };
            match result {
                Ok(summary) => {
                    entry.info.state = TaskState::Done;
                    entry.info.summary = Some(summary.clone());
                    entry.info.eta_secs = Some(0);
                    Event::Finished { id, summary }
                }
                Err(Error::Cancelled) => {
                    entry.info.state = TaskState::Cancelled;
                    Event::Cancelled(id)
                }
                Err(e) => {
                    let error = TaskError::from(&e);
                    entry.info.state = TaskState::Failed;
                    entry.info.error = Some(error.clone());
                    Event::Failed { id, error }
                }
            }
        };
        shared.emit(event);
    }
}

/// Builds the progress callback: stores the latest state and tells subscribers.
fn progress_sink<'a>(
    shared: &'a Arc<Shared>,
    id: TaskId,
    token: &'a CancelToken,
) -> impl ProgressSink + 'a {
    let mut meter = SpeedMeter::default();
    let mut speed = 0u64;
    move |p: Progress| {
        if token.is_paused() {
            meter.reset();
        } else if let Some(s) = meter.sample(Instant::now(), p.bytes_done) {
            speed = s;
        }
        let eta = eta_secs(&p, speed);
        {
            let mut inner = shared.lock();
            if let Some(e) = inner.tasks.get_mut(&id) {
                e.info.progress = p;
                e.info.speed_bps = speed;
                e.info.eta_secs = eta;
            }
        }
        shared.emit(Event::Progress {
            id,
            progress: p,
            speed_bps: speed,
            eta_secs: eta,
        });
    }
}

fn run(
    shared: &Arc<Shared>,
    id: TaskId,
    spec: &JobSpec,
    token: &CancelToken,
    started: Instant,
) -> Result<Summary, Error> {
    let mut sink = progress_sink(shared, id, token);
    match spec {
        JobSpec::Transform {
            archive,
            output,
            opts,
            recover,
        } => {
            let rep = rz_engine::transform(archive, output, opts, *recover, token, &mut sink)?;
            Ok(Summary {
                files: rep.files,
                bytes: rep.bytes,
                output: output.clone(),
                reveal: output.clone(),
                elapsed: started.elapsed(),
                recovery: (*recover).then_some(rep),
                hashes: Vec::new(),
                duplicates: Vec::new(),
            })
        }
        JobSpec::Compress {
            sources,
            output,
            format,
            opts,
        } => {
            let rep = compress(sources, output, *format, opts, token, &mut sink)?;
            let reveal =
                if opts.volume_size.is_some() && *format == rz_formats::OutputFormat::SevenZ {
                    let mut first = output
                        .file_name()
                        .ok_or_else(|| Error::UnsafePath(output.display().to_string()))?
                        .to_os_string();
                    first.push(".001");
                    output.with_file_name(first)
                } else {
                    output.clone()
                };
            Ok(Summary {
                files: rep.files,
                bytes: rep.bytes,
                output: output.clone(),
                reveal,
                elapsed: started.elapsed(),
                recovery: None,
                hashes: Vec::new(),
                duplicates: Vec::new(),
            })
        }
        JobSpec::Hash { file, algorithms } => {
            let bytes = std::fs::metadata(file)?.len();
            let hashes = crate::hash::hash_file(file, algorithms, token, &mut sink)?;
            Ok(Summary {
                files: 1,
                bytes,
                output: file.clone(),
                reveal: file.clone(),
                elapsed: started.elapsed(),
                recovery: None,
                hashes,
                duplicates: Vec::new(),
            })
        }
        JobSpec::ScanDuplicates { sources } => {
            let duplicates = crate::duplicates::find_duplicates(sources, token, &mut sink)?;
            let files = duplicates
                .iter()
                .map(|group| group.paths.len() as u64)
                .sum();
            let bytes = duplicates.iter().fold(0u64, |total, group| {
                total.saturating_add(group.size.saturating_mul(group.paths.len() as u64))
            });
            Ok(Summary {
                files,
                bytes,
                output: PathBuf::new(),
                reveal: PathBuf::new(),
                elapsed: started.elapsed(),
                recovery: None,
                hashes: Vec::new(),
                duplicates,
            })
        }
        JobSpec::Extract {
            archive,
            dest,
            opts,
            smart,
            encoding,
        } => {
            let mut reader = rz_formats::open(archive)?;
            if let Some(enc) = encoding {
                reader.set_encoding(enc)?;
            }
            if let Some(pw) = &opts.password {
                reader.set_password(pw)?;
            }
            let target = if *smart {
                extraction_dir(&reader.entries()?, archive, dest)
            } else {
                dest.clone()
            };
            let entries = reader.entries()?;
            let rep = extract(&mut *reader, &target, opts, token, &mut sink)?;
            // The engine knows the names it settled on, numbered ones included.
            let reveal = rep
                .roots
                .iter()
                .find(|p| p.symlink_metadata().is_ok())
                .filter(|_| target == *dest)
                .cloned()
                .unwrap_or_else(|| reveal_target(&entries, opts.only.as_deref(), dest, &target));
            Ok(Summary {
                files: rep.files,
                bytes: rep.bytes,
                output: target,
                reveal,
                elapsed: started.elapsed(),
                recovery: None,
                hashes: Vec::new(),
                duplicates: Vec::new(),
            })
        }
        JobSpec::Edit {
            archive,
            edit,
            opts,
            level,
            encoding,
        } => {
            let rep = edit_archive(
                archive,
                edit,
                opts,
                *level,
                encoding.as_deref(),
                token,
                &mut sink,
            )?;
            Ok(Summary {
                files: rep.files,
                bytes: rep.bytes,
                output: archive.clone(),
                reveal: archive.clone(),
                elapsed: started.elapsed(),
                recovery: None,
                hashes: Vec::new(),
                duplicates: Vec::new(),
            })
        }
        JobSpec::Test {
            archive,
            password,
            encoding,
        } => {
            let mut reader = rz_formats::open(archive)?;
            if let Some(enc) = encoding {
                reader.set_encoding(enc)?;
            }
            if let Some(pw) = password {
                reader.set_password(pw)?;
            }
            let total: u64 = reader.entries()?.iter().map(|e| e.size).sum();
            let (mut files, mut bytes) = (0u64, 0u64);
            let mut buf = vec![0u8; 256 * 1024];
            reader.for_each(&mut |e, data| {
                token.check()?;
                if e.kind == EntryKind::File {
                    loop {
                        token.check()?;
                        let n = data
                            .read(&mut buf)
                            .map_err(|err| Error::Corrupt(format!("{}: {err}", e.path)))?;
                        if n == 0 {
                            break;
                        }
                        bytes += n as u64;
                        sink.update(Progress {
                            files_done: files,
                            files_total: 0,
                            bytes_done: bytes,
                            bytes_total: total,
                        });
                    }
                }
                files += 1;
                Ok(())
            })?;
            Ok(Summary {
                files,
                bytes,
                output: archive.clone(),
                reveal: archive.clone(),
                elapsed: started.elapsed(),
                recovery: None,
                hashes: Vec::new(),
                duplicates: Vec::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_is_smoothed_and_ignores_samples_that_are_too_close() {
        let t0 = Instant::now();
        let mut m = SpeedMeter::default();
        assert_eq!(m.sample(t0, 0), None);
        assert_eq!(m.sample(t0 + Duration::from_millis(100), 1000), None);
        // 1 MB in 1 s.
        assert_eq!(
            m.sample(t0 + Duration::from_secs(1), 1_000_000),
            Some(1_000_000)
        );
        // Speed doubles: the average moves only part of the way.
        let s = m
            .sample(t0 + Duration::from_secs(2), 3_000_000)
            .unwrap_or(0);
        assert!(s > 1_000_000 && s < 2_000_000, "{s}");
    }

    #[test]
    fn reset_drops_the_time_spent_paused() {
        let t0 = Instant::now();
        let mut m = SpeedMeter::default();
        m.sample(t0, 0);
        m.sample(t0 + Duration::from_secs(1), 1_000_000);
        m.reset();
        // The first sample after a reset only starts a new window.
        assert_eq!(m.sample(t0 + Duration::from_secs(600), 1_000_001), None);
        let s = m.sample(t0 + Duration::from_secs(601), 2_000_001);
        assert!(s.is_some_and(|v| v > 500_000), "{s:?}");
    }

    #[test]
    fn eta_rounds_up_and_handles_unknowns() {
        let p = Progress {
            bytes_done: 250,
            bytes_total: 1000,
            ..Progress::default()
        };
        assert_eq!(eta_secs(&p, 100), Some(8));
        assert_eq!(eta_secs(&p, 0), None);
        let done = Progress {
            bytes_done: 1000,
            bytes_total: 1000,
            ..Progress::default()
        };
        assert_eq!(eta_secs(&done, 100), Some(0));
        let odd = Progress {
            bytes_done: 2000,
            bytes_total: 1000,
            ..Progress::default()
        };
        assert_eq!(eta_secs(&odd, 100), None);
    }

    #[test]
    fn errors_map_to_stable_codes() {
        let code = |e: Error| TaskError::from(&e).code;
        assert_eq!(code(Error::Password), ErrorCode::Password);
        assert_eq!(code(Error::UnsafePath("x".into())), ErrorCode::UnsafePath);
        assert_eq!(code(Error::Corrupt("x".into())), ErrorCode::Corrupt);
        assert_eq!(code(Error::UnknownFormat), ErrorCode::UnknownFormat);
        assert_eq!(
            code(Error::LimitExceeded("x".into())),
            ErrorCode::LimitExceeded
        );
    }
}
