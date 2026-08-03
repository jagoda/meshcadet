// SPDX-License-Identifier: GPL-3.0-only
//! The UI task — ADR-0012's core-1 half of the dispatcher/UI task split.
//!
//! # Why this file exists (D4.2)
//!
//! `firmware/Cargo.toml` builds `slint`/`i-slint-core` with
//! `unsafe-single-threaded`, which REMOVES Slint's thread-affinity checks
//! rather than satisfying them: every Slint interaction — platform
//! registration, bitmap-font registration, window-adapter creation, every
//! property write, every render — must happen on one and the same thread,
//! and nothing in the build will tell you if it does not (ADR-0012 D4, R8).
//!
//! This module holds the ONLY `use crate::ui::UiRuntime` in the crate — by
//! convention, not by the compiler. `mod ui;` is declared at the crate root
//! and `UiRuntime` is plain `pub`, so nothing in the type system actually
//! stops `main.rs` from writing `crate::ui::UiRuntime`; a stray Slint call
//! from the dispatcher task would compile cleanly and manifest as silent UB
//! at runtime instead, because `firmware/Cargo.toml`'s
//! `unsafe-single-threaded` feature is what's cited above — it removes
//! Slint's own thread-affinity checks, it doesn't add Rust's. What
//! mechanically enforces the barrier today is `xtask`'s
//! `slint_thread_affinity` host harness (`cargo test -p xtask`), which greps
//! every file under `firmware/src/` outside this one and `ui/` for
//! `UiRuntime`/`slint::`/`i_slint*` and fails the build if one appears. The
//! one item this module exposes, [`spawn`], takes the raw peripherals `main.rs`
//! constructed (SPI/I2C device REGISTRATION already happened there, on the
//! main task, before this function is ever called — D2's corollary: every
//! `spi_bus_add_device` call happens single-threaded, before `ui_task`
//! exists) and returns the dispatcher's own halves of the two channels
//! (D3). `UiRuntime` itself is constructed, used, and dropped entirely on
//! the spawned thread (D4.1): the task is spawned first; its entry point
//! performs display/touch/keyboard bring-up, then `UiRuntime::new()`.
//!
//! # Ownership partition (D2)
//!
//! `ui_task` is the exclusive, lifetime-long owner of: the ST7789
//! `SpiDeviceDriver` (LCD, CS GPIO12, 40 MHz), the DC/RST pins, the LEDC
//! backlight channel/timer, I2C1 (GT911 touch @0x5D, the ESP32-C3 keyboard
//! co-processor @0x55), the I2S0 buzzer, the trackball's five GPIOs, and —
//! obviously — the whole Slint runtime and every `UiRuntime` field. None of
//! it is ever touched by the dispatcher task again once `spawn` returns.
//!
//! # Boot sequencing (D8) and the "headless" fallback
//!
//! `main.rs`'s pre-split code degraded to a UI-less ("headless") boot on any
//! I2C/SPI/display/touch init failure, logging and continuing rather than
//! aborting — real hardware absence must not be a boot-blocking fault. That
//! behaviour is preserved here, just relocated:
//!
//! - `i2c1`/`lcd_spi` are already-attempted `Result`s (registration ran on
//!   `main` — see the module doc above); a hard failure there means no
//!   thread is even spawned. The dispatcher-side channel endpoints are
//!   still returned so `main.rs`'s unconditional `try_send`/`try_recv` call
//!   sites keep compiling and working unchanged — sends just accumulate
//!   until C2's drop-and-count policy kicks in, and no command is ever
//!   produced. This is the direct descendant of the pre-split
//!   `(Err(e), _) | (_, Err(e)) => { ...; None }` match arms.
//! - Display bring-up (`TDeckDisplay::new`), the GT911 touch probe
//!   (`TouchDriver::new`), and `UiRuntime::new()` itself run INSIDE the
//!   spawned thread and can still fail there (real hardware can be present
//!   at the I2C/SPI-registration level yet fail to answer its own bring-up
//!   sequence). On failure the thread logs and returns WITHOUT ever
//!   subscribing to the TWDT (D7's "first action after `UiRuntime`
//!   construction" — a construction that never completed never subscribes,
//!   so a genuinely absent display cannot trip a TWDT reboot loop, matching
//!   the pre-split contract exactly) and without entering the steady-state
//!   loop. Its channel halves are dropped, so the dispatcher's future
//!   `try_send`s degrade via the same disconnected-channel path C2 already
//!   covers.
//!
//! # Steady state (C7, D7)
//!
//! Once construction succeeds: TWDT-subscribe, then loop on
//! `evt_rx.recv_timeout(UI_TICK_MS)` — waking on a message OR the 16ms tick
//! ceiling, so animations advance on a steady cadence with no busy-wait.
//! `UiEvent::AppReady` is intercepted directly by this loop (D8 step 9,
//! `run_splash_ripple`'s ~1.15s dedicated render loop has no business
//! running inside `UiRuntime::step()`'s per-tick body); every other event is
//! forwarded to [`crate::ui::UiRuntime::post_event`] for `step()` to process
//! on its next call. The TWDT is petted once per loop iteration — at most
//! 16ms apart in steady state (D7 item 3) — including the (rare, one-shot,
//! ~1.15s) `run_splash_ripple` window, which pets its own tight loop
//! internally (D7 item 2).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use esp_idf_hal::cpu::Core;
use esp_idf_hal::gpio::{Output, OutputPin, PinDriver};
use esp_idf_hal::i2c::I2cDriver;
use esp_idf_hal::ledc::{LedcChannel, LedcTimerDriver};
use esp_idf_hal::spi::{SpiDeviceDriver, SpiDriver};
use esp_idf_hal::sys::EspError;
use esp_idf_hal::task::thread::ThreadSpawnConfiguration;

