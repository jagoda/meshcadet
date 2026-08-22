// SPDX-License-Identifier: GPL-3.0-only
//! Battery status driver for the LilyGo T-Deck Plus — ADC voltage-divider read.
//!
//! # Hardware (feasibility check)
//!
//! The T-Deck Plus reports battery state through a **plain ADC voltage
//! divider**, not a fuel-gauge IC. There is no AXP192/AXP2101 power-management
//! chip on this board (unlike the T-Beam family, which has one — see
//! `meshcore-dev/MeshCore`'s `TBeamBoard.h`, `I2C_PMU_ADD 0x34`). Confirmed
//! against two independent upstream sources:
//! - LilyGo's own `examples/UnitTest/utilities.h`: `#define BOARD_BAT_ADC 4`.
//! - `meshcore-dev/MeshCore`'s `variants/lilygo_tdeck/TDeckBoard.h`:
//!   `#define PIN_VBAT_READ 4` / `#define ADC_MULTIPLIER (2.0f * 3.3f * 1000)`
//!   read via `analogRead(PIN_VBAT_READ)` at 12-bit resolution — i.e. exactly
//!   the plain-ADC path this module implements, not a PMU register read.
//!
//! | Signal | GPIO | Notes |
//! |--------|------|-------|
//! | Battery ADC | 4 | `BOARD_BAT_ADC` / `PIN_VBAT_READ` — ADC1 channel 3 on ESP32-S3 |
//!
//! GPIO4 is not claimed by any other peripheral in this firmware — SPI2
//! (40/41/38/12/9), I2C1 (18/8), UART1 (43/44), and the discrete GPIOs used
//! for reset/backlight/interrupts (see `docs/adr/0003-ui-toolkit.md`'s pin
//! table and `gps.rs`'s GPIO43/44 note) leave GPIO4 free. No collision.
//!
//! The pack is wired through a 2:1 resistor divider (LilyGo's own
//! `ADC_MULTIPLIER = 2.0 * 3.3 V`), so `Vbat = 2 * Vadc`. Reads are taken at
//! 12-bit resolution with 12 dB attenuation (~0–3.1 V ADC input range on the
//! S3) and averaged over [`BATTERY_SAMPLES`] samples to damp ADC noise,
//! mirroring the reference `getBattMilliVolts()` implementation.
//!
//! # No charge-status pin — "charging" is INFERRED, not read directly
//!
//! Unlike a fuel-gauge IC, a plain voltage divider carries no charge/discharge
//! signal, and — per a HIL bug report (2026-07-05,
//! charge-inflation) — there is also **no reachable external-power-present
//! signal** on this board to substitute for one. This was checked, not
//! assumed: LilyGo's own `examples/UnitTest/utilities.h` and
//! `meshcore-dev/MeshCore`'s `TDeckBoard.h` / `ESP32Board.h` (the same
//! upstream sources that established the ADC-only hardware fact above) define
//! no VBUS-detect, charge-status, or PMU pin for the T-Deck Plus anywhere —
//! only `BOARD_BAT_ADC`/`PIN_VBAT_READ` on GPIO4. The USB-Serial-JTAG
//! peripheral used for the host CLI is a *data*-presence signal (is a USB
//! host enumerated on the console endpoint), not a *power*-presence signal
//! (VBUS can be present — and charging can be happening — with no host
//! attached to the console at all, e.g. a wall-charger brick); it was
//! considered and rejected as a charging-status substitute for that reason.
//! So charging must be inferred from the ADC voltage alone, and the fix below
//! is a voltage-domain mitigation, not a new hardware signal.
//!
//! **2026-07-05 update:** a follow-on HIL capture (below) found that the ADC
//! voltage, inferred alone, still carries a reliable in-band proxy for
//! "external power present" — a raw reading physically impossible for a
//! battery. It is a proxy inferred from the same GPIO4 divider, not a new
//! pin, so the hardware fact above (no dedicated VBUS-detect pin) still
//! holds.
//!
//! ## The bug this module works around
//!
//! The pack's *terminal* voltage while on external power is elevated toward
//! the charger's ~4.2 V CC/CV setpoint well above the *open-circuit* voltage
//! the same true state of charge would show at rest — so a plain
//! voltage→percent map reads ~100% while actually charging, even when the
//! true SoC is much lower (confirmed HIL: reads 100% while charging, collapses
//! to the true ~36% the instant external power is removed and the pack
//! settles).
//!
//! An initial fix inferred "charging" from a *rise* in the live ADC voltage
//! above a resting baseline (latched with hysteresis so a later charger
//! float/CV plateau — where the voltage stops climbing but power stays
//! connected — didn't fall back to "not charging"). A follow-on HIL capture
//! (2026-07-05) then handed us a number the rise heuristic
//! could not use: **raw 4888 mV while plugged in** — a physical impossibility
//! for a single-cell pack (max ~4200 mV at full charge). That number proves
//! the ADC divider node reads the USB/charge rail directly whenever external
//! power is connected, not merely an IR-drop-elevated battery terminal. It
//! also explains why the rise trigger sometimes failed to engage even though
//! it looked correct on paper: it fires by comparing the live reading against
//! a **prior** resting-baseline sample, so on any poll where no such
//! below-ceiling prior sample exists yet (e.g. the very first sample taken
//! while already on power, with nothing to rise *from*), no delta is ever
//! seen, and the contaminated raw voltage leaks straight through as the
//! reported percent — exactly the observed "100% / not charging" report.
//!
//! ## Fix: freeze the percent basis, detect power via an impossible-voltage threshold
//!
//! [`BatteryDriver`] keeps a `settled_mv` value — the percent basis — that is
//! kept in lock-step with the live ADC reading **only while the live reading
//! is at or below [`EXTERNAL_POWER_MV_THRESHOLD`]**. [`BatteryStatus::percent`]
//! always derives from `settled_mv`, never the raw live voltage, so a
//! charge-rail-contaminated read never surfaces there. The moment a poll's
//! live voltage exceeds that threshold, external power is inferred present:
//! `settled_mv` freezes at whatever it last held (the last known good,
//! off-power SoC) and `charging` reports `true` (the fix
//! direction: hold the last valid unplugged SoC rather than the contaminated
//! live read). Unlike the superseded rise trigger, this is a **stateless,
//! per-poll** check against a fixed, physically-grounded ceiling — it needs
//! no delta from a prior sample, so it engages correctly on the very first
//! poll above the ceiling, holds unconditionally through a charger's
//! float/CV plateau (there is no "stopped rising" moment for a threshold
//! check to be confused by), and clears the instant the live voltage falls
//! back at/under the threshold (an actual unplug) — at which point
//! `settled_mv` resyncs to that fresh post-unplug reading, so any real
//! capacity gained during the charge session is picked up rather than the
//! basis staying frozen forever. See [`battery_poll_step`] for the exact,
//! host-tested state transition, and its test module for a full
//! plug/plateau/unplug regression matching the HIL report.
//!
//! This remains an honest best-effort heuristic, not fuel-gauge-grade truth:
//! a device that *boots* already attached to a charger has no prior off-power
//! sample to freeze `settled_mv` at, so `percent` will still show that first
//! contaminated reading until the pack is next seen at/under
//! [`EXTERNAL_POWER_MV_THRESHOLD`] (i.e. unplugged once). `charging`, however,
//! is now correctly reported `true` immediately in that case too, since the
//! threshold check needs no prior sample — a strict improvement over the
//! superseded rise trigger, which reported `false` in exactly that case. The
//! residual `percent` gap is a direct consequence of "no power-present signal
//! exists on this board other than the contaminated rail itself" above; the
//! "(A)" section below closes most of it (persisting a last-known-good SoC
//! across reboots) for a device with prior NVS history.
//!
//! **`meshcadet-battery-level-reads-full-when-depleted` (2026-08-17)
//! follow-on — the gap was worse than this section originally described:**
//! a VIRGIN device (no persisted `settled_mv` yet, so this exact residual
//! gap applies) that boots already plugged in over a truly depleted pack
//! did eventually see `percent` catch up once the pack was confirmed off
//! power — but only by crawling down through `slew_limit_percent`'s
//! (removed 2026-08-22 — see module docs' "Three-state voltage-domain
//! bucket" section) discharge-monotonic cap (2 percentage points per
//! ~[`PEAK_WINDOW_MS`] window), the same limiter that smoothed *legitimate*
//! discharge noise once a basis is trustworthy. Applied to the FIRST
//! correction of an unconfirmed, contaminated boot-time guess, that same cap
//! meant up to ~25 minutes reading a near-full `percent`/`level` on a device
//! that was, the whole time, sitting at its true depleted charge — for
//! this board's plain-ADC design, effectively invisible to the user as
//! "the indicator reads full when depleted, and doesn't change whether the
//! cable is attached or not." The original (2026-08-17) fix special-cased
//! the slew limiter to skip the poll that first proves the basis confirmed.
//! `meshcadet-battery-three-state-pipeline` (2026-08-22) superseded that
//! special case by deleting the slew limiter outright —
//! [`battery_window_close_step`] now always snaps `settled_mv`/`level`
//! straight to the truth on every window close, confirmed or not — see
//! module docs' "Three-state voltage-domain bucket" section for the full
//! rationale.
//!
//! ## ADC calibration (2026-07-05 redirect) and the diagnostic `raw_mv` field
//!
//! The channel was originally opened with `AdcChannelConfig { attenuation:
//! DB_12, ..Default::default() }` — `..Default::default()` leaves
//! `calibration: Calibration::None` (esp-idf-hal 0.46's default), so
//! `AdcChannelDriver::read()` was converting raw counts to millivolts with
//! the *uncalibrated* piecewise-linear attenuation-curve table
//! (`DirectConverter`), not the ESP32-S3's factory eFuse curve-fitting
//! calibration (`esp_adc_cal`/`esp_adc`'s `Calibration::Curve` scheme, which
//! the S3 supports). The uncalibrated table is known to read low in the
//! upper part of the ADC's range — exactly where a near-full pack (~4.2 V,
//! ~2.1 V post 2:1 divider) sits — which was the prime suspect for a HIL
//! report of the gauge reading ~36% while the charge-complete LED indicated a
//! full pack. `Calibration::Curve` is now requested below so `read()` returns
//! the factory-curve-fit millivolts instead.
//!
//! To let that be verified with data instead of inferred, [`BatteryStatus`]
//! now carries a third field, `raw_mv`: the last live (post-divider, still
//! averaged over [`BATTERY_SAMPLES`]) ADC millivolt reading, updated on every
//! poll regardless of the charging latch above — i.e. it is NOT frozen at
//! `settled_mv` while charging, unlike `percent`. This started as a
//! deliberate, temporary relaxation of the 2026-07-03 "expose only
//! percent+charging" scoping, for diagnosis only: `raw_mv` was wired into
//! the host CLI's `status` command (`protocol::provisioning::
//! RspStatusPayload`) and this module's own init/poll log lines, but not
//! the on-device UI.
//!
//! **`meshcadet-battery-glanceable-indicator` (2026-08-04) update:** the
//! on-device admin-menu row (`ui/admin_menu.rs::format_battery_display`) now
//! renders `raw_mv` too, alongside `percent` — the diagnosis-only scoping
//! above is accordingly no longer accurate for that one consumer (the row's
//! `format!`+Slint-push is delta-gated on `raw_mv`, not exact-equality
//! gated, so this doesn't reintroduce the allocation churn the row's own
//! dedup guard exists to prevent — see `battery_display_fields_changed`'s
//! doc). `held_raw_mv` (below) remains diagnostic-only, read only by the
//! host CLI.
//!
//! **`meshcadet-telemetry-raw-mv-over-air` (2026-08-22) update:** the
//! over-the-air telemetry RESPONSE (`main.rs::build_telemetry_response`) now
//! reads `raw_mv` too, as a Cayenne `LPP_GENERIC_SENSOR` entry appended after
//! the existing percent/charging pair (never inserted before it, so an older
//! peer's decoder still recovers percent/charging on an unrecognised entry
//! type). `held_raw_mv`/`level` still do not reach the air. Because `raw_mv`
//! is not frozen by the charging latch, a contact's decoded reading can show
//! `raw_mv` well above what `percent`/`charging` alone implies while charging
//! — see `build_telemetry_response`'s own doc for that divergence spelled out
//! at the call site.
//!
//! ### Reconciliation with the charge-inflation "hold last unplugged SoC" fix
//!
//! The fix that landed [`battery_poll_step`]'s freeze/latch logic (just
//! above) and this ADC-calibration fix address two **different, independent**
//! mechanisms, not one bug seen from two angles:
//!
//! - **Calibration** (this section): a fixed measurement error in
//!   raw-counts→mV conversion, present at every sample regardless of charging
//!   state. Fixing it shifts every reading (charging or not) toward the true
//!   voltage.
//! - **Charge inflation** (`battery_poll_step`): even with perfectly
//!   calibrated mV, a pack's *terminal* voltage while a charge current is
//!   flowing sits above its *open-circuit* voltage for the same true state of
//!   charge (internal-resistance IR drop + the charger's own CC/CV
//!   regulation) — a real electrical effect, not a measurement artifact.
//!
//! So the freeze/latch logic is not superseded by the calibration fix and is
//! kept as-is: calibration corrects *what the ADC reports for a given pin
//! voltage*; the freeze/latch logic corrects *for the pin voltage itself
//! being elevated by charging*. Both can be true simultaneously (as the HIL
//! report's 36%-while-LED-off symptom suggests: an under-read pack that is
//! also genuinely below 100%). A raw-mV HIL capture (the
//! acceptance criterion for this fix) is what distinguishes, WITH DATA, how much
//! of the ~3624 mV reading was calibration error vs. genuine partial charge.
//! If that capture
//! shows the calibration fix alone now reports ~4200 mV on a charge-LED-off
//! pack, no further change to the freeze/latch logic is needed; if a
//! meaningful gap remains, that is new evidence for a follow-on fix, not
//! a reason to have preemptively removed working charge-inflation logic here.
//!
//! **Follow-on outcome (2026-07-05):**
//! the capture landed exactly the "meaningful gap" scenario flagged above, and
//! then some — unplugged, the calibration fix alone brought the reading to a
//! plausible ~4038 mV/82%; plugged in, `raw_mv` read 4888 mV, *above* the
//! physical single-cell ceiling entirely, meaning the divider is reading the
//! charge rail, not an IR-elevated battery terminal. The freeze/latch
//! *concept* was not superseded, but its *trigger* was: the rise-based
//! comparison was replaced with the impossible-voltage threshold check
//! described in the "Fix" section above — see that section and
//! [`battery_poll_step`] for the current mechanism.
//!
//! ## `held_raw_mv` — the last-unplugged raw reading, contamination-free (2026-07-05 follow-on)
//!
//! On this board USB carries BOTH the host CLI UART AND charge power, so
//! *any* CLI read is necessarily taken while the charger's contaminated
//! ~4.2-4.9 V rail is on the pin — `raw_mv` (above) can never show a
//! clean battery voltage while a cable is attached to read it.
//! `settled_mv` (the percent basis — see the "Fix" section above) is already
//! exactly that clean reading: it tracks the live voltage only while not
//! charging, and freezes at the last pre-charge value the instant a charge is
//! detected. [`BatteryStatus`] now exposes that basis directly, in
//! millivolts, as `held_raw_mv` — distinct from both `raw_mv` (live, rail-
//! contaminated while charging) and `percent` (the same basis, but lossy-
//! rounded through [`percent_from_millivolts`]). Reading `held_raw_mv` after
//! unplugging and replugging (to re-attach the CLI) surfaces the exact
//! millivolt figure the pack settled to before the charger went on — the
//! instrument needed to confirm or refute the full-scale anchor
//! below with real hardware data.
//!
//! ## Full-scale anchor: resting-voltage curve, not charging voltage (2026-07-05 follow-on)
//!
//! [`percent_from_millivolts`] used to be a straight line from
//! [`BATTERY_EMPTY_MV`] to [`BATTERY_FULL_MV`] (4200 mV) — but 4200 mV is the
//! charger's CC/CV *terminal* voltage, not a voltage a rested pack ever
//! reaches: a rested single-cell Li-ion/LiPo settles to roughly 4.10-4.15 V
//! at true 100% SoC. Anchoring the map at 4200 mV therefore capped every
//! rested-full pack at ~89-94%, structurally — confirmed HIL: ~82% unplugged
//! on a pack the charge-complete LED reported full. `percent_from_millivolts`
//! now interpolates over [`RESTING_SOC_CURVE`], a piecewise open-circuit-
//! voltage → SoC table anchored at [`RESTING_FULL_MV`] (4150 mV, the top of
//! the standard rested-full range) for 100%, keeping [`BATTERY_EMPTY_MV`]
//! (3300 mV) for 0%. The breakpoints approximate the well-known flat-middle /
//! steep-ends shape of a Li-ion discharge curve rather than a single straight
//! line, so mid-range readings track real pack behavior instead of a coarse
//! linear guess. `BATTERY_FULL_MV` is kept as a named constant purely to
//! document the charging terminal voltage referenced elsewhere in these
//! docs — it is no longer read by `percent_from_millivolts`.
//!
//! This is still a default curve, not a per-pack calibration: if a
//! `held_raw_mv` capture at a known-full (charge-LED-off) charge
//! state comes back suspiciously low (e.g. under ~4.0 V), that points at a
//! residual ADC under-read beyond this curve (the calibration fix moved
//! 36%→82% but may not be fully accurate) — the fix for that is a follow-on
//! ADC-calibration effort, not further lowering this anchor to paper over a
//! measurement error.
//!
//! ## Persistence, peak sampling, slew-limit/latch, and coarse buckets (2026-08-04)
//!
//! Four independent additions on top of everything above:
//!
//! ### (A) `settled_mv` persistence — closing the boot-while-plugged gap
//!
//! [`BatteryDriver::new`] (firmware side) now restores a persisted
//! `settled_mv` from NVS (reusing `config_store`'s `mc_cfg` provisioning
//! namespace, key `batt_mv` — no new namespace) and feeds it to
//! [`battery_poll_step`] as the boot's PRIOR basis, via [`seed_boot_state`],
//! instead of seeding from the first live sample. A device that boots
//! already on external power now reports the last known GOOD off-power
//! reading (persisted) rather than leaking the raw, charge-rail-contaminated
//! first sample through as `percent` — this is the residual gap the "Fix"
//! section above documented as out of scope; it's in scope now.
//!
//! **Bound-at-entry, not repair-at-rest:**
//! `settled_mv` is only ever written by [`battery_poll_step`], which already
//! applies the one plausibility bound that matters
//! ([`EXTERNAL_POWER_MV_THRESHOLD`]) at the point the raw ADC reading first
//! becomes trusted state. Restoring a persisted value at boot deliberately
//! does NOT re-apply a second, storage-side plausibility check — there is
//! nothing to repair, because nothing that reaches NVS was ever unbounded.
//!
//! **Latched trust flag, not a load-time repair:**
//! the one case where `settled_mv` legitimately holds an untrustworthy value
//! is a VIRGIN device (no persisted value yet) whose very first sample is
//! already on external power — [`battery_poll_step`]'s documented residual
//! gap. [`advance_settled_confirmed`] latches a `confirmed` flag `true`
//! forever the first time a poll observes the pack off external power (or
//! immediately, if a value was ever successfully restored from NVS — nothing
//! reaches flash without having passed this same latch once already).
//! [`should_persist_settled_mv`] refuses to persist while `confirmed ==
//! false`, so the one poisoned-basis case above can never seed the NEXT
//! boot's restore.
//!
//! **Bounded write-wear, quantified** (mirrors the write-frequency budgeting
//! used for a comparable NVS-backed watermark elsewhere in this firmware):
//! [`should_persist_settled_mv`] persists the very first confirmed sample
//! immediately (closes the gap on the NEXT reboot as soon as possible), then
//! gates every later write on BOTH a minimum `settled_mv` movement
//! ([`PERSIST_MIN_DELTA_MV`]) since what's on flash AND a minimum elapsed
//! time ([`PERSIST_MIN_INTERVAL_MS`]) since the last write — bounding worst-
//! case writes to at most one per [`PERSIST_MIN_INTERVAL_MS`] (12/hour at the
//! tuned constants), the same order of magnitude budgeted for a comparable
//! NVS-backed watermark elsewhere in this firmware. The delta gate also
//! bounds the OTHER direction — how stale a restored value can be relative
//! to true `settled_mv` at any instant — to at most `PERSIST_MIN_DELTA_MV`
//! worth of drift plus one poll's additional movement, never an unbounded
//! write-behind window.
//!
//! ### (B) Peak-over-window sampling — LoRa-TX/backlight rail sags stop dragging the reading down
//!
//! [`PeakWindowSampler`] replaces feeding each 2 s poll's ADC-averaged
//! reading straight into [`battery_poll_step`]. Instead, every poll's sample
//! updates a rolling peak; only once [`PEAK_WINDOW_MS`] (~30 s) has elapsed
//! does the window's peak get fed to `battery_poll_step` (and the next
//! window starts fresh, reseeded with that same sample). A transient rail
//! sag — LoRa TX current draw, backlight PWM duty — is masked by peak-hold
//! rather than leaking through as if it were the pack's steady-state
//! voltage the instant a poll happens to land mid-sag; a REAL, sustained
//! voltage drop (genuine discharge) is still tracked window-over-window,
//! since each new window's peak starts fresh from that window's own
//! samples. `live_mv`/`raw_mv` (the diagnostic field) is unaffected — it
//! still reflects the freshest 2 s-cadence ADC-averaged read, unfiltered,
//! same as before this change.
//!
//! ### (C) [REMOVED 2026-08-22] Slew-limited, discharge-monotonic `percent`
//!
//! This section used to describe `slew_limit_percent`: a max-step-per-update
//! cap plus a discharge-monotonic latch (floor `percent` at its own previous
//! value until charging is detected) applied to the DISPLAYED `percent`.
//! `meshcadet-battery-three-state-pipeline` (2026-08-22) deleted it, along
//! with `PERCENT_MAX_SLEW_PER_UPDATE_PCT` and the driver's
//! `displayed_percent` field, because it was directly responsible for a
//! hardware-reported bug: a device charged to 100% and unplugged stayed
//! pinned near its PRE-charge percent forever, because the latch only
//! releases while `charging == true`, and `settled_mv` (the target the
//! display is allowed to rise toward) is itself frozen during the charge
//! session — so the display could rise only while charging, and while
//! charging there was nothing above it to rise to. The only escape was an
//! off-power reboot. See the "Three-state voltage-domain bucket" section
//! below for the fix and the second, independent mechanism (an unprotected
//! boot-time seed) that converged on the same symptom. This is also a
//! second, distinct instance of the general "a rate-limiter must gate on a
//! confirmed prior, not apply uniformly to an unconfirmed one" defect class
//! — not the same instance PR #163 already fixed (that one made the FIRST
//! correction of an unconfirmed seed instant; this one is about every
//! SUBSEQUENT correction while `settled_mv` itself is frozen by charging).
//!
//! ### (D) [REVISED 2026-08-22] Coarse bucket level
//!
//! `BatteryLevel` used to have four percent buckets (`Critical`/`Low`/
//! `Medium`/`High`) computed from the slew-limited `percent`. It is now
//! THREE buckets (`Low`/`Partial`/`Full`) computed directly from
//! `settled_mv` in millivolts — see the "Three-state voltage-domain bucket"
//! section below for the full rationale and mechanism.
//!
//! ## Three-state voltage-domain bucket (2026-08-22)
//!
//! `meshcadet-battery-three-state-pipeline`: the user only needs the
//! on-device indicator to distinguish three charge states (`Low`/`Partial`/
//! `Full`), plus the already-working `Charging`/`Unknown`. Recon for that
//! relaxed requirement found the display could get permanently stuck LOW on
//! a freshly-charged device — the *opposite* symptom from the one PR #163
//! fixed — via **two independent mechanisms that converge on the same wrong
//! output**:
//!
//! 1. **The display could never rise while off external power.** The
//!    (now-deleted) discharge-monotonic latch's documented "closer" was the
//!    `charging` flag, but `settled_mv` (the value the display rises
//!    *toward*) is itself frozen at the pre-charge reading for the whole
//!    charge session (see the "Fix" section above). So the display could
//!    only rise while charging, and while charging there was nothing above
//!    it to rise to — charge 20%→100%, unplug, and the display stayed
//!    pinned at ~20% forever. The only escape was an off-power reboot.
//! 2. **The boot basis was a single un-peak-held sample taken during
//!    inrush.** [`PeakWindowSampler`] exists precisely because
//!    LoRa-TX/backlight rail sag drags readings down mid-window — but
//!    [`BatteryDriver::new`]'s `initial_mv` seed is constructed *from* that
//!    sampler without being protected *by* it: a boot-time sag could seed a
//!    falsely-low basis, and mechanism (1) then made that low reading
//!    permanent for the rest of the power cycle.
//!
//! Both mechanisms lived in the percent-DISPLAY filter layer (the slew
//! limiter, the discharge-monotonic latch, and the percent-domain bucket
//! hysteresis stacked on top of them) — a layer that existed to smooth a
//! precise percent NUMBER. A 3-state display doesn't need a smoothed
//! number; it needs a stable BUCKET, and a bucket ±[`LEVEL_HYSTERESIS_MV`]
//! wide is already immune to the ADC/curve-breakpoint jitter the slew
//! limiter was built to hide. **Deleting the filter layer is both the
//! simplification and the fix**: [`battery_level_bucket`] now buckets
//! directly on `settled_mv` (millivolts), with hysteresis expressed in
//! millivolts, at [`LEVEL_LOW_PARTIAL_MV`] and [`LEVEL_PARTIAL_FULL_MV`].
//! `BatteryStatus::percent` becomes a pure, stateless
//! `percent_from_millivolts(settled_mv)` — the same value the wire, host
//! CLI, and admin row already consumed, minus the filter — and no
//! percent-domain state (no `displayed_percent`) survives in
//! [`BatteryDriver`] at all. With no stateful filter carrying a prior
//! output, there is no untrustworthy prior to protect, and the whole bug
//! class (#163's symptom, this symptom, and the next one in the family)
//! becomes structurally unreachable rather than individually patched.
//!
//! **Boot seed hardening:** [`BatteryDriver::new`]'s `initial_mv` is
//! treated as provisional — it may seed the display so the first screen
//! isn't blank, but the first closed [`PeakWindowSampler`] window
//! unconditionally overwrites `settled_mv` via the normal
//! [`battery_window_close_step`] chain, whether the corrected value is
//! higher or lower, with no rate limit to fight through. The raw boot
//! sample is now also recorded separately as `boot_mv` (see
//! [`BatteryStatus::boot_mv`]) so a HIL capture can see the gap between
//! "what boot read" and "what the first settled window corrected it to" —
//! the discriminator between "inrush sag" and "the pack really is low."
//! The NVS-restore precedence (a persisted confirmed basis beats a live
//! boot sample while on external power — the #163 fix) is unchanged.
//!
//! **What stays:** the peak-hold sampler ((B) above), the
//! impossible-voltage external-power threshold ([`battery_poll_step`]), the
//! `confirmed` latch and NVS `settled_mv` persistence ((A) above) — all of
//! that defends against *physical* contamination (charge-rail voltage, TX
//! sag, a poisoned boot seed), not against ordinary display jitter, and the
//! relaxed 3-state requirement doesn't touch any of it.
//!
//! Every millivolt constant this section introduces
//! ([`LEVEL_LOW_PARTIAL_MV`], [`LEVEL_PARTIAL_FULL_MV`],
//! [`LEVEL_HYSTERESIS_MV`]) ships PROVISIONAL, derived from
//! [`RESTING_SOC_CURVE`] (itself uncalibrated) rather than a fresh
//! measurement — a measured constant sourced from a code comment (or, here,
//! from another uncalibrated derived table) decays the moment the tree
//! underneath it moves, so none of these are final until a real capture
//! reports each one's absolute value. A user-run HIL capture (this build
//! ships the probes, not the capture itself — that needs real device time)
//! is what locks them.

