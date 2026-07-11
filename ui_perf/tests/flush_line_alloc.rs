// SPDX-License-Identifier: GPL-3.0-only
//! Host-native ALLOCATION + PARITY measurement for the per-dirty-line SPI
//! flush path (UI performance pass, optimization 3).
//!
//! THE DEFECT (before this fix): `ui/platform.rs::process_line` — Slint's
//! `LineBufferProvider` callback, invoked once per dirty scanline by
//! `render_by_line` — converted the dirty Slint `Rgb565Pixel` range into
//! embedded-graphics `Rgb565` by `.collect()`-ing into a heap `Vec`, then
//! passed the slice to `TDeckDisplay::flush_line_range`. That is a fresh heap
//! allocation (and matching deallocation) on EVERY dirty line — up to 240
//! times for a full-window repaint (confirmed baseline hotspot, `docs/perf/
//! ui-perf-baseline.md` §5 item 1) — nested inside `ui::step()` →
//! `render_if_needed()`, the SAME iteration whose wall-clock length gates how
//! promptly `main.rs`'s dispatcher loop gets back around to the NEXT
//! iteration's CAD attempt / RX poll (§6 "contention direction A" in that same
//! doc).
//!
//! THE FIX (landed in `firmware/src/ui/display.rs` +
//! `firmware/src/ui/platform.rs`): `flush_line_range` now takes a plain
//! `impl ExactSizeIterator<Item = Rgb565>` instead of a `&[Rgb565]` slice —
//! `mipidsi::fill_contiguous` already accepts (and streams) any
//! `IntoIterator`, so it never needed a materialized buffer. The call site
//! passes its `.map(..)` conversion straight through with no `.collect()`.
//!
//! This test cannot import firmware's xtensa-only types directly (detached
//! workspace — see `firmware/Cargo.toml`'s doc comment), so it PORTS the
//! exact per-pixel conversion formula from `platform.rs::process_line`
//! (bit-for-bit: `r = (px >> 11) & 0x1F`, `g = (px >> 5) & 0x3F`, `b = px &
//! 0x1F`) and proves, under a real counting `GlobalAlloc`:
//!
//! 1. **PARITY**: the OLD (collect-into-Vec) and NEW (iterator-direct) paths
//!    emit byte-identical pixel streams for the same input — the fix changes
//!    only where the conversion result is buffered, never what it computes.
//! 2. **IMPROVEMENT**: the OLD path allocates once per line (N lines ⇒ N
//!    allocations); the NEW path allocates ZERO times, for any line count,
//!    including the worst measured real-render case found
//!    (`RocketOnSend`'s 86-line peak dirty frame — see `motif_repaint.rs` /
//!    the baseline doc §3b) and the full 240-line worst case.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

// ── Counting allocator (own integration-test binary ⇒ own process) ─────────
//
// THREAD-LOCAL, not a shared process-wide atomic: `cargo test` runs each
// `#[test]` fn on its own thread, in parallel by default, all sharing one
// `#[global_allocator]` instance. A single process-wide counter would let
// concurrently-running sibling tests' allocations bleed into this file's
// measurement windows (observed directly while authoring this test: a
// 14-line window measured 19 allocations, not 14, until this fix). A
// thread-local counter gives each test's own OS thread an isolated tally,
// making the per-scenario counts exact and reproducible regardless of test
// execution order/parallelism.
thread_local! {
    static ALLOC_COUNT: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            ALLOC_COUNT.with(|c| c.set(c.get() + 1));
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn alloc_count() -> usize {
    ALLOC_COUNT.with(|c| c.get())
}

// ── Ported conversion formula (pinned to `firmware/src/ui/platform.rs::
// TDeckLineRenderer::process_line`) ─────────────────────────────────────────

/// Mirrors Slint's `Rgb565Pixel` — a raw RGB565 `u16` word, one per source
/// pixel (see `platform.rs`'s `line_buf: [Rgb565Pixel; DISPLAY_WIDTH]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rgb565Word(u16);

/// Host stand-in for `embedded_graphics::pixelcolor::Rgb565` — same three
/// 5/6/5-bit fields the real type stores, compared for byte-for-byte parity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EgRgb565 {
    r: u8,
    g: u8,
    b: u8,
}

