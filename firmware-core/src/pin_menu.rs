// SPDX-License-Identifier: GPL-3.0-only
//! PIN-gated on-device admin menu — runtime toggle logic.
//!
//! This module is pure Rust with NO ESP-IDF dependencies so its tests compile
//! on a stable host toolchain (`cargo test` from the workspace root).
//!
//! # Responsibilities
//!
//! - `verify_pin()`: constant-time PIN check.
//! - `apply_menu_action()`: apply a `MenuAction` to `RuntimeSettings`.
//! - Callers (e.g. an on-device OLED menu loop) are responsible for:
//!   - Soliciting the PIN from the user (button / touchpad input).
//!   - Persisting `RuntimeSettings` to NVS after `apply_menu_action` returns.
//!   - Enforcing lock flags before presenting the menu.
//!
//! # Security notes
//!
//! `verify_pin` uses a constant-time byte comparison that does not short-circuit
//! on mismatch.  This avoids timing side-channels on devices without hardware
//! constant-time facilities.  A zero-length stored PIN means "no PIN set" and
//! always returns `false` regardless of the entered PIN.

/// Maximum PIN length in bytes (matches `provisioning::MAX_PIN_LEN`).
pub const MAX_PIN_LEN: usize = 16;

/// Maximum number of contacts tracked for per-contact telemetry toggle.
pub const MAX_CONTACTS: usize = 16;

/// Screen-sleep (backlight-off) inactivity timeout, in whole seconds.
///
/// `0` is the sentinel for "never sleep" (a deliberate design decision, 2026-07-03 —
/// chosen over a separate disable toggle). Valid range is `0..=SCREEN_SLEEP_MAX_S`.
pub const SCREEN_SLEEP_DEFAULT_S: u8 = 30;
/// Upper bound of the screen-sleep timeout range (inclusive).
pub const SCREEN_SLEEP_MAX_S: u8 = 120;

// ── RuntimeSettings ───────────────────────────────────────────────────────────

/// Mutable runtime settings that the on-device admin menu can toggle.
///
/// These are separate from the provisioning config (contacts, channels, radio)
/// and can be changed without a laptop after a PIN is verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSettings {
    /// Whether visual notifications are active (LED/screen).
    pub notif_visual: bool,
    /// Whether audible notifications are active (buzzer).
    pub notif_audible: bool,
    /// Per-contact telemetry enable flags.  Index matches contact slot.
    pub contact_telemetry: [bool; MAX_CONTACTS],
    /// Number of contact slots in use (0..=MAX_CONTACTS).
    pub contact_count: u8,
    /// Feature-lock flags (see `protocol::provisioning::LOCK_*`).
    pub lock_flags: u8,
    /// Screen-sleep (backlight-off) inactivity timeout, in seconds.
    /// `0..=SCREEN_SLEEP_MAX_S`; `0` means "never sleep". Default
    /// `SCREEN_SLEEP_DEFAULT_S` (30s).
    pub screen_sleep_timeout_s: u8,
}

impl RuntimeSettings {
    /// Create a default `RuntimeSettings` with notifications enabled, no locks,
    /// and all contact telemetry disabled.
    pub fn default_enabled() -> Self {
        Self {
            notif_visual: true,
            notif_audible: true,
            contact_telemetry: [false; MAX_CONTACTS],
            contact_count: 0,
            lock_flags: 0,
            screen_sleep_timeout_s: SCREEN_SLEEP_DEFAULT_S,
        }
    }
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self::default_enabled()
    }
}

// ── MenuAction ────────────────────────────────────────────────────────────────

