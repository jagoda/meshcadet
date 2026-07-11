// SPDX-License-Identifier: GPL-3.0-only
//! SX1262 LoRa driver — locked MeshCadet preset.
//!
//! Implements a minimal synchronous SPI command driver for the SX1262 as
//! fitted on the LilyGo T-Deck Plus.  Uses esp-idf-hal SPI directly rather
//! than an async LoRa-phy stack, matching the single-task polling model of
//! the MeshCore dispatcher loop.
//!
//! # Locked preset (MeshCore v1.15 interop)
//! | Parameter | Value |
//! |-----------|-------|
//! | Frequency | 910.525 MHz |
//! | Bandwidth | 62.5 kHz |
//! | Spreading Factor | 7 |
//! | Coding Rate | 4/5 |
//! | TX Power | +22 dBm (SX1262 HP PA) |
//! | Preamble | 8 symbols |
//! | Sync Word | 0x1424 (LoRa private network) |
//! | Path hash size | 2 bytes |
//! | Header mode | Explicit |
//! | CRC | On |
//!
//! # T-Deck Plus SX1262 pin mapping
//! Verified against LilyGo `examples/UnitTest/utilities.h` (Xinyuan-LilyGO/T-Deck).
//! ```
//! BOARD_POWERON → GPIO 10   (master peripheral rail; drive HIGH before SPI)
//! SPI SCK   → GPIO 40       (BOARD_SPI_SCK)
//! SPI MISO  → GPIO 38       (BOARD_SPI_MISO)
//! SPI MOSI  → GPIO 41       (BOARD_SPI_MOSI)
//! SPI CS    → GPIO 9        (RADIO_CS_PIN)
//! RESET     → GPIO 17       (RADIO_RST_PIN)
//! BUSY      → GPIO 13       (RADIO_BUSY_PIN)
//! DIO1/IRQ  → GPIO 45       (RADIO_DIO1_PIN)
//! ```
//! The SX1262 reference clock is a TCXO powered from DIO3 — the LilyGo UnitTest
//! brings the radio up via RadioLib `begin()`, whose default `tcxoVoltage` is
//! 1.6 V; the driver must therefore issue SetDIO3AsTcxoCtrl before XOSC.
//!
//! Source references: SX1262 datasheet rev 2.1 (Semtech), MeshCore
//! `src/helpers/SX126XLT.cpp` @ dee3e26a.

use esp_idf_hal::{
    delay::FreeRtos,
    gpio::{Input, Output, PinDriver},
    spi::{SpiDeviceDriver, SpiDriver},
};

// ── Pin assignments ───────────────────────────────────────────────────────────

// All pins per LilyGo examples/UnitTest/utilities.h (Xinyuan-LilyGO/T-Deck).
/// Master peripheral power-enable (BOARD_POWERON) — must be HIGH before SPI.
#[allow(dead_code)]
pub const PIN_POWERON: i32 = 10;
#[allow(dead_code)]
pub const PIN_SCK: i32 = 40; // BOARD_SPI_SCK
#[allow(dead_code)]
pub const PIN_MISO: i32 = 38; // BOARD_SPI_MISO
#[allow(dead_code)]
pub const PIN_MOSI: i32 = 41; // BOARD_SPI_MOSI
#[allow(dead_code)]
pub const PIN_CS: i32 = 9; // RADIO_CS_PIN
#[allow(dead_code)]
pub const PIN_RST: i32 = 17; // RADIO_RST_PIN
#[allow(dead_code)]
pub const PIN_BUSY: i32 = 13; // RADIO_BUSY_PIN
#[allow(dead_code)]
pub const PIN_DIO1: i32 = 45; // RADIO_DIO1_PIN

// ── Preset constants ──────────────────────────────────────────────────────────

/// RF frequency register word for 910.525 MHz.
///
/// Formula: freq_word = round(freq_hz × 2²⁵ / 32_000_000)
/// = round(910_525_000 × 33_554_432 / 32_000_000) = 954_754_662  (0x38E8_6666)
pub const FREQ_WORD: u32 = 0x38E8_6666;

/// Bandwidth: 62.5 kHz → SX1262 register code 0x03.
///
/// DS_SX1261-2 V2.1, Table 13-47 (LoRa ModParam2 BW): 0x03 = 62.5 kHz,
/// 0x04 = 125 kHz. The deployed MeshCore mesh runs 62.5 kHz; an earlier
/// value of 0x04 put the radio off-bandwidth (125 kHz) and broke RF interop.
pub const BW_CODE: u8 = 0x03;
/// Spreading Factor: SF7.
pub const SF_CODE: u8 = 7;
/// Coding Rate: CR 4/5 → SX1262 register code 0x01.
pub const CR_CODE: u8 = 0x01;
/// Low Data Rate Optimize: disabled (symbol time < 16 ms at SF7/62.5 kHz).
pub const LDRO_CODE: u8 = 0x00;

