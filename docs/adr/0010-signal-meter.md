# ADR-0010 — Repeater Signal Meter: Hop-Gated RSSI, Max-With-Decay, Per-Screen Placement

- **Status:** Accepted (2026-07-13)
- **Deciders:** Maintainer design review (`meshcadet-signal-meter` campaign)
- **Supersedes:** —
- **Implements:** the tracker child (`meshcadet-signal-meter-tracker`) of the
  `meshcadet-signal-meter` campaign. **This ADR IS the contract** the UI
  child (`meshcadet-signal-meter-ui`) consumes — the `SignalLevel` type, the
  `SignalTracker`/`SignalConfig` API, the bars table, the SNR gate, and the
  decay curve below are frozen as of this ADR's acceptance; a breaking
  change to any of them needs its own ADR revision, not a silent edit.
- **Code:** `firmware-core/src/signal_tracker.rs` (+ `firmware-core/src/lib.rs`'s
  `pub mod signal_tracker;`). The rx-tap wiring
  (`firmware/src/main.rs`) and the Slint `SignalMeter` widget are the UI
  child's deliverable, out of scope here.

## Context

MeshCadet operators currently have no signal indicator of any kind — no way
to judge, before sending a message, whether it is likely to reach the rest
of the mesh at all. Every received packet already carries the SX1262 link
quality (RSSI dBm = `-(rssi_raw)/2`, SNR dB = `snr_raw/4`, decoded in
`radio.rs`'s `get_packet_status()` and read at `main.rs`'s RX-poll site,
before the dedup drop) and a hop count (`path_len`, parsed via
`protocol::header::PathLen::hop_count()`) — both logged and then discarded.

MeshCore's relay model makes that hop count load-bearing: **companion nodes
never relay traffic; only designated repeaters retransmit.** So a received
packet with hop count ≥ 1 was, by construction, last transmitted by a
repeater, and its RSSI/SNR is therefore the downlink quality from the
nearest audible repeater to this device — obtainable with **no advert
parsing** at all. A zero-hop packet came straight from its origin companion
and is not repeater evidence.

This ADR freezes the pure tracking logic and its tunables as a `firmware-core`
contract (host-testable, no esp-idf dependency) so the rx-tap wiring and the
Slint widget (both landing in the UI child) consume one frozen API and one
frozen bars/decay table, rather than each re-deriving their own thresholds.

## Decision

### D1 — Repeater-signal definition: hop ≥ 1, no advert parsing, no per-repeater identity

A signal sample is repeater evidence **iff** its hop count is ≥ 1
(`SignalTracker::record` ignores `hop_count == 0` outright). No MeshCore
advert (which would carry a specific repeater's identity/name) is ever
parsed or required. Consequently, this system knows nothing about *which*
repeater was heard, how many are in range, or per-repeater identity/naming —
only "was the strongest recent hop≥1 packet strong". Per-repeater identity
is explicitly out of scope; it would require advert parsing this project
does not do.

**Alternatives considered — advert-based repeater discovery.** Rejected:
MeshCadet neither emits nor parses adverts today, and adding that parsing
solely to attribute a signal reading to a named repeater is a materially
larger surface for a meter whose only job is "is a repeater audible, and how
well" — the hop-count-gated RSSI approach answers that with data already on
every packet.

### D2 — Bars table + SNR knock-down

`SignalLevel` is either `DirectOnly` (no repeater heard, zero bars) or
`Bars(1..=5)`. The RSSI floor for each bar count (`SignalConfig::bar_floor_dbm`,
strongest-first, checked "reading ≥ floor"):

| RSSI (dBm) | Bars |
|---|---|
| ≥ -70 | 5 |
| -70 … -85 | 4 |
| -85 … -100 | 3 |
| -100 … -110 | 2 |
| -110 … -120 | 1 |
| < -120 / none | 0 (`DirectOnly`) |

Each range's own lower bound is inclusive of that tier (e.g. exactly -85 dBm
reads 4 bars, not 3) — see `signal_tracker.rs`'s `bars_threshold_boundaries`
test for the exact boundary each side of every seam.

