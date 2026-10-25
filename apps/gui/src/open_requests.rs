use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct OpenRequests {
    next_id: u64,
    active_id: Option<u64>,
    pending: HashMap<PathBuf, u64>,
}

impl OpenRequests {
    pub(crate) fn begin(&mut self, path: &Path) -> (u64, bool) {
        if let Some(&id) = self.pending.get(path) {
            self.active_id = Some(id);
            return (id, false);
        }
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        self.pending.insert(path.to_path_buf(), id);
        self.active_id = Some(id);
        (id, true)
    }

    pub(crate) fn complete(&mut self, path: &Path, id: u64) -> Option<bool> {
        if self.pending.get(path) != Some(&id) {
            return None;
        }
        self.pending.remove(path);
        Some(self.active_id == Some(id))
    }

    pub(crate) fn is_active(&self, id: u64) -> bool {
        self.active_id == Some(id)
    }

    pub(crate) fn finish_active(&mut self, id: u64) {
        if self.is_active(id) {
            self.active_id = None;
        }
    }

    pub(crate) fn supersede_active(&mut self) {
        self.active_id = None;
    }
}

#[cfg(test)]
mod tests {
    use super::OpenRequests;
    use std::path::Path;

    #[test]
    fn a_late_open_result_cannot_take_focus_from_the_newest_request() {
        let mut requests = OpenRequests::default();
        let (slow, start_slow) = requests.begin(Path::new("slow.zip"));
        let (fast, start_fast) = requests.begin(Path::new("fast.zip"));
        assert!(start_slow && start_fast);

        assert_eq!(requests.complete(Path::new("fast.zip"), fast), Some(true));
        requests.finish_active(fast);
        assert_eq!(requests.complete(Path::new("slow.zip"), slow), Some(false));
        assert!(!requests.is_active(slow));
    }

    #[test]
    fn a_duplicate_pending_path_shares_one_open_request() {
        let mut requests = OpenRequests::default();
        let (first, start_first) = requests.begin(Path::new("same.zip"));
        let (second, start_second) = requests.begin(Path::new("same.zip"));
        assert_eq!(first, second);
        assert!(start_first);
        assert!(!start_second);
        assert_eq!(requests.complete(Path::new("same.zip"), first), Some(true));
        assert_eq!(requests.complete(Path::new("same.zip"), second), None);
    }

    #[test]
    fn an_explicit_navigation_supersedes_pending_open_focus() {
        let mut requests = OpenRequests::default();
        let (request, _) = requests.begin(Path::new("slow.zip"));
        requests.supersede_active();
        assert_eq!(
            requests.complete(Path::new("slow.zip"), request),
            Some(false)
        );
    }
}
