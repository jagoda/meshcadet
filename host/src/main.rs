// SPDX-License-Identifier: GPL-3.0-only
//! MeshCadet admin CLI (`meshcadet`).
//!
//! Connects to a MeshCadet device over USB-serial and provisions / inspects it.
//! Physical USB possession is the authentication factor (ADR-0001 §4).
//!
//! # Usage
//!
//! ```text
//! meshcadet --port /dev/ttyUSB0 status
//! meshcadet --port /dev/ttyUSB0 identity
//! meshcadet --port /dev/ttyUSB0 identity --set-name "Alex's MeshCadet"
//! meshcadet --port /dev/ttyUSB0 add-contact --pubkey <HEX64> --name "Alice" --telemetry
//! meshcadet --port /dev/ttyUSB0 add-channel --secret <HEX64> --name "family" --primary
//! meshcadet --port /dev/ttyUSB0 set-notif-defaults --visual --audible
//! meshcadet --port /dev/ttyUSB0 set-pin --pin 1234
//! meshcadet --port /dev/ttyUSB0 commit
//! meshcadet --port /dev/ttyUSB0 reset-pin --pin 5678
//! meshcadet --port /dev/ttyUSB0 clear-history
//! ```

use clap::{ArgAction, Parser, Subcommand};
use host::session::Session;
use host::transport::SerialTransport;

// ── CLI shape ─────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "meshcadet",
    about = "MeshCadet admin CLI — USB-serial provisioning, identity readout, and PIN reset",
    long_about = None,
    version
)]
struct Cli {
    /// USB-serial port path (e.g. /dev/ttyUSB0, /dev/ttyACM0, COM3).
    #[arg(short, long)]
    port: String,

    /// Serial baud rate.
    #[arg(short, long, default_value = "115200")]
    baud: u32,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Query device provisioning status and identity.
    Status,

    /// Read device identity and print a scannable MeshCore contact QR code.
    ///
    /// The QR encodes a `meshcore://contact/add?...` URI that a real MeshCore
    /// companion app accepts to add this node as a chat contact.
    Identity {
        /// Display name to embed in the contact URI for THIS invocation only
        /// (does not persist). Defaults to the device's persisted name (see
        /// `--set-name`), or `MeshCadet-<hash>` if none is set.
        #[arg(long)]
        name: Option<String>,

        /// Persist a new device display name to the device (NVS-backed
        /// identity store) before reading identity back. Survives a reboot.
        /// Max 32 bytes UTF-8. Pass an empty string ("") to clear the stored
        /// name.
        #[arg(long = "set-name")]
        set_name: Option<String>,
    },

    /// List the device's configured contacts (name + pubkey + telemetry flag).
    ///
    /// Enumerates the in-progress provisioning staging set; run before `commit`
    /// alongside `add-contact` / `del-contact` to verify the configured list.
    ListContacts,

    /// List the device's configured channels (name + channel hash + key length).
    ListChannels,

    /// Add a contact to the device.
    AddContact {
        /// Contact Ed25519 public key — 64 hex characters (32 bytes).
        #[arg(long)]
        pubkey: String,

        /// Display name shown on screen (optional; defaults to routing hash if absent).
        #[arg(long)]
        name: Option<String>,

        /// Allow this contact to pull telemetry (GPS location) from the device.
        #[arg(long, action = ArgAction::SetTrue)]
        telemetry: bool,
    },

    /// Remove a contact from the device.
    DelContact {
        /// Contact Ed25519 public key — 64 hex characters.
        #[arg(long)]
        pubkey: String,
    },

    /// Add (or replace) a channel on the device.
    AddChannel {
        /// Channel secret — 32 hex chars (128-bit / 16-byte) or 64 hex chars (256-bit / 32-byte).
        #[arg(long)]
        secret: String,

        /// Channel name shown on screen.
        #[arg(long)]
        name: Option<String>,

        /// Set this channel as the primary (default outgoing) channel.
        #[arg(long, action = ArgAction::SetTrue)]
        primary: bool,
    },