/// A single action the admin menu can apply to `RuntimeSettings`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// Enable or disable visual notifications.
    SetNotifVisual(bool),
    /// Enable or disable audible notifications.
    SetNotifAudible(bool),
    /// Enable or disable telemetry forwarding for a contact slot.
    ///
    /// `apply_menu_action` below has a full, test-covered handler for this,
    /// but no on-device screen constructs it yet — `ui::mod::navigate_to_admin_menu`
    /// only wires up notif-visual/audible and screen-sleep-timeout today.
    /// Kept for the per-contact telemetry toggle screen this is designed for.
    #[allow(dead_code)]
    SetContactTelemetry {
        /// Contact slot index (0..MAX_CONTACTS).
        contact_index: usize,
        /// Whether telemetry is enabled for this contact.
        enabled: bool,
    },
    /// Overwrite all feature-lock flags at once.
    ///
    /// Same status as `SetContactTelemetry` above: handled, tested, not yet
    /// reachable from any on-device menu row.
    #[allow(dead_code)]
    SetLockFlags(u8),
    /// Set the screen-sleep inactivity timeout, in seconds. Clamped to
    /// `0..=SCREEN_SLEEP_MAX_S` by `apply_menu_action` — `0` means never sleep.
    SetScreenSleepTimeout(u8),
}

// ── verify_pin ────────────────────────────────────────────────────────────────

/// Verify a PIN entered by the user against the stored PIN.
///
/// Returns `false` immediately if `stored_pin_len == 0` (no PIN configured).
///
/// Uses a constant-time comparison that does not short-circuit on mismatch.
pub fn verify_pin(entered: &[u8], stored_pin: &[u8; MAX_PIN_LEN], stored_pin_len: u8) -> bool {
    let slen = stored_pin_len as usize;
    // A zero-length stored PIN means "no PIN set" — always deny.
    if slen == 0 {
        return false;
    }
    // Length mismatch: set a flag but still do the full comparison to avoid
    // leaking length via timing.
    let mut mismatch: u8 = if entered.len() != slen { 1 } else { 0 };
    // Compare up to MAX_PIN_LEN bytes; pad entered with 0x00 if shorter.
    for i in 0..MAX_PIN_LEN {
        let a = if i < entered.len() { entered[i] } else { 0x00 };
        let b = stored_pin[i];
        mismatch |= a ^ b;
    }
    // Also OR in any bytes in `entered` beyond slen (catches longer input).
    for &b in entered.iter().skip(slen) {
        mismatch |= b;
    }
    mismatch == 0
}

// ── apply_menu_action ─────────────────────────────────────────────────────────