/// Port of the per-pixel conversion in `platform.rs::process_line`:
/// ```ignore
/// let r = ((px.0 >> 11) & 0x1F) as u8;
/// let g = ((px.0 >> 5)  & 0x3F) as u8;
/// let b = (px.0 & 0x1F) as u8;
/// embedded_graphics::pixelcolor::Rgb565::new(r, g, b)
/// ```
fn convert_pixel(px: Rgb565Word) -> EgRgb565 {
    let r = ((px.0 >> 11) & 0x1F) as u8;
    let g = ((px.0 >> 5) & 0x3F) as u8;
    let b = (px.0 & 0x1F) as u8;
    EgRgb565 { r, g, b }
}

/// OLD path: `.collect()` into a heap `Vec` (pre-fix `process_line` +
/// pre-fix `TDeckDisplay::flush_line_range(&[Rgb565])`). Returns the
/// converted pixels so a caller can compare them, but the point under test is
/// the ALLOCATION this collect performs, not the return value.
fn convert_old_via_vec(line_buf: &[Rgb565Word], range: std::ops::Range<usize>) -> Vec<EgRgb565> {
    range.map(|i| convert_pixel(line_buf[i])).collect()
}

/// NEW path: stream the conversion straight into a sink with no intermediate
/// buffer at all — mirrors the fixed `process_line` passing `.map(..)`
/// directly into `flush_line_range(.., impl ExactSizeIterator<Item = Rgb565>)`, which
/// `mipidsi::fill_contiguous` consumes pixel-by-pixel. `sink` here stands in
/// for `fill_contiguous`'s internal `set_pixels` SPI stream.
fn convert_new_streamed(
    line_buf: &[Rgb565Word],
    range: std::ops::Range<usize>,
    mut sink: impl FnMut(EgRgb565),
) {
    for i in range {
        sink(convert_pixel(line_buf[i]));
    }
}

/// A synthetic 320-px-wide line buffer with a distinct, reproducible value in
/// every slot, so a byte-for-byte comparison actually exercises the full
/// conversion range rather than a degenerate all-zero/all-same input.
fn synthetic_line_buf(width: usize) -> Vec<Rgb565Word> {
    (0..width)
        .map(|i| Rgb565Word((i as u16).wrapping_mul(40503u16) ^ 0xACE5))
        .collect()
}

// ── 1. PARITY: identical pixel output, old vs. new ──────────────────────────

#[test]
fn old_and_new_paths_emit_byte_identical_pixels() {
    const WIDTH: usize = 320;
    let line_buf = synthetic_line_buf(WIDTH);

    // A partial dirty x-range, same shape as a real incremental redraw
    // (`ReusedBuffer` can deliver `range.end < DISPLAY_WIDTH` — see
    // `process_line`'s own doc).
    let range = 37..233usize;

    let old_pixels = convert_old_via_vec(&line_buf, range.clone());

    let mut new_pixels = Vec::with_capacity(range.len());
    convert_new_streamed(&line_buf, range.clone(), |px| new_pixels.push(px));

    assert_eq!(
        old_pixels, new_pixels,
        "iterator-direct flush path must emit the identical pixel stream as \
         the old collect-into-Vec path — this is a buffering change only"
    );
    assert_eq!(old_pixels.len(), range.len());
}

#[test]
fn old_and_new_paths_agree_on_a_full_width_line_too() {
    const WIDTH: usize = 320;
    let line_buf = synthetic_line_buf(WIDTH);
    let range = 0..WIDTH;

    let old_pixels = convert_old_via_vec(&line_buf, range.clone());
    let mut new_pixels = Vec::with_capacity(WIDTH);
    convert_new_streamed(&line_buf, range.clone(), |px| new_pixels.push(px));

    assert_eq!(old_pixels, new_pixels);
    assert_eq!(old_pixels.len(), WIDTH);
}

// ── 2. IMPROVEMENT: allocation count, old vs. new ───────────────────────────

