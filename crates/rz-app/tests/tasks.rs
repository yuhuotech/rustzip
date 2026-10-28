#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_app::tasks::{ErrorCode, Event, JobKind, JobSpec, TaskManager, TaskState};
use rz_core::{CreateOptions, Progress};
use rz_engine::ExtractOptions;
use rz_formats::OutputFormat;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(30);

/// True when no RustZIP temporary file for `output_name` sits in `dir`.
fn no_temp_files(dir: &Path, output_name: &str) -> bool {
    fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).all(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        !n.starts_with(&format!(".{output_name}.rz-"))
            && !n.starts_with(&format!("{output_name}.rz-"))
    })
}

#[test]
fn conversion_jobs_pause_retry_passwords_and_repair_retains_a_report() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.zip");
    let manager = TaskManager::new(1);
    let mut spec = compress_spec(vec![tree(dir.path())], source.clone());
    if let JobSpec::Compress { opts, .. } = &mut spec {
        opts.password = Some("secret".into());
    }
    let compressed = manager.submit(spec);
    assert_eq!(
        manager.wait_for(compressed, WAIT).unwrap().state,
        TaskState::Done
    );
    let output = dir.path().join("converted.7z");
    let opts = rz_app::TransformOptions {
        format: OutputFormat::SevenZ,
        create: CreateOptions {
            threads: 1,
            ..Default::default()
        },
        limits: ExtractOptions::default(),
        encoding: None,
    };
    let id = manager.submit_paused(JobSpec::Transform {
        archive: source.clone(),
        output: output.clone(),
        opts: opts.clone(),
        recover: false,
    });
    assert_eq!(manager.get(id).unwrap().state, TaskState::Paused);
    assert!(!output.exists());
    manager.resume(id);
    assert_eq!(
        manager.wait_for(id, WAIT).unwrap().error.unwrap().code,
        ErrorCode::Password
    );
    let retried = manager.retry_with_password(id, "secret").unwrap();
    assert_eq!(
        manager.wait_for(retried, WAIT).unwrap().state,
        TaskState::Done
    );
    let mut repair_opts = opts;
    repair_opts.limits.password = Some("secret".into());
    let repair = manager.submit(JobSpec::Transform {
        archive: source,
        output: dir.path().join("recovered.7z"),
        opts: repair_opts,
        recover: true,
    });
    let done = manager.wait_for(repair, WAIT).unwrap();
    assert_eq!(done.state, TaskState::Done, "{:?}", done.error);
    let report = done.summary.unwrap().recovery.unwrap();
    assert!(report.files > 0);
    assert_eq!(report.skipped, 0);
    assert_eq!(done.kind, JobKind::Repair);
}

fn tree(root: &Path) -> PathBuf {
    let p = root.join("proj");
    fs::create_dir_all(p.join("docs")).unwrap();
    fs::write(p.join("readme.txt"), "hello rustzip").unwrap();
    fs::write(p.join("docs/说明.md"), "# 中文").unwrap();
    fs::write(p.join("big.bin"), vec![7u8; 400_000]).unwrap();
    p
}

fn compress_spec(sources: Vec<PathBuf>, output: PathBuf) -> JobSpec {
    let format = OutputFormat::from_path(&output).unwrap();
    JobSpec::Compress {
        sources,
        output,
        format,
        opts: CreateOptions {
            threads: 2,
            ..CreateOptions::default()
        },
    }
}

fn extract_spec(archive: &Path, dest: &Path, smart: bool) -> JobSpec {
    JobSpec::Extract {
        archive: archive.to_path_buf(),
        dest: dest.to_path_buf(),
        opts: ExtractOptions::default(),
        smart,
        encoding: None,
    }
}

/// Builds an archive directly through the manager and waits for it.
fn make_archive(m: &TaskManager, sources: Vec<PathBuf>, output: &Path) {
    let id = m.submit(compress_spec(sources, output.to_path_buf()));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Done, "{:?}", info.error);
}

fn wait_event(rx: &Receiver<Event>, what: &str, mut pred: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| panic!("timed out waiting for {what}"));
        let ev = rx
            .recv_timeout(left)
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        if pred(&ev) {
            return ev;
        }
    }
}

