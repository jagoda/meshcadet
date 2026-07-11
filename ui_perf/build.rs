// SPDX-License-Identifier: GPL-3.0-only
// See Cargo.toml's `[build-dependencies] slint-build` comment: this build
// script exists only so slint-build is a build-graph dependency, pulling the
// software-renderer/image features into slint-macros' i-slint-compiler
// instance. Mirrors ui_sim/build.rs.
fn main() {
    // Nothing to generate — the effect is purely on the dependency graph.
    println!("cargo:rerun-if-changed=build.rs");
}
