# Provisioning connect/reboot/CLI-hang — device verification kit

**ROOT CAUSE CONFIRMED, 2026-09-23 (round 11) — read "Update, 2026-09-23
(round 11)" below before anything earlier in this file.** The campaign's
actual defect is a device-confirmed `pthread` stack overflow in
`admin_server` during `FRAME_QUERY_ADVERT` (an Ed25519 sign plus two NVS
round-trips against an unmeasured, too-tight stack budget). Every earlier
round (4 through 10) diagnosed a real but recoverable symptom that sat in
front of this one on the connect path — the DTR/RTS reset, the host-side
USB wedge it leaves behind, and a stale dev-flash bootloader — and clearing
each in turn is what let a session finally survive long enough to reach
the real crash. `rst:0x15 (USB_UART_CHIP_RESET)` and the bootloader/IDF
skew are demoted to nuisance/build-hygiene respectively; round 8's
unplug/replug guidance stays correct as a recovery action for a real, but
non-root-cause, consequence.

**Round 8's "terminal round" framing was premature — see "Update,
2026-09-23 (round 10)" below.** Round 10 found and fixed a second,
independent defect (the local dev-flash path was writing a stale,
IDF-mismatched bootloader) whose device evidence materially changed
observed behaviour, but left a later, unexplained mid-session reset in
place. The wedge is NOT confirmed solved; do not read round 8's language
below as the campaign's final word.

**HOST-SIDE ROOT CAUSE CONFIRMED, 2026-09-22 (round 8, the campaign's
terminal round) — see "Update, 2026-09-22 (round 8)" below before reading
anything earlier in this file as current.** Round 7's "the device
re-enumerates" mechanism is REFUTED by direct kernel evidence
(`journalctl -k` shows no enumeration event at all across a reproducing
connect); the confirmed mechanism is that the broken state lives in the
HOST's own per-device USB/cdc_acm state and survives a full device-side
chip reset, cleared only by the host kernel re-enumerating the device
(physical unplug/replug, or the `/sys/.../authorized` equivalent — never by
reopening the same node, and never by a device-side reset alone). Round 7's
reopen-based host recovery and its "reconnect to continue" browser message
are both retracted as unworkable; round 8 replaces them with accurate
recovery guidance and a fixed descriptor leak. Everything above the
round-8 section is the chronological record of seven earlier rounds of
diagnosis and is kept for history, but several of its findings are
superseded and marked `RETRACTED` inline.

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
**RETRACTED — see "Update, 2026-09-19 (later)" below:** Maintainer evidence
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

**Reasoning correction, round 6 (2026-09-21):** the CONCLUSION above
("the device was never non-functional") is right, but the REASONING —
"screen showed normal app UI" ⇒ "the MCU was running" — is unsound on its
own, and it was load-bearing enough to kill an entire theory family across
rounds #200/#201. An ST7789 (this board's display controller) holds its
last-written frame in its own on-panel GRAM independent of the host MCU;
a halted, crashed, or ROM-bootloader-stuck MCU that had rendered a normal
screen just before halting would look identical, on the screen alone, to
one still running — the screen is not wired to reflect live MCU state, it
only reflects the last SPI write. Round 6 has independent, stronger
evidence for the same conclusion: the device was directly observed to be
**touch-responsive** after a reproduced wedge, not just visually normal —
touch input requires the MCU's UI thread to actually be polling and
redrawing, which a halted MCU cannot do. Use touch/input responsiveness
(or any other live-round-trip check), not screen content alone, to argue
"the device was running" in any future round.

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

