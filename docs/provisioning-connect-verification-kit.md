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
wedge requiring physical intervention).
**RETRACTED — see "Update, 2026-09-19 (later)" below:** Commander evidence
(2026-09-19T14:12Z) refuted the "never a wedge" claim in the sentence above:
post-#201, on this same pre-#200 path, the device still wedged until a
physical reset. Left in place, unedited, for the kit's chronology — read the
later section for the current understanding, not this one. `esptool-js` is
unchanged — it deliberately wants that reset to enter its own bootloader for
flashing.

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

## Update, 2026-09-19 (later): "read timeout after reset" — hardening landed, root cause still open: `admin_server`'s RX buffer gained a full-buffer escape valve, but the mechanism first proposed for it does not hold up

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

**Root cause: not established. The mechanism first proposed here — a false
`PROV_MAGIC` match inside this device's own boot-time log noise — is
directionally impossible and is retracted; see the audit that refuted it
(`meshcadet-provisioner-fix-retraction-audit-20260919-141943195`).** What
*is* landed, and correct on its own terms, is defense-in-depth hardening
against a different, real hazard: a host-originated oversized or desynced
frame permanently starving `admin_server`'s read loop. The two are not the
same claim, and only the second is backed by source.

Why the log-noise mechanism cannot be right: `rx_buf` is filled **only** by
`usb_serial_jtag_read_bytes` (this file's read loop, above), which drains
the driver RX ring `main.rs` installs at boot
(`usb_serial_jtag_driver_config_t`, `main.rs:646-655`) — that ring carries
**host→device OUT transfers only**. This device's own `log::info!`/
`log::warn!` output leaves over the separate VFS **TX** path (`main.rs:676`
disables its LF→CRLF translation specifically because `admin_server`'s
*replies* ride that same TX path). There is no loopback anywhere in
`firmware/src`: a byte the device writes out never reappears in a buffer
the device reads from. The device's own boot log can therefore never enter
its own RX buffer, regardless of how much of it there is or what byte pairs
it happens to contain. The `10560 bytes total this command` figure in the
trace above is a count the **browser's** `#recvFrame` kept of bytes *it*
received from the device — it says nothing about what did or didn't enter
`admin_server`'s RX buffer, and cannot be read as corroborating the
log-noise theory.

Because that mechanism is the only thing this section previously offered to
explain "used to work" (rising boot-log volume raising the odds of a stray
match), that explanation is retracted with it — not replaced by a
narrower version, retracted outright. There is currently no established
account of what changed, or whether anything did.

**What is genuinely landed, and why it is worth keeping regardless:**
`RX_BUF_LEN`'s own doc comment (`admin_server.rs:130-137`) already names a
real host-originated hazard this same escape valve defends against: a
legitimate but oversized or desynced frame stream whose length bytes decode
to a `plen` bigger than `RX_BUF_LEN` will ever hold, or a retry burst that
spans frame boundaries awkwardly. Without a full-buffer escape valve, that
class hits the identical stuck-forever failure mode the log-noise theory
described (`find_magic_start` re-confirms the same match at offset 0 every
iteration, `rx_len` latches at `RX_BUF_LEN`, no `delay_ms` in that arm) —
the failure mechanism downstream of "buffer stuck full" is real and correct
source-reading; only the claim about *how* the buffer reliably gets stuck
full in practice was wrong.

`provisioning_server::run` — the *sibling* loop, used only during
first-boot unprovisioned setup — has carried the equivalent guard since
this repo's very first commit (`git log -S` on its log message pins it to
`ef0374f`, the import commit); `admin_server::run` never had it. There is
no identified commit that removed it or introduced a regression window —
this is not evidence of a "drift between two hand-duplicated loops" that
recently opened, only of an omission in `admin_server` that has existed for
as long as the file has. Mirrored into `admin_server`'s `TruncatedFrame` arm
below as hardening against the real class named above. (Note, added on this
doc pass: the snippet below is `admin_server`'s actual landed arm as of PR
#204/`251f1ee`, which extracted this comparison into a shared
`firmware_core::rx_loop_guard::flush_if_rx_buffer_full` helper called from
both loops — not the hand-rolled inline check this section originally
described when it was first written.)
```rust
Err(ProvError::TruncatedFrame) => {
    if firmware_core::rx_loop_guard::flush_if_rx_buffer_full(&mut rx_len, RX_BUF_LEN) {
        log::warn!("admin_server: RX buffer full with no valid frame — flushing");
    }
}
```
`RX_BUF_LEN`'s own doc comment already records a prior, partial fix to a
related bug class (bumping 64→512 bytes after a long-name edit frame was
found to hang the same way, permanently, with no escape valve) — this
change closes the remaining gap that fix left open for any host-originated
stream, of any size, that manages to desync in a way that reads as an
oversized `plen`.

