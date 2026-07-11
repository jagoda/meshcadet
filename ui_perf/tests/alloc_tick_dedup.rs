// SPDX-License-Identifier: GPL-3.0-only
//! Host-native ALLOCATION measurement for the `alloc-and-tick` optimization
//! (UI performance pass, optimization 2).
//!
//! THE DEFECT (before this fix): `UiRuntime::set_gps_status` /
//! `set_battery_status` (`firmware/src/ui/mod.rs`) run UNCONDITIONALLY every
//! `main.rs` dispatcher-loop iteration — far more often than the values they
//! format actually change (`fix_age_secs`/`clock_sync_age_secs` tick once a
//! second; `percent`/`charging` change only on a real ADC-derived state
//! transition). Whenever the GpsStatus/AdminMenu screen is open, EVERY
//! iteration re-ran `format!`/`to_string()` (4 heap `String`s for GPS, 1 for
//! battery) and re-pushed them into Slint properties, even when the snapshot
//! was byte-identical to the one already on screen — "unconditional recompute
//! of state that rarely changes" + "String churn per step()", exactly the
//! targets this fix eliminates.
//!
//! THE FIX (landed in `firmware/src/ui/mod.rs`): both setters now early-return
//! before formatting/pushing when the fields the screen actually renders
//! haven't changed — full `GpsStatus` equality for the GPS row (the whole
//! struct is exactly what the four rows render); `(percent, charging)` only
//! for battery (`raw_mv`/`held_raw_mv` are live diagnostic-only fields the row
//! never shows — comparing the whole struct would defeat the dedup, since
//! those jitter every ADC sample independent of the displayed percentage).
//!
//! This test cannot import firmware's types directly (`firmware` is a
//! DETACHED xtensa-only workspace — see its `Cargo.toml`'s doc comment), so it
//! PORTS the exact formatting/dedup logic host-side, pinned line-for-line
//! against firmware's own fixtures (`gps_status.rs`'s and `admin_menu.rs`'s
//! `#[cfg(test)]` modules — see each port function's doc for its firmware
//! source), the same "host port pinned against firmware's never-executed-
//! on-host fixtures" pattern the `profile-baseline` measurement rig
//! established. It then drives a realistic dispatcher-loop tick sequence
//! through a global counting allocator and proves the guarded path allocates
//! for only the ticks that actually changed the display, not every tick.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

// ── Counting allocator ──────────────────────────────────────────────────────
//
// Forwards to `System`; counts every successful (non-null) allocation and its
// byte size. `#[global_allocator]` is process-wide and this file is its own
// integration-test binary (own process), so it cannot collide with any other
// `tests/*.rs` file in this crate — but `cargo test` still runs this file's
// OWN four `#[test]` fns concurrently on separate threads within that one
// process by default, and a shared PROCESS-WIDE counter does not distinguish
// between them.
//
// FIX (found while
// authoring `flush_line_alloc.
// rs`'s own allocation-counting test, which hit the identical failure mode): a `AtomicUsize` pair here let
// `ported_gps_formatters_match_firmware_fixtures` /
// `ported_battery_formatter_matches_firmware_fixtures`'s own incidental
// `String` allocations bleed into whichever OTHER test's before/after
// snapshot happened to be mid-flight on another thread at the same moment —
// a real, reproduced flake (roughly 1 in 15 runs locally: `cargo test -p
// ui_perf --test alloc_tick_dedup` in a loop). Thread-local counters give
// each test's own OS thread (Rust's test harness spawns one per `#[test]`) an
// isolated tally, making every scenario's count exact and reproducible
// regardless of test execution order/parallelism.

struct CountingAllocator;

thread_local! {
    static ALLOC_COUNT: Cell<usize> = const { Cell::new(0) };
    static ALLOC_BYTES: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            ALLOC_COUNT.with(|c| c.set(c.get() + 1));
            ALLOC_BYTES.with(|c| c.set(c.get() + layout.size()));
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn alloc_snapshot() -> (usize, usize) {
    (ALLOC_COUNT.with(|c| c.get()), ALLOC_BYTES.with(|c| c.get()))
}

// ── Ported formatting logic (pinned to firmware/src/ui/screens/gps_status.rs) ─

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FixState {
    NoSignal,
    Acquiring,
    Fix,
}

/// Port of `firmware/src/ui/screens/gps_status.rs::format_fix_state`.
fn format_fix_state(state: FixState) -> &'static str {
    match state {
        FixState::NoSignal => "No signal",
        FixState::Acquiring => "Acquiring\u{2026}",
        FixState::Fix => "Fix acquired",
    }
}

/// Port of `firmware/src/ui/screens/gps_status.rs::format_sat_count`.
fn format_sat_count(sat_count: u8) -> String {
    if sat_count == 1 {
        "1 satellite".to_string()
    } else {
        format!("{} satellites", sat_count)
    }
}

