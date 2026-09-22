# Provisioning connect/reboot/CLI-hang — device verification kit

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

## Update, 2026-09-22 (round 9): SUPERSEDED MID-ROUND — the screen-lock analysis below was retracted by fresh device evidence before this section could be finalized; see the correction that follows it for the current, correct scope

**This subsection is preserved for the record, not as current guidance —
read the "Correction" subsection immediately below before acting on
anything here.** Round 9 began scoped to the four screen-lock commits
(`9f0a2d2`, `3873c33`, `56edde5`, `77401e5`, 2026-08-22/23) as the
regression-window candidates, on the premise that the entire `site/`
provisioner page changed in only three commits between 2026-08-01 and
2026-09-17. That premise held for the client side but never accounted for
firmware commits outside those four — and new maintainer device evidence
(below) shows the wedge predates all four of them. The analysis this round
originally produced (a suspicion list centered on `lock_store::load()`'s
added boot-time NVS read, `main.rs:1296-1300`, and a proposed "old page vs.
current firmware" localhost test) is **retracted, not merely superseded** —
screen-lock is cleared as a suspect entirely, and the proposed localhost
test re-answers a question that direct device testing already answered the
other way. Kept here, struck through in spirit, so the historical record of
what was believed and when stays intact, matching this kit's convention for
every earlier round's retractions (see round 7/8 above).

### Correction: new device evidence retracts the screen-lock premise — the actual regression window is `v0.6.0..v0.7.0`

Using the web flasher, the maintainer flashed firmware **v0.7.0** and
**v0.6.0** and tested the web provisioner against each, no code changes:
**v0.7.0 behaves like latest `main` (reset + wedge). v0.6.0 WORKS — no
reset at all, status read succeeded.**

This is decisive on three points at once:

1. **Screen-lock is eliminated.** All four commits the original round-9
   scope named (`9f0a2d2`/`3873c33`, 2026-08-22 14:25/14:26; `56edde5`/
   `77401e5`, 2026-08-23) land **after** the v0.7.0 release commit `c6b6b0c`
   (2026-08-22 14:35). v0.7.0 is already broken, so nothing in screen-lock
   can be the cause — drop that analysis entirely, not just its ranking.
2. **The client-side artifact-split question is already answered, the
   opposite way from what the original scope anticipated — do not build a
   localhost kit to re-ask it.** The maintainer ran the CURRENT provisioner
   page (served from the same deployed site as the web flasher) against OLD
   v0.6.0 firmware, and it worked. Current page + old firmware = works ⇒
   **the regression is firmware-side and the client/page is eliminated
   too.** (One assumption worth stating, not re-testing: the page used was
   the current deployed one, since the flasher and provisioner are served
   from the same site.) Step 5 below records this instead of re-deriving it.
3. **The DTR/RTS-triggered reset is FIRMWARE-CONTROLLED, not immutable
   hardware behavior — this retracts a framing every prior round in this
   kit (including round 8) carried unquestioned.** On v0.6.0, the browser's
   unavoidable `open()` DTR/RTS assert produces **no reset whatsoever**.
   Every earlier round in this document treated "Chromium's `open()` always
   resets this chip" as a fixed hardware fact about the CH343/native-USB
   wiring. It isn't fixed — something in `v0.6.0..v0.7.0` changed *whether*
   that assert resets the chip at all, and that is the actual bug.

**New, tighter window: `v0.6.0..v0.7.0`** — 32 commits (13 on
`--first-parent`, i.e. 13 merged PRs), spanning 2026-08-03 → 2026-08-22.
Content is battery SoC filtering/indicator, GPS backup-RTC sync, DM/room
delivery-state model + send auto-retry, clock-source provenance, raw-
millivolt telemetry, and UI header work — nothing obviously USB- or
console-related, which is itself notable. `firmware/src/main.rs` changed by
~1012 lines in the range, the largest single surface.

**Sharper good/bad criterion, replacing "does status succeed":** v0.6.0
answers cleanly with **no reset at all** — watch the serial monitor for a
boot banner immediately after clicking Connect, rather than waiting out a
retry budget or cross-checking the host CLI. This is both faster per cycle
and a cleaner binary signal for bisecting than step 1's original
PASS/FAIL, which was written for a device that always resets and sometimes
also wedges; see step 6 below.

### What this pass already ruled out in-container (do not re-check these)

- `firmware/sdkconfig.defaults` is byte-identical across `v0.6.0..v0.7.0`.
- `firmware/Cargo.lock`'s only change for `esp-idf-hal`/`esp-idf-svc`/
  `esp-idf-sys` is absent — the only version-string changes in the lockfile
  are this workspace's own `firmware`/`firmware-core`/`protocol` crates
  bumping `0.6.0` → `0.7.0`, not a dependency bump. Rules out an ESP-IDF
  toolchain-version drift silently changing a USB-auto-reset default.
- `usb_serial_jtag_driver_install`/`esp_vfs_usb_serial_jtag_use_driver`/the
  RX/TX line-ending calls (`main.rs:646-689`) are present and **byte-for-
  byte unchanged** in both tags — the driver-install call itself, and its
  `usb_serial_jtag_driver_config_t` (only `tx_buffer_size`/`rx_buffer_size`
  fields — no DTR/RTS-reset-arming field exists in that struct at all), are
  not the delta.
- **Peripheral/driver bring-up ORDER in `run()` is structurally unchanged**
  — every numbered boot-sequence comment (`// 1.5.`, `// 2.`, `// 2.5.`,
  `// 2.6.`, `// 4.`, `// 6.`, `// 6.5.`, `// 2.7.`, `// 7.`, `// 8.` …) sits
  at the same relative position in both tags. Rules out "a peripheral now
  initializes before/after USB-Serial-JTAG when it didn't before" as a
  category outright — nothing reordered, only content *within* existing
  steps grew.
- Grepping the full `v0.6.0..v0.7.0` diff for `usb`/`jtag`/`dtr`/`rts`
  (case-insensitive) across `firmware/` turns up exactly one hit, a doc-
  comment-only change in `admin_server.rs`'s `FRAME_QUERY_ADVERT` handler
  (wording about "USB-only, host-driven" — no code changed) and one
  unrelated `RTS unused` comment on the GPS UART1 pin config (UART hardware
  flow control, not USB DTR/RTS). **No firmware code anywhere in this crate
  explicitly arms, disarms, or configures DTR/RTS-to-reset behavior** — on
  this source-only pass, that behavior is inherited entirely from the
  ESP32-S3 boot ROM / USB-Serial-JTAG peripheral, not from any call site
  this codebase controls. This is the honest limit of what a no-hardware
  pass can determine — see "What this pass cannot determine" below.
- `firmware/src/dispatcher.rs`'s `OutstandingSends`/DM-delivery-state
  refactor and every UI-only change (`ui/mod.rs`, `ui/screens/*.rs`,
  `battery_indicator.slint`) grep clean for `usb`/`jtag`/`dtr`/`rts` too —
  message-ACK tracking and rendering, no USB-adjacent code path.

### Ranked suspicion list — UNCONFIRMED, suspects only, not findings; ordered for bisect priority

1. **(weak-moderate, top lead)** `firmware/src/battery.rs`'s added
   `settled_mv` NVS persistence (`load_persisted_settled_mv`,
   `battery.rs:57-75`; call site `main.rs:1588`, `BatteryDriver::new`),
   landed in **`2508553`** (PR #159, `meshcadet-battery-soc-filtering`,
   chronologically the second commit after `v0.6.0`). Adds one
   `EspNvs::new` + `get_u32` call to the same synchronous, pre-`admin_server`
   boot block that already existed (`admin_server` thread spawns at
   `main.rs:1748-1768`, strictly after `BatteryDriver::new` returns) —
   structurally the same shape as every "boot-time NVS work that could
   delay the RX loop" category this kit has flagged before. Same caveat as
   ever: a single extra NVS open+read is a sub-millisecond-to-few-
   millisecond addition, which is a weak mechanism for turning a
   *non-resetting* DTR/RTS assert into a resetting one — nothing about
   *timing* should change *whether* a reset fires. Ranked #1 only because
   it is the most concrete, narrowly-attributable boot-path addition found,
   not because the arithmetic makes a strong case for it.
2. **(weak, second lead)** `firmware/src/gps.rs`'s new backup-RTC-cell
   pre-fix clock sync (`settimeofday` now callable from an unverified,
   pre-fix `$GPRMC`/`$GNRMC` sentence within seconds of boot, not only from
   a verified outdoor fix — `gps.rs:1172-1200`, `set_system_clock_from_utc`
   at `gps.rs:1282`), landed in **`f07eded`** (PR #158,
   `meshcadet-gnss-backup-rtc-prefix-time-sync`, the FIRST firmware-
   touching commit after `v0.6.0`). This is a genuinely new category of
   boot-time behavior — an early, unrequested wall-clock jump via
   `settimeofday` that is now much more likely to fire in the first few
   seconds after boot than before. Speculative mechanism: if anything in
   the runtime computes a deadline from wall-clock time
   (`CLOCK_REALTIME`/`gettimeofday`) rather than a monotonic clock, an
   abrupt jump could desync a timeout. **No call site was found** linking
   `admin_server`'s read loop, the USB-Serial-JTAG driver, or anything on
   that thread to wall-clock time — `grep`ing `firmware/src/*.rs` for
   `SystemTime`/`gettimeofday` outside `gps.rs` itself turns up nothing.
   Ranked below the battery lead because the runs entirely on the MAIN
   thread's dispatcher loop (which only starts after `admin_server`'s own
   thread is already spawned and listening), not on any path between boot
   and the RX loop's readiness — its plausible blast radius, if any, is
   runtime timeout behavior, not connect-time boot latency. Flagged because
   it is the only OTHER genuinely new boot-adjacent behavior found in the
   window, not because a mechanism was identified.
