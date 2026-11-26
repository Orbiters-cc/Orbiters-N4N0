//! Tests: how much a piece of work allocates on its own thread, the measure of what a frame of a page makes anew.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local!(static MADE: Cell<u64> = const { Cell::new(0) });

fn one_more() {
    // (A thread on its way out has no count any more.)
    let _ = MADE.try_with(|n| n.set(n.get() + 1));
}

// SAFETY: every call goes to the system's allocator as it came.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        one_more();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        one_more();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        one_more();
        unsafe { System.realloc(at, layout, size) }
    }
    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        unsafe { System.dealloc(at, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// How many times `work` allocated on this thread.
pub fn count(work: impl FnOnce()) -> u64 {
    let before = MADE.with(Cell::get);
    work();
    MADE.with(Cell::get) - before
}
