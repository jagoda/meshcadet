# Screen-lock bench procedure (MeshCadet, real T-Deck)

**Maintainer-run.** This document batches every genuinely device-bound check
for the screen-lock feature into a single sitting on real hardware. It is
self-contained — no Houston dependency, no queued follow-on mission; the
checks below are not, and should not become, an automated CI/HIL job.

Everything the screen-lock feature does that does **not** require a real
device (the state machine, the backoff escalation, the wire-format
round-trip, the CLI/provisioner validation) is already covered by
`cargo test --workspace` and `node --test site/` and is out of scope here.
This procedure exists only for the behavior that a host-native test cannot
observe: real idle timing, a real touchscreen unlock, a real boot, and a
real notification arriving on hardware.

See [`README.md`](../README.md#screen-lock) for the feature's user-facing
description and honest posture, and
[`docs/adr/0013-screen-lock-policy-layer.md`](adr/0013-screen-lock-policy-layer.md)
for the full design rationale (D1–D7).

## 0. Setup

**Prerequisites:** a LilyGo T-Deck Plus flashed with a build that includes
the screen-lock feature, connected over USB, and already provisioned with
at least one contact (so a DM can be sent to it in step 3). See the
top-level [`README.md`](../README.md#2-firmware-flashing-a-t-deck-plus) for
the flash procedure and
[`README.md`](../README.md#3-provisioning-a-device-the-admin-cli) for
provisioning.

Enable the lock and set a known PIN from the host CLI before starting
(these two commands are themselves host-testable and are not part of the
device-bound checks below — they're just how you get the device into the
state the rest of this procedure needs):

```sh
cargo run -p host -- --port /dev/ttyACM0 set-lock-pin --pin 1234
cargo run -p host -- --port /dev/ttyACM0 lock-config --enable --timeout 30
```

`--timeout 30` (the minimum allowed, `LOCK_TIMEOUT_MIN_S`) keeps step 1
short; use a larger value if you'd rather not wait exactly 30 s. Confirm the
config landed:

```sh
cargo run -p host -- --port /dev/ttyACM0 lock-status
# expect: enabled=true, timeout=30s, pin set=true
```

## 1. Idle timeout trips the lock

- Leave the device untouched (no touch, no keypress) for the configured
  `--timeout` (30 s above).
- **Expect:** the currently-displayed screen (contacts, a thread, compose —
  whatever was active) disappears and the lock screen appears: a 4-dot PIN
  entry pad, the numpad, and the unread-count badge (0 if there's nothing
  unread yet). No contact names, message text, or thread content should be
  visible anywhere on screen.
- Tap or press a key **immediately** after the lock trips. **Expect:** it
  does not reach the underlying screen (no navigation, no compose-text
  insertion) — only the lock pad reacts. This is D3's swallow-ordering
  guarantee; a real touch/keypress is the only way to observe it, since
  `ui_sim` renders appearance but not live input routing.

## 2. Unlock with the correct PIN

- Enter `1234` on the lock screen's pad.
- **Expect:** the lock screen disappears and the screen that was active
  when the lock tripped reappears (contacts/thread/compose — whatever it
  was in step 1), with its state intact.

## 3. Message received while locked: chirp + badge, no disclosure

- Re-trigger the lock (wait out the idle timeout again, or see step 5 for a
  faster way to get back to locked).
- From another MeshCore-speaking node (or another MeshCadet), send a DM to
  this device.
- **Expect, while still locked:**
  - An audible chirp (and/or keyboard-backlight blink, matching the
    existing asleep-state notification behavior).
  - The lock screen's unread-count badge increments.
  - **No sender name and no message preview appear anywhere on the lock
    screen.** This is the check that most directly verifies D5 — it cannot
    be verified any other way, since the badge-only behavior is a rendering
    decision only visible on the actual lock screen.
- Unlock (step 2) and confirm the message is still shown as **unread** in
  its conversation — the lock does not mark anything read on arrival.

## 4. Compose draft survives a lock/unlock cycle

- Unlock the device if it's currently locked.
- Open Compose against any contact and type a partial message — enough text
  to recognize later (e.g. `testing screen lock draft`). Do **not** send it.
- Wait out the idle timeout (or use step 5's shortcut) so the lock trips
  with the draft still in progress.
- Unlock with the PIN.
- **Expect:** Compose reappears with the exact draft text still present,
  cursor and all — the draft was never dropped or cleared by the lock/unlock
  cycle. This is D3's overlay-not-a-ninth-variant guarantee in practice.

## 5. Device boots locked

- With the lock still enabled (`lock-status` shows `enabled=true`), power
  cycle the device (unplug/replug USB, or use the reset button).
- **Expect:** immediately after boot completes (past the splash screen),
  the device shows the lock screen — **not** the contact list or whatever
  screen was active before the reboot. No PIN entry, no button press, should
  be needed to reach this state; it should already be locked.
- Unlock with the PIN to confirm the device is otherwise fully booted and
  functional.

*(Shortcut for steps 3–5: instead of waiting out the idle timeout each
time, a power cycle after step 0's `lock-config --enable` will boot the
device directly into the locked state per this section — useful for getting
back to "locked" without a real wait between checks.)*

## 6. Wrong-PIN backoff escalates: 30 / 60 / 120 / 300 s, capped

- From the locked screen, enter an incorrect PIN (e.g. `0000`, assuming the
  real PIN isn't `0000`) **five times in a row**, without a correct entry
  in between.
- **Expect after the 5th consecutive wrong PIN:** the pad locks out and the
  screen shows a backoff countdown starting at **30 s**. During the
  countdown, PIN entry should be refused/ignored (the countdown, not a
  fresh attempt, is what's live).
- Wait for that 30 s countdown to expire, then enter another wrong PIN.
  **Expect:** the next lockout is **60 s**.
- Repeat once more with another wrong PIN after the 60 s countdown expires.
  **Expect:** the next lockout is **120 s**.
- Repeat once more. **Expect:** the next lockout is **300 s** — and any
  further consecutive wrong PINs after that stay capped at **300 s** (they
  do not keep climbing).
- Enter the **correct** PIN once a countdown has expired. **Expect:** the
  device unlocks normally and the attempt counter resets — verify this by
  deliberately entering one wrong PIN afterward and confirming it does
  *not* immediately re-trigger a lockout (only the 5th-in-a-row does).
- This step is the slowest in the procedure (30 + 60 + 120 + 300 = 420 s of
  cumulative countdown, plus the time spent entering PINs) — budget roughly
  10 minutes for it alone.

## Result

Record a pass/fail for each of the six sections above. A failure in any
section is a real defect (or a design gap worth raising) in the screen-lock
feature — file it the normal way; this procedure does not auto-file
anything and queues no follow-on mission of its own.
