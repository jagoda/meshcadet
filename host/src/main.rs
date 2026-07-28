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
//! meshcadet gen-channel-secret --bits 256
//! meshcadet --port /dev/ttyUSB0 add-contact --pubkey <HEX64> --name "Alice" --telemetry
//! meshcadet --port /dev/ttyUSB0 add-channel --secret <HEX64> --name "family" --primary
//! meshcadet --port /dev/ttyUSB0 add-room --pubkey <HEX64> --name "Lobby" --password-stdin
//! meshcadet --port /dev/ttyUSB0 list-rooms
//! meshcadet --port /dev/ttyUSB0 set-notif-defaults --visual --audible
//! meshcadet --port /dev/ttyUSB0 set-pin --pin 1234
//! meshcadet --port /dev/ttyUSB0 commit
//! meshcadet --port /dev/ttyUSB0 reset-pin --pin 5678
//! meshcadet --port /dev/ttyUSB0 clear-history
//! ```
//!
//! `gen-channel-secret` is the one exception to "every command needs
//! `--port`": it never touches the device, so it works standalone (see
//! example above).

use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
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
    ///
    /// Required for every command except `gen-channel-secret`, which is a
    /// pure-local CSPRNG operation with no device round trip.
    #[arg(short, long)]
    port: Option<String>,

    /// Serial baud rate.
    #[arg(short, long, default_value = "115200")]
    baud: u32,

    #[command(subcommand)]
    cmd: Cmd,
}

/// Channel-secret strength for [`Cmd::GenChannelSecret`], matching the two
/// widths [`parse_channel_secret_hex`] accepts.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum SecretBits {
    #[value(name = "128")]
    Bits128,
    #[value(name = "256")]
    Bits256,
}

