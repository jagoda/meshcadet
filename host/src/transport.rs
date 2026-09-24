// SPDX-License-Identifier: GPL-3.0-only
//! USB-serial transport layer for the MeshCadet host CLI.
//!
//! Defines the `Transport` trait (byte-stream I/O) and its concrete
//! `SerialTransport` implementation backed by the `serialport` crate.
//!
//! Test code can implement `Transport` directly (see `tests/integration.rs`
//! for `MockTransport`).

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ── Transport trait ───────────────────────────────────────────────────────────

/// Byte-stream I/O abstraction over a serial (or mock) port.
///
/// Invariants:
/// - `send` must write all supplied bytes or return an error.
/// - `recv` may return 0 bytes (timeout / not yet available) without being an
///   error; callers must retry.
pub trait Transport {
    fn send(&mut self, data: &[u8]) -> anyhow::Result<()>;
    /// Read up to `buf.len()` bytes.  Returns 0 on timeout/empty.
    fn recv(&mut self, buf: &mut [u8]) -> anyhow::Result<usize>;
    /// Discard any bytes buffered in the inbound receive path (kernel / OS
    /// serial buffer and any driver-level accumulation).
    ///
    /// Called by `Session::send_recv_with_retry` before each retry send so
    /// that a late-arriving response from the previous attempt cannot be
    /// mistaken for the reply to the re-sent command frame.
    ///
    /// The default implementation is a no-op, which is correct for in-process
    /// mock transports where byte delivery is synchronous (no OS buffer exists).
    /// `SerialTransport` overrides this with `port.clear(ClearBuffer::Input)`.
    fn flush_input(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

// ── SerialTransport ───────────────────────────────────────────────────────────

/// Upper bound on how long `SerialTransport::send` will wait for
/// `write_all` + `flush` to complete before giving up and reporting a
/// diagnosable error, instead of hanging forever.
///
/// CONFIRMED BY DEVICE EVIDENCE (round 6, `meshcadet-connect-wedge-round6-
/// transmit-side`, 2026-09-21): with the connect wedge reproduced, the host
/// CLI printed "host CLI: serial port opened in ..." and then hung; the
/// process's `/proc/<pid>/wchan` read `tty_wait_until_sent` — i.e. blocked
/// inside `tcdrain(2)`, called from `self.port.flush()` below.  In
/// `serialport` 4.9.0's `posix/tty.rs`, `TTYPort::write` is bounded (its
/// `wait_write_fd` honors `self.timeout`), but `TTYPort::flush` calls
/// `nix::sys::termios::tcdrain` with NO timeout on the syscall itself — the
/// `timeout` field there only bounds the local `EINTR`-retry loop around
/// `tcdrain`, not `tcdrain`'s own wait. `Session`'s 500ms-per-attempt/10s-
/// total deadlines (`host/src/session.rs`) are evaluated only BETWEEN
/// transport calls, so a `tcdrain` that never returns is never interrupted by
/// them — this is a genuine, unbounded-at-the-syscall-level hang, not merely
/// a slow one.
///
/// This refutes an earlier round's finding that `SerialTransport::open()`
/// was "the one call in the host CLI's entire path with no deadline" — see
/// that method's own doc comment below.
///
/// 3 seconds is chosen to be comfortably above any legitimate `tcdrain`
/// latency on a healthy link (a 512-byte frame drains in microseconds to
/// low milliseconds) while still surfacing a wedge quickly rather than
/// silently eating into `Session`'s own 10s overall retry budget.
const SEND_TIMEOUT: Duration = Duration::from_secs(3);

/// Human-actionable recovery guidance for a HOST-side USB/cdc_acm wedge
/// observed once, on hardware, with kernel-log evidence (2026-09-22): a
/// device reset left the host kernel's per-device USB/cdc_acm state broken
/// while `journalctl -k` showed no re-enumeration event at all, and the
/// broken state SURVIVED a full device-side chip reset (the device
/// reboots, firmware, driver and peripheral registers all reinitialize, yet
/// host->device stays dead) — so a device-side action (power-cycle the
/// device, hit its reset button) does not clear it.
///
/// This is kept as a real, recoverable-in-principle *consequence* of a
/// device reset, not as an explanation for why the device resets or why a
/// connect attempt fails in the first place — see
/// `docs/provisioning-connect-verification-kit.md`'s "host-side USB/cdc_acm
/// wedge" section. It is also NOT a guarantee that clearing it is
/// sufficient: a session that clears this wedge and reaches `QUERY_STATUS`
/// can still crash later via a separate, unrelated defect (the
/// `admin_server` `pthread` stack overflow on `QUERY_ADVERT`, fixed — see
/// the kit's "Real defects found and fixed" section).
///
/// What recovers it, if anything reliably does, is **not established**:
/// the specific action once thought to clear it (physically unplugging and
/// replugging the USB cable, or the software equivalent,
/// `/sys/bus/usb/devices/<dev>/authorized` toggled 0 then 1) has since been
/// tried, more than once, against a live instance of this wedge and did NOT
/// reliably clear it. The text below states only what is actually known —
/// it does not prescribe an action that has not been verified to work.
const HOST_WEDGE_GUIDANCE: &str = "no recovery action is currently known to reliably clear \
     it; simply re-running this command will NOT help.";

/// Marks a `send_bounded` timeout as distinct from any other transport
/// error, so a caller can recognize it — via
/// `anyhow::Error::downcast_ref::<SendTimedOut>()` — without pattern-matching
/// an error message string.
///
/// One reproduction, with kernel-log evidence (2026-09-22), localized a
/// send timeout like this to the HOST's per-device USB/cdc_acm state, not
/// the device: `journalctl -k` across the reproducing connect showed no
/// re-enumeration event at all, yet the broken state survived a full
/// device-side chip reset. That reproduction does not establish this as the
/// explanation for every send timeout, and no root cause for the connect
/// failures this campaign investigated is asserted here — see
/// `docs/provisioning-connect-verification-kit.md`'s "Verified conclusion"
/// and "Eliminated hypotheses" sections.
///
/// LABELED HYPOTHESIS, not confirmed: the leading candidate for the
/// host-side mechanism observed in that one reproduction is an OUT-endpoint
/// data-toggle/sequence desync — the device-side reset reinitializes its
/// endpoints (FIFOs cleared, toggle zeroed) while the host's `cdc_acm`
/// retains its pre-reset toggle, so host->device packets are silently
/// discarded while device->host keeps working (the device drives IN
/// transfers). This explains the observed shape (unidirectional, survives
/// port close/reopen and process exit) but is NOT confirmed at the kernel
/// level; do not treat it as fact.
#[derive(Debug)]
pub struct SendTimedOut {
    pub timeout: Duration,
}

impl std::fmt::Display for SendTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "serial send timed out after {:?} (a send timeout like this has previously \
             coincided with a host-side USB/cdc_acm state that a device-side reset does not \
             clear; the underlying cause is not established -- see docs/provisioning-connect-\
             verification-kit.md; {})",
            self.timeout, HOST_WEDGE_GUIDANCE
        )
    }
}

