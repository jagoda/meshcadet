// SPDX-License-Identifier: GPL-3.0-only
//! Inbound DM text-subtype disposition — the pure half of
//! `firmware/src/main.rs::handle_dm`'s subtype gate.
//!
//! # The gap this closes
//!
//! A DM's plaintext carries a `txt_type_attempt` flags byte whose upper 6
//! bits are the **text subtype** ([`protocol::TxtType`]). Until this module,
//! `handle_dm` decoded that byte but never inspected the subtype: every
//! successfully decrypted DM took the plain-text path regardless. Against
//! stock MeshCore v1.18.0 firmware (`BaseChatMesh::onPeerDataRecv`,
//! `BaseChatMesh.cpp:235-300` @ `companion-v1.18.0`) that is wrong for three
//! separate traffic classes:
//!
//! - `TXT_TYPE_CLI_DATA` (1) — a CLI command *or* a CLI reply. Upstream
//!   routes it to `onCommandDataRecv` and notes explicitly: "no ack expected
//!   for CLI_DATA replies".
//! - `TXT_TYPE_CLI_COMMAND` (3) — an explicit CLI command (new in v1.18.0,
//!   `TxtDataHelpers.h`). Upstream routes it to `onCLICommandRecv`, likewise
//!   with **no** ACK.
//! - Any other reserved subtype (4..=63) — upstream's `if`/`else if` chain
//!   matches nothing and the datagram is silently dropped.
//!
//! Treating those as plain text made MeshCadet render remote CLI traffic as
//! an incoming DM, append it to history, run `is_telemetry_request` on it
//! (so a remote `?loc` triggered an automatic location reply), ACK it, and
//! spend an inbound-replay-ring slot plus a possible `last_ts` high-water-
//! mark advance on it ([`crate::inbound_replay`]).
//!
//! # Why the decision is a pure function here
//!
//! `firmware/` is a detached workspace whose `#[cfg(test)]` blocks never
//! execute on the host, so a gate written inline in `handle_dm` could not be
//! proven here. [`classify_inbound_dm`] has no NVS, no UI, no clock and no
//! side effects at all: `handle_dm` calls it once, before the replay gate,
//! history, UI event, telemetry check and ACK, and returns early unless the
//! disposition is [`DmDisposition::Accept`]. Because every ignored
//! disposition returns from that one call site *ahead of* all of those side
//! effects, "ignored" here means "no ACK, no history entry, no UI event, no
//! telemetry reply, no replay-state consumption" — see the tests below for
//! the exhaustive classification that guarantees no subtype other than
//! `TXT_TYPE_PLAIN`/`TXT_TYPE_SIGNED_PLAIN` can reach the accept path.

use protocol::TxtType;

/// What `handle_dm` should do with a decrypted DM, decided purely from its
/// TXT_MSG text subtype.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmDisposition {
    /// `TXT_TYPE_PLAIN` (0) or `TXT_TYPE_SIGNED_PLAIN` (2): the pre-existing
    /// inbound DM path runs unchanged (display, history, `?loc` telemetry
    /// pull, ACK, replay gate).
    Accept,
    /// `TXT_TYPE_CLI_DATA` (1) or `TXT_TYPE_CLI_COMMAND` (3): admin CLI
    /// traffic. MeshCadet has no CLI surface, so it is dropped without any
    /// of the accept path's side effects.
    IgnoreCli {
        /// The exact subtype that was dropped, for the log line.
        subtype: TxtType,
    },
    /// A subtype MeshCore does not define today (4..=63). Ignored exactly
    /// like CLI traffic — and, deliberately, **not** an error or a warning:
    /// this is legitimate forward-compatible traffic from a newer peer, not
    /// a fault.
    IgnoreUnknown {
        /// The exact subtype that was dropped, for the log line.
        subtype: TxtType,
    },
}

impl DmDisposition {
    /// Whether this DM follows the normal accept path.
    pub const fn is_accept(self) -> bool {
        matches!(self, DmDisposition::Accept)
    }

    /// A single human-readable reason for dropping the DM, or `None` when it
    /// is accepted. `handle_dm` logs exactly ONE info-level line from this —
    /// kept here (rather than inline in `firmware/`) so the wording is
    /// covered by this crate's host-executed tests.
    pub const fn ignore_reason(self) -> Option<&'static str> {
        match self {
            DmDisposition::Accept => None,
            // Upstream's own comment for the CLI arms: "no ack expected".
            DmDisposition::IgnoreCli { .. } => Some("CLI traffic not supported, ignored"),
            DmDisposition::IgnoreUnknown { .. } => {
                Some("unrecognized text subtype (forward-compatible), ignored")
            }
        }
    }
}