#[test]
fn a_task_reports_added_started_progress_and_finished_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(2);
    let rx = m.subscribe();
    let out = tmp.path().join("a.zip");
    let id = m.submit(compress_spec(vec![src], out.clone()));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Done);
    assert_eq!(info.kind, JobKind::Compress);
    assert_eq!(info.title, "a.zip");
    let summary = info.summary.unwrap();
    assert_eq!(summary.files, 3);
    assert_eq!(summary.output, out);
    assert_eq!(info.progress.bytes_done, info.progress.bytes_total);

    let events: Vec<Event> = rx.try_iter().collect();
    let pos = |f: &dyn Fn(&Event) -> bool| events.iter().position(f).unwrap();
    let added = pos(&|e| matches!(e, Event::Added(i) if i.id == id));
    let started = pos(&|e| matches!(e, Event::Started(i) if *i == id));
    let finished = pos(&|e| matches!(e, Event::Finished { id: i, .. } if *i == id));
    assert!(added < started && started < finished);

    let bytes: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            Event::Progress { progress, .. } => Some(progress.bytes_done),
            _ => None,
        })
        .collect();
    assert!(!bytes.is_empty());
    assert!(bytes.windows(2).all(|w| w[0] <= w[1]));
}

#[test]
fn extraction_uses_a_folder_only_when_the_archive_has_loose_files() {
    let tmp = tempfile::tempdir().unwrap();
    let m = TaskManager::new(2);

    // Loose files: two top-level items.
    let loose = tmp.path().join("loose");
    fs::create_dir_all(&loose).unwrap();
    fs::write(loose.join("a.txt"), "a").unwrap();
    fs::write(loose.join("b.txt"), "b").unwrap();
    let pack = tmp.path().join("pack.zip");
    make_archive(&m, vec![loose.join("a.txt"), loose.join("b.txt")], &pack);
    let out = tmp.path().join("out");
    let id = m.submit(extract_spec(&pack, &out, true));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Done, "{:?}", info.error);
    assert_eq!(info.summary.unwrap().output, out.join("pack"));
    assert_eq!(fs::read_to_string(out.join("pack/a.txt")).unwrap(), "a");

    // Extracting again must not mix into the first folder.
    let id = m.submit(extract_spec(&pack, &out, true));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.summary.unwrap().output, out.join("pack (2)"));

    // One wrapper folder: extracted in place.
    let src = tree(tmp.path());
    let wrapped = tmp.path().join("wrapped.zip");
    make_archive(&m, vec![src], &wrapped);
    let out2 = tmp.path().join("out2");
    let id = m.submit(extract_spec(&wrapped, &out2, true));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.summary.unwrap().output, out2);
    assert!(out2.join("proj/readme.txt").exists());

    // With smart extraction off, loose files land directly in the destination.
    let out3 = tmp.path().join("out3");
    let id = m.submit(extract_spec(&pack, &out3, false));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.summary.unwrap().output, out3);
    assert!(out3.join("a.txt").exists());
}

#[test]
fn a_task_added_paused_waits_until_resumed() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(1);
    let rx = m.subscribe();
    let out = tmp.path().join("p.zip");
    let id = m.submit_paused(compress_spec(vec![src], out.clone()));
    wait_event(&rx, "start", |e| matches!(e, Event::Started(i) if *i == id));

    std::thread::sleep(Duration::from_millis(200));
    let info = m.get(id).unwrap();
    assert_eq!(info.state, TaskState::Paused);
    assert!(!out.exists(), "a paused task must not produce output");
    assert_eq!(info.progress.bytes_done, 0);

    assert!(m.resume(id));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Done);
    assert!(out.exists());
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Resumed(i) if *i == id)));
}

#[test]
fn a_paused_running_task_can_be_cancelled_and_leaves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(1);
    let rx = m.subscribe();
    let out = tmp.path().join("c.zip");
    let id = m.submit_paused(compress_spec(vec![src], out.clone()));
    wait_event(&rx, "start", |e| matches!(e, Event::Started(i) if *i == id));
    assert!(m.cancel(id));
    wait_event(
        &rx,
        "cancel",
        |e| matches!(e, Event::Cancelled(i) if *i == id),
    );
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Cancelled);
    assert!(!out.exists());
    assert!(no_temp_files(tmp.path(), "c.zip"));
    assert!(!m.cancel(id), "cancelling a finished task reports false");
}