    /// Remove a channel from the device.
    DelChannel {
        /// Channel secret — 32 hex chars (128-bit) or 64 hex chars (256-bit);
        /// must match exactly what was passed to add-channel.
        #[arg(long)]
        secret: String,
    },

    /// Set notification defaults (what happens on message receipt before the user changes them).
    SetNotifDefaults {
        /// Enable visual notifications (screen flash / LED).
        #[arg(long, action = ArgAction::SetTrue)]
        visual: bool,

        /// Enable audible notifications (buzzer / speaker).
        #[arg(long, action = ArgAction::SetTrue)]
        audible: bool,
    },

    /// Set the admin PIN (used to access the on-device admin menu).
    SetPin {
        /// PIN string (UTF-8, max 16 bytes).
        #[arg(long)]
        pin: String,
    },

    /// Commit provisioning: persist config to flash.
    ///
    /// Run this after all contacts, channels, and settings have been provisioned.
    /// On a first-boot device the firmware reboots into the mesh after committing;
    /// on an already-provisioned device it re-persists live config without rebooting.
    Commit,

    /// Reset the admin PIN (physical USB possession is the auth factor).
    ///
    /// Equivalent to set-pin but clearly named for the recovery flow.
    ResetPin {
        /// New PIN string (UTF-8, max 16 bytes).
        #[arg(long)]
        pin: String,
    },

    /// Export conversation history from the device (oldest-first).
    ///
    /// Prints a header row followed by one fixed-width, left-aligned entry
    /// per line: `idx  timestamp  type  from  text`.
    ExportHistory,

    /// Clear ALL persisted message history on the device.
    ///
    /// Erases every sent and received message across every conversation —
    /// both DM contacts and channels — from the device's flash-backed history
    /// store. The erase takes effect on flash immediately; the device's live
    /// on-screen conversation views are only refreshed by a reboot (they hold
    /// an in-memory copy hydrated at boot — see
    /// `docs/adr/0002-provisioning-wire-format.md`'s `CLEAR_HISTORY`
    /// amendment). Reboot the device afterward to see the cleared state.
    ClearHistory,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let transport = SerialTransport::open(&cli.port, cli.baud)?;
    let mut session = Session::new(transport);

