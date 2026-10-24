//! Application services shared by every front end (GUI and CLI): the task
//! queue, settings, smart extraction, update verification, install stats and
//! the password book.

// Tests may unwrap; library code may not (see docs/tech-spec.md).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod browse;
pub mod modify;
pub mod passwords;
pub mod preview;
pub mod settings;
pub mod smart;
pub mod stats;
pub mod tasks;
pub mod update;

pub use settings::{OnConflict, Settings, Theme};
pub use tasks::{Event, JobSpec, TaskId, TaskInfo, TaskManager, TaskState};