/// Preamble length (symbols).
pub const PREAMBLE_LEN: u16 = 8;
/// LoRa private-network sync word, hi/lo register bytes.
///
/// MeshCore v1.15 stock nodes bring the radio up via RadioLib in
/// `src/helpers/radiolib/CustomSX1262.h` `std_init()`:
/// `begin(..., RADIOLIB_SX126X_SYNC_WORD_PRIVATE, ...)`. In RadioLib that symbol
/// is the 1-byte value `0x12` (`SX126x_registers.h`); `SX126x::setSyncWord`
/// (`SX126x_config.cpp`) expands it with the default control bits `0x44` into the
/// two bytes written at `REG_LORA_SYNC_WORD` (0x0740/0x0741):
///   data[0] = (0x12 & 0xF0) | ((0x44 & 0xF0) >> 4) = 0x14
///   data[1] = ((0x12 & 0x0F) << 4) | (0x44 & 0x0F) = 0x24
/// → register word 0x1424. The earlier 0x3444 is RadioLib's *public* word (0x34);
/// the SX1262 matches the LoRa sync word in hardware during demod, so a stock
/// MeshCore node's packets were filtered at the PHY before RxDone/CrcErr could
/// fire — the exact "zero RX, not even a CRC error" symptom.
pub const SYNC_WORD_HI: u8 = 0x14;
pub const SYNC_WORD_LO: u8 = 0x24;

/// PA config for +22 dBm on SX1262 HP PA path.
pub const PA_DUTY_CYCLE: u8 = 0x04;
pub const PA_HP_MAX: u8 = 0x07;
/// TX power: +22 dBm (SX1262 raw value = 0x16).
pub const TX_POWER: u8 = 0x16;
/// Ramp time: 200 µs (code 0x04).
pub const RAMP_TIME: u8 = 0x04;

// ── SX1262 opcodes ────────────────────────────────────────────────────────────

const CMD_SET_STANDBY: u8 = 0x80;
const CMD_SET_PACKET_TYPE: u8 = 0x8A;
const CMD_SET_RF_FREQUENCY: u8 = 0x86;
const CMD_SET_PA_CONFIG: u8 = 0x95;
const CMD_SET_TX_PARAMS: u8 = 0x8E;
const CMD_SET_MODULATION_PARAMS: u8 = 0x8B;
const CMD_SET_PACKET_PARAMS: u8 = 0x8C;
const CMD_WRITE_REGISTER: u8 = 0x0D;
const CMD_SET_DIO_IRQ_PARAMS: u8 = 0x08;
const CMD_CLEAR_IRQ_STATUS: u8 = 0x02;
const CMD_GET_IRQ_STATUS: u8 = 0x12;
const CMD_WRITE_BUFFER: u8 = 0x0E;
const CMD_READ_BUFFER: u8 = 0x1E;
const CMD_GET_RX_BUFFER_STATUS: u8 = 0x13;
const CMD_GET_PACKET_STATUS: u8 = 0x14;
const CMD_SET_RX: u8 = 0x82;
const CMD_SET_TX: u8 = 0x83;
const CMD_SET_CAD: u8 = 0xC5;
const CMD_SET_CAD_PARAMS: u8 = 0x88;
#[allow(dead_code)]
const CMD_RESET: u8 = 0x00; // Not an SPI command; handled via GPIO
const CMD_CALIBRATE: u8 = 0x89;
const CMD_SET_DIO3_AS_TCXO_CTRL: u8 = 0x97;
const CMD_SET_DIO2_AS_RF_SWITCH_CTRL: u8 = 0x9D;
const CMD_SET_REGULATOR_MODE: u8 = 0x96;
const CMD_CALIBRATE_IMAGE: u8 = 0x98;
const CMD_READ_REGISTER: u8 = 0x1D;

// ── Standby / TCXO constants (SX1262 datasheet rev 2.1, Semtech) ───────────────

/// SetStandby config byte: 0x00 = STDBY_RC (internal RC), 0x01 = STDBY_XOSC.
const STDBY_RC: u8 = 0x00;
const STDBY_XOSC: u8 = 0x01;

/// SetDIO3AsTcxoCtrl tcxoVoltage code (DS Table 13-35): 0x00 = 1.6 V.
/// The T-Deck Plus SX1262 reference is a DIO3-powered TCXO; the LilyGo UnitTest
/// relies on RadioLib `begin()`'s default `tcxoVoltage` of 1.6 V.
const TCXO_CTRL_1_6V: u8 = 0x00;

/// TCXO startup timeout in 15.625 µs steps. 5000 µs / 15.625 µs = 320 = 0x000140,
/// matching RadioLib SX126x::setTCXO's default 5000 µs delay.
const TCXO_TIMEOUT_STEPS: u32 = 320;