/// Port of `firmware/src/ui/screens/gps_status.rs::format_coords`.
fn format_coords(has_fix: bool, lat_e7: i32, lon_e7: i32, fix_age_secs: u32) -> String {
    if !has_fix {
        return "\u{2014}".to_string();
    }
    let lat_deg = lat_e7 as f64 / 10_000_000.0;
    let lon_deg = lon_e7 as f64 / 10_000_000.0;
    format!("{:.6}, {:.6} (age {}s)", lat_deg, lon_deg, fix_age_secs)
}

/// Port of `firmware/src/ui/screens/gps_status.rs::format_time_sync`.
fn format_time_sync(clock_synced: bool, clock_sync_age_secs: u32) -> String {
    if clock_synced {
        format!("Synced (age {}s)", clock_sync_age_secs)
    } else {
        "Not synced".to_string()
    }
}

/// Port of `firmware::gps::GpsStatus` — a plain `Copy`/`PartialEq` snapshot,
/// pinned to the same field set (see that struct's doc in `firmware/src/gps.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GpsStatus {
    has_fix: bool,
    fix_state: FixState,
    lat_e7: i32,
    lon_e7: i32,
    fix_age_secs: u32,
    sat_count: u8,
    clock_synced: bool,
    clock_sync_age_secs: u32,
}

impl GpsStatus {
    const fn never() -> Self {
        GpsStatus {
            has_fix: false,
            fix_state: FixState::NoSignal,
            lat_e7: 0,
            lon_e7: 0,
            fix_age_secs: 0,
            sat_count: 0,
            clock_synced: false,
            clock_sync_age_secs: 0,
        }
    }
}

/// Port of `GpsStatusScreen::set_status` — the 4-`String` build the row push
/// pays every time it runs (this is the cost the guard skips).
fn build_gps_row_strings(status: &GpsStatus) -> [String; 4] {
    [
        format_fix_state(status.fix_state).to_string(),
        format_sat_count(status.sat_count),
        format_coords(status.has_fix, status.lat_e7, status.lon_e7, status.fix_age_secs),
        format_time_sync(status.clock_synced, status.clock_sync_age_secs),
    ]
}

// ── Ported formatting logic (pinned to firmware/src/ui/screens/admin_menu.rs) ─

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BatteryStatus {
    percent: u8,
    charging: bool,
    raw_mv: u32,
    held_raw_mv: u32,
}

/// Port of `firmware/src/ui/screens/admin_menu.rs::format_battery_display`.
fn format_battery_display(status: BatteryStatus) -> String {
    if status.charging {
        format!("{}% (charging)", status.percent)
    } else {
        format!("{}%", status.percent)
    }
}

/// Port of `battery_display_fields_changed` (`firmware/src/ui/mod.rs`) — the
/// dedup predicate this fix adds: only `percent`/`charging` gate the row,
/// NOT the live/noisy `raw_mv`/`held_raw_mv` diagnostic fields.
fn battery_display_fields_changed(prev: BatteryStatus, new: BatteryStatus) -> bool {
    prev.percent != new.percent || prev.charging != new.charging
}

// ── Parity: pin the ported strings against firmware's own fixtures ─────────
//
// firmware/src/ui/screens/gps_status.rs's and admin_menu.rs's `#[cfg(test)]`
// modules assert these exact expected strings against the real functions;
// this proves the host port hasn't drifted from what's actually landed.

#[test]
fn ported_gps_formatters_match_firmware_fixtures() {
    assert_eq!(format_fix_state(FixState::NoSignal), "No signal");
    assert_eq!(format_fix_state(FixState::Acquiring), "Acquiring\u{2026}");
    assert_eq!(format_fix_state(FixState::Fix), "Fix acquired");
    assert_eq!(format_sat_count(0), "0 satellites");
    assert_eq!(format_sat_count(1), "1 satellite");
    assert_eq!(format_sat_count(8), "8 satellites");
    assert_eq!(format_coords(false, 0, 0, 0), "\u{2014}");
    assert_eq!(
        format_coords(true, 481_173_000, 115_166_667, 42),
        "48.117300, 11.516667 (age 42s)"
    );
    assert_eq!(
        format_coords(true, -335_100_000, -1_511_200_000, 5),
        "-33.510000, -151.120000 (age 5s)"
    );
}

#[test]
fn ported_battery_formatter_matches_firmware_fixtures() {
    let not_charging = BatteryStatus { percent: 63, charging: false, raw_mv: 0, held_raw_mv: 0 };
    assert_eq!(format_battery_display(not_charging), "63%");
    let charging = BatteryStatus { percent: 9, charging: true, raw_mv: 0, held_raw_mv: 0 };
    assert_eq!(format_battery_display(charging), "9% (charging)");
}

// ── THE MEASUREMENT ─────────────────────────────────────────────────────────

/// A dispatcher-loop tick sequence shaped like the real one: a GPS fix (and
/// its `fix_age_secs`) that's genuinely fresh only once every `period` ticks
/// — mirroring `RX_POLL_YIELD_MS`-cadence `step()` calls (many/sec) against
/// once-a-second age/coordinate updates.
fn gps_ticks(n: usize, period: usize) -> Vec<GpsStatus> {
    (0..n)
        .map(|i| GpsStatus {
            has_fix: true,
            fix_state: FixState::Fix,
            lat_e7: 481_173_000,
            lon_e7: 115_166_667,
            fix_age_secs: (i / period) as u32,
            sat_count: 8,
            clock_synced: true,
            clock_sync_age_secs: (i / period) as u32,
        })
        .collect()
}

