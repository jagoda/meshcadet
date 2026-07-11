// SPDX-License-Identifier: GPL-3.0-only
//! Serial-console write serialisation (USB-Serial-JTAG shared-stream fix).
//!
//! # The defect this closes
//!
//! On a provisioned device the USB-Serial-JTAG stdout is shared by **two
//! independent writers**:
//!
//! 1. the ESP-IDF C logger — every `log::*` call from the radio / UI / GPS
//!    threads flows through `esp_log` → a `vprintf`-style sink → fd 1; and
//! 2. the [`crate::admin_server`] binary frame TX — Rust `std::io::stdout()`
//!    (`write_all` + `flush`) → fd 1.
//!
//! These two paths do **not** share a lock: the ESP-IDF logger bypasses Rust's
//! `Stdout` mutex entirely.  So a log line emitted by the radio thread can land
//! in the *middle* of a `RSP_CHANNEL` frame burst, corrupting the bytes the host
//! is parsing.  The host's `recv_frame` then sees bad magic / bad CRC, resyncs,
//! and drops the mangled frames → `list-channels` prints "no channels
//! configured".  Eight channel frames give ~8× the interleave exposure of a
//! single contact frame, which is why the symptom looked channel-specific.  It
//! is invisible to host `cargo test` (mock transport, no concurrent logger).
//!
//! # The fix
//!
//! Route **both** writers through one global mutex:
//!
//! - [`install`] registers a custom `esp_log` vprintf hook ([`locked_vprintf`])
//!   that takes [`SERIAL_TX`] around each *complete* log line before forwarding
//!   to the default `vprintf` sink.
//! - [`lock_tx`] is taken by `admin_server` / `provisioning_server` around each
//!   binary frame's `write_all` + `flush`.
//!
//! With both writers holding the same lock, no log bytes can ever interleave
//! mid-frame.  Logs still share the wire but only ever land *between* frames,
//! which the host parser already tolerates (resync on `PROV_MAGIC`).
//!
//! No path acquires the lock twice, so there is no reentrancy / deadlock: the
//! frame-TX critical sections perform no logging, and the log hook performs no
//! frame TX.

use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

/// The ABI of an `esp_log` output sink (matches `vprintf`), per
/// `esp_idf_svc::sys::vprintf_like_t`.
type VprintfFn =
    unsafe extern "C" fn(*const core::ffi::c_char, esp_idf_svc::sys::va_list) -> core::ffi::c_int;

/// The single mutex serialising every writer on the USB-Serial-JTAG stdout.
static SERIAL_TX: Mutex<()> = Mutex::new(());

/// The default `esp_log` sink that was installed before [`install`] ran — the
/// libc-`vprintf`-equivalent that writes to the configured console
/// (USB-Serial-JTAG).  [`locked_vprintf`] forwards to it under the lock so log
/// output is unchanged except for being serialised.  Set exactly once, in
/// `install()`, on the main thread before any other thread can log.
static ORIG_VPRINTF: OnceLock<VprintfFn> = OnceLock::new();

/// Acquire the shared serial-TX lock.
///
/// Held by the frame-TX path (`send_frame`) across each binary frame's
/// `write_all` + `flush` so the ESP-IDF C logger cannot interleave mid-frame.
/// Poison is ignored (`into_inner`): a panicked writer must not wedge the
/// console for every other task — the worst case is one already-corrupted line.
pub fn lock_tx() -> MutexGuard<'static, ()> {
    SERIAL_TX.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Custom `esp_log` vprintf hook: serialises every log line against
/// [`SERIAL_TX`], then forwards to the original console sink captured in
/// [`ORIG_VPRINTF`].
///
/// ESP-IDF documents this callback as needing to be re-entrant (it can fire from
/// multiple tasks in parallel); the mutex satisfies that by serialising parallel
/// invocations.  The original sink writes to the console and never calls
/// `esp_log`, so it cannot re-enter this hook → no deadlock.
///
/// # Safety
/// Matches the `vprintf_like_t` ABI required by `esp_log_set_vprintf`.  The
/// `va_list` is forwarded to the original sink exactly once.
unsafe extern "C" fn locked_vprintf(
    format: *const core::ffi::c_char,
    args: esp_idf_svc::sys::va_list,
) -> core::ffi::c_int {
    let _tx = lock_tx();
    match ORIG_VPRINTF.get() {
        // Forward to the original console sink under the lock.
        Some(orig) => orig(format, args),
        // Unreachable in practice: install() populates ORIG_VPRINTF before this
        // hook can be invoked by any other thread. Drop the line rather than
        // risk undefined behaviour if the invariant is ever broken.
        None => 0,
    }
}

/// Install the serialising log hook.  Call once, early in `main()`, right after
/// `EspLogger::initialize_default()` and before any frame-TX thread is spawned.
///
/// `esp_log_set_vprintf` returns the handler it replaced — the default console
/// sink — which we stash in [`ORIG_VPRINTF`] so [`locked_vprintf`] can forward
/// to it.  Capturing it *before* the hook can run concurrently is why this must
/// be called on the main thread during single-threaded init.
pub fn install() {
    let prev: esp_idf_svc::sys::vprintf_like_t =
        unsafe { esp_idf_svc::sys::esp_log_set_vprintf(Some(locked_vprintf)) };
    if let Some(orig) = prev {
        let _ = ORIG_VPRINTF.set(orig);
    }
}
