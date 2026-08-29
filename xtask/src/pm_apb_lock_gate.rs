// SPDX-License-Identifier: GPL-3.0-only
//! Host-run structural guard for `meshcadet-power-optimization` Phase 7's
//! `ESP_PM_APB_FREQ_MAX` bracketing invariant (`docs/adr/0014-power-policy.md`
//! D8): every SPI2 transaction the radio driver issues, and the GPS driver's
//! whole UART ACTIVE window, must be bracketed by an
//! [`firmware::pm::ApbFreqMaxLock`] acquire/release pair — an APB frequency
//! change mid-transaction is the failure mode that would breach constraints
//! P1 (GPS UART baud) and P4 (SPI2 clock), both APB-derived.
//!
//! Same "textual precedes"/section-sliced proxy discipline as
//! `xtask::lock_gate`/`xtask::render_asleep_gate` — see either module's doc
//! for the general pattern this mirrors. Three sites are checked:
//!
//! 1. `Radio::write_cmd` — `apb_lock.acquire()` before, `apb_lock.release()`
//!    after, the single `self.spi.write(...)` call.
//! 2. `Radio::spi_transfer` — same shape, around
//!    `self.spi.transfer_in_place(...)`.
//! 3. `GpsDriver` — `apb_lock.acquire()` before the driver's initial
//!    `active: true` construction, `apb_lock.release()` before the
//!    ACTIVE→QUIET transition (`self.active = false;`), and
//!    `apb_lock.acquire()` before the QUIET→ACTIVE transition
//!    (`self.active = true;`).

use std::fs;
use std::path::Path;

pub const RADIO_REL_PATH: &str = "firmware/src/radio.rs";
pub const GPS_REL_PATH: &str = "firmware/src/gps.rs";

const ACQUIRE: &str = "self.apb_lock.acquire();";
const RELEASE: &str = "self.apb_lock.release();";

/// Find exactly one occurrence of `needle` in `haystack`. Returns `Err` on
/// zero or more-than-one hits — ambiguity is a scanner-needs-updating
/// condition, never a guess (same discipline every other xtask scanner
/// uses).
fn find_once(haystack: &str, needle: &str, path: &str) -> Result<usize, String> {
    let mut it = haystack.match_indices(needle);
    match (it.next(), it.next()) {
        (None, _) => Err(format!(
            "{path}: anchor `{needle}` not found — this scanner needs updating"
        )),
        (Some(_), Some(_)) => Err(format!(
            "{path}: anchor `{needle}` occurs more than once — this scanner cannot tell which \
             is the real one"
        )),
        (Some((i, _)), None) => Ok(i),
    }
}

/// Assert `first` occurs, `second` occurs, and `first` precedes `second`, in
/// the MASKED (comment/string-blanked) text of `section` — a comment
/// mentioning either string can never satisfy this.
fn check_precedes(
    section: &str,
    first: &str,
    second: &str,
    label: &str,
    path: &str,
    violations: &mut Vec<String>,
) {
    let masked = crate::tokenize(section).masked;
    let first_idx = masked.find(first);
    let second_idx = masked.find(second);
    match (first_idx, second_idx) {
        (None, _) => violations.push(format!(
            "{path}: {label} — expected `{first}` somewhere before `{second}`, but `{first}` \
             was not found"
        )),
        (_, None) => violations.push(format!(
            "{path}: {label} — expected `{second}` somewhere after `{first}`, but `{second}` \
             was not found"
        )),
        (Some(f), Some(s)) if f >= s => violations.push(format!(
            "{path}: {label} — `{first}` must precede `{second}`, but it does not (found at \
             byte {f} vs {s})"
        )),
        _ => {}
    }
}

