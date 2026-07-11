// SPDX-License-Identifier: GPL-3.0-only
//! Process-wide counting allocator for `ui_sim`'s Slint-rendering rigs.
//!
//! The UI perf-pass baseline needs an allocation-count hook for BOTH halves
//! of the render hot path it's baselining: the pure render-logic/state-build
//! side (`build_message_items`/`render_mentions` — covered by the `ui_perf`
//! crate's own `counting_alloc`, see `docs/perf/ui-perf-baseline.md`) and the
//! Slint RENDER path itself (`render_by_line`/`draw_if_needed` — this
//! module, paired with `perf_profile.rs`'s dirty-region rig). The two live in
//! separate crates because they measure different processes (`ui_perf`'s
//! `#[global_allocator]` never touches Slint at all; this one only makes
//! sense alongside a `MinimalSoftwareWindow`), so this is NOT a duplicate of
//! `ui_perf::counting_alloc` — it is a second, narrower instance of the same
//! transparent-`System`-wrapper idea, scoped to `ui_sim`'s own binaries.
//!
//! # Why a `#[global_allocator]`, and why it is safe to install unconditionally
//!
//! Every method here delegates the actual allocation decision to `System`
//! unchanged (same layout in, same pointer/behavior out) — this wrapper only
//! adds atomic counters around each call. Installing it process-wide has
//! zero effect on any existing `ui_sim` test's behavior or the allocations
//! it performs, only on whether they're now counted.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOC_COUNT: AtomicU64 = AtomicU64::new(0);

/// Counting wrapper over `System`. Install as `#[global_allocator]` exactly
/// once per binary (Rust enforces this at compile time).
pub struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        // Only charge a SUCCESSFUL allocation (non-null) — see
        // `ui_perf::counting_alloc::CountingAllocator::alloc`'s identical
        // reasoning: an OOM failure didn't actually claim any memory, so
        // counting it here would over-report a churn/allocation count this
        // hook's whole job is to be trustworthy about.
        if !ptr.is_null() {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    // `realloc`'s default trait-provided body calls `self.alloc` +
    // `self.dealloc` internally, both already instrumented above — no
    // override needed to keep it counted.
}

/// A point-in-time reading of the counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub allocs: u64,
    pub bytes: u64,
    pub deallocs: u64,
}

impl Snapshot {
    pub fn delta(self, since: Snapshot) -> Snapshot {
        Snapshot {
            allocs: self.allocs.saturating_sub(since.allocs),
            bytes: self.bytes.saturating_sub(since.bytes),
            deallocs: self.deallocs.saturating_sub(since.deallocs),
        }
    }
}

/// Read the current counters without resetting them.
pub fn snapshot() -> Snapshot {
    Snapshot {
        allocs: ALLOC_COUNT.load(Ordering::Relaxed),
        bytes: ALLOC_BYTES.load(Ordering::Relaxed),
        deallocs: DEALLOC_COUNT.load(Ordering::Relaxed),
    }
}

/// Measure the allocator activity of `f`: `snapshot()` before and after,
/// returned as a delta alongside `f`'s return value.
pub fn measure<T>(f: impl FnOnce() -> T) -> (T, Snapshot) {
    let before = snapshot();
    let out = f();
    let after = snapshot();
    (out, after.delta(before))
}
