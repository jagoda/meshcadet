// SPDX-License-Identifier: GPL-3.0-only
//! ESP-IDF dynamic frequency scaling (DFS) — SoC-level power management.
//!
//! Phase 7 of the `meshcadet-power-optimization` campaign
//! (see `docs/adr/0014-power-policy.md` §D8 for the full decision record).
//! **DFS only** — light sleep is explicitly out of scope for this leg (see
//! ADR-0014's light-sleep/RX-integrity contract, recorded there but NOT
//! implemented by this campaign): [`configure_dynamic_frequency_scaling`]
//! hard-sets `light_sleep_enable = false`, and `CONFIG_FREERTOS_USE_
//! TICKLESS_IDLE` is deliberately left disabled in `sdkconfig.defaults`
//! (see that file's comment — tickless idle exists solely to let the CPU
//! enter automatic light sleep between ticks, which this leg never does).
//!
//! # Frequency choice
//!
//! `max_freq_mhz` stays at [`MAX_FREQ_MHZ`] (240, unchanged from
//! `CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ_240`) — constraints P2 (LoRa timing
//! accuracy) and P4 (UI responsiveness) depend on the peak clock never
//! moving; only the idle floor below is new.
//!
//! [`MIN_FREQ_MHZ`] is 80, not a lower value, deliberately:
//! `rtc_clk_cpu_freq_mhz_to_config` (`esp_hw_support/port/esp32s3/rtc_clk.c`)
//! accepts 80/160/240 unconditionally (PLL-sourced, `divider` of the fixed
//! 480 MHz PLL) but only accepts a value below the board's XTAL frequency if
//! it divides the XTAL exactly — a fact this repo has no on-hardware
//! measurement of for the T-Deck Plus's specific crystal. 80 MHz sidesteps
//! that uncertainty entirely while still landing a 3× idle-frequency
//! reduction, within the plan's stated "40–80 MHz" range.
//!
//! One consequence worth recording: `esp_pm`'s own mode derivation
//! (`pm_impl.c`, the non-ESP32 branch) computes `apb_max_freq =
//! MIN(max_freq_mhz, esp_clk_apb_freq())`, and ESP32-S3's real APB
//! peripheral clock is a fixed 80 MHz tap off the same 480 MHz PLL whenever
//! the CPU runs off PLL (i.e. at 80/160/240) — so with `min_freq_mhz = 80`,
//! the actual APB clock is 80 MHz in *every* reachable PM mode (CPU_MAX,
//! APB_MAX, APB_MIN) and never itself changes. The [`ApbFreqMaxLock`]
//! bracketing below is still implemented exactly as the plan directs — it
//! is the correct defense-in-depth against any future `min_freq_mhz`
//! change that WOULD make the APB clock move (an XTAL-sourced value below
//! 80), and holding it costs nothing today.
//!
//! # Locks
//!
//! [`ApbFreqMaxLock`] wraps one `ESP_PM_APB_FREQ_MAX` lock
//! (`esp_pm_lock_create`/`_acquire`/`_release`, `esp_pm.h`). The radio's
//! SPI2 transactions (`radio.rs::Radio::write_cmd`/`spi_transfer`) and the
//! GPS driver's UART ACTIVE window (`gps.rs::GpsDriver::poll`) each own an
//! independent instance and bracket their own critical section with it — an
//! APB frequency change mid-transaction is the failure mode that would
//! breach constraint P1 (GPS UART baud, APB-derived) and P4 (SPI2 clock,
//! also APB-derived). The lock is recursive (ESP-IDF's own contract — see
//! `esp_pm_lock_acquire`'s doc comment in `esp_pm.h`), so nested/repeated
//! acquire calls are safe as long as they are matched 1:1 by release calls.
//!
//! Neither lock is ever deleted: both `Radio` and `GpsDriver` are
//! constructed once in `main.rs::run()` and live for the process lifetime,
//! so there is no scope in which `esp_pm_lock_delete` would ever run.

use esp_idf_svc::sys::{
    esp_pm_config_t, esp_pm_configure, esp_pm_lock_acquire, esp_pm_lock_create,
    esp_pm_lock_handle_t, esp_pm_lock_release, esp_pm_lock_type_t_ESP_PM_APB_FREQ_MAX,
};

/// Peak CPU frequency, MHz — UNCHANGED from `CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ_240`
/// (`sdkconfig.defaults`). See module doc — constraints P2/P4 depend on this.
pub const MAX_FREQ_MHZ: i32 = 240;