/// Check `firmware/src/radio.rs`'s two SPI2 transaction funnel points.
pub fn check_radio_source(src: &str) -> Vec<String> {
    let mut violations = Vec::new();

    let write_cmd_start = "fn write_cmd(&mut self, data: &[u8]) -> Result<(), RadioError> {";
    let spi_transfer_start =
        "fn spi_transfer(&mut self, buf: &mut [u8]) -> Result<(), RadioError> {";
    let spi_transfer_end = "fn clear_irq(&mut self, mask: u16) -> Result<(), RadioError> {";

    let wc_start = match find_once(src, write_cmd_start, RADIO_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    let st_start = match find_once(src, spi_transfer_start, RADIO_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    let st_end = match find_once(src, spi_transfer_end, RADIO_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    if !(wc_start < st_start && st_start < st_end) {
        violations.push(format!(
            "{RADIO_REL_PATH}: expected anchor order write_cmd < spi_transfer < clear_irq (got \
             byte offsets {wc_start}/{st_start}/{st_end}) — this file's function ordering \
             changed; this scanner needs updating"
        ));
        return violations;
    }

    let write_cmd_section = &src[wc_start..st_start];
    let spi_transfer_section = &src[st_start..st_end];

    let write_cmd_risky = "self.spi.write(&buf[..n])";
    let spi_transfer_risky = "self.spi.transfer_in_place(buf)";

    check_precedes(
        write_cmd_section,
        ACQUIRE,
        write_cmd_risky,
        "write_cmd: apb_lock.acquire() must precede the SPI write",
        RADIO_REL_PATH,
        &mut violations,
    );
    check_precedes(
        write_cmd_section,
        write_cmd_risky,
        RELEASE,
        "write_cmd: apb_lock.release() must follow the SPI write",
        RADIO_REL_PATH,
        &mut violations,
    );
    check_precedes(
        spi_transfer_section,
        ACQUIRE,
        spi_transfer_risky,
        "spi_transfer: apb_lock.acquire() must precede the SPI transfer",
        RADIO_REL_PATH,
        &mut violations,
    );
    check_precedes(
        spi_transfer_section,
        spi_transfer_risky,
        RELEASE,
        "spi_transfer: apb_lock.release() must follow the SPI transfer",
        RADIO_REL_PATH,
        &mut violations,
    );

    violations
}

/// Check `firmware/src/gps.rs`'s three ACTIVE/QUIET lock-bracketing sites:
/// construction (starts ACTIVE), the ACTIVE→QUIET close, and the
/// QUIET→ACTIVE reopen.
pub fn check_gps_source(src: &str) -> Vec<String> {
    let mut violations = Vec::new();

    let new_start = "pub fn new(\n        uart: UartDriver<'d>,";
    let poll_start = "pub fn poll(&mut self, now_ms: u64) {";
    let poll_end = "pub fn get_fix_and_age(&self, now_ms: u64) -> Option<(i32, i32, u32)> {";

    let n_start = match find_once(src, new_start, GPS_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    let p_start = match find_once(src, poll_start, GPS_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    let p_end = match find_once(src, poll_end, GPS_REL_PATH) {
        Ok(v) => v,
        Err(e) => {
            violations.push(e);
            return violations;
        }
    };
    if !(n_start < p_start && p_start < p_end) {
        violations.push(format!(
            "{GPS_REL_PATH}: expected anchor order new < poll < get_fix_and_age (got byte \
             offsets {n_start}/{p_start}/{p_end}) — this file's function ordering changed; this \
             scanner needs updating"
        ));
        return violations;
    }

    // `new` spans everything up to `poll` — intervening private helpers
    // (`full_probe_and_persist`, `send_init_commands`, ...) are along for
    // the ride but contain none of the needles below, so this remains a
    // precise check on `new`'s own body.
    let new_section = &src[n_start..p_start];
    let poll_section = &src[p_start..p_end];

    check_precedes(
        new_section,
        // Bare `apb_lock.acquire()`, not `self.apb_lock.acquire()` —
        // `apb_lock` is still a plain local/parameter at this point in
        // `new()`, not yet moved into `self`.
        "apb_lock.acquire();",
        "active: true,",
        "GpsDriver::new: apb_lock.acquire() must precede the initial `active: true` (the \
         driver starts ACTIVE)",
        GPS_REL_PATH,
        &mut violations,
    );
    check_precedes(
        poll_section,
        RELEASE,
        "self.active = false;",
        "GpsDriver::poll: apb_lock.release() must precede the ACTIVE\u{2192}QUIET transition",
        GPS_REL_PATH,
        &mut violations,
    );
    check_precedes(
        poll_section,
        ACQUIRE,
        "self.active = true;",
        "GpsDriver::poll: apb_lock.acquire() must precede the QUIET\u{2192}ACTIVE transition",
        GPS_REL_PATH,
        &mut violations,
    );

    violations
}

/// Read [`RADIO_REL_PATH`]/[`GPS_REL_PATH`] under `repo_root` and return
/// every violation across both.
pub fn check(repo_root: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    match fs::read_to_string(repo_root.join(RADIO_REL_PATH)) {
        Ok(src) => violations.extend(check_radio_source(&src)),
        Err(e) => violations.push(format!("reading {RADIO_REL_PATH}: {e}")),
    }
    match fs::read_to_string(repo_root.join(GPS_REL_PATH)) {
        Ok(src) => violations.extend(check_gps_source(&src)),
        Err(e) => violations.push(format!("reading {GPS_REL_PATH}: {e}")),
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual guard: the shipped `radio.rs`/`gps.rs` bracket every site.
    #[test]
    fn pm_apb_lock_gate_contract_holds() {
        let violations = check(&crate::repo_root_from_manifest_dir());
        assert!(
            violations.is_empty(),
            "ESP_PM_APB_FREQ_MAX bracketing contract violated:\n  - {}",
            violations.join("\n  - ")
        );
    }

    // ── Synthetic radio.rs stand-ins ─────────────────────────────────────────

    fn synthetic_radio(write_cmd_bracketed: bool, spi_transfer_bracketed: bool) -> String {
        let write_cmd_body = if write_cmd_bracketed {
            "self.apb_lock.acquire();\n        let result = self.spi.write(&buf[..n]).map_err(|_| RadioError::Spi);\n        self.apb_lock.release();"
        } else {
            "let result = self.spi.write(&buf[..n]).map_err(|_| RadioError::Spi);"
        };
        let spi_transfer_body = if spi_transfer_bracketed {
            "self.apb_lock.acquire();\n        let result = self.spi.transfer_in_place(buf).map_err(|_| RadioError::Spi);\n        self.apb_lock.release();"
        } else {
            "let result = self.spi.transfer_in_place(buf).map_err(|_| RadioError::Spi);"
        };
        format!(
            "fn write_cmd(&mut self, data: &[u8]) -> Result<(), RadioError> {{\n        {write_cmd_body}\n        result\n    }}\n\n    fn spi_transfer(&mut self, buf: &mut [u8]) -> Result<(), RadioError> {{\n        {spi_transfer_body}\n        result\n    }}\n\n    fn clear_irq(&mut self, mask: u16) -> Result<(), RadioError> {{\n"
        )
    }

    #[test]
    fn synthetic_radio_baseline_bracketed_is_clean() {
        assert_eq!(
            check_radio_source(&synthetic_radio(true, true)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn synthetic_radio_unbracketed_write_cmd_is_caught() {
        let violations = check_radio_source(&synthetic_radio(false, true));
        assert!(
            violations.iter().any(|v| v.contains("write_cmd")),
            "expected a write_cmd violation, got {violations:?}"
        );
        assert!(
            !violations.iter().any(|v| v.contains("spi_transfer:")),
            "spi_transfer is still bracketed and must not be flagged, got {violations:?}"
        );
    }

    #[test]
    fn synthetic_radio_unbracketed_spi_transfer_is_caught() {
        let violations = check_radio_source(&synthetic_radio(true, false));
        assert!(
            violations.iter().any(|v| v.contains("spi_transfer:")),
            "expected a spi_transfer violation, got {violations:?}"
        );
    }

    /// Only an acquire with no matching release (e.g. an early return added
    /// later) is exactly as dangerous as no bracket at all.
    #[test]
    fn synthetic_radio_acquire_without_release_is_caught() {
        let src = synthetic_radio(true, true).replace(
            "self.apb_lock.acquire();\n        let result = self.spi.write(&buf[..n]).map_err(|_| RadioError::Spi);\n        self.apb_lock.release();",
            "self.apb_lock.acquire();\n        let result = self.spi.write(&buf[..n]).map_err(|_| RadioError::Spi);",
        );
        let violations = check_radio_source(&src);
        assert!(
            violations
                .iter()
                .any(|v| v.contains("write_cmd") && v.contains("release")),
            "expected a missing-release violation, got {violations:?}"
        );
    }

    // ── Synthetic gps.rs stand-ins ────────────────────────────────────────────

    fn synthetic_gps(new_gated: bool, close_gated: bool, reopen_gated: bool) -> String {
        let new_body = if new_gated {
            "apb_lock.acquire();\n\n        Self {\n            active: true,\n            apb_lock,\n        }"
        } else {
            "Self {\n            active: true,\n            apb_lock,\n        }"
        };
        let close_body = if close_gated {
            "self.apb_lock.release();\n                self.active = false;"
        } else {
            "self.active = false;"
        };
        let reopen_body = if reopen_gated {
            "self.apb_lock.acquire();\n                self.active = true;"
        } else {
            "self.active = true;"
        };
        format!(
            "pub fn new(\n        uart: UartDriver<'d>,\n    ) -> Self {{\n        {new_body}\n    }}\n\n    pub fn poll(&mut self, now_ms: u64) {{\n        if self.active {{\n            {close_body}\n        }} else {{\n            {reopen_body}\n        }}\n    }}\n\n    pub fn get_fix_and_age(&self, now_ms: u64) -> Option<(i32, i32, u32)> {{\n"
        )
    }

    #[test]
    fn synthetic_gps_baseline_bracketed_is_clean() {
        assert_eq!(
            check_gps_source(&synthetic_gps(true, true, true)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn synthetic_gps_unbracketed_construction_is_caught() {
        let violations = check_gps_source(&synthetic_gps(false, true, true));
        assert!(
            violations.iter().any(|v| v.contains("GpsDriver::new")),
            "expected a construction violation, got {violations:?}"
        );
    }

    #[test]
    fn synthetic_gps_unbracketed_close_is_caught() {
        let violations = check_gps_source(&synthetic_gps(true, false, true));
        assert!(
            violations.iter().any(|v| v.contains("ACTIVE\u{2192}QUIET")),
            "expected a close-transition violation, got {violations:?}"
        );
    }

    #[test]
    fn synthetic_gps_unbracketed_reopen_is_caught() {
        let violations = check_gps_source(&synthetic_gps(true, true, false));
        assert!(
            violations.iter().any(|v| v.contains("QUIET\u{2192}ACTIVE")),
            "expected a reopen-transition violation, got {violations:?}"
        );
    }

    /// A comment merely MENTIONING the lock, with no real code, must not
    /// satisfy this scanner — `tokenize()` blanks comment bodies before the
    /// keyword search runs.
    #[test]
    fn comment_only_mention_does_not_satisfy_the_gate() {
        let src = synthetic_radio(false, true).replace(
            "let result = self.spi.write(&buf[..n]).map_err(|_| RadioError::Spi);",
            "// self.apb_lock.acquire(); is handled elsewhere, trust me\n        let result = self.spi.write(&buf[..n]).map_err(|_| RadioError::Spi);",
        );
        let violations = check_radio_source(&src);
        assert!(
            violations.iter().any(|v| v.contains("write_cmd")),
            "a comment-only mention must not satisfy the gate; got {violations:?}"
        );
    }

    /// Parse gaps fail loud rather than passing silently.
    #[test]
    fn missing_anchor_is_a_violation_not_a_silent_pass() {
        let violations = check_radio_source("fn other() {}");
        assert!(
            violations
                .iter()
                .any(|v| v.contains("anchor") && v.contains("not found")),
            "expected a missing-anchor violation, got {violations:?}"
        );
    }
}