use crate::ui::display::TDeckDisplay;
use crate::ui::keyboard::KeyboardDriver;
use crate::ui::touch::TouchDriver;
use crate::ui::trackball::TrackballDriver;
use crate::ui::{BuzzerDriver, UiCommand, UiEvent, UiRuntime};

/// C7: wake on a message or this tick deadline — the direct analogue of the
/// loop model's `split_ui_idle_tick` parameter (`perf_loop_model/src/
/// sim.rs:356`).
///
/// COUPLED CONSTANT — read before retuning. This value happens to equal
/// `ui::UiRuntime::RENDER_MIN_INTERVAL_MS` (also 16 ms), and
/// `docs/perf/ui-residual-opt-r1.md` §4.1 leans on that coincidence: it is
/// why M3 concluded the split ALREADY supplies the render-cadence cap in a
/// quiet steady state, and therefore why the entry-fade repaint item was
/// demoted rather than optimized further. The two are independent knobs (one
/// bounds the whole `ui_task` loop and its TWDT pet, the other bounds only
/// frame FLUSHES while an animation settles), so they are deliberately NOT
/// asserted equal — but raising this one above `RENDER_MIN_INTERVAL_MS`
/// re-opens that demotion, and lowering it makes the throttle load-bearing
/// again in the steady state as well as under the event bursts it already
/// covers. Either direction wants §4.1's argument re-derived, not just
/// re-read.
const UI_TICK_MS: u64 = 16;

/// D3: dispatcher → UI event queue capacity. Steady-state production is
/// ≲2 events per dispatcher iteration; this is a safety valve, not a design
/// path (C2).
const EVENT_QUEUE_CAP: usize = 32;
/// D3: UI → dispatcher command queue capacity. Human-typing-rate production
/// (one command per Send press) makes this unreachable in practice (C2).
const COMMAND_QUEUE_CAP: usize = 16;

/// D6: `ui_task`'s pthread stack budget, derived from the one hard HWM data
/// point this repo has (see the ADR for the full derivation) — a strict
/// subset of what the pre-split single task carried.
const UI_TASK_STACK_SIZE: usize = 32_768;

/// D1: `ui_task` runs at the pthread default priority. Affinity, not
/// priority, is the arbiter here (see the ADR) — two tasks pinned to
/// different cores never compete for a run slot.
const UI_TASK_PRIORITY: u8 = 5;

