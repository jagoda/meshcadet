# ADR-0007 — Web Provisioner Codec: Pure JS + Golden-Vector CI Guard

- **Status:** Accepted (2026-07-11)
- **Deciders:** Maintainer design review (`meshcadet-web-provisioner` campaign)
- **Supersedes:** —
- **Implements:** ADR-0001 §4 (admin configuration interface, physical
  possession = admin authority), ADR-0002 (provisioning wire format)
- **Code:** `site/provisioner/codec.js`, `site/provisioner/codec.conformance.test.mjs`,
  `xtask/src/golden.rs`, `xtask/src/bin/gen_prov_golden_vectors.rs`,
  `.github/workflows/pages-check.yml` (`codec-conformance` job)

## Context

The `meshcadet-web-provisioner` campaign adds a browser-based device
provisioner page to the existing GitHub Pages site (`site/`), mirroring the
host `meshcadet` CLI's capabilities (status/identity readout, contact/channel
management, notification defaults, device-name/PIN set, history export/clear)
over the Web Serial API — the same one-click, no-toolchain ethos as the
existing web flasher (ADR-0006). Every one of those operations is a
`protocol::provisioning` frame: `magic(2) + type(1) + len(2) + payload(N) +
crc16(2)`, with fixed-layout payload encoders/decoders (ADR-0002). The browser
needs a byte-exact implementation of that codec, and the campaign's up-front
recon surfaced two load-bearing findings that shape this decision.

### Finding 1: the provisioner needs the FRAME codec, not MeshCore crypto

The provisioning path is **plaintext framing only**. Adding a contact takes a
hex Ed25519 pubkey; adding a channel takes a hex 32-byte secret (or one freshly
generated client-side via `crypto.getRandomValues`); the on-air channel hash
is computed **by the firmware**, never by the host/browser. None of
`protocol::{crypto, identity}` (AES-128-ECB, ECDH, HMAC-SHA256, Ed25519
signing) is exercised anywhere on the provisioning path — that machinery
exists solely for **on-air message** encryption (DM ECDH, channel AES),
which the provisioner page never touches. This narrows the reusable surface
to `protocol::provisioning`'s frame/CRC/payload codec (plus the
`FRAME_RSP_HISTORY_ENTRY` payload codec in `protocol::history`, needed for
the `export-history` capability) — small (a few hundred lines), pure
byte-shuffling, and crypto-free. That smallness is what makes a hand port
tractable at all (see Decision).

### Finding 2: `host/src/session.rs` needs no async refactor

`Session<T: Transport>` is already generic over its transport, but its
orchestration (`recv_frame`'s retry loop, magic-byte resync) is welded to
`std::time::Instant` / `std::thread::sleep` — synchronous blocking that
fights the browser's async, single-threaded Web Serial model. Because this
ADR does NOT WASM-compile `protocol` or `session.rs` (see Decision), no
refactor of `session.rs` is required or attempted; it is read as a reference
for orchestration shape (retry cadence, magic-resync algorithm) and is not
modified by this or any downstream campaign mission. The browser session
(a later campaign milestone) is a fresh, small async reimplementation
driving the codec this ADR defines.

## Decision

### 1. Pure-JS codec, hand-ported from `protocol::provisioning`

`site/provisioner/codec.js` is a plain ES module (no build step) that
reimplements, byte-for-byte:

- Frame encode/decode (`encodeFrame`/`decodeFrame`): the same
  `magic(2)+type(1)+len_lo(1)+len_hi(1)+payload+crc16(2)` layout and the same
  `BadMagic`/`TruncatedFrame`/`CrcMismatch` error precedence as
  `encode_frame`/`decode_frame`.
- CRC-16/ARC (`crc16`), byte-identical algorithm to the private `crc16` helper
  in `provisioning.rs` (polynomial `0x8005` reflected, init `0x0000`, no final
  XOR; known-answer `0xBB3D` for `"123456789"`, asserted directly in the
  conformance test — see §3).
- `findMagicStart`, mirroring `find_magic_start` in `host/src/session.rs` /
  `firmware/src/provisioning_server.rs` (ESP-IDF log-noise resync on the
  shared USB-serial stream) — logic-only, not itself part of
  `protocol::provisioning`'s public codec surface, so it has no golden
  vectors; it's covered by local tests in `codec.conformance.test.mjs`
  instead.
- Every payload encoder the page needs to send: `ADD_CONTACT`/`DEL_CONTACT`,
  `ADD_CHANNEL`/`DEL_CHANNEL`, `SET_NOTIF_DEFAULTS`, `SET_PIN`,
  `SET_DEVICE_NAME` (`QUERY_STATUS`/`QUERY_CONTACTS`/`QUERY_CHANNELS`/
  `COMMIT_PROVISIONING`/`EXPORT_HISTORY`/`CLEAR_HISTORY` have empty payloads —
  no dedicated encoder needed beyond `encodeFrame(FRAME_X)`).
- Every payload decoder the page needs to read:
  `RSP_OK`/`RSP_ERROR`/`RSP_STATUS`/`RSP_IDENTITY`/
  `RSP_CONTACT`/`RSP_CONTACTS_DONE`/`RSP_CHANNEL`/`RSP_CHANNELS_DONE`/
  `RSP_HISTORY_ENTRY`/`RSP_HISTORY_DONE` — including `decodeRspStatus`'s
  legacy 55-/57-/59-byte backward-compatibility acceptance (ADR-0002's
  `battery_raw_mv`/`battery_held_raw_mv` staged-rollout amendments) and
  `decodeRspHistoryEntry`'s `Option`-shaped (return-`null`-on-failure, not
  throw) contract mirroring `protocol::history::decode_rsp_history_entry`.

