# ADR-0013 — Screen Lock: Policy/UI Layer, No Wire-Protocol Change

- **Status:** Accepted (2026-08-23)
- **Deciders:** Maintainer design review (`meshcadet-screen-lock` campaign)
- **Supersedes:** —
- **Implements:** ADR-0001 §4 (physical USB possession = admin authority),
  ADR-0005 (`firmware-core` extraction — the pure lock state machine lives
  there for the same host-testability reason)
- **Code:** `protocol/src/provisioning.rs` (`LOCK_*` constants, frame types,
  payloads), `site/provisioner/codec.js` (JS mirror), `firmware-core/src/
  pin_menu.rs` (`RuntimeSettings::lock_flags`/`lock_timeout_s`,
  `MenuAction::SetLockEnabled`/`SetLockTimeout`), `firmware-core/src/
  runtime_settings_store.rs` (`mc_rts` v0x01→v0x02), `firmware-core/src/
  lock_store.rs` (lock-PIN blob codec + constant-time `verify`),
  `firmware-core/src/ui/lock.rs` (idle→lock decision, backoff state
  machine), `firmware/src/lock_store.rs` (`mc_lock` NVS wrapper),
  `firmware/src/admin_server.rs` (`SET_LOCK_PIN`/`SET_LOCK_CONFIG`/
  `QUERY_LOCK` handlers), `firmware/src/ui/mod.rs` (`UiRuntime::locked`,
  the `step()` input gate, `trip_lock`/`unlock`), `firmware/src/ui/screens/
  lock.rs` (the lock screen), `host/src/main.rs` (`set-lock-pin`/
  `reset-lock-pin`/`lock-config`/`lock-status`), `site/provisioner.js` /
  `site/provisioner/validation.js` (web-provisioner lock section).

## Context

MeshCadet had no screen lock: a provisioned T-Deck left unattended exposed
the full contact list, every conversation thread, and the compose surface to
anyone who picked it up. The maintainer wanted a configurable idle lock
gated on a 4-digit PIN **distinct from the existing admin-menu PIN**, with
notifications still firing while locked, the lock PIN writable only from
the web provisioner and the host CLI, and enable/timeout adjustable from
all three surfaces (on-device admin menu, web provisioner, host CLI).

Two tree facts, verified before implementation started, shaped every
decision below:

1. `ProvisionedConfig` is `Box`ed on both threads that hold it
   (`admin_server`, `provisioning_server`) as of the
   `boot-pthread-stack-overflow-fix` mission — growing that struct is a
   **heap** cost, not a stack cost, so the thread stack budgets that once
   made a config-blob bump expensive no longer apply. Only the doc
   comments citing the old figures (`firmware/src/main.rs`,
   `firmware/src/provisioning_server.rs`) would go stale if the struct
   ever grew — and this campaign did not grow it (see D2).