impl std::error::Error for SendTimedOut {}

/// Real USB-serial transport backed by `serialport`.
///
/// The port is held behind `Arc<Mutex<_>>` (rather than a bare `Box`) so
/// `send` can hand a clone to a background thread and bound its own wait on
/// that thread with `SEND_TIMEOUT`, without requiring the port itself to be
/// killable — POSIX gives no way to interrupt a thread blocked in
/// `tcdrain(2)` from another thread. On a timeout, the spawned thread is
/// simply abandoned (never joined): it keeps holding the mutex (and the
/// underlying file descriptor) for as long as the underlying `tcdrain` stays
/// blocked, which for the host-side USB/cdc_acm wedge observed once on
/// hardware (see `SendTimedOut`'s doc comment) was, in that reproduction,
/// forever. `poisoned` (added to fix a descriptor leak an earlier
/// implementation of this recovery path left behind) is what stops that
/// from stranding the port silently: once `send` times out, `poisoned` is
/// set, and every later call on THIS `SerialTransport` (another `send`, a
/// `recv`, `flush_input`) checks it first and fails fast instead of
/// blocking forever on a mutex the abandoned thread will never release —
/// without this, `recv`/`flush_input` in particular would hang with no
/// timeout at all, silently, which defeats the entire point of `send` being
/// "bounded" in the first place. Recovery is never in-process (see
/// `HOST_WEDGE_GUIDANCE`): no recovery action is currently known to
/// reliably clear this wedge; a caller that hits it must open a fresh
/// `SerialTransport` and cannot assume any particular host-side action will
/// make that succeed.
pub struct SerialTransport {
    port: Arc<Mutex<Box<dyn serialport::SerialPort>>>,
    poisoned: Arc<AtomicBool>,
}