/// Spawn `ui_task`, pinned to core 1 (D1), and return the dispatcher's own
/// halves of the two boundary channels (D3): a `SyncSender<UiEvent>` to post
/// radio/state events, and a `Receiver<UiCommand>` to drain UI-initiated
/// sends. See this module's doc for the full ownership/boot-sequencing
/// contract, including the headless-fallback behaviour on `i2c1`/`lcd_spi`
/// registration failure or a bring-up failure inside the spawned thread.
///
/// `i2c1`/`lcd_spi` are the `Result`s of the SPI/I2C device REGISTRATION
/// `main.rs` already attempted on its own task (D2's corollary) — this
/// function does not retry them, it only decides what to do with the
/// outcome. `dc`/`rst`/`backlight_channel`/`backlight_timer`/
/// `backlight_pin`/`buzzer`/`trackball` are `ui_task`-owned peripherals
/// `main.rs` constructed (fallibly, via `?`, or gracefully-degrading to
/// `None`) before calling this function — see `main.rs::run()`'s "Touch UI"
/// bring-up section.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn<C>(
    i2c1: Result<I2cDriver<'static>, EspError>,
    lcd_spi: Result<SpiDeviceDriver<'static, &'static SpiDriver<'static>>, EspError>,
    dc: PinDriver<'static, Output>,
    rst: PinDriver<'static, Output>,
    backlight_channel: C,
    backlight_timer: LedcTimerDriver<'static, C::SpeedMode>,
    backlight_pin: impl OutputPin + Send + 'static,
    buzzer: Option<BuzzerDriver<'static>>,
    trackball: Option<TrackballDriver<'static>>,
    provisioned: bool,
    pubkey_hex: String,
    self_name: String,
) -> anyhow::Result<(SyncSender<UiEvent>, Receiver<UiCommand>)>
where
    // `Send` (beyond what `LedcChannel` itself requires): this generic
    // peripheral singleton is about to cross into the spawned thread's
    // closure — `std::thread::Builder::spawn` requires the whole closure
    // `Send`, which requires every capture `Send`. Every concrete
    // `esp_idf_hal` peripheral singleton this crate ever substitutes here
    // (`peripherals.ledc.channel1`) carries an explicit `unsafe impl Send`
    // from the HAL's own `impl_peripheral!` macro; this bound just makes
    // that fact visible to the generic function.
    C: LedcChannel + Send + 'static,
{
    let (evt_tx, evt_rx) = sync_channel::<UiEvent>(EVENT_QUEUE_CAP);
    let (cmd_tx, cmd_rx) = sync_channel::<UiCommand>(COMMAND_QUEUE_CAP);

    // Headless fallback #1: I2C1 or the LCD SPI device failed to register on
    // `main`'s own task. Mirrors the pre-split `(Err(e), _) | (_, Err(e))`
    // match arms in `main.rs::run()` — same log lines, same "keep booting
    // without a display" outcome, just no thread ever spawned.
    let (i2c1, lcd_spi) = match (i2c1, lcd_spi) {
        (Ok(i2c1), Ok(lcd_spi)) => (i2c1, lcd_spi),
        (Err(e), _) => {
            log::error!("I2C/touch init failed: {:?} — running headless", e);
            return Ok((evt_tx, cmd_rx));
        }
        (_, Err(e)) => {
            log::error!("LCD SPI init failed: {:?} — running headless", e);
            return Ok((evt_tx, cmd_rx));
        }
    };

    // D1: pin the next spawn to core 1. This is a PENDING thread-local
    // config consumed by the very next `std::thread::Builder::spawn` only —
    // restored to `Default` immediately after so no later thread spawned
    // elsewhere in this crate (`admin_server`, `prov_server`) inherits it.
    ThreadSpawnConfiguration {
        name: Some(
            std::ffi::CStr::from_bytes_with_nul(b"ui_task\0")
                .expect("literal is a valid, single-NUL-terminated C string"),
        ),
        stack_size: UI_TASK_STACK_SIZE,
        priority: UI_TASK_PRIORITY,
        inherit: false,
        pin_to_core: Some(Core::Core1),
        ..Default::default()
    }
    .set()
    .map_err(|e| anyhow::anyhow!("ui_task: ThreadSpawnConfiguration::set failed: {:?}", e))?;

    let spawn_result = std::thread::Builder::new()
        .name("ui_task".into())
        .stack_size(UI_TASK_STACK_SIZE)
        .spawn(move || {
            ui_task_main(
                i2c1,
                lcd_spi,
                dc,
                rst,
                backlight_channel,
                backlight_timer,
                backlight_pin,
                buzzer,
                trackball,
                provisioned,
                &pubkey_hex,
                &self_name,
                evt_rx,
                cmd_tx,
            );
        });

    // Restore the default config immediately — see the comment above.
    ThreadSpawnConfiguration::default()
        .set()
        .map_err(|e| anyhow::anyhow!("ui_task: ThreadSpawnConfiguration restore failed: {:?}", e))?;

    spawn_result.map_err(|e| anyhow::anyhow!("ui_task: thread spawn failed: {:?}", e))?;

    Ok((evt_tx, cmd_rx))
}