// This module is pure Rust with no ADC/hardware dependency — see
// `firmware::battery` for `BatteryDriver` (the real ADC1 read path), which
// stays in the firmware crate and re-exports the pure helpers/`BatteryStatus`
// below via a `pub use firmware_core::battery::*;` shim. See
// `docs/adr/0005-firmware-core-extraction.md`.
//
// `BATTERY_SAMPLES`/`DIVIDER_RATIO` (the ADC-sampling tunables) and
// `BATTERY_POLL_INTERVAL_MS` stay in `firmware::battery` alongside
// `BatteryDriver` — they only matter to the real ADC read path.

// ── Tunables ──────────────────────────────────────────────────────────────────

/// Empty-pack cutoff in millivolts → 0%. Conservative single-cell Li-ion
/// "empty under light load" figure.
pub const BATTERY_EMPTY_MV: u32 = 3300;

/// Charging terminal (CC/CV setpoint) millivolts. Standard single-cell
/// Li-ion/LiPo full-*charge* voltage — kept as a named constant purely to
/// document that figure for the rest of this module's docs. **Not** the
/// percent gauge's 100% anchor: see [`RESTING_FULL_MV`] and the module docs'
/// "Full-scale anchor" section for why a rested pack never reaches this
/// voltage and anchoring here structurally under-reads a full battery.
/// `#[allow(dead_code)]`: no longer read by any non-test production code
/// (only by this module's own `#[cfg(test)]` regressions) now that
/// `percent_from_millivolts` anchors on `RESTING_FULL_MV` instead — kept
/// `pub` anyway as reference documentation for the charging-voltage figure.
#[allow(dead_code)]
pub const BATTERY_FULL_MV: u32 = 4200;