impl SerialTransport {
    /// Open `path` at `baud_rate`.  Per-read timeout: 100 ms (non-blocking feel
    /// while allowing the session layer to accumulate frames).
    ///
    /// Modem-control lines (DTR/RTS) are left at their post-open defaults and
    /// are **not** explicitly asserted or cleared.
    ///
    /// Background: a previous implementation called
    /// `write_data_terminal_ready(true)` on the hypothesis that the ESP32-S3
    /// USB-Serial-JTAG controller gated its CDC RX path on DTR.  That
    /// hypothesis was WRONG — `screen` (which leaves DTR/RTS at tty defaults)
    /// delivers bytes to the firmware correctly, while the explicit DTR
    /// assertion disrupted the modem-control state and prevented delivery.
    /// Matching `screen`'s default (no explicit DTR/RTS writes) restores the
    /// host→device byte path.
    ///
    /// The EN+IO0 reset circuit on ESP32 boards is triggered by the DTR/RTS
    /// *pair* toggling together (the esptool programming sequence); leaving
    /// both lines at their tty-open defaults also avoids inadvertent resets.
    /// On this board's ESP32-S3, external corroboration (an unrelated
    /// project hitting the identical symptom on the same chip family — see
    /// `site/provisioner/session.js`'s `connect()` doc comment) points to
    /// the precise trigger being the control-line state passing through
    /// DTR=0/RTS=1 specifically — never asserting or clearing either line
    /// at all, as this method does, structurally cannot produce that (or
    /// any other) transition. This method has never needed to change on
    /// that basis: it already avoided DTR/RTS entirely before that
    /// corroboration existed, and still does. (A later round attempted to
    /// have the BROWSER client explicitly avoid the same transition
    /// post-open; tested on hardware, it did not clear the connect wedge
    /// and was reverted — see `site/provisioner/session.js`'s `connect()`
    /// doc comment for the current state. That result does not change
    /// anything about this method.)
    ///
    /// NOTE (round 6): this `open()`/`clear()` call is bounded by the OS —
    /// device evidence shows a genuine connect-wedge hang lives in `send`'s
    /// `flush()` (see `SEND_TIMEOUT`'s doc comment), not here. An earlier
    /// round's claim that this was "the one call with no deadline" is
    /// retracted; see `docs/provisioning-connect-verification-kit.md`.
    pub fn open(path: &str, baud_rate: u32) -> anyhow::Result<Self> {
        let port = serialport::new(path, baud_rate)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| anyhow::anyhow!("cannot open {}: {}", path, e))?;
        // Flush any bytes left in the kernel receive buffer from a previous
        // process invocation.  Without this, stale response frames from an
        // earlier session can pollute the first recv_frame call and cause
        // command/response desync across separate cargo-run invocations.
        port.clear(serialport::ClearBuffer::Input)
            .map_err(|e| anyhow::anyhow!("cannot flush serial input on open: {}", e))?;
        Ok(Self {
            port: Arc::new(Mutex::new(port)),
            poisoned: Arc::new(AtomicBool::new(false)),
        })
    }
}

/// The error every call on a `SerialTransport` returns once `poisoned` is
/// set, instead of blocking forever on a mutex an abandoned `send_bounded`
/// worker thread will never release. See `SerialTransport`'s doc comment.
fn stranded_port_error() -> anyhow::Error {
    anyhow::anyhow!(
        "serial port already stranded by a previous timed-out send on this handle -- the \
         abandoned worker thread is still holding it (POSIX gives no way to interrupt a \
         blocked tcdrain(2)), so no further I/O on this handle can succeed; {}",
        HOST_WEDGE_GUIDANCE
    )
}

