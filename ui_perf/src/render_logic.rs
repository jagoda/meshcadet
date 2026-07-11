// SPDX-License-Identifier: GPL-3.0-only
//! Benchmark fixtures for `firmware_core::ui::message_view::build_message_items`
//! / `render_mentions` — the MessageView state-build hot path.
//!
//! This module used to hand-port those two functions from
//! `firmware/src/ui/mod.rs` (a detached, cross-compiled workspace this crate
//! can't depend on directly — see this crate's own `Cargo.toml` doc) into a
//! plain-Rust duplicate, with its own mirrored test suite pinned against the
//! firmware original ("the drift contract that comes with that", per this
//! module's prior doc). The `firmware-core-extract-ui-runtime` increment
//! moved `build_message_items`/`render_mentions` (plus the `MessageRecord`/
//! `MessageItem` types they operate over) into `firmware-core` — a
//! root-workspace crate this crate CAN depend on directly, host-testable and
//! `slint`-free — so this module now benchmarks the real functions instead
//! of a shadow copy, closing that drift risk entirely: `ui_perf_bench`'s
//! numbers are for the exact code `firmware` ships, and the correctness
//! tests live once, in `firmware-core`, rather than twice.
//!
//! Only [`bench_fixtures`] remains genuinely local to this crate: synthetic
//! conversation-shape generation for the benchmark harness, not duplicated
//! production logic.

pub use firmware_core::ui::message_view::{build_message_items, render_mentions};
pub use firmware_core::ui::MessageRecord;

/// Synthetic bench inputs — representative conversation shapes, not
/// exercised by firmware-core's own correctness tests (those pin exact
/// behavior on small fixtures; these exist purely to give `ui_perf_bench` a
/// realistic distribution of plain/DM/channel/mention traffic at scale).
pub mod bench_fixtures {
    use super::MessageRecord;

    /// One synthetic conversation of `n` records, cycling through: a plain
    /// DM, a channel message with a sender prefix, and a channel message
    /// with a sender prefix AND an other-node mention — so every branch in
    /// `build_message_items`/`render_mentions` is exercised proportionally
    /// on every bench run, not just the cheapest path.
    pub fn conversation(n: usize) -> Vec<MessageRecord> {
        (0..n)
            .map(|i| match i % 3 {
                0 => MessageRecord {
                    text: format!(
                        "plain DM body number {i} with a little more text to size it realistically"
                    ),
                    is_ours: i % 2 == 0,
                    acked: true,
                    ts_ms: 0,
                },
                1 => MessageRecord {
                    text: format!("Alice: channel message number {i} reporting status normally"),
                    is_ours: false,
                    acked: false,
                    ts_ms: 0,
                },
                _ => MessageRecord {
                    text: format!("Bob: hey @[Carol] channel message number {i} needs your eyes"),
                    is_ours: false,
                    acked: false,
                    ts_ms: 0,
                },
            })
            .collect()
    }

    /// The known-names set matching `conversation`'s Alice/Bob/Carol cast.
    pub const KNOWN: &[&str] = &["Alice", "Bob", "Carol"];
}