Decoded objects use the **same snake_case field names as the Rust payload
structs** (`gps_lat_e7`, `battery_raw_mv`, `device_name_len`, …) rather than
idiomatic-JS camelCase — a deliberate choice to keep `codec.js` a literal,
line-searchable mirror of `protocol/src/provisioning.rs` for whoever has to
diff the two later, at the cost of a one-line ESLint-style nitpick a future
contributor may raise (accepted).

### 2. WASM-compiling `protocol` — rejected

The alternative is compiling `protocol` (or a thin `provisioning`-only
wrapper crate) to WebAssembly via `wasm-pack`, giving true single-source-of-
truth with a compiler enforcing the two sides never diverge. Rejected:

- **Breaks "no build step, on purpose"** (`site/README.md`, reaffirmed by
  ADR-0006): a WASM build needs `cargo` + `wasm-pack` in the Pages deploy
  pipeline, a bundler or hand-wired `.wasm`-loading glue, and a committed or
  CI-rebuilt binary blob — exactly the toolchain weight this site has twice
  now (ADR-0006, this ADR) deliberately stayed out of.
- **Doesn't remove the JS half of the work anyway.** The async Web Serial
  I/O (port open, chunked read/write, retry/resync orchestration) has to be
  hand-written JS regardless of where the codec lives — Web Serial has no
  Rust binding. WASM would only remove drift risk from the codec's ~300
  lines while adding real build-pipeline complexity to the whole site.
- **The codec is small and crypto-free** (Finding 1): the correctness-
  critical surface is exactly the part a golden-vector CI guard can pin down
  cheaply, unlike (say) a full crypto stack where a subtle hand-port bug
  could be exploitable and much harder to catch with example-based tests
  alone.

Revisit only if the codec's scope grows substantially past today's frame
types, or the maintainer comes to prefer single-source-of-truth guarantees
over the no-build-step convention enough to accept the pipeline cost.

### 3. Drift is a CI failure, not a silent field bug: golden-vector conformance guard

A hand port has no compiler cross-check, so drift between `codec.js` and
`protocol::provisioning` would otherwise surface as a silently-corrupt frame
on a real device — the worst possible failure mode for a physical-possession-
authenticated admin channel. Two new pieces close that gap:

1. **`xtask --bin gen-prov-golden-vectors`** (`xtask/src/golden.rs`) calls the
   REAL `protocol::provisioning`/`protocol::history` encode/decode functions
   — never hand-authored bytes — to emit 31 representative vectors as JSON:
   17 **encode**-direction command vectors (JS given the same logical
   `params` must reproduce `frame_hex`/`payload_hex` exactly) and 14
   **decode**-direction response vectors, where — critically — `expect` is
   populated by actually calling the Rust `decode_*` function on the
   Rust-`encode_*`-built payload, not by echoing the input values back. That
   closes the one gap a naive "encode with X, assert JS decodes to X" design
   would leave open: an encode/decode asymmetry already present in the Rust
   codec itself would otherwise never show up in a vector, because the
   vector's *expectation* is the Rust codec's own round-trip output, not an
   assumption about it. `xtask`'s own `cargo test -p xtask` (part of
   `ci.yml`'s `test` job) self-checks that every vector's `frame_hex`
   re-decodes to its recorded `frame_type`/`payload_hex` before it's ever
   handed to JS.
2. **`site/provisioner/codec.conformance.test.mjs`** (plain `node`, zero
   dependencies — no `package.json`, matching the site's build-step-free
   posture) loads that JSON and, for every vector, either calls `codec.js`'s
   matching `encode*` function and byte-compares the result, or decodes the
   frame and field-compares the decoded object against `expect`. It also
   carries a handful of local, non-golden-vector sanity checks: the
   CRC-16/ARC known-answer value, and `findMagicStart` resync behavior
   against synthetic log-noise. This was verified to actually catch drift,
   not just pass vacuously, by deliberately corrupting `codec.js`'s CRC
   polynomial constant during development and confirming 32/38 checks failed
   before reverting.