/// Calibrate parameter (DS Table 13-30): 0x7F = recalibrate all blocks. Required
/// after changing the reference clock to the TCXO.
const CALIBRATE_ALL: u8 = 0x7F;

/// SetRegulatorMode data byte (DS Table 13-21): 0x00 = LDO (reset default),
/// 0x01 = DC-DC + LDO. Stock RadioLib `SX126x::modSetup()` issues this right
/// after the post-reset `Calibrate(0x7F)` (`SX126x_commands.cpp` `setRegulatorMode`,
/// called from `modSetup` via `setRegulatorDCDC()` since MeshCore's
/// `CustomSX1262::std_init()` never passes `useRegulatorLDO = true`). The T-Deck
/// Plus SX1262 is DC-DC capable; leaving the reset-default LDO-only regulation
/// in place wastes power and — per DS §9.1 — increases current-limited PA droop
/// under TX load versus the DC-DC-assisted path stock nodes run.
const REGULATOR_DCDC: u8 = 0x01;

/// CalibrateImage frequency-band bytes for the 902–928 MHz band (DS Table 13-75 /
/// RadioLib `RADIOLIB_SX126X_CAL_IMG_902_MHZ_{1,2}`), which covers the locked
/// 910.525 MHz preset. Must be issued after `SetRfFrequency` (DS §13.4.11): it
/// tunes image-rejection calibration for the actual RF band in use, distinct
/// from the wideband `Calibrate(0x7F)` already run once in `configure_tcxo`
/// after the reference-clock switch.
const CAL_IMAGE_902MHZ_1: u8 = 0xE1;
const CAL_IMAGE_902MHZ_2: u8 = 0xE9;

/// TX-clamp antenna-mismatch erratum register (DS_SX1261-2 V2.1 Chapter 15,
/// §15.2 "Better Resistance of the SX1262 to Antenna Mismatch"): bits [4:1] of
/// register 0x08D8 must be set to force the PA clamping threshold higher,
/// preventing the TX PA from folding back its output under antenna mismatch.
/// Mirrors RadioLib `SX126x::fixPaClamping()` (`SX126x.cpp`): read-modify-write
/// with `clampConfig |= 0x1E`, preserving the other register bits.
const REG_TX_CLAMP_CONFIG: u16 = 0x08D8;
const TX_CLAMP_CONFIG_MASK: u8 = 0x1E;

/// SetDIO2AsRfSwitchCtrl data byte (DS_SX1261-2 V2.1 §13.3.5 / Table 13-25):
/// 0x00 = DIO2 acts as a normal IRQ line; 0x01 = DIO2 drives the *external RF
/// switch*, auto-toggling with the radio's TX/RX state (HIGH in TX → PA path,
/// LOW in RX → LNA path; the SX1262 sequences it internally).
///
/// The T-Deck Plus wires the SX1262's DIO2 to its external antenna SPDT switch:
/// `examples/UnitTest/utilities.h` defines *no* RXEN/TXEN/LNA-enable GPIO, and
/// stock bring-up (RadioLib `SX126x::begin()`, T-Deck `LoRaWAN_Starter.ino`)
/// unconditionally calls `setDio2AsRfSwitch(true)` → this opcode with 0x01.
/// Left at the reset default (0x00 = IRQ), DIO2 sits low, so the external switch
/// stays on its default (PA/TX) leg: TX radiates fine but the LNA/RX leg is never
/// connected and the PHY hears nothing — the exact "0 RxDone / 0 CrcErr" symptom.
const DIO2_AS_RF_SWITCH: u8 = 0x01;

/// SX1262 LoRa sync word register address (two bytes at 0x0740–0x0741).
const REG_LORA_SYNC_WORD: u16 = 0x0740;

// ── IRQ bit masks ─────────────────────────────────────────────────────────────

pub const IRQ_TX_DONE: u16 = 1 << 0;
pub const IRQ_RX_DONE: u16 = 1 << 1;
pub const IRQ_CRC_ERR: u16 = 1 << 6;
pub const IRQ_CAD_DONE: u16 = 1 << 7;
pub const IRQ_CAD_DETECTED: u16 = 1 << 8;

// ── Radio struct ──────────────────────────────────────────────────────────────

/// SX1262 radio driver.
pub struct Radio<'d> {
    spi: SpiDeviceDriver<'d, &'d SpiDriver<'d>>,
    rst: PinDriver<'d, Output>,
    busy: PinDriver<'d, Input>,
    dio1: PinDriver<'d, Input>,
    /// `true` while the radio is armed in continuous RX (SetRx 0xFFFFFF).
    /// `transmit` and `channel_activity_detection` clear it because they take
    /// the radio out of RX; `ensure_continuous_rx` re-arms only when it is clear,
    /// so the steady state issues SetRx exactly once and never re-arms per loop.
    in_continuous_rx: bool,
}

