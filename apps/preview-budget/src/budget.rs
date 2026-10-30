use std::alloc::{GlobalAlloc, Layout, System};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const LIVE_ALLOCATION_LIMIT: usize = 256 * 1024 * 1024;
pub const TOTAL_ALLOCATION_LIMIT: usize = 512 * 1024 * 1024;

pub struct BudgetAllocator {
    live_limit: usize,
    total_limit: usize,
    live: AtomicUsize,
    total: AtomicUsize,
}

impl BudgetAllocator {
    pub const fn new() -> Self {
        Self::with_limits(LIVE_ALLOCATION_LIMIT, TOTAL_ALLOCATION_LIMIT)
    }

    const fn with_limits(live_limit: usize, total_limit: usize) -> Self {
        Self {
            live_limit,
            total_limit,
            live: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
        }
    }

    fn reserve(counter: &AtomicUsize, limit: usize, amount: usize) -> bool {
        let mut current = counter.load(Ordering::Relaxed);
        loop {
            let Some(next) = current.checked_add(amount) else {
                return false;
            };
            if next > limit {
                return false;
            }
            match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }
}

impl Default for BudgetAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: each allocation reserves both budgets before calling `System`; deallocation returns
// the exact live-size reservation associated with the original layout.
unsafe impl GlobalAlloc for BudgetAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !Self::reserve(&self.live, self.live_limit, layout.size()) {
            return ptr::null_mut();
        }
        if !Self::reserve(&self.total, self.total_limit, layout.size()) {
            self.live.fetch_sub(layout.size(), Ordering::Relaxed);
            return ptr::null_mut();
        }

        // SAFETY: `layout` is forwarded unchanged to the system allocator.
        let pointer = unsafe { System.alloc(layout) };
        if pointer.is_null() {
            self.live.fetch_sub(layout.size(), Ordering::Relaxed);
            self.total.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the pointer and layout are the live block returned by this allocator.
        unsafe { System.dealloc(pointer, layout) };
        self.live.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn live_heap_budget_is_released_but_cumulative_budget_is_not() {
        let allocator = BudgetAllocator::with_limits(32, 40);
        let layout = Layout::from_size_align(24, 8).unwrap();

        // SAFETY: the test returns the block with the same layout immediately below.
        let first = unsafe { allocator.alloc(layout) };
        assert!(!first.is_null());
        // SAFETY: `first` is live and was allocated above with this exact layout.
        unsafe { allocator.dealloc(first, layout) };

        // SAFETY: a null return is expected because the cumulative budget would be exceeded.
        let second = unsafe { allocator.alloc(layout) };
        assert!(second.is_null());
    }
}