3. **(no evidence found)** every other commit in the range — the DM/room
   delivery-state model, send auto-retry, clock-source provenance
   (`a3bb136`, layered on top of #2's mechanism but adds no new
   `settimeofday` call site of its own), header/icon UI alignment, and the
   `ci-fix-meshcadet-changelog-vocabulary-leak` commit (`ba7a834`, touches
   only `.github/workflows/ci.yml` — no firmware code at all) — grep clean
   for anything USB/JTAG/DTR/RTS-adjacent, per the ruled-out list above.

### What this pass cannot determine — and why that's the honest conclusion, not a gap

This is a source-only, no-hardware pass, and it hits a real limit here: no
code in this firmware crate visibly arms or disarms DTR/RTS-triggered
chip reset, so a diff read alone cannot explain **why** v0.6.0 doesn't
reset while v0.7.0 does. That is exactly the question step 6's bisect below
is built to answer mechanically, with real hardware, rather than something
a second, deeper source read is likely to resolve — repeating the
"diagnose from source, then discover a device-only mechanism no read could
see" pattern this whole kit's earlier rounds (2-8) already worked through
more than once would not be a responsible use of another in-container pass.
See steps 5 and 6, after the original hardware-verification steps below,
for the artifact-split answer already in hand and the mechanical bisect
procedure this round's analysis feeds into.

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