    match cli.cmd {
        Cmd::Status => {
            let s = session.query_status()?;
            println!("provisioned : {}", s.provisioned);
            println!("pubkey      : {}", hex::encode(s.pubkey));
            println!("pub_hash    : 0x{:02X}", s.pubkey[0]);
            println!("contacts    : {}", s.contact_count);
            println!("channels    : {}", s.channel_count);
            // Mirrors the on-device admin-menu GPS status view: fix state,
            // coordinates + age, and clock-sync state + age. Status/display
            // only — there is no GPS control surface on either side.
            println!("gps fix     : {}", format_gps_fix(&s));
            println!("gps coords  : {}", format_gps_coords(&s));
            println!("gps clock   : {}", format_gps_clock(&s));
            // Mirrors the on-device admin-menu battery display and the radio
            // telemetry RESPONSE — all three derive from the same
            // `battery::BatteryStatus` reading (see firmware `battery` module docs).
            println!("battery     : {}", format_battery(&s));
            // Diagnostic-only raw ADC millivolts (2026-07-05 ADC-calibration
            // investigation) — the live, unfrozen voltage, for comparing
            // against a multimeter / the charger's LED across charge states.
            // NOTE: on this board USB carries both the CLI UART and charge
            // power, so this line is ALWAYS taken with the charger's
            // contaminated rail on the pin while a cable is attached to read
            // it — it can never show a clean battery voltage as built.
            println!("battery raw : {}", format_battery_raw_mv(&s));
            // Held last-unplugged raw millivolts (2026-07-05
            // full-anchor-and-held-raw-exposure work) — frozen the instant
            // charging starts, so THIS is the contamination-free reading:
            // unplug, wait a moment, replug just to read this value, and it
            // reports the true pre-charge pack voltage rather than the live
            // USB rail.
            println!("battery held raw : {}", format_battery_held_raw_mv(&s));
        }

        Cmd::Identity { name, set_name } => {
            // SET (optional): persist a new device display name to the
            // identity store before reading it back. Additive to the
            // pre-existing read-only behavior below — with `set_name` absent
            // (the common case), nothing here runs and the read/QR output is
            // unchanged from before this command existed.
            if let Some(new_name) = &set_name {
                if new_name.len() > protocol::provisioning::MAX_NAME_LEN {
                    anyhow::bail!(
                        "device name must be at most {} bytes (UTF-8); got {} bytes",
                        protocol::provisioning::MAX_NAME_LEN,
                        new_name.len()
                    );
                }
                session.set_device_name(new_name.as_bytes())?;
                if new_name.is_empty() {
                    println!("device name cleared");
                } else {
                    println!("device name set: \"{}\"", new_name);
                }
            }

            let s = session.query_status()?;
            let pubkey_hex = hex::encode(s.pubkey);
            println!("pubkey   : {}", pubkey_hex);
            println!(
                "pub_hash : 0x{:02X}  (routing hash = pubkey[0])",
                s.pubkey[0]
            );
            println!(
                "name     : {}",
                session.last_device_name().unwrap_or("(unnamed)")
            );

            // Build the MeshCore companion contact-add URI.
            // Format (meshcore-dev/MeshCore docs/faq.md §7.5, companion-v1.16.0):
            //   meshcore://contact/add?name=<name>&public_key=<hex>&type=<type>
            //   type: chat=1, repeater=2, room=3, sensor=4 — MeshCadet is a chat node.
            //
            // `--name` overrides the URI label for this invocation only; absent
            // that, the persisted device name (just set above, or read from
            // NVS) is used; absent that too, fall back to the pub_hash label —
            // this is the pre-existing default, unchanged.
            let node_name = name
                .or_else(|| session.last_device_name().map(str::to_string))
                .unwrap_or_else(|| format!("MeshCadet-{:02X}", s.pubkey[0]));
            let uri = format!(
                "meshcore://contact/add?name={}&public_key={}&type=1",
                url_encode(&node_name),
                pubkey_hex,
            );
            println!("\nMeshCore contact URI (chat node):\n{}\n", uri);

            // Render the URI as a terminal QR code.  Dense1x2 packs two QR rows
            // per text line (Unicode half-blocks) so the code stays compact and
            // square in a normal terminal; the quiet zone is required for
            // reliable scanning.
            match qrcode::QrCode::new(uri.as_bytes()) {
                Ok(code) => {
                    let rendered = code
                        .render::<qrcode::render::unicode::Dense1x2>()
                        .quiet_zone(true)
                        .build();
                    println!("{}", rendered);
                    println!("Scan with a MeshCore companion app to add this node as a contact.");
                }
                Err(e) => {
                    eprintln!(
                        "warning: could not render QR code ({}); use the URI above.",
                        e
                    );
                }
            }
        }

        Cmd::ListContacts => {
            let contacts = session.list_contacts()?;
            if contacts.is_empty() {
                println!("no contacts configured");
            } else {
                println!("idx\tpubkey                                                           \ttelemetry\tname");
                for c in &contacts {
                    let name = std::str::from_utf8(&c.display_name[..c.display_name_len as usize])
                        .unwrap_or("<invalid utf-8>");
                    println!(
                        "{}\t{}\t{}\t{}",
                        c.index,
                        hex::encode(c.pubkey),
                        c.telemetry_enable,
                        name,
                    );
                }
                println!("{} contact(s)", contacts.len());
            }
        }

        Cmd::ListChannels => {
            let channels = session.list_channels()?;
            if channels.is_empty() {
                println!("no channels configured");
            } else {
                println!("idx\thash\tbits\tprimary\tname");
                for ch in &channels {
                    let name = std::str::from_utf8(&ch.name[..ch.name_len as usize])
                        .unwrap_or("<invalid utf-8>");
                    println!(
                        "{}\t0x{:02X}\t{}\t{}\t{}",
                        ch.index,
                        ch.channel_hash,
                        ch.key_len as u32 * 8,
                        ch.primary,
                        name,
                    );
                }
                println!("{} channel(s)", channels.len());
            }
        }

        Cmd::AddContact {
            pubkey,
            name,
            telemetry,
        } => {
            let pk = parse_32bytes_hex(&pubkey, "pubkey")?;
            let name_bytes = name.as_deref().unwrap_or("").as_bytes().to_vec();
            session.add_contact(&pk, telemetry, &name_bytes)?;
            println!(
                "contact added: {} (telemetry={}{})",
                hex_short(&pk),
                telemetry,
                name.map(|n| format!(", name=\"{}\"", n))
                    .unwrap_or_default()
            );
            // The on-air dispatcher's allowlist + telemetry gate is a boot-time
            // snapshot of the provisioned config (see firmware/src/main.rs). A
            // runtime edit persists to flash and shows up in `list-contacts`
            // immediately, but does NOT change the running radio path until the
            // device reboots. Make that explicit so an enabled-telemetry contact
            // is not silently dropped on air despite list-contacts showing it.
            println!(
                "  note: reboot the device to apply this to the live mesh (allowlist + telemetry gate are loaded at boot)."
            );
        }

        Cmd::DelContact { pubkey } => {
            let pk = parse_32bytes_hex(&pubkey, "pubkey")?;
            session.del_contact(&pk)?;
            println!("contact removed: {}", hex_short(&pk));
        }

        Cmd::AddChannel {
            secret,
            name,
            primary,
        } => {
            let (sec, key_len) = parse_channel_secret_hex(&secret)?;
            let name_bytes = name.as_deref().unwrap_or("").as_bytes().to_vec();
            session.add_channel(&sec, key_len, primary, &name_bytes)?;
            println!(
                "channel added: {} ({}bit, primary={}{})",
                hex_short(&sec),
                key_len as u32 * 8,
                primary,
                name.map(|n| format!(", name=\"{}\"", n))
                    .unwrap_or_default()
            );
        }

        Cmd::DelChannel { secret } => {
            let (sec, _key_len) = parse_channel_secret_hex(&secret)?;
            session.del_channel(&sec)?;
            println!("channel removed: {}", hex_short(&sec));
        }

        Cmd::SetNotifDefaults { visual, audible } => {
            session.set_notif_defaults(visual, audible)?;
            println!(
                "notification defaults set: visual={}, audible={}",
                visual, audible
            );
        }

        Cmd::SetPin { pin } => {
            session.set_pin(pin.as_bytes())?;
            println!("PIN set successfully");
        }

        Cmd::Commit => {
            session.commit()?;
            println!("provisioning committed — config persisted to flash");
        }

        Cmd::ResetPin { pin } => {
            session.set_pin(pin.as_bytes())?;
            println!("PIN reset successfully (physical possession authenticated)");
        }

        Cmd::ExportHistory => {
            let entries = session.export_history()?;
            if entries.is_empty() {
                println!("no history entries");
            } else {
                let iw = host::history_format::idx_width(entries.len());
                println!("{}", host::history_format::history_header(iw));
                for (i, (e, is_ours)) in entries.iter().enumerate() {
                    println!(
                        "{}",
                        host::history_format::format_history_line(i, e, *is_ours, iw)
                    );
                }
            }
        }

        Cmd::ClearHistory => {
            session.clear_history()?;
            println!("history cleared — all conversations (DMs and channels) wiped on flash");
            println!("  note: reboot the device to refresh the on-screen conversation views.");
        }
    }

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Decode a 64-char hex string into a 32-byte array.
///
/// Used for Ed25519 public keys (contacts), which are always exactly 32 bytes.
/// For channel secrets (which may be 16 or 32 bytes), use
/// [`parse_channel_secret_hex`] instead.
fn parse_32bytes_hex(s: &str, label: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = hex::decode(s).map_err(|e| anyhow::anyhow!("invalid {} hex: {}", label, e))?;
    if bytes.len() != 32 {
        anyhow::bail!(
            "{} must be exactly 32 bytes (64 hex chars); got {} bytes",
            label,
            bytes.len()
        );
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// Parse a channel secret from hex.  Returns `(secret_bytes: [u8; 32], key_len: u8)`.
///
/// Accepts:
/// - **32 hex chars (16 bytes, 128-bit)**: bytes `[0..16]` = secret; bytes
///   `[16..32]` = zero-padded.  Returns `key_len = 16`.
/// - **64 hex chars (32 bytes, 256-bit)**: all bytes significant.  Returns
///   `key_len = 32`.
///
/// The `key_len` is forwarded to the device in the `ADD_CHANNEL` frame so the
/// firmware can compute the correct 1-byte channel hash:
/// - 128-bit: `SHA-256(secret[0..16])[0]`
/// - 256-bit: `SHA-256(secret)[0]`
fn parse_channel_secret_hex(s: &str) -> anyhow::Result<([u8; 32], u8)> {
    let bytes = hex::decode(s).map_err(|e| anyhow::anyhow!("invalid secret hex: {}", e))?;
    match bytes.len() {
        16 => {
            let mut arr = [0u8; 32];
            arr[..16].copy_from_slice(&bytes);
            Ok((arr, 16))
        }
        32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Ok((arr, 32))
        }
        n => anyhow::bail!(
            "secret must be 16 bytes (32 hex chars, 128-bit) or 32 bytes (64 hex chars, 256-bit); got {} bytes ({})",
            n,
            s.len()
        ),
    }
}

/// First 4 bytes of a 32-byte value as `aabbccdd…` shorthand for display.
fn hex_short(b: &[u8; 32]) -> String {
    format!("{}…", hex::encode(&b[..4]))
}

/// Percent-encode a string for use as a URI query-component value (RFC 3986).
///
/// Leaves the "unreserved" set (`A-Z a-z 0-9 - _ . ~`) intact and percent-encodes
/// every other byte (spaces, `&`, `=`, `#`, UTF-8 multibyte, …) so a contact
/// display name with arbitrary characters round-trips through the MeshCore
/// companion QR scanner without breaking the URI grammar.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{:02X}", b));
            }
        }
    }
    out
}

