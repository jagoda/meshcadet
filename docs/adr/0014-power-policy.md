# ADR-0014 — Power Policy: Invariants, Estimate Labelling, and the Light-Sleep/RX-Integrity Contract

- **Status:** Accepted (2026-08-23)
- **Deciders:** Maintainer design review (`meshcadet-power-optimization`
  campaign)
- **Supersedes:** —
- **Implements:** ADR-0012 (dispatcher/UI task split — the light-sleep
  contract below is stated against ADR-0012's DIO1-interrupt architecture and
  its core-0/core-1 split)
- **Code:** None. This ADR is docs-only — no firmware, `firmware-core`,
  protocol, or host code accompanies it. It is the contract every
  implementing leg of this campaign (`meshcadet-power-gps-standby`,
  `meshcadet-power-backlight-brightness`, `meshcadet-power-idle-screen`,
  `meshcadet-power-dfs`) is required to argue against, in that leg's own
  diff and PR body; this ADR is extended by those legs, not superseded by
  them.

## Context

The maintainer reported the T-Deck Plus "seems a little power hungry —
battery runs out a little fast." MeshCadet has never had a power pass: no
`CONFIG_PM_*` anywhere in the tree, both cores fixed at 240 MHz
(`firmware/sdkconfig.defaults:102`), the GPS receiver's RF front-end run at
full power 100% of the time despite a duty cycle that only stops *reading*
the UART, the backlight binary on/off at full duty, and a 62 Hz UI tick
running unconditionally against a dark panel while the screen is "asleep."

The maintainer ruled out building a power-measurement kit: *"Let's skip the
measurement kit and make reasonable implementation decisions."* Every power
claim in this campaign is therefore reasoned from datasheet-order evidence
and duty-cycle arithmetic, never from a milliamp reading, and every claim
must say so. This ADR is the contract-first leg of a nine-leg
power-optimization campaign — it exists so several different implementing
legs make the same *kind* of argument instead of each inventing its own.
The remaining legs are named throughout this document by their own mission
identifiers as they land.

## Decision

### D1 — The four hard constraints, as named invariants P1–P4

Every leg in this campaign is bound by four maintainer-stated constraints,
non-negotiable, and cited by name from here on:

| Invariant | Statement |
|---|---|
| **P1** | GPS must still report location on a reasonable interval. |
| **P2** | UI must remain responsive when being used. |
| **P3** | Notifications must work in a reasonable way. |
| **P4** | Radio operations must always work — no dropped or missed RX or TX. |

A leg's PR is not accepted on its power win alone: it must state, in its own
artifact (PR body or code comment, not merely this ADR), which of P1–P4 it
touches and why the change does not regress it. "Argued per leg, in the
leg's own artifact" is itself a binding predicate at this campaign's
milestone checkpoints — a constraint defense that exists only in this ADR
and not in the implementing PR does not satisfy it.

### D2 — No-measurement-kit ruling and the estimate-labelling rule

**Ruling.** No power-measurement kit exists for this device and none will be
built for this campaign (see the not-attempted register, D5). Consequently,
**no leg's acceptance is a milliamp delta.** Every power number this
campaign introduces — anywhere: this ADR, a PR body, a code comment, a
later write-up — is a **labelled estimate with its reasoning basis stated
inline**, never a bare number.

**The tag vocabulary** (deliberately compatible with, but not identical to,
`docs/perf/ui-perf-baseline.md` §0's provenance legend — that document tags
*timing* numbers this campaign never touches; this is the *power* analogue):

| Tag | Meaning |
|---|---|
| **`[DATASHEET]`** | Taken directly from a component datasheet or a documented electrical spec, with the source cited. Exact, but not a measurement of *this* device. |
| **`[ESTIMATE]`** | A reasoned, datasheet-order projection — never a measurement. Must state its reasoning basis inline (e.g. "duty-cycle fraction × datasheet delta"), not just carry the bracket. |
| **`[MEASURED, date, method]`** | An actual reading taken on real hardware, with the date and method named. **No number in this campaign carries this tag** — the maintainer's ruling means the kit to produce one does not exist. The tag is specified now so a *future* campaign (once a measurement path exists — see the light-sleep contract, D4) has the vocabulary ready and so any number that ever *does* get measured is distinguishable at a glance from everything estimated here. |

