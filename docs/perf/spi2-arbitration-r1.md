# SPI2 bus arbitration — R1 verdict

**Campaign:** `meshcadet-perf-rearchitecture`, M0.
**Risk:** R1 — SPI2 bus arbitration between the LCD (ST7789, 40 MHz) and the
radio (SX1262, 8 MHz) once they move from one task to two, on different
cores.
**Consumed by:** ADR-0012 (`meshcadet-perf-task-split-adr`, M1). This document
answers R1 from source and datasheet so the ADR can cite it rather than
re-derive it.
**Provenance:** every claim below is either (a) quoted from ESP-IDF v5.2.2 —
the version pinned in `firmware/.cargo/config.toml`'s `ESP_IDF_VERSION` — or
`esp-idf-hal` v0.46.2 — the version pinned in `firmware/Cargo.lock` — with a
file/line citation, or (b) a computation shown in full from those primary
sources. No number in this document is invented or inferred without a shown
derivation.

## Verdict, up front

**The arbitration guarantee holds for this repo's usage, and the latency
bound is derivable on paper — it does not need silicon.** Both devices are
already used exactly the way ESP-IDF's own thread-safety contract requires
("each Device is accessed by only one task"), and the resulting worst-case
wait for the radio to acquire the bus during a concurrent LCD repaint is
**one already-in-flight SPI transaction of at most 64 bytes at 40 MHz — ≤
12.8 µs**, not the length of a line or a repaint. That is roughly three
orders of magnitude below LoRa airtime (83–800 ms) and below the radio's own
DIO1 poll quantization (`FreeRtos::delay_ms(1)`, R2 territory). **R1 is
closed by static analysis; it is not the irreducible-silicon item the
campaign plan flagged it as a candidate for** (§4b of the campaign plan)
— see "What still needs silicon" below for the one item that remains, which
is a confidence check, not a correctness gap.

## The construction (source)

`firmware/src/main.rs` constructs one `SpiDriver` on SPI2 and two
`SpiDeviceDriver`s borrowing it:

- `SpiDriver::new(peripherals.spi2, ...)` — `main.rs:676`, default
  `SpiDriverConfig::new()` (DMA disabled, no custom interrupt flags).
- LCD device: `SpiDeviceDriver::new(&spi_driver, Some(gpio12), &SpiConfig::new().baudrate(40u32.MHz().into()))`
  — `main.rs:738-741`. Hardware CS (`Some(pin)`), all other `Config` fields
  default: `polling: true`, `duplex: Duplex::Full`, `write_only: false`,
  `queue_size: 1`.
- Radio device: `SpiDeviceDriver::new(&spi_driver, Some(gpio9), &SpiConfig::new().baudrate(8u32.MHz().into()))`
  — `main.rs:1398-1401`. Same defaults.

Neither device overrides `.polling()`, `.duplex()`, `.write_only()`, or
`.allow_pre_post_delays()` anywhere in the repo (`grep` over
`firmware/src/main.rs`, `firmware/src/radio.rs`, `firmware/src/ui/display.rs`
confirms no such call sites exist). Both are plain, default, hardware-CS,
polling-mode, full-duplex devices. This matters for every answer below —
it is the "usage" the analysis is scoped to.

## Q1 — Does `spi_master` serialise transactions across devices, from different tasks on different cores?

**Yes, with one precondition this repo already satisfies.** ESP-IDF's own
docs (`spi_master.html`, v5.2.2) state the exact contract:

> "As long as each Device is accessed by only one task, the driver is
> thread-safe. However, if multiple tasks try to access the same SPI Device,
> the driver is **not thread-safe**."

and, on cross-device behaviour:

> "Automatic time-division multiplexing of data coming from different
> Devices on the same signal bus."

The precondition is **per-device**, not per-bus: one task per *device*, not
one task for the whole bus. Under the M1 split — UI task owns the LCD
`SpiDeviceDriver`, dispatcher/radio task owns the radio `SpiDeviceDriver`,
each device touched from exactly one task — this is precisely the supported
pattern, not an edge case of it.

The mechanism (not just the doc claim) is in
`components/driver/spi/gpspi/spi_master.c` (ESP-IDF v5.2.2) and
`components/driver/spi/spi_bus_lock.c`. Every `spi_device_polling_transmit`
call (the only transmit path this repo uses — see Q2/usage above) goes
through `spi_device_polling_start`, which calls
`spi_bus_lock_acquire_start(handle->dev_lock, ticks_to_wait)`
(`spi_master.c:1112`) *before* touching hardware, and
`spi_device_polling_end` calls `spi_bus_lock_acquire_end`
(`spi_master.c:1175`) after. This happens **on every transaction**,
regardless of whether the caller ever touches the optional
`spi_device_acquire_bus`/`release_bus` API — the lock is not opt-in, it is
the substrate every transmit path runs through.