#[test]
fn only_max_concurrent_tasks_run_and_the_rest_wait_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(2);
    let rx = m.subscribe();
    let ids: Vec<_> = (0..4)
        .map(|i| {
            m.submit_paused(compress_spec(
                vec![src.clone()],
                tmp.path().join(format!("{i}.zip")),
            ))
        })
        .collect();
    let started: Vec<_> = (0..2)
        .map(
            |_| match wait_event(&rx, "start", |e| matches!(e, Event::Started(_))) {
                Event::Started(i) => i,
                _ => unreachable!(),
            },
        )
        .collect();
    assert_eq!(started, [ids[0], ids[1]], "first come, first served");
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !rx.try_iter().any(|e| matches!(e, Event::Started(_))),
        "a third task started although two workers are busy"
    );

    // Finishing one frees a slot for the next in line.
    assert!(m.cancel(ids[0]));
    let next = wait_event(&rx, "next start", |e| matches!(e, Event::Started(_)));
    assert!(matches!(next, Event::Started(i) if i == ids[2]));

    for id in &ids[1..] {
        m.resume(*id);
    }
    for id in &ids[1..] {
        assert_eq!(m.wait_for(*id, WAIT).unwrap().state, TaskState::Done);
    }
}

#[test]
fn a_queued_task_can_be_cancelled_and_never_starts() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(1);
    let rx = m.subscribe();
    let blocker = m.submit_paused(compress_spec(vec![src.clone()], tmp.path().join("a.zip")));
    wait_event(
        &rx,
        "start",
        |e| matches!(e, Event::Started(i) if *i == blocker),
    );
    let queued = m.submit(compress_spec(vec![src], tmp.path().join("b.zip")));
    assert_eq!(m.get(queued).unwrap().state, TaskState::Queued);

    assert!(m.cancel(queued));
    assert_eq!(m.get(queued).unwrap().state, TaskState::Cancelled);
    assert!(m.cancel(blocker));
    assert_eq!(
        m.wait_for(blocker, WAIT).unwrap().state,
        TaskState::Cancelled
    );
    std::thread::sleep(Duration::from_millis(100));
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::Started(i) if *i == queued)));
    assert!(!tmp.path().join("b.zip").exists());
}

#[test]
fn a_missing_password_fails_with_a_code_and_a_retry_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(2);
    let archive = tmp.path().join("secret.zip");
    let id = m.submit(JobSpec::Compress {
        sources: vec![src],
        output: archive.clone(),
        format: OutputFormat::Zip,
        opts: CreateOptions {
            password: Some("pw1".into()),
            threads: 2,
            ..CreateOptions::default()
        },
    });
    assert_eq!(m.wait_for(id, WAIT).unwrap().state, TaskState::Done);

    let out = tmp.path().join("out");
    let failed = m.submit(extract_spec(&archive, &out, false));
    let info = m.wait_for(failed, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Failed);
    assert_eq!(
        info.error.as_ref().map(|e| e.code),
        Some(ErrorCode::Password)
    );

    // A wrong password fails the same way, and can be retried again.
    let wrong = m.retry_with_password(failed, "nope").unwrap();
    let info = m.wait_for(wrong, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Failed);
    assert_eq!(info.error.map(|e| e.code), Some(ErrorCode::Password));

    let retry = m.retry_with_password(failed, "pw1").unwrap();
    assert_ne!(retry, failed);
    let info = m.wait_for(retry, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Done, "{:?}", info.error);
    assert_eq!(
        fs::read_to_string(out.join("proj/readme.txt")).unwrap(),
        "hello rustzip"
    );
}

#[test]
fn retry_is_only_offered_for_password_failures() {
    let tmp = tempfile::tempdir().unwrap();
    let m = TaskManager::new(1);
    let junk = tmp.path().join("junk.zip");
    fs::write(&junk, "not an archive").unwrap();
    let id = m.submit(extract_spec(&junk, &tmp.path().join("o"), false));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.error.map(|e| e.code), Some(ErrorCode::UnknownFormat));
    assert!(m.retry_with_password(id, "x").is_none());
    assert!(m.retry_with_password(9999, "x").is_none());

    // A task that is not finished cannot be retried either.
    let src = tree(tmp.path());
    let running = m.submit_paused(compress_spec(vec![src], tmp.path().join("r.zip")));
    assert!(m.retry_with_password(running, "x").is_none());
    m.cancel(running);
}