**This still stands independent of the retraction above:** a
"provisioning session/lock never released" theory raised while chasing this
same symptom needs no session/lock construct — a single, un-scoped shared
resource (the RX buffer, and the one thread reading it) is sufficient to
explain the host-CLI-also-hangs fact *if* something gets that buffer stuck
full. What is now open again is what, on real hardware, actually gets it
there.

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
compile-risk edit as a firmware change can be. **Step 1 below confirms the
fix is at least harmless**, not that it addressed the reported symptom —
those are different claims now that the mechanism above is retracted. With
the fix in hand, step 1 should still land on the "best case" or "acceptable
case" branch on the very first try, every
time, never the wedge case this mission was opened to chase.

## Update, 2026-09-20 (round 5): re-diagnosed from the corrected data-flow direction — one structural fact confirmed, one new mechanism proposed, both unverified against real hardware

`meshcadet-provisioner-fix-retraction-audit` refuted round 4's log-noise
mechanism as directionally impossible. This round re-diagnosed from the
corrected direction (host/browser RECEIVE side, not device RX) against the
settled facts: the device runs normally throughout (normal app UI, never
held in reset), the device→host link carried 10560 real bytes during a
failing `query_status`, the browser parsed zero valid frames out of them,
the host CLI also fails against the same device afterward and clears only on
a **physical reset**, and #201's `setSignals()` removal must not be undone.

**1. CONFIRMED (source fact, no device needed): `SerialTransport::open()` is
the one call in the host CLI's entire path with no deadline.** Every byte
read after a `Session` exists is bounded (`transport.rs`'s 100 ms per-read
timeout, wrapped by `session.rs`'s 500 ms/10 s retry deadlines). But
`host/src/main.rs` calls `SerialTransport::open()` — which includes the
mandatory `port.clear(ClearBuffer::Input)` — directly in `main()`, *before*
`Session::new` ever runs (`main.rs:556`, `transport.rs:66-78`). Neither the
OS-level `open()` syscall nor `port.clear()` carries any timeout of its own.
This means: if the host CLI's reported "hang" is ever confirmed to be a
genuine, unbounded stall (no output, no error, indefinitely), it
*structurally must* live here or in the OS/USB-driver layer beneath it —
nowhere else in the call chain lacks a deadline. This round adds a one-line
`eprintln!` timing marker around the `open()` call (`main.rs`) so a future
reproduction can observe directly whether the CLI ever gets past it: no
output at all pins the hang inside `open()`/`clear()`; a printed elapsed time
followed by a further hang points at a mechanism `Session`'s own bounded
retry logic does not currently explain (worth flagging loudly if it ever
happens, per the existing step 3 guidance below).

**2. LEADING HYPOTHESIS (unverified — needs a device): a genuinely truncated
HOST-sent candidate frame gets stuck in `admin_server`'s (or
`provisioning_server`'s) `rx_buf`, below the size the existing full-buffer
flush guard needs to fire.** The existing guard
(`firmware_core::rx_loop_guard::flush_if_rx_buffer_full`) only fires once
`rx_len` reaches `RX_BUF_LEN` (512 bytes). But a stuck candidate whose
declared `plen` is small and *individually plausible* (a real command's
header, whose remaining payload bytes were lost — e.g. a USB transfer
truncated by an earlier session ending mid-command) sits at offset 0
forever: `find_magic_start` re-confirms the same magic match every
iteration, so nothing is ever discarded, and `decode_frame` just keeps
returning `TruncatedFrame`. Every subsequent host command's bytes — from a
*brand-new* browser tab or host CLI process — get appended behind it and
swallowed the same way, since nothing about a new process/connection resets
the *device's* `rx_buf`/`rx_len` (those are stack-local to
`admin_server::run`'s/`provisioning_server::run`'s own `loop {}`, entirely
independent of the host-side connection). This satisfies every constraint
this round is bound by:
- **Physical-reset-only:** only a full reboot re-initializes that
  stack-local state to empty — no browser/host-side action (tab close, port
  close, process exit) touches it. This is the single most load-bearing fact
  the kit's step 1 already flags, and this mechanism is the first one this
  campaign has proposed that explains it *precisely*, not just plausibly.