/// Write `data` to `port` (`write_all` + `flush`) on a background thread,
/// bounded to `timeout` from the caller's perspective.
///
/// Generic over `W` (rather than hardcoded to `Box<dyn serialport::SerialPort>`)
/// purely so this — the actual bug-fixing logic — can be exercised by a unit
/// test below without constructing a real `serialport::SerialPort` (whose
/// trait surface, `name`/`baud_rate`/`clear`/…, a hand-rolled test double
/// would otherwise have to implement in full just to reach `Write`).
///
/// If `port.flush()` (in the real `SerialTransport` case, `tcdrain(2)`) never
/// returns, the spawned thread never returns either — there is no portable
/// way to interrupt a thread blocked in a blocking syscall from the outside.
/// This function bounds only how long *this call* waits for a result, not
/// the underlying blocking operation itself; see `SEND_TIMEOUT`'s doc
/// comment on `SerialTransport` for why that is still the right fix (a
/// diagnosable error beats a silent, permanent hang) and what it does NOT
/// fix (the port is unusable for the rest of the process after a timeout).
fn send_bounded<W>(port: &Arc<Mutex<W>>, data: &[u8], timeout: Duration) -> anyhow::Result<()>
where
    W: Write + Send + 'static,
{
    let port = Arc::clone(port);
    let data = data.to_vec();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result: anyhow::Result<()> = (|| {
            let mut guard = port.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.write_all(&data)?;
            guard.flush()?;
            Ok(())
        })();
        // If the receiver already gave up (timed out) and dropped `rx`,
        // there is nothing left to notify — ignore the send error.
        let _ = tx.send(result);
    });
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow::Error::new(SendTimedOut { timeout })),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow::anyhow!(
            "serial send worker thread terminated without reporting a result"
        )),
    }
}

/// `send_bounded`, plus the descriptor-leak fix: check `poisoned` BEFORE
/// touching `port` at all, and set it on a fresh timeout.
///
/// Round 7 abandoned the timed-out thread and left the mutex (and
/// descriptor) held forever with nothing marking that fact — every *later*
/// call on the same handle would spawn yet another thread and itself block
/// on `port.lock()`, compounding the leak by one more stranded thread per
/// call. Checking `poisoned` up front means a stranded handle fails fast
/// (no lock attempt, no thread spawn) instead of silently piling up more
/// abandoned threads behind the one that is actually wedged.
///
/// Generic over `W` for the same reason as `send_bounded` — see its doc
/// comment; exercised directly by the tests below without a real
/// `serialport::SerialPort`.
fn send_bounded_guarded<W>(
    port: &Arc<Mutex<W>>,
    poisoned: &Arc<AtomicBool>,
    data: &[u8],
    timeout: Duration,
) -> anyhow::Result<()>
where
    W: Write + Send + 'static,
{
    if poisoned.load(Ordering::SeqCst) {
        return Err(stranded_port_error());
    }
    let result = send_bounded(port, data, timeout);
    if let Err(e) = &result {
        if e.downcast_ref::<SendTimedOut>().is_some() {
            poisoned.store(true, Ordering::SeqCst);
        }
    }
    result
}

