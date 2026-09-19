# Provisioning connect/reboot/CLI-hang — device verification kit

**Maintainer-run, one sitting.** Self-contained: no external dependency, no
queued follow-on. This kit diagnosed and fixed everything reachable from
source + host-testable surface, but could not reproduce the two reported
symptoms without a real device. Run this once against real hardware; the
result block at the end tells us whether the fix in hand is sufficient or
whether the defect has a device-only component that needs a fresh diagnosis.

## What was reported (2026-09-16, not yet reproduced against real hardware)

1. **Web provisioner** — when it tries to connect, the device reboots.
2. **Host CLI** — hangs with no result.

## Update, 2026-09-18: symptoms decoupled — this is now two defects, not one

A maintainer ran this on a build containing the `key_len` fix above (PR
#197): **the host CLI hang is gone**, but **the web provisioner still
reboots the device on connect and fails to connect** — unchanged. That rules
out the "one shared cause" framing this kit originally worked from. A
follow-up source read found that the host CLI's fix left DTR/RTS untouched
(`host/src/transport.rs`), but the browser
client's `session.js` never got the equivalent treatment — Chromium's
`SerialPort.open()` asserts DTR/RTS unconditionally, and on this board's
CH343 wiring (EN/IO0) that resets the chip. Fixed: `connect()` now
de-asserts both lines immediately post-open (best-effort — see its doc
comment for why "best-effort" is the honest framing, not a guarantee), and
the existing retry/resync machinery is documented and tested as the
fallback for a reset the de-assert doesn't prevent. **Step 1 below is now
the discriminating test for this specific fix** — see its expanded
"Expect"/"If" branches.

## What this fix found and addressed (source + host-testable surface only)

- **Refuted:** a Rust/JS wire-codec divergence from the recent screen-lock
  feature. The cross-language round-trip conformance suite
  (`node site/provisioner/codec.conformance.test.mjs`, backed by
  `xtask`'s golden-vector generator) passes for all 48 vectors across every
  frame type, including the four new screen-lock frames — the two codecs
  agree field-for-field. The screen-lock commits are also purely additive to
  the wire format (no shared encode/decode/CRC code touched), and both the
  unprovisioned `provisioning_server` and the runtime `admin_server` already
  reply cleanly (`RSP_ERROR`, not a crash) to a lock frame sent against a
  device state that doesn't handle it.
- **Confirmed and fixed:** `protocol::decode_add_channel` accepted an
  `ADD_CHANNEL` frame's `key_len` byte with no validation. Every downstream
  consumer (`channel_hash_var(&secret[..key_len])`, five call sites across
  `provisioning_server.rs` and `admin_server.rs`) trusted it as a valid index
  into a 32-byte `secret` array — a `key_len` outside `{16, 32}` is an
  out-of-bounds slice, which panics the firmware thread, which the ESP-IDF
  panic handler turns into a device reset. This is fixed at the wire
  boundary (`decode_add_channel` now rejects anything but 16/32) and,
  defense-in-depth, at every consumption site (`Channel::key_len_resolved`,
  `firmware-core::config_store`), so a legacy/corrupted NVS blob can't
  reopen it either. **However:** neither the CLI's argument parser nor the
  web provisioner's form validation can ever produce an out-of-range
  `key_len` through normal use — so while this is a real, now-fixed crash
  bug, it is not proven to be the cause of the reported "reboot on connect"
  (which happens before any channel is ever added). Step 4 below tests it
  directly against real firmware.
- **Checked, found already correct:** the host CLI's retry/timeout logic
  (`host/src/session.rs`) already bounds every device round-trip (500 ms
  retry cadence, 10 s overall budget, 5 s per-frame timeout) and returns an
  `anyhow::Error` naming the timeout, rather than blocking indefinitely. A
  new regression test
  (`test_query_status_fails_with_diagnostic_against_an_unresponsive_device`)
  pins this against a transport that never sends a single byte back. If the
  reported CLI hang was a genuine indefinite block (not just "took ~10 s and
  then printed an error, which felt like nothing was happening"), this kit's
  step 3 is where that would show up as a contradiction of the fixed code's
  own test suite — worth flagging loudly if it happens.

A bisect window supplied after the initial diagnosis pass named the
unreleased `v0.7.0..main` DFS/power and on-air-protocol commits as
candidates alongside screen-lock. Each was evaluated, not just noted:

- **Refuted, directly:** dynamic frequency scaling
  (`feat(power): ESP-IDF dynamic frequency scaling`) as a UART/USB-timing
  cause. The commit's own record (`docs/adr/0014-power-policy.md` D8) states
  a HARD-ABORT-style verification that this board's real APB peripheral
  clock is pinned at a fixed 80 MHz in *every* reachable power-management
  mode with the shipped `min_freq_mhz = 80` config — it never itself
  changes at runtime, regardless of which CPU frequency is active. A UART
  baud-divisor-glitch mechanism requires the clock it derives from to
  actually move; this one provably doesn't. Independently: provisioning
  runs over the ESP32-S3's native USB-Serial-JTAG peripheral, not a
  classic APB-clocked UART with a baud-rate divisor at all.
- **Refuted, by thread/peripheral separation:** the idle-screen sleep/wake
  feature (`feat(power): idle-screen enabler`) runs entirely on the UI
  thread against the ST7789 display over SPI2, on its own render-tick
  cadence — a wholly different thread and peripheral from
  `provisioning_server`'s/`admin_server`'s USB-Serial-JTAG read loop. It has
  no code path that touches USB-serial I/O or the provisioning frame
  parser.
- **Refuted, by layer:** `fix(protocol): dedup a transport-coded frame's
  payload correctly` and the `v1.17 currency bump / transport-code RX fix`
  commit both touch `protocol::frame`/`DuplicateFilter` — the on-air
  MeshCore mesh **radio** packet layer (`ROUTE_TYPE_TRANSPORT_FLOOD`
  dedup). Neither touches `protocol::provisioning` (the USB-serial wire
  format this kit is about) at all, and the radio is never initialized
  during unprovisioned first-boot provisioning in the first place (ADR-0002
  §6).

None of the above rules out a device-only cause this source-only pass
cannot see (real USB/DTR reset behavior on `port.open()`, a firmware code
path missed by this read, a timing interaction only visible on real
hardware). That is exactly what this kit is for.

## 0. Setup

**Prerequisites:**
- A LilyGo T-Deck Plus (or whatever board this repo currently targets),
  reachable over USB.
- This branch/PR's code, built and flashed — see the top-level
  [`README.md`](../README.md#2-firmware-flashing-a-t-deck-plus) §2 for the
  one-time Espressif toolchain setup, then from `firmware/`:
  ```sh
  cd firmware
  cargo run --release
  ```
  `cargo run --release` builds, flashes, **and attaches a serial monitor** —
  leave that monitor running for every step below. If it can't auto-detect
  the port, pass `--port /dev/ttyACM0` (or your OS's equivalent).
- The host CLI built from the same checkout:
  ```sh
  cargo build -p host --release
  ```
- A browser for the web provisioner (any Chromium-based browser with Web
  Serial — Chrome, Edge, Brave). See
  [`site/README.md`](../site/README.md) for how to serve `site/` locally if
  you don't already have it hosted.
- **Erase/reset the device to a factory-fresh, unprovisioned state before
  starting**, so step 1 exercises the exact "first connect" path the report
  describes. `espflash erase-flash` (or the board's physical reset-to-factory
  procedure, if the device was already provisioned from an earlier session)
  gets you there; a fresh flash of a clean image is equivalent.

## 1. Web provisioner: does connecting reboot the device?

- Open the web provisioner in the browser (`site/provisioner.html`), with
  the serial monitor from step 0 visible.
- Click **Connect** and select the T-Deck Plus's port in the browser's
  device picker.
- **Best case — Expect:** the page shows device status (pubkey, "0 contacts,
  0 channels") within a couple of seconds. The serial monitor shows normal
  `prov_server: QUERY_STATUS` log lines — **no panic, no reboot banner, no
  gap where the monitor goes silent and a fresh boot banner appears.** This
  means `connect()`'s post-open `setSignals({dataTerminalReady: false,
  requestToSend: false})` de-assert beat the reset pulse outright — the
  ideal outcome, but not the one the fix depends on (see next bullet).
- **Acceptable case — the device still reboots, but the page recovers on
  its own within ~10 seconds** (no manual reconnect needed), and the serial
  monitor shows a full, ordinary boot banner (no panic) before
  `prov_server:` logging resumes and status appears in the browser. This
  means the de-assert did **not** beat the pulse (worth noting explicitly —
  it rules out "post-open de-assert is sufficient" as a claim, even though
  the connect still succeeds), but the reset-tolerant retry path (boot-noise
  resync + `#sendRecvWithRetry`'s 10 s budget) did its job. **This still
  counts as a PASS for step 1** — record which of the two outcomes above
  actually happened in the result block; both are acceptable, but they mean
  different things about whether the de-assert itself is doing anything on
  this specific board/browser/cable combination.
- **FAIL — the device reboots and the page never recovers** (stuck on
  "Reading status…", or a visible error after ~10 s), **or** the connection
  is lost outright (browser reports the port/device gone, "Disconnected."
  fires without user action): capture, verbatim, into the result block
  below:
  - The full serial monitor output from immediately before the click through
    the reboot and (if it happens at all) back to `prov_server:` logging
    resuming (a `panic`/`Guru Meditation Error` line here is the single most
    useful new fact this kit can produce — it names the exact line that
    crashed, and rules out this mission's DTR/RTS diagnosis in favor of a
    firmware-side one).
  - Whether the monitor shows a firmware panic at all, or instead goes
    completely silent and then shows a fresh ESP-IDF boot banner with **no**
    panic text in between (a clean reboot banner with no panic confirms a
    hardware-level reset — DTR/RTS/EN, this mission's diagnosis — rather
    than a firmware crash; a panic instead means a different, firmware-side
    bug, and this mission's fix is not the one to revisit).
  - Whether the browser ever reports the device as physically
    disconnected/re-enumerated (a Web Serial `"disconnect"` event, or the
    port simply vanishing) rather than just going quiet for a few seconds —
    this discriminates a full USB re-enumeration (the case
    `session.js`'s `connect()` doc comment names as **not** handled: the
    open `port.readable`/`port.writable` streams die and only a fresh
    `connect()` — new device picker click — can recover, no automatic
    retry can) from a soft in-place reboot (which the existing retry/resync
    path already tolerates, per the "Acceptable case" above).
  - Browser devtools console output (F12 → Console) for the same window —
    a `DeviceError`/timeout message here should now name the real cause
    (e.g. "device disappeared") rather than a generic "timeout waiting for
    response frame", per this mission's `#fatalError` fix; if it still
    reads as a bare generic timeout after a visible disconnect, that fix
    did not fully hold on real hardware and is worth flagging back.

## 2. Web provisioner: repeat after adding a channel

- With the device still connected (or reconnected), use the provisioner's
  **Add channel** form to add one channel (any valid secret).
- **Expect:** `RSP_OK`, and the channel appears in the channel list. No
  reboot.
- This exercises the `ADD_CHANNEL` path end-to-end with a *valid* `key_len`
  (16 or 32) through the real UI — the normal case the fix in hand must not
  regress.

## 3. Host CLI: does `status` hang?

- With the device connected (fresh terminal, provisioner disconnected so
  they don't fight over the port):
  ```sh
  time cargo run -p host --release -- --port /dev/ttyACM0 status
  ```
- **Expect:** a status readout (pubkey, provisioned=false, 0 contacts, 0
  channels) in well under a second, or — if something IS wrong on the
  device side — an error message naming a timeout within **at most ~10
  seconds** (`time`'s real/user/sys line confirms this), never an unbounded
  hang requiring Ctrl-C.
- **If it hangs past ~10-15 seconds with no output and no error:** this
  directly contradicts the fixed code's own bounded-retry logic
  (`host/src/session.rs`) and its new regression test
  (`test_query_status_fails_with_diagnostic_against_an_unresponsive_device`)
  — which would mean either a different build is running than the one just
  flashed/built, or there's a genuinely new hang mechanism this source read
  missed (e.g. the OS-level serial port read itself blocking
  forever, upstream of anything `Session` controls). Capture:
  - The exact command line and how long you waited before Ctrl-C.
  - `ls -l /dev/ttyACM0` (or equivalent) before and after, to check whether
    the device re-enumerated (renamed port) mid-command — a classic silent
    hang cause if the CLI is still reading from a now-stale file descriptor.
  - Serial monitor output for the same window.

## 4. Host CLI: does the fixed `key_len` hardening hold on real firmware?

This exercises the fix directly against real firmware (see "What this fix
found and addressed" above for why the CLI and web provisioner's own UIs
can't reach this path):

```sh
cargo run -p host --example raw_add_channel_bad_key_len -- --port /dev/ttyACM0
```

- **Expect:** four lines reading `rejected cleanly: device returned error 5:
  add_channel decode error` (the firmware collapses every `ADD_CHANNEL`
  decode failure into this one generic wire message by design — see
  `provisioning_server.rs`/`admin_server.rs`'s `FRAME_ADD_CHANNEL` decode-error
  arm; the specific `KeyLenInvalid` reason is only visible in the host-side
  test suite's mock, not on the wire) — one per invalid `key_len` value
  tried — and the program exits 0. **No device reboot, no hang.**
- **If the device reboots or the command hangs instead:** the fix did not
  take on real hardware (possibly a stale flash — reflash and retry once
  before treating this as a new finding), or there's a firmware-side
  encode/decode path this static read and host-side tests didn't cover.
  Capture the same serial-monitor/console evidence as step 1.

## Result block

Fill in and paste back:

```
provisioning-connect-verification-kit — result
date: <UTC timestamp>
firmware commit: <git rev flashed>
board: <T-Deck Plus / other>

step 1 (web provisioner connect):        PASS | FAIL
  if PASS, which outcome? no reboot at all | reboot, but connect recovers within ~10s
step 2 (web provisioner add-channel):    PASS | FAIL
step 3 (host CLI status):                PASS | FAIL
step 4 (bad key_len hardening probe):    PASS | FAIL

when did provisioning last definitely work (if known)? <date / "unknown">

for any FAIL above, paste verbatim:
- serial monitor output (from just before the action through any reboot)
- browser devtools console output (step 1/2 only)
- exact command + wall-clock time waited (step 3/4)
- (step 1 FAIL only) did the browser ever report the device as physically
  disconnected/re-enumerated, or did it just go quiet and never come back?
```

A clean PASS on all four steps means the fix in hand resolves both reported
symptoms and no further work is needed for this defect. Any FAIL should come
back with this result block attached — the serial monitor panic text (or its
conspicuous absence) is the single fact most likely to turn a second
diagnosis pass from "read the source again" into "here is line X."

**Step 1's "which outcome" line matters even on a PASS.** If every run comes
back "reboot, but connect recovers" and never "no reboot at all", that is
itself a finding worth recording back on the
`meshcadet-web-provisioner-webserial-dtr-rts-reset` mission (or its
lesson/follow-on, if one exists): it means the post-open `setSignals`
de-assert is not, in practice, beating the reset pulse on real hardware —
the fix that is actually load-bearing is the retry/resync tolerance, not the
de-assert, and the doc comment's "may or may not beat the pulse" hedge
should be firmed up to "does not" rather than left open.