#[test]
fn a_malicious_archive_fails_with_unsafe_path_and_writes_nothing_outside() {
    let tmp = tempfile::tempdir().unwrap();
    let evil = tmp.path().join("evil.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&evil).unwrap());
    w.start_file("../escaped.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    w.write_all(b"x").unwrap();
    w.finish().unwrap();

    let m = TaskManager::new(1);
    let id = m.submit(extract_spec(&evil, &tmp.path().join("out"), true));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Failed);
    assert_eq!(info.error.map(|e| e.code), Some(ErrorCode::UnsafePath));
    assert!(!tmp.path().join("escaped.txt").exists());
}

#[test]
fn test_jobs_verify_good_archives_and_catch_damage() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(2);
    let good = tmp.path().join("good.7z");
    make_archive(&m, vec![src], &good);
    let spec = |p: &Path| JobSpec::Test {
        archive: p.to_path_buf(),
        password: None,
        encoding: None,
    };
    let id = m.submit(spec(&good));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Done, "{:?}", info.error);
    assert_eq!(info.summary.unwrap().bytes, 13 + 8 + 400_000);

    // A stored zip with one flipped data byte.
    let bad = tmp.path().join("bad.zip");
    let mut w = zip::ZipWriter::new(fs::File::create(&bad).unwrap());
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    w.start_file("a.txt", stored).unwrap();
    w.write_all(&vec![b'A'; 4096]).unwrap();
    w.finish().unwrap();
    let mut bytes = fs::read(&bad).unwrap();
    let at = bytes
        .windows(64)
        .position(|w| w.iter().all(|b| *b == b'A'))
        .unwrap();
    bytes[at + 500] = b'B';
    fs::write(&bad, bytes).unwrap();
    let id = m.submit(spec(&bad));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Failed);
    assert!(matches!(
        info.error.map(|e| e.code),
        Some(ErrorCode::Corrupt | ErrorCode::Io)
    ));
}

#[test]
fn every_subscriber_gets_every_event_and_a_closed_one_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(1);
    let a = m.subscribe();
    let b = m.subscribe();
    drop(m.subscribe()); // a UI window that closed
    let id = m.submit(compress_spec(vec![src], tmp.path().join("s.zip")));
    m.wait_for(id, WAIT).unwrap();
    let kinds = |rx: &Receiver<Event>| -> Vec<&'static str> {
        rx.try_iter()
            .filter(|e| !matches!(e, Event::Progress { .. }))
            .map(|e| match e {
                Event::Added(_) => "added",
                Event::Started(_) => "started",
                Event::Finished { .. } => "finished",
                _ => "other",
            })
            .collect()
    };
    assert_eq!(kinds(&a), ["added", "started", "finished"]);
    assert_eq!(kinds(&b), ["added", "started", "finished"]);
}

#[test]
fn snapshot_lists_tasks_in_order_and_finished_ones_can_be_cleared() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(1);
    let done = m.submit(compress_spec(vec![src.clone()], tmp.path().join("1.zip")));
    m.wait_for(done, WAIT).unwrap();
    let held = m.submit_paused(compress_spec(vec![src], tmp.path().join("2.zip")));
    let snap = m.snapshot();
    assert_eq!(snap.iter().map(|t| t.id).collect::<Vec<_>>(), [done, held]);
    m.clear_finished();
    assert_eq!(
        m.snapshot().iter().map(|t| t.id).collect::<Vec<_>>(),
        [held]
    );
    assert!(m.get(done).is_none());
    assert!(m.wait_for(done, Duration::from_millis(10)).is_none());
}

#[test]
fn pausing_a_finished_or_unknown_task_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(1);
    let id = m.submit(compress_spec(vec![src], tmp.path().join("f.zip")));
    m.wait_for(id, WAIT).unwrap();
    assert!(!m.pause(id));
    assert!(!m.resume(id));
    assert!(!m.pause(777));
}

#[test]
fn dropping_the_manager_stops_blocked_and_queued_tasks_without_hanging() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let (tx, rx) = std::sync::mpsc::channel();
    let dir = tmp.path().to_path_buf();
    std::thread::spawn(move || {
        let m = TaskManager::new(2);
        let events = m.subscribe();
        for i in 0..5 {
            m.submit_paused(compress_spec(
                vec![src.clone()],
                dir.join(format!("{i}.zip")),
            ));
        }
        wait_event(&events, "start", |e| matches!(e, Event::Started(_)));
        drop(m);
        let _ = tx.send(());
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(20)).is_ok(),
        "dropping the manager hung"
    );
    assert!(!tmp.path().join("0.zip").exists());
}

#[test]
fn progress_reaches_the_task_info_for_extraction_too() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    let m = TaskManager::new(1);
    let archive = tmp.path().join("e.tar.zst");
    make_archive(&m, vec![src], &archive);
    let id = m.submit(extract_spec(&archive, &tmp.path().join("out"), true));
    let info = m.wait_for(id, WAIT).unwrap();
    assert_eq!(info.state, TaskState::Done);
    let p: Progress = info.progress;
    assert_eq!(p.bytes_done, p.bytes_total);
    assert_eq!(p.bytes_total, 13 + 8 + 400_000);
    assert_eq!(info.eta_secs, Some(0));
}