`spi_bus_lock.c`'s lock itself is core-agnostic: a lock-free `status` word
via C11 `atomic_uint_fast32_t` (`atomic_fetch_or`/`atomic_fetch_and`,
`spi_bus_lock.c:196`), a `portMUX_TYPE` spinlock for the one documented
short critical section (`spi_bus_lock.c:257`), and FreeRTOS binary
semaphores (`xSemaphoreCreateBinary`/`Take`/`Give`) for the actual blocking
wait. All three primitives are SMP-safe on the dual-core S3 by construction
— nothing here is core-pinned or relies on single-core reasoning. **This
answers the "different tasks on different cores" half of Q1 directly: the
lock does not care which core requested it.**

## Q2 — Does the differing baudrate (40 MHz vs 8 MHz) change anything? Is the clock reconfigured inside the arbitrated section?

**Reconfigured per device-handoff, and yes, strictly inside the arbitrated
section — after the lock is held, never before or racing it.**

`spi_new_trans` (`spi_master.c:602`), the function that actually programs
the transaction onto the hardware, is only ever reached *after*
`spi_device_polling_start` has already returned from
`spi_bus_lock_acquire_start` successfully (`spi_master.c:1112`-`1129`). Inside
`spi_new_trans`, the very first thing it does is call `spi_setup_device(dev)`
(`spi_master.c:612`), which reprograms the clock:

```c
if (spi_bus_lock_touch(dev_lock)) {
    /* Configuration has not been applied yet. */
    spi_hal_setup_device(hal, hal_dev);
    SPI_MASTER_PERI_CLOCK_ATOMIC() {
        spi_ll_set_clk_source(hal->hw, hal_dev->timing_conf.clock_source);
    }
}
```
(`spi_master.c:559-565`)

`spi_bus_lock_touch` only reports "needs reconfiguring" when the acquiring
device differs from the last device the peripheral was configured for
(tracked as `host->last_dev`) — i.e. the driver already knows the two
devices have different clocks and reprograms on every LCD↔radio handoff,
never assumes the previous device's clock still applies. Because this
reconfiguration is inside `spi_new_trans`, which is only reachable once the
bus lock is held, there is no window where device B could start clocking
data out at device A's rate, or vice versa — the register write and the
transaction it configures share the same critical section. The 40 MHz /
8 MHz split is exactly the case ESP-IDF's per-device config model exists to
handle and does not weaken the arbitration guarantee.

## Q3 — What does `esp-idf-hal`'s `SpiDeviceDriver` wrapper add/remove, and does the borrowed form survive a `'static` split?

**Adds** (relative to the raw C `spi_bus_add_device`/`spi_device_*` API,
`esp-idf-hal/src/spi.rs` v0.46.2):
- Safe `embedded-hal` 0.2 and 1.0 trait impls (`SpiDevice`, `Transfer`,
  `Write`, `WriteIter`, `Transactional`).
- Automatic chunking of any `Operation` larger than the hardware/DMA max
  transfer size into multiple `spi_transaction_t`s (`spi_operations`,
  `spi.rs:1263-1270`, `spi_write_transactions`/`spi_read_transactions` etc.,
  `spi.rs:1870-1992`).
- Automatic, opt-out-only `BusLock` wrapping (`spi_device_acquire_bus`/
  `release_bus`) whenever a *single logical operation* chunks into more than
  one hardware transaction with hardware CS enabled (`CsCtl::needs_bus_lock`,
  `spi.rs:1754-1761`; `run()`, `spi.rs:1104-1147`) — so a multi-chunk write
  stays atomic on the bus without the caller ever touching the raw
  acquire/release functions.
- RAII cleanup: `spi_bus_remove_device` on `SpiDeviceDriver::drop`
  (`spi.rs:1277-1280`), `spi_bus_free` on `SpiDriver::drop` (`spi.rs:649-653`).

