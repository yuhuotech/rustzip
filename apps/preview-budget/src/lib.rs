#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

#[allow(unsafe_code)]
mod budget;

pub use budget::{BudgetAllocator, LIVE_ALLOCATION_LIMIT, TOTAL_ALLOCATION_LIMIT};