#[test]
fn a_cancel_and_pause_storm_ends_with_one_terminal_state_per_task() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    fs::write(src.join("heavy.bin"), vec![7u8; 4 << 20]).unwrap();
    let m = TaskManager::new(4);
    let rx = m.subscribe();
    let dir = tmp.path().to_path_buf();
    let ids: Vec<_> = (0..10)
        .map(|i| {
            let out = dir.join(format!("s{i}.zip"));
            m.submit(compress_spec(vec![src.clone()], out))
        })
        .collect();

    // Two threads hammer every task with the same click sequence a user can
    // produce: pause, resume, cancel, plus repeats on already-final states.
    let manager = &m;
    std::thread::scope(|s| {
        for round in 0..2 {
            let ids = ids.clone();
            s.spawn(move || {
                for id in &ids {
                    let _ = manager.pause(*id);
                    let _ = manager.resume(*id);
                    if round == 0 {
                        let _ = manager.cancel(*id);
                    }
                    let _ = manager.cancel(*id);
                    let _ = manager.pause(*id);
                    let _ = manager.resume(*id);
                }
            });
        }
    });

    for id in &ids {
        let info = m
            .wait_for(*id, WAIT)
            .unwrap_or_else(|| panic!("{id} never ended"));
        assert!(info.state.is_finished(), "{id} stuck in {:?}", info.state);
    }

    // `wait_for` observes the final task state just before its worker emits the
    // matching event, so drain until every terminal event has reached the channel.
    let mut events: Vec<Event> = rx.try_iter().collect();
    while events
        .iter()
        .filter(|event| {
            matches!(
                event,
                Event::Finished { .. } | Event::Failed { .. } | Event::Cancelled(_)
            )
        })
        .count()
        < ids.len()
    {
        events.push(
            rx.recv_timeout(WAIT)
                .unwrap_or_else(|error| panic!("terminal events did not arrive: {error}")),
        );
    }

    // Every task produced exactly one terminal event, after its Added event.
    for id in &ids {
        let mut added_at = None;
        let mut terminals = 0;
        let mut first_terminal = None;
        for (i, e) in events.iter().enumerate() {
            let terminal = matches!(
                e,
                Event::Finished { id: t, .. } | Event::Failed { id: t, .. } | Event::Cancelled(t) if *t == *id
            );
            if matches!(e, Event::Added(i2) if i2.id == *id) {
                added_at = Some(i);
            }
            if terminal {
                terminals += 1;
                first_terminal = first_token(first_terminal, i);
            }
        }
        assert_eq!(terminals, 1, "{id} produced {terminals} terminal events");
        assert!(
            added_at.is_some_and(|a| a < first_terminal.unwrap_or(usize::MAX)),
            "{id} finished before it was added"
        );
    }

    // Illegal or repeated operations on finished tasks are refused.
    for id in &ids {
        assert!(!m.cancel(*id));
        assert!(!m.pause(*id));
        assert!(!m.resume(*id));
    }
    assert!(m.get(999_999).is_none());
}

fn first_token(current: Option<usize>, candidate: usize) -> Option<usize> {
    Some(current.map_or(candidate, |c| c.min(candidate)))
}

#[test]
fn dropping_the_manager_mid_cancel_storm_never_hangs() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tree(tmp.path());
    fs::write(src.join("heavy.bin"), vec![7u8; 4 << 20]).unwrap();
    let dir = tmp.path().to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let m = TaskManager::new(3);
        let mut ids = Vec::new();
        for i in 0..3 {
            ids.push(m.submit(compress_spec(
                vec![src.clone()],
                dir.join(format!("r{i}.zip")),
            )));
        }
        for i in 0..2 {
            m.submit_paused(compress_spec(
                vec![src.clone()],
                dir.join(format!("q{i}.zip")),
            ));
        }
        // Cancel everything while the workers are mid-flight, then close the window.
        for id in &ids {
            let _ = m.cancel(*id);
        }
        drop(m);
        let _ = tx.send(());
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(20)).is_ok(),
        "dropping the manager mid-cancel hung"
    );
    for i in 0..3 {
        let out = tmp.path().join(format!("r{i}.zip"));
        assert!(
            !out.exists() || no_temp_files(tmp.path(), &format!("r{i}.zip")),
            "leftovers for r{i}.zip"
        );
    }
}