SNR knock-down then applies to any sample that clears at least one RSSI
floor: `snr < 0 dB` drops one bar; `snr < -10 dB` (approaching the ≈-20 dB
LoRa noise floor) drops one further bar (two total). This is deliberately
**never allowed to knock a genuinely-heard repeater below 1 bar** — a
sample that cleared the weakest RSSI floor stays at `Bars(1)` at minimum
regardless of how bad its SNR is; only aging with no fresher packet (decay,
D3) can bring it to `DirectOnly`. Rationale: SNR knock-down exists to
distinguish "strong but noisy" from "genuinely solid", not to fabricate a
false "no repeater at all" reading for a link that is demonstrably being
heard.

All of the above (the five RSSI floors, both SNR thresholds) are
`SignalConfig` fields, not baked-in constants — `SignalConfig::new`
validates/clamps degenerate input (out-of-order floors are forced strictly
descending; `snr_floor_db` is forced below `snr_knockdown_db`) so a future
tuning pass can adjust the table without touching tracker logic, and cannot
construct a config that produces non-monotonic bars.

### D3 — Decay: max-with-decay, not a hard window

The tracker holds exactly one state: the strongest recent hop≥1 reading
(after SNR knock-down) and its arrival timestamp. `SignalTracker::level`
reports that peak **aged by its own timestamp**: held at full strength for
`hold_full_ms` (default 60 s), then stepped down one bar every
`decay_step_ms` (default 45 s), reaching `DirectOnly` after ~4–5 minutes of
no qualifying packet. A fresh stronger-or-equal reading resets both the peak
and its timestamp — so the peak always ages from the *most recent* evidence
for its current bar count, not from whenever the all-time-strongest packet
happened to arrive. This is why "one lucky packet" cannot pin full bars
indefinitely: it decays on its own schedule the moment no further evidence
at that strength arrives, exactly like the later, weaker peaks would.

**Why max-with-decay, not a hard "was a repeater packet seen in the last N
seconds" window:** a hard window makes the meter flicker between full bars
and empty on ordinary duty-cycle gaps between repeater transmissions
(adverts/beacons are not constant), which is a worse operator experience
than a meter that gracefully steps down over minutes. `hold_full_ms` and
`decay_step_ms` are both `SignalConfig` fields (not hard-coded), so the
hold/decay pacing can be retuned from HIL feedback without a tracker-logic
change.

### D4 — Time is caller-supplied; no reboot persistence

`record`/`level` take an explicit monotonic `now_ms: u64` rather than
reading any clock internally — this keeps `firmware-core`'s tracker
completely host-testable (no esp-idf dependency), matching the
`firmware-core` extraction pattern (ADR-0005). The real rx-tap (UI child)
is responsible for supplying a genuine monotonic millisecond clock (e.g.
`esp_timer_get_time`), not a loop counter.

`SignalTracker` carries no state across a restart — `SignalTracker::new`
always starts at `DirectOnly` (matching "device just booted, no repeater
heard yet"), and nothing is persisted to NVS. This is deliberate: the meter
is meant to reflect *current* conditions, and a stale pre-reboot peak
surviving a restart would misrepresent them for no benefit — the tracker
re-establishes a true reading within one hold window of the first repeater
packet after boot.

### D5 — Per-screen placement, no global overlay (UI child's responsibility, frozen here)

MeshCadet's UI (`ui/mod.rs`'s `ActiveScreen`) has no global status-bar /
root-window overlay today. Rather than build one solely for this meter, the
UI child embeds a small `SignalMeter` Slint component directly in each of
the four operational screens' headers — `contact_list`, `message_view`,
`compose`, `gps_status` — and excludes `splash`, `unprovisioned`, and
`pin_entry` (signal is meaningless before the radio and provisioning are
even up). This is frozen here so the UI child does not have to re-litigate
scope: no global-overlay refactor is in scope for this campaign.

### D6 — The downlink-proxy caveat (honest, not over-promising)

The meter measures **downlink audibility only**: how well this device can
hear the nearest repeater. It says nothing about:
- whether that repeater (or any further hop) can hear a message *this*
  device transmits (uplink is not measured at all — this is a receive-side
  proxy, not a bidirectional link check);