**The tag is a first-class field, not decoration.** A bracket without an
inline reasoning basis is the same defect as no bracket at all: a label that
merely looks right, without encoding the property it claims, is worse than
no label because it reads as verified when it is not. `[ESTIMATE]` on a
number with no stated duty cycle, no cited datasheet current, and no
arithmetic connecting them is not an estimate — it is a guess wearing an
estimate's tag. **A bare number without one of the three tags above is a
defect in whatever document contains it.**

### D3 — Per-leg acceptance template

Every implementing leg of this campaign is held to the same acceptance
shape, so that several legs produce comparable evidence rather than each
inventing its own notion of "done":

1. **A labelled estimate** (D2's vocabulary) in the leg's PR body and, if the
   leg introduces a genuinely new number, in this ADR's not-attempted
   register or its own D-section.
2. **Host unit tests** covering the leg's pure logic, in `firmware-core`
   where the logic is pure enough to live there. `firmware/` is a detached,
   cross-compiled workspace, so a `#[cfg(test)]` block written there
   type-checks but never runs under `cargo test --workspace` — pure logic
   belongs in `firmware-core` for exactly that reason.
3. **An `xtask` static guard** wherever the leg's diff contains a shape
   invariant worth pinning mechanically rather than trusting a reviewer to
   re-check by eye (e.g. "the standby command is unreachable except behind
   the detected-variant discriminator," "`render_if_needed` is unreachable
   from an asleep-state code path"). Not every leg necessarily earns one;
   whether it does is a judgment call the leg's own PR body must state, not
   silently skip.
4. **Workspace `cargo test`, `cargo clippy --workspace --all-targets -- -D
   warnings`, and `cargo fmt --all -- --check` green.** This is the real,
   host-runnable acceptance surface every leg can actually execute.
5. **Cross-compile delegated to CI, never claimed locally.** The `esp`
   cross-toolchain does not reliably bootstrap in every development sandbox
   (a prior fix, `ci-fix-meshcadet-lock-firmware-ui-20260823-174742559`,
   recorded "Could not install all requested Tools" and had to source its
   figure from CI). `cd firmware && cargo check --target
   xtensa-esp32s3-espidf` is CI's gate on the PR — no leg may claim green on
   a cross-compile it could not itself run.

The one leg that moves the app-image materially (`meshcadet-power-dfs`)
additionally reports a **fresh, absolute** partition-budget figure from
`cargo run -p xtask --bin xtask -- verify-partition-budget` (or CI's own step
output for that branch's head commit) — never a delta, and never today's
baseline cited as if it were this leg's own measurement.

### D4 — The light-sleep/RX-integrity contract: specified in full, implemented by no leg this campaign

This campaign implements dynamic frequency scaling (DFS) only
(`meshcadet-power-dfs`) and explicitly does **not** implement ESP-IDF light
sleep. The contract below is nonetheless specified completely, as the
executable ADR candidate a **future** campaign runs against once a hardware
validation path exists. A contract that is merely gestured at ("a future
campaign will handle it") without an explicit owner is not a contract, it is
a wish — so this section is the explicit specification, and it deliberately
assigns **no leg of this campaign** as implementor. Any future campaign that
picks it up must itself name an explicit owning child mission for every
clause below rather than inherit an implicit one from here.

1. **Wake source.** DIO1 (GPIO45, `firmware/src/radio.rs:82`) must be armed
   as a light-sleep GPIO wake source via `esp_sleep_enable_gpio_wakeup`, with
   its edge/level configuration matched to the SX1262's DIO1 latch-high-on-
   assert semantics already documented in `radio.rs`'s DIO1-wait module
   (`radio.rs:282` onward — "DIO1 is a LATCH, not a pulse").
2. **The load-bearing correctness property.** A DIO1 assertion that lands
   during the transition into or out of light sleep must still wake the SoC
   and be observed by the dispatcher task — i.e. the wake source must be
   proven not to drop an edge across the sleep/wake boundary. This is a
   direct defense of **P4** and it is the single hardest clause in this
   contract: it cannot be validated off hardware. A logic analyzer or
   equivalent is required, independent of whether a *power*-measurement kit
   ever exists — this is a correctness instrument, not a power instrument,
   and the maintainer's no-measurement-kit ruling is scoped to the latter,
   but no such instrument exists for this campaign either.
3. **Lock discipline.** An `esp_pm_lock` of type `ESP_PM_NO_LIGHT_SLEEP` must
   be held across every SPI2 transaction (both the radio's and the display
   controller's — they share the bus, ADR-0012 §7) and across the GPS UART's
   ACTIVE window (`firmware/src/gps.rs:938`–`949`). Anywhere a peripheral
   clock or bus state must not be disturbed mid-transaction by a sleep entry
   is a lock site; an unbracketed one is the failure mode, symmetric with
   `meshcadet-power-dfs`'s own `ESP_PM_APB_FREQ_MAX` lock discipline.
4. **Octal PSRAM retention — an unresolved hazard, not a solved problem.**
   `CONFIG_SPIRAM_MODE_OCT=y` (`firmware/sdkconfig.defaults:99`) backs the UI
   framebuffers and the rotating conversation history. Whether this
   project's pinned ESP-IDF version retains octal-PSRAM contents correctly
   across a light-sleep cycle is not established anywhere in this repo and
   must be verified against that IDF version's own release notes/errata
   before this contract is executed — this is a **precondition** to
   attempting light sleep at all, not a detail to discover mid-implementation.
5. **Deep sleep is permanently out — not merely deferred.** GPIO45 lies
   outside the ESP32-S3's RTC-GPIO range (GPIO0–21), so it cannot serve as an
   `EXT0`/`EXT1` deep-sleep wake source; DIO1 is therefore unusable as a wake
   signal the instant the SoC enters deep sleep. Deep sleep also destroys
   volatile SPI2/UART peripheral state on wake, requiring a full radio
   re-init that is structurally incompatible with **P4**'s continuous-RX
   invariant (`radio.rs:604`'s `SetRx 0xFFFFFF`, armed once). This is a
   structural exclusion tied to this board's pin assignment, not an
   implementation gap a future campaign could close — see D5.

**Expected incremental win over DFS alone: order 5–15 mA `[ESTIMATE —
datasheet-order, not measured]`.** Real, but smaller than the GPS leg
(`meshcadet-power-gps-standby`, order ~20 mA) and gated entirely behind
clause 2's unfalsifiable-without-hardware correctness argument — which is
why this campaign stages DFS first and does not attempt light sleep at all.

### D5 — The deliberately-not-attempted register, with magnitudes

Every item this campaign considered and did not implement, ranked by
estimated magnitude where one exists. Every number below carries its D2 tag.
This is the complete register as scoped when this ADR was authored;
`meshcadet-power-gps-standby` adds a u-blox row here if it takes its
documented abort, and `meshcadet-power-idle-screen` adds rows here if its
abort reshapes the DFS leg's scope.

| # | Item | Magnitude | Disposition |
|---|---|---|---|
| 1 | **Light sleep** (full contract, D4) | ~5–15 mA `[ESTIMATE]` incremental over DFS | Specified fully, not implemented. Permanently gated on a hardware validation path for D4 clause 2 and clause 4. Follow-on: a dedicated light-sleep campaign, gated on a hardware session. |
| 2 | **Deep sleep** | No magnitude estimated — structurally excluded, not a cost/benefit call | Permanently out. GPIO45 is not an RTC GPIO (D4 clause 5). No future campaign should re-open this without a board revision. |
| 3 | **ESP32-C3 keyboard co-processor sleep management** | 5–20 mA `[ESTIMATE — order-of-magnitude, wide band, unverified]` | Out of scope for this campaign — **not because it is small.** Ranked by magnitude it would sit **second**, ahead of backlight brightness and DFS, plausibly comparable to the idle-screen and DFS wins combined. Excluded because it requires flashing a *separate* MCU's firmware — a different kind of problem than anything else in this campaign. **Named follow-on:** a recon investigation into whether the T-Deck's C3 keyboard firmware exposes any sleep/idle command over the existing I²C interface (the backlight write at `firmware/src/ui/keyboard.rs:190` already proves the interface is host-writable, so the question is live). To be queued independently of this campaign; no dependency on any leg here. |
| 4 | **Radio duty-cycling / RX windowing** | 4.6–5.5 mA `[DATASHEET]` (SX1262 continuous-RX current) | Excluded by **P4** directly — no dropped or missed RX. The magnitude confirms the exclusion is cheap: genuinely small next to the GPS receiver's ~20–30 mA and the backlight's 40–100 mA. Confirmed correct as built (`radio.rs:604` arms true continuous RX once; CAD only pre-TX); left alone. |
| 5 | **Dispatcher loop cadence (`RX_POLL_YIELD_MS`)** | <1 mA `[ESTIMATE]` | Excluded. The dispatcher loop already blocks on a DIO1 interrupt/notification wait rather than spinning (`main.rs:1842`, `RX_POLL_YIELD_MS = 20`; see `meshcadet-perf-radio-dio1-interrupt`), so an idle iteration costs one blocking wait, not a poll storm. The remaining win is not worth a separate leg against a documented, deliberate RX-notice-latency tuning decision that touches **P4**. If `meshcadet-power-dfs` finds DFS starved specifically by this task's wake rate, an adaptive cadence folds into that leg's own scope with `perf_loop_model` re-validation — it does not open a new leg. |
| 6 | **Building a power-measurement kit, or a device-measurement procedure** | Not applicable — a procedural exclusion, not a power lever | Ruled out by the maintainer directly (see Context). Nothing in this campaign re-litigates it. This is the premise D2's estimate-labelling rule exists to serve, not an item competing with the others on magnitude. |
| 7 | **Lowering the shipped `screen_sleep_timeout_s` default (30 s)** | Not estimated — a product decision, not a power lever this campaign is scoped to evaluate | Out of scope. The maintainer did not ask for this behavior change. `meshcadet-power-backlight-brightness` makes brightness *settable*; changing the sleep-timeout default is a separate product decision, deliberately not conflated with it. |
| 8 | **Host-native execution** | Not applicable | No leg of this campaign targets a host-native relaunch; all device-side validation stays with the maintainer, on real hardware. |

**A sub-decision that is *not* on this register:** the GPS leg's choice of
whether `$PCAS04,7` (GPS + GLONASS + BeiDou, `firmware-core/src/gps.rs:171`)
should drop to two constellations. That decision belongs to
`meshcadet-power-gps-standby` — it is attempted-and-decided within that
leg's own scope, not deliberately not attempted, and the recommendation
carried into that leg (leave it at 3; fix quality and reacquisition
robustness are **P1**'s substance, and the second-order constellation
saving is the one change here that could degrade P1 in a way no host test
can catch) is that leg's to record in its own artifact, per D1.

### D6 — One deferred-predicate register for the whole campaign; this ADR does not start a second

Any predicate in this campaign that can only be closed on real hardware
belongs in `docs/perf/ui-perf-baseline.md` §9 — the project's existing,
single consolidated deferred-predicate register — **not** in a new register
here or anywhere else. This ADR's not-attempted register (D5) records *what
was decided and why*, with a magnitude; it is not a hardware-closure
procedure list and does not compete with §9. Concretely:

- The device-only predicates this campaign's landing review
  (`meshcadet-power-acceptance`) identifies — observed battery-life
  improvement, GPS reacquisition latency/fix quality after a real
  standby/wake cycle on both shield variants, RX/TX integrity across a full
  idle→wake cycle with DFS enabled, and perceived UI responsiveness/wake-
  from-sleep feel — are each written into §9 by that leg, not by this ADR.
- The light-sleep contract's own hardware-only closure conditions (D4
  clauses 2 and 4) become §9 entries only when a future campaign actually
  stages an attempt at implementing them — this ADR specifies the contract;
  it does not pre-register predicates for work nobody has scheduled yet.
- If `meshcadet-power-gps-standby` takes its documented u-blox abort, the
  resulting u-blox-standby deferred predicate is a §9 entry, added by that
  leg itself as part of its abort's reshape pass — not duplicated here.

## Consequences

- Every leg of this campaign now has one place to look for the shared
  vocabulary (D2), the shared acceptance shape (D3), and the shared
  constraint names (D1) — the failure mode this ADR exists to prevent
  (several implementers inventing several interpretations) requires each
  leg to cite this ADR by section, not restate its own version.
- The light-sleep contract (D4) exists in the repo as a fully specified,
  zero-code decision. A future campaign can execute it without re-deriving
  the DIO1/PSRAM/lock analysis from scratch, but it inherits none of the
  ownership assignment automatically — a specification is not an
  assignment, so that future campaign must still name an explicit owning
  child mission for each clause.
- The not-attempted register (D5) is the authoritative record of what this
  campaign chose not to do and why; a later effort that wants to revisit any
  of these eight items should update this table in place rather than
  re-litigate the decision in a new document.
- `docs/perf/ui-perf-baseline.md` §9 remains the single hardware-deferred-
  predicate register project-wide; this campaign adds to it (via its landing
  review and any leg that aborts) rather than forking it.

## Alternatives Considered

Each decision's rejected alternative is recorded inline under its own
section above, per this project's usual ADR style (see ADR-0007 §2, ADR-0013
§ Alternatives Considered):

- **D2 — a single `[ESTIMATE]` tag for everything, no `[DATASHEET]`/
  `[MEASURED]` distinction.** Rejected: collapsing "read off a datasheet" and
  "reasoned projection from a duty cycle" into one tag would have hidden
  exactly the distinction a future reader most needs — whether a number is
  exact-but-not-device-specific or genuinely a projection. The three-tag
  vocabulary costs nothing to maintain and is already how
  `docs/perf/ui-perf-baseline.md` §0 reasons about timing numbers; this ADR
  keeps that pattern's *shape* for power numbers without pretending they are
  the same kind of number.
- **D4 — silently dropping the light-sleep contract instead of specifying
  it.** Rejected: leaving the DIO1/PSRAM/lock analysis unrecorded even
  though nothing implements it this campaign would force a future campaign
  to start from zero. A contract that exists only in someone's memory is not
  discoverable; one in an ADR is.
- **D6 — a dedicated power-predicate register in this ADR, mirroring §9's
  shape locally.** Rejected: a second register is a second place to go
  stale, and §9 already has a working promotion path (§9.6: move the closed
  predicate's number into the document body with a `[DEVICE]` tag, strike
  the row) that a second register would either duplicate or drift from.

## Out of scope

- Implementing light sleep — see D4; specified, not implemented.
- Implementing deep sleep in any form — permanently excluded, D4 clause 5.
- Any change to the ESP32-C3 keyboard co-processor's firmware — D5 row 3.
- Radio duty-cycling / RX windowing — D5 row 4.
- Any change to `RX_POLL_YIELD_MS` — D5 row 5 (folds into
  `meshcadet-power-dfs` if DFS reveals starvation, does not open a separate
  leg).
- Building a power-measurement kit or device-measurement procedure — D5
  row 6, ruled out by the maintainer.
- Lowering the shipped `screen_sleep_timeout_s` default — D5 row 7.
- Host-native execution for any leg — D5 row 8.
- Any firmware code change of any kind. This ADR is docs-only.