// ── GPS status formatting (host `status` command) ───────────────────────────
//
// Mirrors the on-device admin-menu GPS status view (fix state, coordinates +
// age, time-sync state + age) — see firmware `ui::screens::gps_status`'s
// `format_*` helpers for the on-device analogues. Status/display only.

/// Format the `gps fix` line: `"yes"` / `"no"`.
fn format_gps_fix(s: &protocol::provisioning::RspStatusPayload) -> &'static str {
    if s.gps_has_fix {
        "yes"
    } else {
        "no"
    }
}

/// Format the `gps coords` line: `"<lat>, <lon> (age <n>s)"`, or an em-dash
/// placeholder when the device has never obtained a fix.
fn format_gps_coords(s: &protocol::provisioning::RspStatusPayload) -> String {
    if !s.gps_has_fix {
        return "\u{2014}".to_string();
    }
    let lat_deg = s.gps_lat_e7 as f64 / 10_000_000.0;
    let lon_deg = s.gps_lon_e7 as f64 / 10_000_000.0;
    format!(
        "{:.6}, {:.6} (age {}s)",
        lat_deg, lon_deg, s.gps_fix_age_secs
    )
}

/// Format the `gps clock` line: `"synced (age <n>s)"` or `"not synced"`.
fn format_gps_clock(s: &protocol::provisioning::RspStatusPayload) -> String {
    if s.gps_clock_synced {
        format!("synced (age {}s)", s.gps_clock_sync_age_secs)
    } else {
        "not synced".to_string()
    }
}