## 5. The artifact-split question — already answered, do not rebuild a kit to re-ask it

The original round-9 scope planned an "old page vs. current firmware"
localhost test as this kit's second deliverable. **Skip it — it has already
been run, decisively, and answered the opposite way from what that scope
anticipated.** See the round-9 "Correction" section above, point 2: the
maintainer ran the CURRENT provisioner page against OLD (v0.6.0) firmware
and it worked cleanly. Current page + old firmware = works, which by itself
eliminates the client/page as a suspect — no localhost checkout, no
`http.server`, no second Web-Serial permission grant needed to re-derive
that. If a future round ever needs the complementary direction (old page +
current firmware) for some other reason, the mechanics are simple (`site/`
is build-step-free ES modules; Web Serial accepts `localhost` as a secure
context) but there is no open question left that direction would resolve
right now.

## 6. Firmware-side git-bisect procedure over `v0.6.0..v0.7.0` (hardware + firmware build required, maintainer-run only)

The container this analysis ran in has no Xtensa toolchain — every step
below needs your own machine.

**Predicate, every cycle, identical (see the round-9 section's "Sharper
good/bad criterion" above):** flash the candidate commit, watch the serial
monitor, click **Connect** in the CURRENT (HEAD) web provisioner. **`good`**
= no reboot banner appears at all. **`bad`** = a reboot banner appears (a
wedge, per round 8, is expected to follow, but the reset itself is the
faster, cleaner signal to bisect on — no need to wait out a retry budget or
run the host CLI cross-check per cycle; save that for confirming the FINAL
first-bad commit once bisect converges).

### 6a. Quick-check order — test the ranked suspicion list before a blind bisect

Cheaper than a full binary search, and uses this round's own analysis
rather than discarding it. Test these two commits directly first, in
order, each a self-contained build+flash+test cycle:

```sh
cd <your meshcadet checkout>
git checkout 2508553   # PR #159 — battery settled_mv NVS persistence (suspicion #1)
cd firmware && cargo run --release
```
Serve `site/` from a FIXED checkout pinned at current `main` (not the
commit you're bisecting — see 6b's note on why) and click Connect.

- **Resets:** suspicion #1 is confirmed as (at least) sufficient — the
  regression is present by this commit. Move on to `f07eded` (PR #158, GPS
  backup-RTC sync) to check whether it alone is ALSO sufficient, or whether
  `2508553` is the actual first-bad commit; either way, report which.
- **Does not reset:** suspicion #1 is cleared. Try `f07eded` next
  (`git checkout f07eded`, rebuild, reflash, retest) the same way.
  - **Resets:** suspicion #2 confirmed as (at least) sufficient.
  - **Does not reset either:** both leads in the ranked list are cleared —
    proceed to 6b's full bisect with no shortcut; the first-bad commit is
    somewhere else in the 32-commit range.

### 6b. Full bisect (only if 6a doesn't land on a bad commit)

**Exact anchors — the 13 first-parent (PR-merge) commits in this window,
oldest to newest, each already a coherent, mergeable, buildable unit (this
vehicle never squash-merges — see the vehicle's PR policy — so every commit
on `main` already passed its own PR's CI):**

```
ba7a834  PR #157  ci-fix-meshcadet-changelog-vocabulary-leak       (.github/ only — not firmware)
f07eded  PR #158  gnss-backup-rtc-prefix-time-sync                 (suspicion #2)
2508553  PR #159  battery-soc-filtering                            (suspicion #1)
e6a0019  PR #161  battery-glanceable-indicator
aa6dbc0  PR #163  battery-level-reads-full-when-depleted
3ce2ea2  PR #162  drop-comment-icon-from-messaging-header
a3bb136  PR #166  clock-source-provenance-and-sync-age
d6585ae  PR #164  header-icon-edge-alignment
0315458  PR #165  dm-room-delivery-state-model
d6eb9d0  PR #167  dm-room-send-auto-retry
60abad2  PR #168  messaging-status-icon-vertical-alignment
208f45f  PR #169  telemetry-raw-mv-over-air
f3803c5  PR #160  release-please branches main                     (newest — this is v0.7.0)
```

`git bisect` always walks the full commit graph — there is no built-in
"first-parent only" mode. To bisect at PR-merge granularity (13 candidates
instead of 32, using the coherent, already-CI-passed anchors above), skip
`git bisect` itself and just `git checkout` each SHA from the ordered list
directly, in the same halving order `git bisect` would use (start at the
middle: `a3bb136`) — see "Each cycle" below. If a PR-level "bad" result
ever needs localizing to one commit WITHIN that PR, run `git bisect` proper
(`git bisect start`, then `git bisect bad <bad-sha-inside-PR>` and
`git bisect good <good-sha-inside-PR>`) over that PR's own smaller commit
range instead.

**Each cycle (mechanical, identical every time):**
1. `git checkout <candidate-sha>` from the ordered list above (start at the
   middle: `a3bb136`).
2. **Serve `site/` from a SEPARATE, FIXED checkout pinned at current
   `main`** for the entire bisect (e.g. a second `git worktree add
   /tmp/meshcadet-current-site main`) — this isolates firmware as the only
   variable across cycles; do not serve the bisected commit's own `site/`
   (already established client-side-clean, and it would move at every step,
   confounding the result regardless).
3. Build and flash: `cd firmware && cargo run --release`.
4. Click Connect in the browser (pointed at the fixed-`main` `site/` from
   #2), watch the serial monitor. Apply the predicate above.
5. Move to the next candidate by binary search over the ordered list (half
   the remaining range each cycle) until two ADJACENT commits in the list
   disagree — that boundary is the first-bad PR merge.
6. Confirm the boundary with `git log <good-sha>..<bad-sha> --oneline` to
   see exactly which PR it is, then run step 1's FULL procedure (including
   the host-CLI wedge cross-check) against that specific commit once, to
   confirm the round-8-documented wedge shape follows the reset — not just
   that A reset occurred.
7. `git checkout main` (or your working branch) when done — no detached-HEAD
   state to clean up since this uses plain `git checkout`, not `git bisect`,
   for the PR-level search.

**Off-ramp — read before you start blaming a commit:** if `v0.6.0` itself
(the chosen good anchor) ever reset on a re-test, or if every commit in the
range resets, **stop bisecting this window** — the regression predates
`v0.6.0`, or isn't a code regression in this repository at all (a host-side
OS/kernel `cdc_acm` driver update, a Chromium update, a different
cable/hub — see the round-9 section's "What this pass cannot determine"
above). Re-test `v0.6.0` alone first if this happens; a stale flash or a
different USB port/cable than the original v0.6.0 test is a far more
likely explanation than the window itself being wrong, given the
maintainer's own v0.6.0 test already showed a clean no-reset connect once.

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

round 9 artifact-split (step 5 — already answered before this kit update;
  recorded for completeness, not re-run):
  current page + v0.6.0 firmware: worked (no reset) — maintainer-run,
    pre-dates this kit revision — client/page eliminated as a suspect

round 9 firmware bisect over v0.6.0..v0.7.0 (step 6):
  6a quick-check order — 2508553 (PR #159, battery settled_mv NVS,
    suspicion #1): resets | does not reset | not run
    (if not run or cleared) f07eded (PR #158, GPS backup-RTC sync,
    suspicion #2): resets | does not reset | not run
  6b full bisect run at all? yes | no
  (if yes) first-bad PR-merge commit found (from the 13 first-parent
    anchors listed in step 6b): <sha, e.g. f07eded/2508553/other — name it>
  (if yes) confirmed with step 1's full procedure (reset AND the round-8
    wedge shape, not just a reset)? yes | no
  off-ramp hit (v0.6.0 itself reset on re-test, or the whole range
    resets)? yes | no — if yes, do NOT report a first-bad commit; report
    the re-test of v0.6.0 instead

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