**Removes/hides:** manual `spi_transaction_t` construction, manual handle
lifecycle, and — this is the point most relevant to R1 — the *need* to call
`spi_device_acquire_bus`/`release_bus` yourself for correctness. The crate's
own module doc states multi-transaction bus acquisition ("`Transfer`,
`Write`... lock the APB frequency") is about avoiding *per-call overhead*,
not about correctness; correctness is the C driver's job regardless (Q1).

**The borrow.** `SpiDeviceDriver<'d, T>` is generic over
`T: Borrow<SpiDriver<'d>> + 'd`; this repo instantiates `T = &SpiDriver<'_>`
(`main.rs:738`, `main.rs:1398`), a plain immutable borrow of a stack-local
`SpiDriver`. That borrowed form cannot itself be captured by a `'static`
task spawn — the referent lives on `main()`'s stack frame, and the borrow
checker has no way to know that frame outlives the spawned task (even though
it does today, since `main()` never returns). **This is exactly R2's
lifetime/`Send` problem, not R1's.** Two facts bound it, though, and are
worth putting in the ADR directly:

1. **Any resolution R2 picks preserves R1's guarantee unchanged.** The
   crate's own module doc lists the legal `Borrow<SpiDriver>` carriers
   explicitly — "`SpiDriver`, `&SpiDriver`, `&mut SpiDriver`, `Rc(SpiDriver)`
   or `Arc(SpiDriver)`" (`spi.rs`, top-of-file doc). `SpiDeviceDriver::run()`
   — the function that does the arbitration-relevant work (Q1/Q2) — never
   inspects `T`'s concrete type; it only ever calls `self.driver.borrow()`.
   Whatever container R2 chooses (`Box::leak`, `Arc`, scoped threads) to
   satisfy `'static`, the arbitration mechanism underneath is identical.
2. **One concrete constraint for R2 to know about, surfaced by this
   analysis:** `SpiDriver` is marked `unsafe impl Send` (`spi.rs:655`) but
   **not** `Sync` — no `unsafe impl Sync for SpiDriver` exists anywhere in
   the crate. `Arc<T>: Send` requires `T: Send + Sync`
   ([std docs](https://doc.rust-lang.org/std/sync/struct.Arc.html)), so a
   naive `Arc<SpiDriver>` shared into a second task will not compile as-is —
   R2 will need either a wrapper that asserts `Sync` (sound here only
   because the two devices never touch `SpiDriver` concurrently themselves,
   only through their own already-synchronized `SpiDeviceDriver`s) or a
   non-`Arc` resolution (`Box::leak`, one device fully owned per task with
   no shared driver handle at all). Flagged for the ADR; not resolved here —
   it is R2's design choice to make, not a correctness gap in R1.

## Q4 — Usage patterns in this repo that would break the guarantee?

Checked directly against source (`grep` across `firmware/src/main.rs`,
`firmware/src/radio.rs`, `firmware/src/ui/display.rs`,
`firmware/src/ui/platform.rs`):

- **`spi_device_acquire_bus` called directly by application code:** none.
  The only call sites are inside `esp-idf-hal` itself (Q3), triggered
  automatically and safely by chunking, never by this repo.
- **`SPI_DEVICE_NO_DUMMY` / `write_only`:** not set on either device
  (`Config::default().write_only == false`, never overridden).
- **DMA ownership conflicts:** DMA is disabled (`SpiDriverConfig::new()`
  default, `Dma::Disabled`) — both devices transfer through the CPU FIFO,
  chunked to `SOC_SPI_MAXIMUM_BUFFER_SIZE` = **64 bytes on the ESP32-S3**
  (`esp-idf/components/soc/esp32s3/include/soc/soc_caps.h:317`). No DMA
  channel to contend over.
- **Half-duplex tricks:** `Duplex::Full` (default) on both devices; no
  `.duplex(Duplex::Half)` or `Half3Wire` call anywhere.
- **Direct register access / `unsafe` SPI code:** none in `radio.rs`,
  `display.rs`, or the SPI construction sites in `main.rs`.
- **Radio command shape:** every SX1262 command in `radio.rs` is a single
  `spi.write(&buf[..n])` (`write_cmd`, `radio.rs:659-664`, buffers ≤16
  bytes) or a single `spi.transfer_in_place(buf)` (`spi_transfer`,
  `radio.rs:667-669`). The one buffer that exceeds the 64-byte chunk size is
  the LoRa TX payload write (`CMD_WRITE_BUFFER`, up to 2+255 = 257 bytes,
  `radio.rs:285-289`) — this legitimately chunks into up to 5 transactions
  and correctly triggers `esp-idf-hal`'s automatic `BusLock` (Q3), which is
  the *safe*, intended path, not a hand-rolled one.

**No usage pattern in this repo defeats the arbitration guarantee.** Every
call site goes through the default, fully-arbitrated path.

## Q5 — Bus-hold latency: how long can the radio be made to wait?

This is the question that actually matters for priority 1, and the one the
campaign plan explicitly warned could leave R1 "correct but unbounded."
**It is bounded, and the bound is small — but getting there requires
correcting an inaccurate assumption already written into this repo's own
comments.**

### The wrong intuition, and why it's wrong

`firmware/src/ui/display.rs:33-38` documents `flush_line_range` as issuing
"multiple SPI writes per refresh cycle — acceptable because the SPI bus runs
at 40 MHz and a 320-pixel line takes ≤ 13 µs." Read naively, that suggests a
single `flush_line_range` call — and by extension the `SpiDeviceDriver`
`run()` call underneath it — holds the bus for a whole 320-pixel line as one
atomic unit. If that were true, a 240-line full repaint
(`ui/platform.rs::process_line`, one `flush_line_range` call per dirty line)
could in the worst case force the radio to wait up to 240× that per-line
figure before the bus comes free, which **would** be a real priority-1 risk
worth silicon time to confirm.

**That intuition does not match what the write path actually does.** Tracing
the call chain from `flush_line_range` down:

1. `flush_line_range` (`display.rs:271`) calls `mipidsi::fill_contiguous` →
   `set_pixels` → `ST7789::write_pixels` (`mipidsi` v0.8.0,
   `models/st7789.rs:65-71`), which does `dcs.write_command(WriteMemoryStart)`
   then `dcs.di.send_data(DataFormat::U16BEIter(...))` — **one logical call**
   for the whole line's pixel stream.
2. `send_data` reaches `display-interface-spi` v0.5.0's `send_u8`
   (`src/lib.rs:81-103`), which does **not** pass the whole line to the SPI
   device in one call. It buffers into a **64-byte** (`BUFFER_SIZE`,
   `src/lib.rs:13`) stack array and calls `spi.write(&buf)` — a fresh,
   independent `embedded_hal::spi::SpiDevice::write()` call — **every time
   the buffer fills**. A 320-pixel RGB565 line is 640 bytes, so one
   `flush_line_range` call issues **10 separate `write()` calls**, not one.
3. Each of those 64-byte `write()` calls reaches `esp-idf-hal`'s
   `SpiDeviceDriver::run()` (`spi.rs:1096`) as its own `Operation::Write`.
   `esp-idf-hal`'s own chunk size for this bus is *also* 64 bytes
   (`SpiDriver.max_transfer_size`, set from `Dma::Disabled`'s
   `TRANS_LEN = min(SOC_SPI_MAXIMUM_BUFFER_SIZE, 64) = 64`,
   `spi.rs:93-103`), so each 64-byte `write()` chunks to **exactly one**
   hardware transaction (`words.chunks(64)` on a 64-byte slice yields one
   chunk, `spi.rs:1896`). `transactions_count == 1` → `CsCtl::needs_bus_lock()`
   returns `false` (`spi.rs:1754-1761`, `Hardware { transactions_count > 1 }`
   is the only `true` case) → **no** `esp-idf-hal`-level `BusLock` is taken.

**The consequence: the SPI bus is released and re-arbitrated after every
64-byte chunk of every line, not after every line and not after the whole
repaint.** `esp-idf-hal` never holds the bus across the LCD's own 10 chunks
of a line, because it never sees them as one operation — `display-interface-
spi` already split them into 10 independent `SpiDevice::write()` calls
before `esp-idf-hal` gets involved.

### The actual bound

Combined with Q1's finding that the underlying C driver's
`spi_bus_lock_acquire_start`/`_end` wraps *every* `spi_device_polling_
transmit` regardless of whether `esp-idf-hal` layers its own `BusLock` on
top, the worst case for the radio is: **it may have to wait for one
already-in-flight elementary SPI transaction to finish, and no more** — the
biggest such transaction on this bus is one 64-byte LCD chunk at 40 MHz:

```
64 bytes × 8 bits / 40 000 000 Hz = 512 / 40e6 s = 12.8 µs
```

With exactly two devices sharing the bus (no third device to jump a queued
request — `spi_bus_lock.c`'s FSM grants the lock to whichever device holds
a set `LOCK` bit at the moment the current acquiring device releases,
`spi_bus_lock.c:105-112`), the radio cannot be starved for longer than one
such alternation: LCD-chunk-in-flight → radio's request is already queued →
radio gets the very next grant. **12.8 µs is the derived worst-case bound**,
not 240 lines' worth, and not even one full line's worth.

For scale: LoRa airtime is 83–800 ms (the actual priority-1 blocking cost
the campaign is chasing) and the radio's own DIO1 poll quantization is up to
1 ms (`FreeRtos::delay_ms(1)`, R2). **12.8 µs is 4-5 orders of magnitude
below either** — nowhere near a priority-1 threat, whether the split lands
or not.

### A documentation correction this analysis surfaced

`firmware/src/ui/display.rs:37-38`'s "a 320-pixel line takes ≤ 13 µs" is off
by roughly 10× as a claim about the whole line (640 bytes ≈ 128 µs at
40 MHz, ignoring per-chunk CS/setup overhead) — it is, numerically, almost
exactly the correct figure for **one 64-byte chunk** (12.8 µs), which
strongly suggests the original author computed the right number for the
wrong unit. This is a comment-accuracy defect, not a functional one — no
code path relies on the stated figure — but it is the same 64-byte quantity
this analysis needed for the real bound, so it is corrected here and worth
a follow-up doc fix at `display.rs:37-38` when M1 touches that file (out of
scope for this pure-analysis mission to edit).

## What still needs silicon

**Nothing needs to be measured for correctness.** The arbitration guarantee
covers this repo's two-device, two-clock, two-task usage exactly as
documented, and the worst-case bus-hold latency (§Q5) is fully derived from
source with no unmeasured constant in the derivation — every number above
(64-byte hardware FIFO limit, 40 MHz/8 MHz clocks, ESP32-S3
`SOC_SPI_MAXIMUM_BUFFER_SIZE`) is a fixed, documented hardware/config
constant, not something that varies at runtime or needs a bench reading.

One item is genuinely a confidence check rather than an open correctness
question, and is named here so the collection kit can carry it if the
Commander wants the belt-and-suspenders reading:

- **Confirm the 12.8 µs bound empirically under real concurrent load.** The
  derivation in §Q5 reads ESP-IDF's `spi_bus_lock` FSM design (request/lock
  bits, release-time rescheduling to a waiting device) to conclude
  "at most one in-flight transaction of wait, no starvation with two
  devices." That FSM behavior is documented in the driver's own design
  comments (`spi_master.c:41-74`, `spi_bus_lock.c:21-141`) but is **not**
  restated as a formal timing SLA in Espressif's public API reference —
  it is a reading of the reference implementation, correct as of ESP-IDF
  v5.2.2, not a contract Espressif promises to preserve across versions.
  **On-device probe, if ever run:** toggle a spare GPIO (or use the existing
  `diagnostics`-feature instrumentation once PR #120 lands) immediately
  before the radio's `spi_device_acquire_bus`-equivalent wait point in
  `radio.rs`'s SPI calls, and again immediately after the transaction
  returns, while a full-screen repaint (`process_line` × 240) runs
  concurrently on the other task/core. Expected reading: every such
  interval ≤ ~15-20 µs (12.8 µs bound + scheduler/ISR jitter headroom); a
  reading in that range confirms the static bound with margin, and a
  reading that blows past it by an order of magnitude or more would be the
  signal that some assumption in this document (chunk size, DMA state,
  device count) does not hold on the real board and needs re-derivation.
  **This is not a gate on M1** — it is a confirmatory reading the collection
  kit can carry opportunistically, not a blocking predicate; §Q1-Q4's
  correctness argument does not depend on it.

## Sources consulted

- `esp-idf-hal` v0.46.2 (pinned, `firmware/Cargo.lock`), full text of
  `src/spi.rs` — <https://github.com/esp-rs/esp-idf-hal/blob/v0.46.2/src/spi.rs>
- ESP-IDF v5.2.2 (pinned, `firmware/.cargo/config.toml`'s
  `ESP_IDF_VERSION`):
  - `spi_master` API reference, ESP32-S3 —
    <https://docs.espressif.com/projects/esp-idf/en/v5.2.2/esp32s3/api-reference/peripherals/spi_master.html>
  - `components/driver/spi/gpspi/spi_master.c`
  - `components/driver/spi/spi_bus_lock.c`
  - `components/soc/esp32s3/include/soc/soc_caps.h`
    (`SOC_SPI_MAXIMUM_BUFFER_SIZE`)
- `mipidsi` v0.8.0 (pinned, `firmware/Cargo.lock`): `src/lib.rs`,
  `src/graphics.rs`, `src/models/st7789.rs`, `src/models/ili934x.rs`
- `display-interface-spi` v0.5.0 (pinned, `firmware/Cargo.lock`): `src/lib.rs`
- This repo: `firmware/src/main.rs`, `firmware/src/radio.rs`,
  `firmware/src/ui/display.rs`, `firmware/src/ui/platform.rs`