/// Format the `battery` line: `"<n>% (charging)"` or `"<n>%"`.
///
/// Mirrors the on-device admin-menu battery row and the radio telemetry
/// RESPONSE's battery entries — all three read the same
/// `battery::BatteryStatus` (percent + charging), just formatted for their
/// own surface. See firmware's `battery` module docs for how the reading is
/// derived (ADC voltage divider; charging inferred from a voltage-rise trend,
/// not read from a dedicated hardware signal).
fn format_battery(s: &protocol::provisioning::RspStatusPayload) -> String {
    if s.battery_charging {
        format!("{}% (charging)", s.battery_percent)
    } else {
        format!("{}%", s.battery_percent)
    }
}

/// Format the diagnostic `battery raw` line: `"<n> mV"`.
///
/// Added 2026-07-05 for the ADC-calibration investigation: unlike
/// `format_battery`'s `battery_percent` (frozen at the pre-charge basis while
/// charging — see firmware `battery` module docs), `battery_raw_mv` is the
/// live, unfrozen ADC-derived pack voltage every time this command runs —
/// the number to compare directly against a multimeter or the charger's
/// charge-complete LED state. Diagnostic-only: not shown on the on-device
/// admin-menu screen.
fn format_battery_raw_mv(s: &protocol::provisioning::RspStatusPayload) -> String {
    format!("{} mV", s.battery_raw_mv)
}

