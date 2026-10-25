use crate::recent::{RecentItem, RecentList};
use gpui_kit::UniformListScrollHandle;

pub struct LocationsState {
    pub list: RecentList,
    pub loading: bool,
    pub sorted: bool,
    generation: u64,
    pub(crate) scroll: UniformListScrollHandle,
}

impl Default for LocationsState {
    fn default() -> Self {
        Self {
            list: RecentList::for_folder(),
            loading: false,
            sorted: false,
            generation: 0,
            scroll: UniformListScrollHandle::new(),
        }
    }
}

impl LocationsState {
    pub(crate) fn begin_scan(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.loading = true;
        self.generation
    }

    pub(crate) fn apply_scan(&mut self, generation: u64, items: Vec<RecentItem>) -> bool {
        if self.generation != generation {
            return false;
        }
        self.loading = false;
        if self.sorted {
            self.list.set_items(items);
        } else {
            if self
                .list
                .selected
                .as_ref()
                .is_some_and(|p| !items.iter().any(|i| &i.path == p))
            {
                self.list.selected = None;
            }
            self.list.items = items;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn item(path: &str) -> RecentItem {
        RecentItem {
            path: PathBuf::from(path),
            name: path.to_string(),
            folder: String::new(),
            created: None,
            modified: None,
            size: None,
            exists: true,
            is_dir: true,
        }
    }

    #[test]
    fn an_old_folder_scan_cannot_restore_stale_items_or_selection() {
        let mut state = LocationsState::default();
        state.list.selected = Some(PathBuf::from("removed"));
        let old = state.begin_scan();
        let current = state.begin_scan();

        assert!(!state.apply_scan(old, vec![item("stale")]));
        assert!(state.loading);
        assert!(state.list.items.is_empty());
        assert_eq!(state.list.selected.as_deref(), Some(Path::new("removed")));

        assert!(state.apply_scan(current, vec![item("current")]));
        assert!(!state.loading);
        assert_eq!(state.list.items.len(), 1);
        assert_eq!(state.list.items[0].path, Path::new("current"));
        assert_eq!(state.list.selected, None);
    }
}