impl Transport for SerialTransport {
    fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        // `write_all` + `flush` (== `tcdrain(2)` on POSIX) run on a
        // background thread so this call can never block indefinitely — see
        // `SEND_TIMEOUT`'s doc comment for the device evidence that pins the
        // hang here specifically. `send_bounded_guarded` additionally marks
        // `self.poisoned` on a timeout so later calls on this handle fail
        // fast instead of leaking one more abandoned thread each — see
        // `SerialTransport`'s doc comment.
        send_bounded_guarded(&self.port, &self.poisoned, data, SEND_TIMEOUT)
    }

    fn recv(&mut self, buf: &mut [u8]) -> anyhow::Result<usize> {
        if self.poisoned.load(Ordering::SeqCst) {
            return Err(stranded_port_error());
        }
        let mut guard = self
            .port
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(e.into()),
        }
    }

    fn flush_input(&mut self) -> anyhow::Result<()> {
        if self.poisoned.load(Ordering::SeqCst) {
            return Err(stranded_port_error());
        }
        let guard = self
            .port
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .clear(serialport::ClearBuffer::Input)
            .map_err(|e| anyhow::anyhow!("serial flush_input: {}", e))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A `Write` whose `flush()` blocks forever — the in-test stand-in for a
    /// wedged `tcdrain(2)` (device stopped draining the OUT endpoint, so the
    /// kernel's `tcdrain` wait on the TX buffer never returns). `write()`
    /// itself succeeds immediately, matching the real-world finding that the
    /// hang is specifically in `flush`/`tcdrain`, not `write`.
    struct BlockingFlushWriter {
        written: Arc<AtomicUsize>,
    }

    impl Write for BlockingFlushWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.written.fetch_add(buf.len(), Ordering::SeqCst);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            // Simulate an unbounded tcdrain(2): park this thread forever.
            // Nothing ever unparks it — same as a real wedged syscall, this
            // thread is abandoned when the test's `send_bounded` call times
            // out and returns.
            loop {
                thread::park();
            }
        }
    }

    #[test]
    fn send_bounded_returns_a_diagnosable_error_instead_of_hanging_forever() {
        let written = Arc::new(AtomicUsize::new(0));
        let port = Arc::new(Mutex::new(BlockingFlushWriter {
            written: Arc::clone(&written),
        }));

        let result = send_bounded(&port, b"hello", Duration::from_millis(100));

        assert!(
            result.is_err(),
            "a wedged flush() must surface as an error, not a hang: {:?}",
            result
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out"),
            "error should name the timeout, got: {}",
            msg
        );
        // write() itself is not what's blocked — it should have completed
        // before flush() wedged, confirming the timeout is attributable to
        // flush specifically (matches the device evidence: wchan pinned the
        // hang inside tcdrain, i.e. flush, not write).
        assert_eq!(written.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn send_bounded_succeeds_promptly_on_a_healthy_port() {
        struct InstantWriter {
            written: Vec<u8>,
        }
        impl Write for InstantWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.written.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let port = Arc::new(Mutex::new(InstantWriter {
            written: Vec::new(),
        }));
        let result = send_bounded(&port, b"hello", Duration::from_secs(1));
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(port.lock().unwrap().written, b"hello");
    }

    /// Regression guard for the round-8 descriptor-leak fix: a `send`
    /// timeout must not just report an error once — it must mark the handle
    /// so every LATER call fails immediately instead of leaking one more
    /// thread (each blocked forever on the poisoned mutex) per call.
    #[test]
    fn send_bounded_guarded_poisons_the_handle_on_timeout_and_later_calls_fail_fast() {
        let written = Arc::new(AtomicUsize::new(0));
        let port = Arc::new(Mutex::new(BlockingFlushWriter {
            written: Arc::clone(&written),
        }));
        let poisoned = Arc::new(AtomicBool::new(false));

        let first = send_bounded_guarded(&port, &poisoned, b"hello", Duration::from_millis(100));
        assert!(first.is_err(), "first call must surface the timeout");
        assert!(
            poisoned.load(Ordering::SeqCst),
            "a send timeout must poison the handle"
        );

        // A second call must NOT spawn another thread and block on the
        // still-held mutex (the first thread is parked forever, holding the
        // lock permanently) — it must fail immediately by checking
        // `poisoned` up front. Bound this with a generous timeout the
        // poisoned-fast-path has no business approaching; a regression back
        // to "spawn unconditionally" would hang this test for the full
        // duration instead.
        let started = std::time::Instant::now();
        let second = send_bounded_guarded(&port, &poisoned, b"world", Duration::from_secs(5));
        let elapsed = started.elapsed();

        assert!(second.is_err(), "a poisoned handle must keep failing");
        assert!(
            elapsed < Duration::from_secs(1),
            "a poisoned handle must fail fast (no lock attempt, no thread spawn), took {:?}",
            elapsed
        );
        let msg = second.unwrap_err().to_string();
        assert!(
            msg.contains("stranded"),
            "poisoned-handle error should say the port is stranded, got: {}",
            msg
        );
        // No further write reached the still-wedged writer from the second
        // call — only the first attempt's "hello" (5 bytes) landed.
        assert_eq!(written.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn send_bounded_guarded_does_not_poison_a_healthy_port() {
        struct InstantWriter;
        impl Write for InstantWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let port = Arc::new(Mutex::new(InstantWriter));
        let poisoned = Arc::new(AtomicBool::new(false));

        let result = send_bounded_guarded(&port, &poisoned, b"hello", Duration::from_secs(1));

        assert!(result.is_ok(), "{:?}", result);
        assert!(
            !poisoned.load(Ordering::SeqCst),
            "a successful send must not poison the handle"
        );
    }
}
