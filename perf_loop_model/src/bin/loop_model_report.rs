// SPDX-License-Identifier: GPL-3.0-only
//! Prints the full sensitivity sweep + dominance table
//! ([`perf_loop_model::report::render_text_report`]) to stdout. Same
//! pattern as `ui_perf`'s `src/bin/bench.rs`: run this, paste the output
//! (labelled SIMULATED) into `docs/perf/perf-loop-model-baseline.md`.
//!
//! Reproduce: `cargo run -p perf_loop_model --release --bin loop_model_report`

fn main() {
    print!("{}", perf_loop_model::report::render_text_report());
}