**RETRACTED, round 6 (`meshcadet-connect-wedge-round6-transmit-side`,
2026-09-21) — refuted by the exact device-confirmed evidence this finding
asked for.** The reproduction this finding's own timing marker was added to
catch happened: the host CLI printed `host CLI: serial port opened in
201.52us` — so `open()`/`clear()` were not where it hung — and then hung
past that point. `/proc/<pid>/wchan` for the stuck process read
`tty_wait_until_sent`, which is `tcdrain(2)`, called from
`SerialTransport::send -> self.port.flush()`
(`host/src/transport.rs:86` at the time). Reading `serialport` 4.9.0's
`posix/tty.rs` explains why this specific call was the gap this finding
missed: `TTYPort::write` **is** bounded (`wait_write_fd` honors
`self.timeout`), but `TTYPort::flush` calls `nix::sys::termios::tcdrain`
with no timeout on the syscall itself — the `timeout` value there only
bounds the local `EINTR`-retry loop *around* `tcdrain`, not `tcdrain`'s own
wait. `Session`'s 500 ms/10 s deadlines are evaluated only *between*
transport calls (`send_frame` then `recv_frame`), so a `send` that never
returns is never interrupted by them — this finding's own "structurally
must live in `open()`/`clear()` … nowhere else in the call chain lacks a
deadline" claim was simply wrong: `send`'s `flush()` also lacked one, and
that is where the real hang lives. **Fixed this round:**
`SerialTransport::send` now bounds `write_all`+`flush` to `SEND_TIMEOUT`
(3s) on a background thread (see that constant's doc comment in
`transport.rs` for the device evidence and the tradeoff — the underlying
`tcdrain` still can't be interrupted, so this converts the hang into a
diagnosable `anyhow::Error` rather than actually unblocking the syscall).
The generalizable lesson this leaves — a deadline wrapper also fails to
cover a blocking call made *through* it that doesn't itself honor a
timeout, not just the wrapper's own setup call — is flagged for a
process-side follow-up in the maintainer's own notes; not edited from this
mission (vehicle-scoped work only).

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
  **RETRACTED, round 7 (2026-09-22, kernel-evidence-confirmed) — see
  "Update, 2026-09-22 (round 7)" below.** The bytes are not unrelated
  chatter: they are the OLD USB interface's own final output during a
  connect-triggered teardown/re-enumeration, directly caused by this same
  connect attempt. **Refined, round 8:** "re-enumeration" is the wrong label
  (see round 8's retractions) — the bytes are the device's own boot/reset
  traffic (ROM banner etc.) from its confirmed DTR/RTS-triggered self-reset,
  over a USB session that never actually re-enumerates. The
  not-unrelated-chatter conclusion stands; only the re-enumeration framing
  is retracted.

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

## Update, 2026-09-21 (round 6): device-confirmed hang location — the wedge is on the TRANSMIT path, not receive

`meshcadet-connect-wedge-round6-transmit-side` got the first DEVICE-CONFIRMED
hang location this campaign has had (five prior rounds all inferred from
source or from bounded/clean failures — never from an actual process stuck
mid-syscall). With the wedge reproduced:

- The host CLI printed `host CLI: serial port opened in 201.52us` and then
  hung — `open()`/`clear()` are not where it hung.
- `/proc/<pid>/wchan` for the stuck process read `tty_wait_until_sent` —
  `tcdrain(2)`, called from `SerialTransport::send -> self.port.flush()`.
  This is a genuine, unbounded-at-the-syscall-level hang: `serialport`
  4.9.0's `TTYPort::flush` has no timeout on `tcdrain` itself (see the
  retraction of round 5's finding 1, above, for the full mechanism).
- A reflash to clean state still reproduced the wedge on the **next**
  connect — this is deterministic from a known-empty `rx_buf`, not a rare
  accidental planting of stale state.
- The device **resets on web connect and then runs normally** — directly
  observed touch-responsive, app healthy. It is not halted and not in ROM
  download mode (see the reasoning correction above for why "screen looks
  normal" alone can't establish this, and what does).

**This reframes the whole "10560/10473 bytes received, zero valid frames"
trace family (rounds 4-5) as ordinary device log chatter, not a receive-side
parsing defect.** TX (device→host, what the browser/host receive and parse)
and RX (host→device) are independent paths on the device
(`main.rs:663`/`main.rs:676` handle them as separate VFS RX/TX line-ending
configs, and `admin_server`'s RX loop at `admin_server.rs:257-272` reads via
`usb_serial_jtag_read_bytes` directly, entirely separate from anything the
device transmits). If the wedge is on the TRANSMIT path — the host's bytes
never reaching the device, not a bad device reply — then the device's own
periodic log/telemetry output continues completely normally on TX regardless
of the RX-side wedge, and the browser correctly parses zero provisioning
frames out of it because **no reply was ever owed**: the device's
`admin_server`/`provisioning_server` never received the command that would
have triggered one. Rounds 2-5 searched the receive side for a parsing bug
because that is where the symptom (garbage, unparsed bytes) was visible —
but the actual defect was upstream, on the side that was never producing the
bytes an RX-side fix could act on. Round 5's "mechanism 2" (a stuck
small-`plen` candidate in `admin_server`'s `rx_buf`) is no longer needed to
explain this trace and is superseded, though not itself disproven.

**Fixed this round, on the code side only — see "What this fix found and
addressed" below for the full list.** In summary: `SerialTransport::send`
is now bounded (`transport.rs`'s `SEND_TIMEOUT`, 3s), converting the
device-confirmed hang above into a diagnosable error instead of a silent,
permanent block; the browser's `session.js` now reports bytes ARRIVED
per retry attempt (not bytes RETAINED after magic-resync discard — the two
diverge exactly when a busy attempt is 100% non-frame noise) and
hex/ASCII-dumps the first ~512 bytes it discards as noise on every timeout,
so the next reproduction can actually read what that ~10.5KB of traffic
contains instead of just counting it.

**Noted, not fixed this round (out of this mission's explicit scope — the
brief named `site/provisioner/session.js` only): `host/src/session.rs`'s
own `recv_frame` timeout message (`"timeout waiting for response frame
(accumulated {} bytes)"`, `session.rs:213`) has the identical
RETAINED-vs-ARRIVED conflation `session.js`'s `#timeoutMessage` had before
this round's fix — `self.acc_buf.len()` is post-discard, not raw bytes
received.** A future round diagnosing the host CLI's own receive side
(distinct from this round's transmit-side fix) should give this the same
treatment rather than rediscovering the gap from scratch.

**LEADING CANDIDATE for the device-side half of the mechanism — explicitly
UNVERIFIED, not landed as a root-cause claim, and now source-REFUTED (see
below) rather than confirmed:** `admin_server` writes every reply via
`std::io::stdout()` (`admin_server.rs:230`, `send_frame` at
`admin_server.rs:1298-1310`) into the ESP-IDF USB-Serial-JTAG driver's
256-byte TX ring (`tx_buffer_size: 256`, `main.rs:647`), and `send_frame` is
called synchronously from inside the RX-servicing loop
(`admin_server.rs:318-332`). The hypothesis: a host that stops reading
(closed tab, crashed process) leaves that TX ring full; if the write into it
blocked, it would block the SAME thread that drains the RX ring, which would
NAK the device's OUT endpoint, which would explain every host `tcdrain`
hanging forever afterward — and would explain physical-reset-only recovery
(nothing else restarts that thread). **Source analysis this round — no
hardware required, done against the exact ESP-IDF v5.2.2 sources this repo
vendors and builds against
(`firmware/.embuild/espressif/esp-idf/v5.2.2/components/{vfs/vfs_usb_serial_jtag.c,driver/usb_serial_jtag/usb_serial_jtag.c}`)
— REFUTES the "blocks forever" half of this hypothesis:** the driver-backed
write path `admin_server`'s `stdout` uses
(`esp_vfs_usb_serial_jtag_use_driver`, installed at `main.rs:656`) routes
through `usbjtag_tx_char_via_driver`/`usb_serial_jtag_fsync`, both of which
are bounded by `TX_FLUSH_TIMEOUT_US` (50ms, `vfs_usb_serial_jtag.c:63`) —
the ring-buffer send itself (`xRingbufferSend`, `usb_serial_jtag.c:248`)
genuinely honors that tick timeout rather than blocking on some other
primitive underneath it, confirmed by reading the FreeRTOS ring-buffer call
directly. The driver's own top-of-file comment names this as deliberate
design: "the tx routine will fail fast" once a stall is confirmed once, then
silently drops subsequent bytes rather than blocking — worst case, one
`send_frame` call after the host stops draining costs roughly 2×50ms
(one write-byte blocking retry, one flush/fsync wait), not an indefinite
block. **This means the write path, by itself, cannot hang the RX-servicing
thread for more than ~100ms — nowhere near long enough to explain a
persistent, reboot-required wedge.** The admin_server-blocking-stdout
mechanism as originally framed is therefore refuted by source, not
confirmed; no bounded-write/drop-on-overflow hardening is landed in
`admin_server` this round (this round's own brief was to land it
*only if* source analysis confirmed the write path could actually block —
it didn't). What this does NOT do: explain the actual wedge mechanism.
**RETRACTED BY NAME, round 7 (2026-09-22, kernel-evidence-confirmed) — see
"Update, 2026-09-22 (round 7)" below.** Source analysis's refutation above
is now also hardware-confirmed: the device logs continuously and is fully
alive while the host side is wedged, and the real cause is a host/kernel-side
dead file descriptor after a USB re-enumeration, with nothing on the device
side to fix.
~~The real cause of a `tcdrain`-forever on the host side remains open — see
the device predicate immediately below.~~ **No longer open as of round 7** —
see "Update, 2026-09-22 (round 7)" below for the confirmed mechanism. The
device predicate immediately below is left in place for the record; its two
branches are no longer diagnostically live now that the mechanism is
confirmed by kernel evidence directly.

**Device predicate for the maintainer's next hardware session (binding scope
note: this mission ran no hardware tests and did not pass `--host-native` —
this predicate is for the maintainer to run, not this mission):** while the
wedge is reproduced (host CLI hung in `tcdrain`, per the evidence above),
without physically resetting the device, run:
```sh
timeout 30 cat /dev/ttyACM0 | xxd
```
- **Still shows log lines streaming** ⇒ the device's TX path is alive; the
  block is RX-specific (host bytes not being read/serviced, not a
  bidirectional latch). This refutes the admin_server-shared-thread
  candidate above outright (a genuinely blocked RX-servicing thread would
  also stop producing new log output, since logging goes through the same
  serial console lock — `send_frame`'s `crate::serial_console::lock_tx()`,
  `admin_server.rs:1308` — as the reply frames) and points toward something
  further down the USB stack: the OUT endpoint itself NAK'd or halted at
  the peripheral/driver level, independent of whether the servicing thread
  is otherwise healthy.
- **Silent (no output within the 30s window)** ⇒ both directions are wedged
  from the device's side, consistent with — though still not proof of — a
  single shared blocking point taking down both RX servicing and TX log
  output together.

## Update, 2026-09-22 (round 7): ROOT CAUSE CONFIRMED by kernel evidence — the device's USB endpoint tears down and re-enumerates; every prior round diagnosed a symptom of that, not a separate defect

**RETRACTED, 2026-09-22 (round 8) — see "Update, 2026-09-22 (round 8)"
below.** This section's central claim — that the device's USB endpoint
**re-enumerates** on a web connect, and that a fresh `open()` after it
finishes is what clears the wedge — is REFUTED by a later, more careful
maintainer-run test: `journalctl -k` monitored LIVE across a reproducing web
connect shows NO enumeration event at all (no "New USB device found", no
fresh `cdc_acm` attach) — the USB session survives the device's self-reset
completely unchanged from the host's point of view. The `usb 3-2: New USB
device found` / `cdc_acm 3-2:1.0` log lines quoted immediately below were
real kernel output from this round's own session, but — per round 8's
correction — do not correspond to a re-enumeration triggered BY the connect
attempt that wedged the port. The enumeration line was produced by an
**`esptool` reflash** run earlier in the same debugging session — a
genuinely different action that also resets the ESP32-S3 and also produces
USB kernel-log output, but does so by fully detaching and re-attaching the
USB device, which a DTR/RTS-triggered in-place chip reset does not. Round 7
never checked which of the two actions in its session had produced the log
line it was looking at; it took the first enumeration event found near a
failure and treated proximity as causation. Round 8's own decisive test
held `journalctl -kf` across a LIVE reproduction of the specific connect
under diagnosis, with no reflash in the same window, and found nothing.
Kept below for the historical record, not as current fact — see round 8's
RETRACTIONS below for the full list of what this section gets wrong and
why. General lesson: a kernel-log event is not evidence of causation until
you have confirmed which specific action produced it — two different
actions in the same debugging session (a reflash and a connect attempt) can
both reset the same chip and both produce superficially similar USB
kernel-log output, and only reproducing the SPECIFIC action under
diagnosis, in isolation, tells them apart.

`meshcadet-connect-wedge-round7-stale-handle-reenumeration` reproduced the
wedge with the host kernel's own log (`journalctl -k`) captured live across
the failure. This is decisive, first-hand evidence — not source inference,
not a device screen observation, an actual kernel record of what happened
to the USB device — and it explains every open question the previous six
rounds accumulated. Quoted verbatim:

```
usb 3-2: New USB device found, idVendor=303a, idProduct=1001
cdc_acm 3-2:1.0: ttyACM0: USB ACM device
```

captured immediately after the device's own ROM printed:

```
rst:0x15 (USB_UART_CHIP_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)
```

**The mechanism:** opening the port asserts DTR/RTS (Chromium's
`SerialPort.open()` does this unconditionally; the round-6-confirmed
transmit-side hang shows the host CLI's own `serialport`-backed open does
too, despite leaving the lines at tty defaults post-open — the ASSERT during
the open syscall itself is what matters, not the steady-state level
afterward). That DTR/RTS transition resets the ESP32-S3's native
USB-Serial-JTAG peripheral (`rst:0x15`), and the reset is severe enough that
the chip's USB device **fully re-enumerates**: the kernel tears down the old
`cdc_acm` interface and attaches a fresh one — under the exact same
`idVendor`/`idProduct`/node name (`ttyACM0`), so nothing about the resulting
device path looks wrong to userspace. Any process still holding a file
descriptor into the OLD interface — which is unavoidable, since the host CLI
and the browser both send their first command through the handle they just
used to open the port — now holds a **dead handle**: writes into it queue
into a kernel buffer that nothing on the other end will ever drain, because
the peripheral instance that owned the file descriptor's underlying URBs no
longer exists. That is the entire defect. One mechanism, confirmed, explains
every observation this campaign has collected:

- **(a)** The round-6 device-confirmed `tcdrain(2)`/`tty_wait_until_sent`
  block: `write_all` lands in the kernel's TX buffer fine, then `tcdrain`
  waits forever on URBs belonging to an interface the kernel already
  destroyed.
- **(b)** The host CLI printing `host CLI: serial port opened in 337us` and
  then failing: its OWN `open()` call is what asserts DTR/RTS and triggers
  the reset — the successful, fast open it just reported is the handle that
  gets invalidated microseconds later.
- **(c)** The ~10KB-then-silence browser traces (10560 / 10473 / 9990 bytes
  across three separate reproductions — tightly clustered because this is
  deterministic, not noise): those are the bytes the OLD interface delivered
  during its own teardown window — the running app's trailing log output,
  the ROM banner, and part of the fresh boot log — not an unrelated,
  ongoing chatter stream.
- **(d)** The device being perfectly healthy throughout every prior round's
  observation (touch-responsive, LoRa/UI running, `admin_server` reaching
  "admin server thread started" at 2869ms after reset): the device was never
  the problem. It boots clean, every time; only the HOST's handle into it is
  broken.
- **(e)** **"Clears only on a physical reset" — FALSE.** **RETRACTED,
  round 8 — this point is itself wrong, and round 8 RESTORES "physical
  reset required" as a true, load-bearing observation.** It was correct
  from round 2 onward; this round wrongly retracted it mid-session on the
  strength of the (also retracted) re-enumeration claim above. What round 8
  proves is more precise than either version: a **device-side** reset
  (power-cycle, reset button, or the chip's own DTR/RTS-triggered self-reset)
  does NOT clear the wedge on its own — the decisive test's step 4 shows the
  wedge surviving a full device reboot to a healthy running state. What DOES
  clear it is forcing the **host** to physically re-enumerate the device —
  unplug/replug the USB cable (proven directly: the T-Deck stayed on
  battery power and never rebooted across that unplug, yet host->device was
  restored immediately) or the software equivalent
  (`/sys/bus/usb/devices/<dev>/authorized`, never tried live but implied by
  the same host-side mechanism). A **fresh `open()` alone, with no physical
  re-enumeration — round 7's actual claim here — does NOT clear it**: this
  is exactly what round 8's reopen-recovery removal (see below) is
  grounded in, and what the decisive test's step 3 independently confirms
  ("a SECOND browser connect with NO reset at all... ALSO fails").
- **(f)** "It used to work" with no discoverable regression window: no
  `meshcadet` commit is or was ever required to explain this. Whether the
  wedge fires turns on kernel/Chromium USB re-enumeration timing and
  whatever DTR state a previous port holder left behind — not on this
  repo's code, which is why six rounds of `git log -S` / commit-bisection
  framing never found a culprit.

### RETRACTED BY NAME (refuted against real hardware this session)

- **ROM/download-mode entry.** *Refuted:* `boot:0x8 (SPI_FAST_FLASH_BOOT)` is
  a completely normal flash boot, not a download-mode strap, and the
  application returns fully healthy afterward (see (d) above). The device is
  never stuck in the ROM serial bootloader at any point in this mechanism.
- **An `admin_server` TX-ring write deadlock** (round 6's own "LEADING
  CANDIDATE", already source-refuted there — see that section above — and
  now also hardware-refuted). *Refuted:* the device logs continuously and is
  fully alive and responsive while the host side is wedged; nothing on the
  device's own RX-servicing or TX-log threads is blocked at all. The wedge
  is entirely a host/kernel-side artifact (a dead file descriptor), with
  nothing for a device-side fix to address.
- **Boot time exceeding the browser's 10s `RETRY_TOTAL_MS` budget.**
  *Refuted:* the confirmed boot timeline has `admin_server` up and accepting
  commands at 2869ms post-reset — a 3.5× margin under the 10s retry budget.
  A slow boot was never the constraint; a dead handle that no amount of
  waiting within the SAME connection can heal was.
- **A repeating reset loop.** *Refuted:* the captured kernel/serial evidence
  shows exactly ONE `rst:0x15` event followed by one complete, clean boot to
  a healthy running state — not a crash-reboot cycle. (This round's own
  browser-side reboot counter, added below, exists to make a GENUINE
  repeating case visible if one is ever seen in the field — it has not been
  observed yet.)
- **Round 5's reading of the ~10560/10473/9990-byte traces as "the device's
  own unrelated periodic log chatter (GPS/battery/UI-pump ticks)"** (see
  "Update, 2026-09-20 (round 5)" above, point 3). *Retracted:* those bytes
  are not unrelated background chatter — they are the OLD USB interface's
  final output during its own teardown, directly caused by the connect
  attempt itself, per (c) above.

**What this round fixes (client side only — see "FIRMWARE" below for why
the device side is deliberately not touched):**

- `host/src/transport.rs` — the round-6 `SEND_TIMEOUT` timeout is now a
  distinct, downcastable error type (`SendTimedOut`) instead of a plain
  string, so a caller can tell "this specific confirmed failure mode" apart
  from any other transport error without pattern-matching text.
- `host/src/main.rs` / `host/src/session.rs` — `run_with_reset_recovery`
  catches a `SendTimedOut` on the first attempt of a command, waits
  `RESET_SETTLE_DELAY` (3.5s — past the confirmed 2869ms boot-ready mark),
  reopens the port fresh, and retries the SAME command exactly once against
  the new handle. This recovers from the confirmed failure automatically
  instead of merely reporting it; see `host/src/session.rs`'s doc comment on
  `run_with_reset_recovery` for the full mechanism and its unit tests.
  **RETRACTED, round 8 — `run_with_reset_recovery` and `RESET_SETTLE_DELAY`
  have been REMOVED entirely, not repaired.** Reopening the same `cdc_acm`
  node cannot rebuild the kernel's per-device endpoint state (there is no
  re-enumeration for a reopen to "catch up to" — round 8 proves none
  occurs), so this recovery could never have worked; worse, the abandoned
  `send_bounded` worker thread from the ORIGINAL timeout still held the
  port's file descriptor exclusively (`TIOCEXCL`) when the reopen ran,
  observed directly as `cannot open /dev/ttyACM0: Device or resource busy`.
  See "Update, 2026-09-22 (round 8)" below for the replacement (accurate
  recovery guidance + a poisoned-handle fix for the descriptor leak).
- `site/provisioner/session.js` — the ESP-IDF ROM banner (`ESP-ROM:esp32s3`)
  is unmistakable in the discarded (non-frame) traffic on a genuine reset;
  `#scanForRebootBanner` counts its occurrences, and a resulting timeout
  message now reads `... — device rebooted N times during this command; the
  device reset on connect — reconnect to continue.` instead of a generic
  "timeout waiting for response frame" that leaves the cause to be
  reverse-engineered. The browser cannot recover automatically the way the
  host CLI does (Web Serial gives no equivalent of "reopen this exact port
  without a fresh user gesture" — `requestPort()` requires one), so the
  actionable message IS this side's fix.
  **RETRACTED, round 8 — "reconnect to continue" is wrong and has been
  replaced.** A browser-side reconnect is exactly the same "reopen the same
  node" action just retracted above for the host CLI, and fails for the
  identical reason: it cannot force host-side re-enumeration. The message
  now names the accurate recovery action and honestly states that Web
  Serial exposes no way for the page to perform it itself — see "Update,
  2026-09-22 (round 8)" below.
- `site/provisioner/session.js`'s `#logDiscardedPreview` now uses
  `console.warn`, not `console.debug` — six rounds of this campaign carried
  this exact diagnostic dump and nobody read it, because Chrome's console
  filter hides the "Verbose" level by default. A diagnostic written to be
  read by a human during a failure must actually show up at the console's
  default level.

**Missed guard, noted for the record:** `provisioner.js:283` already
registers `navigator.serial.addEventListener('disconnect', ...)`, and it did
NOT fire during this campaign's reproductions — because the device returns
under the same node identity (same `idVendor`/`idProduct`, same enumeration
order) so Chromium's own watcher for THIS port object never sees a
removal it recognizes as "this port disconnected." The one guard already
built for exactly this class of failure sat silent, which is why the user
saw a generic timeout instead of "Device disconnected." No code change
follows from this (the browser's `disconnect` event is keyed on facts
outside this repo's control), but it is worth naming so a future round does
not re-propose it as a fix without first confirming it can fire for this
specific case.

### FIRMWARE: the exact ESP-IDF mechanism, from source — deliberately NOT changed this round

**Established from source, not guessed:** the ESP32-S3's on-chip
USB-Serial-JTAG peripheral treats the CDC-ACM DTR/RTS control lines as a
**hardware** chip-reset trigger — confirmed directly against
`espressif/esp-idf`'s own register definitions at the exact version this
repo pins (`firmware/rust-toolchain.toml` → `esp` toolchain → ESP-IDF
v5.2.2; this container has no Xtensa toolchain installed, so the vendored
copy under `firmware/.embuild/` could not be read locally this session —
the same tag fetched directly from upstream is byte-identical and cited
below by path):

- `components/soc/esp32c6/include/soc/usb_serial_jtag_reg.h` (tag
  `v5.2.2`) defines a `USB_SERIAL_JTAG_CHIP_RST_REG` register with `RTS`
  (bit 0) and `DTR` (bit 1) status flags — "Chip reset is detected from usb
  serial/jtag channel" — **and** a software-controllable
  `USB_SERIAL_JTAG_USB_UART_CHIP_RST_DIS` bit (bit 2): "Set this bit to
  disable chip reset from usb serial channel to reset chip." On the ESP32-C6
  (and, per Espressif forum guidance, the H2), this reset behavior CAN be
  turned off in software.
- `components/soc/esp32s3/include/soc/usb_serial_jtag_reg.h` (same tag) has
  **no `USB_SERIAL_JTAG_CHIP_RST_REG` at all** — grepped directly, not
  inferred. The ESP32-S3's USB-Serial-JTAG peripheral has no
  software-visible register for this behavior: not a status flag, not a
  disable bit, nothing. This matches Espressif's own esptool documentation,
  which states plainly (`esptool` docs, ESP32-S3 advanced options): *"With
  USB-Serial/JTAG, the peripheral interprets the RTS serial control signal
  as a core reset."* — described as an unconditional peripheral behavior,
  with no accompanying software escape hatch, on this chip family.

**Conclusion — and why nothing is landed here this round:** on the ESP32-S3
specifically, there is currently no known ESP-IDF-level (or any other
software-level) way to stop this reset from happening on this hardware —
this is not merely "unverified, might work if compiled"; the register this
round would need to write to disable it does not exist in this chip's
peripheral at all. Even setting aside `firmware/rust-toolchain.toml`'s
Xtensa toolchain not being installed in this container (which alone would
already block a compile-verified change, and landing an unbuildable guess is
precisely the pattern rounds 2-5 repeated), there is no source-confirmed
register write that would constitute a real fix to propose. The durable fix
this objective asked to scope out — "stop USB-Serial-JTAG resetting the
chip on a host DTR/RTS transition" — may not be achievable in firmware on
this SoC at all; if a fix exists, it is more likely a board-level hardware
change (e.g. gating DTR/RTS at the connector, the way an external
USB-UART-bridge board can) than anything `firmware/` can express. The
client-side recovery landed this round (reopen-and-retry on the host,
count-and-report on the browser) should be treated as the durable mitigation
for THIS chip, not a stopgap awaiting a firmware patch that may not exist.

**Confirmed still valid, round 8 (2026-09-22) — carried forward, not
retracted.** This subsection's register-level finding (no
`USB_SERIAL_JTAG_CHIP_RST_REG`/`..._CHIP_RST_DIS` bit exists in the ESP32-S3
peripheral, unlike the ESP32-C6) is about the DEVICE side of the mechanism —
whether the chip resetting on a DTR/RTS transition can be prevented — which
round 8's hardware test never touched or contradicted; the device DOES
reset on a host DTR/RTS transition, exactly as this section establishes,
and that observation is unretracted. What round 8 retracts is a DIFFERENT,
downstream claim: that the device's reset causes the HOST to re-enumerate
(see "Update, 2026-09-22 (round 8)" below) — the device-side trigger this
FIRMWARE section is about, and the host-side consequence round 7
mis-diagnosed, are two different links in the chain. Independent
community-sourced corroboration gathered this round (not a primary source,
included for completeness, not as the basis of the conclusion above): an
ESP32 forum discussion of ESP32-S3 vs. C6/H2 auto-reset notes "later
versions of the USB-Serial-JTAG peripheral (the C6, and iirc H2) have a
function that can [disable RTS-as-reset], but the S3 doesn't" — consistent
with the register-level absence found directly in source above — and a
live `espressif/esp-idf` issue (#13946) documents that the ONLY other
control surface, the `DIS_USB_SERIAL_JTAG`/`DIS_USB_JTAG` eFuses, disables
the peripheral **permanently and irreversibly**, and doing so on a board
with no secondary flash/debug path (this board's situation, unconfirmed
but likely — the T-Deck Plus exposes a single USB-C port) has been reported
to brick the device with no recovery. Burning that eFuse is not a safe
recommendation for this hardware and is explicitly NOT proposed here. Round
8's own firmware scope is therefore: reconfirm this finding still holds
(it does), make no firmware change (there remains no safe one to make), and
stop pointing at "reopen after re-enumeration" as the workaround, since
round 8 disproves that a reopen — of either kind — is what clears the
wedge in the first place.

## Update, 2026-09-22 (round 8, the campaign's terminal round): HOST-SIDE root cause confirmed by a decisive unplug/replug test — round 7's re-enumeration mechanism is refuted, the actual fix ships client-side, firmware stays untouched

`meshcadet-connect-wedge-round8-host-usb-endpoint-state` ran a maintainer
hardware session built around one decisive test, after round 7's kernel-log
evidence turned out to have been mis-attributed (see the retraction inline
in the round-7 section above). Five steps, each building on the last:

1. **Clean baseline.** Power cycle, nothing else touching the port ->
   `cargo run -- --port /dev/ttyACM0 status` SUCCEEDS, full status
   returned, port opened in 886us. The firmware RX path, `admin_server`,
   and the `usb_serial_jtag` driver ring are all fine — this campaign was
   never chasing a device-side defect.
2. **The trigger.** A web-provisioner connect -> Chromium's `open()`
   asserts DTR/RTS, the ESP32-S3 resets itself (ROM prints `rst:0x15
   (USB_UART_CHIP_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)`), and
   host->device delivery is dead from that point on. Unretracted from round
   7 — this device-side reset genuinely happens, confirmed directly, and
   the FIRMWARE section above (carried forward from round 7) explains why
   it cannot currently be prevented on this SoC.
3. **The reset is a trigger, not the failure itself.** A SECOND browser
   connect, with NO reset at all this time (reboot counter reports 0,
   device uptime continuously 100079->102619ms, LoRa TX/RX and NVS all
   working), ALSO fails. If the reset itself were the failure, a
   reset-free connect attempt should succeed — it doesn't, so whatever
   broke persists independently of any further reset.
4. **The broken state survives a full chip reset.** The device rebooted
   itself in step 2 and reached `admin_server`'s "admin server thread
   started" at 2869ms — firmware, driver, ring buffer, and every peripheral
   register all reinitialize on a chip reset — yet host->device stayed
   dead. This rules out EVERY device-side candidate this campaign has ever
   proposed: there is nothing left on the device for a reset to fail to
   clear.
5. **DECISIVE.** With the wedge active, **unplugging and replugging the
   USB cable restores host->device immediately.** The T-Deck runs on its
   battery (charging, 4900mV) — it never lost power and never rebooted
   across that unplug. The only thing that changed was the **host kernel**
   tearing down and rebuilding `cdc_acm`.

### CONFIRMED CONCLUSION

The failure state lives in the **HOST's** per-device USB/cdc_acm state,
and is cleared only by kernel re-enumeration (physical unplug/replug, or
the software equivalent, `/sys/bus/usb/devices/<dev>/authorized` toggled
0 then 1). It is NOT the device, and it is NOT cleared by any device-side
action (power-cycle, reset button, or the chip's own self-reset) — see the
restored point (e) in the round-7 section above.

**RE-SCOPED, round 11 (`admin-server-stack-overflow-fix`, 2026-09-23,
device evidence) — this heading's "the campaign's terminal round" framing
does not hold, but this section's own diagnosis and guidance DO.** This
wedge is real, its localization (host, not device) is correct, and
unplug/replug remains the right recovery action. What was wrong was
treating it as *the reason the provisioner fails* — a session that clears
this wedge and reaches `QUERY_STATUS` can still crash the device later, at
`QUERY_ADVERT`, via a wholly separate, more severe defect (a firmware
`pthread` stack overflow). See "Update, 2026-09-23 (round 11)" below for
the actual root cause this campaign was chasing.

### NOT CONFIRMED — labeled hypothesis, not fact

The PRECISE host-side kernel mechanism was not directly instrumented this
round (no `usbmon`/URB-level trace was captured — the unplug/replug test
proves WHICH MACHINE holds the broken state, not WHICH state). The leading
candidate: an **OUT-endpoint data-toggle/sequence desync**. The device-side
reset reinitializes its own endpoints (FIFOs cleared, DATA0/DATA1 toggle
zeroed), while the host's `cdc_acm` driver retains whatever toggle state it
had before the reset — so every host->device packet the kernel sends after
that point carries a toggle bit the device's freshly-reset endpoint no
longer expects, and gets silently discarded (or NAK'd/ignored) forever.
This explains every observed shape:
- **Unidirectional** — device->host keeps working because the device
  drives IN transfers and generates its own toggle sequence fresh each
  time; only the HOST-driven OUT direction inherits stale host-side state.
- **Survives port close/reopen and process exit** — the toggle state lives
  in the kernel's `cdc_acm`/USB core per-endpoint structures, not in any
  userspace file descriptor; closing and reopening the character device
  node does not touch it.
- **Only re-enumeration clears it** — tearing down and re-creating the USB
  interface (which unplug/replug forces) is the only kernel-level operation
  that resets the host's own recorded toggle state back to a value that
  matches the device's freshly-zeroed one.

This is a labeled hypothesis, presented as the leading candidate, not as
confirmed fact. A future round with `usbmon`/`wireshark`-usb capture across
a reproducing wedge would be the direct test (watch for a host OUT packet's
toggle bit relative to the device's post-reset expectation, and/or a
device-side NAK/stall response to it).

### RETRACTIONS (against round 7's committed artifacts, all corrected this round)

- **(a)** `host/src/transport.rs`'s `SEND_TIMEOUT` doc comment and
  `SendTimedOut`'s runtime error string asserted "a stale handle left
  behind by a connect-triggered USB re-enumeration" and called it
  "confirmed". *Retracted and corrected* — both now state the confirmed
  host-side localization and the labeled (unconfirmed) toggle-desync
  hypothesis, and the error string tells the user the action that actually
  works instead. This was the worst-placed retraction of the three source
  artifacts: it was user-facing, printed directly to anyone who hit the
  wedge from the CLI.
- **(b)** The reopen-based recovery this same type advertised (`main.rs`'s
  `run_with_reset_recovery` calling `SerialTransport::open` again after a
  `SendTimedOut`) **could not work and has been removed, not repaired**:
  `send_bounded` (`host/src/transport.rs`) spawns a thread holding the
  `Arc<Mutex<port>>` and blocks it in `tcdrain` forever on a genuine wedge;
  on timeout the main thread abandons that thread while it STILL holds the
  mutex and the file descriptor, so `serialport`'s `TIOCEXCL` makes the
  in-process retry open fail — observed directly as `cannot open
  /dev/ttyACM0: Device or resource busy`. And even a successful reopen
  would not have helped: reopening the same `cdc_acm` endpoint does not
  rebuild the kernel's endpoint state (see the CONFIRMED CONCLUSION above).
  Replaced with: accurate guidance (unplug/replug, or
  `/sys/bus/usb/devices/<dev>/authorized`) in `SendTimedOut`'s message, and
  a fix for the descriptor-leak half of this bug — a timed-out `send` now
  poisons the `SerialTransport` (an `AtomicBool`, checked before every
  later `send`/`recv`/`flush_input` on the same handle) so a stranded port
  fails FAST instead of leaking one more thread — permanently blocked on
  the same wedged mutex — per subsequent call. Regression tests:
  `send_bounded_guarded_poisons_the_handle_on_timeout_and_later_calls_fail_fast`,
  `send_bounded_guarded_does_not_poison_a_healthy_port`
  (`host/src/transport.rs`).
- **(c)** This document's own round-7 section asserted the same refuted
  re-enumeration mechanism as fact, and its "physical reset required"
  retraction (point (e)) was itself wrong. *Both corrected inline, in
  place, above* — not rewritten from scratch, so the historical record of
  what was believed and when stays intact; see the `RETRACTED, round 8`
  annotations on the round-7 section.
- **(d)** `site/provisioner/session.js`: `REBOOT_BANNER`'s doc comment
  claimed the ROM banner's presence meant the device "re-enumerated out
  from under this session", and `#timeoutMessage`'s reboot-count message
  told the user to "reconnect to continue" — both wrong for the same
  reason as (b)/(a): a browser-side reconnect is exactly the same "reopen
  the same node" action, and cannot force host-side re-enumeration any
  more than the host CLI's retracted reopen could. *Retracted and
  corrected* — both now point at `HOST_WEDGE_GUIDANCE`: the accurate,
  physical recovery action, with an honest statement that Web Serial
  exposes no way for the page to perform the equivalent of
  `/sys/.../authorized` itself (see "Web Serial limitation" below).

### Also fixed this round: `#sendFrame` outside `#sendRecvWithRetry`'s `try`

`site/provisioner/session.js:1193` awaited `#sendFrame` OUTSIDE the
`try` block starting at line 1194, so a bounded-send (write-stall) timeout
escaped `#sendRecvWithRetry` without ever reaching the same reboot-count
enrichment a receive timeout already got from `#timeoutMessage()` — a write
stall after an already-observed device reboot surfaced as a bare "write
stalled" with no context, instead of naming the reboot count and the
accurate recovery action. Fixed by moving the call inside the `try`; no
retry behavior changes (`#fatalError` still short-circuits immediately —
see `#sendFrame`'s and `#sendRecvWithRetry`'s doc comments for why retrying
a write stall was never going to help regardless of which side of the
`try` it sat on). Regression test:
`writeStallAfterObservedRebootReportsRebootContext`
(`site/provisioner/session.smoke.test.mjs`).

### Web Serial limitation — recorded honestly, not buried

Web Serial exposes **no primitive equivalent to
`/sys/bus/usb/devices/<dev>/authorized`** or a physical unplug/replug —
there is no API surface for a page to ask the browser (let alone the OS)
to force host-side re-enumeration of an already-open serial device. This
means: if a future attempt to prevent the ESP32-S3's DTR/RTS-triggered
self-reset in firmware also fails (see the FIRMWARE section above — it
already has, for this round), **the browser provisioner may be
structurally unable to recover from this wedge on this board at all**,
short of asking the user to physically unplug and replug the cable
themselves. This is a real product limitation, not a bug this codebase can
fix — `HOST_WEDGE_GUIDANCE` (`site/provisioner/session.js`) says exactly
this to the user rather than implying a "Reconnect" button click will do
it.

### Acceptance criteria, walked

1. **No committed artifact or error string still asserts the refuted
   re-enumeration/stale-handle mechanism.** ✓ — `host/src/transport.rs`
   (`SEND_TIMEOUT`/`SendTimedOut` doc comments + runtime message),
   `site/provisioner/session.js` (`REBOOT_BANNER` doc comment,
   `#timeoutMessage`'s reconnect claim), and this kit's round-7 section
   (retraction markers added in place) all corrected. Grepped for
   `re-enumerat` / `reenumerat` / `stale handle` / `reconnect to continue`
   across `host/src`, `site/provisioner`, and `docs/` after editing — the
   only remaining hits are inside retraction markers explicitly labeling
   the claim as refuted, never asserting it as current fact.
2. **The reopen recovery is removed and the `send_bounded` descriptor leak
   is fixed, with a test.** ✓ — `run_with_reset_recovery`/
   `RESET_SETTLE_DELAY` deleted entirely from `host/src/session.rs` and
   `host/src/main.rs`; `SerialTransport` poisoning added to
   `host/src/transport.rs` with two new unit tests (see retraction (b)
   above). `cargo test -p host`: 76 (lib) + 73 (integration) + 5 (room
   dispatch) passed.
3. **The CLI's failure message tells the user the action that actually
   works.** ✓ — `SendTimedOut`'s `Display` impl now embeds
   `HOST_REENUM_GUIDANCE` (unplug/replug, or the `/sys/.../authorized`
   equivalent; explicitly states a device-side reset alone does NOT clear
   it and simply re-running the command will NOT help).
4. **The kit records the confirmed host-side localization, the labeled
   mechanism hypothesis, the restored physical-reset observation, and the
   Web Serial limitation.** ✓ — all four are this section, above.
5. **Any firmware change is minimal and explicitly marked
   compile-unverified.** ✓ vacuously — no firmware change was made. The
   FIRMWARE section (carried forward from round 7, reconfirmed above) shows
   there is no known safe ESP-IDF-level change to make on the ESP32-S3
   specifically; the only other control surface (`DIS_USB_SERIAL_JTAG`
   eFuse) is a permanent, irreversible peripheral disable with documented
   real-world bricking risk on boards with no secondary flash path, which
   is not a responsible recommendation for this hardware. Writing an
   unbuildable or unsafe "minimal change" just to satisfy this criterion's
   letter would repeat exactly the pattern (landing an unverified guess)
   this campaign has spent multiple rounds correcting for.

## Update, 2026-09-23 (round 10): the local dev-flash path was writing a stale, IDF-mismatched bootloader — a real build-hygiene defect, fixed, but NOT a claim that the connect wedge is solved

`meshcadet-connect-wedge-round10-dev-flash-bootloader` is a different,
independently-confirmed defect from rounds 7/8's host-side USB state
diagnosis above — round 8's "the campaign's terminal round" framing turned
out to be premature in a different sense than rounds 4/7 were: not a
refuted mechanism, but an incomplete one. There was a second, unrelated bug
sitting underneath it the whole time.

**THE DEFECT.** The project pins `ESP_IDF_VERSION = "v5.2.2"`
(`firmware/.cargo/config.toml`) and esp-idf-sys builds a matching
`bootloader.bin` into `target/<triple>/<profile>/` next to
`partition-table.bin` and the ELF on every build. The local dev flash
path (`firmware/scripts/flash-with-partition-table.sh`, `cargo run`'s
runner) used `espflash flash` to write the app and then repaired only the
partition-table sector (0x8000) with `write-bin`; nothing ever repaired
the bootloader sector (0x0), which `espflash flash` fills with **its own
bundled default bootloader** — a binary that tracks espflash's release
cadence, not this project's ESP-IDF pin. The script's own header and
`firmware/.cargo/config.toml`'s comment both said as much, in words that
treated it as an accepted tradeoff rather than a defect. Meanwhile the
RELEASE path (`firmware/release-container/build.sh`'s `merge_bin` step)
already flashes bootloader@0x0 + partition-table@0x8000 + app@0x10000
correctly — so dev and release flashing had silently diverged, and only
dev was wrong.

**DEVICE EVIDENCE (maintainer-run, 2026-09-22/23).** A boot log from a
device flashed via the (pre-fix) dev path showed:

```
I (27) boot: ESP-IDF v5.5.1-838-gd66ebb86d2e 2nd stage bootloader / compile time Nov 26 2025 12:27:56
...
I (2064) cpu_start: ESP-IDF: v5.2.2
```

A bootloader three minor versions ahead of the project's pin, produced by
no meshcadet build, roughly ten months stale relative to the pinned app —
paired with a `v5.2.2` app. Flashing the project's own bootloader by hand
(`espflash write-bin 0x0 firmware/target/xtensa-esp32s3-espidf/release/bootloader.bin`)
**materially changed device behaviour**: before it, a web-provisioner
connect reset the device immediately and returned zero status data; after
it, status data flows and the failure moves to a later, mid-session reset.

**THE FIX (this round, build-hygiene only, no firmware source change —
this container has no Xtensa toolchain so firmware can't be built here).**
`flash-with-partition-table.sh` now also `write-bin`s `bootloader.bin` to
0x0 (after the `espflash flash` step, before the 0x8000 partition-table
repair, both at espflash's default `--after hard-reset` — an explicit
`--after no-reset` on a `write-bin` call was previously found to fail on
real hardware with "Communication error while flashing device", so that
constraint was preserved, not re-litigated), gained a fail-loud
precondition check mirroring the existing `partition-table.bin` check, and
the two stale comments (the script's own header, and
`firmware/.cargo/config.toml`'s runner comment) were corrected to state
that the project's own bootloader IS now flashed and why bootloader/app
IDF skew is a hazard rather than an accepted default. A recurrence guard
was considered and explicitly declined, with reasoning recorded inline in
`firmware/.cargo/config.toml` (short version: the two IDF-version inputs
are, by construction, always produced by the same build once this repair
step runs; skew can only recur by bypassing the repair step entirely,
which a version-string comparison can't detect any more reliably than the
existing missing-file precondition checks already do, and a firmware-side
boot check is not buildable in this container).

**CONFIDENCE DISCIPLINE — read before treating this as closing the
campaign.** This is a real, independently-confirmed build-hygiene defect,
correct on its own terms, and the device evidence above shows it changes
device behaviour materially. **It is NOT a claim that the connect wedge is
solved.** The mid-session reset that remains after the hand-flash test is
UNEXPLAINED — its reset reason has not yet been read — and this campaign
has already landed refuted "confirmed" claims twice before (round 4, round
7). Reading the mid-session reset reason is the natural next round if this
fix lands and the wedge persists; it is explicitly out of scope here.

**RE-SCOPED, round 11 — the mid-session reset above IS now explained, and
it is a THIRD, distinct defect, not a confirmation of round 10 closing
anything.** See "Update, 2026-09-23 (round 11)" below.

## Update, 2026-09-23 (round 11): ROOT CAUSE CONFIRMED by device evidence — `admin_server`'s `pthread` stack overflows during `QUERY_ADVERT`; the reset chased since round 6 was always a recoverable nuisance one layer up from the real defect

`meshcadet-connect-wedge-admin-server-stack-overflow-fix` closes this
campaign's real defect. Every prior round (4 through 10) diagnosed a real
but recoverable symptom that sat IN FRONT OF this one on the connect path —
the DTR/RTS reset (round 6/7/8), the host-side USB wedge it leaves behind
(round 8), and the stale dev-flash bootloader (round 10) — and each was
cleared or worked around in turn, which is exactly what let a browser
session finally survive long enough to reach the actual crash.

### THE DEFECT (device-confirmed, maintainer-run HIL evidence, 2026-09-23)

A web-provisioner session got through `QUERY_STATUS` successfully — the
host-side wedge was clear, the bootloader/app were IDF-matched — and then
**crashed the device** during `QUERY_ADVERT` (the "share my card" action;
browser-side call chain `site/provisioner/session.js:704` `queryAdvert` ->
`renderCardUri` -> `site/provisioner/provisioner.js:532`):

```
***ERROR*** A stack overflow in task pthread has been detected.
Backtrace: 0x4037823a:0x3fcc3bf0 0x4037c861:0x3fcc3c10 0x4037d53e:0x3fcc3c30
0x4037e706:0x3fcc3cb0 0x4037d670:0x3fcc3ce0 0x4037d666:0x3b4c3f83 |<-CORRUPTED
ELF file SHA256: 7c9382079
Rebooting...
```

followed by a reboot whose reason is `rst:0xc (RTC_SW_CPU_RST)` — **not**
`rst:0x15 (USB_UART_CHIP_RESET)`, the reset every round from 6 onward
chased. This is a different failure mode entirely: a firmware-internal
stack exhaustion, not a USB-peripheral self-reset.

### MECHANISM (source-confirmed)

`admin_server` is spawned with `.stack_size(12288)` (`firmware/src/
main.rs:1766`, pre-fix), and its own boot-time high-water-mark sample
showed 7196 B peak of 12288 B — **5092 B free** (observed in the
maintainer's boot log). The `FRAME_QUERY_ADVERT` arm
(`firmware/src/admin_server.rs:475-501`, pre-fix) then stacks, on top of
that already-thin baseline: an on-stack `card_buf` (134 B —
`MAX_ADVERT_CARD_LEN`, small on its own), an NVS read
(`advert_ts_store::load_last_advert_ts`), an **Ed25519 sign**
(`firmware_core::advert::handle_query_advert` — `curve25519-dalek` +
SHA-512, a large call frame), and an NVS write
(`advert_ts_store::save_last_advert_ts`). The signing call is the real
pressure, not `card_buf` — exceeds the 5092 B margin, overflows.

**Why this was never measured:** `crate::log_thread_stack_hwm` was sampled
in exactly two places — at boot, before the frame loop
(`admin_server.rs:250`, pre-fix), and immediately AFTER a frame is
successfully handled (`admin_server.rs:341`, pre-fix). A frame that
overflows mid-handler never reaches the second sample, so the worst-case
path was structurally invisible to the only instrument watching it. The
spawn site's own (now-superseded) comment conceded exactly this: "12 KiB is
now generous headroom rather than a tight fit — kept at 12 KiB rather than
trimmed back, since no HIL measurement of the new HWM exists yet to size a
smaller budget from."

**Why ten rounds missed it:** every earlier session died at or before
`QUERY_STATUS` — the DTR reset (round 6/7), the bootloader/IDF skew and
host wedge (round 8/10) — so the browser never reached `queryAdvert` until
the flash path was cleaned up enough (round 10) for a session to get that
far.

### THE FIX

1. **Raised `admin_server`'s stack budget 12288 -> 24576**
   (`firmware/src/main.rs`'s spawn-site `.stack_size(...)` call) — doubled,
   mirroring the identical-class fix already applied to the IDF main task
   for its own identity+crypto init path
   (`firmware/sdkconfig.defaults`'s `CONFIG_ESP_MAIN_TASK_STACK_SIZE`
   32768 -> 49152, +50%); doubled rather than matching that ratio because no
   HIL measurement of the QUERY_ADVERT-path HWM exists yet to size a
   tighter number from.
2. **`card_buf` moved off the stack** (heap-allocated, `Box<[u8]>`) — the
   same remedy the `boot-pthread-stack-overflow-fix` mission already
   applied to `ProvisionedConfig` and `config_store`'s blob buffers. Belt
   and suspenders: at 134 B it was never the dominant cost (the Ed25519
   sign is), but it matches the established pattern for this exact hazard
   shape everywhere else in this thread.
3. **A third `log_thread_stack_hwm` sample added inside the
   `FRAME_QUERY_ADVERT` arm itself**, immediately after the sign/NVS call
   completes — this is the instrumentation gap that hid the bug for ten
   rounds, closed as part of the fix, not as an extra. `ADMIN_SERVER_STACK_B`
   was promoted from a `run`-local `const` to a module-level one so both
   `run` and `handle_frame`'s arm can reference the same value.
4. **Every other `admin_server` handler arm audited** for the same shape (a
   large on-stack buffer plus crypto plus an NVS write). Findings:
   `EXPORT_HISTORY`'s per-entry buffer is `MAX_RSP_HISTORY_ENTRY_PAYLOAD + 1`
   = 74 B with no crypto in its call graph; `ADD_CONTACT`/`DEL_CONTACT`/
   `ADD_CHANNEL`/`DEL_CHANNEL`/`ADD_ROOM`/`DEL_ROOM` all persist through
   `persist_or_rollback`/`persist_setting` -> `config_store::
   save_provisioned_config`, whose own blob buffer is already
   heap-allocated (the `boot-pthread-stack-overflow-fix` mission fixed
   this) and none of these arms call into any signing/crypto path.
   **`FRAME_QUERY_ADVERT` is the only handler that combines a large
   on-stack-adjacent call frame (the Ed25519 sign) with an NVS write; no
   other arm shares this hazard shape.** No further handler-arm changes
   are warranted by this audit.

**Compile-unverified.** This container has no Xtensa toolchain; the
firmware change above (`firmware/src/main.rs`, `firmware/src/
admin_server.rs`) could not be built or run here. It is minimal and
syntactically conservative by design. A maintainer will build and flash
from this branch to verify, and the new in-arm HWM sample will report the
actual post-fix headroom on the first `QUERY_ADVERT` of that run.

### RETRACTIONS AND RE-FRAMING

- **(a) `rst:0x15 (USB_UART_CHIP_RESET)` demoted from root cause to
  nuisance.** It is a real, recoverable consequence of Chromium asserting
  DTR/RTS at `port.open()` (unretracted mechanism, rounds 6-8) — but it is
  not, and was never, *the defect this campaign exists to find*. It
  recurs, is recovered from automatically or by unplug/replug, and a
  session that clears it can still crash later via the real defect above.
  See the "RE-SCOPED, round 11" annotations on `host/src/transport.rs`'s
  `HOST_REENUM_GUIDANCE` doc comment and `site/provisioner/session.js`'s
  `HOST_WEDGE_GUIDANCE` doc comment.
- **(b) Round 8's host-side USB/cdc_acm wedge stays CORRECT as a
  diagnosis and as guidance** — unplug/replug remains the right recovery
  action, and a device-side reset genuinely does not clear it. What
  changes is the FRAME: it is a *consequence of any device reset*
  (including the DTR/RTS one), not *the reason the provisioner fails
  overall*. Clearing the wedge and reaching `QUERY_STATUS` was necessary
  but not sufficient — the session could still crash later, at
  `QUERY_ADVERT`, via a wholly separate defect. Re-framed in
  `host/src/transport.rs` and `site/provisioner/session.js`'s guidance doc
  comments (see (a) above); the runtime error strings themselves were
  already accurately scoped to the specific error they report and did not
  need changing.
- **(c) The bootloader/IDF skew is DEMOTED to build hygiene only —
  round 10's fix is real but does not touch this campaign's actual
  defect.** The maintainer's decisive test used the web flasher's UPGRADE
  path (app-only write at `0x10000`, `eraseAll: false`, per
  `site/flash.js:229-236`) for BOTH the v0.6.0 and v0.7.0 comparison, so
  the bootloader was espflash's own v5.5.1 bundled default and the device
  stayed provisioned in BOTH runs — and **v0.6.0 worked with that same
  "bad" bootloader.** The bootloader/IDF mismatch is therefore not, and
  never was, load-bearing for this campaign's failure; round 10's fix
  (matched IDF versions, `mc_hist` actually flashed) is still correct on
  its own terms but must not be described as fixing the connect wedge.
- **(d) A `git bisect` of `v0.6.0..v0.7.0` is NOT recommended.** This is
  cumulative stack creep against an unmeasured budget (see MECHANISM
  above), not a single culprit commit. Two independent, exhaustive source
  passes over those 32 commits (this campaign's own record; see the
  eliminations carried forward below) found nothing that touches USB or
  the admin_server stack directly — a bisect would only ever land on
  whichever commit happened to tip the unmeasured budget over the edge,
  which is not the same thing as identifying a defect to revert. The fix
  is to size and instrument the budget correctly (done above), not to find
  and revert the commit that happened to exhaust it.
- **(e) The DFS (dynamic frequency scaling) elimination stands, for a
  stronger reason than previously stated.** `feat(power): ESP-IDF dynamic
  frequency scaling` (`f433311`, 2026-08-25 01:27 UTC) and the idle-screen
  feature it was bundled with in the earlier evaluation
  (`0ce0f61`, 2026-08-24 01:47 UTC) both **postdate the `v0.7.0` tag
  entirely** (`f3803c5`, 2026-08-22 10:43:33-04:00) — they are not even IN
  the `v0.6.0..v0.7.0` window this campaign's regression lives in, so they
  were never a candidate to begin with, independent of the earlier
  clock-pinning/thread-separation argument (still true, but now
  redundant). The maintainer's own DFS-elimination test (commenting out the
  Rust call while `CONFIG_PM_ENABLE` stayed set in sdkconfig) was
  incomplete for the same reason this note exists: it tested a
  post-v0.7.0 feature against a defect that predates it.
- **(f) Flash-parameter inconsistency — documented as an accepted gap,
  not fixed.** Round 10's bootloader fix reports 80 MHz / clock div:1 in
  its own boot banner (an ESP-IDF default; no explicit
  `CONFIG_ESPTOOLPY_FLASHFREQ`/`FLASHMODE` override in
  `firmware/sdkconfig.defaults`), while `espflash flash`'s app-image write
  (step 1 of `firmware/scripts/flash-with-partition-table.sh`) patches the
  app header with espflash's OWN CLI defaults (previously observed as
  40 MHz / clock div:2), independent of the project's actual config.
  Before round 10 both bootloader and app header came from espflash's own
  bundled defaults and so were mutually consistent (if not
  project-intended); fixing only the bootloader sector introduces this
  mismatch fresh. Left undisturbed rather than patched blind — this
  container has no hardware to confirm a `--flash-freq`/`--flash-mode`
  CLI addition actually resolves it rather than just relocating the
  mismatch, and no functional failure has been observed to trace to it.
  See the comment added at `firmware/scripts/flash-with-partition-table.sh`'s
  header for the full record; a future HIL round should close this with
  the flag/value confirmed on real hardware.

### ELIMINATIONS CARRIED FORWARD FROM PR #210 (closed unmerged, superseded by this branch)

`meshcadet-connect-wedge-round10-followup` (PR #210) reached a bisect/
no-flash-test recommendation this mission's Objective retracts (see (d)
above) — but it also independently re-derived four eliminations whose
value survives that conclusion change. Recorded here so closing #210 loses
nothing:

- **Elimination 1 — the provisioner page is not implicated.** The artifact
  split (site vs firmware release) was already tested on hardware: the
  CURRENT deployed page against OLD v0.6.0 firmware worked. Current page +
  old firmware = works, so the regression was never client-side.
- **Elimination 2 — screen-lock is not in the window.** `9f0a2d2` and
  `3873c33` (2026-08-22 14:25/14:26) plus `56edde5` and `77401e5`
  (2026-08-23) all land AFTER the v0.7.0 release commit `c6b6b0c`
  (2026-08-22 14:35), and v0.7.0 is already broken. This eliminates the
  whole feature by date, a stronger elimination than an earlier round's
  wire-codec-divergence refutation alone.
- **Elimination 3 — the `v0.6.0..v0.7.0` window contains nothing that can
  reach USB.** Verified across two independent source passes:
  `firmware/sdkconfig.defaults` byte-identical across the tags;
  `firmware/Cargo.lock` changes are ONLY the three workspace crates' own
  version strings (`ESP_IDF_VERSION = "v5.2.2"` unchanged);
  `usb_serial_jtag_driver_install` unchanged in both tags;
  `firmware/src/serial_console.rs` unchanged since its import commit;
  `firmware/src/admin_server.rs`, `firmware/src/advert_ts_store.rs`,
  `firmware-core/src/advert.rs`, and `protocol/src/provisioning.rs` are all
  comment-only in range; `firmware-core/src/dispatcher.rs` changes are
  TX-queue tagging/retry constants only; GPIO0/trackball untouched; no
  `esp_restart` added (the only one is `main()`'s fatal-error handler,
  which reports `SW_CPU_RESET`, not `0x15` or `0xc`); no reference to
  native-USB GPIO19/20 in either tag. This is the evidence base for the
  "no bisect" ruling in (d) above.
- **Elimination 4 — device state is not the variable.** The maintainer used
  the web flasher's UPGRADE path (`site/flash.js:229-236`, app-only at
  `0x10000`, `eraseAll: false`) for BOTH the v0.6.0 and v0.7.0 tests, so
  NVS was untouched and the device stayed provisioned and radio-active in
  both. A "factory-fresh vs provisioned" hypothesis is already controlled
  for and should not be re-raised.

### CONFIDENCE DISCIPLINE

The `pthread` stack-overflow crash during `QUERY_ADVERT`, its backtrace,
and the `rst:0xc (RTC_SW_CPU_RST)` reboot reason are **device-confirmed**
(maintainer-run HIL evidence, 2026-09-23). The precise attribution of the
overflow to the Ed25519 sign specifically (rather than, say, the NVS calls
alone) is **source-inferred**, not directly measured — the new in-arm
`log_thread_stack_hwm` sample (added this round) is what will confirm the
actual post-fix headroom once hardware is available, and the backtrace
above can be decoded against a locally-built ELF (`ELF file SHA256:
7c9382079`) for a fully mechanical confirmation of the crashing frame, as a
follow-on once hardware is available.

### Acceptance criteria, walked

1. **`admin_server`'s stack budget is raised and `card_buf` is off the
   stack.** ✓ — `firmware/src/main.rs`'s spawn site (12288 -> 24576),
   `firmware/src/admin_server.rs`'s `FRAME_QUERY_ADVERT` arm (`card_buf`
   now `Box<[u8]>`).
2. **HWM is sampled on the `QUERY_ADVERT` path.** ✓ — new
   `log_thread_stack_hwm` call inside the arm, after the sign/NVS call.
3. **Other handler arms audited with findings reported.** ✓ — see THE FIX,
   point 4, above.
4. **The kit and user-facing strings demote `rst:0x15` and the bootloader
   to their true roles, keep round 8's unplug guidance as a consequence,
   and record why no bisect is warranted.** ✓ — this section plus the
   `RE-SCOPED, round 11` annotations in `host/src/transport.rs` and
   `site/provisioner/session.js`.
5. **The firmware change is labelled compile-unverified.** ✓ — stated
   above, in the source comments at both edit sites, and in the PR
   description.
6. **Pushed to the existing branch, no new PR.** ✓ — landed as an
   additional commit on PR #211 (`jagoda/meshcadet`), not a new branch or
   pull request.

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
  frame (N bytes arrived this attempt, M retained, K bytes arrived total
  this command)` — reworded, round 6, from the earlier `accumulated N bytes
  this attempt, M bytes total this command` phrasing; see `session.js`'s
  `#timeoutMessage` doc comment for why "arrived" vs. "retained" matters —
  or, less likely now that `connect()` no longer calls `setSignals()`, a
  `write stalled — …` message) **rather than hanging with no error at all.**
  **Round 6 also added a console hex/ASCII dump** (`console.debug`, browser
  devtools) of the first ~512 bytes discarded as non-frame noise, printed
  automatically alongside this same timeout — check it if `K` (bytes
  arrived total this command) is nonzero but every attempt still times out;
  it may finally show what that traffic actually contains, something six
  rounds of this investigation have theorized about without looking at.
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
  device side — an error message naming a timeout within **at most ~13
  seconds** (`time`'s real/user/sys line confirms this: up to `SEND_TIMEOUT`
  3s on the very first `send`, round 6, plus `Session`'s existing 10s
  overall retry budget if `send` itself succeeds), never an unbounded hang
  requiring Ctrl-C.
- **Round 6: if the reported wedge reproduces here, expect a NEW error
  shape specifically** — `serial send timed out after 3s (device stopped
  accepting bytes on the transmit path — likely a blocked tcdrain(2); see
  SEND_TIMEOUT's doc comment in transport.rs)`, printed within ~3 seconds,
  **not a hang**. This is the device-confirmed hang this round diagnosed
  (`/proc/<pid>/wchan` = `tty_wait_until_sent` during the original
  reproduction) — `SerialTransport::send` now bounds it. Seeing this exact
  error, quickly, on a reproduction is CONFIRMATION the fix is working as
  designed, not a step-3 FAIL by itself; still capture it (see below) since
  it's the clearest signal yet of exactly where the transmit-path wedge is.
  If the CLI instead hangs with NO output at all past ~13s even after this
  round's fix, that is a genuinely new finding this round's diagnosis does
  not explain — flag it loudly, it would mean the hang moved to yet another
  uncovered call.
- **If it hangs past ~15-20 seconds with no output and no error:** this
  directly contradicts the fixed code's own bounded-retry logic
  (`host/src/session.rs`, and now `transport.rs`'s `SEND_TIMEOUT`) and its
  regression tests
  (`test_query_status_fails_with_diagnostic_against_an_unresponsive_device`,
  `transport::tests::send_bounded_returns_a_diagnosable_error_instead_of_hanging_forever`)
  — which would mean either a different build is running than the one just
  flashed/built, or there's a genuinely new hang mechanism this source read
  missed (e.g. the OS-level serial port read itself blocking
  forever, upstream of anything `Session`/`Transport` controls). Capture:
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

round 6 device predicate (only if the wedge reproduces — see "Update,
  2026-09-21 (round 6)" above; run WITHOUT physically resetting the device
  first):
  `timeout 30 cat /dev/ttyACM0 | xxd` while wedged — result:
    streaming log lines (TX alive, block is RX-specific) |
    silent, no output within 30s (both directions wedged) |
    not run
  step 3's exact error text, if the round-6 `SerialTransport::send`
    bounding fired: <paste — expect `serial send timed out after 3s
    (device stopped accepting bytes on the transmit path — likely a
    blocked tcdrain(2); ...)`, appearing within ~3s, not a hang>
  browser devtools console: did the round-6 discarded-bytes hex dump
    appear on a step-1 wedge/timeout? yes | no — if yes, paste it (this is
    the first time six rounds of this investigation has actually looked at
    the content of that traffic — do not discard this)

round 7 confirmed-mechanism verification — **SUPERSEDED, round 8: round 7's
  reopen-and-retry mechanism was removed (see "Update, 2026-09-22 (round 8)"
  above) because it could not work; the two predicates originally here
  ("did it print ... reopening and retrying ... and SUCCEED" / "reconnect to
  continue") describe behavior that no longer exists. Do not test for
  either. Use the round 8 predicates directly below instead.**

round 8 host-side verification (see "Update, 2026-09-22 (round 8)" above;
  run WITHOUT physically resetting the device — the whole point of this
  round is that a device-side reset does NOT clear the wedge, so a
  physical reset here would corrupt the test):
  step 3 (host CLI), on a reproduced wedge: does the error message name the
    ACCURATE recovery action (unplug/replug the USB cable, or the
    `/sys/.../authorized` equivalent) rather than claiming a re-enumeration
    or offering to retry? yes | no — paste the exact error text either way
  step 3 (host CLI), immediately after: does a SECOND invocation (same
    process not required — a fresh `cargo run ... status`) ALSO fail with
    the same error, confirming the wedge survives an in-process reopen and
    is NOT cleared by simply re-running the command? yes | no
  step 3 (host CLI) descriptor-leak check: after the timeout above, does
    `lsof /dev/ttyACM0` (or `fuser`) show the ORIGINAL (now-exited) `cargo
    run` process gone, i.e. no lingering exclusive hold once the process
    exits? yes | no | not checked
  step 1 (web provisioner), on a reproduced wedge: does the timeout message
    name the reboot count AND state that Web Serial cannot force host-side
    re-enumeration AND tell the user to unplug/replug the cable, rather than
    "reconnect to continue"? yes | no — if yes, what was N? <N>
  DECISIVE unplug/replug test (the actual round 8 finding — reproduce it
    independently here): with the wedge active (host CLI failing per step 3
    above), physically unplug and replug the USB cable, WITHOUT power-
    cycling or resetting the device otherwise, then re-run `cargo run ...
    status`. Does it now succeed? yes | no — this is the single strongest
    confirmation or contradiction of this round's entire conclusion; if
    "no", flag it loudly, everything above is wrong
  did `console.warn`'s discarded-bytes hex dump appear in the browser
    devtools console at its DEFAULT verbosity level (no filter changes)?
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