2. `evt_tx: SyncSender<UiEvent>` is constructed well before `admin_server`
   spawns and `SyncSender` is `Clone` — a host-originated lock-config write
   can be forwarded from `admin_server` to the UI thread as a `UiEvent`,
   keeping the UI thread the sole writer of the `mc_rts` runtime-settings
   blob (see D2's single-writer invariant).

## Decision

**This campaign is a policy/UI layer only. The MeshCore wire protocol is
untouched — no on-air behavior anywhere in this diff changes.** Every new
frame type (`FRAME_QUERY_LOCK`, `FRAME_SET_LOCK_PIN`, `FRAME_SET_LOCK_CONFIG`,
`FRAME_RSP_LOCK`) lives in the USB-serial **provisioning/admin** framing
(`protocol::provisioning`), which is a distinct wire surface from the
MeshCore radio protocol (`protocol::{crypto, identity}` and the on-air
message path) that this device speaks byte-exact against upstream MeshCore.
Nothing in `protocol::crypto`, `protocol::identity`, or the radio dispatch
path changed.

### D1 — Idle timer: independent field, shared clock, no grace period

A new `lock_timeout_s: u16` on `pin_menu::RuntimeSettings`, range
`LOCK_TIMEOUT_MIN_S = 15 ..= LOCK_TIMEOUT_MAX_S = 3600`, default
`LOCK_TIMEOUT_DEFAULT_S = 300` (5 min). No zero sentinel — on/off is carried
by `lock_flags` bit 0 (D6), never an overloaded magic value. The lock timer
reads the same `UiRuntime::last_activity_ms` clock the screen-sleep check
already uses (`firmware_core::ui::lock::idle_lock_due`), and trips
independently of `screen_asleep`.

Rejected: widening `screen_sleep_timeout_s` (a `u8` capped at 120 s with a
`0` = "never sleep" sentinel and a stepper widget contract written to
`0..=120` — ties a display-power decision to an access-control decision and
touches the `mc_rts` blob layout for a field whose existing job is not the
lock's job); lock-on-sleep as a single shared timer (a user who disables
sleep would silently disable the lock too); a post-sleep grace period (a
second exception surface on the sharpest path in the feature — the accepted
trade-off is that a tap two seconds after the lock trips must re-enter the
PIN; that is the feature working, not a bug).

Consequence stated plainly: with `lock_timeout_s >= 15` and
`screen_sleep_timeout_s <= 120`, the lock can trip while the screen is still
lit. That is correct — "normal screen content must not be visible until
unlocked" is the requirement, directly observable in `ui_sim`.

### D2 — Persistence split: PIN to a dedicated store, enable+timeout to `mc_rts`

| State | Home | Writer | Reaches the device via |
|---|---|---|---|
| Lock PIN (4 bytes + len) | new dedicated NVS store, namespace `mc_lock`, key `lock_blob` (`firmware/src/lock_store.rs`, codec in `firmware-core/src/lock_store.rs`) | `admin_server` thread | `FRAME_SET_LOCK_PIN` |
| Lock enable bit + `lock_timeout_s` | `mc_rts` runtime-settings blob (existing) | **UI thread only** | admin menu directly; host/provisioner via `FRAME_SET_LOCK_CONFIG` → `admin_server` → `evt_tx` → UI thread → `UiEvent::LockConfigChanged` → existing `UiCommand::PersistRuntimeSettings` path |

**Rejected: a `ProvisionedConfig` v0x04 blob bump.** The dedicated-store
route follows this vehicle's own precedent for `admin_server`-owned single
values (`advert_ts_store.rs`, `gps_baud_store.rs`), and it removes from the
campaign's cost the v0x03→v0x04 migration path, the pinned
`size_of::<ProvisionedConfig>() == 3560` test, the `MAX_BLOB_LEN == 3544`
budget assertions, `site/provisioner/codec.js`'s blob decoder, and
`validation.js`'s blob bounds — none of which this campaign touches.
`ProvisionedConfig` is unchanged by this campaign: `size_of::
<ProvisionedConfig>()` is still `3560` and `MAX_BLOB_LEN` is still `3544`
(`firmware-core/src/config_store.rs`'s pinned tests remain green
unmodified), and the config version is still `0x03`.

Trade-off named: one more NVS namespace, and the lock PIN is not carried in
the provisioning blob, so re-flashing a config blob does not carry a lock
PIN with it. Recovery is `reset-lock-pin` over USB, mirroring the existing
`reset-pin`.

`mc_rts` versioning: bumped `VERSION` `0x01` → `0x02`
(`firmware-core/src/runtime_settings_store.rs`), keeping a `VERSION_V1`
reader that accepts both existing lengths and defaults `lock_timeout_s` for
an upgrading v0x01 blob without resetting any previously-saved
notification/telemetry/screen-sleep preference. The bump is cheap because
`mc_rts` is device-local — no host codec and no `codec.js` mirror of it
exists.

Known, documented asymmetry (admin PIN only, as of the deep-review pass 1
fix below): the UI thread's admin-PIN comparison state is boot-seeded via
`UiEvent::BootSeed`, so a host-set admin PIN still takes effect only at the
next boot — the *existing* admin-PIN behavior, unchanged by this campaign.

The **lock** PIN no longer shares that posture. It is still boot-seeded the
same way (`BootSeed::lock_pin`/`lock_pin_len`, for the first `step()` before
any live write arrives), but `FRAME_SET_LOCK_PIN` (`admin_server.rs`) now
also forwards a `UiEvent::LockPinChanged` over the same `evt_tx` clone
`FRAME_SET_LOCK_CONFIG` already uses, and `UiRuntime::handle_event` applies
it via the existing `set_lock_pin` immediately — no reboot needed. This
closed a same-USB-session lockout: `set-lock-pin` followed by `lock-config
--enable` used to lock the device against a PIN the running UI thread had
never seen (locked out until power-cycle), and `reset-lock-pin` against an
already-locked device used to silently fail to unlock it (the live
comparison still ran against the stale boot-time PIN). No wire-protocol
change — `FRAME_SET_LOCK_PIN`'s payload shape is unchanged; only what
`admin_server` does after the NVS write succeeds.

### D3 — Lock is an overlay above `ActiveScreen`, not a ninth variant

`UiRuntime` gained `locked: bool` and `lock_screen: Option<screens::
LockScreen>`. `ActiveScreen` is **unchanged**. Locking hides the active
screen component (the existing `hide_active_screen` mechanism) and shows
the lock component; unlocking hides the lock component and re-shows the
retained one.

Rejected: a ninth `ActiveScreen` variant. `ActiveScreen` is matched
exhaustively in several places in `firmware/src/ui/mod.rs`
(`name()`, `set_battery_level`, `set_signal_level`, `handle_trackball`)
plus a dozen `if let` sites — every one would grow an arm for a screen with
no battery, signal, or trackball semantics. Decisively: an enum variant
*owns* its component, so locking would have *dropped* the underlying
screen — destroying an in-flight Compose draft and forcing "which screen
does unlock return to?" to be re-derived from navigation code. The overlay
retains the object instead, so **the draft survives and unlock returns to
whatever was active, by construction, with no navigation logic at all.**

Input swallowing: the lock gate sits in `step()` immediately **after** the
existing global wake/swallow interceptor and **before** any screen dispatch,
for all three input modalities (touch/keyboard/trackball) — wake-swallow
first (so the tap that lights the screen is consumed to wake only), lock
gate second (so nothing reaches the underlying screen), screen dispatch
last. While `locked`, no event reaches `window.dispatch_*` on behalf of the
underlying screen; an `xtask`/grep check pins this.

A `pending_nav` code set in the same tick the lock trips is handled
explicitly: the lock wins — the pending navigation is applied to the
retained screen state, then the lock overlay is presented on top, so unlock
lands on the navigated-to screen rather than replaying a stale nav.

### D4 — Brute force: in-RAM escalating backoff, locked-on-boot, nothing persisted

An attempt counter held in RAM only (`firmware_core::ui::lock::
LockAttemptState`). Attempts 1–4 are free; the 5th consecutive wrong PIN
starts an escalating lockout of 30s → 60s → 120s → 300s, capped at 300s
(`lockout_seconds_after_failure`); a correct PIN resets the counter. **If
the lock is enabled, the device boots locked** (`boots_locked`). No
wipe-after-N.

Rejected: persisting the counter to NVS. An NVS write on every wrong PIN is
an attacker-controlled flash-write amplifier — a trivial wear DoS from the
numpad — and it buys almost nothing: clearing an in-RAM counter requires a
reboot, which requires physical access, which already defeats this control
outright via `reset-lock-pin` over the unauthenticated USB admin channel
(see D-honest). Boot-locked is the meaningful anti-reboot property and it
is free. **No NVS write occurs on a wrong PIN.**

The backoff state machine is pure and lives in `firmware-core/src/ui/
lock.rs` (see D-test), so it is covered by host-runnable unit tests.

### D5 — Lock-screen disclosure: unread count only, no sender, no preview

The lock screen shows a waiting-message **count badge** only — no sender
name, no message preview, no per-conversation breakdown. Unread state is
unaffected by the lock: a message that arrives while locked stays unread
until its conversation is actually opened after unlock.

This mirrors the posture `firmware-core/src/notification.rs` already ships
for the asleep case (audible chirp + keyboard-backlight blink, no sender, no
preview) — a count leaks strictly less than the chirp the device already
emits.

Rejected: no badge at all — it would make the lock screen indistinguishable
from a dead device and push users toward disabling the lock entirely, the
classic security control that gets turned off because it is unusable.

### D6 — `lock_flags`: wire bit 0 only; define and reserve bit 1, do not ship it

`LOCK_SCREEN_ENABLE = 0x01` (bit 0, the enable/disable toggle, wired this
campaign, device-editable from all three surfaces) and
`LOCK_NO_DEVICE_DISABLE = 0x02` (bit 1, provisioner-forbids-on-device-disable,
**constant defined and documented as reserved; behavior not shipped**) both
now exist in `protocol::provisioning`, resolving a citation
`firmware-core/src/config_store.rs`'s module doc already made of constants
that did not exist before this campaign.

Bit 1 is deferred because the on-device admin menu is already gated behind
the *admin* PIN — a different secret from the lock PIN by design (D-honest
explicitly requires that distinction), so someone who cannot unlock the
screen also cannot reach the admin menu. Bit 1's marginal control against
the stated casual-access threat model is near-zero, while it would cost a
policy control in all three configuration UIs plus a refusal path in
`apply_menu_action`. Defining the constant now keeps the door open at zero
cost; a v2 child can ship the behavior if a fleet-management need appears.

New frame opcodes, chosen to avoid the retired, do-not-reuse-without-re-audit
opcodes `0x60` (formerly `FRAME_SET_LOCKS`) and `0x30` (formerly
`FRAME_SET_RADIO_PRESET`) — both remain unused by this campaign:
`FRAME_QUERY_LOCK = 0x06`, `FRAME_SET_LOCK_PIN = 0x52`,
`FRAME_SET_LOCK_CONFIG = 0x53`, `FRAME_RSP_LOCK = 0x8D`.

Readback goes through a dedicated `QUERY_LOCK`/`RSP_LOCK` pair rather than
extending `RspStatusPayload`, which is decoded by both the host CLI and
`site/provisioner/codec.js` — extending it would have dragged a
wire-compatibility question into every consumer for a field unrelated to
device status. `RspStatusPayload` stays frozen.

4-digit enforcement is a **decode-path rejection**, not merely a UI check:
`protocol::provisioning::decode_set_lock_pin` returns a distinguishable
`ProvError` for any `pin_len != 4` byte payload, in addition to (never
instead of) the host CLI's and `validation.js`'s own client-side checks
against the same `LOCK_TIMEOUT_MIN_S`/`MAX_S`/`LOCK_PIN_LEN` constants —
one source of truth all three surfaces clamp against.

### D7 — Emergency affordance: none. Decided, not defaulted

**Nothing is reachable while the device is locked.** No emergency contact,
no SOS send, no panic affordance. This is a deliberate ruling, recorded here
so it is not a decision-by-omission: an emergency-send path is a bypass by
construction (it would transmit from a locked device) and would need its
own contact-selection surface in all three configuration UIs. MeshCadet's
own honest posture (D-honest) is that this is a casual-access control on a
hobbyist mesh device, not a safety-of-life system (see the project
[Disclaimer](../../README.md#-disclaimer--no-warranty-no-guarantee-of-safety-use-at-your-own-risk));
adding an SOS affordance would imply a reliability guarantee the radio layer
does not make. The README states this explicitly, in the same best-effort
register the rest of the README already uses.

### D-honest — Posture statement, carried into every user-facing doc

`firmware/src/admin_server.rs` handles runtime USB-serial admin frames
(including `SET_LOCK_PIN`/`SET_LOCK_CONFIG`) with **no PIN gate of any
kind** — physical USB possession is already the sole authentication factor
for the admin channel (ADR-0001 §4) — and `reset-lock-pin` exists as the
deliberate recovery path for a forgotten lock PIN. The screen lock is
therefore a **casual-access control, not a security boundary**: anyone with
physical access and a USB cable clears it. Both the lock PIN and the
existing admin PIN are stored **in plaintext** in device flash; each is
checked with its own constant-time comparison (`pin_menu::verify_pin` for
the admin PIN, `firmware_core::lock_store::verify` for the lock PIN), which
prevents a timing side-channel but is not hashing. `README.md`'s screen-lock
section states this plainly, matching the language already displayed on the
web provisioner's lock panel (`site/provisioner.html`).

### D-test — Testability constraint that shaped the decomposition

`firmware/` is a detached, cross-compiled workspace: a `#[cfg(test)]` block
written there type-checks but never runs under `cargo test --workspace`.
`ui_sim` renders Slint screen components, so it covers the lock screen's
*appearance* but not the lock *state machine*. Every pure decision in this
feature therefore lands in `firmware-core` — `firmware-core/src/ui/lock.rs`
(new), modelled on the existing `firmware_core::ui::touch::
touch_wake_transition` precedent — and `firmware/src/ui/mod.rs` calls it,
owning only the Slint/hardware plumbing.

## Consequences

- Three independently-maintained configuration surfaces (on-device admin
  menu, web provisioner, host CLI) now enforce the same `LOCK_PIN_LEN` = 4
  and `LOCK_TIMEOUT_MIN_S..=LOCK_TIMEOUT_MAX_S` = `15..=3600` bounds. The
  Rust decode path is the load-bearing enforcement; the CLI and
  `validation.js` checks are host/client-side conveniences that fail fast,
  never a substitute for it.
- `mc_rts` gained a second version rung (v0x01/v0x02). The existing
  accept-two-lengths-at-one-version ladder inside `VERSION_V1` is already at
  its comfortable limit — a third rung there would have been the fragile
  move; bumping `VERSION` instead keeps each version's blob length fixed.
- The lock PIN is not recoverable by re-flashing a provisioning blob — only
  `reset-lock-pin` recovers it. This is consistent with, not worse than, the
  existing admin-PIN recovery story.
- `pin_entry.rs`'s header previously claimed the entered PIN "matches the
  stored hash" — it does not; it is a plaintext constant-time compare. That
  doc claim is corrected as a drive-by fix in the same campaign that touches
  this file's lock-related additions.

## Alternatives Considered

Each design question's rejected alternatives are recorded inline under its
own decision (D1–D7) above, alongside the trade-off actually accepted —
per this project's usual ADR style (see ADR-0007 §2 for the same pattern),
a single recommendation with its cost named, not a menu.

## Out of scope (carried from the plan of record)

- A `ProvisionedConfig` v0x04 bump — actively rejected in D2.
- `LOCK_NO_DEVICE_DISABLE` (bit 1) enforcement — constant defined, behavior
  deferred to a v2 child (D6).
- Any emergency/SOS affordance — ruled out in D7.
- Persisting the brute-force attempt counter — rejected in D4.
- Hashing either PIN — both stay plaintext with a constant-time compare,
  unchanged from today. This is the campaign's largest unaddressed
  weakness (plaintext at rest in NVS on a device whose USB admin channel
  has no PIN gate at all), and it is correctly out of scope: hashing buys
  nothing against a threat model where `reset-lock-pin` over unauthenticated
  USB already clears the control outright, so hashing without first
  authenticating the admin channel would be security theater. The honest
  follow-on is a mission scoped to authenticating the USB admin channel,
  after which hashing becomes worth doing — not "hash the PINs" on its own.
- The MeshCore wire protocol — untouched, per this ADR's title.