/// Rested (open-circuit), not charging, millivolts → 100%. The top of the
/// standard ~4.10-4.15 V rested-full range for a single-cell Li-ion/LiPo —
/// see module docs' "Full-scale anchor" section. This, not
/// [`BATTERY_FULL_MV`], is what [`percent_from_millivolts`] anchors 100% at.
pub const RESTING_FULL_MV: u32 = 4150;

/// Piecewise open-circuit-voltage → state-of-charge breakpoints for a
/// resting (non-charging) single-cell Li-ion/LiPo pack, approximating the
/// well-known flat-middle / steep-ends shape of a Li-ion discharge curve —
/// see module docs' "Full-scale anchor" section. `(millivolts, percent)`,
/// strictly increasing in both columns; [`percent_from_millivolts`] linearly
/// interpolates between adjacent points.
const RESTING_SOC_CURVE: &[(u32, u8)] = &[
    (BATTERY_EMPTY_MV, 0),
    (3_500, 5),
    (3_600, 10),
    (3_700, 20),
    (3_750, 30),
    (3_800, 42),
    (3_850, 55),
    (3_900, 67),
    (3_950, 77),
    (4_000, 85),
    (4_050, 91),
    (4_100, 96),
    (RESTING_FULL_MV, 100),
];

/// Millivolt ceiling above which a reading is physically impossible for a
/// single-cell Li-ion/LiPo pack and therefore reliably indicates the ADC
/// divider node is reading the USB/charge rail rather than the pack itself —
/// i.e. external power is present. Set ~700 mV above [`BATTERY_FULL_MV`]:
/// wide enough that it is never brushed by ADC noise or a genuinely
/// overcharged/out-of-spec cell, but well below the HIL-observed on-power
/// reading of 4888 mV (see module docs' "Fix" section) that this constant is
/// calibrated against. Unlike the superseded rise/drop hysteresis pair this
/// replaces, a single absolute ceiling needs no history: it is a stateless,
/// per-poll check (see [`battery_poll_step`]).
pub const EXTERNAL_POWER_MV_THRESHOLD: u32 = 4300;

/// Peak-hold sampling window — see [`PeakWindowSampler`] and module docs'
/// "(B) Peak-over-window sampling" section. ~30 s, per this mission's
/// acceptance line.
pub const PEAK_WINDOW_MS: u64 = 30_000;

/// `Low|Partial` [`BatteryLevel`] boundary, in millivolts, before
/// [`LEVEL_HYSTERESIS_MV`] is applied — see module docs' "Three-state
/// voltage-domain bucket" section. **PROVISIONAL**: derived from
/// [`RESTING_SOC_CURVE`] (itself uncalibrated) as "~20% SoC, charge soon,"
/// not from a fresh measurement. Closed by a real HIL capture (Q3/Q5/Q7 in
/// the capture checklist), not by this change.
pub const LEVEL_LOW_PARTIAL_MV: u32 = 3_700;

/// `Partial|Full` [`BatteryLevel`] boundary, in millivolts, before
/// [`LEVEL_HYSTERESIS_MV`] is applied — see module docs' "Three-state
/// voltage-domain bucket" section. **PROVISIONAL**, same basis/caveat as
/// [`LEVEL_LOW_PARTIAL_MV`]: derived from [`RESTING_SOC_CURVE`] as "~85%
/// SoC, mostly full."
pub const LEVEL_PARTIAL_FULL_MV: u32 = 4_000;

/// Hysteresis margin (millivolts) applied at each of
/// [`LEVEL_LOW_PARTIAL_MV`]/[`LEVEL_PARTIAL_FULL_MV`]: once in a bucket,
/// `settled_mv` must cross a boundary by MORE than this margin (not just
/// touch it) before [`battery_level_bucket`] reports a different bucket —
/// stops ADC/curve-breakpoint jitter right at a boundary from flapping the
/// reported bucket every window. **PROVISIONAL**: this is the number the
/// capture checklist's Q5 (at-rest jitter band) is specified to measure.
pub const LEVEL_HYSTERESIS_MV: u32 = 30;

