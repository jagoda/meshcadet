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

## Update, 2026-09-19: first real hardware run — the de-assert wedges the device; removed. Three outcomes now, not one.

`meshcadet-provisioner-connect-unbounded-awaits-hang` got the first real
device time any of the three rounds on this defect (#197, #200, this one)
has had. Two datapoints, in order:

1. A failed web-provisioner connect errored with `timeout waiting for
   response frame (accumulated 0 bytes...)` — **not a hang.** The mission's
   own stated Objective (unbounded `setSignals()`/`writer.write()` awaits
   causing an infinite hang) was refuted as the cause of *this* symptom; both
   awaits were still real defects and are now bounded regardless (belt and
   suspenders — see `session.js`'s `#sendFrame`/`disconnect()` doc comments),
   but that was not what a maintainer's own device run actually showed.
2. **Decisive:** after a failed web connect, the **host CLI also hung**
   against the same device — and kept hanging **until the device was
   physically reset.** The host CLI never writes DTR/RTS at all
   (`host/src/transport.rs:51-65`), so this was not residual browser-side
   signal state; something on the device itself had latched. Leading
   mechanism: `setSignals()`'s two line changes are not guaranteed atomic
   below the JS call, and a staggered DTR/RTS transition on this board's
   CH343 wiring (EN/IO0) is indistinguishable from the deliberate
   bootloader-entry toggle sequence `site/flash.js`'s vendored `esptool-js`
   implements on purpose — PR #200 appears to have accidentally reimplemented
   the flasher's enter-download-mode dance inside the provisioner's connect
   path.

**Fix: `connect()` no longer calls `setSignals()` at all** — see its doc
comment for the full history. This returns to the exact pre-#200 path,
which field evidence already showed was reset-and-recoverable (never a
wedge requiring physical intervention). `esptool-js` is unchanged — it
deliberately wants that reset to enter its own bootloader for flashing.

**Severity note, for context on why step 1 below is now the load-bearing
check, not an optional extra:** the currently deployed web provisioner
(Pages, from `main`, since PR #200 merged) put a device into a state
requiring physical intervention to clear. The decision was to fix forward
rather than revert or gate the page — this kit's job is to confirm the
forward fix actually holds.

**Step 1 below now has to discriminate THREE outcomes, not the original
two ("no reboot" / "reboot, recovers"):**

1. **No reboot at all**, or **reboot but the page recovers on its own** — the
   pre-existing two acceptable outcomes, unchanged (see the original 2026-09-18
   branches below).
2. **Wedged link**: the connect attempt now fails with a *bounded, named*
   error (`timeout waiting for response frame (accumulated N bytes this
   attempt, M bytes total this command)`, or — much less likely now that
   `setSignals()` is gone — `write stalled — …`) rather than hanging
   forever. This is the new, first-class check: **after this failure, is
   the device still reachable by the host CLI without a physical reset?**
   If yes, the fix holds even though this particular connect attempt failed
   (a wedged/reset link surfacing a clean, bounded, diagnosable error is the
   correct outcome for a link problem the browser genuinely cannot always
   avoid — see connect()'s doc comment). If no — the device needs a physical
   reset before the CLI works again — **the fix did not hold** and this is
   the single most important fact to report back.

## Update, 2026-09-19 (later): "read timeout after reset" — root cause found and fixed: `admin_server`'s RX buffer has no full-buffer escape valve

`meshcadet-web-provisioner-read-timeout-after-reset` picked this up right
after PR #201 (the `setSignals()` removal above) shipped. **Two maintainer
device datapoints made this investigation's own first two framings dead
ends in turn** (both retractions were earned by evidence, not guessed past)
before the decisive trace landed:

```
timeout waiting for response frame
  (accumulated 0 bytes this attempt, 10560 bytes total this command)
```

— thrown by `#recvFrame`, i.e. a clean, bounded failure, not a hang. Two
more facts pinned it down: the device's screen showed **normal app UI**
throughout (it was never held in reset or in the ROM bootloader — every
theory from rounds #200/#201 that depends on the device being
non-functional is dead), and **the host CLI also failed against the same
device afterward, clearing only on a physical reset** — so whatever this is,
it is a device-side latch, not a browser-only bug.

**Root cause, confirmed by source (not yet cross-build-verified — see
caveat below): `firmware/src/admin_server.rs`'s frame-receive loop has no
escape valve for a full RX buffer stuck on a false frame-magic match.**

This firmware sets `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG=y`
(`firmware/sdkconfig.defaults:121`) — `log::info!`/`log::warn!` output
shares the *same* USB-Serial-JTAG wire the binary provisioning frames ride,
and boot is chatty on it. `admin_server::run`'s read loop (mirroring
`provisioning_server::run`'s, the unprovisioned-boot sibling) resyncs on
`PROV_MAGIC` (`"MC"`) before attempting to decode a frame — but
`find_magic_start` trusts **any** two-byte `4D 43` match unconditionally,
with no check on what follows. Ten-plus KB of boot-time log text is more
than enough for `4D 43` to appear by coincidence, and whatever two bytes
happen to follow it become the frame's `plen` — a value from 0–65535 with
no relationship to the buffer's actual contents. Once `FRAME_OVERHEAD +
plen` exceeds the 512-byte `RX_BUF_LEN`, `decode_frame` returns
`TruncatedFrame` and **can never do anything else** for that candidate: the
same false match re-confirms at offset 0 every loop iteration, so
`rx_len` only grows, until it hits `RX_BUF_LEN` and the `if rx_len <
RX_BUF_LEN` read-gate stops admitting new bytes. The thread is now stuck
forever, spinning `find_magic_start` → `TruncatedFrame` with **no
`delay_ms` in that arm** — it never reads another byte, from *any* client,
until a physical reset re-zeroes `rx_buf`.

This explains every datapoint at once: normal app UI (a separate task/
thread, unaffected by `admin_server` wedging), a clean bounded browser
timeout (bytes did arrive — the 10560 total — they just could never
resync), the host CLI *also* failing (same stuck thread, doesn't care which
client asks), and only a physical reset clearing it (fresh boot re-zeroes
the buffer). It also explains "used to work": this is a **probabilistic**
trigger — the more boot-time log volume, the higher the odds any given boot
happens to contain the `4D 43` byte pair with an oversized trailing length.
Recent PRs on this path (config-store validation in #197, plus ordinary log
growth over time) all plausibly raised that boot-time log volume, raising
the odds of tripping this without needing any single commit to be precisely
at fault. (An early-draft theory pinned this on two specific wire-contract
commits as "immediately preceding #197" — that framing does not hold up
against `git log`'s own timestamps, one of the two postdates #197 by
minutes; not the mechanism below regardless.)

**The fix, already in this branch:** `provisioning_server::run` — the
*sibling* loop, used only during first-boot unprovisioned setup — already
carries the missing guard:
```rust
Err(ProvError::TruncatedFrame) => {
    if rx_len >= RX_BUF_LEN {
        log::warn!("prov_server: RX buffer full with no valid frame — flushing");
        rx_len = 0;
    }
}
```
`admin_server::run` (the loop actually running on every already-provisioned
device — i.e. every device past first-time setup, exactly this mission's
repro scenario) never got it. This was a **drift between two hand-duplicated
loops**, not a deliberate omission — worth remembering next time either file
changes: touch one, check the other. Mirrored verbatim into `admin_server`'s
`TruncatedFrame` arm. `RX_BUF_LEN`'s own doc comment already records a prior,
partial fix to the *identical* bug class (bumping 64→512 bytes after a
long-name edit frame was found to hang the same way) — that raised the bar
for the wedge to trigger without removing the underlying gap, which is
exactly why boot-log noise could still find it.

**This also refutes an earlier "provisioning session/lock never released"
theory** raised while chasing this same symptom: no session/lock construct
is needed to explain the host-CLI-also-hangs fact — a single, un-scoped
shared resource (the RX buffer, and the one thread reading it) explains it
without one.

**Unrelated hardening, found by inspection while diagnosing this, fixed in
the same pass:** the browser's `session.js` `ALL_RSP_FRAME_TYPES` set — the
list of response types `#recvUntilExpected` tolerates as late-but-legitimate
residue rather than treating as corruption — was missing `FRAME_RSP_LOCK`
(0x8D), the response the screen-lock feature (`9f0a2d2`) added. It cannot be
the cause of this mission's symptom (this module has no `queryLock()` caller
yet, so a real session never produces an `RSP_LOCK` frame today), but every
other `FRAME_RSP_*` codec.js defines was already listed — an oversight, not
a deliberate exclusion — so it's fixed regardless, with a new
`session.smoke.test.mjs` regression scenario proving it's now tolerated like
every other recognized type.

**Caveat, stated plainly:** this container's `firmware/rust-toolchain.toml`
pins the `esp` (Xtensa) toolchain, which is not installed here — the same
constraint PR #197's own commit message hit and disclosed. The fix could not
be cross-build-verified in this environment. It is, however, syntactically
and semantically a verbatim mirror of an already-compiling, already-shipped
sibling arm in the same file/crate (`provisioning_server::run`'s identical
guard, five lines above in the same source tree) — about as low a
compile-risk edit as a firmware change can be. **Step 1 below is where this
gets its real confirmation**: with the fix in hand, step 1 should land on
the "best case" or "acceptable case" branch on the very first try, every
time, never the wedge case this mission was opened to chase.

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
  means `open()`'s own forced DTR/RTS assert (the one `connect()` can no
  longer avoid or mitigate — see its doc comment for why it no longer tries)
  did not trigger a visible reset on this run — the ideal outcome, but not
  the one the fix depends on (see next bullet).
- **Acceptable case — the device still reboots, but the page recovers on
  its own within ~10 seconds** (no manual reconnect needed), and the serial
  monitor shows a full, ordinary boot banner (no panic) before
  `prov_server:` logging resumes and status appears in the browser. This is
  the **expected steady-state outcome** now that `connect()` relies purely
  on `#sendRecvWithRetry`'s reset-tolerant retry path (boot-noise resync +
  the 10 s budget) rather than trying to prevent the reset — matches the
  pre-#200, field-proven-survivable behavior. **This counts as a PASS for
  step 1** — record which of the two outcomes above actually happened in the
  result block.
- **RX-BUFFER-STARVATION CHECK (new — `meshcadet-web-provisioner-
  read-timeout-after-reset`): this is now the load-bearing check for this
  fix, the same way step 1's wedge-case branch was load-bearing for #201's.**
  With the `admin_server` RX-buffer-full flush guard in hand (see the
  "Update, 2026-09-19 (later)" section above for the full mechanism), a
  normal connect-after-reset should land on the "Best case" or "Acceptable
  case" branch above **every single time** — never the wedge case below.
  Repeat Connect several times in a row (5+), each after a fresh reset if
  the device doesn't already reset every time, to build confidence this
  isn't just a lucky run: the pre-fix bug was probabilistic (it needed a
  stray `4D 43` byte pair inside that boot's log noise), so a single clean
  pass is weaker evidence than several.
  - **If every run lands clean** ⇒ the fix holds; this is the expected,
    now-confirmed outcome.
  - **If the wedge case below still reproduces even once** ⇒ this was not
    the only contributor, or the fix has a gap — capture everything the FAIL
    branch below asks for, plus the exact serial-monitor line count/content
    right before the device is next queried, and flag it loudly: this would
    mean the diagnosis needs a second pass, not just a bigger buffer.
- **Wedge case — the connect attempt fails with a bounded, named error**
  (in the browser console / on the page: `timeout waiting for response
  frame (accumulated N bytes this attempt, M bytes total this command)`, or
  — less likely now that `connect()` no longer calls `setSignals()` — a
  `write stalled — …` message) **rather than hanging with no error at all.**
  A bounded, named failure here is not automatically a FAIL. Immediately
  after it, **without power-cycling the device**, run step 3 below (host CLI
  `status`):
  - **Host CLI succeeds (or fails with its own bounded timeout, but does NOT
    hang)** ⇒ **PASS.** The link had a bad moment, the browser reported it
    correctly and boundedly instead of hanging, and the device was left in a
    reachable state. Record the exact error text.
  - **Host CLI also hangs, and only resumes after a physical reset** ⇒
    **FAIL — this is the exact regression `meshcadet-web-provisioner-
    read-timeout-after-reset` diagnosed and fixed; reproducing it with the
    fix in hand is the single most important thing to report back.** Capture
    everything below, plus the exact host CLI command and how long you
    waited before the physical reset.
- **FAIL — the device reboots and the page never recovers** (stuck on
  "Reading status…", or a visible error after ~10 s), **or** the connection
  is lost outright (browser reports the port/device gone, "Disconnected."
  fires without user action), **or** the wedge case above's host-CLI check
  fails: capture, verbatim, into the result block below:
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
  which outcome? no reboot at all | reboot, connect recovers within ~10s | wedge case (bounded error, host CLI still reachable)
  (wedge case only) exact browser error text: <paste>
  (wedge case only) host CLI status immediately after, no power-cycle: worked | hung until physical reset
  (wedge case only) does this happen on EVERY connect right after a reset, or only occasionally? <every time / occasional / only tested once>
  RX-buffer-starvation fix (meshcadet-web-provisioner-read-timeout-after-reset):
    number of Connect attempts run: <N, 5+ recommended>
    number that landed clean (best/acceptable case, no wedge): <N>
    number that hit the wedge case: <N, expect 0>
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
- (step 1 FAIL, wedge case only) how long did you wait before physically
  resetting the device, and did the host CLI's own hang ever resolve on its
  own without one?
```

A clean PASS on all four steps means the fix in hand resolves both reported
symptoms and no further work is needed for this defect. Any FAIL should come
back with this result block attached — the serial monitor panic text (or its
conspicuous absence) is the single fact most likely to turn a second
diagnosis pass from "read the source again" into "here is line X."

**Fill in the RX-buffer-starvation block above even on a step-1 PASS.** It
is the confirmation `meshcadet-web-provisioner-read-timeout-after-reset`
needs: several repeated clean runs is meaningfully stronger evidence the
fix holds than one, since the pre-fix bug only triggered when that boot's
log noise happened to contain a stray `4D 43` byte pair — a probabilistic
trigger, not a certainty on any single run. See that mission's "Update,
2026-09-19 (later)" section above for the full root-cause mechanism and the
fix already in this branch (`admin_server.rs`'s `TruncatedFrame` arm).

**Step 1's "which outcome" line matters even on a PASS.** `connect()` no
longer calls `setSignals()` at all (`meshcadet-provisioner-connect-
unbounded-awaits-hang`, 2026-09-19 — see `session.js`'s `connect()` doc
comment for why: the prior post-open de-assert was found, on the first real
hardware run any of the three rounds on this defect has had, to wedge the
device until a physical reset). The retry/resync tolerance is now the ONLY
mechanism handling a reset — if every run comes back "reboot, but connect
recovers" and never "no reboot at all", that is simply confirmation this is
working as designed, not a finding to chase further. The finding that
WOULD matter now is the wedge case actually reproducing — see its own
result-block fields above.