- **"Used to work" with no regression window:** no code change is required
  to explain this. It only takes one earlier session's command to be
  interrupted at exactly the wrong USB-transfer boundary to plant a stuck
  low-`plen` candidate — this can happen at any point in the repo's history,
  which is consistent with `git log -S` finding no drift event in the
  admin_server/provisioning_server guard asymmetry (round 4's own finding,
  still valid): the gap here is not that the flush guard is *missing*, it's
  that the guard's own 512-byte threshold is far larger than a handful of
  short retried command frames (`QUERY_STATUS`'s frame is 7 bytes; even 20
  retries over a full 10 s budget is only ~140 bytes) can realistically
  reach within one command's retry window.
- **No `--host-native` dependency.**
- **Explains the "10560 bytes, zero valid frames" browser trace without
  requiring the guard itself to be insufficient:** see point 3 below — the
  10560 bytes are very likely the device's own *unrelated* periodic log
  chatter (GPS/battery/UI-pump ticks), continuing normally because this
  mechanism never touches the UI thread; the browser correctly never
  extracts a frame from them because the device's admin_server genuinely
  never got to decode the incoming `QUERY_STATUS` in the first place — not
  because a real reply arrived and was missed.

**Caveat, same as round 4's fix and for the identical reason:** this
container's `firmware/rust-toolchain.toml` pins the `esp` (Xtensa)
toolchain, not installed here, so this round's firmware change could not be
cross-build-verified either. `cargo test -p firmware-core` (host-testable,
verified above) covers `oversized_plen`'s own logic exhaustively; the two
call sites in `admin_server.rs`/`provisioning_server.rs` are a small,
syntactically uniform insertion mirroring the existing
`flush_if_rx_buffer_full` call convention already shipped and compiling in
both files — about as low a compile-risk shape as a firmware change can
take, but still unverified against the real target.

**This round's landed fix (`firmware_core::rx_loop_guard::oversized_plen`,
mirrored into both `admin_server.rs` and `provisioning_server.rs`'s decode
loops) closes only the "garbage/oversized `plen`" sub-case of this same
hazard family — immediately, without waiting for `RX_BUF_LEN`.** It does
**not** close the "small, individually plausible `plen` whose payload never
completes" sub-case described above: a genuinely truncated real-looking
command header is, by construction, not oversized, so it still has to wait
out the existing 512-byte escape valve (or a device reboot). Closing that
residual case would need a different mechanism — a *staleness*, not a
*magnitude*, check (a candidate that has stopped growing needs to expire,
regardless of how small it is) — which is **not implemented this round**:
it would require new state threaded through the receive loop
(tracking how long the current unresolved candidate has sat at offset 0)
that has not been checked against real hardware, and landing it now would
repeat exactly the pattern this round exists to break (a confident,
untested fix). It is recorded here as the concrete next candidate if the
predicate below confirms this mechanism.