impl<'d> Radio<'d> {
    /// Initialise the SX1262 at the locked MeshCadet preset.
    ///
    /// Performs a hardware reset, waits for BUSY to deassert, then programs
    /// all modulation/packet/PA/sync-word registers.  Leaves the radio in
    /// Standby-XOSC mode, ready for `transmit()` or `start_receive()`.
    pub fn init(
        spi: SpiDeviceDriver<'d, &'d SpiDriver<'d>>,
        rst: PinDriver<'d, Output>,
        busy: PinDriver<'d, Input>,
        dio1: PinDriver<'d, Input>,
    ) -> Result<Self, RadioError> {
        let mut radio = Self { spi, rst, busy, dio1, in_continuous_rx: false };
        radio.hardware_reset()?;
        radio.configure_tcxo()?;
        radio.configure_preset()?;
        Ok(radio)
    }

    // ── Transmit ─────────────────────────────────────────────────────────────

    /// Transmit `frame` (already fully encoded wire bytes).
    ///
    /// Blocks until TxDone IRQ fires (or timeout after ~500 ms).
    /// Returns the actual airtime in milliseconds.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<u32, RadioError> {
        let n = frame.len().min(255) as u8;
        let airtime = crate::dispatcher::lora_airtime_ms(n as usize);

        self.wait_not_busy()?;
        // Set payload length in packet params (field index 2 = PayloadLength)
        self.set_packet_params(n)?;

        self.wait_not_busy()?;
        // Write payload into TX buffer at offset 0
        let mut cmd = [0u8; 2 + 255];
        cmd[0] = CMD_WRITE_BUFFER;
        cmd[1] = 0x00; // buffer offset
        cmd[2..2 + frame.len()].copy_from_slice(frame);
        self.spi_transfer(&mut cmd[..2 + frame.len()])?;

        self.wait_not_busy()?;
        // Enable TxDone + Timeout IRQs on DIO1
        self.write_cmd(&[
            CMD_SET_DIO_IRQ_PARAMS,
            0x00, (IRQ_TX_DONE as u8),   // IRQ mask hi/lo
            0x00, (IRQ_TX_DONE as u8),   // DIO1 mask hi/lo
            0x00, 0x00,                  // DIO2
            0x00, 0x00,                  // DIO3
        ])?;
        self.clear_irq(0xFFFF)?;

        // SetTx with timeout 0 (no timeout; we poll DIO1). This takes the radio
        // out of continuous RX, so clear the armed flag — `ensure_continuous_rx`
        // re-arms it on the next `try_receive`, immediately after this TX.
        self.in_continuous_rx = false;
        self.wait_not_busy()?;
        self.write_cmd(&[CMD_SET_TX, 0x00, 0x00, 0x00])?;

