//! SVC-04 / SVC-09: half-finished store operations must be visible and
//! recoverable, concurrent book operations must not lose entries, and no
//! password ever shows up in Debug output or error text.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rz_app::passwords::{find_password, MemoryStore, PasswordBook, SecretStore};
use rz_app::tasks::{JobSpec, TaskError};
use rz_core::{CancelToken, CreateOptions, Error, NoProgress};
use rz_engine::compress;
use rz_formats::OutputFormat;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A store whose write succeeded but whose bookkeeping failed: the operation
/// must surface an error while the state stays inspectable and truthful.
struct HalfBrokenStore {
    inner: MemoryStore,
    fail_set: Arc<AtomicBool>,
    fail_delete: Arc<AtomicBool>,
}

impl HalfBrokenStore {
    fn new() -> (Self, Arc<AtomicBool>, Arc<AtomicBool>) {
        let set = Arc::new(AtomicBool::new(false));
        let del = Arc::new(AtomicBool::new(false));
        (
            Self {
                inner: MemoryStore::default(),
                fail_set: Arc::clone(&set),
                fail_delete: Arc::clone(&del),
            },
            set,
            del,
        )
    }
}

impl SecretStore for HalfBrokenStore {
    fn get(&self, label: &str) -> Option<String> {
        self.inner.get(label)
    }
    fn set(&self, label: &str, secret: &str) -> rz_core::Result<()> {
        self.inner.set(label, secret)?;
        if self.fail_set.load(Ordering::Relaxed) {
            return Err(Error::Corrupt("index write failed".into()));
        }
        Ok(())
    }
    fn delete(&self, label: &str) -> rz_core::Result<()> {
        self.inner.delete(label)?;
        if self.fail_delete.load(Ordering::Relaxed) {
            return Err(Error::Corrupt("index delete failed".into()));
        }
        Ok(())
    }
    fn labels(&self) -> Vec<String> {
        self.inner.labels()
    }
}

#[test]
fn half_finished_store_operations_are_visible_and_recoverable() {
    let (store, fail_set, fail_delete) = HalfBrokenStore::new();
    let book = PasswordBook::new(store);

    book.remember("a", "secret-a").unwrap();

    // A set that stored the entry but failed its index reports the failure;
    // the book then tells the truth about what is really in the store.
    fail_set.store(true, Ordering::Relaxed);
    book.remember("b", "secret-b").unwrap_err();
    let mut labels = book.labels();
    labels.sort();
    assert_eq!(labels, ["a", "b"], "the store state must stay truthful");

    // Same for a delete that removed the entry but failed its index.
    fail_set.store(false, Ordering::Relaxed);
    fail_delete.store(true, Ordering::Relaxed);
    book.forget("a").unwrap_err();
    assert_eq!(book.labels(), ["b"]);
    assert_eq!(book.candidates(), ["secret-b".to_string()]);

    // With the fault gone the book is fully usable again.
    fail_delete.store(false, Ordering::Relaxed);
    book.forget("b").unwrap();
    assert!(book.labels().is_empty());
}

#[test]
fn concurrent_book_operations_lose_no_entries() {
    let book = Arc::new(PasswordBook::new(MemoryStore::default()));
    let mut handles = Vec::new();
    for t in 0..8 {
        let book = Arc::clone(&book);
        handles.push(std::thread::spawn(move || {
            for i in 0..25 {
                let label = format!("t{t}-i{i}");
                book.remember(&label, &format!("pw-{label}")).unwrap();
                if i % 3 == 0 {
                    book.forget(&label).unwrap();
                }
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let mut labels = book.labels();
    labels.sort();
    let expected: Vec<String> = {
        let mut v: Vec<String> = (0..8)
            .flat_map(|t| {
                (0..25)
                    .filter(|i| i % 3 != 0)
                    .map(move |i| format!("t{t}-i{i}"))
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(labels, expected);
}

// --------------------------------------------------------------- SVC-09 ----

const SECRET: &str = "TOPSECRET-hunter2";

fn encrypted_archive(dir: &Path) -> PathBuf {
    let src = dir.join("data");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "classified").unwrap();
    let out = dir.join("vault.7z");
    compress(
        &[src],
        &out,
        OutputFormat::SevenZ,
        &CreateOptions {
            password: Some(SECRET.into()),
            ..CreateOptions::default()
        },
        &CancelToken::new(),
        &mut NoProgress,
    )
    .unwrap();
    out
}

#[test]
fn passwords_never_appear_in_debug_output() {
    let create = CreateOptions {
        password: Some(SECRET.into()),
        ..Default::default()
    };
    assert!(!format!("{create:?}").contains(SECRET));
    assert!(format!("{create:?}").contains("<redacted>"));

    let extract = rz_engine::ExtractOptions {
        password: Some(SECRET.into()),
        ..Default::default()
    };
    assert!(!format!("{extract:?}").contains(SECRET));

    // The same holds through the job specs the task queue logs.
    let spec = JobSpec::Compress {
        sources: vec![PathBuf::from("/tmp/x")],
        output: PathBuf::from("/tmp/y.zip"),
        format: OutputFormat::Zip,
        opts: create,
    };
    assert!(!format!("{spec:?}").contains(SECRET));
    let edit = JobSpec::Edit {
        archive: PathBuf::from("/tmp/z.zip"),
        edit: rz_app::modify::Edit::Delete(vec![]),
        opts: extract,
        level: 6,
        encoding: None,
    };
    assert!(!format!("{edit:?}").contains(SECRET));
}

#[test]
fn wrong_password_errors_and_candidates_never_leak_the_secret() {
    let dir = tempfile::tempdir().unwrap();
    let archive = encrypted_archive(dir.path());

    // A wrong password gives a password error whose text carries no secret.
    let mut r = rz_formats::open(&archive).unwrap();
    r.set_password("wrong").unwrap();
    let err = r
        .entries()
        .err()
        .or_else(|| {
            // 7z without header encryption lists entries; reading fails instead.
            let e = r.entries().unwrap()[0].clone();
            let mut sink = Vec::new();
            use std::io::Read;
            r.open(&e)
                .unwrap()
                .read_to_end(&mut sink)
                .err()
                .map(Error::Io)
        })
        .expect("a wrong password was accepted");
    let task_err = TaskError::from(&err);
    assert!(!err.to_string().contains(SECRET));
    assert!(!task_err.message.contains(SECRET));

    // Candidate probing reports none without echoing them anywhere.
    let got = find_password(&archive, &["one".into(), "two".into()]).unwrap();
    assert_eq!(got, None);
    let got = find_password(&archive, &[SECRET.into()]).unwrap();
    assert_eq!(got.as_deref(), Some(SECRET));
}