/// Format the `battery held raw` line: `"<n> mV"`.
///
/// Added 2026-07-05: distinct
/// from `format_battery_raw_mv`'s live, rail-contaminated reading, this is
/// the last known non-charge-inflated ("resting") voltage — frozen the
/// instant charging starts, same latch `format_battery`'s percent is derived
/// from. Because USB carries both the host CLI UART and charge power on this
/// board, a live `battery raw` read is always taken with the charger's rail
/// on the pin; this field is the instrument that lets the operator recover
/// the true pre-charge pack voltage despite that constraint (unplug, briefly
/// wait, replug to re-attach the CLI, then read this line).
fn format_battery_held_raw_mv(s: &protocol::provisioning::RspStatusPayload) -> String {
    format!("{} mV", s.battery_held_raw_mv)
}

#[cfg(test)]
mod tests {
    use super::url_encode;
    use super::{
        format_battery, format_battery_held_raw_mv, format_battery_raw_mv, format_gps_clock,
        format_gps_coords, format_gps_fix,
    };
    use protocol::provisioning::RspStatusPayload;

    fn status_with_gps(
        gps_has_fix: bool,
        gps_lat_e7: i32,
        gps_lon_e7: i32,
        gps_fix_age_secs: u32,
        gps_clock_synced: bool,
        gps_clock_sync_age_secs: u32,
    ) -> RspStatusPayload {
        RspStatusPayload {
            provisioned: true,
            pubkey: [0u8; 32],
            contact_count: 0,
            channel_count: 0,
            gps_has_fix,
            gps_lat_e7,
            gps_lon_e7,
            gps_fix_age_secs,
            gps_clock_synced,
            gps_clock_sync_age_secs,
            battery_percent: 0,
            battery_charging: false,
            battery_raw_mv: 0,
            battery_held_raw_mv: 0,
        }
    }

    fn status_with_battery(battery_percent: u8, battery_charging: bool) -> RspStatusPayload {
        status_with_battery_raw_mv(battery_percent, battery_charging, 0)
    }

    fn status_with_battery_raw_mv(
        battery_percent: u8,
        battery_charging: bool,
        battery_raw_mv: u16,
    ) -> RspStatusPayload {
        status_with_battery_raw_and_held_mv(battery_percent, battery_charging, battery_raw_mv, 0)
    }

    fn status_with_battery_raw_and_held_mv(
        battery_percent: u8,
        battery_charging: bool,
        battery_raw_mv: u16,
        battery_held_raw_mv: u16,
    ) -> RspStatusPayload {
        RspStatusPayload {
            provisioned: true,
            pubkey: [0u8; 32],
            contact_count: 0,
            channel_count: 0,
            gps_has_fix: false,
            gps_lat_e7: 0,
            gps_lon_e7: 0,
            gps_fix_age_secs: 0,
            gps_clock_synced: false,
            gps_clock_sync_age_secs: 0,
            battery_percent,
            battery_charging,
            battery_raw_mv,
            battery_held_raw_mv,
        }
    }

    #[test]
    fn gps_fix_never_had_fix() {
        let s = status_with_gps(false, 0, 0, 0, false, 0);
        assert_eq!(format_gps_fix(&s), "no");
        assert_eq!(format_gps_coords(&s), "\u{2014}");
    }

    #[test]
    fn gps_fix_and_coords_with_age() {
        let s = status_with_gps(true, 481_173_000, 115_166_667, 42, false, 0);
        assert_eq!(format_gps_fix(&s), "yes");
        assert_eq!(format_gps_coords(&s), "48.117300, 11.516667 (age 42s)");
    }