impl SecretBits {
    /// Secret length in bytes: 16 for 128-bit, 32 for 256-bit.
    fn byte_len(self) -> usize {
        match self {
            SecretBits::Bits128 => 16,
            SecretBits::Bits256 => 32,
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Query device provisioning status and identity.
    Status,

    /// Read device identity and print a scannable MeshCore contact QR code,
    /// plus the device's signed self-advert "card".
    ///
    /// Prints TWO contact formats, sourced differently and meant for
    /// different consumers:
    /// - Format A: `meshcore://contact/add?name=...&public_key=...&type=1` +
    ///   its QR code. This is what the official MeshCore companion app
    ///   scans; built locally from the identity readout.
    /// - Format B: `meshcore://<hex-encoded card>`, the device's own signed
    ///   self-advert ("biz card") fetched fresh over serial (`FRAME_QUERY_ADVERT`
    ///   / `FRAME_RSP_ADVERT`) — this device is the only source, since the
    ///   signature requires its Ed25519 private key, which never leaves it.
    ///   This is the format `meshcore-cli import_contact` (alias `ic`) expects.
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

        /// Print ONLY Format B's bare `meshcore://<hex>` card URI to stdout —
        /// no label, no Format A, no QR, no trailing commentary — so it can
        /// be piped directly, e.g.
        /// `meshcore-cli import_contact "$(meshcadet ... identity --raw)"`.
        #[arg(long, action = ArgAction::SetTrue)]
        raw: bool,
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
    ///
    /// The channel secret crosses the USB link in the clear — correct and
    /// intentional, the cable is the authentication (see ADR-0001 §4) — but
    /// it must not leak anywhere else: not in shell history, not in `ps`,
    /// not in `--help`, not in an echoed confirmation or error message.
    /// Supply it via `--secret-file`, `--secret-env`, or `--secret-stdin`
    /// where practical; `--secret` exists for scriptability but is recorded
    /// in shell history (and briefly visible in process listings on most
    /// OSes). Omit all four to be prompted.
    AddChannel {
        /// Channel secret — 32 hex chars (128-bit / 16-byte) or 64 hex chars
        /// (256-bit / 32-byte), given directly on the command line. Exposed
        /// in shell history (and briefly visible in process listings on
        /// most OSes); prefer one of the other --secret-* sources. Mutually
        /// exclusive with them.
        #[arg(long)]
        secret: Option<String>,

        /// Read the channel secret from a file (its trailing newline, if
        /// any, is stripped). Mutually exclusive with the other --secret-*
        /// sources.
        #[arg(long = "secret-file", value_name = "PATH")]
        secret_file: Option<PathBuf>,

        /// Read the channel secret from the named environment variable.
        /// Mutually exclusive with the other --secret-* sources.
        #[arg(long = "secret-env", value_name = "VAR")]
        secret_env: Option<String>,

        /// Read the channel secret as one line from stdin (trailing
        /// newline stripped). Mutually exclusive with the other --secret-*
        /// sources.
        #[arg(long = "secret-stdin", action = ArgAction::SetTrue)]
        secret_stdin: bool,

        /// Channel name shown on screen.
        #[arg(long)]
        name: Option<String>,

        /// Set this channel as the primary (default outgoing) channel.
        #[arg(long, action = ArgAction::SetTrue)]
        primary: bool,
    },

    /// Remove a channel from the device.
    ///
    /// The channel secret must match exactly what was passed to
    /// `add-channel` — see that command's exposure note; the same
    /// `--secret-file`/`--secret-env`/`--secret-stdin` sources (and
    /// interactive prompt) apply here.
    DelChannel {
        /// Channel secret — 32 hex chars (128-bit) or 64 hex chars
        /// (256-bit), given directly on the command line; must match
        /// exactly what was passed to add-channel. Exposed in shell
        /// history (and briefly visible in process listings on most
        /// OSes); prefer one of the other --secret-* sources. Mutually
        /// exclusive with them.
        #[arg(long)]
        secret: Option<String>,

        /// Read the channel secret from a file (its trailing newline, if
        /// any, is stripped). Mutually exclusive with the other --secret-*
        /// sources.
        #[arg(long = "secret-file", value_name = "PATH")]
        secret_file: Option<PathBuf>,

        /// Read the channel secret from the named environment variable.
        /// Mutually exclusive with the other --secret-* sources.
        #[arg(long = "secret-env", value_name = "VAR")]
        secret_env: Option<String>,

        /// Read the channel secret as one line from stdin (trailing
        /// newline stripped). Mutually exclusive with the other --secret-*
        /// sources.
        #[arg(long = "secret-stdin", action = ArgAction::SetTrue)]
        secret_stdin: bool,
    },

    /// Generate a CSPRNG channel secret and print it as hex.
    ///
    /// Uses `OsRng` (matches the RNG `firmware`/`protocol` already use for
    /// identity key material) — never hand-type a channel secret, since
    /// `parse_channel_secret_hex` only validates hex-ness and length, not
    /// entropy. Output is exactly the hex format `add-channel --secret-stdin`
    /// / `del-channel --secret-stdin` expect (lowercase, no `0x` prefix, one
    /// line), so it composes directly via a pipe:
    ///
    /// ```text
    /// meshcadet gen-channel-secret --bits 256 \
    ///   | meshcadet --port /dev/ttyUSB0 add-channel --secret-stdin --name family --primary
    /// ```
    ///
    /// Pipe it — do not capture it into a shell variable and hand it back on
    /// the command line. A command-substituted argument still lands on
    /// argv, which is visible to every other user via `ps`/`/proc/<pid>/cmdline`
    /// for as long as the process runs, exactly the exposure
    /// `--secret-file`/`--secret-env`/`--secret-stdin` exist to avoid (see
    /// `AddChannel`'s doc comment). `--secret-stdin` reads a single line, so
    /// a straight pipe from this command's one-line stdout is the canonical
    /// composition.
    ///
    /// Does not require `--port` — this never touches the device.
    GenChannelSecret {
        /// Secret strength: 128-bit (32 hex chars) or 256-bit (64 hex chars).
        #[arg(long)]
        bits: SecretBits,
    },

    /// List the device's configured room-server contacts (pubkey + name +
    /// sync/permission state).
    ///
    /// A room is stored as a contact with `role = room` (see
    /// `docs/adr/0002-provisioning-wire-format.md` §7); this listing is
    /// scoped to rooms only, so it never has to be filtered client-side. It
    /// never includes the guest password — the device does not echo it back.
    ListRooms,

    /// Add (or replace) a room-server contact: pubkey, display name, and a
    /// guest password.
    ///
    /// The guest password crosses the USB link in the clear — correct and
    /// intentional, the cable is the authentication (see ADR-0001 §4) — but
    /// it must not leak anywhere else: not in shell history, not in `ps`,
    /// not in `--help`, not in an echoed confirmation or error message.
    /// Supply it via `--password-file`, `--password-env`, or
    /// `--password-stdin` where practical; `--password` exists for
    /// scriptability but is recorded in shell history (and briefly visible
    /// in process listings on most OSes). Omit all four to be prompted.
    AddRoom {
        /// Room server Ed25519 public key — 64 hex characters (32 bytes).
        #[arg(long)]
        pubkey: String,

        /// Display name shown on screen (optional; defaults to routing hash if absent).
        #[arg(long)]
        name: Option<String>,

        /// Guest password, given directly on the command line. Exposed in
        /// shell history; prefer one of the other --password-* sources.
        /// Mutually exclusive with them.
        #[arg(long)]
        password: Option<String>,

        /// Read the guest password from a file (its trailing newline, if
        /// any, is stripped). Mutually exclusive with the other
        /// --password-* sources.
        #[arg(long = "password-file", value_name = "PATH")]
        password_file: Option<PathBuf>,

        /// Read the guest password from the named environment variable.
        /// Mutually exclusive with the other --password-* sources.
        #[arg(long = "password-env", value_name = "VAR")]
        password_env: Option<String>,

        /// Read the guest password as one line from stdin (trailing newline
        /// stripped) — e.g. `echo -n "$PW" | meshcadet ... add-room
        /// --password-stdin ...`. Mutually exclusive with the other
        /// --password-* sources.
        #[arg(long = "password-stdin", action = ArgAction::SetTrue)]
        password_stdin: bool,
    },

    /// Remove a room-server contact from the device.
    DelRoom {
        /// Room server Ed25519 public key — 64 hex characters (32 bytes).
        #[arg(long)]
        pubkey: String,
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
    ///
    /// The PIN crosses the USB link in the clear — correct and intentional,
    /// the cable is the authentication (see ADR-0001 §4) — but it must not
    /// leak anywhere else: not in shell history, not in `ps`, not in
    /// `--help`, not in an echoed confirmation or error message. Supply it
    /// via `--pin-file`, `--pin-env`, or `--pin-stdin` where practical;
    /// `--pin` exists for scriptability but is recorded in shell history
    /// (and briefly visible in process listings on most OSes). Omit all
    /// four to be prompted.
    SetPin {
        /// PIN string (UTF-8, max 16 bytes), given directly on the command
        /// line. Exposed in shell history (and briefly visible in process
        /// listings on most OSes); prefer one of the other --pin-*
        /// sources. Mutually exclusive with them.
        #[arg(long)]
        pin: Option<String>,

        /// Read the PIN from a file (its trailing newline, if any, is
        /// stripped). Mutually exclusive with the other --pin-* sources.
        #[arg(long = "pin-file", value_name = "PATH")]
        pin_file: Option<PathBuf>,

        /// Read the PIN from the named environment variable. Mutually
        /// exclusive with the other --pin-* sources.
        #[arg(long = "pin-env", value_name = "VAR")]
        pin_env: Option<String>,

        /// Read the PIN as one line from stdin (trailing newline
        /// stripped). Mutually exclusive with the other --pin-* sources.
        #[arg(long = "pin-stdin", action = ArgAction::SetTrue)]
        pin_stdin: bool,
    },

    /// Commit provisioning: persist config to flash.
    ///
    /// Run this after all contacts, channels, and settings have been provisioned.
    /// On a first-boot device the firmware reboots into the mesh after committing;
    /// on an already-provisioned device it re-persists live config without rebooting.
    Commit,

    /// Reset the admin PIN (physical USB possession is the auth factor).
    ///
    /// Equivalent to set-pin but clearly named for the recovery flow. The
    /// same exposure note and --pin-file/--pin-env/--pin-stdin sources
    /// (and interactive prompt) apply here.
    ResetPin {
        /// New PIN string (UTF-8, max 16 bytes), given directly on the
        /// command line. Exposed in shell history (and briefly visible in
        /// process listings on most OSes); prefer one of the other
        /// --pin-* sources. Mutually exclusive with them.
        #[arg(long)]
        pin: Option<String>,

        /// Read the PIN from a file (its trailing newline, if any, is
        /// stripped). Mutually exclusive with the other --pin-* sources.
        #[arg(long = "pin-file", value_name = "PATH")]
        pin_file: Option<PathBuf>,

        /// Read the PIN from the named environment variable. Mutually
        /// exclusive with the other --pin-* sources.
        #[arg(long = "pin-env", value_name = "VAR")]
        pin_env: Option<String>,

        /// Read the PIN as one line from stdin (trailing newline
        /// stripped). Mutually exclusive with the other --pin-* sources.
        #[arg(long = "pin-stdin", action = ArgAction::SetTrue)]
        pin_stdin: bool,
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

    // `gen-channel-secret` is a pure-local CSPRNG operation — it never
    // touches the device, so it must run before (and without) the
    // `--port`-gated transport open below. `bits` is `Copy`, so this only
    // reads `cli.cmd`'s discriminant + copies that field; it does not move
    // `cli.cmd`, which the `match` below still owns in full.
    if let Cmd::GenChannelSecret { bits } = cli.cmd {
        return gen_channel_secret(bits);
    }

    let port = cli
        .port
        .as_deref()
        .context("--port is required for this command")?;
    let transport = SerialTransport::open(port, cli.baud)?;
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

        Cmd::Identity {
            name,
            set_name,
            raw,
        } => {
            // SET (optional): persist a new device display name to the
            // identity store before reading it back. Additive to the
            // pre-existing read-only behavior below — with `set_name` absent
            // (the common case), nothing here runs and the read/QR output is
            // unchanged from before this command existed. In `--raw` mode
            // the confirmation is routed to stderr rather than stdout, so a
            // `--raw --set-name` invocation still pipes cleanly.
            if let Some(new_name) = &set_name {
                if new_name.len() > protocol::provisioning::MAX_NAME_LEN {
                    anyhow::bail!(
                        "device name must be at most {} bytes (UTF-8); got {} bytes",
                        protocol::provisioning::MAX_NAME_LEN,
                        new_name.len()
                    );
                }
                session.set_device_name(new_name.as_bytes())?;
                let msg = if new_name.is_empty() {
                    "device name cleared".to_string()
                } else {
                    format!("device name set: \"{}\"", new_name)
                };
                if raw {
                    eprintln!("{}", msg);
                } else {
                    println!("{}", msg);
                }
            }

            let s = session.query_status()?;
            let pubkey_hex = hex::encode(s.pubkey);
            if !raw {
                println!("pubkey   : {}", pubkey_hex);
                println!(
                    "pub_hash : 0x{:02X}  (routing hash = pubkey[0])",
                    s.pubkey[0]
                );
                println!(
                    "name     : {}",
                    session.last_device_name().unwrap_or("(unnamed)")
                );
            }

            // Format A: the MeshCore companion contact-add URI, built locally
            // from the identity readout above.
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
            let uri = build_contact_add_uri(&node_name, &pubkey_hex);

            if !raw {
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
                        println!(
                            "Scan with a MeshCore companion app to add this node as a contact."
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "warning: could not render QR code ({}); use the URI above.",
                            e
                        );
                    }
                }
            }

            // Format B: the device's own signed self-advert "card", fetched
            // fresh over serial — the host cannot synthesize this (the
            // signature needs the device's Ed25519 private key, which never
            // leaves it). This is the format `meshcore-cli import_contact`
            // expects.
            //
            // In `--raw` mode this is the command's ENTIRE job, so a failure
            // here (e.g. older firmware that predates `FRAME_QUERY_ADVERT`)
            // propagates and the command exits non-zero — there is nothing
            // sensible to print instead. In normal mode Format A above has
            // already fully succeeded and been printed; treat a Format B
            // fetch failure as non-fatal (mirrors the QR-render-failure
            // fallback above) so `identity` degrades to "Format A only"
            // rather than losing output the user already had before this
            // command grew a second, independent device round-trip.
            match session.query_advert() {
                Ok(card) => {
                    let mut uri_buf = [0u8; protocol::MAX_CARD_URI_LEN];
                    let n = protocol::card_to_uri(&card, &mut uri_buf);
                    let card_uri = std::str::from_utf8(&uri_buf[..n]).expect(
                        "card_to_uri always emits ASCII (meshcore:// scheme + lowercase hex)",
                    );
                    if raw {
                        // Bare URI only — no label, no trailing prose — so
                        // this is safe to capture verbatim, e.g.
                        // `meshcore-cli import_contact "$(meshcadet ... identity --raw)"`.
                        println!("{}", card_uri);
                    } else {
                        println!(
                            "Card URI (paste verbatim into `meshcore-cli import_contact <URI>`):\n{}",
                            card_uri
                        );
                    }
                }
                Err(e) if raw => return Err(e),
                Err(e) => {
                    eprintln!(
                        "warning: could not fetch the device's self-advert card ({}); \
                         Format A above is still valid, but Format B is unavailable this run \
                         (older firmware without FRAME_QUERY_ADVERT support, or a transient \
                         serial error).",
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
            secret_file,
            secret_env,
            secret_stdin,
            name,
            primary,
        } => {
            let secret = resolve_channel_secret(
                secret.as_deref(),
                secret_file.as_deref(),
                secret_env.as_deref(),
                secret_stdin,
            )?;
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

        Cmd::DelChannel {
            secret,
            secret_file,
            secret_env,
            secret_stdin,
        } => {
            let secret = resolve_channel_secret(
                secret.as_deref(),
                secret_file.as_deref(),
                secret_env.as_deref(),
                secret_stdin,
            )?;
            let (sec, _key_len) = parse_channel_secret_hex(&secret)?;
            session.del_channel(&sec)?;
            println!("channel removed: {}", hex_short(&sec));
        }

        Cmd::GenChannelSecret { .. } => {
            unreachable!("handled above, before the device transport is opened")
        }

        Cmd::ListRooms => {
            let rooms = session.list_rooms()?;
            if rooms.is_empty() {
                println!("no rooms configured");
            } else {
                println!("idx\tpubkey                                                           \tsync_since\tperms\tname");
                for r in &rooms {
                    let name = std::str::from_utf8(&r.name[..r.name_len as usize])
                        .unwrap_or("<invalid utf-8>");
                    println!(
                        "{}\t{}\t{}\t0x{:02X}\t{}",
                        r.index,
                        hex::encode(r.pubkey),
                        r.sync_since,
                        r.permissions,
                        name,
                    );
                }
                println!("{} room(s)", rooms.len());
            }
        }

        Cmd::AddRoom {
            pubkey,
            name,
            password,
            password_file,
            password_env,
            password_stdin,
        } => {
            let pk = parse_32bytes_hex(&pubkey, "pubkey")?;
            let guest_password = resolve_guest_password(
                password.as_deref(),
                password_file.as_deref(),
                password_env.as_deref(),
                password_stdin,
            )?;
            // Warn (length only — never the value) if the device will
            // silently truncate the password, rather than let a mismatch
            // surface later as an inexplicable login failure.
            if let Some(warning) = password_truncation_warning(guest_password.len()) {
                eprintln!("{warning}");
            }
            let name_bytes = name.as_deref().unwrap_or("").as_bytes().to_vec();
            session.add_room(&pk, guest_password.as_bytes(), &name_bytes)?;
            println!(
                "room added: {}{}",
                hex_short(&pk),
                name.as_deref()
                    .map(|n| format!(", name=\"{}\"", n))
                    .unwrap_or_default()
            );
            // Same boot-time gate as add-contact: the live allowlist is
            // populated from the provisioned contact list at boot (see
            // firmware-core's `config_store` module docs), and a room is a
            // contact under the hood.
            println!(
                "  note: reboot the device to apply this to the live mesh (allowlist is loaded at boot)."
            );

            let display_name = name.unwrap_or_else(|| format!("Room-{:02X}", pk[0]));
            let uri = build_room_add_uri(&display_name, &hex::encode(pk));
            println!("\nMeshCore room URI:\n{}\n", uri);
            match qrcode::QrCode::new(uri.as_bytes()) {
                Ok(code) => {
                    let rendered = code
                        .render::<qrcode::render::unicode::Dense1x2>()
                        .quiet_zone(true)
                        .build();
                    println!("{}", rendered);
                    println!(
                        "Scan with a MeshCore companion app to add this room as a contact. \
                         The guest password is NOT encoded in this QR (see ADR-0002 §7) — \
                         communicate it separately."
                    );
                }
                Err(e) => {
                    eprintln!(
                        "warning: could not render QR code ({}); use the URI above.",
                        e
                    );
                }
            }
        }

        Cmd::DelRoom { pubkey } => {
            let pk = parse_32bytes_hex(&pubkey, "pubkey")?;
            session.del_room(&pk)?;
            println!("room removed: {}", hex_short(&pk));
            // FINDING E (deep-review pass 2, meshcadet-room-lifecycle-session-store):
            // unlike a deleted plain contact/channel, a room server has an
            // ACTIVE dispatcher-loop session (`RoomRuntime` in firmware's
            // main.rs) that this edit does not touch — see admin_server.rs's
            // FRAME_DEL_ROOM handler doc. Until the device reboots it keeps
            // logging in to, keep-aliving, and syncing from this room exactly
            // as before, so the operator must not be left assuming the device
            // stopped talking to it the moment this command returns.
            println!(
                "  note: reboot the device to stop it logging in to / syncing from this room \
                 (the live session keeps running until then)."
            );
        }

        Cmd::SetNotifDefaults { visual, audible } => {
            session.set_notif_defaults(visual, audible)?;
            println!(
                "notification defaults set: visual={}, audible={}",
                visual, audible
            );
        }

        Cmd::SetPin {
            pin,
            pin_file,
            pin_env,
            pin_stdin,
        } => {
            let pin = resolve_admin_pin(
                pin.as_deref(),
                pin_file.as_deref(),
                pin_env.as_deref(),
                pin_stdin,
            )?;
            session.set_pin(pin.as_bytes())?;
            println!("PIN set successfully");
        }

        Cmd::Commit => {
            session.commit()?;
            println!("provisioning committed — config persisted to flash");
        }

        Cmd::ResetPin {
            pin,
            pin_file,
            pin_env,
            pin_stdin,
        } => {
            let pin = resolve_admin_pin(
                pin.as_deref(),
                pin_file.as_deref(),
                pin_env.as_deref(),
                pin_stdin,
            )?;
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

/// `gen-channel-secret` implementation: fill `bits.byte_len()` bytes from
/// `OsRng` (a CSPRNG — matches `firmware`/`protocol`'s use of the same RNG
/// for identity key material) and print them as lowercase hex, exactly the
/// format [`parse_channel_secret_hex`] expects. No device round trip.
fn gen_channel_secret(bits: SecretBits) -> anyhow::Result<()> {
    println!("{}", generate_channel_secret_hex(bits));
    Ok(())
}

/// Pure core of `gen-channel-secret`, split out from [`gen_channel_secret`]
/// so tests can exercise the actual RNG-to-hex path without capturing
/// stdout.
fn generate_channel_secret_hex(bits: SecretBits) -> String {
    use rand::RngCore;

    let byte_len = bits.byte_len();
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf[..byte_len]);
    hex::encode(&buf[..byte_len])
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
    if let Some(warning) = weak_secret_pattern_warning(&bytes) {
        eprintln!("{}", warning);
    }
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

/// Floor-raiser for [`parse_channel_secret_hex`]: flags obviously
/// low-entropy secret bytes with a stderr warning.
///
/// Deliberately a *floor*, not an entropy estimator: it only catches the
/// patterns a hand-typed placeholder is likely to be (all-zero, all one
/// repeated byte, a simple ascending/descending byte run like
/// `000102030405…` or `ffedcba9…`). It never rejects — any real CSPRNG
/// output, including one that coincidentally resembles a pattern in a tiny
/// slice, still parses and provisions; this is a warning surface for the
/// "operator hand-typed 32 zeroes" case the audit flagged, not a gate.
fn weak_secret_pattern_warning(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 {
        return None;
    }
    const ADVICE: &str =
        "generate a real one with `meshcadet gen-channel-secret --bits 128` (or `--bits 256`).";
    if bytes.iter().all(|&b| b == bytes[0]) {
        return Some(format!(
            "warning: secret is {} repeated 0x{:02x} bytes — this has near-zero entropy; {ADVICE}",
            bytes.len(),
            bytes[0],
        ));
    }
    let ascending = bytes.windows(2).all(|w| w[1] == w[0].wrapping_add(1));
    let descending = bytes.windows(2).all(|w| w[1] == w[0].wrapping_sub(1));
    if ascending || descending {
        return Some(format!(
            "warning: secret is a simple sequential byte pattern — this has near-zero entropy; {ADVICE}"
        ));
    }
    None
}

/// First 4 bytes of a 32-byte value as `aabbccdd…` shorthand for display.
fn hex_short(b: &[u8; 32]) -> String {
    format!("{}…", hex::encode(&b[..4]))
}

/// Resolve `add-room`'s guest password from exactly one of its four
/// mutually-exclusive sources, or prompt for it interactively if none were
/// given.
///
/// Precedence is irrelevant by construction — at most one of `password`,
/// `password_file`, `password_env`, `password_stdin` may be set; more than
/// one is a user error, rejected up front. Never logs, echoes, or embeds the
/// resolved password in an error message (a file-read or env-lookup error
/// names the *path*/*variable*, never the secret).
fn resolve_guest_password(
    password: Option<&str>,
    password_file: Option<&std::path::Path>,
    password_env: Option<&str>,
    password_stdin: bool,
) -> anyhow::Result<String> {
    let sources_given = password.is_some() as u8
        + password_file.is_some() as u8
        + password_env.is_some() as u8
        + password_stdin as u8;
    if sources_given > 1 {
        anyhow::bail!(
            "specify at most one of --password, --password-file, --password-env, --password-stdin"
        );
    }

    if let Some(p) = password {
        return Ok(p.to_string());
    }
    if let Some(path) = password_file {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading --password-file {}", path.display()))?;
        return Ok(raw.trim_end_matches(['\r', '\n']).to_string());
    }
    if let Some(var) = password_env {
        let raw = std::env::var(var)
            .map_err(|_| anyhow::anyhow!("environment variable {var} is not set"))?;
        return Ok(raw);
    }
    if password_stdin {
        let mut raw = String::new();
        std::io::stdin()
            .read_line(&mut raw)
            .context("reading guest password from stdin")?;
        return Ok(raw.trim_end_matches(['\r', '\n']).to_string());
    }

    // No source given: prompt interactively on stderr (so `--raw`-style
    // piping of stdout is never polluted). MeshCadet carries no
    // hidden-input dependency today, so this echoes to the terminal like
    // any other `read_line` prompt — still strictly better than
    // `--password` landing in shell history.
    eprint!("guest password (leave empty for none): ");
    std::io::stderr().flush().ok();
    let mut raw = String::new();
    std::io::stdin()
        .read_line(&mut raw)
        .context("reading guest password from stdin prompt")?;
    Ok(raw.trim_end_matches(['\r', '\n']).to_string())
}

/// Resolve `add-channel`/`del-channel`'s channel secret from exactly one of
/// its four mutually-exclusive sources, or prompt for it interactively if
/// none were given. Mirrors [`resolve_guest_password`].
///
/// Precedence is irrelevant by construction — at most one of `secret`,
/// `secret_file`, `secret_env`, `secret_stdin` may be set; more than one is
/// a user error, rejected up front. Never logs, echoes, or embeds the
/// resolved secret in an error message (a file-read or env-lookup error
/// names the *path*/*variable*, never the secret).
///
/// Only trailing `\r`/`\n` are stripped, never trimmed further —
/// `del-channel --secret` must match EXACTLY what was passed to
/// `add-channel`, and any additional trimming here (or divergent trimming
/// between the file/env/stdin branches) would open a mismatch between the
/// two commands' resolved values.
fn resolve_channel_secret(
    secret: Option<&str>,
    secret_file: Option<&std::path::Path>,
    secret_env: Option<&str>,
    secret_stdin: bool,
) -> anyhow::Result<String> {
    let sources_given = secret.is_some() as u8
        + secret_file.is_some() as u8
        + secret_env.is_some() as u8
        + secret_stdin as u8;
    if sources_given > 1 {
        anyhow::bail!(
            "specify at most one of --secret, --secret-file, --secret-env, --secret-stdin"
        );
    }

    if let Some(s) = secret {
        return Ok(s.to_string());
    }
    if let Some(path) = secret_file {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading --secret-file {}", path.display()))?;
        return Ok(raw.trim_end_matches(['\r', '\n']).to_string());
    }
    if let Some(var) = secret_env {
        let raw = std::env::var(var)
            .map_err(|_| anyhow::anyhow!("environment variable {var} is not set"))?;
        return Ok(raw);
    }
    if secret_stdin {
        let mut raw = String::new();
        std::io::stdin()
            .read_line(&mut raw)
            .context("reading channel secret from stdin")?;
        return Ok(raw.trim_end_matches(['\r', '\n']).to_string());
    }

    // No source given: prompt interactively on stderr (so `--raw`-style
    // piping of stdout is never polluted) — mirrors resolve_guest_password.
    eprint!("channel secret (hex): ");
    std::io::stderr().flush().ok();
    let mut raw = String::new();
    std::io::stdin()
        .read_line(&mut raw)
        .context("reading channel secret from stdin prompt")?;
    Ok(raw.trim_end_matches(['\r', '\n']).to_string())
}

/// Resolve `set-pin`/`reset-pin`'s admin PIN from exactly one of its four
/// mutually-exclusive sources, or prompt for it interactively if none were
/// given. Mirrors [`resolve_guest_password`]/[`resolve_channel_secret`].
///
/// Precedence is irrelevant by construction — at most one of `pin`,
/// `pin_file`, `pin_env`, `pin_stdin` may be set; more than one is a user
/// error, rejected up front. Never logs, echoes, or embeds the resolved PIN
/// in an error message (a file-read or env-lookup error names the
/// *path*/*variable*, never the PIN).
fn resolve_admin_pin(
    pin: Option<&str>,
    pin_file: Option<&std::path::Path>,
    pin_env: Option<&str>,
    pin_stdin: bool,
) -> anyhow::Result<String> {
    let sources_given =
        pin.is_some() as u8 + pin_file.is_some() as u8 + pin_env.is_some() as u8 + pin_stdin as u8;
    if sources_given > 1 {
        anyhow::bail!("specify at most one of --pin, --pin-file, --pin-env, --pin-stdin");
    }

    if let Some(p) = pin {
        return Ok(p.to_string());
    }
    if let Some(path) = pin_file {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading --pin-file {}", path.display()))?;
        return Ok(raw.trim_end_matches(['\r', '\n']).to_string());
    }
    if let Some(var) = pin_env {
        let raw = std::env::var(var)
            .map_err(|_| anyhow::anyhow!("environment variable {var} is not set"))?;
        return Ok(raw);
    }
    if pin_stdin {
        let mut raw = String::new();
        std::io::stdin()
            .read_line(&mut raw)
            .context("reading admin PIN from stdin")?;
        return Ok(raw.trim_end_matches(['\r', '\n']).to_string());
    }

    // No source given: prompt interactively on stderr — mirrors
    // resolve_guest_password.
    eprint!("admin PIN (leave empty for none): ");
    std::io::stderr().flush().ok();
    let mut raw = String::new();
    std::io::stdin()
        .read_line(&mut raw)
        .context("reading admin PIN from stdin prompt")?;
    Ok(raw.trim_end_matches(['\r', '\n']).to_string())
}

/// Build the "your guest password will be truncated" warning for `add-room`,
/// or `None` if `password_len` fits under the device's limit. Reports only
/// the byte counts — never the password itself — so the caller can print it
/// straight to stderr without risking a leak.
///
/// The wire limit is `MAX_ROOM_PASSWORD_LEN` bytes INCLUDING the NUL
/// terminator `encode_anon_req_login` always appends (see that fn's doc) —
/// only `MAX_ROOM_PASSWORD_LEN - 1` password bytes ever reach the wire.
/// Warning at `password_len > MAX_ROOM_PASSWORD_LEN` (the old boundary) let a
/// password of exactly `MAX_ROOM_PASSWORD_LEN` bytes provision with no
/// warning at all, while silently operating as only its first
/// `MAX_ROOM_PASSWORD_LEN - 1` characters.
fn password_truncation_warning(password_len: usize) -> Option<String> {
    let effective_limit = protocol::provisioning::MAX_ROOM_PASSWORD_LEN - 1;
    if password_len > effective_limit {
        Some(format!(
            "warning: guest password is {password_len} bytes; the device only uses the first \
             {effective_limit} and will truncate it."
        ))
    } else {
        None
    }
}

/// Build the MeshCore companion contact-add URI ("Format A") from a display
/// name and a hex-encoded pubkey.
///
/// `meshcore://contact/add?name=<name>&public_key=<hex>&type=<type>`
/// (meshcore-dev/MeshCore docs/faq.md §7.5, companion-v1.16.0). `type=1`
/// (chat) is hardcoded — MeshCadet is always a chat node.
///
/// Extracted verbatim from the pre-existing inline `format!` call so its
/// byte output is unchanged; see `identity_uri_format_is_byte_identical_to_pre_mission`.
fn build_contact_add_uri(name: &str, pubkey_hex: &str) -> String {
    format!(
        "meshcore://contact/add?name={}&public_key={}&type=1",
        url_encode(name),
        pubkey_hex,
    )
}

/// Room node type (`ROLE_ROOM` in `firmware_core::config_store`) as it
/// appears in a `meshcore://contact/add?...&type=<n>` URI's `type` field.
/// (Chat's `type=1` stays a literal in [`build_contact_add_uri`] — see that
/// function's regression-anchor comment.)
const URI_NODE_TYPE_ROOM: u8 = 3;

/// Build the MeshCore companion room-add URI from a display name and a
/// hex-encoded room-server pubkey.
///
/// `meshcore://contact/add?name=<name>&public_key=<hex>&type=3`
///
/// Byte-identical in shape to [`build_contact_add_uri`] except `type=3`
/// (room server) instead of `type=1` (chat) — no other parameter is added.
///
/// # Why no guest-password parameter (ADR-0002 §7)
///
/// The upstream `meshcore://contact/add` scheme (docs/faq.md §7.5) has no
/// slot for a password at ANY `type=` value — a room server is provisioned
/// there exactly like a chat contact, identity-only. A public source for the
/// companion app's query-string parser could not be located to empirically
/// verify whether it ignores or hard-fails on an unrecognized parameter (see
/// ADR-0002 §7's investigation notes), so extending this URI with a
/// non-standard `password=` param was rejected as an unverifiable
/// compatibility risk. Reusing the upstream `type=3` value AS-IS costs
/// nothing and guarantees this URI parses on every companion-app version,
/// present and future, exactly as a chat contact URI does. The guest
/// password is instead communicated out-of-band (spoken, written down,
/// printed by the host CLI alongside — not inside — the QR) and entered by
/// whatever later UI implements the actual room-login flow (out of scope
/// here).
///
/// Called by `add-room` to print the room's contact-add URI/QR alongside the
/// provisioning round trip; see [`Cmd::AddRoom`].
fn build_room_add_uri(name: &str, pubkey_hex: &str) -> String {
    format!(
        "meshcore://contact/add?name={}&public_key={}&type={}",
        url_encode(name),
        pubkey_hex,
        URI_NODE_TYPE_ROOM,
    )
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

/// Percent-decode a URI query-component value — the inverse of [`url_encode`].
#[allow(dead_code)]
fn url_decode(s: &str) -> anyhow::Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s
                .get(i + 1..i + 3)
                .ok_or_else(|| anyhow::anyhow!("url_decode: truncated percent-escape"))?;
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|e| anyhow::anyhow!("url_decode: invalid percent-escape %{hex}: {e}"))?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out)
        .map_err(|e| anyhow::anyhow!("url_decode: invalid UTF-8 after percent-decode: {e}"))
}

/// A decoded `meshcore://contact/add` URI — the round-trip counterpart to
/// [`build_contact_add_uri`] / [`build_room_add_uri`].
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
struct ParsedContactUri {
    name: String,
    pubkey_hex: String,
    node_type: u8,
}

/// Parse a `meshcore://contact/add?name=...&public_key=...&type=...` URI.
///
/// Query parameters may appear in any order. An unrecognized parameter is
/// silently ignored rather than erroring — this parser's own tolerance
/// policy, independent of (and not a stand-in for) the still-unverified
/// upstream companion-app parser (see [`build_room_add_uri`]'s doc comment).
#[allow(dead_code)]
fn parse_contact_uri(uri: &str) -> anyhow::Result<ParsedContactUri> {
    let rest = uri
        .strip_prefix("meshcore://contact/add?")
        .ok_or_else(|| anyhow::anyhow!("not a meshcore://contact/add URI: {uri}"))?;

    let mut name = None;
    let mut pubkey_hex = None;
    let mut node_type = None;
    for pair in rest.split('&') {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("malformed query parameter: {pair}"))?;
        match key {
            "name" => name = Some(url_decode(value)?),
            "public_key" => pubkey_hex = Some(value.to_string()),
            "type" => {
                node_type = Some(
                    value
                        .parse::<u8>()
                        .map_err(|e| anyhow::anyhow!("invalid type value {value}: {e}"))?,
                )
            }
            _ => {} // unrecognized parameter — ignored, not an error
        }
    }
    Ok(ParsedContactUri {
        name: name.ok_or_else(|| anyhow::anyhow!("missing name parameter"))?,
        pubkey_hex: pubkey_hex.ok_or_else(|| anyhow::anyhow!("missing public_key parameter"))?,
        node_type: node_type.ok_or_else(|| anyhow::anyhow!("missing type parameter"))?,
    })
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
        build_contact_add_uri, build_room_add_uri, format_battery, format_battery_held_raw_mv,
        format_battery_raw_mv, format_gps_clock, format_gps_coords, format_gps_fix,
        generate_channel_secret_hex, parse_channel_secret_hex, parse_contact_uri,
        password_truncation_warning, resolve_admin_pin, resolve_channel_secret,
        resolve_guest_password, weak_secret_pattern_warning, SecretBits,
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

    /// Regression anchor: `build_contact_add_uri` ("Format A") must emit
    /// byte-for-byte the same string the inline `format!` call produced
    /// before this mission added Format B alongside it. Any future change to
    /// this literal is a deliberate, reviewed edit — the official MeshCore
    /// companion app depends on this exact query-string shape.
    #[test]
    fn identity_uri_format_is_byte_identical_to_pre_mission() {
        let pubkey_hex = hex::encode([0xABu8; 32]);
        let uri = build_contact_add_uri("Mom & Dad's T-Deck", &pubkey_hex);
        assert_eq!(
            uri,
            "meshcore://contact/add?name=Mom%20%26%20Dad%27s%20T-Deck&public_key=abababababababababababababababababababababababababababababababab&type=1"
        );
    }

    /// Format B's bare card URI (as printed under `--raw`) must be exactly
    /// the `meshcore://<hex>` string `protocol::card_to_uri` renders — no
    /// leading/trailing whitespace, no label, nothing else — so a caller can
    /// pipe it verbatim into `meshcore-cli import_contact`.
    #[test]
    fn card_uri_is_bare_meshcore_scheme_with_no_stray_whitespace() {
        let card = [0x11u8, 0x22, 0x33, 0xAA, 0xBB];
        let mut buf = [0u8; protocol::MAX_CARD_URI_LEN];
        let n = protocol::card_to_uri(&card, &mut buf);
        let uri = std::str::from_utf8(&buf[..n]).unwrap();

        assert_eq!(uri, "meshcore://112233aabb");
        assert_eq!(uri.trim(), uri, "must carry no leading/trailing whitespace");
        assert!(!uri.contains('\n'), "must be a single line");
    }

    /// Format A and Format B must be unambiguously distinguishable to a user
    /// who is copy-pasting: Format A always carries the `contact/add?`
    /// path + query string; Format B is the bare `meshcore://<hex>` scheme
    /// with nothing else after the `//`.
    #[test]
    fn format_a_and_format_b_uris_are_distinguishable() {
        let pubkey_hex = hex::encode([0xCDu8; 32]);
        let format_a = build_contact_add_uri("Cadet", &pubkey_hex);

        let card = [0xCDu8; 4];
        let mut buf = [0u8; protocol::MAX_CARD_URI_LEN];
        let n = protocol::card_to_uri(&card, &mut buf);
        let format_b = std::str::from_utf8(&buf[..n]).unwrap();

        assert!(format_a.contains("contact/add?"));
        assert!(!format_b.contains("contact/add?"));
        assert_ne!(format_a, format_b);
    }

    // ── Room URI (ADR-0002 §7) ────────────────────────────────────────────────

    /// Golden-string test pinning the room URI's exact byte output: same
    /// shape as the `type=1` contact URI, only `type=3` differs.
    #[test]
    fn room_uri_golden_string() {
        let pubkey_hex = hex::encode([0xABu8; 32]);
        let uri = build_room_add_uri("Lobby", &pubkey_hex);
        assert_eq!(
            uri,
            "meshcore://contact/add?name=Lobby&public_key=abababababababababababababababababababababababababababababababab&type=3"
        );
    }

    /// Regression guard: adding the room URI must not change the existing
    /// `type=1` contact URI's byte-for-byte output.
    #[test]
    fn contact_uri_byte_output_unchanged_by_room_uri_addition() {
        let pubkey_hex = hex::encode([0xABu8; 32]);
        let uri = build_contact_add_uri("Mom & Dad's T-Deck", &pubkey_hex);
        assert_eq!(
            uri,
            "meshcore://contact/add?name=Mom%20%26%20Dad%27s%20T-Deck&public_key=abababababababababababababababababababababababababababababababab&type=1"
        );
    }

    #[test]
    fn room_uri_round_trips_through_the_host_cli_parser() {
        let pubkey_hex = hex::encode([0xCDu8; 32]);
        let uri = build_room_add_uri("Mom & Dad's Lobby", &pubkey_hex);
        let parsed = parse_contact_uri(&uri).expect("room URI must parse");
        assert_eq!(parsed.name, "Mom & Dad's Lobby");
        assert_eq!(parsed.pubkey_hex, pubkey_hex);
        assert_eq!(parsed.node_type, 3);
    }

    #[test]
    fn contact_uri_round_trips_through_the_host_cli_parser() {
        let pubkey_hex = hex::encode([0x11u8; 32]);
        let uri = build_contact_add_uri("Alice", &pubkey_hex);
        let parsed = parse_contact_uri(&uri).expect("contact URI must parse");
        assert_eq!(parsed.name, "Alice");
        assert_eq!(parsed.pubkey_hex, pubkey_hex);
        assert_eq!(parsed.node_type, 1);
    }

    #[test]
    fn parse_contact_uri_tolerates_query_param_order() {
        let uri = "meshcore://contact/add?type=3&public_key=aabb&name=Z";
        let parsed = parse_contact_uri(uri).unwrap();
        assert_eq!(parsed.name, "Z");
        assert_eq!(parsed.pubkey_hex, "aabb");
        assert_eq!(parsed.node_type, 3);
    }

    #[test]
    fn parse_contact_uri_rejects_non_contact_scheme() {
        assert!(parse_contact_uri("meshcore://channel/add?name=x").is_err());
    }

    #[test]
    fn parse_contact_uri_rejects_missing_required_param() {
        assert!(parse_contact_uri("meshcore://contact/add?name=x&type=1").is_err());
    }

    // ── Guest-password resolution (add-room) ──────────────────────────────────

    #[test]
    fn resolve_guest_password_direct_flag() {
        let pw = resolve_guest_password(Some("hunter2"), None, None, false).unwrap();
        assert_eq!(pw, "hunter2");
    }

    #[test]
    fn resolve_guest_password_from_file_strips_trailing_newline() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("meshcadet-test-pw-{}.txt", std::process::id()));
        std::fs::write(&path, "s3cret\n").unwrap();
        let pw = resolve_guest_password(None, Some(path.as_path()), None, false).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(pw, "s3cret");
    }

    #[test]
    fn resolve_guest_password_from_env_var() {
        let var = format!("MESHCADET_TEST_PW_{}", std::process::id());
        std::env::set_var(&var, "envpass");
        let pw = resolve_guest_password(None, None, Some(var.as_str()), false).unwrap();
        std::env::remove_var(&var);
        assert_eq!(pw, "envpass");
    }

    #[test]
    fn resolve_guest_password_missing_env_var_errors() {
        let var = format!("MESHCADET_TEST_PW_MISSING_{}", std::process::id());
        std::env::remove_var(&var);
        let err = resolve_guest_password(None, None, Some(var.as_str()), false).unwrap_err();
        assert!(err.to_string().contains(&var));
    }

    #[test]
    fn resolve_guest_password_rejects_multiple_sources() {
        let err =
            resolve_guest_password(Some("supersecretvalue12345"), None, Some("SOME_VAR"), false)
                .unwrap_err();
        assert!(err.to_string().contains("at most one"));
        // The error message must name the flags, never a candidate value.
        assert!(!err.to_string().contains("supersecretvalue12345"));
    }

    #[test]
    fn resolve_guest_password_file_not_found_error_names_path_not_content() {
        let missing = std::path::Path::new("/nonexistent/meshcadet-test-pw-does-not-exist");
        let err = resolve_guest_password(None, Some(missing), None, false).unwrap_err();
        assert!(err.to_string().contains("password-file"));
    }

    #[test]
    fn password_truncation_warning_none_when_within_limit() {
        // `MAX_ROOM_PASSWORD_LEN` includes the wire NUL terminator
        // (`encode_anon_req_login`'s doc) — only `- 1` bytes ever transmit.
        let effective_limit = protocol::provisioning::MAX_ROOM_PASSWORD_LEN - 1;
        assert_eq!(password_truncation_warning(effective_limit), None);
        assert_eq!(password_truncation_warning(0), None);
    }

    /// REGRESSION (F6): a password of exactly `MAX_ROOM_PASSWORD_LEN` bytes
    /// (the wire constant) is one byte too long once the NUL terminator is
    /// accounted for — it must warn. Before this fix the warning boundary
    /// was `> MAX_ROOM_PASSWORD_LEN`, so this exact, common length (16, a
    /// round number a user is likely to pick) provisioned with no warning
    /// and silently operated as only its first 15 characters.
    #[test]
    fn password_truncation_warning_fires_at_exactly_the_wire_limit() {
        let limit = protocol::provisioning::MAX_ROOM_PASSWORD_LEN;
        let warning = password_truncation_warning(limit)
            .expect("a password of exactly MAX_ROOM_PASSWORD_LEN bytes must warn");
        assert!(warning.contains(&limit.to_string()));
    }

    #[test]
    fn password_truncation_warning_fires_over_limit_without_leaking_the_value() {
        let limit = protocol::provisioning::MAX_ROOM_PASSWORD_LEN;
        let effective_limit = limit - 1;
        let warning = password_truncation_warning(limit + 5).expect("must warn over the limit");
        assert!(warning.contains(&(limit + 5).to_string()));
        assert!(warning.contains(&effective_limit.to_string()));
        assert!(warning.contains("truncat"));
    }

    // ── Channel-secret resolution (add-channel / del-channel) ────────────────

    #[test]
    fn resolve_channel_secret_direct_flag() {
        let secret = resolve_channel_secret(Some("deadbeef"), None, None, false).unwrap();
        assert_eq!(secret, "deadbeef");
    }

    #[test]
    fn resolve_channel_secret_from_file_strips_trailing_newline() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("meshcadet-test-secret-{}.txt", std::process::id()));
        std::fs::write(&path, "abcdef0123456789\n").unwrap();
        let secret = resolve_channel_secret(None, Some(path.as_path()), None, false).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(secret, "abcdef0123456789");
    }

    #[test]
    fn resolve_channel_secret_from_env_var() {
        let var = format!("MESHCADET_TEST_SECRET_{}", std::process::id());
        std::env::set_var(&var, "cafebabe");
        let secret = resolve_channel_secret(None, None, Some(var.as_str()), false).unwrap();
        std::env::remove_var(&var);
        assert_eq!(secret, "cafebabe");
    }

    #[test]
    fn resolve_channel_secret_missing_env_var_errors() {
        let var = format!("MESHCADET_TEST_SECRET_MISSING_{}", std::process::id());
        std::env::remove_var(&var);
        let err = resolve_channel_secret(None, None, Some(var.as_str()), false).unwrap_err();
        assert!(err.to_string().contains(&var));
    }

    #[test]
    fn resolve_channel_secret_rejects_multiple_sources() {
        let err = resolve_channel_secret(
            Some("00112233445566778899aabbccddeeff"),
            None,
            Some("SOME_VAR"),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("at most one"));
        // The error message must name the flags, never a candidate value.
        assert!(!err.to_string().contains("00112233445566778899aabbccddeeff"));
    }

    #[test]
    fn resolve_channel_secret_file_not_found_error_names_path_not_content() {
        let missing = std::path::Path::new("/nonexistent/meshcadet-test-secret-does-not-exist");
        let err = resolve_channel_secret(None, Some(missing), None, false).unwrap_err();
        assert!(err.to_string().contains("secret-file"));
    }

    /// REGRESSION guard for the exact-match contract between `add-channel`
    /// and `del-channel` (B1 finding): the file/stdin sources must trim
    /// identically to the direct-flag path (i.e. only a trailing line
    /// ending, nothing else) so a secret round-tripped through
    /// `--secret-file` on `add-channel` still matches on `del-channel`.
    #[test]
    fn resolve_channel_secret_file_and_direct_flag_agree_after_trim() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "meshcadet-test-secret-roundtrip-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, "1234567890abcdef1234567890abcdef\n").unwrap();
        let from_file = resolve_channel_secret(None, Some(path.as_path()), None, false).unwrap();
        std::fs::remove_file(&path).ok();
        let from_flag =
            resolve_channel_secret(Some("1234567890abcdef1234567890abcdef"), None, None, false)
                .unwrap();
        assert_eq!(from_file, from_flag);
    }