/// Apply a `MenuAction` to `RuntimeSettings`.
///
/// The caller is responsible for:
/// 1. Verifying the PIN before calling this.
/// 2. Checking lock flags to restrict available actions.
/// 3. Persisting the modified `RuntimeSettings` to NVS after returning.
///
/// Invalid contact indices are silently ignored.
pub fn apply_menu_action(action: &MenuAction, settings: &mut RuntimeSettings) {
    match action {
        MenuAction::SetNotifVisual(v) => {
            settings.notif_visual = *v;
        }
        MenuAction::SetNotifAudible(v) => {
            settings.notif_audible = *v;
        }
        MenuAction::SetContactTelemetry {
            contact_index,
            enabled,
        } => {
            if *contact_index < MAX_CONTACTS {
                settings.contact_telemetry[*contact_index] = *enabled;
            }
        }
        MenuAction::SetLockFlags(flags) => {
            settings.lock_flags = *flags;
        }
        MenuAction::SetScreenSleepTimeout(secs) => {
            settings.screen_sleep_timeout_s = (*secs).min(SCREEN_SLEEP_MAX_S);
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pin(bytes: &[u8]) -> ([u8; MAX_PIN_LEN], u8) {
        let len = bytes.len().min(MAX_PIN_LEN);
        let mut buf = [0u8; MAX_PIN_LEN];
        buf[..len].copy_from_slice(&bytes[..len]);
        (buf, len as u8)
    }

    // ── verify_pin ────────────────────────────────────────────────────────

    /// Acceptance: correct PIN is accepted.
    #[test]
    fn pin_correct_accepted() {
        let (stored, slen) = make_pin(b"1234");
        assert!(verify_pin(b"1234", &stored, slen));
    }

    /// Acceptance: wrong PIN is rejected.
    #[test]
    fn pin_wrong_rejected() {
        let (stored, slen) = make_pin(b"1234");
        assert!(!verify_pin(b"5678", &stored, slen));
    }

    #[test]
    fn pin_empty_stored_always_rejected() {
        let stored = [0u8; MAX_PIN_LEN];
        // stored_pin_len = 0 means no PIN configured.
        assert!(!verify_pin(b"", &stored, 0));
        assert!(!verify_pin(b"1234", &stored, 0));
    }

    #[test]
    fn pin_longer_entry_rejected() {
        let (stored, slen) = make_pin(b"1234");
        assert!(!verify_pin(b"12345", &stored, slen));
    }

    #[test]
    fn pin_shorter_entry_rejected() {
        let (stored, slen) = make_pin(b"1234");
        assert!(!verify_pin(b"123", &stored, slen));
    }

    #[test]
    fn pin_empty_entry_rejected_when_pin_set() {
        let (stored, slen) = make_pin(b"1234");
        assert!(!verify_pin(b"", &stored, slen));
    }

    #[test]
    fn pin_max_length_accepted() {
        let pin = [0xAB_u8; MAX_PIN_LEN];
        let (stored, slen) = make_pin(&pin);
        assert!(verify_pin(&pin, &stored, slen));
    }

    #[test]
    fn pin_single_byte_mismatch_rejected() {
        let (stored, slen) = make_pin(b"correct-pin");
        let mut wrong = b"correct-pin".to_vec();
        wrong[5] = b'X';
        assert!(!verify_pin(&wrong, &stored, slen));
    }

    // ── apply_menu_action ─────────────────────────────────────────────────

    #[test]
    fn set_notif_visual_off() {
        let mut s = RuntimeSettings::default();
        assert!(s.notif_visual);
        apply_menu_action(&MenuAction::SetNotifVisual(false), &mut s);
        assert!(!s.notif_visual);
    }

    #[test]
    fn set_notif_visual_on() {
        let mut s = RuntimeSettings {
            notif_visual: false,
            ..Default::default()
        };
        apply_menu_action(&MenuAction::SetNotifVisual(true), &mut s);
        assert!(s.notif_visual);
    }

    #[test]
    fn set_notif_audible_off() {
        let mut s = RuntimeSettings::default();
        apply_menu_action(&MenuAction::SetNotifAudible(false), &mut s);
        assert!(!s.notif_audible);
    }

    #[test]
    fn set_contact_telemetry_enable() {
        let mut s = RuntimeSettings::default();
        apply_menu_action(
            &MenuAction::SetContactTelemetry {
                contact_index: 3,
                enabled: true,
            },
            &mut s,
        );
        assert!(s.contact_telemetry[3]);
        // Other slots unaffected.
        assert!(!s.contact_telemetry[0]);
        assert!(!s.contact_telemetry[15]);
    }

    #[test]
    fn set_contact_telemetry_disable() {
        let mut s = RuntimeSettings::default();
        s.contact_telemetry[7] = true;
        apply_menu_action(
            &MenuAction::SetContactTelemetry {
                contact_index: 7,
                enabled: false,
            },
            &mut s,
        );
        assert!(!s.contact_telemetry[7]);
    }

    #[test]
    fn set_contact_telemetry_out_of_bounds_ignored() {
        let mut s = RuntimeSettings::default();
        // Must not panic; index MAX_CONTACTS is out of range.
        apply_menu_action(
            &MenuAction::SetContactTelemetry {
                contact_index: MAX_CONTACTS,
                enabled: true,
            },
            &mut s,
        );
        // All slots still false.
        for &v in &s.contact_telemetry {
            assert!(!v);
        }
    }

    #[test]
    fn set_lock_flags() {
        let mut s = RuntimeSettings::default();
        assert_eq!(s.lock_flags, 0);
        apply_menu_action(&MenuAction::SetLockFlags(0x03), &mut s);
        assert_eq!(s.lock_flags, 0x03);
    }

    #[test]
    fn set_lock_flags_clear() {
        let mut s = RuntimeSettings {
            lock_flags: 0xFF,
            ..Default::default()
        };
        apply_menu_action(&MenuAction::SetLockFlags(0x00), &mut s);
        assert_eq!(s.lock_flags, 0x00);
    }

    // ── screen-sleep timeout ──────────────────────────────────────────────

    /// Acceptance: default is 30s.
    #[test]
    fn screen_sleep_timeout_defaults_to_30s() {
        let s = RuntimeSettings::default();
        assert_eq!(s.screen_sleep_timeout_s, 30);
        assert_eq!(s.screen_sleep_timeout_s, SCREEN_SLEEP_DEFAULT_S);
    }

    #[test]
    fn set_screen_sleep_timeout_within_range() {
        let mut s = RuntimeSettings::default();
        apply_menu_action(&MenuAction::SetScreenSleepTimeout(45), &mut s);
        assert_eq!(s.screen_sleep_timeout_s, 45);
    }

    /// Acceptance: 0 is the "never sleep" sentinel and must be settable.
    #[test]
    fn set_screen_sleep_timeout_zero_is_never_sleep() {
        let mut s = RuntimeSettings::default();
        apply_menu_action(&MenuAction::SetScreenSleepTimeout(0), &mut s);
        assert_eq!(s.screen_sleep_timeout_s, 0);
    }

    /// Acceptance: the upper bound (120s) is settable.
    #[test]
    fn set_screen_sleep_timeout_max_bound() {
        let mut s = RuntimeSettings::default();
        apply_menu_action(&MenuAction::SetScreenSleepTimeout(120), &mut s);
        assert_eq!(s.screen_sleep_timeout_s, 120);
    }

    /// A value above the 0-120 range is clamped, not silently accepted —
    /// `apply_menu_action` is the single point of truth for the invariant so
    /// no caller (UI stepper, future protocol path) can push an out-of-range
    /// timeout into NVS.
    #[test]
    fn set_screen_sleep_timeout_above_max_clamped() {
        let mut s = RuntimeSettings::default();
        apply_menu_action(&MenuAction::SetScreenSleepTimeout(255), &mut s);
        assert_eq!(s.screen_sleep_timeout_s, SCREEN_SLEEP_MAX_S);
    }

    /// Acceptance: wrong PIN is rejected, correct PIN is accepted,
    /// menu action is applied after correct PIN.
    #[test]
    fn pin_menu_acceptance_gate() {
        let (stored, slen) = make_pin(b"s3cr3t");

        // Wrong PIN denied.
        assert!(!verify_pin(b"wrong", &stored, slen));

        // Correct PIN accepted.
        assert!(verify_pin(b"s3cr3t", &stored, slen));

        // Apply a menu action after verification.
        let mut settings = RuntimeSettings::default();
        apply_menu_action(&MenuAction::SetNotifAudible(false), &mut settings);
        assert!(
            !settings.notif_audible,
            "action must apply after PIN accepted"
        );
    }

    #[test]
    fn multiple_actions_accumulate() {
        let mut s = RuntimeSettings::default();
        apply_menu_action(&MenuAction::SetNotifVisual(false), &mut s);
        apply_menu_action(&MenuAction::SetNotifAudible(false), &mut s);
        apply_menu_action(
            &MenuAction::SetContactTelemetry {
                contact_index: 0,
                enabled: true,
            },
            &mut s,
        );
        apply_menu_action(&MenuAction::SetLockFlags(0x01), &mut s);

        assert!(!s.notif_visual);
        assert!(!s.notif_audible);
        assert!(s.contact_telemetry[0]);
        assert_eq!(s.lock_flags, 0x01);
    }
}
