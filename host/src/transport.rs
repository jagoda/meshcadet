// SPDX-License-Identifier: GPL-3.0-only
//! USB-serial transport layer for the MeshCadet host CLI.
//!
//! Defines the `Transport` trait (byte-stream I/O) and its concrete
//! `SerialTransport` implementation backed by the `serialport` crate.
//!
//! Test code can implement `Transport` directly (see `tests/integration.rs`
//! for `MockTransport`).

use std::io::{Read, Write};
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

/// Real USB-serial transport backed by `serialport`.
pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
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
        Ok(Self { port })
    }
}

impl Transport for SerialTransport {
    fn send(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.port.write_all(data)?;
        // Flush ensures the kernel serial buffer is pushed to the USB endpoint
        // immediately; without it, small frames can sit in the write buffer
        // until a subsequent write triggers a drain, causing spurious timeouts.
        self.port.flush()?;
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> anyhow::Result<usize> {
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(e.into()),
        }
    }

    fn flush_input(&mut self) -> anyhow::Result<()> {
        self.port
            .clear(serialport::ClearBuffer::Input)
            .map_err(|e| anyhow::anyhow!("serial flush_input: {}", e))
    }
}
