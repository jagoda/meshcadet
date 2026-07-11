# MeshCadet

A polished, deliberately-limited **MeshCore-interop firmware** for the
**LilyGo T-Deck Plus**, written in Rust (esp-idf / std, ESP32-S3 / Xtensa).

MeshCadet is a fully compliant [MeshCore](https://github.com/meshcore-dev/MeshCore)
citizen on the air — it interoperates byte-exact with an existing MeshCore mesh —
while layering a deliberately constrained policy on top: allowlist-only contacts,
no advertising or discovery, pull-only telemetry, and a curated emoji set. All of
that constrained behavior lives in a policy + UI layer on top of a byte-exact
protocol port, so the device never has to fork or weaken the underlying protocol
to apply those limits. These are deliberate design choices for a controlled,
minimal comms device — not anything the underlying protocol requires. Whether
they suit any given use is for you to decide; see the
[Disclaimer](#-disclaimer--no-warranty-no-guarantee-of-safety-use-at-your-own-risk)
below.

> **Origin.** MeshCadet grew out of wanting a simple, controlled way to stay in
> touch with my kids over a MeshCore mesh.

The full design rationale is recorded in
**[`docs/adr/0001-charter.md`](docs/adr/0001-charter.md)** and the two follow-on
ADRs in [`docs/adr/`](docs/adr/).

## ⚠️ Disclaimer — no warranty, no guarantee of safety, use at your own risk

**Read this before you build, flash, or rely on MeshCadet.**

- **Provided "AS IS" and "AS AVAILABLE", with all faults.** To the maximum
  extent permitted by applicable law, the author and contributors disclaim
  **all** warranties, express or implied, including but not limited to the
  implied warranties of merchantability, fitness for a particular purpose,
  title, and non-infringement, and **specifically** any warranty of safety,
  security, reliability, or suitability for any particular use.
- **No guarantee that any feature works.** The limits documented here —
  allowlist-only contacts, no advertising, pull-only telemetry, the PIN-gated
  admin menu, and the rest — are **best-effort design choices, not
  guarantees.** Nothing in this project guarantees that any feature works, is
  effective, is free of defects, or will prevent any contact, exposure, or
  other risk. The software may contain defects and **may fail**, silently or
  otherwise. A constrained design does not make a device safe.
- **You assume all risk.** By downloading, building, flashing, using,
  conveying, or modifying MeshCadet, you accept **full responsibility** for
  that decision and assume **all risk** arising from it.
- **No support, maintenance, updates, or fixes.** This is an independent,
  volunteer project. The author and contributors have **no obligation** to
  provide support, to maintain the software, to release updates, or to
  investigate, patch, or fix any defect or vulnerability, and make **no**
  service-level commitment of any kind.
- **No liability.** To the maximum extent permitted by applicable law, in no
  event will the author or any contributor be liable for any direct, indirect,
  incidental, special, exemplary, or consequential damages, or for any injury,
  harm, or loss of any kind, arising out of or in connection with MeshCadet or
  its use — even if advised of the possibility of such damages.
- **Indemnification.** By using, conveying, or modifying MeshCadet, you agree
  to indemnify, defend, and hold harmless the author and all contributors from
  and against any and all claims, demands, damages, losses, liabilities,
  costs, and expenses (including reasonable legal fees) arising out of or
  related to your use, conveyance, or modification of the software.
- **RF / regulatory compliance.** This firmware transmits on LoRa in the
  ISM band. **You are solely responsible for operating this device in
  compliance with the RF regulations of your jurisdiction** (e.g. FCC Part 15
  / CE / regional ISM-band duty-cycle and spectrum rules), including
  frequency, power, and duty-cycle limits. The author and contributors make no
  representation that any particular configuration or build is compliant with
  any regulatory regime.

MeshCadet is licensed under **GPLv3**, whose sections 15 and 16 already
disclaim warranty and limit liability. The assumption-of-risk and
indemnification terms above are offered as **additional terms under GPLv3
section 7** — which expressly permits supplementing the license with further
disclaimers of warranty and limitations of liability, and with a requirement
that recipients indemnify the licensor and authors — and are meant to be read
consistently with, not in contradiction of, the license. Where any conflict is
found, the GPLv3 governs the license grant itself. This summary is for readers;
the controlling license text is in [`LICENSE`](LICENSE), and the security
posture is detailed in [`SECURITY.md`](SECURITY.md).

## What it does

- Talks real MeshCore v1.15.0 over LoRa (SX1262): DMs, channel messages, ACKs,
  and pull-only location telemetry.
- Touch-screen UI (Slint) for contacts, conversations, and composing messages
  with a curated `:shortcode:` emoji set.
- Allowlist-only: an admin provisions every contact and channel over USB; the
  device never auto-discovers or auto-adds anyone, and never advertises itself
  on the mesh.
- On-device PIN-gated admin menu for lightweight runtime toggles, plus a USB
  host CLI for provisioning, PIN reset, and history export.

See [`docs/adr/0001-charter.md`](docs/adr/0001-charter.md) for the complete
behavioral contract.

## Status and known limitations

Functional and interop-tested against real MeshCore hardware for the core
DM / ACK / channel-message / pull-telemetry paths. Before relying on it:

- **No at-rest encryption.** Provisioned identity, contacts, and channel keys
  are stored unencrypted in device flash (no flash encryption / secure boot).
  A lost or stolen device should be treated as having disclosed its keys;
  rotate channels/keys on the rest of the mesh if that happens. See
  [`SECURITY.md`](SECURITY.md) for the full threat model.
- **Single-device project.** MeshCadet has been built and tested one T-Deck
  Plus at a time; multi-device fleets are not a tested configuration.
- **No CI yet.** Tests are run locally; there is no automated build/test
  pipeline in this repository yet.

## Hardware prerequisites

- A **[LilyGo T-Deck Plus](https://github.com/Xinyuan-LilyGO/T-Deck)**
  (ESP32-S3, SX1262 LoRa radio, touch display, physical keyboard, GPS module).
- A USB-C cable capable of data transfer (not charge-only) — used both to
  flash the board and to run the host provisioning CLI.
- At least one other MeshCore-speaking node (another MeshCadet, or a stock
  MeshCore device/companion app) to talk to — MeshCadet does not create a mesh
  by itself.

## Repo layout

| Crate | Role | Builds with |
|-------|------|-------------|
| `protocol/` | MeshCore v1.15 wire port (framing, crypto, codec); shared | stable, host-native |
| `firmware-core/` | Decoupled, host-testable half of firmware logic (dispatcher, PIN-menu, notifications, GPS/battery/runtime-settings parsing+codecs) — no `esp-idf-*`/Slint dep; also a `path` dep of `firmware/` (see ADR-0005) | stable, host-native |
| `firmware/` | T-Deck device app (radio, GPS, touch UI, storage, admin menu) | `esp` toolchain, `xtensa-esp32s3-espidf` |
| `host/` | Admin CLI (`meshcadet`): USB provisioning, history export, PIN reset | stable, host-native |
| `xtask/` | Host-side glyph-coverage verification harness | stable, host-native |
| `ui_sim/` | Host-sim render rig for the Slint image-asset pipeline + shared motif library (see `ui_sim/README.md`) | stable, host-native |
| `ui_perf/` | Host-native UI performance measurement harness (see `ui_perf/README.md`) | stable, host-native |

The firmware crate is a **detached** Cargo workspace (it declares its own
`[workspace]` table) so it can cross-compile for ESP32-S3/Xtensa without
pulling the `esp` toolchain into the root workspace's `cargo build`/`cargo
test`. Toolchain layout is recorded in [ADR-0001](docs/adr/0001-charter.md).

## Building from a fresh clone

### 1. Host-native crates (`protocol`, `firmware-core`, `host`, `xtask`, `ui_sim`, `ui_perf`)

These build and test on stable Rust with no special toolchain:

```sh
git clone <this-repo-url>
cd meshcadet
cargo test --workspace
```

`rust-toolchain.toml` at the repo root pins stable Rust automatically via
`rustup`; no manual toolchain step is needed for this part.

### 2. Firmware (flashing a T-Deck Plus)

The firmware crate cross-compiles for `xtensa-esp32s3-espidf` and needs the
Espressif Rust toolchain plus a couple of small helper tools.

The firmware build also generates its bundled emoji font at build time
(`firmware/build.rs`), which requires some native, non-Rust prerequisites: a C
compiler, `pkg-config`, the FreeType development headers, and the DejaVu Sans
system font. On Debian/Ubuntu:

```sh
sudo apt install build-essential pkg-config libfreetype6-dev fonts-dejavu-core
```

On other distros, install the equivalent packages for a C toolchain,
`pkg-config`, FreeType's development headers (`freetype2` `pkg-config`
module), and a DejaVu Sans TTF available at
`/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf`.

```sh
# One-time toolchain setup
cargo install espup --locked
espup install
# Source the ESP environment in every NEW shell before building firmware:
. "$HOME/export-esp.sh"

# Tools the firmware build/flash needs
cargo install ldproxy espflash --locked
```

`firmware/rust-toolchain.toml` pins the `esp` channel automatically once
`espup install` has installed it, so `rustup` selects it whenever your working
directory is inside `firmware/`.

Then, with the T-Deck Plus connected over USB:

```sh
cd firmware
cargo run --release
```

`cargo run` builds, flashes, and attaches a serial monitor in one step — the
crate's `.cargo/config.toml` wires a custom flash runner
(`scripts/flash-with-partition-table.sh`) that also lands the project's custom
flash-partition layout (needed for on-device message history). If `espflash`
can't auto-detect the serial port, pass it explicitly:

```sh
cargo run --release -- --port /dev/ttyACM0   # or COMx on Windows, /dev/cu.usbmodem* on macOS
```

At first boot an unprovisioned device shows a "connect me to an admin over
USB" screen and accepts no messages until provisioned (see below).

### 3. Provisioning a device (the admin CLI)

The `host` crate provides `meshcadet`, a USB-serial CLI an admin uses to set
up a device — register contacts and channels, set notification defaults and a
PIN, and later export history or reset a forgotten PIN. It's part of the
host-native workspace, so it builds and runs from the repo root:

```sh
cargo run -p host -- --port /dev/ttyACM0 status
cargo run -p host -- --port /dev/ttyACM0 identity
cargo run -p host -- --port /dev/ttyACM0 add-contact --pubkey <HEX64> --name "Alice" --telemetry
cargo run -p host -- --port /dev/ttyACM0 add-channel --secret <HEX64> --name "family" --primary
cargo run -p host -- --port /dev/ttyACM0 commit
```

Run `cargo run -p host -- --help` for the full command list. Physical USB
possession of the device is the sole authentication factor for provisioning
(see [ADR-0001 §4](docs/adr/0001-charter.md)).

## Dependencies

Notable direct dependencies (see `Cargo.lock` for the full resolved graph):

- [Slint](https://slint.dev/) — the touch-screen UI toolkit (software
  renderer, no GPU). See [ADR-0003](docs/adr/0003-ui-toolkit.md) for why.
- `esp-idf-svc` / `esp-idf-hal` — ESP-IDF (std) bindings used by the firmware.
- `mipidsi` / `embedded-graphics` — the ST7789 display driver stack.
- `clap` — the host CLI's argument parsing.

A full third-party dependency license audit is at
[`docs/licensing/DEPENDENCY-AUDIT.md`](docs/licensing/DEPENDENCY-AUDIT.md),
with the complete resolved license list in
[`docs/licensing/THIRD-PARTY-LICENSES.md`](docs/licensing/THIRD-PARTY-LICENSES.md).

## License

GPLv3 — see [`LICENSE`](LICENSE). Chosen because `firmware/` reaches into
`i-slint-core` internals (`RendererSealed` et al.), and GPL-3.0-only is the
license Slint offers for that with no separate royalty/registration agreement.
Upstream attribution (MeshCore, RadioLib): [`NOTICE`](NOTICE).

MeshCore, MeshCadet, T-Deck, LILYGO, ESP32, Espressif, LoRa, and Semtech are
trademarks of their respective owners; use here is nominative and implies no
affiliation or endorsement.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for how to build, test, and submit
changes.

## Security

See [`SECURITY.md`](SECURITY.md) for the threat model, known limitations, and
how to report a vulnerability.
