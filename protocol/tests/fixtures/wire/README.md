<!-- SPDX-License-Identifier: GPL-3.0-only -->
# Upstream wire vectors

Generated fixtures — do not hand-edit `*.json` here. They are written by:

```sh
cargo run -p xtask --bin xtask -- regen-wire-vectors \
  --upstream <MeshCore-checkout> --tag <companion-vX.Y.Z>
```

Each vector records its inputs, exact on-air bytes, upstream tag/commit, and
per-rule `file:line` citations. `protocol/tests/upstream_conformance.rs` loads
these files; see [`docs/upstream-conformance.md`](../../../../docs/upstream-conformance.md)
for provenance, regeneration, and the firmware-bump checklist.
