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
