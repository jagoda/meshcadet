// SPDX-License-Identifier: GPL-3.0-only
//! USB-serial transport layer for the MeshCadet host CLI.
//!
//! Defines the `Transport` trait (byte-stream I/O) and its concrete
//! `SerialTransport` implementation backed by the `serialport` crate.
//!
//! Test code can implement `Transport` directly (see `tests/integration.rs`
//! for `MockTransport`).

use std::io::{Read, Write};
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
/// This refutes round 5's finding that `SerialTransport::open()` was "the
/// one call in the host CLI's entire path with no deadline" — see that
/// method's own doc comment, retracted below, and
/// `docs/provisioning-connect-verification-kit.md`'s finding 1 retraction.
///
/// 3 seconds is chosen to be comfortably above any legitimate `tcdrain`
/// latency on a healthy link (a 512-byte frame drains in microseconds to
/// low milliseconds) while still surfacing a wedge quickly rather than
/// silently eating into `Session`'s own 10s overall retry budget.
const SEND_TIMEOUT: Duration = Duration::from_secs(3);

/// Real USB-serial transport backed by `serialport`.
///
/// The port is held behind `Arc<Mutex<_>>` (rather than a bare `Box`) so
/// `send` can hand a clone to a background thread and bound its own wait on
/// that thread with `SEND_TIMEOUT`, without requiring the port itself to be
/// killable — POSIX gives no way to interrupt a thread blocked in
/// `tcdrain(2)` from another thread. On a timeout, the spawned thread is
/// simply abandoned (never joined): it keeps holding the mutex for as long
/// as the underlying `tcdrain` stays blocked, which means any *later* call
/// on this same `SerialTransport` (another `send`, a `recv`, `flush_input`)
/// will itself block trying to acquire the lock. This is intentional and
/// harmless in practice: `host/src/main.rs` is a one-shot-per-invocation CLI
/// (see its `fn main` doc comment) — a `send` timeout error propagates
/// straight up through `Session` and out of `main`, and the process exits
/// before any further transport call could be attempted.
pub struct SerialTransport {
    port: Arc<Mutex<Box<dyn serialport::SerialPort>>>,
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
        })
    }
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
        Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow::anyhow!(
            "serial send timed out after {:?} (device stopped accepting bytes on the transmit \
             path — likely a blocked tcdrain(2); see SEND_TIMEOUT's doc comment in transport.rs)",
            timeout
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow::anyhow!(
            "serial send worker thread terminated without reporting a result"
        )),
    }
}

impl Transport for SerialTransport {
    fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        // `write_all` + `flush` (== `tcdrain(2)` on POSIX) run on a
        // background thread so this call can never block indefinitely — see
        // `SEND_TIMEOUT`'s doc comment for the device evidence that pins the
        // hang here specifically.
        send_bounded(&self.port, data, SEND_TIMEOUT)
    }

    fn recv(&mut self, buf: &mut [u8]) -> anyhow::Result<usize> {
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

        let port = Arc::new(Mutex::new(InstantWriter { written: Vec::new() }));
        let result = send_bounded(&port, b"hello", Duration::from_secs(1));
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(port.lock().unwrap().written, b"hello");
    }
}