- the health of the mesh beyond the first hop.

Any label, icon, or copy the UI child attaches to the meter **must not
claim delivery** ("your message will get through") — the honest framing is
closer to "you can likely reach the nearest repeater", not "your message
will arrive". This caveat is a first-class design decision, not an
afterthought: a signal meter that reads full bars while a message
downstream silently fails to route would be actively misleading, worse than
no meter at all.

## Consequences

- The tracker is a pure, `#[cfg(test)]`-covered `firmware-core` module with
  zero esp-idf surface — its 17 unit tests execute under
  `cargo test --workspace` today (unlike anything living behind the
  detached `firmware/` workspace, per ADR-0005), covering every bars
  boundary, both SNR knock-down thresholds (including the "never below 1
  bar" floor), `hop_count == 0` rejection, a hop≥1 duplicate still resetting
  the hold window, the full hold-then-decay-to-`DirectOnly` curve, and the
  boot-time `DirectOnly` default.
- Nothing in this mission wires the tracker to a live packet source or
  renders anything — `firmware/src/main.rs`'s rx-tap and the Slint
  `SignalMeter` widget are both the UI child's deliverable, consuming this
  frozen API. Until that child lands, `signal_tracker` compiles and tests
  but has no runtime caller. **Update:** the UI child has since landed —
  the rx-tap (`firmware/src/main.rs`'s RX-poll block, before the dedup
  drop) and the `SignalMeter` widget (`firmware/src/ui/signal_meter.slint`,
  embedded on the four operational screens) both now consume this API
  exactly as specified above; no change to the frozen contract itself.
- `SignalConfig`'s fields are all `pub`, giving the UI child (or a later HIL
  follow-on) latitude to retune bars/SNR/decay pacing without touching
  `signal_tracker.rs`'s logic — at the cost of no compile-time guarantee
  that a hand-rolled `SignalConfig` literal is sane; `SignalConfig::new`'s
  validation/clamping is the only backstop (a caller that constructs the
  struct literal directly, bypassing `new`, could still produce an
  out-of-order table). Acceptable: the only two callers in this campaign are
  `SignalConfig::default()` and, later, the UI child's own tuning, both
  reviewed in a PR.
- **Maintainer HIL steps (post-merge, non-blocking)** — with a real
  repeater, confirm:
  1. **Bars track distance** — more bars nearer the repeater, fewer stepping
     away.
  2. **Decay behaves** — bars hold for ~60 s after the last repeater packet,
     then step down to empty over ~4–5 minutes.
  3. **Direct-only state** — powering the repeater off (or moving out of
     range) drives the meter to `DirectOnly` (empty bars + the zero-hop
     icon) once the decay window elapses.
  4. **Noisy-but-strong doesn't read solid** — a high-RSSI, low-SNR link
     reads fewer bars than RSSI alone would suggest (SNR knock-down).
  5. **No header overlap** — the meter never overlaps or blocks any header
     control on any of the four operational screens.
  This HIL pass is explicitly **not a blocking acceptance gate** for either
  child or the campaign — it is maintainer-run, post-merge, documented here
  so it isn't forgotten, not a condition of landing.

## Alternatives Considered

### A. Advert-based per-repeater discovery

See D1 — rejected: MeshCadet neither emits nor parses adverts today, and
hop-count-gated RSSI already answers the meter's actual question ("is a
repeater audible, how well") without that added surface.

### B. Hard time-window instead of max-with-decay

See D3 — rejected: a hard "seen in the last N seconds" cutoff flickers on
ordinary gaps between repeater transmissions; graceful decay over minutes is
a materially better operator experience for the same underlying data.

### C. A global status-bar overlay for the meter

See D5 — rejected: no such overlay exists in this UI today, and building
one solely for this meter is out of proportion to the feature — four
per-screen embeds reuse the existing header layout in each screen instead.

### D. Persist the last-known level across a reboot

Rejected: the meter is meant to reflect current conditions; a device that
just rebooted has not yet re-verified repeater audibility, so starting at
`DirectOnly` and re-establishing a true reading within one hold window is
more honest than replaying a possibly-stale pre-reboot value.
