//! Anonymous install statistics: how many installs, how many started, how many
//! are still in use. There are no accounts. A random ID identifies an install and
//! nothing else is sent: no file names, paths, archive contents or usage.

use rz_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// Everything that ever leaves the machine. Adding a field here is a privacy
/// change and must also be reflected in the privacy notice shown to users.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    /// `activate` the first time the app runs, `heartbeat` at most once per day after.
    pub event: &'static str,
    pub install_id: String,
    pub app_version: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub language: String,
}

/// Facts about the running environment, filled in by the front end.
#[derive(Debug, Clone)]
pub struct Environment {
    pub app_version: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub language: String,
}

pub trait Sender {
    fn send(&self, report: &Report) -> std::result::Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StatsState {
    pub install_id: String,
    pub activated: bool,
    /// Days since the Unix epoch of the last successful report.
    pub last_report_day: Option<i64>,
}

impl StatsState {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string(self).map_err(|e| Error::Corrupt(e.to_string()))?;
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// A random version-4 UUID.
pub fn new_install_id() -> String {
    let mut b = [0u8; 16];
    if getrandom::fill(&mut b).is_err() {
        // No randomness available: better no ID than a guessable one.
        return String::new();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Decides whether a report is due and sends it.
///
/// Returns the report that was sent, if any. Nothing happens when the user has
/// turned statistics off, and in that case no install ID is even created. A failed
/// send changes nothing, so it is simply tried again at the next launch.
pub fn report_if_due(
    state: &mut StatsState,
    enabled: bool,
    today: i64,
    env: &Environment,
    sender: &dyn Sender,
) -> Option<Report> {
    if !enabled {
        return None;
    }
    let event = if !state.activated {
        "activate"
    } else if state.last_report_day.is_none_or(|d| d < today) {
        "heartbeat"
    } else {
        return None;
    };
    if state.install_id.is_empty() {
        state.install_id = new_install_id();
        if state.install_id.is_empty() {
            return None;
        }
    }
    let report = Report {
        event,
        install_id: state.install_id.clone(),
        app_version: env.app_version.clone(),
        os: env.os.clone(),
        os_version: env.os_version.clone(),
        arch: env.arch.clone(),
        language: env.language.clone(),
    };
    sender.send(&report).ok()?;
    state.activated = true;
    state.last_report_day = Some(today);
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Recorder {
        sent: RefCell<Vec<Report>>,
        fail: bool,
    }
    impl Sender for Recorder {
        fn send(&self, r: &Report) -> std::result::Result<(), String> {
            if self.fail {
                return Err("offline".into());
            }
            self.sent.borrow_mut().push(r.clone());
            Ok(())
        }
    }

    fn rec(fail: bool) -> Recorder {
        Recorder {
            sent: RefCell::new(Vec::new()),
            fail,
        }
    }

    fn env() -> Environment {
        Environment {
            app_version: "0.1.0".into(),
            os: "macos".into(),
            os_version: "15.1".into(),
            arch: "aarch64".into(),
            language: "zh-CN".into(),
        }
    }

    #[test]
    fn first_run_activates_then_one_heartbeat_per_day() {
        let net = rec(false);
        let mut st = StatsState::default();
        let r = report_if_due(&mut st, true, 20000, &env(), &net);
        assert_eq!(r.as_ref().map(|r| r.event), Some("activate"));
        assert!(report_if_due(&mut st, true, 20000, &env(), &net).is_none());
        let r = report_if_due(&mut st, true, 20001, &env(), &net);
        assert_eq!(r.as_ref().map(|r| r.event), Some("heartbeat"));
        assert!(report_if_due(&mut st, true, 20001, &env(), &net).is_none());
        assert_eq!(net.sent.borrow().len(), 2);
        // Same install each time.
        let sent = net.sent.borrow();
        assert_eq!(sent[0].install_id, sent[1].install_id);
    }

    #[test]
    fn turned_off_sends_nothing_and_creates_no_id() {
        let net = rec(false);
        let mut st = StatsState::default();
        assert!(report_if_due(&mut st, false, 20000, &env(), &net).is_none());
        assert!(net.sent.borrow().is_empty());
        assert!(st.install_id.is_empty());
        assert!(!st.activated);
    }

    #[test]
    fn a_failed_send_is_retried_next_time_without_a_new_id() {
        let mut st = StatsState::default();
        assert!(report_if_due(&mut st, true, 20000, &env(), &rec(true)).is_none());
        assert!(!st.activated);
        let id = st.install_id.clone();
        assert!(!id.is_empty());
        let net = rec(false);
        let r = report_if_due(&mut st, true, 20000, &env(), &net);
        assert_eq!(r.map(|r| r.install_id), Some(id));
        assert!(st.activated);
    }

    #[test]
    fn the_report_contains_exactly_the_documented_fields() {
        let net = rec(false);
        let mut st = StatsState::default();
        let r = report_if_due(&mut st, true, 1, &env(), &net).expect("a report");
        let json = serde_json::to_value(&r).unwrap_or_default();
        let mut keys: Vec<&str> = json
            .as_object()
            .map(|o| o.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "app_version",
                "arch",
                "event",
                "install_id",
                "language",
                "os",
                "os_version"
            ]
        );
    }

    #[test]
    fn install_ids_are_valid_random_v4_uuids() {
        let a = new_install_id();
        let b = new_install_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(parts[2].starts_with('4'));
        assert!(matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'));
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
    }

    #[test]
    fn state_survives_restarts_and_damage() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        let p = dir.path().join("sub/stats.json");
        let st = StatsState {
            install_id: new_install_id(),
            activated: true,
            last_report_day: Some(5),
        };
        assert!(st.save(&p).is_ok());
        assert_eq!(StatsState::load(&p), st);
        let _ = std::fs::write(&p, "garbage");
        assert_eq!(StatsState::load(&p), StatsState::default());
        assert_eq!(
            StatsState::load(&dir.path().join("none")),
            StatsState::default()
        );
    }
}