/// The spawned thread's entry point. Runs entirely on `ui_task` (core 1):
/// display/touch/keyboard bring-up, `UiRuntime::new()` (D4.1 — constructed
/// ON this thread, never moved onto it), TWDT subscribe (D7), then the
/// steady-state `recv_timeout` loop (C7) until the dispatcher-side channel
/// halves are dropped.
#[allow(clippy::too_many_arguments)]
fn ui_task_main<C>(
    i2c1: I2cDriver<'static>,
    lcd_spi: SpiDeviceDriver<'static, &'static SpiDriver<'static>>,
    dc: PinDriver<'static, Output>,
    rst: PinDriver<'static, Output>,
    backlight_channel: C,
    backlight_timer: LedcTimerDriver<'static, C::SpeedMode>,
    backlight_pin: impl OutputPin + Send + 'static,
    buzzer: Option<BuzzerDriver<'static>>,
    trackball: Option<TrackballDriver<'static>>,
    provisioned: bool,
    pubkey_hex: &str,
    self_name: &str,
    evt_rx: Receiver<UiEvent>,
    cmd_tx: SyncSender<UiCommand>,
) where
    C: LedcChannel + Send + 'static,
{
    // The GT911 touch IC and the T-Deck keyboard co-processor share I2C1 —
    // wrap it so both drivers can borrow the bus; borrows are
    // software-serialised (one transaction at a time), same as pre-split.
    let i2c_bus: crate::ui::touch::I2cBus<'static> = Rc::new(RefCell::new(i2c1));

    let display = match TDeckDisplay::new(lcd_spi, dc, rst, backlight_channel, backlight_timer, backlight_pin) {
        Ok(d) => d,
        Err(e) => {
            log::error!("display init failed: {:?} — running headless", e);
            return;
        }
    };

    let touch = match TouchDriver::new(i2c_bus.clone()) {
        Ok(t) => t,
        Err(e) => {
            log::error!("GT911 touch probe failed: {:?} — running headless", e);
            return;
        }
    };

    // Probe the physical QWERTY keyboard co-processor (0x55) on the same
    // bus. Absence is non-fatal: the UI degrades to touch-only.
    let keyboard = match KeyboardDriver::new(i2c_bus.clone()) {
        Ok(kb) => Some(kb),
        Err(e) => {
            log::warn!(
                "keyboard co-processor probe failed: {:?} — running touch-only (no physical keyboard)",
                e,
            );
            None
        }
    };

    let mut ui = match UiRuntime::new(
        display, touch, keyboard, buzzer, trackball, provisioned, pubkey_hex, self_name, cmd_tx,
    ) {
        Ok(runtime) => {
            log::info!(
                "touch UI runtime initialised — {}×240 ST7789 + GT911",
                crate::ui::display::DISPLAY_WIDTH,
            );
            runtime
        }
        Err(e) => {
            log::error!("UI runtime init failed: {:?} — running headless", e);
            return;
        }
    };

    // D7 item 1: subscribe to the TWDT as the first action AFTER
    // `UiRuntime` construction succeeds — before `run_splash_ripple`, so no
    // window of the task's life is unwatched, and so a construction that
    // never completed (the three early returns above) never subscribes at
    // all (preserves the pre-split "absent display never reboots the
    // device" contract — see this module's doc).
    let twdt_subscribed = unsafe { esp_idf_svc::sys::esp_task_wdt_add(core::ptr::null_mut()) };
    if twdt_subscribed == 0 {
        log::info!("ui_task: subscribed to Task WDT (30 s timeout)");
    } else {
        log::warn!(
            "ui_task: esp_task_wdt_add failed (0x{:08x}) — loop not WDT-covered",
            twdt_subscribed,
        );
    }

    let mut app_ready_seen = false;
    // D6: periodic `ui_task` stack-HWM sample, matching the dispatcher's own
    // 30 s cadence and — like that one — unconditional, not diagnostics-only
    // (a stack budget is a production concern).
    let mut last_hwm_log_ms: u64 = crate::uptime_ms();
    const HWM_LOG_INTERVAL_MS: u64 = 30_000;

    // ADR-0012 D9 row 10 restore (collection-kit D1-D4/D-E): `ui_task`'s
    // own on-device perf rollup, diagnostics-only, same shape as the
    // dispatcher's `perf_rollup` in `main.rs`. The M1 split correctly
    // dropped that task's `ui_step` phase and `record_ui_starvation` call
    // (this loop, not that one, now owns `ui.step()`) but added no
    // replacement here — `record_ui_starvation` was left exported and
    // unit-tested (`firmware-core/src/perf.rs`) with no on-device caller.
    // This rollup, and the two call sites below, are that replacement.
    #[cfg(feature = "diagnostics")]
    let mut perf_rollup = Box::new(crate::perf::PerfRollup::new());
    // Timestamp of the previous `ui.step()` call. `None` until the first
    // call completes so thread bring-up (display/touch/keyboard probe,
    // `UiRuntime::new()`) is never counted as a starvation gap — the first
    // recorded gap is between the first and second `ui.step()` calls, both
    // inside the steady-state loop.
    #[cfg(feature = "diagnostics")]
    let mut last_ui_step_ms: Option<u64> = None;

    loop {
        // D7 item 3: `recv_timeout(UI_TICK_MS)` guarantees a pet at least
        // every 16 ms in steady state, with or without traffic.
        unsafe { esp_idf_svc::sys::esp_task_wdt_reset(); }

        match evt_rx.recv_timeout(Duration::from_millis(UI_TICK_MS)) {
            // D8 step 9: on the FIRST AppReady, fire the boot splash's
            // dedicated-render-loop ripple (D7 item 2 pets the TWDT inside
            // its own tight loop — see `run_splash_ripple`'s doc). Guarded
            // so a defensive double-delivery (there should never be one —
            // single producer, C3) can't re-block the task for another
            // ~1.15s; `run_splash_ripple` itself is separately idempotent.
            Ok(UiEvent::AppReady) if !app_ready_seen => {
                app_ready_seen = true;
                ui.mark_app_ready();
                ui.run_splash_ripple();
            }
            Ok(event) => ui.post_event(event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                log::error!("ui_task: dispatcher event channel disconnected — exiting");
                return;
            }
        }

        #[cfg(feature = "diagnostics")]
        let ui_step_t0_us = crate::uptime_us();

        if let Err(e) = ui.step(crate::uptime_ms()) {
            log::warn!("ui_task: step error: {:?}", e);
        }

        // D9 row 10 restore (diagnostics-only): `ui_step`'s own call
        // duration, same "phase" shape `main.rs` used pre-split, plus the
        // UI-starvation gap. Unlike the pre-split dispatcher — where
        // "starvation" meant how much of a shared iteration OTHER work
        // stole from `ui.step()` — `ui_task` runs nothing else of
        // consequence in this loop, so the coherent proxy here is simply
        // the wall-clock gap between one `ui.step()` call and the next:
        // `recv_timeout`'s up-to-16ms wait, event handling, and this same
        // periodic-log block all land inside that gap.
        #[cfg(feature = "diagnostics")]
        {
            let ui_step_dur_us = (crate::uptime_us().saturating_sub(ui_step_t0_us)) as u32;
            perf_rollup.ui_step.record(ui_step_dur_us);

            let ui_step_now_ms = crate::uptime_ms();
            if let Some(last_ms) = last_ui_step_ms {
                perf_rollup.record_ui_starvation(ui_step_now_ms.saturating_sub(last_ms) as u32);
            }
            last_ui_step_ms = Some(ui_step_now_ms);
        }

        // D6: periodic `ui_task` HWM sample (unconditional — see above).
        let now = crate::uptime_ms();
        if now.saturating_sub(last_hwm_log_ms) >= HWM_LOG_INTERVAL_MS {
            last_hwm_log_ms = now;
            crate::log_thread_stack_hwm("ui_task", UI_TASK_STACK_SIZE as u32);
            // D9 row 10: `ui_task` gets its own rollup. The input-to-first-
            // paint stat `main.rs` used to read via
            // `ui.take_input_paint_stats()` moved here unchanged at the M1
            // split, still diagnostics-only; `ui_step`'s own phase histogram
            // and the UI-starvation counters are restored below (see the
            // `ui.step()` call site above for what each measures) — same 30s
            // cadence and log-line shape as the dispatcher's own rollup in
            // `main.rs`.
            #[cfg(feature = "diagnostics")]
            {
                let paint = ui.take_input_paint_stats();
                log::info!(
                    "PERF input-to-first-paint: n={} min={}ms mean={}ms max={}ms p95={}ms",
                    paint.count, paint.min, paint.mean, paint.max, paint.p95,
                );

                let ui_step_snap = perf_rollup.ui_step.snapshot();
                log::info!(
                    "PERF phase=ui_step: n={} min={}us mean={}us max={}us p95={}us",
                    ui_step_snap.count,
                    ui_step_snap.min,
                    ui_step_snap.mean,
                    ui_step_snap.max,
                    ui_step_snap.p95,
                );
                log::info!(
                    "PERF ui-starvation: cumulative={}ms longest={}ms (window={}s)",
                    perf_rollup.ui_starvation_cumulative_ms,
                    perf_rollup.ui_starvation_longest_ms,
                    HWM_LOG_INTERVAL_MS / 1000,
                );
                // Reset for the next window — same reset-by-reassignment
                // idiom as `main.rs`'s `*perf_rollup = perf::PerfRollup::new();`.
                // `last_ui_step_ms` is NOT reset here: it tracks the
                // continuous cross-window `ui.step()` cadence, not a
                // per-window accumulator.
                *perf_rollup = crate::perf::PerfRollup::new();
            }
        }
    }
}