#[test]
fn new_path_allocates_zero_times_per_line_old_path_allocates_once_per_line() {
    const WIDTH: usize = 320;
    let line_buf = synthetic_line_buf(WIDTH);
    let range = 0..WIDTH;

    // OLD: one heap Vec allocation for this single line's flush.
    let before = alloc_count();
    let _old = std::hint::black_box(convert_old_via_vec(&line_buf, range.clone()));
    let old_allocs = alloc_count() - before;
    assert!(
        old_allocs >= 1,
        "sanity: the pre-fix collect()-into-Vec path must allocate at least \
         once per line (measured {old_allocs})"
    );

    // NEW: stream into a stack-fixed sink (no Vec, no heap at all) — this is
    // what `flush_line_range`'s `impl ExactSizeIterator` parameter now lets the call
    // site do; `fill_contiguous`/`set_pixels` on real hardware stream
    // straight to the SPI peripheral the same way.
    let mut sum: u32 = 0; // stack accumulator; stands in for "bytes shipped over SPI"
    let before = alloc_count();
    convert_new_streamed(&line_buf, range.clone(), |px| {
        sum = sum.wrapping_add(px.r as u32 + px.g as u32 + px.b as u32);
    });
    let new_allocs = alloc_count() - before;
    std::hint::black_box(sum);

    assert_eq!(
        new_allocs, 0,
        "iterator-direct flush path must perform ZERO heap allocations per \
         line (measured {new_allocs}) — this is the whole fix"
    );
}

/// Extrapolates the per-frame allocation count the OLD vs. NEW path would pay
/// at the real dirty-line counts MEASURED via the actual
/// production Slint renderer (`motif_repaint.rs` / `model_update_repaint.rs`
/// in this crate, and `docs/perf/ui-perf-baseline.md` §3b): idle (0 lines),
/// `CometOnNotify` peak (14 lines), `RocketOnSend` peak (86 lines), and a
/// full-window navigation paint (240 lines, `HEIGHT`). Ties the isolated
/// per-line allocation result above back to whole-frame numbers comparable
/// across this pass's other optimizations.
#[test]
fn per_frame_allocation_projection_at_measured_dirty_line_counts() {
    use ui_perf::HEIGHT;

    const WIDTH: usize = 320;
    let line_buf = synthetic_line_buf(WIDTH);
    let range = 0..WIDTH;

    // Measured dirty-scanline counts from this crate's real-renderer harness
    // (`motif_repaint.rs`: CometOnNotify peak; `model_update_repaint.rs` /
    // baseline doc §3b: RocketOnSend peak) plus the two structural bounds
    // (idle, full window).
    let scenarios: &[(&str, usize)] = &[
        ("idle (no dirty lines)", 0),
        ("CometOnNotify peak frame", 14),
        ("RocketOnSend peak frame", 86),
        ("full-window navigation paint", HEIGHT as usize),
    ];

    println!("[flush-alloc] per-frame allocation projection (old vs. new flush path):");
    for (label, lines) in scenarios {
        let before = alloc_count();
        for _ in 0..*lines {
            let _v = std::hint::black_box(convert_old_via_vec(&line_buf, range.clone()));
        }
        let old_frame_allocs = alloc_count() - before;

        let before = alloc_count();
        let mut sink_acc: u32 = 0;
        for _ in 0..*lines {
            convert_new_streamed(&line_buf, range.clone(), |px| {
                sink_acc = sink_acc.wrapping_add(px.r as u32);
            });
        }
        std::hint::black_box(sink_acc);
        let new_frame_allocs = alloc_count() - before;

        println!(
            "  {label:<32} lines={lines:<4} old_allocs={old_frame_allocs:<5} new_allocs={new_frame_allocs}"
        );

        assert_eq!(
            new_frame_allocs, 0,
            "{label}: new flush path must allocate zero times regardless of dirty-line count"
        );
        if *lines > 0 {
            assert_eq!(
                old_frame_allocs, *lines,
                "{label}: old flush path allocates exactly once per dirty line"
            );
        }
    }
}