/// Classify a decoded inbound DM's text subtype.
///
/// Mirrors `BaseChatMesh::onPeerDataRecv`'s flags chain
/// (`BaseChatMesh.cpp:239-300` @ `companion-v1.18.0`): only
/// `TXT_TYPE_PLAIN` and `TXT_TYPE_SIGNED_PLAIN` reach a message callback (and
/// get an ACK); both CLI subtypes reach a CLI callback instead, and anything
/// else is dropped.
pub const fn classify_inbound_dm(txt_type: TxtType) -> DmDisposition {
    match txt_type {
        TxtType::Plain | TxtType::SignedPlain => DmDisposition::Accept,
        TxtType::CliData | TxtType::CliCommand => DmDisposition::IgnoreCli { subtype: txt_type },
        TxtType::Unknown(_) => DmDisposition::IgnoreUnknown { subtype: txt_type },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Accept path: unchanged behaviour ─────────────────────────────────────

    #[test]
    fn plain_text_is_accepted() {
        let disposition = classify_inbound_dm(TxtType::Plain);
        assert_eq!(disposition, DmDisposition::Accept);
        assert!(disposition.is_accept());
        // Accepted DMs take the full existing path — no drop reason to log.
        assert_eq!(disposition.ignore_reason(), None);
    }

    #[test]
    fn signed_plain_is_accepted() {
        let disposition = classify_inbound_dm(TxtType::SignedPlain);
        assert_eq!(disposition, DmDisposition::Accept);
        assert!(disposition.is_accept());
        assert_eq!(disposition.ignore_reason(), None);
    }

    // ── CLI subtypes: dropped, with no accept-path side effects ──────────────

    #[test]
    fn cli_data_is_ignored() {
        let disposition = classify_inbound_dm(TxtType::CliData);
        assert_eq!(
            disposition,
            DmDisposition::IgnoreCli {
                subtype: TxtType::CliData
            }
        );
        // Not accepted => `handle_dm` returns before the replay gate, history,
        // UI event, telemetry-request check and ACK — so none of them happen.
        assert!(!disposition.is_accept());
        assert_eq!(
            disposition.ignore_reason(),
            Some("CLI traffic not supported, ignored")
        );
    }

    #[test]
    fn cli_command_is_ignored() {
        // `TXT_TYPE_CLI_COMMAND` = 3, added upstream in MeshCore v1.18.0.
        let disposition = classify_inbound_dm(TxtType::CliCommand);
        assert_eq!(
            disposition,
            DmDisposition::IgnoreCli {
                subtype: TxtType::CliCommand
            }
        );
        assert!(!disposition.is_accept());
        assert_eq!(
            disposition.ignore_reason(),
            Some("CLI traffic not supported, ignored")
        );
    }

    // ── Unknown subtypes: ignored, never an error ────────────────────────────

    #[test]
    fn unknown_subtype_is_ignored_without_warning() {
        let disposition = classify_inbound_dm(TxtType::Unknown(4));
        assert_eq!(
            disposition,
            DmDisposition::IgnoreUnknown {
                subtype: TxtType::Unknown(4)
            }
        );
        assert!(!disposition.is_accept());
        // A plain info-level `ignore_reason` — never a warn/error: a newer
        // peer's reserved subtype is legitimate forward-compatible traffic.
        assert_eq!(
            disposition.ignore_reason(),
            Some("unrecognized text subtype (forward-compatible), ignored")
        );
        // The dropped subtype round-trips verbatim for the log line.
        assert_eq!(TxtType::Unknown(4).as_byte(), 4);
    }

    #[test]
    fn every_unknown_subtype_value_is_ignored() {
        // Exhaustive over the whole 6-bit subtype space: ONLY 0 (PLAIN) and
        // 2 (SIGNED_PLAIN) may reach the accept path. Everything in between
        // and above — 1, 3 and every reserved value — is dropped, so no
        // future subtype can silently start taking the ACK/history/UI/
        // telemetry/replay side effects again.
        for raw in 0u8..64 {
            let disposition = classify_inbound_dm(TxtType::from_byte(raw));
            let should_accept = raw == 0 || raw == 2;
            assert_eq!(
                disposition.is_accept(),
                should_accept,
                "subtype {} disposition must be is_accept={}",
                raw,
                should_accept
            );
            assert_eq!(
                disposition.ignore_reason().is_some(),
                !should_accept,
                "subtype {} must have a drop reason iff it is dropped",
                raw
            );
        }
    }

    // ── Drop reasons are distinct per class ─────────────────────────────────

    #[test]
    fn drop_reasons_distinguish_cli_from_unknown() {
        let cli = classify_inbound_dm(TxtType::CliCommand).ignore_reason();
        let unknown = classify_inbound_dm(TxtType::Unknown(63)).ignore_reason();
        assert!(cli.is_some() && unknown.is_some());
        assert_ne!(cli, unknown, "CLI and unknown drops log differently");
    }
}