#[test]
fn gps_status_push_allocates_only_on_genuine_change() {
    let ticks = gps_ticks(100, 20); // 100 ticks, value changes every 20th (5 real changes)

    // ── BEFORE (old firmware behavior): format + push every single tick ────
    let mut prev = GpsStatus::never();
    let before = alloc_snapshot();
    for status in &ticks {
        let strings = build_gps_row_strings(status);
        std::hint::black_box(&strings);
        prev = *status;
    }
    std::hint::black_box(prev);
    let after = alloc_snapshot();
    let unconditional_allocs = after.0 - before.0;
    let unconditional_bytes = after.1 - before.1;

    // ── AFTER (this fix): skip format+push when unchanged ────────
    let mut prev = GpsStatus::never();
    let before = alloc_snapshot();
    let mut pushes = 0usize;
    for status in &ticks {
        if *status == prev {
            continue;
        }
        prev = *status;
        let strings = build_gps_row_strings(status);
        std::hint::black_box(&strings);
        pushes += 1;
    }
    let after = alloc_snapshot();
    let guarded_allocs = after.0 - before.0;
    let guarded_bytes = after.1 - before.1;

    println!(
        "[alloc-tick] GPS row, {} ticks: UNCONDITIONAL {} allocs / {} bytes vs GUARDED {} allocs / {} bytes ({} pushes)",
        ticks.len(), unconditional_allocs, unconditional_bytes, guarded_allocs, guarded_bytes, pushes,
    );

    // THE WIN: the guarded path allocates only on the ticks that actually
    // changed the displayed value — one push per distinct `fix_age_secs`
    // value the sequence takes (0..=4 over 100 ticks/period 20 = 5 groups;
    // the first group's push is the `never()` -> first-real-status
    // transition).
    assert_eq!(pushes, 5, "expected 5 distinct fix_age_secs groups over 100 ticks/period 20");
    assert!(
        guarded_allocs < unconditional_allocs / 10,
        "guarded allocs ({guarded_allocs}) should be an order of magnitude below \
         unconditional allocs ({unconditional_allocs}) over {} ticks",
        ticks.len(),
    );
    assert!(guarded_bytes < unconditional_bytes / 10);
}

#[test]
fn battery_display_push_allocates_only_on_percent_or_charging_change() {
    // Battery poll every tick, but percent/charging are stable while raw_mv
    // jitters (live ADC noise) — the exact shape `battery.rs`'s live sampling
    // produces. 3 genuine display changes over 50 ticks.
    let ticks: Vec<BatteryStatus> = (0..50u32)
        .map(|i| BatteryStatus {
            percent: 80 - (i / 20) as u8, // changes at i=20, i=40
            charging: i >= 45,            // changes at i=45
            raw_mv: 3700 + i,             // jitters every tick — must NOT gate the row
            held_raw_mv: 3690 + i,
        })
        .collect();

    // ── BEFORE: format + push every tick ────────────────────────────────────
    let before = alloc_snapshot();
    for status in &ticks {
        let s = format_battery_display(*status);
        std::hint::black_box(&s);
    }
    let after = alloc_snapshot();
    let unconditional_allocs = after.0 - before.0;

    // ── AFTER: gate on (percent, charging) only ─────────────────────────────
    let mut prev = BatteryStatus { percent: 0, charging: false, raw_mv: 0, held_raw_mv: 0 };
    let before = alloc_snapshot();
    let mut pushes = 0usize;
    for status in &ticks {
        if battery_display_fields_changed(prev, *status) {
            let s = format_battery_display(*status);
            std::hint::black_box(&s);
            pushes += 1;
        }
        prev = *status;
    }
    let after = alloc_snapshot();
    let guarded_allocs = after.0 - before.0;

    println!(
        "[alloc-tick] battery row, {} ticks: UNCONDITIONAL {} allocs vs GUARDED {} allocs ({} pushes) \
         — raw_mv jitters every tick and correctly does NOT gate the row",
        ticks.len(), unconditional_allocs, guarded_allocs, pushes,
    );

    assert_eq!(pushes, 4, "expected 1 initial change + percent drop at 20 + percent drop at 40 + charging flip at 45");
    assert!(
        unconditional_allocs >= ticks.len(),
        "at least one String alloc per unconditional tick ({} ticks, {} allocs)",
        ticks.len(),
        unconditional_allocs,
    );
    // THE WIN: >80% fewer allocations by only formatting on the 4 ticks that
    // actually moved the displayed percentage/charging state, out of 50.
    assert!(
        guarded_allocs * 5 < unconditional_allocs,
        "guarded allocs ({guarded_allocs}) should be under 20% of unconditional \
         allocs ({unconditional_allocs}) over {} ticks ({} genuine display changes)",
        ticks.len(),
        pushes,
    );
}
