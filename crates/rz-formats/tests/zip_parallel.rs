#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_core::{CreateOptions, Error, FileSource, NewEntry, Result};
use rz_formats::{create, OutputFormat};
use std::io::{Cursor, Read};

fn entry(name: &str, size: u64) -> NewEntry {
    NewEntry {
        name: name.into(),
        size,
        unix_mode: None,
        modified: None,
    }
}

/// Yields `n` small files; the one at `fail_at` returns a read error.
struct Source {
    i: usize,
    n: usize,
    fail_at: Option<usize>,
}

struct Broken;
impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("disk went away"))
    }
}

impl Iterator for Source {
    type Item = Result<FileSource<'static>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.i >= self.n {
            return None;
        }
        let i = self.i;
        self.i += 1;
        let reader: Box<dyn Read + Send> = if Some(i) == self.fail_at {
            Box::new(Broken)
        } else {
            Box::new(Cursor::new(vec![b'x'; 1000]))
        };
        Some(Ok((entry(&format!("f{i}.txt"), 1000), reader)))
    }
}

fn opts(threads: usize) -> CreateOptions {
    CreateOptions {
        threads,
        ..CreateOptions::default()
    }
}

#[test]
fn a_failing_source_aborts_the_parallel_writer_without_hanging() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.zip");
    let mut w = create(&path, OutputFormat::Zip, &opts(4)).unwrap();
    let mut src = Source {
        i: 0,
        n: 200,
        fail_at: Some(120),
    };
    let res = w.add_files(&mut src);
    assert!(matches!(res, Err(Error::Io(_))), "{res:?}");
}

#[test]
fn an_early_parallel_error_closes_idle_workers_before_join() {
    const CHILD: &str = "RZ_EARLY_ZIP_ERROR_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.zip");
        let mut options = opts(4);
        options.memory_budget = 1;
        let mut writer = create(&path, OutputFormat::Zip, &options).unwrap();
        let mut src = Source {
            i: 0,
            n: 2,
            fail_at: Some(0),
        };
        assert!(matches!(writer.add_files(&mut src), Err(Error::Io(_))));
        return;
    }

    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("an_early_parallel_error_closes_idle_workers_before_join")
        .arg("--nocapture")
        .env(CHILD, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = child.wait_with_output().unwrap();
            assert!(
                status.success(),
                "child failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("parallel writer hung while joining idle workers after an early error");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn an_error_from_the_iterator_itself_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.zip");
    let mut w = create(&path, OutputFormat::Zip, &opts(4)).unwrap();
    let items: Vec<Result<FileSource<'static>>> = vec![
        Ok((entry("ok.txt", 3), Box::new(Cursor::new(b"abc".to_vec())))),
        Err(Error::Cancelled),
    ];
    let res = w.add_files(&mut items.into_iter());
    assert!(matches!(res, Err(Error::Cancelled)), "{res:?}");
}

#[test]
fn many_files_through_the_writer_read_back_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.zip");
    let mut w = create(&path, OutputFormat::Zip, &opts(8)).unwrap();
    let mut src = Source {
        i: 0,
        n: 500,
        fail_at: None,
    };
    w.add_files(&mut src).unwrap();
    w.finish().unwrap();
    let mut r = rz_formats::open(&path).unwrap();
    let names: Vec<String> = r.entries().unwrap().into_iter().map(|e| e.path).collect();
    let expected: Vec<String> = (0..500).map(|i| format!("f{i}.txt")).collect();
    assert_eq!(names, expected);
}

#[test]
fn large_deflated_entry_round_trips_across_parallel_chunk_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.zip");
    let mut seed = 0x5eedu64.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let block: Vec<u8> = (0..1024 * 1024usize)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 24) as u8
        })
        .collect();
    let data: Vec<u8> = block
        .iter()
        .copied()
        .cycle()
        .take(36 * 1024 * 1024)
        .collect();
    let mut options = opts(4);
    options.portable = true;
    let mut writer = create(&path, OutputFormat::Zip, &options).unwrap();
    let mut large_entry = entry("大文件.bin", data.len() as u64);
    large_entry.unix_mode = Some(0o755);
    let files = [Ok((
        large_entry,
        Box::new(Cursor::new(data.clone())) as Box<dyn Read + Send>,
    ))];
    writer.add_files(&mut files.into_iter()).unwrap();
    writer.finish().unwrap();

    match std::process::Command::new("unzip")
        .arg("-t")
        .arg(&path)
        .output()
    {
        Ok(output) => assert!(
            output.status.success(),
            "Info-ZIP rejected the archive; source CRC {:08x}: {}{}",
            crc32fast::hash(&data),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("could not run Info-ZIP: {error}"),
    }

    let mut archive = rz_formats::open(&path).unwrap();
    let entries = archive.entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "大文件.bin");
    assert_eq!(entries[0].unix_mode, Some(0o100644));
    let mut extracted = Vec::new();
    archive
        .open(&entries[0])
        .unwrap()
        .read_to_end(&mut extracted)
        .unwrap();
    assert_eq!(extracted, data);
}

#[test]
fn failed_large_source_stops_parallel_deflate_workers() {
    struct FailsAfterChunks {
        remaining: usize,
    }

    impl Read for FailsAfterChunks {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::Error::other("source failed"));
            }
            let count = output.len().min(self.remaining);
            output[..count].fill(b'x');
            self.remaining -= count;
            Ok(count)
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("failure.zip");
    let mut writer = create(&path, OutputFormat::Zip, &opts(4)).unwrap();
    let files = [Ok((
        entry("large.bin", 40 * 1024 * 1024),
        Box::new(FailsAfterChunks {
            remaining: 2 * 1024 * 1024,
        }) as Box<dyn Read + Send>,
    ))];
    let result = writer.add_files(&mut files.into_iter());
    assert!(matches!(result, Err(Error::Io(_))), "{result:?}");
}
