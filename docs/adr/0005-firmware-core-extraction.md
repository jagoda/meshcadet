# ADR-0005 — `firmware-core` Extraction

- **Status:** Accepted (2026-07-11)
- **Deciders:** Maintainer design review
- **Supersedes:** —
- **Implements:** —
- **Code:** `firmware-core/` (new root-workspace crate), `Cargo.toml`
  (`members`), `firmware/Cargo.toml` (`firmware-core` path dependency,
  `diagnostics` feature forwarding), `firmware/src/dispatcher.rs`,
  `firmware/src/pin_menu.rs`, `firmware/src/ui/notification.rs`,
  `firmware/src/ui/screens/gps_status.rs`, `firmware/src/gps.rs`,
  `firmware/src/battery.rs`, `firmware/src/runtime_settings_store.rs` (all
  reduced to thin re-export shims over their hardware-owning remainder).

## Context

`firmware/` is a **detached** Cargo workspace (its own `[workspace]` table in
`firmware/Cargo.toml`) because it cross-compiles for `xtensa-esp32s3-espidf`
under the Espressif `esp` toolchain and links against an ESP-IDF sysroot —
see the root `Cargo.toml`'s doc comment for why that split exists (keeps
`cargo test`/`fmt`/`clippy` at the repo root fast, stable-toolchain-only, and
independent of `espup`/ESP-IDF being installed).

That split has a cost the codebase had been absorbing silently: `firmware`'s
`[[bin]]` sets `harness = false` (a Slint-related build requirement), and
nothing at the repo root ever builds the detached workspace, so **every
`#[cfg(test)]` block written inside `firmware/src/**` type-checks but never
executes.** CI's `firmware` job only runs `cargo build`/`check-all-features.sh`
— it proves the cross-compile stays green, not that any of firmware's own
unit tests pass. Several `firmware/src/*.rs` modules had accumulated large,
carefully-written test suites (pure PIN-verification logic, dispatcher
dedup/airtime/TX-queue state machines, NMEA parsing, battery percent
inference, notification/blink-loop timing, …) that had sat as documentation
of intent, never once run as a regression guard, for as long as the
detached-workspace split existed.

A large and growing share of that logic has no hardware dependency at all —
it is plain Rust operating on bytes, integers, and small state structs, with
no `esp-idf-*` or `slint` import anywhere in the call graph. `protocol`
(the wire-format crate shared by `firmware` and the host CLI) already proves
the shape of the fix: it is a root-workspace member **and** a `path`
dependency of the detached `firmware` crate, so it compiles for host (tests
execute) and for `xtensa-esp32s3-espidf` (as a firmware dependency) with zero
target-specific code.

## Decision

**Extract the hardware-independent half of firmware's non-UI logic into a new
root-workspace crate, `firmware-core`, following the exact `protocol`
pattern — and prove the pattern end-to-end by moving the fully-decoupled
modules plus the already-isolated pure helpers first.**

### 1. `firmware-core`: pure Rust, `std`, `protocol`-only

`firmware-core/Cargo.toml` depends only on `protocol` (`version.workspace =
true`, inheriting the one project-wide version — see ADR-0004). No
`esp-idf-*` crate, no `slint`. It is added to the root `Cargo.toml`
`members` list alongside `protocol`, and it is *also* added as a `path`
dependency of `firmware/Cargo.toml` (`firmware-core = { path =
"../firmware-core" }`) — the same dual membership `protocol` already has
across the workspace boundary.

### 2. Behavior-preserving MOVE, not a rewrite

Each moved module's logic is copied verbatim (byte-for-byte where the module
was already 100% pure) into `firmware-core`; nothing is redesigned. Every
`#[cfg(test)]` block moves with the functions/types it tests.

`firmware/src/<module>.rs` is reduced to a thin shim:

```rust
pub use firmware_core::<module>::*;
```

so every existing call site (`crate::gps::FixState`,
`crate::dispatcher::DuplicateFilter`, `battery::BatteryStatus`, …) resolves
completely unchanged — callers never learn the implementation moved.

### 3. The split within a module: pure logic vs. hardware ownership

Four of the seven modules touched are already 100% pure and moved wholesale
(`dispatcher.rs`, `pin_menu.rs`, `ui/notification.rs`'s model, and the
`gps_status.rs` display-string formatters). Three needed an actual split
between pure logic and the piece that owns real hardware:

| Module | Pure half → `firmware_core::*` | Hardware half stays in `firmware/src/*` |
|---|---|---|
| `gps.rs` | NMEA GGA/RMC parsing, checksum/baud-framing validation, calendar arithmetic, duty-cycle transition predicates, `FixState`/`GpsStatus`/`GpsFix`, baud/init-command tables | `GpsDriver` (UART1 ownership, baud probing/self-heal state machine, NVS baud cache, `settimeofday`) |
| `battery.rs` | `BatteryStatus`, `percent_from_millivolts`, `battery_poll_step`, calibration/threshold constants | `BatteryDriver` (ADC1 sampling) |
| `runtime_settings_store.rs` | `serialize`/`deserialize` blob codec, `fallback_settings` | `load`/`save` (`EspNvs` read/write) |

The split point in each case is exactly the module's own pre-existing
"pure — host-testable" doc-comment section header (e.g. `gps.rs`'s "Tests
(pure functions — no hardware dependency)" and "Duty-cycle transition
predicates (pure — testable without hardware)"); nothing was reclassified
against the author's own prior judgment about where the hardware boundary
sat, only relocated across a crate boundary.

`ui/screens/gps_status.rs` splits differently: the `slint::slint!{}` markup
and the `GpsStatusScreen` Rust wrapper depend on Slint and stay in
`firmware/`, while the four `format_*` display-string helpers move to
`firmware_core::ui::gps_status`.

### 4. Diagnostics feature forwarding

`gps::hex_dump_tail` (and its tests) are gated behind `--features
diagnostics` in the original code. `firmware-core` grew the same
`diagnostics` feature; `firmware/Cargo.toml`'s own `diagnostics` feature now
forwards to it (`diagnostics = ["firmware-core/diagnostics"]`), so a single
`--features diagnostics` firmware build still compiles in exactly the same
set of diagnostic-only code as before.

### 5. Latent test defects surfaced by making tests executable

Making a `#[cfg(test)]` block run for the first time is exactly the point of
this extraction — and it immediately found three: `parse_rmc_typical_active_fix`
asserted an internally-inconsistent date/year that never matched either its
own comment or the documented 2-digit-year decode rule;
`second_message_while_asleep_keeps_single_loop` polled the blink loop at a
timestamp that lands in the "off" half of a blink phase while asserting
"on"; `dispatcher_fire_is_audio_only_for_non_incoming_event` fired an event
(`DmAcked`) whose documented default is `audible: false`, then asserted a
tone was queued. All three are corrected in place (test-only fixes — the
production logic they exercise needed no change, and two of the three are
independently corroborated by sibling tests that already covered the same
logic correctly). See the in-file comments at each fix site for the full
diagnosis. No other module's tests needed correction — with 140 tests now
executing, three latent defects is a small, plausible yield for logic that
had never once been run.

## Consequences

- `cargo test --workspace` now exercises ~140 tests that previously only
  type-checked. Every one of them regresses instantly, in CI, the moment a
  future change breaks the invariant it pins — instead of silently bit-rotting
  until (or unless) someone thought to run `firmware`'s own toolchain by hand.
- `firmware-core` inherits the workspace version automatically
  (`version.workspace = true`); it does **not** trip CI's
  `version-drift-guard` job (ADR-0004 §1), which only compares the root
  workspace version against the detached `firmware/Cargo.toml`'s hardcoded
  literal.
- `ci.yml` needed **no changes** — `cargo test --workspace --locked` (the
  `test` job) and `cargo clippy --workspace --all-targets --locked -- -D
  warnings` (the `clippy` job) automatically pick up the new root-workspace
  member.
- This is leg 1 of a multi-leg campaign extracting more of `firmware`'s pure
  logic the same way. Later legs are scoped and landed independently; this
  ADR records the pattern each of them follows, not the full target end
  state.

## Alternatives Considered

### A. Fold `firmware/` into the root Cargo workspace

Rejected for the same reason ADR-0004 rejects it: the root workspace's whole
point is staying buildable on stable rustc with no `espup`/ESP-IDF install.
Merging the detached workspace back in would drag the `esp` toolchain and a
multi-gigabyte ESP-IDF sysroot into every `cargo test`/`clippy` invocation at
the repo root.

### B. A host-only `#[cfg(test)]`-gated shadow crate, not a real dependency

Considered generating a separate test-only host build of the pure modules
rather than a real `firmware-core` library `firmware` depends on for
production too. Rejected: `protocol` already proves the simpler alternative
(one real crate, dual-compiled) works, and a shadow/test-only copy would
either duplicate the source (drift risk) or need its own include-mechanism
more complex than a Cargo `path` dependency.
