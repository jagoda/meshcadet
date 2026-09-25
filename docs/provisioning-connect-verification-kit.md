# Provisioning connect/reboot/CLI-hang — campaign closeout

**Case status: CLOSED, 2026-09-24.** This is a closeout report, not an
active investigation. The multi-round diagnostic narrative that used to live
in this file (thirteen rounds, 2026-09-16 through 2026-09-24) has been
retired: every hypothesis it chased through the browser client, the host
CLI, the wire protocol, and the device is either a real, fixed defect (kept
below) or an eliminated hypothesis (kept below, with how it died). Nothing
below is a live theory. If a new symptom appears, it gets a new
investigation — this file is not the place to reopen the old one on a hunch.

## Verified conclusion

The web provisioner, the firmware, the wire protocol, and the device are
**all working**. The same deployed provisioner page, against the same
device running the same firmware, connected and read status correctly from
a Chromebook running native Chrome. The failure reported throughout this
campaign is specific to **one host**: a Pop!_OS machine running Flatpak
Chromium, where the **native host CLI works fine against the same device**
but the **Flatpak-sandboxed browser does not**.

The root cause on that host is **not identified**. What discriminates a
sandboxing/packaging explanation from a host OS/kernel/USB-stack
explanation — native Chromium (`.deb`) vs. Flatpak Chromium, on the exact
same Pop!_OS machine — has not been tested. See "Open next step" below.

## Real defects found and fixed (kept — device-backed)

Thirteen rounds produced six real, independently-confirmed fixes on this
connect/provisioning path. None of them explain the Pop!_OS/Flatpak
failure above — that failure reproduces with all six already in place —
but all six are real defects this campaign is right to have found and
fixed, and none is retracted:

1. **`admin_server`'s `pthread` stack overflow on `QUERY_ADVERT`.**
   Device-confirmed (`***ERROR*** A stack overflow in task pthread has been
   detected`, `rst:0xc RTC_SW_CPU_RST`). Fixed by raising
   `admin_server`'s stack budget 12288 → 24576 bytes
   (`firmware/src/main.rs`'s spawn site), heap-allocating `card_buf`
   (`firmware/src/admin_server.rs`'s `FRAME_QUERY_ADVERT` arm), and adding a
   high-water-mark sample inside that arm. Verified on hardware at 7212 B
   peak of 24576 B — 70% headroom.
2. **Dev-flash bootloader/IDF skew.** `espflash`'s bundled ESP-IDF v5.5.1
   bootloader was being paired with this project's v5.2.2 app on the local
   dev-flash path. Fixed by writing the project's own bootloader to `0x0`
   (`firmware/scripts/flash-with-partition-table.sh`).
3. **`#sendFrame` awaited outside its `try` block.** A bounded-send
   (write-stall) timeout in `site/provisioner/session.js`'s
   `#sendRecvWithRetry` escaped the retry loop instead of reaching the same
   error enrichment a receive timeout already got. Fixed by moving the call
   inside the `try`.
4. **Retained-byte loss across a retry boundary.** `#sendRecvWithRetry`
   cleared `#accBuf` on every failed retry attempt, not just on entry to a
   new command, silently discarding a reply that straddled a retry
   deadline. Fixed by no longer clearing `#accBuf` on a retry boundary
   (only on a new command). Regression test:
   `replySplitAcrossRetryBoundaryIsStillParsed`
   (`site/provisioner/session.smoke.test.mjs`).
5. **64-byte-multiple USB frame hazard.** A USB full-speed bulk transfer
   whose total length is an exact multiple of the 64-byte max packet size
   needs a following short/zero-length packet to signal completion; without
   one, the host can wait rather than treat the transfer as done. An audit
   of every reply-frame builder in `protocol::provisioning` found six
   reachable exact-multiple cases (`RSP_IDENTITY`, `RSP_CONTACT`, `RSP_ROOM`
   ×2, `RSP_ERROR`, `RSP_HISTORY_ENTRY`, `RSP_ADVERT`). Closed generically in
   `admin_server::send_frame` and `provisioning_server::send_frame`: when a
   frame's encoded length lands on an exact 64-byte multiple, the final byte
   is written and flushed as its own separate call. Pinned as host-testable
   unit tests: `cargo test -p protocol usb_packet_boundary`.
   **Upstream caveat, carried forward, not resolved:** MeshCore's own
   firmware tracker documents the identical hazard class
   (`OffbandMesh/meshcore-firmware#1093`), and MeshCore's own fix for it is
   **on hold after review found a regression** — it has not shipped
   upstream. This port's fix is a different implementation (a split
   write+flush at the boundary, not MeshCore's approach) and is not known to
   share that regression, but it has not been proven not to either — this
   crate is xtensa-only and this container has no Xtensa toolchain, so
   neither the fix nor MeshCore's shelved one has been device-verified here.
   Watch for a regression in this class (a reply that arrives truncated or
   the host waiting past a complete reply) if similar symptoms appear
   post-deploy.
6. **Instrumentation that made this campaign's endgame possible.** A reboot
   counter, the arrived-vs-retained byte split in timeout messages,
   `console.warn` (not `console.debug`, which Chrome's default filter
   hides) for the discarded-bytes dump, and the hex/ASCII dump of discarded
   (non-frame) bytes itself — all in `site/provisioner/session.js`.

## The host-side USB/cdc_acm wedge: real, but a consequence, not the explanation

One specific reproduction (2026-09-22, kernel-log evidence) showed a device
reset leaving the **host's** kernel-side USB/cdc_acm state broken —
`journalctl -k` confirmed no re-enumeration event occurred, yet host→device
delivery stayed dead across a full device-side reboot. This is real and is
kept as a recovery note, not as the reason the provisioner fails: a session
that clears this wedge and reaches `QUERY_STATUS` can still hit defect (1)
above later in the same session, and a session that never wedges at all can
still fail on the Pop!_OS/Flatpak host described in "Verified conclusion."
Whether the specific recovery this wedge was once thought to have
(unplug/replug, or the `/sys/.../authorized` equivalent) reliably clears it
is **not established** — it has since been tried, more than once, against a
live instance of this wedge and did not reliably clear it either. No
recovery action is currently known to reliably clear it, and no shipped
message instructs one (`host/src/transport.rs`'s `HOST_WEDGE_GUIDANCE`,
`site/provisioner/session.js`'s `HOST_WEDGE_GUIDANCE`).

## Eliminated hypotheses (do not re-propose without new evidence)

- **A client-side root cause for the connect wedge.** Refuted: the
  identical deployed page, against the identical device and firmware,
  connects and reads status correctly from a different host (a Chromebook
  running native Chrome).
- **"The browser path is structurally unworkable on this board."** Refuted
  by the same Chromebook result — a Web Serial client connects to this
  chip family without incident.
- **Same-identity USB re-enumeration / stale-handle mechanism.** Refuted:
  `journalctl -k`, monitored live across a reproducing connect, shows no
  enumeration event at all.
- **A firmware regression between `v0.6.0` and `v0.7.0`.** Refuted:
  `v0.6.0` fails too, after a replug — this was never a regression window
  to bisect.
- **DTR/RTS ordering (clear RTS before DTR) as the fix.** Tested on
  hardware; did not clear the connect wedge. Separately, Chromium's own
  `serial_io_handler_posix.cc` `PostOpen()` only sets `TIOCEXCL` and never
  touches DTR/RTS itself — the browser does not manipulate those lines
  unless the page's own code asks it to, which narrows what a client-side
  signal fix could ever have addressed in the first place.
- **The Chromium `SerialSplitDtrAndRts` feature flag as implicated.** The
  operator A/B'd `--enable-features` and `--disable-features` for this flag
  against a live reproduction; both behaved identically.
- **The host-side USB/cdc_acm wedge as a root cause.** It is real (see
  above) and is cleared, when it clears at all, only by host-side
  re-enumeration — but it is a *consequence* of a device reset, not an
  explanation for why the device resets or why the provisioner fails to
  connect in the first place. Kept as a recovery note, not presented as the
  explanation.
- **"Unplug and replug the USB cable, then click Connect again" as a
  remedy.** Disproven twice on hardware. No shipped message instructs this
  action (`host/src/transport.rs`, `site/provisioner/session.js`).
- **"Immediate first write" and "retry storm" as explanations for the
  browser-specific failure.** Eliminated: the host CLI does both — writes
  immediately after `open()` and retransmits on the identical 500ms/10s
  retry schedule — and it works.

**Eliminations with lasting value, carried forward:**

- **Hardware/peripheral.** MeshCore's own working web client drives the
  same USB_SERIAL_JTAG peripheral via `ARDUINO_USB_MODE=1` — the peripheral
  itself is not the obstacle.
- **Connect-path code shape.** Before the DTR/RTS detour (rounds 12-13,
  reverted), this client's `connect()` already matched
  `config.meshcore.io`'s `serial-cli.js` and `meshtastic/js`'s
  `transport-web-serial`: `port.open({ baudRate })` and nothing else. No
  independently-working Web Serial client against this hardware class calls
  `setSignals()` at all.
- **Wire protocol and framing.** The host CLI speaks the identical protocol
  over the identical log-polluted stream and works — the protocol and its
  framing are not the obstacle.
- **Volume / TX-ring / TX-mutex.** The host CLI's `list-contacts`,
  `list-channels`, and `export-history` — each a sustained multi-frame
  exchange — all work.
- **Linux-side port grabbers.** `ModemManager` is inactive on this device,
  `brltty` is inactive, and replug enumeration is clean — no competing
  process is grabbing the port out from under the provisioner.

## Open next step

**Untested discriminator: native Chromium (`.deb`) vs. Flatpak Chromium, on
the same Pop!_OS host, against the same device.** Everything else held
constant (host, kernel, USB stack, device, firmware, page) while varying
only the browser's packaging/sandboxing would separate a
sandbox/packaging-layer explanation from a host OS/kernel/USB-stack
explanation. This is the single open next step this campaign leaves behind
— not a finding, not a leading theory, an untested test.

## Housekeeping

`reset-probe.py`, a diagnostic scratch script from round 13's investigation
into why this campaign's own POSIX probes never reproduced the DTR/RTS
trigger, was never committed and has been removed from the repository root.

## Regression verification

The steps below confirm the six fixes above still hold; they are not a
diagnostic procedure for the open Pop!_OS/Flatpak question (there is
nothing here to diagnose that with — see "Open next step").

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
  leave that monitor running for every step below.
- The host CLI built from the same checkout:
  ```sh
  cargo build -p host --release
  ```
- A browser for the web provisioner (any Chromium-based browser with Web
  Serial — Chrome, Edge, Brave). See
  [`site/README.md`](../site/README.md) for how to serve `site/` locally if
  you don't already have it hosted.
- Erase/reset the device to a factory-fresh, unprovisioned state before
  starting: `espflash erase-flash`, or a fresh flash of a clean image.

### 1. Web provisioner: does connecting work?

Open the web provisioner (`site/provisioner.html`), click **Connect**,
select the device. **Expect:** device status (pubkey, "0 contacts, 0
channels") within a couple of seconds, no panic on the serial monitor. A
reset-and-recover within ~10 seconds also counts as a pass — the existing
retry/resync machinery tolerates a reset from `port.open()`'s own
unavoidable DTR/RTS assert, whose cause is not established (see "Verified
conclusion").

If this step fails on your host: before treating it as a new finding,
check whether your host and browser packaging match the profile in
"Verified conclusion" (Pop!_OS + Flatpak Chromium). If so, this is the
known, still-open failure — see "Open next step," not a new bug to chase.
If your host/browser combination does NOT match that profile and this step
still fails, that IS new information this campaign did not have; capture
the serial monitor output (from before the click through any reboot) and
the browser devtools console output.

### 2. Web provisioner: add a channel

With the device connected, use **Add channel** (any valid secret).
**Expect:** `RSP_OK`, no reboot. Exercises `ADD_CHANNEL` with a valid
`key_len` end-to-end.

### 3. Host CLI: does `status` work?

```sh
time cargo run -p host --release -- --port /dev/ttyACM0 status
```

**Expect:** a status readout in well under a second, or a bounded, named
timeout error within roughly 13 seconds — never an unbounded hang. If it
hangs past ~15-20 seconds with no output and no error, this contradicts the
bounded-retry logic's own regression tests
(`test_query_status_fails_with_diagnostic_against_an_unresponsive_device`,
`transport::tests::send_bounded_returns_a_diagnosable_error_instead_of_hanging_forever`)
and is worth reporting: either a different build is running, or there is a
genuinely new hang mechanism.

### 4. Host CLI: does the `key_len` hardening hold on real firmware?

```sh
cargo run -p host --example raw_add_channel_bad_key_len -- --port /dev/ttyACM0
```

**Expect:** four lines reading `rejected cleanly: device returned error 5:
add_channel decode error`, program exits 0. No device reboot, no hang.

### Result block

```
provisioning-connect-verification-kit — result
date: <UTC timestamp>
firmware commit: <git rev flashed>
board: <T-Deck Plus / other>
host: <OS, and if Linux, native package vs Flatpak/Snap for the browser used>
browser: <name, version, packaging>

step 1 (web provisioner connect):        PASS | FAIL
step 2 (web provisioner add-channel):    PASS | FAIL
step 3 (host CLI status):                PASS | FAIL
step 4 (bad key_len hardening probe):    PASS | FAIL

for any FAIL: serial monitor output, browser devtools console output (step
1/2 only), exact command + wall-clock time waited (step 3/4), and whether
your host/browser packaging matches the known Pop!_OS + Flatpak Chromium
failure profile in "Verified conclusion" above.
```