3. **`.github/workflows/pages-check.yml`** gained a `codec-conformance` job
   running exactly those two commands (generate → conform), and the
   workflow's `paths:` trigger was extended beyond `site/**` to also include
   `protocol/src/provisioning.rs`, `protocol/src/history.rs`, and the
   `xtask` golden-vector generator files — **not just `site/**`** — so a PR
   that changes only the Rust wire format (no `site/` touch at all) still
   runs the guard. Without that extension, exactly the drift this guard
   exists to catch could land through a PR the guard never even saw.

This mirrors the `MAX_VERSIONS`/`max_versions` manual-invariant-with-guard
philosophy ADR-0006 §4 already established for this site: where a shared
build step isn't available to enforce a cross-file invariant automatically,
pin it with an explicit, loud CI check instead of hoping nobody forgets.

## Client-side security model

Stated deliberately here (per the campaign plan) rather than left implicit,
since this ADR is the first artifact in the campaign that a security-minded
reviewer would look at:

- **Auth factor unchanged.** Web Serial requires an explicit per-port user
  permission gesture (the browser's native "choose a device to connect"
  prompt) before any byte can be read or written. That gesture — physical
  possession of the USB cable plus explicit browser consent — remains the
  sole authority, exactly as ADR-0001 §4 intends. This ADR introduces no new
  auth surface; the codec is transport-agnostic plumbing, not a policy
  decision.
- **All-client-side, no backend.** GitHub Pages is fully static (ADR-0004/
  ADR-0006); nothing this codec touches is ever sent to a server. Downstream
  provisioner-page missions building on this codec must uphold: no
  analytics/telemetry beacons on the provisioner page; secrets (channel
  secrets, PIN, exported history text) never placed in the URL,
  `localStorage`, `sessionStorage`, or `console.log`; PIN input masked;
  history export is an explicit user-initiated local download, never
  automatic; sensitive in-memory state dropped on disconnect/page unload.
- **No transport encryption, same as the wire format itself.** ADR-0002 §4
  already establishes the CRC is an integrity check, not a MAC, and that the
  cable is the authentication. This codec makes no attempt to add
  confidentiality/authenticity the wire format itself doesn't have — that
  would be security theater over a channel whose actual security property
  (physical possession) is unaffected by anything client-side JS does.

## Consequences

- `codec.js` and `protocol/src/provisioning.rs` are two independently
  maintained implementations of the same wire format. The golden-vector guard
  (§3) makes divergence a loud, PR-blocking CI failure instead of a silent
  field bug, but a maintainer changing `provisioning.rs` still needs to
  update `codec.js` by hand — there is no compiler enforcing it, only a test
  suite. Acceptable given Finding 1's scope (crypto-free, small, low churn:
  ADR-0002's revision history shows on the order of one amendment per
  quarter).
- `site/provisioner/` has no `package.json`/`node_modules` — the conformance
  test imports `codec.js` directly and uses only Node's built-in
  `node:assert`/`node:fs`. `pages-check.yml`'s `codec-conformance` job needs a
  Rust toolchain (to run the generator) in addition to Node, which
  `check`'s (relative-path) job does not — kept as two separate jobs so a
  Rust-toolchain hiccup doesn't block the fast relative-path check and vice
  versa, mirroring `ci.yml`'s job-separation rationale.
- Downstream campaign work (the Web Serial session, `provisioner.html`,
  contact/channel/PIN/history capabilities) all import `codec.js` as their
  shared, already-conformance-guarded frame layer — no further codec design
  decisions should be needed there, only session orchestration and UI.

## Alternatives Considered

### A. WASM-compile `protocol::provisioning`
Covered above (Decision §2) — rejected for breaking the no-build-step
convention and not actually eliminating the hand-written JS transport layer.

### B. Skip the golden-vector guard; rely on manual review + the existing Rust test suite
`protocol/src/provisioning.rs`'s own `#[cfg(test)]` module already round-trips
every payload type on the Rust side, so a reviewer might argue that's
"enough" coverage of the format. Rejected: those tests say nothing about
whether `codec.js` still agrees with `provisioning.rs` after either changes —
exactly the drift this ADR's threat model (a silently corrupt frame reaching
a physically-connected device) cares about. A guard that only runs on the
Rust side can't catch a JS-side typo, and vice versa.

### C. Hand-copy input values into golden-vector `expect` fields instead of round-tripping through the real `decode_*` functions
Simpler to write, but — see Decision §3 point 1 — would leave a real gap: any
existing encode/decode asymmetry in the Rust codec itself would never appear
in a vector, since the vector's expectation would just restate the assumption
rather than the codec's own output. Rejected in favor of the full
encode-via-Rust → decode-via-Rust → serialize-the-decoded-struct pipeline.
