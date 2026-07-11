// SPDX-License-Identifier: GPL-3.0-only
//! `ui_perf_bench` — runnable host benchmark for the UI perf-pass Phase-1
//! baseline.
//!
//! Run with `cargo run -p ui_perf --release --bin ui_perf_bench`. `--release`
//! matters: these are µs/ns-scale hot paths and debug-profile overhead would
//! swamp the signal (firmware itself ships `opt-level = "z"` per the root
//! `Cargo.toml`; `--release` here is the closer host proxy of the two
//! default profiles, though still not identical codegen to the xtensa
//! target — see `docs/perf/ui-perf-baseline.md` for that caveat).
//!
//! Prints one line per measurement in `key=value` shape (grep-friendly for
//! before/after diffing across optimization children) plus a short banner.
//! Not wired to any pass/fail threshold — this IS the baseline being
//! established, there is nothing yet to compare against.

use std::time::Instant;
use ui_perf::counting_alloc::{AllocStats, CountingAllocator};
use ui_perf::render_logic::{bench_fixtures, build_message_items, render_mentions};

// This binary's OWN global allocator — declared here, not in the `ui_perf`
// library, because `#[global_allocator]` is a process-wide lang item and
// `ui_perf/tests/flush_line_alloc.rs` + `ui_perf/tests/alloc_tick_dedup.rs`
// (separate binary crates that also link this library) each already declare
// their own local one. A crate-wide declaration in lib.rs would collide with
// those at link time; this bin target is its own crate, so a bin-local
// declaration here is collision-free. See lib.rs's module doc for the full
// rationale.
#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator::new();

fn time_it<F: FnMut()>(iters: u32, mut f: F) -> std::time::Duration {
    // Warm up (page faults, branch predictor, allocator arena growth) before
    // the timed pass so steady-state cost is what gets measured.
    for _ in 0..(iters / 10).max(3) {
        f();
    }
    let start = Instant::now();
    for _ in 0..iters {
        f();
    }
    start.elapsed()
}

fn report_timing(label: &str, iters: u32, elapsed: std::time::Duration) {
    let ns_per_op = elapsed.as_nanos() as f64 / iters as f64;
    println!(
        "{label}: iters={iters} total_ms={:.3} ns_per_op={:.1}",
        elapsed.as_secs_f64() * 1000.0,
        ns_per_op
    );
}

fn report_alloc(label: &str, stats: AllocStats) {
    println!(
        "{label}: alloc_count={} dealloc_count={} realloc_count={} bytes_allocated={} bytes_freed={} net_live_bytes={}",
        stats.alloc_count, stats.dealloc_count, stats.realloc_count,
        stats.bytes_allocated, stats.bytes_freed, stats.net_live_bytes(),
    );
}

fn main() {
    println!("=== ui_perf_bench — Phase-1 host baseline ===");
    println!(
        "profile={} (run --release for representative numbers)",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!();

    // ── render_mentions — per-call timing ───────────────────────────────
    println!("-- render_mentions (firmware_core::ui::message_view::render_mentions) --");
    let plain = "just a plain message with no mentions in it at all";
    let one_mention = "hi @[Alice] how's the weather over there today";
    let self_mention = "hey @[Bob] can you check the north relay";
    let known = bench_fixtures::KNOWN;

    for (label, body, who) in [
        ("plain", plain, "Bob"),
        ("other_mention", one_mention, "Bob"),
        ("self_mention", self_mention, "Bob"),
    ] {
        let iters = 200_000;
        let elapsed = time_it(iters, || {
            std::hint::black_box(render_mentions(std::hint::black_box(body), who, known));
        });
        report_timing(&format!("render_mentions[{label}]"), iters, elapsed);
    }
    println!();

    // ── build_message_items — per-conversation-size timing + allocations ─
    println!("-- build_message_items (firmware_core::ui::message_view::build_message_items) --");
    for n in [10usize, 50, 200] {
        let records = bench_fixtures::conversation(n);
        let iters = (20_000 / n.max(1)).max(50) as u32;
        let elapsed = time_it(iters, || {
            std::hint::black_box(build_message_items(
                std::hint::black_box(&records),
                true,
                "Carol",
                known,
            ));
        });
        report_timing(&format!("build_message_items[n={n}]"), iters, elapsed);

        // Allocation count for ONE call at this conversation size — the
        // "per-step() allocation count" proxy: MessageView only rebuilds its
        // model on navigate/refresh (`refresh_message_view_for`,
        // `navigate_to_message_view` — firmware/src/ui/mod.rs:2430/:2538),
        // not on every dispatcher-loop `step()`, so this is the per-REBUILD
        // cost, the right unit for that call site.
        GLOBAL.reset();
        std::hint::black_box(build_message_items(
            std::hint::black_box(&records),
            true,
            "Carol",
            known,
        ));
        let stats = GLOBAL.snapshot();
        report_alloc(&format!("build_message_items[n={n}].alloc"), stats);
    }
    println!();

    println!("=== end baseline — see docs/perf/ui-perf-baseline.md for the ledger these numbers feed ===");
}