/// Minimum `settled_mv` movement (since what's currently on flash) before
/// [`should_persist_settled_mv`] considers a write worth the flash wear —
/// roughly the finest [`RESTING_SOC_CURVE`] breakpoint spacing, so a write
/// corresponds to an actually meaningful SoC change rather than ADC noise.
pub const PERSIST_MIN_DELTA_MV: u32 = 50;

/// Minimum elapsed time between NVS `settled_mv` writes — defense-in-depth
/// against a pathological `settled_mv` oscillation sitting exactly on the
/// [`PERSIST_MIN_DELTA_MV`] boundary (each crossing would otherwise
/// re-trigger a write every poll). Bounds worst-case write frequency to at
/// most 12 writes/hour — see module docs' "(A)" section.
pub const PERSIST_MIN_INTERVAL_MS: u64 = 5 * 60_000;

// ── BatteryStatus — the ONE shared representation ────────────────────────────

/// Battery status: charge percentage, charging state, raw millivolts, and a
/// coarse bucketed level.
///
/// `percent`/`charging` are the two fields originally scoped in
/// (2026-07-03); `raw_mv` was added 2026-07-05 for the ADC-calibration
/// investigation (originally diagnosis-only — see this module's "ADC
/// calibration ... raw_mv" doc section for how that scoping later widened);
/// `level` was added 2026-08-04 as data-only, then wired to the header
/// `BatteryIndicator` widget by `meshcadet-battery-glanceable-indicator`;
/// `boot_mv`/`confirmed` were added 2026-08-22
/// (`meshcadet-battery-three-state-pipeline`) as HIL capture probes — see
/// module docs' "Three-state voltage-domain bucket" section.
///
/// This is the single representation wired into every consumer: the radio
/// telemetry RESPONSE (`main.rs::build_telemetry_response`, `percent`/
/// `charging`/`raw_mv`/`held_raw_mv`/`level` — see `raw_mv`'s own field doc
/// for how that widened), the host `status` command (`protocol::
/// provisioning::RspStatusPayload`, every field), the on-device admin-menu
/// row (every field except `held_raw_mv`'s wire-only sibling — see
/// `ui::admin_menu::format_battery_display`'s doc for the exact row
/// layout), and the on-device header `BatteryIndicator` widget on the four
/// operational screens (`level` only) — so every consumer reports the same
/// numbers by construction rather than independent reads/formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryStatus {
    /// Charge percentage, `0..=100`. A pure, stateless
    /// `percent_from_millivolts(held_raw_mv)` as of 2026-08-22 — no
    /// percent-domain filter (slew limit, discharge-monotonic latch) sits
    /// between the basis and this field anymore; see module docs'
    /// "Three-state voltage-domain bucket" section.
    pub percent: u8,
    /// `true` if the pack is inferred to be charging (see module docs).
    pub charging: bool,
    /// Last live (post-divider, averaged) ADC millivolt reading — NOT frozen
    /// by the charging latch that `percent` is (see module docs' "ADC
    /// calibration ... raw_mv" section). Surfaced via the host CLI `status`
    /// command AND (as of `meshcadet-battery-glanceable-indicator`) the
    /// on-device admin-menu row, delta-gated there rather than exact-equality
    /// gated (see `ui::admin_menu::battery_display_fields_changed`'s doc);
    /// also read by the over-the-air telemetry RESPONSE (as of
    /// `meshcadet-telemetry-raw-mv-over-air`, appended as a Cayenne
    /// `LPP_GENERIC_SENSOR` entry after the percent/charging pair — see
    /// `main.rs::build_telemetry_response`'s doc for the charging-divergence
    /// caveat that entails).
    pub raw_mv: u32,
    /// Last known non-charge-inflated ("resting") millivolt reading —
    /// `settled_mv`, the SAME basis `percent` and `level` are derived from,
    /// exposed as raw millivolts instead of a lossy-rounded percentage (as
    /// of 2026-08-22, `percent`/`level` derive from this value directly —
    /// no filter sits between them anymore, see module docs' "Three-state
    /// voltage-domain bucket" section). Unlike `raw_mv`, this is frozen
    /// while charging (contamination-free by construction) — see module
    /// docs' "`held_raw_mv`" section. Rendered on-device as of 2026-08-22
    /// (the admin-menu row's `b:` field — see
    /// `ui::admin_menu::format_battery_display`'s doc); previously host-CLI
    /// only.
    pub held_raw_mv: u32,
    /// This boot's raw, un-peak-held ADC seed sample (`BatteryDriver::new`'s
    /// `initial_mv`) — fixed for the life of the boot, added 2026-08-22
    /// (`meshcadet-battery-three-state-pipeline`) as the discriminator
    /// between "inrush sag" and "the pack really is low" (see module docs'
    /// "Three-state voltage-domain bucket" section). Rendered on-device (the
    /// admin-menu row's `B:` field) and read only diagnostically — no
    /// production logic branches on it; the first closed peak window already
    /// unconditionally overwrites `held_raw_mv`/`percent`/`level` regardless
    /// of what this field reads.
    pub boot_mv: u32,
    /// Whether `held_raw_mv` is a trustworthy (CONFIRMED) basis — see
    /// [`advance_settled_confirmed`] and module docs' "(A)" section. Added
    /// 2026-08-22 as a HIL capture probe (the admin-menu row's `f:` field —
    /// see `ui::admin_menu::format_battery_display`'s doc); the underlying
    /// `confirmed` latch itself predates this field (2026-08-04) and already
    /// gated NVS persistence — this just makes it visible on-device.
    pub confirmed: bool,
    /// Coarse bucketed level — see [`BatteryLevel`] and module docs' "(D)"
    /// section. Drives the header `BatteryIndicator` widget on the four
    /// operational screens (`ContactList`/`MessageView`/`Compose`/
    /// `GpsStatus` — ADR-0010 D5), wired by
    /// `meshcadet-battery-glanceable-indicator` via
    /// `ui::battery_indicator::level_to_indicator_level`; also rendered by
    /// the admin-menu row (`L:` field, as of 2026-08-22) and the
    /// over-the-air telemetry RESPONSE (as of 2026-08-22).
    pub level: BatteryLevel,
}

impl BatteryStatus {
    /// Status before the first ADC sample has been taken (device just booted).
    pub const fn unknown() -> Self {
        BatteryStatus {
            percent: 0,
            charging: false,
            raw_mv: 0,
            held_raw_mv: 0,
            boot_mv: 0,
            confirmed: false,
            level: BatteryLevel::Unknown,
        }
    }
}

// ── BatteryLevel — coarse voltage-domain bucket (D) ──────────────────────────

/// Coarse battery-level bucket: 3 voltage-domain buckets (`Low`/`Partial`/
/// `Full`) plus `Unknown` and `Charging` — see module docs' "Three-state
/// voltage-domain bucket" section. As of 2026-08-22
/// (`meshcadet-battery-three-state-pipeline`) this is computed directly from
/// `settled_mv` (millivolts), not from the (now-deleted) slew-limited
/// `percent`; the prior 4-bucket percent-domain version
/// (`Critical`/`Low`/`Medium`/`High`) is gone, not renamed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryLevel {
    /// No sample taken yet (mirrors [`BatteryStatus::unknown`]).
    Unknown,
    /// External power present (see [`battery_poll_step`]) — takes priority
    /// over every voltage-based bucket below, regardless of what the frozen
    /// `settled_mv` basis underneath it currently reads.
    Charging,
    /// Below [`LEVEL_LOW_PARTIAL_MV`] (before hysteresis) — charge soon.
    Low,
    /// Between [`LEVEL_LOW_PARTIAL_MV`] and [`LEVEL_PARTIAL_FULL_MV`] (before
    /// hysteresis) — fine to operate.
    Partial,
    /// At or above [`LEVEL_PARTIAL_FULL_MV`] (before hysteresis) —
    /// mostly/fully charged.
    Full,
}

/// Map `settled_mv` (millivolts) to a [`BatteryLevel`] bucket, with
/// hysteresis at each of [`LEVEL_LOW_PARTIAL_MV`]/[`LEVEL_PARTIAL_FULL_MV`]
/// ([`LEVEL_HYSTERESIS_MV`] wide) so `settled_mv` oscillating right at a
/// boundary doesn't flap the reported bucket every window — see module
/// docs' "Three-state voltage-domain bucket" section.
///
/// `prev` is the previously reported bucket (for hysteresis's "which
/// direction of travel" context). `charging` always wins outright
/// (`BatteryLevel::Charging`), matching `settled_mv`'s own frozen-while-
/// charging basis. Leaving `Charging`/`Unknown` (no established bucket to
/// apply hysteresis against) lands on the plain, no-hysteresis bucket for
/// the current `settled_mv` — same as a fresh boot.
pub fn battery_level_bucket(prev: BatteryLevel, settled_mv: u32, charging: bool) -> BatteryLevel {
    if charging {
        return BatteryLevel::Charging;
    }

    let prev_bucket_index = match prev {
        BatteryLevel::Low => Some(0u8),
        BatteryLevel::Partial => Some(1u8),
        BatteryLevel::Full => Some(2u8),
        BatteryLevel::Unknown | BatteryLevel::Charging => None,
    };

    let index = match prev_bucket_index {
        None => plain_bucket_index_mv(settled_mv),
        Some(prev_idx) => hysteresis_bucket_index_mv(prev_idx, settled_mv),
    };

    match index {
        0 => BatteryLevel::Low,
        1 => BatteryLevel::Partial,
        _ => BatteryLevel::Full,
    }
}

/// The two [`BatteryLevel`] boundaries, in ascending mV order — `settled_mv`
/// past index `i` means at/above `LEVEL_BOUNDARIES_MV[i]`.
const LEVEL_BOUNDARIES_MV: [u32; 2] = [LEVEL_LOW_PARTIAL_MV, LEVEL_PARTIAL_FULL_MV];

/// Plain (no-hysteresis) bucket index for `settled_mv`: the count of
/// [`LEVEL_BOUNDARIES_MV`] entries `settled_mv` has reached or passed.
fn plain_bucket_index_mv(settled_mv: u32) -> u8 {
    LEVEL_BOUNDARIES_MV
        .iter()
        .filter(|&&b| settled_mv >= b)
        .count() as u8
}

/// Hysteresis-adjusted bucket index, starting from `prev_idx`: rising past a
/// boundary requires clearing it by more than [`LEVEL_HYSTERESIS_MV`];
/// falling past a boundary requires dropping below it by more than the same
/// margin. A `settled_mv` within the margin of a boundary leaves `prev_idx`
/// unchanged in that direction.
fn hysteresis_bucket_index_mv(prev_idx: u8, settled_mv: u32) -> u8 {
    let mut idx = prev_idx;
    while (idx as usize) < LEVEL_BOUNDARIES_MV.len()
        && settled_mv >= LEVEL_BOUNDARIES_MV[idx as usize].saturating_add(LEVEL_HYSTERESIS_MV)
    {
        idx += 1;
    }
    while idx > 0
        && settled_mv < LEVEL_BOUNDARIES_MV[(idx - 1) as usize].saturating_sub(LEVEL_HYSTERESIS_MV)
    {
        idx -= 1;
    }
    idx
}

