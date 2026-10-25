//! The task history: a short, durable record of what finished, kept next to the
//! settings so it survives restarts and in-place upgrades.

use crate::tasks::TaskId;
use rz_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Older records are dropped past this many, so the file stays small.
pub const MAX_RECORDS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Success,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// What kind of job it was (`extract`, `compress`, ...), stable across languages.
    pub kind: String,
    /// The archive's file name; never a full path.
    pub title: String,
    pub outcome: Outcome,
    /// One sentence for the user: the result, or why it failed.
    pub detail: String,
    /// Seconds since the Unix epoch.
    pub at: i64,
    /// The user has looked at this record.
    #[serde(default)]
    pub seen: bool,
    /// The task in this session's queue, while it is still there (for its report).
    #[serde(skip)]
    pub task: Option<TaskId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    /// Newest first.
    #[serde(default)]
    pub records: Vec<Record>,
}

/// What the menu badges show.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub failed: usize,
    pub succeeded: usize,
}

impl History {
    /// A missing file is an empty history; an unreadable one is moved aside
    /// (`history.json.bad`) so the app starts and nothing is silently destroyed.
    pub fn load(path: &Path) -> History {
        let Ok(text) = std::fs::read_to_string(path) else {
            return History::default();
        };
        match serde_json::from_str::<History>(&text) {
            Ok(mut h) => {
                h.records.truncate(MAX_RECORDS);
                h
            }
            Err(_) => {
                let mut bad = path.as_os_str().to_owned();
                bad.push(".bad");
                let _ = std::fs::rename(path, bad);
                History::default()
            }
        }
    }

    /// Writes through a temporary file in the same folder, so a crash mid-save
    /// leaves the previous file whole.
    pub fn save(&self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self).map_err(|e| Error::Corrupt(e.to_string()))?;
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = dir.join(format!(
            ".history.{}-{}.tmp",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let written = std::fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(json.as_bytes())?;
            f.sync_all()
        });
        if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::Io(e));
        }
        Ok(())
    }

    pub fn push(&mut self, record: Record) {
        self.records.insert(0, record);
        self.records.truncate(MAX_RECORDS);
    }

    /// Records the user has not looked at yet, by outcome. Cancelled ones are not news.
    pub fn unseen(&self) -> Counts {
        let mut c = Counts::default();
        for r in self.records.iter().filter(|r| !r.seen) {
            match r.outcome {
                Outcome::Failed => c.failed += 1,
                Outcome::Success => c.succeeded += 1,
                Outcome::Cancelled => {}
            }
        }
        c
    }

    /// Returns whether anything changed, so the caller knows to save.
    pub fn mark_all_seen(&mut self) -> bool {
        let mut changed = false;
        for r in self.records.iter_mut().filter(|r| !r.seen) {
            r.seen = true;
            changed = true;
        }
        changed
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(outcome: Outcome, title: &str) -> Record {
        Record {
            kind: "extract".into(),
            title: title.into(),
            outcome,
            detail: String::new(),
            at: 1,
            seen: false,
            task: None,
        }
    }

    #[test]
    fn records_survive_a_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let mut h = History::default();
        h.push(record(Outcome::Success, "a.zip"));
        h.push(record(Outcome::Failed, "b.zip"));
        h.save(&path).unwrap();
        let back = History::load(&path);
        assert_eq!(back, h);
        assert_eq!(back.records[0].title, "b.zip");
    }

    #[test]
    fn only_unseen_successes_and_failures_count() {
        let mut h = History::default();
        h.push(record(Outcome::Success, "a"));
        h.push(record(Outcome::Failed, "b"));
        h.push(record(Outcome::Failed, "c"));
        h.push(record(Outcome::Cancelled, "d"));
        assert_eq!(
            h.unseen(),
            Counts {
                failed: 2,
                succeeded: 1
            }
        );
        assert!(h.mark_all_seen());
        assert_eq!(h.unseen(), Counts::default());
        assert!(!h.mark_all_seen());
    }

    #[test]
    fn the_history_keeps_only_the_newest_records() {
        let mut h = History::default();
        for i in 0..MAX_RECORDS + 5 {
            h.push(record(Outcome::Success, &i.to_string()));
        }
        assert_eq!(h.records.len(), MAX_RECORDS);
        assert_eq!(h.records[0].title, (MAX_RECORDS + 4).to_string());
    }

    #[test]
    fn a_damaged_file_is_set_aside_and_the_app_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(History::load(&path), History::default());
        assert!(dir.path().join("history.json.bad").exists());
        assert_eq!(
            History::load(&dir.path().join("missing.json")),
            History::default()
        );
    }
}