    #[test]
    fn gps_clock_never_synced() {
        let s = status_with_gps(false, 0, 0, 0, false, 0);
        assert_eq!(format_gps_clock(&s), "not synced");
    }

    #[test]
    fn gps_clock_synced_shows_age() {
        let s = status_with_gps(true, 0, 0, 0, true, 300);
        assert_eq!(format_gps_clock(&s), "synced (age 300s)");
    }

    #[test]
    fn battery_not_charging_shows_bare_percent() {
        let s = status_with_battery(82, false);
        assert_eq!(format_battery(&s), "82%");
    }

    #[test]
    fn battery_charging_appends_suffix() {
        let s = status_with_battery(14, true);
        assert_eq!(format_battery(&s), "14% (charging)");
    }

    #[test]
    fn battery_zero_percent_formats_cleanly() {
        let s = status_with_battery(0, false);
        assert_eq!(format_battery(&s), "0%");
    }

    #[test]
    fn battery_full_charge_formats_cleanly() {
        let s = status_with_battery(100, true);
        assert_eq!(format_battery(&s), "100% (charging)");
    }

    #[test]
    fn battery_raw_mv_formats_with_unit_suffix() {
        let s = status_with_battery_raw_mv(36, false, 3624);
        assert_eq!(format_battery_raw_mv(&s), "3624 mV");
    }

    #[test]
    fn battery_raw_mv_defaults_to_zero_when_unset() {
        let s = status_with_battery(0, false);
        assert_eq!(format_battery_raw_mv(&s), "0 mV");
    }

    #[test]
    fn battery_held_raw_mv_formats_with_unit_suffix() {
        // The held/last-unplugged reading is distinct from the live raw_mv —
        // exercise a case where the two differ, matching the real scenario
        // this field exists for (charging: live shows the contaminated rail,
        // held shows the frozen pre-charge basis).
        let s = status_with_battery_raw_and_held_mv(36, true, 4888, 3624);
        assert_eq!(format_battery_held_raw_mv(&s), "3624 mV");
        assert_eq!(format_battery_raw_mv(&s), "4888 mV");
    }

    #[test]
    fn battery_held_raw_mv_defaults_to_zero_when_unset() {
        let s = status_with_battery(0, false);
        assert_eq!(format_battery_held_raw_mv(&s), "0 mV");
    }

    #[test]
    fn url_encode_passes_unreserved() {
        assert_eq!(url_encode("MeshCadet-AB_1.2~3"), "MeshCadet-AB_1.2~3");
    }

    #[test]
    fn url_encode_escapes_space_and_reserved() {
        assert_eq!(url_encode("Mom & Dad"), "Mom%20%26%20Dad");
        assert_eq!(url_encode("a=b#c"), "a%3Db%23c");
    }

    #[test]
    fn url_encode_escapes_utf8_multibyte() {
        // "é" is U+00E9 → UTF-8 0xC3 0xA9
        assert_eq!(url_encode("é"), "%C3%A9");
    }

    /// The identity QR must encode the exact MeshCore companion contact URI
    /// (faq.md §7.5) and be renderable as a QR code without error for a full
    /// 64-hex-char pubkey and a name needing percent-encoding.
    #[test]
    fn identity_uri_builds_and_encodes_as_qr() {
        let pubkey = [0xABu8; 32];
        let pubkey_hex = hex::encode(pubkey);
        let name = "Mom & Dad's T-Deck";
        let uri = format!(
            "meshcore://contact/add?name={}&public_key={}&type=1",
            url_encode(name),
            pubkey_hex,
        );
        assert!(uri.starts_with("meshcore://contact/add?name="));
        assert!(uri.contains(
            "&public_key=abababababababababababababababababababababababababababababababab"
        ));
        assert!(uri.ends_with("&type=1"));
        assert!(!uri.contains(' '), "URI must not contain raw spaces");
        // Must encode as a QR code (byte mode); the companion app scans this.
        qrcode::QrCode::new(uri.as_bytes()).expect("identity URI must encode as a QR code");
    }
}