// ── Pure helpers (host-testable, no ADC dependency) ──────────────────────────

/// Clamp a `raw_mv` reading to the `battery_raw_mv: u16` wire field
/// (`protocol::provisioning::RspStatusPayload`).
///
/// Saturating, not truncating: a real pack voltage is always well under
/// `u16::MAX` millivolts, so a legitimate reading is never affected. This
/// only guards against a corrupt/overflowed ADC sample silently wrapping
/// into a small, plausible-looking wrong value on the wire instead of
/// pinning at the (visibly implausible) ceiling.
pub fn clamp_raw_mv_for_wire(raw_mv: u32) -> u16 {
    raw_mv.min(u16::MAX as u32) as u16
}

/// Map a battery-pack millivolt reading to a `0..=100` percentage.
///
/// Piecewise-linear interpolation over [`RESTING_SOC_CURVE`], clamped to
/// `[BATTERY_EMPTY_MV, RESTING_FULL_MV]` — anchors 100% at a realistic
/// rested-full voltage rather than the charger's terminal voltage (see module
/// docs' "Full-scale anchor" section). This assumes `mv` is a resting
/// (non-charging) reading, same as the old linear map; [`BatteryDriver`]
/// only ever calls this on `settled_mv`, which the freeze/latch logic (see
/// [`battery_poll_step`]) guarantees is non-charge-inflated.
pub fn percent_from_millivolts(mv: u32) -> u8 {
    if mv <= BATTERY_EMPTY_MV {
        return 0;
    }
    if mv >= RESTING_FULL_MV {
        return 100;
    }
    for window in RESTING_SOC_CURVE.windows(2) {
        let (lo_mv, lo_pct) = window[0];
        let (hi_mv, hi_pct) = window[1];
        if mv <= hi_mv {
            let span_mv = (hi_mv - lo_mv) as u64;
            let span_pct = (hi_pct - lo_pct) as u64;
            let offset = (mv - lo_mv) as u64;
            return (lo_pct as u64 + (offset * span_pct) / span_mv) as u8;
        }
    }
    // Unreachable: the `mv >= RESTING_FULL_MV` guard above already handles
    // everything at/beyond the curve's last breakpoint. Kept as a safe
    // fallback rather than a `panic!`/`unreachable!` for a non-critical
    // status readout.
    100
}

/// One poll-cycle charging/percent-basis state transition (host-testable, no
/// ADC dependency) — the exact logic [`BatteryDriver::poll`] drives.
///
/// - `settled_mv` is the current percent basis: the last known
///   off-power/valid voltage. [`BatteryStatus::percent`] is always derived
///   from this, never from the raw live voltage.
/// - `live_mv` is this poll's fresh ADC reading.
///
/// Returns the updated `(settled_mv, charging)`.
///
/// This is a **stateless** decision — it needs no charging flag or peak
/// tracker carried in from the previous poll, unlike the rise/drop hysteresis
/// pair this superseded. Whether external power is present is decided fresh
/// every poll, purely from `live_mv` against [`EXTERNAL_POWER_MV_THRESHOLD`],
/// a fixed physical ceiling — so it engages correctly even on the very first
/// poll that is over the ceiling (no prior baseline needed), stays correctly
/// latched through a charger's float/CV plateau (nothing can "stop rising"
/// for a threshold check to misread), and clears the instant the live
/// voltage falls back to a battery-plausible reading. See the module docs'
/// "Fix" section for the rationale, and this module's test suite for a full
/// plug-in / plateau / unplug regression matching the HIL bug report.
pub fn battery_poll_step(settled_mv: u32, live_mv: u32) -> (u32, bool) {
    if live_mv > EXTERNAL_POWER_MV_THRESHOLD {
        // External power present: the raw reading is contaminated by the
        // charge rail. Hold the percent basis at its last known good value
        // rather than let the impossible voltage leak into `percent`.
        return (settled_mv, true);
    }
    // Off external power (or a plausible, uncontaminated reading): the live
    // voltage IS the percent basis.
    (live_mv, false)
}

// ── PeakWindowSampler (B) ──────────────────────────────────────────────────

/// Peak-over-window sampler — see module docs' "(B) Peak-over-window
/// sampling" section. Rather than feed each fresh ADC-averaged reading
/// straight into [`battery_poll_step`], track the MAXIMUM reading seen
/// within a rolling [`PEAK_WINDOW_MS`] window and only emit that window's
/// peak once it elapses, immediately reseeding the next window with the
/// same sample that closed the prior one (so no reading is ever dropped
/// between windows).
///
/// Note on settling time: since the sample that closes a window is honestly
/// still part of it (and reseeds the next window), a step-change that
/// happens to coincide with a window's closing sample can take up to ONE
/// extra window to fully settle to a new, lower level — a real, high
/// reading right at the boundary legitimately carries forward. Negligible
/// in practice at this module's cadence (~15 samples/window at the 2 s ADC
/// poll interval, real discharge measured in hours), but worth stating
/// precisely rather than claiming instant per-window settling.
#[derive(Clone, Copy, Debug)]
pub struct PeakWindowSampler {
    window_start_ms: u64,
    peak_mv: u32,
}

impl PeakWindowSampler {
    /// Start a new sampler, seeded with the first sample (which is, by
    /// construction, the window's peak so far).
    pub fn new(now_ms: u64, first_sample_mv: u32) -> Self {
        PeakWindowSampler {
            window_start_ms: now_ms,
            peak_mv: first_sample_mv,
        }
    }

    /// Feed one fresh sample. Returns `Some(peak_mv)` — the just-closed
    /// window's peak — once [`PEAK_WINDOW_MS`] has elapsed since the window
    /// opened; returns `None` while still accumulating within the current
    /// window.
    pub fn sample(&mut self, now_ms: u64, mv: u32) -> Option<u32> {
        self.peak_mv = self.peak_mv.max(mv);
        if now_ms.saturating_sub(self.window_start_ms) >= PEAK_WINDOW_MS {
            let peak = self.peak_mv;
            self.window_start_ms = now_ms;
            self.peak_mv = mv;
            Some(peak)
        } else {
            None
        }
    }
}

// ── settled_mv persistence — confirmed latch, boot seed, write-wear policy (A) ──

/// Advance the `confirmed` latch — see module docs' "(A)" section.
/// Latches `true` forever the first time a poll observes the pack off
/// external power (a genuine, trustworthy resting sample); never clears
/// once set.
pub fn advance_settled_confirmed(confirmed: bool, charging: bool) -> bool {
    confirmed || !charging
}

/// Boot-time seed for `(settled_mv, charging, confirmed)` — see module docs'
/// "(A)" section.
///
/// - `persisted`: the last CONFIRMED `settled_mv` restored from NVS (`None`
///   on a virgin device, or a missing/failed read).
/// - `initial_mv`: this boot's own first live ADC sample.
///
/// If a persisted value exists, it is used as [`battery_poll_step`]'s PRIOR
/// basis instead of `initial_mv` itself — a device that boots already on
/// power reports the last known GOOD off-power reading, not the raw
/// contaminated first sample, and `confirmed` starts `true` (a persisted
/// value only ever reached flash after passing the confirmation latch once
/// already). With no persisted value (virgin device), this collapses to the
/// pre-persistence boot behavior — including its one documented residual
/// gap (boot-already-on-power with no prior basis at all), in which case
/// `confirmed` correctly starts `false`.
pub fn seed_boot_state(persisted: Option<u32>, initial_mv: u32) -> (u32, bool, bool) {
    let prior = persisted.unwrap_or(initial_mv);
    let (settled_mv, charging) = battery_poll_step(prior, initial_mv);
    let confirmed = persisted.is_some() || !charging;
    (settled_mv, charging, confirmed)
}

/// Decide whether this update's `settled_mv` is worth persisting to NVS —
/// see module docs' "(A)" section for the bounded write-wear rationale.
///
/// - Never persists an UNCONFIRMED basis (`confirmed == false`) — persisting
///   the raw contaminated boot-while-plugged value would seed the NEXT
///   boot's restore with a poisoned baseline.
/// - Always persists the very first confirmed sample this boot has ever had
///   (`last_persisted_mv.is_none()`) — closes the boot-while-plugged gap on
///   the device's NEXT reboot as soon as possible.
/// - Otherwise persists only once `settled_mv` has moved by at least
///   [`PERSIST_MIN_DELTA_MV`] from what's on flash AND at least
///   [`PERSIST_MIN_INTERVAL_MS`] has elapsed since the last write.
pub fn should_persist_settled_mv(
    last_persisted_mv: Option<u32>,
    settled_mv: u32,
    ms_since_last_persist: u64,
    confirmed: bool,
) -> bool {
    if !confirmed {
        return false;
    }
    match last_persisted_mv {
        None => true,
        Some(persisted) => {
            settled_mv.abs_diff(persisted) >= PERSIST_MIN_DELTA_MV
                && ms_since_last_persist >= PERSIST_MIN_INTERVAL_MS
        }
    }
}

// ── battery_window_close_step — the full per-window pipeline ────────────────

/// One full state transition for a just-closed [`PeakWindowSampler`] window —
/// host-testable, no ADC dependency — the exact chain [`BatteryDriver::poll`]
/// drives: [`battery_poll_step`] (settled_mv/charging) →
/// [`advance_settled_confirmed`] (the confirmed latch) →
/// [`battery_level_bucket`] (the coarse bucket, computed directly on the new
/// `settled_mv`). Returns the updated `(settled_mv, charging, confirmed,
/// level)`. `BatteryStatus::percent` is not tracked here at all as of
/// 2026-08-22 — it is a pure, stateless `percent_from_millivolts(settled_mv)`
/// computed wherever a `BatteryStatus` snapshot is built (see
/// [`BatteryDriver::status`]), not a value this pipeline carries forward.
///
/// **Fixes `meshcadet-battery-level-reads-full-when-depleted` (2026-08-17)
/// and `meshcadet-battery-three-state-pipeline` (2026-08-22) — both from the
/// SAME root simplification.** The former shipped a special case (skip the
/// slew limiter on the poll that first proves `confirmed`) to escape a
/// percent-domain filter that shouldn't have applied to an unconfirmed seed
/// in the first place. The latter found a SECOND way that same filter layer
/// produced a stuck-wrong display (a charged-then-unplugged pack pinned near
/// its pre-charge reading — see module docs' "Three-state voltage-domain
/// bucket" section) and, rather than adding a second special case, deleted
/// the filter layer outright: no slew limit, no discharge-monotonic latch,
/// no `was_confirmed` snap-on-first-confirm branch. `settled_mv`'s own
/// [`battery_poll_step`]/[`advance_settled_confirmed`] chain and
/// [`battery_level_bucket`]'s millivolt hysteresis are now the ONLY filtering
/// between the ADC and the display — with no stateful percent-domain filter
/// carrying a prior output, there is no untrustworthy prior left to protect,
/// and both bug instances (and the next one in the family) are structurally
/// unreachable rather than individually patched.
pub fn battery_window_close_step(
    settled_mv: u32,
    level: BatteryLevel,
    confirmed: bool,
    window_peak_mv: u32,
) -> (u32, bool, bool, BatteryLevel) {
    let (new_settled_mv, charging) = battery_poll_step(settled_mv, window_peak_mv);
    let new_confirmed = advance_settled_confirmed(confirmed, charging);
    let new_level = battery_level_bucket(level, new_settled_mv, charging);
    (new_settled_mv, charging, new_confirmed, new_level)
}

