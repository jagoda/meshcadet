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

**Mechanized, not just stated.** This rule broke inside the very document
that authored it (D5 row 4 originally shipped two bare, untagged power
figures at this ADR's own landing) — proof that a prose-only rule lapses
under review pressure exactly like any other. `xtask verify-power-provenance`
(part of the default `cargo run -p xtask --bin xtask` battery, and also a
`cargo test`) runs `xtask::power_provenance::check`, which scans every
`.md` file under `docs/` for a power-current figure (`mA`/`µA`/`uA`) and
fails loudly if one has no `[DATASHEET]`/`[ESTIMATE]`/`[MEASURED]` tag
nearby. It checks tag
*presence*, not reasoning-basis *quality* — whether an `[ESTIMATE]`'s
bracket actually argues its number, versus just wearing the tag, stays a
review-time judgment (see the module's own doc comment for why that half is
deliberately not automated).

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
   is a lock site; an unbracketed one is the failure mode. **Record
   correction (`meshcadet-power-record-corrections`): this clause is NOT, in
   fact, symmetric with `meshcadet-power-dfs`'s own landed `ESP_PM_APB_FREQ_
   MAX` lock discipline** — D8 records that DFS brackets only the radio's two
   SPI2 funnel points (`write_cmd`/`spi_transfer`) and the GPS UART ACTIVE
   window; the ST7789 display controller (also SPI2), the GT911/keyboard I2C
   bus, and the LEDC backlight timer are deliberately unbracketed there (see
   D8's own amendment). A future leg implementing THIS clause must satisfy
   "both the radio's and the display controller's" itself, in full — it
   cannot lean on DFS's discipline as a precedent for display coverage,
   because DFS never established one.
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
datasheet-order, not measured: light sleep additionally stops core
execution and gates peripheral clocks entirely, dropping the SoC's own idle
draw from DFS's residual 40–80 MHz active-idle floor to its documented
light-sleep quiescent current — order hundreds of µA, near-negligible
against that floor. Clause 3's lock discipline keeps SPI2 and the GPS
UART's clock domain held live across their bracketed windows, so light
sleep cannot claim the full floor-to-near-zero delta; that residual
bus-clock overhead is what narrows the naive near-total reduction down to
the order 5–15 mA quoted here]`.** Real, but smaller than the GPS leg's
*foregone* estimate (`meshcadet-power-gps-standby`, order ~20 mA
`[ESTIMATE]` — not landed for either variant, see D5 row 9) and gated
entirely behind clause 2's unfalsifiable-without-hardware correctness
argument — which is why this campaign stages DFS first and does not attempt
light sleep at all.

### D5 — The deliberately-not-attempted register, with magnitudes

Every item this campaign considered and did not implement, ranked by
estimated magnitude where one exists. Every number below carries its D2 tag.
This is the complete register as scoped when this ADR was authored, extended
by row 9 below (`meshcadet-power-gps-standby` landed a **broader** abort than
this ADR anticipated — not merely a u-blox row, but the entire GPS
RF-front-end standby lever, both variants; see row 9's own text), and
`meshcadet-power-idle-screen` adds rows here if its abort reshapes the DFS
leg's scope.

| # | Item | Magnitude | Disposition |
|---|---|---|---|
| 1 | **Light sleep** (full contract, D4) | ~5–15 mA `[ESTIMATE — see D4's own inline arithmetic]` incremental over DFS | Specified fully, not implemented. Permanently gated on a hardware validation path for D4 clause 2 and clause 4. Follow-on: a dedicated light-sleep campaign, gated on a hardware session. |
| 2 | **Deep sleep** | No magnitude estimated — structurally excluded, not a cost/benefit call | Permanently out. GPIO45 is not an RTC GPIO (D4 clause 5). No future campaign should re-open this without a board revision. |
| 3 | **ESP32-C3 keyboard co-processor sleep management** | 5–20 mA `[ESTIMATE — order-of-magnitude, wide band, unverified]` | Out of scope for this campaign — **not because it is small.** Ranked by magnitude it would sit **second**, ahead of backlight brightness and DFS, plausibly comparable to the idle-screen and DFS wins combined. Excluded because it requires flashing a *separate* MCU's firmware — a different kind of problem than anything else in this campaign. **Named follow-on:** a recon investigation into whether the T-Deck's C3 keyboard firmware exposes any sleep/idle command over the existing I²C interface (the backlight write at `firmware/src/ui/keyboard.rs:190` already proves the interface is host-writable, so the question is live). To be queued independently of this campaign; no dependency on any leg here. |
| 4 | **Radio duty-cycling / RX windowing** | 4.6–5.5 mA `[DATASHEET]` (SX1262 continuous-RX current) | Excluded by **P4** directly — no dropped or missed RX. The magnitude confirms the exclusion is cheap: genuinely small next to the GPS receiver's ~20–30 mA `[ESTIMATE — datasheet-order: continuous acquisition/tracking current for this class of GNSS module, not a duty-weighted average and not measured on this device; row 9 below applies this same figure's duty-cycle weighting]` and the backlight's 40–100 mA `[ESTIMATE — datasheet-order: full-duty backlight boost-converter current for a T-Deck-class panel while lit, not measured on this device; see `meshcadet-power-backlight-brightness`'s own D3.1 estimate]`. Confirmed correct as built (`radio.rs:604` arms true continuous RX once; CAD only pre-TX); left alone. |
| 5 | **Dispatcher loop cadence (`RX_POLL_YIELD_MS`)** | <1 mA `[ESTIMATE]` | Excluded. The dispatcher loop already blocks on a DIO1 interrupt/notification wait rather than spinning (`main.rs:1842`, `RX_POLL_YIELD_MS = 20`; see `meshcadet-perf-radio-dio1-interrupt`), so an idle iteration costs one blocking wait, not a poll storm. The remaining win is not worth a separate leg against a documented, deliberate RX-notice-latency tuning decision that touches **P4**. If `meshcadet-power-dfs` finds DFS starved specifically by this task's wake rate, an adaptive cadence folds into that leg's own scope with `perf_loop_model` re-validation — it does not open a new leg. |
| 6 | **Building a power-measurement kit, or a device-measurement procedure** | Not applicable — a procedural exclusion, not a power lever | Ruled out by the maintainer directly (see Context). Nothing in this campaign re-litigates it. This is the premise D2's estimate-labelling rule exists to serve, not an item competing with the others on magnitude. |
| 7 | **Lowering the shipped `screen_sleep_timeout_s` default (30 s)** | Not estimated — a product decision, not a power lever this campaign is scoped to evaluate | Out of scope. The maintainer did not ask for this behavior change. `meshcadet-power-backlight-brightness` makes brightness *settable*; changing the sleep-timeout default is a separate product decision, deliberately not conflated with it. |
| 8 | **Host-native execution** | Not applicable | No leg of this campaign targets a host-native relaunch; all device-side validation stays with the maintainer, on real hardware. |
| 9 | **GPS RF-front-end low-power standby, both variants** (`meshcadet-power-gps-standby`) | ~20 mA `[ESTIMATE — 80% QUIET-time duty fraction (30 s ACTIVE / 120 s QUIET cycle, `gps.rs:938`–`949`) × the ~20–30 mA continuous acquisition/tracking current above (row 4), against a standby/backup current of order tens of µA — negligible against that delta — nets order ~20 mA of average draw removed]` foregone — this campaign's single largest anticipated lever (the plan's own Phase 2 framing: "the largest single lever in the campaign") | Attempted and aborted for **both** GNSS variants, not only u-blox as this ADR anticipated when authored. **L76K**: its documented command surface (`$PCAS01`–`$PCAS04`/`$PCAS10` NMEA-ASCII, plus the binary CASIC ACK/CFG-PRT/CFG-MSG/CFG-RST/CFG-RATE messages — Quectel `L76K_GNSS_Protocol_Specification` V1.1, 2021-12-16, the full and only documented command set for this exact module) contains **no standby/sleep command of any kind**. Quectel support confirmed this directly on their own forum: *"there is no equivalent of `PMTK_CMD_STANDBY_MODE` in PCAS messages"* — genuine standby requires pulling the module's hardware STANDBY pin low, or a ≥1 s VCC power-cycle while `V_BCKP` stays powered — both physical-pin operations, not UART commands. This board's GPS shield does not wire a STANDBY/EN pin to the ESP32-S3 (`firmware/src/gps.rs`'s own hardware table lists only GPIO43/44, UART TX/RX) — the pin doesn't exist for this firmware to command even if a code path were written. The nearby `$PMTK161,0` "standby" command occasionally cited online belongs to MediaTek MT3333-family modules (e.g. plain L76/L80), a different chipset family than this board's CASIC-based L76K, and does not apply here. **u-blox M10Q**: unchanged from this ADR's original anticipation — power management is `UBX-CFG-PMS`/`UBX-RXM-PMREQ`, binary, not ASCII/NMEA-checksum-verifiable off-hardware. Since **neither** variant has an ASCII/checksum-verifiable, host-testable command, the acceptance template's own host-test requirement (D3.2) cannot be met by any code path for this lever — no firmware or `firmware-core` change accompanies this row, matching D3's leg-may-decide-not-every-leg-earns-a-guard posture (there is no standby path to guard). Follow-on: a hardware-revision campaign wiring the L76K STANDBY pin to a spare GPIO, gated on a bench session (the same hardware-validation-path pattern as D4's light-sleep contract); independently, a u-blox-only leg validating the binary UBX sequence on real u-blox hardware. Deferred predicate: `docs/perf/ui-perf-baseline.md` §9, D13. |

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
- `meshcadet-power-gps-standby` took a broader abort than anticipated here —
  covering both GNSS variants, not only u-blox (D5 row 9) — and the
  resulting deferred predicate is a §9 entry (D13), added by that leg itself
  as part of its abort's reshape pass, not duplicated here.

### D7 — Idle-screen leg (`meshcadet-power-idle-screen`): landed estimate and the honest wake-latency re-derivation

**M2-gate correction (`meshcadet-power-m2-gate-20260823-223120079`,
`meshcadet-power-asleep-tick-touch-wake-loss-20260825-000816136`):**
`ASLEEP_IDLE_TICK_MS` shipped at 120ms below, but the M2 gate found that
value silently drops screen-tap wakes — the GT911 does not queue events, so
a tap whose entire press-then-release cycle completes inside one 120ms poll
gap is lost outright (not delayed), at ~17% of 100ms taps. It is now 50ms
(`firmware_core::ui::idle_tick::ASLEEP_IDLE_TICK_MS`), bound above by
`firmware_core::ui::touch::GT911_MIN_RELIABLE_TAP_MS` and pinned by that
module's own host test. The wake-latency and I²C-poll-rate figures below
are re-derived against 50ms, not the original 120ms; the D7 narrative below
is otherwise left as landed (no other part of the idle-screen leg changed).

Landed as specified in the plan of record — no abort taken: `SLPIN`/`DISPOFF`
on sleep and `SLPOUT`/`DISPON` + one forced full repaint on wake
(`firmware/src/ui/display.rs`'s `TDeckDisplay::sleep`/`wake`), `render_if_needed`
skipped entirely while asleep (`firmware_core::ui::idle_tick::render_gate`),
an adaptive asleep tick (`firmware_core::ui::idle_tick::next_tick_period_ms`),
and a dim-before-sleep step on top of Phase 4's `set_brightness`
(`firmware_core::ui::idle_tick::screen_idle_action`/`dim_brightness_pct`).

**Expected win.** Order a few mA of average idle draw `[ESTIMATE —
datasheet-order, per the plan of record (Phase 5): removing ~42 I²C
transaction pairs/s (GT911 touch poll + keyboard co-processor poll — the
asleep-idle tick cuts this poll rate from ~62.5 Hz awake to 20 Hz
asleep-idle, post-M2-gate-correction; it does not eliminate the poll
entirely, contrary to an earlier draft of this estimate that read
"eliminating ~62 I²C transaction pairs/s" — the asleep-idle tick still
polls, only 42.5/s slower, matching
`firmware_core::ui::idle_tick::ASLEEP_IDLE_TICK_MS`'s own doc) plus periodic
full-region SPI flushes to a dark panel, and putting the ST7789 in
sleep-in, is a small, second-order term next to the GPS (row 9) and
backlight (row 4) levers. This "few mA" conclusion is unaffected by the
pairs/s correction above — this leg's LARGER value was originally argued as
structural rather than this milliwatt-class number: an unconditional 62 Hz
tick defeats tickless idle outright, so `meshcadet-power-dfs` (Phase 7)
cannot deliver anything without this leg landing first. **RETRACTED by D8's
own landing:** `meshcadet-power-dfs` deliberately does NOT enable
`CONFIG_FREERTOS_USE_TICKLESS_IDLE` — that Kconfig option exists solely to
support automatic light sleep between ticks, which the DFS leg never
attempts (`light_sleep_enable: false`, D8) — so DFS's win never depended on
this leg's tick slowdown, and this leg landing first was never a
precondition for it. D8 also records the actual limiter on core 0 reaching
its own idle floor: the unchanged 20ms `RX_POLL_YIELD_MS` dispatcher
cadence, not `ui_task`'s tick. This structural framing is amended, not
deleted, the same way the M2-gate correction note above amends this leg's
120ms figure rather than erasing it — this leg's real independent value is
the I²C/render savings this estimate otherwise argues, not a DFS
precondition that never existed]`.

**Honest wake-latency bound — CORRECTS the plan's rough "~150 ms"
aspiration, does not merely restate it.** The ST7789's own `SLPOUT`
settling delay is a mandatory, non-tunable 120 ms (datasheet requirement —
`mipidsi::Display::wake`, wrapped by `TDeckDisplay::wake`), and this leg's
forced full repaint on wake costs ~30.7 ms
(`docs/perf/ui-perf-baseline.md` §4.1, currency 2026-08-03) — together already ~150.7 ms BEFORE
the asleep tick's own poll-latency contribution is even added. Worst-case
touch/keyboard wake-to-first-paint is therefore `ASLEEP_IDLE_TICK_MS`
(50 ms, post-M2-gate-correction — see the note above; 120 ms as originally
landed) + 120 ms + 30.7 ms ≈ 200.7 ms
(`firmware_core::ui::idle_tick::ASLEEP_IDLE_TICK_MS`'s own doc carries the
identical derivation). The plan's aspirational ~150 ms bound turns out to
be unreachable once the mandatory `SLPOUT` settling delay it ALSO requires
is accounted for — that fixed floor alone already sits at the aspiration,
before this leg's own tunable tick period is even added. This is the
constraint-P2 finding Milestone 2 (`meshcadet-power-m2-gate`) must
re-derive and record as a number, not silently pass — the M2 gate then
found `ASLEEP_IDLE_TICK_MS` itself was over-wide against a THIRD constraint
(GT911 tap-loss, see the note above), correcting 120ms to 50ms and this
figure from ~270.7ms to ~200.7ms in turn.

**Constraint P3 (notifications) — a correctness bound, not a nicety.** The
incoming-message blink is RENDERED, not merely fired, at the tick period
(`sync_keyboard_backlight`'s `notif.poll_blink` call). The asleep tick only
slows to `ASLEEP_IDLE_TICK_MS` while no blink burst is live; a live burst
forces the tick back to `ASLEEP_BLINK_TICK_MS` (50 ms), inside the Nyquist
bound against `BLINK_PHASE_MS` (150 ms, `firmware_core::notification`) —
`next_tick_period_ms`'s own host tests re-derive this bound directly against
the real constant rather than a hardcoded literal.

**Software-observable proxy (`[SIM]`, not `[MEASURED]` — no device/HIL/QEMU
path exists for `perf_loop_model`).** Re-running the existing
`split_ui_idle_tick` parameter at the Phase 5 asleep-idle cadence
(`perf_loop_model::report::asleep_tick_comparison`, `Corner::High`):
`ui_task`'s own service rate drops from 21.31 Hz (awake, `UI_TICK_MS` =
16 ms ceiling) to 12.36 Hz (asleep-idle, `ASLEEP_IDLE_TICK_MS` = 50 ms
ceiling, post-M2-gate-correction — see the note above; was 6.63 Hz at the
original 120 ms) — reproduce with `cargo test -p perf_loop_model
asleep_idle_tick_reduces_ui_task_service_rate_vs_awake -- --nocapture`.

### D8 — DFS leg (`meshcadet-power-dfs`): landed as specified, no abort, one sdkconfig correction applied

Landed as amended at the M2 gate (this ADR's own Phase 7 amendment): `CONFIG_PM_ENABLE=y`,
`esp_pm_configure(max_freq_mhz=240, min_freq_mhz=80, light_sleep_enable=false)`
(`firmware/src/pm.rs`), called once at the top of `main.rs::run()` before any peripheral driver
is constructed. `CONFIG_FREERTOS_USE_TICKLESS_IDLE` is **not** enabled — amendment item 1 is
followed exactly: `CONFIG_PM_ENABLE` does not `depend on` it
(`esp_pm/Kconfig`, confirmed by reading the shipped v5.2.2 Kconfig directly), and it exists
solely to let a core enter *automatic light sleep* between ticks, which this leg never does
(`light_sleep_enable: false`). Enabling it would have added cross-compile surface for zero
effect. DFS ONLY, per the brief: no `light_sleep_enable = true` anywhere, and the two stages are
not combined.

**`min_freq_mhz = 80`, not the plan's "40–80" range's lower end, and why that is not a hedge.**
ESP32-S3's `rtc_clk_cpu_freq_mhz_to_config` (`esp_hw_support/port/esp32s3/rtc_clk.c`) accepts
80/160/240 unconditionally (PLL-sourced) but accepts a value below the board's XTAL frequency
only if it divides that XTAL exactly — a fact this repo has no on-hardware measurement of for the
T-Deck Plus's specific crystal, and `esp_pm_configure` returns `ESP_ERR_INVALID_ARG` (logged, not
fatal — `pm::configure_dynamic_frequency_scaling`) if it doesn't. 80 MHz sidesteps that
uncertainty entirely (see `firmware/src/pm.rs`'s module doc for the full derivation) while still
landing a 3× idle-frequency reduction.

**The CPU does not sit at 80 MHz whenever idle-eligible in the naive sense — ESP-IDF's own
FreeRTOS port already restores 240 MHz automatically the instant either core has real work.**
`esp_pm_impl_init` (`pm_impl.c`) creates one internal `ESP_PM_CPU_FREQ_MAX` lock per core
(`"rtos0"`/`"rtos1"`), held from boot; it is released only when that core's FreeRTOS idle task
runs (`esp_pm_impl_idle_hook`) and re-acquired the instant the core leaves idle
(`leave_idle()`, called from `esp_pm_impl_isr_hook`/the scheduler) — i.e. whenever an ISR fires or
a task becomes ready. This is the actual mechanism behind the plan's "only the idle floor moves"
claim, not merely an assumption: both cores run at the unchanged 240 MHz ceiling for the entire
span either is doing real work (dispatcher loop iteration, ISR handling, `ui_task` rendering), and
only reach the 80 MHz floor during a core's genuinely-idle stretches (blocked, nothing ready).
Neither this leg nor any earlier one needs to acquire `ESP_PM_CPU_FREQ_MAX` itself for that
property to hold.

**`ESP_PM_APB_FREQ_MAX` locks (`firmware/src/pm.rs::ApbFreqMaxLock`) bracket the radio's SPI2
transactions and the GPS UART's ACTIVE window — ONE of the two SPI2 masters, not both, and this
is deliberate, not an oversight (record correction, `meshcadet-power-record-corrections`; see
below).** `Radio::write_cmd`/`spi_transfer`
(the two funnel points every SX1262 command goes through, `radio.rs`) acquire/release around each
individual `self.spi.write`/`transfer_in_place` call — the same bracket span as the existing D9/D11
diagnostics probe. `GpsDriver` acquires the lock at construction (the driver starts ACTIVE) and at
every QUIET→ACTIVE reopen, releasing at every ACTIVE→QUIET close
(`firmware_core::gps::active_window_pm_lock_action`, a pure, host-tested decision function whose
host tests pin the bracket's symmetry; in a **release** firmware build the `debug_assert_eq!` call
sites in `GpsDriver::poll` that cross-check the live acquire/release calls against this function
compile out entirely — `firmware/Cargo.toml`'s `[profile.release]` does not set
`debug-assertions = true` — so in the shipped binary the bracket's correctness is pinned
structurally only by `xtask::pm_apb_lock_gate`'s static source guard plus this function's own host
tests, not by anything the release binary itself runs; the earlier framing ("decided by the pure,
host-tested function," this leg's own PR body) overstated the release-build mechanism). **Worth
recording plainly: with `min_freq_mhz = 80` specifically, these locks are not currently
load-bearing for P1/P4 by themselves** — `esp_pm`'s own mode derivation
(`pm_impl.c`, non-ESP32 branch) computes `apb_max_freq = MIN(max_freq_mhz, esp_clk_apb_freq())`,
and ESP32-S3's real APB peripheral clock is a fixed 80 MHz tap off the 480 MHz PLL whenever the CPU
runs off PLL (80/160/240) — so with `min_freq_mhz = 80` the actual APB clock is 80 MHz in *every*
reachable PM mode and never itself changes, and the radio's SPI2 transactions are additionally
already covered by the automatic per-core `ESP_PM_CPU_FREQ_MAX` lock above for the common case (a
single blocking SPI transfer keeps its calling task off the idle task throughout). The locks are
still implemented exactly as the plan directs: correct defense-in-depth against any future
`min_freq_mhz` change that WOULD move the APB clock (an XTAL-sourced value below 80), and against
an SPI DMA transfer that internally yields to the scheduler — a case the automatic per-core lock
does not cover, since a core going idle mid-yield inside a nominally-single "transaction" is
exactly the gap this campaign has no measurement path to rule out on this board. Holding them costs
nothing today and removes that gap entirely regardless of any future config change — **for the
radio and GPS UART specifically.**

**Record correction: the ST7789 display controller, the GT911/keyboard I2C bus, and the LEDC
backlight timer are also APB-derived and are deliberately left unbracketed — a partial, not
complete, instance of the lock discipline D4.3 specifies for the (unimplemented) light-sleep
contract.** The ST7789 shares SPI2 with the radio (`firmware/src/main.rs:742`, `:788`, `:853`);
neither `TDeckDisplay::flush_line_range` (the hot render path, up to 240 SPI transactions per full
repaint) nor any GT911/keyboard I2C call site nor the LEDC backlight PWM timer acquires
`apb_lock`. The recommended fix is this record, not the code: `flush_line_range` is the exact hot
path a prior mission (`meshcadet-power-idle-screen`) spent its whole budget de-allocating on, and a
lock bracket around up to 240 ESP-IDF calls per repaint is not a change to make casually against
that budget. Two reasons this exclusion is acceptable as recorded, not merely unexamined: (1) the
same APB-pinned-at-80 argument above applies identically to the display/I2C/LEDC — with
`min_freq_mhz = 80` the APB clock never moves regardless of which peripheral is transacting, so
none of these brackets are load-bearing today either; (2) unlike a corrupted SX1262 command
(P4) or a corrupted GPS UART frame (P1), a corrupted pixel row from an APB glitch mid-transaction
is cosmetic and self-heals on the very next repaint — there is no P1–P4 invariant this omission
threatens. **Consequently, the defense-in-depth this discipline offers against a future
`min_freq_mhz` below 80 is PARTIAL, not complete: it protects the radio and GPS today, and would
need a genuinely new bracket added to `flush_line_range`/the I2C call sites/the LEDC timer before
any future config change actually moved the APB clock, not merely a re-read of this record.**

**HARD ABORT condition checked directly, not merely inferred from CI.** `CONFIG_PM_ENABLE=y`
together with `CONFIG_SPIRAM_MODE_OCT=y` does **not** conflict — confirmed by an actual
`xtensa-esp32s3-espidf` release cross-compile run to completion (cmake configure, full ESP-IDF C
build, Rust link, `esptool elf2image`) in this leg's own sandbox (not CI), after working around two
environment-local gaps unrelated to the sdkconfig combination itself (a missing `libxml2.so.2` for
the bundled `esp-clang`, and `ldproxy` not on `PATH`). No abort taken.

**Fresh, absolute app-image measurement (never a delta, per the brief) —
`cargo run -p xtask --bin xtask -- verify-partition-budget`, run locally against this leg's own
tree:**

```
measured 4,678,800 B (4.46 MiB) vs. baseline 4,641,888 B (4.43 MiB) — drift +0.80%
(threshold ±5.0%). factory partition 6,291,456 B (6.00 MiB); headroom 1,612,656 B (1.54 MiB).
```

Under the ±5% drift threshold — `firmware/app-image-budget-baseline.txt` is not bumped.

**Expected win, restated downward per the M2/R4 amendment (items 2/3) — a duty-cycle-weighted
figure, not a sleep-fraction one, and explicitly not re-measured.** Order **low-single-digit to
low-double-digit mA** of average draw across both cores combined `[ESTIMATE — datasheet-order:
ESP32-S3 active-mode current is broadly linear in core clock for the CPU-bound term — community
measurements against the Espressif datasheet's active-mode range report roughly 33–47 mA at
80 MHz with both cores active vs. a documented range up to ~107 mA at 240 MHz under load
(https://esp32.com/viewtopic.php?t=29964), i.e. a full per-core delta of very roughly 20–40 mA IF
a core ran 100% idle-eligible. The automatic per-core `ESP_PM_CPU_FREQ_MAX` lock (see above) means
that full delta is reachable only during each core's genuinely-idle fraction, not its whole
schedule: core 0's shortest periodic wake is the unchanged 20 ms `RX_POLL_YIELD_MS` dispatcher
cadence (50 Hz — the limiter per M2-amendment item 2, not Phase 5), and core 1's is
`ui_task`'s tick (16 ms awake / 50 ms asleep, post-R4-correction 20 Hz asleep — item 3's
re-derivation). This campaign has no on-device idle-time profiler to state either core's actual
idle fraction within its wake period directly (D2's no-measurement-kit ruling), so this is a wide
band restating the plan's original "10–25 mA" order downward to reflect a duty-cycle-weighted
mechanism, not a re-measurement of it]`.

**Acceptance evidence.** Host tests: `firmware-core/src/gps.rs`'s
`pm_lock_acquires_on_window_open`/`pm_lock_releases_on_window_close`/
`pm_lock_no_action_when_state_holds`/`pm_lock_bracket_is_symmetric_across_a_transition_sequence`
pin the GPS-side bracket's symmetry. An `xtask` static guard
(`xtask::pm_apb_lock_gate`) asserts, over the raw source, that every SPI2 transaction funnel
(`write_cmd`/`spi_transfer`) and both GPS ACTIVE-window transition sites acquire/release the lock
in the correct order — the SPI/UART critical-section guard D3 calls for. Workspace
test/clippy/fmt green; cross-compile verified directly (above), not merely delegated to CI.

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