    // ── Admin-PIN resolution (set-pin / reset-pin) ───────────────────────────

    #[test]
    fn resolve_admin_pin_direct_flag() {
        let pin = resolve_admin_pin(Some("1234"), None, None, false).unwrap();
        assert_eq!(pin, "1234");
    }

    #[test]
    fn resolve_admin_pin_from_file_strips_trailing_newline() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("meshcadet-test-pin-{}.txt", std::process::id()));
        std::fs::write(&path, "5678\n").unwrap();
        let pin = resolve_admin_pin(None, Some(path.as_path()), None, false).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(pin, "5678");
    }

    #[test]
    fn resolve_admin_pin_from_env_var() {
        let var = format!("MESHCADET_TEST_PIN_{}", std::process::id());
        std::env::set_var(&var, "9012");
        let pin = resolve_admin_pin(None, None, Some(var.as_str()), false).unwrap();
        std::env::remove_var(&var);
        assert_eq!(pin, "9012");
    }

    #[test]
    fn resolve_admin_pin_missing_env_var_errors() {
        let var = format!("MESHCADET_TEST_PIN_MISSING_{}", std::process::id());
        std::env::remove_var(&var);
        let err = resolve_admin_pin(None, None, Some(var.as_str()), false).unwrap_err();
        assert!(err.to_string().contains(&var));
    }

    #[test]
    fn resolve_admin_pin_rejects_multiple_sources() {
        let err = resolve_admin_pin(Some("13579"), None, Some("SOME_VAR"), false).unwrap_err();
        assert!(err.to_string().contains("at most one"));
        // The error message must name the flags, never a candidate value.
        assert!(!err.to_string().contains("13579"));
    }

    #[test]
    fn resolve_admin_pin_file_not_found_error_names_path_not_content() {
        let missing = std::path::Path::new("/nonexistent/meshcadet-test-pin-does-not-exist");
        let err = resolve_admin_pin(None, Some(missing), None, false).unwrap_err();
        assert!(err.to_string().contains("pin-file"));
    }

    // ── weak_secret_pattern_warning / gen-channel-secret entropy floor ───────

    #[test]
    fn weak_pattern_flags_all_zero() {
        let warning = weak_secret_pattern_warning(&[0u8; 32]);
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("gen-channel-secret"));
    }

    #[test]
    fn weak_pattern_flags_all_same_nonzero_byte() {
        let warning = weak_secret_pattern_warning(&[0xAAu8; 16]);
        assert!(warning.is_some());
    }

    #[test]
    fn weak_pattern_flags_ascending_sequence() {
        let bytes: Vec<u8> = (0..16).collect();
        assert!(weak_secret_pattern_warning(&bytes).is_some());
    }

    #[test]
    fn weak_pattern_flags_descending_sequence() {
        let bytes: Vec<u8> = (0..16).rev().collect();
        assert!(weak_secret_pattern_warning(&bytes).is_some());
    }

    #[test]
    fn weak_pattern_does_not_flag_high_entropy_bytes() {
        // A fixed, non-patterned byte string — not sequential, not a
        // repeated byte. Must NOT warn: this is the "don't block valid
        // high-entropy input" floor the mission scope calls out.
        let bytes: [u8; 16] = [
            0x4f, 0x1a, 0xc3, 0x08, 0x92, 0xe7, 0x15, 0x6b, 0xd4, 0x33, 0xa0, 0x5c, 0x77, 0x0e,
            0xf9, 0x21,
        ];
        assert!(weak_secret_pattern_warning(&bytes).is_none());
    }

    #[test]
    fn weak_pattern_does_not_flag_single_byte() {
        // Degenerate input too short to have a "pattern"; the real length
        // validation in `parse_channel_secret_hex` rejects this separately.
        assert!(weak_secret_pattern_warning(&[0u8]).is_none());
    }

    #[test]
    fn parse_channel_secret_hex_still_accepts_weak_patterns() {
        // The weak-pattern check is a warning, never a rejection — a
        // deliberately weak (or already-provisioned) secret must still
        // parse successfully.
        let (bytes, key_len) = parse_channel_secret_hex(&"00".repeat(32))
            .expect("all-zero secret must still parse (warning only, not a rejection)");
        assert_eq!(key_len, 32);
        assert_eq!(bytes, [0u8; 32]);
    }

    #[test]
    fn gen_channel_secret_128_and_256_round_trip_through_parse() {
        // `gen-channel-secret`'s hex output must compose directly with
        // `parse_channel_secret_hex` (same case, no prefix, exact length).
        for (bits, expected_key_len) in [(SecretBits::Bits128, 16u8), (SecretBits::Bits256, 32u8)] {
            let hex_out = generate_channel_secret_hex(bits);
            assert_eq!(hex_out.len(), bits.byte_len() * 2);
            assert!(hex_out
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

            let (_sec, key_len) =
                parse_channel_secret_hex(&hex_out).expect("gen-channel-secret output must parse");
            assert_eq!(key_len, expected_key_len);
        }
    }

    #[test]
    fn gen_channel_secret_two_calls_differ() {
        // Sanity that this is actually drawing from the RNG each call, not
        // returning a fixed/zeroed buffer.
        let a = generate_channel_secret_hex(SecretBits::Bits256);
        let b = generate_channel_secret_hex(SecretBits::Bits256);
        assert_ne!(a, b);
    }
}