// ── Tests ─────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_raw_mv_for_wire_passes_through_realistic_readings() {
        assert_eq!(clamp_raw_mv_for_wire(0), 0);
        assert_eq!(clamp_raw_mv_for_wire(3624), 3624);
        assert_eq!(clamp_raw_mv_for_wire(4200), 4200);
    }

    #[test]
    fn clamp_raw_mv_for_wire_saturates_instead_of_wrapping() {
        // A corrupt/overflowed sample must saturate at u16::MAX, not silently
        // wrap into a small, plausible-looking wrong value (e.g. `as u16`
        // truncation of 65_536 would wrap to 0 — indistinguishable from "no
        // reading yet").
        assert_eq!(clamp_raw_mv_for_wire(u16::MAX as u32 + 1), u16::MAX);
        assert_eq!(clamp_raw_mv_for_wire(u32::MAX), u16::MAX);
    }

    #[test]
    fn percent_clamps_at_floor_and_ceiling() {
        assert_eq!(percent_from_millivolts(0), 0);
        assert_eq!(percent_from_millivolts(BATTERY_EMPTY_MV), 0);
        assert_eq!(percent_from_millivolts(BATTERY_EMPTY_MV - 100), 0);
        assert_eq!(percent_from_millivolts(RESTING_FULL_MV), 100);
        assert_eq!(percent_from_millivolts(RESTING_FULL_MV + 500), 100);
        // The OLD charging-voltage anchor (4200 mV) must now also read 100% —
        // it is above the new resting-full anchor, not below it — but must
        // NOT be required to reach it exactly (see "full-scale anchor" bug:
        // that was the whole defect being fixed).
        assert_eq!(percent_from_millivolts(BATTERY_FULL_MV), 100);
    }

    #[test]
    fn percent_full_rested_pack_reads_at_or_near_100() {
        // The original HIL bug report: a rested-full pack (charge LED off)
        // must read ~100%, not the ~82-94% the old 3300-4200 linear map
        // structurally capped it at. Standard rested-full range is ~4.10-4.15V.
        for mv in [4_100u32, 4_120, 4_150] {
            let p = percent_from_millivolts(mv);
            assert!(
                p >= 90,
                "expected >=90% at {mv}mV (rested-full range), got {p}%"
            );
        }
    }

    #[test]
    fn percent_midpoint_of_curve_breakpoints_matches_table() {
        // Spot-check a handful of the RESTING_SOC_CURVE breakpoints directly
        // — this is a resting open-circuit-voltage curve, not a straight
        // line, so "half the mV range" is no longer "half the percent range"
        // (that was the old, since-retired, linear-map assumption).
        assert_eq!(percent_from_millivolts(3_500), 5);
        assert_eq!(percent_from_millivolts(3_800), 42);
        assert_eq!(percent_from_millivolts(4_000), 85);
    }

    #[test]
    fn percent_interpolates_between_breakpoints() {
        // Halfway between the (3750, 30) and (3800, 42) breakpoints must read
        // halfway between 30% and 42%.
        let mid_mv = (3_750 + 3_800) / 2;
        assert_eq!(percent_from_millivolts(mid_mv), 36);
    }

    #[test]
    fn percent_monotonic_in_millivolts() {
        // A higher voltage must never map to a lower percentage — a monotonicity
        // violation would show up as a battery reading that "drains" while
        // physically charging.
        let mut last = percent_from_millivolts(BATTERY_EMPTY_MV);
        let mut mv = BATTERY_EMPTY_MV;
        while mv <= RESTING_FULL_MV {
            let p = percent_from_millivolts(mv);
            assert!(p >= last, "percent decreased at {mv}mV: {p}% < {last}%");
            last = p;
            mv += 37; // odd step to exercise non-round intermediate values
        }
    }

    #[test]
    fn unknown_status_is_zero_percent_not_charging() {
        let s = BatteryStatus::unknown();
        assert_eq!(s.percent, 0);
        assert!(!s.charging);
        assert_eq!(s.raw_mv, 0);
        assert_eq!(s.held_raw_mv, 0);
        assert_eq!(s.boot_mv, 0);
        assert!(!s.confirmed);
        assert_eq!(s.level, BatteryLevel::Unknown);
    }

    // ── battery_poll_step ────────────────────────────────────────────────

    #[test]
    fn resting_basis_tracks_live_voltage_when_flat() {
        // Off external power, voltage unchanged: the live voltage IS the basis.
        let (settled, charging) = battery_poll_step(3700, 3700);
        assert_eq!(settled, 3700);
        assert!(!charging);
    }

    #[test]
    fn resting_basis_follows_slow_natural_discharge() {
        // A slowly falling voltage (normal discharge) must not be mistaken
        // for anything but a resting pack, and the basis must follow it down
        // rather than sticking at an old higher reading.
        let (settled, charging) = battery_poll_step(3700, 3680);
        assert_eq!(settled, 3680);
        assert!(!charging);
    }

    #[test]
    fn reading_at_threshold_is_still_battery_plausible() {
        // The boundary itself is inclusive of a real reading (not yet
        // "impossible"): must not be misread as on-power.
        let (settled, charging) = battery_poll_step(3624, EXTERNAL_POWER_MV_THRESHOLD);
        assert!(!charging);
        assert_eq!(settled, EXTERNAL_POWER_MV_THRESHOLD);
    }

    #[test]
    fn reading_one_mv_over_threshold_is_already_impossible() {
        // Pins down the exact `>` (not `>=`) boundary from the other side:
        // one mV over the ceiling must already flip to on-power, freezing the
        // basis rather than tracking the now-impossible live reading.
        let settled_before = EXTERNAL_POWER_MV_THRESHOLD;
        let (settled, charging) =
            battery_poll_step(settled_before, EXTERNAL_POWER_MV_THRESHOLD + 1);
        assert!(
            charging,
            "one mV over the ceiling must already be treated as external power present"
        );
        assert_eq!(
            settled, settled_before,
            "basis must freeze, not adopt the just-over-ceiling reading"
        );
    }

    #[test]
    fn reading_above_threshold_holds_last_basis_and_reports_power() {
        // This is the exact HIL data point: raw 4888 mV, far above the
        // physical single-cell ceiling — must hold the prior basis, not track
        // the impossible live voltage.
        let settled_before = 3_775; // 36% on RESTING_SOC_CURVE, the last valid unplugged reading
        let (settled, charging) = battery_poll_step(settled_before, 4888);
        assert!(
            charging,
            "an impossible-for-a-battery reading means external power is present"
        );
        assert_eq!(
            settled, settled_before,
            "percent basis must hold the last known good value, not the contaminated live voltage"
        );
    }

    #[test]
    fn engages_on_the_very_first_poll_with_no_prior_history() {
        // Unlike the superseded rise trigger (which needs a delta from a
        // prior sample and so misses a device that boots already on power),
        // the threshold check needs no history: even seeded with itself as
        // "prior settled", an above-threshold live reading is flagged.
        let (_settled, charging) = battery_poll_step(4888, 4888);
        assert!(
            charging,
            "must detect power-present on the very first sample, no prior baseline required"
        );
    }

    #[test]
    fn boot_already_on_power_is_flagged_charging_immediately() {
        // Mirrors exactly the call BatteryDriver::new makes with its initial
        // sample: a device that boots already attached to a charger must be
        // flagged `charging: true` from that very first sample, not leak the
        // contaminated first read through as a false 100% while reporting
        // "not charging".
        let initial_mv = 4888;
        let (settled, charging) = battery_poll_step(initial_mv, initial_mv);
        assert!(
            charging,
            "boot-on-power must be detected on the initial sample"
        );
        // `percent` still shows the contaminated first reading in this edge
        // case (no prior off-power sample exists to freeze at) — a documented
        // residual gap (see module docs' "Fix" section) — but `charging`
        // being correct immediately is the strict improvement over the
        // superseded rise trigger, which required a delta to ever see this.
        assert_eq!(settled, initial_mv);
    }

    #[test]
    fn holds_basis_indefinitely_while_above_threshold_no_plateau_confusion() {
        // The old rate-of-rise heuristic reported "not charging" once voltage
        // stopped climbing at the charger's CV plateau. The threshold check
        // has nothing to "stop rising" — repeated polls at the same
        // above-threshold voltage must keep reporting charging.
        let settled_before = 3_775; // 36%
        let mut settled = settled_before;
        let mut charging;
        for _ in 0..10 {
            (settled, charging) = battery_poll_step(settled, 4888);
            assert!(
                charging,
                "must not drop to 'not charging' while still above the ceiling"
            );
            assert_eq!(
                settled, settled_before,
                "basis must stay frozen the whole time on power"
            );
        }
    }

    #[test]
    fn drop_back_under_threshold_ends_power_and_resyncs_basis() {
        let settled_before = 3_775; // frozen pre-plug basis (36%)
        let (settled, charging) = battery_poll_step(settled_before, 3_900); // unplugged, pack settled a bit higher
        assert!(
            !charging,
            "a reading back under the ceiling means external power is gone"
        );
        assert_eq!(
            settled, 3_900,
            "basis must resync to the fresh post-unplug reading, not stay frozen at the stale pre-plug value"
        );
    }

    #[test]
    fn full_plug_unplug_cycle_never_reports_a_false_100_percent() {
        // End-to-end regression for the original HIL report: a known ~36%
        // pack is plugged in, raw voltage jumps to the observed
        // battery-impossible 4888 mV, holds there for a while, and is later
        // unplugged.
        let resting_mv = 3_775; // an exact 36% on RESTING_SOC_CURVE
        assert_eq!(percent_from_millivolts(resting_mv), 36);

        let mut settled = resting_mv;
        let mut charging;

        // Plug in: raw voltage jumps to the charge-rail reading.
        (settled, charging) = battery_poll_step(settled, 4888);
        assert!(charging);
        assert_eq!(
            percent_from_millivolts(settled),
            36,
            "must not read 100% the instant external power is detected"
        );

        // Hold on power for a while — must keep reporting charging AND must
        // never read 100%.
        for _ in 0..10 {
            (settled, charging) = battery_poll_step(settled, 4888);
            assert!(
                charging,
                "must not drop to 'not charging' while still on power"
            );
            assert_eq!(
                percent_from_millivolts(settled),
                36,
                "must stay at the true SoC while on power"
            );
        }

        // Unplug: raw voltage falls back to a battery-plausible reading.
        (settled, charging) = battery_poll_step(settled, 3_850);
        assert!(!charging, "must detect the unplug");
        let unplugged_percent = percent_from_millivolts(settled);
        assert_eq!(
            unplugged_percent, 55,
            "must resync to the fresh post-unplug reading's true SoC, got {unplugged_percent}%"
        );
    }

    // ── PeakWindowSampler (B) ────────────────────────────────────────────────

    #[test]
    fn peak_window_masks_a_transient_sag_within_the_window() {
        // A LoRa-TX/backlight rail sag mid-window must not leak through as
        // the window's reported reading — the peak (pre-sag steady-state)
        // must win.
        let mut s = PeakWindowSampler::new(0, 3_800);
        assert_eq!(s.sample(2_000, 3_780), None, "still accumulating");
        assert_eq!(s.sample(4_000, 3_650), None, "transient sag mid-window");
        assert_eq!(s.sample(6_000, 3_795), None, "recovered");
        let peak = s.sample(PEAK_WINDOW_MS, 3_790);
        assert_eq!(
            peak,
            Some(3_800),
            "the window's peak (pre-sag steady-state), not the sag, must be reported"
        );
    }

    #[test]
    fn peak_window_returns_none_before_the_window_elapses() {
        let mut s = PeakWindowSampler::new(1_000, 3_700);
        assert_eq!(s.sample(1_000 + PEAK_WINDOW_MS - 1, 3_710), None);
    }

    #[test]
    fn peak_window_closes_exactly_at_the_boundary() {
        let mut s = PeakWindowSampler::new(0, 3_700);
        assert_eq!(s.sample(PEAK_WINDOW_MS, 3_700), Some(3_700));
    }

    #[test]
    fn peak_window_tracks_a_real_sustained_drop_across_successive_windows() {
        // Genuine, sustained discharge (not a transient sag) must still be
        // tracked window-over-window: several consecutive windows of a
        // steadily lower level, sampled at a realistic ~2s cadence (many
        // samples per window), must converge the reported peak down to that
        // level. A single level-step can take up to one extra window to
        // fully settle (see `PeakWindowSampler`'s own doc note on the shared
        // boundary sample) — repeat the lowest level once more so this test
        // asserts the fully-settled value, not an in-between one.
        let mut s = PeakWindowSampler::new(0, 3_900);
        let mut now = 0u64;
        let mut last_peak = 3_900;
        for level in [3_900u32, 3_850, 3_800, 3_800] {
            for _ in 0..15 {
                now += 2_000;
                if let Some(p) = s.sample(now, level) {
                    last_peak = p;
                }
            }
        }
        assert_eq!(
            last_peak, 3_800,
            "after enough windows at the new, lower level, the peak must fully settle there"
        );
    }

    #[test]
    fn peak_window_new_window_does_not_inherit_the_prior_windows_higher_peak() {
        let mut s = PeakWindowSampler::new(0, 3_900);
        assert_eq!(s.sample(PEAK_WINDOW_MS, 3_700), Some(3_900));
        // The new window's own samples never touch 3_900 again — its peak
        // must reflect ONLY this window's readings, not leak the prior
        // window's higher value in.
        assert_eq!(s.sample(PEAK_WINDOW_MS + 10_000, 3_690), None);
        assert_eq!(
            s.sample(2 * PEAK_WINDOW_MS, 3_695),
            Some(3_700),
            "new window's peak must be its own max (3700 seed), not the prior window's 3900"
        );
    }

    // ── BatteryLevel bucket + hysteresis (D) — voltage-domain, 3-state ──────

    #[test]
    fn bucket_charging_always_wins() {
        assert_eq!(
            battery_level_bucket(BatteryLevel::Low, 4_150, true),
            BatteryLevel::Charging,
            "charging must win regardless of the frozen settled_mv basis underneath"
        );
    }

    #[test]
    fn bucket_plain_assignment_from_unknown_or_charging() {
        assert_eq!(
            battery_level_bucket(BatteryLevel::Unknown, 3_500, false),
            BatteryLevel::Low
        );
        assert_eq!(
            battery_level_bucket(BatteryLevel::Unknown, 3_850, false),
            BatteryLevel::Partial
        );
        assert_eq!(
            battery_level_bucket(BatteryLevel::Unknown, 4_150, false),
            BatteryLevel::Full
        );
        assert_eq!(
            battery_level_bucket(BatteryLevel::Charging, 3_850, false),
            BatteryLevel::Partial,
            "leaving Charging with no prior bucket lands on the plain bucket"
        );
    }

    #[test]
    fn bucket_hysteresis_holds_near_a_boundary_from_below() {
        // Sitting in Low, a settled_mv that only just touches the
        // LEVEL_LOW_PARTIAL_MV boundary (without clearing it by more than
        // LEVEL_HYSTERESIS_MV) must NOT flip to Partial yet.
        let held = battery_level_bucket(BatteryLevel::Low, LEVEL_LOW_PARTIAL_MV + 1, false);
        assert_eq!(
            held,
            BatteryLevel::Low,
            "1mV past the boundary is within the hysteresis band"
        );
    }

    #[test]
    fn bucket_hysteresis_flips_once_clearly_past_the_boundary() {
        let flipped = battery_level_bucket(
            BatteryLevel::Low,
            LEVEL_LOW_PARTIAL_MV + LEVEL_HYSTERESIS_MV + 1,
            false,
        );
        assert_eq!(flipped, BatteryLevel::Partial);
    }

    #[test]
    fn bucket_hysteresis_holds_near_a_boundary_from_above() {
        // Sitting in Partial, a settled_mv that only just dips under
        // LEVEL_LOW_PARTIAL_MV must NOT flip back to Low yet.
        let held = battery_level_bucket(BatteryLevel::Partial, LEVEL_LOW_PARTIAL_MV - 1, false);
        assert_eq!(held, BatteryLevel::Partial);
    }

    #[test]
    fn bucket_hysteresis_falls_once_clearly_past_the_boundary() {
        let fell = battery_level_bucket(
            BatteryLevel::Partial,
            LEVEL_LOW_PARTIAL_MV.saturating_sub(LEVEL_HYSTERESIS_MV + 1),
            false,
        );
        assert_eq!(fell, BatteryLevel::Low);
    }

    #[test]
    fn bucket_hysteresis_does_not_flap_across_repeated_jitter_at_a_boundary() {
        let mut bucket = BatteryLevel::Low;
        // Jitter back and forth right around the LEVEL_LOW_PARTIAL_MV
        // boundary, never clearing the hysteresis margin in either direction.
        let b = LEVEL_LOW_PARTIAL_MV;
        for &mv in &[b, b - 5, b + 5, b - 10, b + 10, b - 5, b + 5] {
            bucket = battery_level_bucket(bucket, mv, false);
            assert_eq!(bucket, BatteryLevel::Low, "must not flap at {mv}mV");
        }
    }

    #[test]
    fn bucket_can_traverse_every_boundary_when_genuinely_discharging() {
        let mut bucket = BatteryLevel::Unknown;
        bucket = battery_level_bucket(bucket, 4_150, false);
        assert_eq!(bucket, BatteryLevel::Full);
        bucket = battery_level_bucket(bucket, 3_850, false);
        assert_eq!(bucket, BatteryLevel::Partial);
        bucket = battery_level_bucket(bucket, 3_400, false);
        assert_eq!(bucket, BatteryLevel::Low);
    }

    // ── settled_mv persistence (A): confirmed latch / boot seed / write-wear ─

    #[test]
    fn advance_settled_confirmed_latches_true_on_first_off_power_poll() {
        assert!(
            !advance_settled_confirmed(false, true),
            "still charging, never confirmed yet"
        );
        assert!(
            advance_settled_confirmed(false, false),
            "first off-power poll confirms"
        );
        assert!(
            advance_settled_confirmed(true, true),
            "latch must stay true even while charging again"
        );
    }

    #[test]
    fn seed_boot_state_virgin_device_off_power_is_confirmed() {
        let (settled, charging, confirmed) = seed_boot_state(None, 3_700);
        assert_eq!(settled, 3_700);
        assert!(!charging);
        assert!(
            confirmed,
            "an off-power first sample is trustworthy immediately"
        );
    }

    #[test]
    fn seed_boot_state_virgin_device_boot_while_plugged_is_unconfirmed() {
        // The one residual gap `battery_poll_step`'s own docs call out: no
        // prior basis exists at all, so the contaminated first sample leaks
        // through as `settled_mv` — and must NOT be trusted for persistence.
        let (settled, charging, confirmed) = seed_boot_state(None, 4_888);
        assert_eq!(settled, 4_888);
        assert!(charging);
        assert!(
            !confirmed,
            "boot-while-plugged with no persisted basis must not be trusted to persist"
        );
    }

    #[test]
    fn seed_boot_state_restores_persisted_basis_and_closes_boot_while_plugged_gap() {
        // The mission's headline acceptance case: a device with a persisted
        // 36%-basis boots already plugged in. `percent` must show the last
        // known GOOD reading, not the contaminated raw sample, and must be
        // marked confirmed (a persisted value already passed the latch once).
        let (settled, charging, confirmed) = seed_boot_state(Some(3_775), 4_888);
        assert_eq!(
            percent_from_millivolts(settled),
            36,
            "must restore the persisted resting basis, not the contaminated boot sample"
        );
        assert!(charging, "must still correctly detect power is present");
        assert!(confirmed);
    }

    #[test]
    fn seed_boot_state_restores_persisted_basis_boot_off_power_resyncs_fresh() {
        let (settled, charging, confirmed) = seed_boot_state(Some(3_775), 3_900);
        assert_eq!(
            settled, 3_900,
            "boots off power: resync to the fresh reading"
        );
        assert!(!charging);
        assert!(confirmed);
    }

    #[test]
    fn should_not_persist_an_unconfirmed_basis() {
        assert!(!should_persist_settled_mv(None, 4_888, u64::MAX, false));
        assert!(!should_persist_settled_mv(
            Some(3_775),
            4_888,
            u64::MAX,
            false
        ));
    }

    #[test]
    fn should_persist_the_first_ever_confirmed_sample_immediately() {
        assert!(should_persist_settled_mv(None, 3_700, 0, true));
    }

    #[test]
    fn should_not_persist_a_small_move_even_after_the_interval_elapses() {
        let moved = 3_700 + PERSIST_MIN_DELTA_MV - 1;
        assert!(!should_persist_settled_mv(
            Some(3_700),
            moved,
            PERSIST_MIN_INTERVAL_MS,
            true
        ));
    }

    #[test]
    fn should_not_persist_a_big_move_before_the_interval_elapses() {
        let moved = 3_700 + PERSIST_MIN_DELTA_MV + 10;
        assert!(!should_persist_settled_mv(
            Some(3_700),
            moved,
            PERSIST_MIN_INTERVAL_MS - 1,
            true
        ));
    }

    #[test]
    fn should_persist_once_both_delta_and_interval_gates_clear() {
        let moved = 3_700 + PERSIST_MIN_DELTA_MV;
        assert!(should_persist_settled_mv(
            Some(3_700),
            moved,
            PERSIST_MIN_INTERVAL_MS,
            true
        ));
    }

    #[test]
    fn should_persist_bounds_write_frequency_to_the_documented_worst_case() {
        // 12 writes/hour == one write every 5 minutes == PERSIST_MIN_INTERVAL_MS.
        const WRITES_PER_HOUR: u64 = 3_600_000 / PERSIST_MIN_INTERVAL_MS;
        assert_eq!(WRITES_PER_HOUR, 12);
    }

    // ── battery_window_close_step / full pipeline ────────────────────────────
    //
    // A tiny host-side stand-in for `BatteryDriver` — the exact
    // boot-then-poll sequence `firmware::battery::BatteryDriver` drives,
    // built entirely from this module's own public pure functions so the
    // full level pipeline (voltage in, `BatteryStatus::level` out) is
    // exercised end to end without any ADC/hardware dependency. This is the
    // mission's acceptance instrument: "an in-container test drives the
    // level pipeline with synthetic depleted/full voltage-or-SoC inputs and
    // with charging vs discharging state, asserting the reported level
    // tracks the input in each case."
    struct SimDriver {
        settled_mv: u32,
        charging: bool,
        confirmed: bool,
        level: BatteryLevel,
        peak_sampler: PeakWindowSampler,
    }

    impl SimDriver {
        fn boot(persisted: Option<u32>, initial_mv: u32, now_ms: u64) -> Self {
            let (settled_mv, charging, confirmed) = seed_boot_state(persisted, initial_mv);
            let level = battery_level_bucket(BatteryLevel::Unknown, settled_mv, charging);
            SimDriver {
                settled_mv,
                charging,
                confirmed,
                level,
                peak_sampler: PeakWindowSampler::new(now_ms, initial_mv),
            }
        }

        /// One ~2s ADC sample, mirroring `BatteryDriver::poll`'s cadence.
        /// Only actually updates state once a peak window closes.
        fn poll(&mut self, now_ms: u64, mv: u32) {
            if let Some(window_peak_mv) = self.peak_sampler.sample(now_ms, mv) {
                let (settled_mv, charging, confirmed, level) = battery_window_close_step(
                    self.settled_mv,
                    self.level,
                    self.confirmed,
                    window_peak_mv,
                );
                self.settled_mv = settled_mv;
                self.charging = charging;
                self.confirmed = confirmed;
                self.level = level;
            }
        }

        /// Drive `n` polls at the fixed ADC cadence, all reporting `mv`.
        fn run(&mut self, now_ms: &mut u64, n: u32, mv: u32) {
            for _ in 0..n {
                *now_ms += 2_000;
                self.poll(*now_ms, mv);
            }
        }

        /// `percent` as of 2026-08-22 is a pure function of `settled_mv` —
        /// no state to track separately (see module docs' "Three-state
        /// voltage-domain bucket" section).
        fn percent(&self) -> u8 {
            percent_from_millivolts(self.settled_mv)
        }
    }

    #[test]
    fn pipeline_synthetic_full_voltage_discharging_reads_full_not_charging() {
        let mut d = SimDriver::boot(None, RESTING_FULL_MV, 0);
        let mut now = 0u64;
        d.run(&mut now, 20, RESTING_FULL_MV);
        assert!(!d.charging);
        assert_eq!(d.level, BatteryLevel::Full);
        assert!(d.percent() >= 90);
    }

    #[test]
    fn pipeline_synthetic_depleted_voltage_discharging_reads_low_not_charging() {
        let mut d = SimDriver::boot(None, BATTERY_EMPTY_MV + 50, 0);
        let mut now = 0u64;
        d.run(&mut now, 20, BATTERY_EMPTY_MV + 50);
        assert!(!d.charging);
        assert_eq!(d.level, BatteryLevel::Low);
    }

    #[test]
    fn pipeline_synthetic_charging_voltage_reads_charging_regardless_of_underlying_soc() {
        let mut d = SimDriver::boot(None, BATTERY_EMPTY_MV + 50, 0);
        let mut now = 0u64;
        // Confirm off-power first (a real prior basis), then plug in.
        d.run(&mut now, 20, BATTERY_EMPTY_MV + 50);
        d.run(&mut now, 20, EXTERNAL_POWER_MV_THRESHOLD + 500);
        assert!(d.charging);
        assert_eq!(d.level, BatteryLevel::Charging);
    }

    #[test]
    fn pipeline_boot_already_plugged_with_persisted_basis_shows_correct_percent_immediately() {
        // The already-working case (NVS persistence closes this gap): a
        // persisted, previously-confirmed low basis means `confirmed` is
        // true from the very first sample, so this was never affected by
        // either bug mechanism this mission fixes.
        let d = SimDriver::boot(Some(3_775), EXTERNAL_POWER_MV_THRESHOLD + 500, 0);
        assert!(d.charging);
        assert!(d.confirmed);
        assert_eq!(d.percent(), 36);
        assert_eq!(d.level, BatteryLevel::Charging);
    }

    // ── Named acceptance regressions (meshcadet-battery-three-state-pipeline) ──
    //
    // These three prove the mission's two independent root-cause mechanisms
    // — the charge-session ratchet and the unprotected boot seed — are
    // fixed, and that the #163 depleted-reads-full direction still holds.
    // Confirmed via static trace against the pre-fix pipeline (this
    // module's "Three-state voltage-domain bucket" doc section) to FAIL
    // there: mechanism (1) means the pre-fix `slew_limit_percent`'s
    // discharge-monotonic latch could only crawl a displayed percent upward
    // at `PERCENT_MAX_SLEW_PER_UPDATE_PCT` (2 points) per closed window —
    // recovering from ~10% to ~100% took on the order of 45 windows, not
    // two. Mechanism (2) means a boot-time sag was immediately `confirmed`
    // (an off-power boot sample confirms instantly — see
    // `seed_boot_state_virgin_device_off_power_is_confirmed`), so its first
    // correction was ALSO slew-limited rather than snapping to truth — only
    // the boot-while-PLUGGED/unconfirmed case had a special-cased escape.

    #[test]
    fn charge_session_then_unplug_recovers_to_full_within_two_windows() {
        // Boots off-power at a low-but-plausible basis (confirmed immediately).
        let mut d = SimDriver::boot(None, 3_600, 0);
        assert_eq!(d.level, BatteryLevel::Low);

        let mut now = 0u64;
        // Charge session: two whole windows above the external-power ceiling.
        // `settled_mv` freezes at the pre-charge basis for the entire
        // session — the correct, still-kept freeze/latch behaviour (see
        // `battery_poll_step`), NOT the bug.
        d.run(&mut now, 15, EXTERNAL_POWER_MV_THRESHOLD + 500);
        d.run(&mut now, 15, EXTERNAL_POWER_MV_THRESHOLD + 500);
        assert!(d.charging);
        assert_eq!(
            d.settled_mv, 3_600,
            "basis stays frozen at the pre-charge reading for the whole session"
        );

        // Unplug: the pack is now genuinely at RESTING_FULL_MV. The first
        // post-unplug window's peak still carries the just-closed charging
        // window's reseed sample (`PeakWindowSampler`'s documented
        // one-window settling lag — see that struct's own doc), so it takes
        // the SECOND window to see a genuinely-low peak.
        d.run(&mut now, 15, RESTING_FULL_MV);
        d.run(&mut now, 15, RESTING_FULL_MV);

        assert!(!d.charging, "must detect the unplug");
        assert_eq!(
            d.level,
            BatteryLevel::Full,
            "must recover to Full within two closed windows post-unplug, not stay pinned \
             near the pre-charge Low bucket forever"
        );
        assert!(
            d.percent() >= 90,
            "percent must also recover, not just the bucket: got {}%",
            d.percent()
        );
    }

    #[test]
    fn boot_sag_seeded_basis_is_corrected_by_the_first_closed_window() {
        // A boot-time sag (LoRa-TX/backlight rail sag during inrush, per
        // `BatteryDriver::new`'s doc) seeds `initial_mv` 400 mV BELOW the
        // true resting voltage. The device boots off external power, so
        // `seed_boot_state` marks it `confirmed` immediately — exactly the
        // case the pre-fix slew limiter did NOT protect.
        let true_resting_mv = 3_900; // ~67% on RESTING_SOC_CURVE
        let sagged_boot_mv = true_resting_mv - 400; // 3_500, ~5%
        let mut d = SimDriver::boot(None, sagged_boot_mv, 0);
        assert!(d.confirmed, "an off-power boot sample confirms immediately");
        assert_eq!(d.settled_mv, sagged_boot_mv);

        let mut now = 0u64;
        // First closed window: the rail has recovered post-inrush, so every
        // sample in this window reads the true resting voltage.
        d.run(&mut now, 15, true_resting_mv);

        assert_eq!(
            d.settled_mv, true_resting_mv,
            "must fully correct to the true reading in the FIRST closed window, not crawl"
        );
        assert_eq!(d.level, BatteryLevel::Partial);

        // Same proof "in either direction" the plan's acceptance line
        // names — but a boot-HIGH-then-correct-DOWN variant of the FIRST
        // window specifically would conflate this fix with
        // `PeakWindowSampler`'s own, separate, documented one-window
        // settling lag (it seeds a window's peak with the sample that
        // opened it, so a high boot seed legitimately survives its own
        // first window's peak-hold — that is correct peak-hold behavior,
        // not the slew-limiter bug this test targets; the third named
        // regression below covers exactly that lag for the boot-while-
        // plugged case). This asserts the actual fix instead: once
        // `confirmed`, a later, ordinary single-window movement — away from
        // any boot seed's peak-hold shadow — snaps fully to the new target
        // in ONE window, in EITHER direction, with no slew delay stacked on
        // top of the window relationship.
        let mut d2 = SimDriver::boot(None, true_resting_mv, 0); // off-power boot: confirmed immediately
        let mut now2 = 0u64;
        d2.run(&mut now2, 15, true_resting_mv); // window 1: flat, settles the boot seed's own shadow
        d2.run(&mut now2, 15, true_resting_mv - 400); // window 2: closing sample reseeds window 3 below
        assert_eq!(
            d2.settled_mv, true_resting_mv,
            "the window-2 peak still carries window 1's closing (flat) sample forward — \
             PeakWindowSampler's own documented one-window reseed lag, not this fix's concern"
        );
        d2.run(&mut now2, 15, true_resting_mv - 400); // window 3: now genuinely, fully low
        assert_eq!(
            d2.settled_mv,
            true_resting_mv - 400,
            "once the peak is genuinely at the new target, the basis must snap there in ONE \
             window — no slew delay stacked on top of the window relationship"
        );
    }

    #[test]
    fn boot_while_plugged_over_depleted_pack_reaches_low_within_two_windows_no_crawl() {
        // The #163 direction, re-proven under the new voltage-domain bucket:
        // a VIRGIN device boots already on external power over a truly
        // depleted pack. `charging` is detected immediately; the basis
        // initially shows the contaminated boot-time reading (the documented
        // residual gap — module docs' "Fix" section). The instant the pack
        // is genuinely confirmed off external power, the bucket/percent must
        // snap to the truth, not crawl down over ~25 minutes.
        let depleted_mv = BATTERY_EMPTY_MV + 50; // deep in Low range
        let mut d = SimDriver::boot(None, EXTERNAL_POWER_MV_THRESHOLD + 500, 0);
        assert!(
            d.charging,
            "boot-while-plugged must be flagged charging immediately"
        );

        let mut now = 0u64;
        d.run(&mut now, 15, depleted_mv); // closes the contaminated window
        d.run(&mut now, 15, depleted_mv); // closes a genuinely-low window

        assert!(
            !d.charging,
            "must detect the unplug once the peak is genuinely low"
        );
        assert!(d.confirmed, "an off-power sample must confirm the basis");
        assert_eq!(
            d.level,
            BatteryLevel::Low,
            "must reflect the true depleted charge within two windows, not crawl down further"
        );
        assert!(
            d.percent() < 10,
            "percent must also snap to the true low reading, got {}%",
            d.percent()
        );
    }
}
