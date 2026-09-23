//! Per-thread allocation checks. Compiled only into test executables: this
//! crate's own, and dependents' through the `test-alloc` feature.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn count() {
    // Thread-local teardown may already have run when System frees test state.
    let _ = ALLOCATIONS.try_with(|count| {
        if let Some(current) = count.get() {
            count.set(Some(current + 1));
        }
    });
}

// SAFETY: Every operation forwards the same pointer/layout to System; the
// thread-local counter neither allocates nor changes allocation ownership.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Counts this thread's allocations from `start` until `finish` or drop.
pub struct Count(());

impl Count {
    pub fn start() -> Self {
        assert_eq!(ALLOCATIONS.get(), None, "nested allocation check");
        ALLOCATIONS.set(Some(0));
        Self(())
    }

    pub fn finish(self) -> usize {
        ALLOCATIONS.get().unwrap()
    }
}

impl Drop for Count {
    fn drop(&mut self) {
        ALLOCATIONS.set(None);
    }
}

pub fn assert_no_allocations<T>(operation: impl FnOnce() -> T) -> T {
    let count = Count::start();
    let result = operation();
    assert_eq!(count.finish(), 0, "report processing allocated");
    result
}
