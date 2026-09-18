// SPDX-License-Identifier: GPL-3.0-only
//! Device-verification probe for the `ADD_CHANNEL` `key_len` hardening fix
//! (see `docs/adr/0002-provisioning-wire-format.md`'s 2026-09-17 amendment).
//!
//! `meshcadet add-channel` (the CLI) and the web provisioner's "Add channel"
//! form can never themselves produce an out-of-range `key_len` byte — both
//! only ever offer 128-bit or 256-bit secrets. That means the wire-level
//! defect fixed here (`protocol::decode_add_channel` used to accept
//! ANY `key_len` byte, including values that later panicked the firmware's
//! `channel_hash_var(&secret[..key_len])` call with an out-of-bounds slice —
//! an index-out-of-bounds panic on the firmware's own thread, which the
//! ESP-IDF panic handler turns into a device reset) was never reachable
//! through either normal client's UI. It's fully proven host-side (see
//! `protocol::provisioning::tests::decode_add_channel_rejects_invalid_key_len`
//! and `host`'s own `test_add_channel_rejects_invalid_key_len`), but neither
//! of those runs against REAL firmware — this example does, by calling
//! `Session::add_channel` directly with a deliberately invalid `key_len`,
//! which that method (unlike the CLI's argument parser) does not itself
//! validate.
//!
//! See `docs/provisioning-connect-verification-kit.md` step 4 for how to run
//! this and what each outcome means.
//!
//! Usage:
//! ```sh
//! cargo run -p host --example raw_add_channel_bad_key_len -- --port /dev/ttyACM0
//! ```

use host::session::Session;
use host::transport::SerialTransport;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut port: Option<String> = None;
    while let Some(arg) = args.next() {
        if arg == "--port" {
            port = args.next();
        }
    }
    let port = port.ok_or_else(|| anyhow::anyhow!("usage: --port <PATH>, e.g. /dev/ttyACM0"))?;

    println!("raw_add_channel_bad_key_len: opening {port}…");
    let transport = SerialTransport::open(&port, 115_200)?;
    let mut session = Session::new(transport);

    let secret = [0x11u8; 32];
    for bad_key_len in [0u8, 33, 200, 255] {
        print!("sending ADD_CHANNEL with key_len={bad_key_len} … ");
        match session.add_channel(&secret, bad_key_len, false, b"bad-key-len-probe") {
            Ok(()) => {
                println!("UNEXPECTED: device accepted it (RSP_OK) — the fix did not take.");
                return Err(anyhow::anyhow!(
                    "device accepted an invalid key_len={bad_key_len}"
                ));
            }
            Err(e) => println!("rejected cleanly: {e}"),
        }
    }

    println!(
        "\nAll {} invalid key_len values were rejected with a clean device error — \
         no reboot, no hang. Paste the full output (including any device reset you \
         observed independently, e.g. on the serial monitor or the device's own \
         screen) into the verification kit's result block.",
        4
    );
    Ok(())
}