**3. Are the host/browser receive-side `plen` guards
(`host/src/session.rs:244`, `site/provisioner/session.js`'s
`MAX_VALID_FRAME_PAYLOAD_LEN` check, proven by
`oversizedAdvertFrameSurvivesLogNoiseResync`) actually sufficient against a
10 KB boot-log-style burst? Very likely yes, on source-level byte-value
grounds — the false-positive window is narrower than it looks.** The guard
only misclassifies a spurious `"MC"` (0x4D 0x43) match in log text as a real
frame if the byte exactly 4 positions after the match (`buf[4]`, the
length field's high byte) is `0x00` — otherwise `plen` computes to at least
`0x00xx | (0x20..0x7E)<<8`, i.e. several thousand, always past
`MAX_VALID_FRAME_PAYLOAD_LEN` (134). Ordinary printable ESP-IDF log
output — including ANSI color escapes (`\x1b[0;32m`: digits, `;`, `m`, all
ASCII 0x20-0x7E) — essentially never contains a raw `0x00` byte. This is a
source-level argument, not a device-confirmed one (a raw binary dump logged
without hex-encoding could in principle contain a `0x00` at the wrong
offset), but it means the "zero valid frames out of 10560 bytes" trace is
much more likely explained by mechanism 2 above (the device never actually
sent a real reply during that window) than by a guard failure letting real
log noise masquerade as a frame.

**Device-verification predicates for this round, folded into the result
block below (do not create a second kit):**
- **Self-heal-without-reset test (discriminates mechanism 2 from a harder
  latch, e.g. a USB-peripheral-level condition no amount of retrying could
  ever clear):** if the wedge reproduces, do **not** physically reset
  immediately. Instead keep retrying the host CLI `status` command (a fresh
  invocation each time, several minutes' worth, comfortably past the point
  where cumulative retried command bytes across those invocations would
  exceed 512) *before* resetting. If it self-heals on its own without a
  physical reset, mechanism 2 (stuck-below-threshold small candidate) is
  strongly supported. If it does **not** self-heal no matter how long you
  retry, mechanism 2 is refuted and the latch is something this source read
  does not reach (most likely a hardware/USB-peripheral-level condition).
- **Watch for which log line fires, if any**, distinguishing the two
  sub-cases of the same hazard family: `admin_server: oversized plen in
  candidate frame — resyncing` (this round's new guard, firing immediately)
  vs. `admin_server: RX buffer full with no valid frame — flushing` (the
  existing guard, firing only after 512 bytes accumulate) vs. neither ever
  firing (the mechanism is not this hazard family at all). (On a
  factory-reset/unprovisioned device — the state step 0 puts it in —
  `provisioning_server::run` is the loop actually running, not
  `admin_server::run`; expect the identical pair of lines with a
  `prov_server:` prefix instead, `provisioning_server.rs:244` and `:298`.)
- **Confirm whether `host CLI: serial port opened in ...` (this round's new
  timing marker, `main.rs`) ever fails to print during a reproduced hang** —
  decisive confirmation or refutation of the `SerialTransport::open()`
  unbounded-hang candidate in finding 1 above.

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
  read-timeout-after-reset`): watch for the guard actually firing, not just
  for a clean connect.** The `admin_server` RX-buffer-full flush guard is
  landed hardening (see the "Update, 2026-09-19 (later)" section above), but
  the mechanism originally proposed to explain why it would matter — a false
  `PROV_MAGIC` match inside this device's own boot-time log noise — is
  retracted as directionally impossible (the RX buffer only ever receives
  host-sent bytes; see that section). **This means N clean connects in a row
  proves nothing on its own: it cannot distinguish "the fix worked" from
  "the guard never had anything to catch, and the fix is irrelevant to
  whatever the real cause turns out to be."** The only thing that
  discriminates those two is whether the serial monitor ever actually prints
  the guard's own line:
  ```
  admin_server: RX buffer full with no valid frame — flushing
  ```
  Repeat Connect several times in a row (5+), each after a fresh reset if
  the device doesn't already reset every time, watching the serial monitor
  continuously, and record in the result block whether that line appears at
  all, and how many times.
  - **Line never appears, across every run** ⇒ the escape valve never fired.
    A clean run tells us nothing about whether the reported symptom is
    fixed — it only tells us the buffer never got stuck full during this
    test. Report this plainly rather than as a PASS for the diagnosis.
  - **Line appears, and the device recovers on its own instead of wedging**
    ⇒ the guard did catch a real full-buffer condition and the hardening is
    doing real work — genuine, positive evidence, worth reporting as such,
    though still not proof of what put the buffer in that state.
  - **If the wedge case below still reproduces even once** ⇒ this would
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
    "admin_server: RX buffer full with no valid frame — flushing" observed on
      serial monitor? yes | no — if yes, how many times, and on which
      attempt number(s)? <count / attempt#s>
    "admin_server: oversized plen in candidate frame — resyncing" observed on
      serial monitor (round 5's new immediate-reject guard)? yes | no — if
      yes, how many times, and on which attempt number(s)? <count / attempt#s>
step 2 (web provisioner add-channel):    PASS | FAIL
step 3 (host CLI status):                PASS | FAIL
  exact command run: <paste, e.g. `cargo run -p host --release -- --port /dev/ttyACM0 status`>
  "host CLI: serial port opened in ..." printed before the outcome below?
    yes (elapsed: <duration>) | no — printed nothing at all
  outcome: succeeded | failed with a bounded/named error | genuinely hung
    (no output, no error, until Ctrl-C or physical reset)
  wall-clock elapsed before it returned (or before you gave up and
    interrupted it): <seconds>
step 4 (bad key_len hardening probe):    PASS | FAIL

round 5 self-heal-without-reset test (only if the wedge reproduces — see
  "Update, 2026-09-20" above before physically resetting the device):
  number of separate `status` invocations retried before giving up or
    resetting: <N>
  approximate total wall-clock time spent retrying: <minutes>
  did it self-heal on its own, with NO physical reset? yes | no
  if yes: which invocation number did it succeed on? <N>
  if no: did you eventually reset physically to confirm that clears it?
    yes | no

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

A clean PASS on all four steps is evidence the hardening in hand is at
least harmless, but is **not**, by itself, evidence the reported symptom is
resolved — see the retraction in the "Update, 2026-09-19 (later)" section
above for why N clean runs cannot carry that claim on their own. Any FAIL
should come back with this result block attached — the serial monitor panic
text (or its conspicuous absence) is the single fact most likely to turn a
second diagnosis pass from "read the source again" into "here is line X."

**Fill in the RX-buffer-starvation block above, including the flush-line
question, even on a step-1 PASS.** The flush line's presence or absence is
the only thing in this kit that distinguishes "the guard caught a real
full-buffer condition" from "the guard never had anything to catch, and a
clean run says nothing about the reported symptom." Several repeated clean
runs with the line absent every time is not strong evidence the underlying
cause is fixed — it may simply mean this test session never reproduced
whatever puts the buffer in that state. See the "Update, 2026-09-19 (later)"
section above for the retraction and the mechanism that is and isn't
established.

**Step 3's outcome line matters regardless of PASS/FAIL.** `host/src/session.rs`
and `send_recv_with_retry` are both deadline-bounded by construction — a
genuine, unbounded hang (no output, no error, past the ~10-15s budget)
points to something this kit's diagnosis does not reach (a different build,
or a hang mechanism upstream of `Session`, e.g. the OS-level serial read
itself blocking forever). A bounded failure with a named error, even if it
counts as a step-3 FAIL, is a materially different and less alarming finding
than a true hang — report which one occurred, not just PASS/FAIL.

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