        // Poll DIO1 (TxDone)
        let deadline_ms = airtime as u64 + 500; // generous timeout
        let start = uptime_ms();
        loop {
            if self.dio1.is_high() {
                break;
            }
            if uptime_ms() - start > deadline_ms {
                return Err(RadioError::TxTimeout);
            }
            FreeRtos::delay_ms(1);
        }
        self.clear_irq(IRQ_TX_DONE)?;
        Ok(airtime)
    }

    // ── Receive ──────────────────────────────────────────────────────────────

    /// Arm the radio for TRUE continuous RX and enable RxDone + CrcErr on DIO1.
    ///
    /// Issues `SetRx` with the 24-bit timeout 0xFFFFFF (DS_SX1261-2 V2.1
    /// §13.1.5 = continuous RX): the radio never times out and stays in RX
    /// across packets. RxDone latches DIO1 high the instant a packet completes
    /// and holds it until cleared, so a packet whose preamble arrives *between*
    /// software polls is still demodulated and waiting at the next poll — the
    /// property the old windowed `SetRx`/standby-on-timeout path lacked.
    ///
    /// Always issues the SPI commands and sets `in_continuous_rx`. Prefer
    /// `ensure_continuous_rx` on the hot path so re-arming happens only after a
    /// TX or CAD pass actually took the radio out of RX.
    pub fn start_continuous_rx(&mut self) -> Result<(), RadioError> {
        self.wait_not_busy()?;
        // Enable RxDone + CrcErr IRQs on DIO1.
        self.write_cmd(&[
            CMD_SET_DIO_IRQ_PARAMS,
            (IRQ_RX_DONE >> 8) as u8, (IRQ_RX_DONE | IRQ_CRC_ERR) as u8,
            (IRQ_RX_DONE >> 8) as u8, (IRQ_RX_DONE | IRQ_CRC_ERR) as u8,
            0x00, 0x00,
            0x00, 0x00,
        ])?;
        self.clear_irq(0xFFFF)?;
        // SetRx 0xFFFFFF = continuous RX. Issued ONCE; not re-armed per loop.
        self.wait_not_busy()?;
        self.write_cmd(&[CMD_SET_RX, 0xFF, 0xFF, 0xFF])?;
        self.in_continuous_rx = true;
        Ok(())
    }

    /// Ensure the radio is listening in continuous RX, re-arming only if a TX or
    /// CAD pass cleared the armed flag. A genuine no-op (zero SPI traffic) when
    /// already in RX — this is what eliminates the per-loop re-arm and the
    /// standby gap that made the windowed path drop async inbound packets.
    pub fn ensure_continuous_rx(&mut self) -> Result<(), RadioError> {
        if !self.in_continuous_rx {
            self.start_continuous_rx()?;
        }
        Ok(())
    }

    /// Poll the continuously-listening radio for a completed packet.
    ///
    /// The radio is (and remains) in continuous RX for the entire call: this
    /// only watches DIO1 for up to `poll_ms` to give the dispatcher a yield
    /// cadence — it is NOT a listening window. It never re-arms `SetRx` and
    /// never drops to standby, so nothing is missed between calls.
    ///
    /// Returns `Ok(None)` if no packet has completed, `Ok(Some(len))` with `buf`
    /// filled on RxDone, or `Err(CrcError)` on a CRC failure (RSSI/SNR remain
    /// valid via `get_packet_status` until the next RX operation).
    pub fn try_receive(
        &mut self,
        buf: &mut [u8; 255],
        poll_ms: u32,
    ) -> Result<Option<usize>, RadioError> {
        // Arm continuous RX if a prior TX/CAD took us out of it; no-op otherwise.
        self.ensure_continuous_rx()?;

        // Watch DIO1. The radio stays in continuous RX throughout — this loop
        // only spreads the poll over `poll_ms` so the task yields; it does NOT
        // re-arm RX and does NOT drop to standby on expiry.
        let deadline = uptime_ms() + poll_ms as u64;
        loop {
            if self.dio1.is_high() {
                break;
            }
            if uptime_ms() >= deadline {
                return Ok(None);
            }
            FreeRtos::delay_ms(1);
        }

        // A DIO1 edge fired — read and clear the IRQ. In continuous RX the radio
        // keeps listening after RxDone, so `in_continuous_rx` stays true.
        let irq = self.get_irq()?;
        self.clear_irq(0xFFFF)?;
        if irq & IRQ_CRC_ERR != 0 {
            return Err(RadioError::CrcError);
        }
        if irq & IRQ_RX_DONE == 0 {
            return Ok(None);
        }

        // GetRxBufferStatus → (payloadLength, rxBufferOffset)
        let status = self.get_rx_buffer_status()?;
        let payload_len = status.0 as usize;
        let offset = status.1;

        // ReadBuffer
        let mut read_cmd = [0u8; 3 + 255];
        read_cmd[0] = CMD_READ_BUFFER;
        read_cmd[1] = offset;
        read_cmd[2] = 0x00; // NOP status byte
        self.spi_transfer(&mut read_cmd[..3 + payload_len])?;
        buf[..payload_len].copy_from_slice(&read_cmd[3..3 + payload_len]);

        Ok(Some(payload_len))
    }

    // ── Channel Activity Detection ────────────────────────────────────────────

    /// Perform a single CAD pass.
    ///
    /// Returns `true` if channel activity was detected (back off before TX).
    pub fn channel_activity_detection(&mut self) -> Result<bool, RadioError> {
        self.wait_not_busy()?;
        // SetCad is only valid from STANDBY (DS §13.1.7 / Table 11-3). The
        // continuous-RX refactor holds SetRx 0xFFFFFF and never drops to
        // standby, so issuing SetCad while still in RX leaves CadDone unfired
        // and every poll times out (CadTimeout). Take the radio out of
        // continuous RX into STDBY_XOSC first; the next `try_receive` re-arms
        // continuous RX (in_continuous_rx is cleared below).
        self.write_cmd(&[CMD_SET_STANDBY, STDBY_XOSC])?;
        self.in_continuous_rx = false;
        self.wait_not_busy()?;
        // CAD params: 2 CAD symbols, peak threshold / min threshold per Semtech AN
        self.write_cmd(&[
            CMD_SET_CAD_PARAMS,
            0x02,  // cadSymbolNum = CAD_ON_4_SYMB (Table 13-58: 0x02 = 4 symbols)
            0x22,  // cadDetPeak (SF7 recommended value)
            0x0A,  // cadDetMin
            0x00,  // cadExitMode = 0 (return to STBY after CAD)
            0x00, 0x00, 0x00, // cadTimeout (unused for exit mode 0)
        ])?;
        self.clear_irq(0xFFFF)?;
        self.write_cmd(&[
            CMD_SET_DIO_IRQ_PARAMS,
            (IRQ_CAD_DONE >> 8) as u8, ((IRQ_CAD_DONE | IRQ_CAD_DETECTED) as u8),
            (IRQ_CAD_DONE >> 8) as u8, ((IRQ_CAD_DONE | IRQ_CAD_DETECTED) as u8),
            0x00, 0x00,
            0x00, 0x00,
        ])?;

        // The radio is already in STDBY_XOSC (set above) and the armed flag is
        // cleared, so SetCad runs from a CAD-valid state and CadDone will fire.
        self.wait_not_busy()?;
        self.write_cmd(&[CMD_SET_CAD])?;

        // Poll up to 20 ms for CAD completion
        let deadline = uptime_ms() + 20;
        loop {
            if self.dio1.is_high() {
                break;
            }
            if uptime_ms() > deadline {
                return Err(RadioError::CadTimeout);
            }
            FreeRtos::delay_ms(1);
        }

        let irq = self.get_irq()?;
        self.clear_irq(0xFFFF)?;
        Ok(irq & IRQ_CAD_DETECTED != 0)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn hardware_reset(&mut self) -> Result<(), RadioError> {
        self.rst.set_low().map_err(|_| RadioError::Spi)?;
        FreeRtos::delay_ms(1);
        self.rst.set_high().map_err(|_| RadioError::Spi)?;
        FreeRtos::delay_ms(10);
        self.wait_not_busy()
    }

    /// Power and start the DIO3-controlled TCXO, then recalibrate.
    ///
    /// The T-Deck Plus SX1262 derives its reference clock from a TCXO whose
    /// supply is gated by the chip's DIO3 pin. After reset the part runs on its
    /// internal RC oscillator (STDBY_RC); if we switch straight to XOSC the
    /// crystal/TCXO oscillator never receives power, never starts, and BUSY
    /// latches high forever (the BusyTimeout fault). This sequence — drawn from
    /// the SX1262 datasheet §9.2.2 / §13.3.6 and §13.1.12 — must run before any
    /// SetStandby(XOSC):
    ///   1. SetStandby(STDBY_RC)
    ///   2. SetDIO3AsTcxoCtrl(voltage, startup-timeout)
    ///   3. Calibrate(all blocks)  [reference clock changed → recalibrate]
    ///   4. SetRegulatorMode(DC-DC) — stock RadioLib issues this immediately
    ///      after the post-reset Calibrate(all), still in STDBY_RC, before any
    ///      packet/frequency configuration (`SX126x::modSetup()`).
    fn configure_tcxo(&mut self) -> Result<(), RadioError> {
        // 1. Ensure we are on the internal RC oscillator before touching DIO3.
        self.write_cmd(&[CMD_SET_STANDBY, STDBY_RC])?;
        self.wait_not_busy()?;

        // 2. SetDIO3AsTcxoCtrl: power the TCXO from DIO3 at 1.6 V; 24-bit startup
        //    timeout in 15.625 µs steps (DS Table 13-35).
        self.write_cmd(&[
            CMD_SET_DIO3_AS_TCXO_CTRL,
            TCXO_CTRL_1_6V,
            ((TCXO_TIMEOUT_STEPS >> 16) & 0xFF) as u8,
            ((TCXO_TIMEOUT_STEPS >> 8) & 0xFF) as u8,
            (TCXO_TIMEOUT_STEPS & 0xFF) as u8,
        ])?;
        self.wait_not_busy()?;

        // 3. Recalibrate all blocks now the reference clock has changed.
        self.write_cmd(&[CMD_CALIBRATE, CALIBRATE_ALL])?;
        // Full calibration takes up to ~3.5 ms; BUSY rises during it. Give margin
        // before polling so wait_not_busy observes a settled line.
        FreeRtos::delay_ms(5);
        self.wait_not_busy()?;

        // 4. SetRegulatorMode(DC-DC) — still in STDBY_RC. Reset default is
        //    LDO-only; stock nodes run DC-DC (RadioLib `useRegulatorLDO`
        //    defaults false), matching the T-Deck Plus's DC-DC-capable supply.
        self.write_cmd(&[CMD_SET_REGULATOR_MODE, REGULATOR_DCDC])?;
        self.wait_not_busy()?;
        Ok(())
    }

    fn configure_preset(&mut self) -> Result<(), RadioError> {
        // 1. Standby (XOSC) — safe now that the TCXO is powered and started.
        self.write_cmd(&[CMD_SET_STANDBY, STDBY_XOSC])?;
        self.wait_not_busy()?;

        // 2. Packet type = LoRa (0x01)
        self.write_cmd(&[CMD_SET_PACKET_TYPE, 0x01])?;
        self.wait_not_busy()?;

        // 3. RF frequency
        let fw = FREQ_WORD;
        self.write_cmd(&[
            CMD_SET_RF_FREQUENCY,
            ((fw >> 24) & 0xFF) as u8,
            ((fw >> 16) & 0xFF) as u8,
            ((fw >> 8) & 0xFF) as u8,
            (fw & 0xFF) as u8,
        ])?;
        self.wait_not_busy()?;

        // 4. CalibrateImage for the 902–928 MHz band (covers 910.525 MHz), issued
        //    after SetRfFrequency per DS §13.4.11. Distinct from the wideband
        //    Calibrate(0x7F) in configure_tcxo: this tunes image rejection for the
        //    specific RF band in use. Mirrors RadioLib SX1262::setFrequency() →
        //    calibrateImage(freq) around the SetRfFrequency call.
        //
        //    Same BUSY-rise race as the wideband Calibrate(0x7F) above: BUSY
        //    takes a moment to assert after the SPI transaction completes, so
        //    wait_not_busy()'s first check can read a not-yet-risen line and
        //    return immediately, treating a still-running calibration as done.
        //    Give it the same settling margin before polling.
        self.write_cmd(&[CMD_CALIBRATE_IMAGE, CAL_IMAGE_902MHZ_1, CAL_IMAGE_902MHZ_2])?;
        FreeRtos::delay_ms(2);
        self.wait_not_busy()?;

        // 5. TX-clamp antenna-mismatch erratum (DS_SX1261-2 V2.1 Chapter 15 §15.2):
        //    read-modify-write register 0x08D8, forcing bits [4:1] high to raise
        //    the PA clamping threshold. Preserves the other register bits — a
        //    blind overwrite would corrupt reset-default bits this register also
        //    carries. Mirrors RadioLib SX126x::fixPaClamping()'s
        //    `clampConfig |= 0x1E` read-modify-write, called after SetRfFrequency
        //    and before SetPaConfig/SetTxParams.
        let clamp_config = self.read_register(REG_TX_CLAMP_CONFIG)? | TX_CLAMP_CONFIG_MASK;
        self.write_cmd(&[
            CMD_WRITE_REGISTER,
            ((REG_TX_CLAMP_CONFIG >> 8) & 0xFF) as u8,
            (REG_TX_CLAMP_CONFIG & 0xFF) as u8,
            clamp_config,
        ])?;
        self.wait_not_busy()?;

        // 6. PA config (+22 dBm HP PA)
        self.write_cmd(&[CMD_SET_PA_CONFIG, PA_DUTY_CYCLE, PA_HP_MAX, 0x00, 0x01])?;
        self.wait_not_busy()?;

        // 7. TX params
        self.write_cmd(&[CMD_SET_TX_PARAMS, TX_POWER, RAMP_TIME])?;
        self.wait_not_busy()?;

        // 8. Modulation params: SF7, BW 62.5 kHz, CR 4/5, LDRO off
        self.write_cmd(&[CMD_SET_MODULATION_PARAMS, SF_CODE, BW_CODE, CR_CODE, LDRO_CODE])?;
        self.wait_not_busy()?;

        // 9. Packet params (initial payload length = 0; updated per-TX)
        self.set_packet_params(0)?;
        self.wait_not_busy()?;

        // 10. LoRa sync word: write 0x1424 (RadioLib PRIVATE) to regs 0x0740–0x0741
        //    WriteRegister opcode: 0x0D addr_hi addr_lo data...
        self.write_cmd(&[
            CMD_WRITE_REGISTER,
            ((REG_LORA_SYNC_WORD >> 8) & 0xFF) as u8,
            (REG_LORA_SYNC_WORD & 0xFF) as u8,
            SYNC_WORD_HI,
            SYNC_WORD_LO,
        ])?;
        self.wait_not_busy()?;

        // 11. Route DIO2 to the external antenna RF switch (SetDIO2AsRfSwitchCtrl,
        //    opcode 0x9D, data 0x01). The T-Deck Plus has no discrete RXEN/TXEN/LNA
        //    GPIO (see utilities.h); the SX1262's DIO2 drives the board's antenna
        //    SPDT switch and must be put under radio control so it auto-toggles the
        //    LNA path into circuit during RX. Without this the switch idles on its
        //    PA leg, leaving the receiver deaf at the PHY. Mirrors stock RadioLib
        //    SX126x::begin() → setDio2AsRfSwitch(true), issued in STDBY.
        self.write_cmd(&[CMD_SET_DIO2_AS_RF_SWITCH_CTRL, DIO2_AS_RF_SWITCH])?;
        self.wait_not_busy()?;

        log::info!(
            "radio: SX1262 configured — 910.525 MHz / 62.5 kHz / SF7 / CR4/5 / +22 dBm / DIO2 RF-switch on"
        );
        Ok(())
    }

    /// SetPacketParams for LoRa explicit-header, CRC-on, no IQ inversion.
    fn set_packet_params(&mut self, payload_len: u8) -> Result<(), RadioError> {
        let pre_hi = ((PREAMBLE_LEN >> 8) & 0xFF) as u8;
        let pre_lo = (PREAMBLE_LEN & 0xFF) as u8;
        self.write_cmd(&[
            CMD_SET_PACKET_PARAMS,
            pre_hi,      // preamble hi
            pre_lo,      // preamble lo
            0x00,        // header type: explicit (0x00)
            payload_len, // payload length
            0x01,        // CRC on
            0x00,        // IQ not inverted
        ])
    }

    fn wait_not_busy(&mut self) -> Result<(), RadioError> {
        let deadline = uptime_ms() + 100; // 100 ms max
        while self.busy.is_high() {
            if uptime_ms() > deadline {
                return Err(RadioError::BusyTimeout);
            }
            FreeRtos::delay_ms(1);
        }
        Ok(())
    }

    fn write_cmd(&mut self, data: &[u8]) -> Result<(), RadioError> {
        let mut buf = [0u8; 16];
        let n = data.len().min(16);
        buf[..n].copy_from_slice(&data[..n]);
        self.spi.write(&buf[..n]).map_err(|_| RadioError::Spi)
    }

    fn spi_transfer(&mut self, buf: &mut [u8]) -> Result<(), RadioError> {
        self.spi.transfer_in_place(buf).map_err(|_| RadioError::Spi)
    }

    fn clear_irq(&mut self, mask: u16) -> Result<(), RadioError> {
        self.write_cmd(&[CMD_CLEAR_IRQ_STATUS, ((mask >> 8) & 0xFF) as u8, (mask & 0xFF) as u8])
    }

    fn get_irq(&mut self) -> Result<u16, RadioError> {
        let mut buf = [CMD_GET_IRQ_STATUS, 0x00, 0x00, 0x00];
        self.spi_transfer(&mut buf)?;
        Ok(((buf[2] as u16) << 8) | buf[3] as u16)
    }

    fn get_rx_buffer_status(&mut self) -> Result<(u8, u8), RadioError> {
        let mut buf = [CMD_GET_RX_BUFFER_STATUS, 0x00, 0x00, 0x00];
        self.spi_transfer(&mut buf)?;
        Ok((buf[2], buf[3])) // (payloadLength, rxBufferOffset)
    }

    /// ReadRegister: opcode(1) + addr_hi(1) + addr_lo(1) + status NOP(1) + data(1).
    /// The register value is clocked back during the final NOP byte (buf[4]).
    fn read_register(&mut self, addr: u16) -> Result<u8, RadioError> {
        let mut buf = [CMD_READ_REGISTER, ((addr >> 8) & 0xFF) as u8, (addr & 0xFF) as u8, 0x00, 0x00];
        self.spi_transfer(&mut buf)?;
        Ok(buf[4])
    }

    /// Read the last-received packet RSSI and SNR (SX1262 LoRa mode, DS §13.5.3).
    ///
    /// Call immediately after `try_receive` returns `Ok(Some(_))` or
    /// `Err(CrcError)` — the registers remain valid until the next RX operation.
    ///
    /// Returns `(rssi_raw, snr_raw)`:
    ///   - RSSI dBm  = -(rssi_raw as i32) / 2   (e.g. 140 → −70 dBm)
    ///   - SNR  dB   = (snr_raw  as i32) / 4    (signed; e.g. 28 → 7 dB)
    pub fn get_packet_status(&mut self) -> Result<(u8, i8), RadioError> {
        // GetPacketStatus: opcode(1) + status NOP(1) + rssiPkt(1) + snrPkt(1) + signalRssiPkt(1)
        // DS_SX1261-2 V2.1 §13.5.3, LoRa mode response layout.
        let mut buf = [CMD_GET_PACKET_STATUS, 0x00, 0x00, 0x00, 0x00];
        self.spi_transfer(&mut buf)?;
        // buf[1] = device status (ignored), buf[2] = rssiPkt, buf[3] = snrPkt (signed)
        Ok((buf[2], buf[3] as i8))
    }
}

// ── Radio error type ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioError {
    Spi,
    BusyTimeout,
    TxTimeout,
    CadTimeout,
    CrcError,
}

impl core::fmt::Display for RadioError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for RadioError {}

// ── Uptime helper ─────────────────────────────────────────────────────────────

/// Return FreeRTOS tick uptime in milliseconds.
#[inline]
fn uptime_ms() -> u64 {
    // esp_idf_svc::sys::esp_timer_get_time() returns microseconds since boot.
    // SAFETY: FFI call; documented as safe when ESP-IDF is initialised.
    unsafe { esp_idf_svc::sys::esp_timer_get_time() as u64 / 1000 }
}