/// Conservative idle floor, MHz. See module doc "Frequency choice" for why
/// 80 (a PLL-sourced value, not an XTAL-derived one) was chosen.
pub const MIN_FREQ_MHZ: i32 = 80;

/// Configure ESP-IDF dynamic frequency scaling.
///
/// Call once, early in `main.rs::run()`, before any peripheral whose driver
/// assumes a fixed APB clock is initialised. `light_sleep_enable: false` is
/// load-bearing, not a placeholder — this campaign leg is DFS only (see
/// module doc).
///
/// A failure here is logged, not fatal: the firmware still boots and runs
/// pinned at [`MAX_FREQ_MHZ`] (today's behaviour) rather than DFS silently
/// doing nothing while the rest of the system assumes it is active — the
/// log line is what makes that distinguishable from "PM is running and just
/// hasn't scaled down yet" in a field capture.
pub fn configure_dynamic_frequency_scaling() {
    let config = esp_pm_config_t {
        max_freq_mhz: MAX_FREQ_MHZ,
        min_freq_mhz: MIN_FREQ_MHZ,
        light_sleep_enable: false,
    };
    let ret = unsafe { esp_pm_configure(&config as *const _ as *const _) };
    if ret != 0 {
        log::warn!(
            "pm: esp_pm_configure(max={MAX_FREQ_MHZ}, min={MIN_FREQ_MHZ}) failed (0x{ret:08x}) \
             — DFS not active; CPU stays pinned at {MAX_FREQ_MHZ} MHz"
        );
    } else {
        log::info!(
            "pm: DFS configured — CPU {MIN_FREQ_MHZ}-{MAX_FREQ_MHZ} MHz, light sleep disabled"
        );
    }
}

/// One `ESP_PM_APB_FREQ_MAX` power-management lock. See module doc for the
/// invariant this exists to bracket.
pub struct ApbFreqMaxLock {
    handle: esp_pm_lock_handle_t,
}

impl ApbFreqMaxLock {
    /// Create a new, initially-UNACQUIRED `ESP_PM_APB_FREQ_MAX` lock.
    ///
    /// `name` is a `\0`-terminated label surfaced only by ESP-IDF's
    /// `esp_pm_dump_locks` diagnostics — pass a short, unique byte-string
    /// literal per owner (e.g. `b"radio_apb\0"`, `b"gps_apb\0"`).
    ///
    /// Fails (propagated via `anyhow`, same convention every other
    /// peripheral-init call site in `main.rs::run()` uses) on
    /// `ESP_ERR_NO_MEM` (lock structure can't be allocated) or
    /// `ESP_ERR_NOT_SUPPORTED` (`CONFIG_PM_ENABLE` somehow not compiled
    /// in) — a lock this leg's P1/P4 defense depends on that silently
    /// failed to construct would be worse than a boot abort: every
    /// `acquire`/`release` call downstream would need to degrade to a
    /// no-op, exactly defeating the bracket this module exists to provide.
    pub fn create(name: &'static [u8]) -> anyhow::Result<Self> {
        debug_assert!(
            name.last() == Some(&0),
            "ApbFreqMaxLock::create: name must be NUL-terminated"
        );
        let mut handle: esp_pm_lock_handle_t = core::ptr::null_mut();
        let ret = unsafe {
            esp_pm_lock_create(
                esp_pm_lock_type_t_ESP_PM_APB_FREQ_MAX,
                0,
                name.as_ptr().cast(),
                &mut handle,
            )
        };
        if ret != 0 {
            anyhow::bail!("esp_pm_lock_create failed (0x{ret:08x})");
        }
        Ok(Self { handle })
    }

    /// Acquire the lock. From this call until the matching [`release`]
    /// call, ESP-IDF's power-management algorithm will not drop the APB
    /// clock below its maximum. Recursive per ESP-IDF's own contract: safe
    /// to call while already held (by the same owner), as long as every
    /// `acquire` is matched by exactly one `release`.
    ///
    /// [`release`]: Self::release
    pub fn acquire(&self) {
        let ret = unsafe { esp_pm_lock_acquire(self.handle) };
        if ret != 0 {
            log::warn!("pm: esp_pm_lock_acquire failed (0x{ret:08x})");
        }
    }

    /// Release a previously [`acquire`](Self::acquire)d lock. Must be
    /// matched 1:1 with a prior `acquire` call (ESP-IDF's own contract).
    pub fn release(&self) {
        let ret = unsafe { esp_pm_lock_release(self.handle) };
        if ret != 0 {
            log::warn!("pm: esp_pm_lock_release failed (0x{ret:08x})");
        }
    }
}
