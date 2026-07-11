// SPDX-License-Identifier: GPL-3.0-only
//! `cargo run -p xtask --bin gen-prov-golden-vectors` — prints the
//! provisioning-codec golden-vector JSON to stdout. See `xtask::golden` for
//! the generator design; `.github/workflows/pages-check.yml`'s conformance
//! step pipes this into `site/provisioner/codec.conformance.test.mjs`.

fn main() {
    print!("{}", xtask::golden::golden_vectors_json());
}
