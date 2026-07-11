# Security Policy

MeshCadet is radio-facing firmware for a deliberately-limited communication
device. This
document describes the threat model, documents known limitations up front
(so they're found here, not "discovered" by someone else), and explains how
to report a vulnerability.

## Reporting a vulnerability

**Do not open a public GitHub issue for a security vulnerability.**

Email **jeff@jagoda.me** with a description of the issue, steps to reproduce,
and its potential impact. Please include "MeshCadet security" in the subject
line.

Reports are read on a **best-effort, discretionary basis**. Please understand
what this reporting channel is and is not:

- **No guarantee of acknowledgment, response, investigation, or fix, and no
  timeline of any kind.** Providing this email address is a courtesy to enable
  responsible disclosure; it is **not** a commitment to reply, to triage, to
  investigate, or to remediate any report, and there is **no** service-level
  agreement, response-time target, or support obligation, express or implied.
  The author may act on a report, or not, at their sole discretion.
- If you do plan public disclosure, giving advance notice is appreciated as a
  courtesy, but nothing here obligates the author to act within any window
  before you disclose.
- This is a small, independently maintained, volunteer project — there is no
  bug bounty. Reporters may, at the author's discretion and with permission,
  be credited if and when a related change is released.

## Threat model

### Assets being protected

- The confidentiality of message content (DMs and channel messages) in
  transit over the mesh.
- The integrity/authenticity of messages, so the device only receives messages
  that actually came from an allowlisted contact or channel.
- The allowlist policy itself. The **design goal** is that unapproved
  contacts cannot reach the device and that the device is not
  discoverable / addable by strangers on the mesh. This is the objective the
  allowlist policy is built toward — it is a best-effort risk-reduction goal,
  **not a guarantee**; see the [Disclaimer](#disclaimer--no-warranty-and-no-guarantee-of-safety)
  below.

### Trust boundaries

- **The admin, via physical USB possession, is fully trusted.** Provisioning
  (adding contacts/channels, setting the PIN, changing notification
  defaults) requires a USB-serial connection to the device — see
  [ADR-0001 §4](docs/adr/0001-charter.md). There is no remote/wireless
  provisioning path. Anyone with physical USB access to an unlocked device
  and the host CLI has full admin authority over it; this is a deliberate
  design choice (a lost/stolen device is a physical-security problem, not a
  remote one), not an oversight.
- **The mesh itself is untrusted.** MeshCadet assumes any node on the air —
  allowlisted or not — may be hostile, and relies on MeshCore's own
  cryptography (Ed25519 identity, ECDH-derived shared secrets, AES-128 +
  HMAC-SHA256) plus MeshCadet's allowlist policy for protection. See
  [ADR-0001 §1](docs/adr/0001-charter.md) for the exact primitives, which are
  fixed by the need for byte-exact interop with MeshCore v1.15 — this project
  cannot unilaterally change them without breaking interop.
- **Non-allowlisted traffic is silently dropped**, by design: DMs and
  telemetry requests from unknown senders get no ACK and no reply, so a
  scanning adversary cannot distinguish "device offline" from "you're not on
  the allowlist" (see ADR-0001 §2).

### Out of scope

- Attacks that require compromising the admin's own device/credentials, or
  attacks on the physical security of the device itself (see "Known
  limitations" below for what physical possession of the device grants).
- Vulnerabilities in the upstream MeshCore protocol or in third-party
  dependencies — please report those to the respective upstream project
  ([MeshCore](https://github.com/meshcore-dev/MeshCore),
  [RadioLib](https://github.com/jgromes/RadioLib), etc.), though a report
  here noting that MeshCadet inherits the issue is also welcome so it can be
  tracked and addressed on this project's side.

## Known limitations

These are accepted, documented trade-offs — not surprises for anyone
auditing this codebase:

- **No at-rest encryption.** Provisioned identity keys, contact/channel
  configuration, and message history are stored unencrypted in device flash
  (no flash encryption, no secure boot). A device that is lost or stolen
  should be treated as having disclosed everything provisioned on it;
  mitigation is operational — rotate the affected channel keys / re-register
  contacts on the rest of the mesh. See
  [ADR-0001, "Consequences"](docs/adr/0001-charter.md).
- **No PIN attempt lockout.** The on-device admin-menu PIN check
  (`firmware/src/pin_menu.rs`) is a constant-time comparison (protects
  against timing side-channels) but does not rate-limit or lock out repeated
  incorrect attempts. Brute-forcing a short PIN via the on-screen keypad is
  slow but not formally prevented.
- **Inherited protocol-level limitation: AES-128-ECB.** MeshCore v1.15 (which
  MeshCadet ports byte-exact for interop) encrypts DM/channel payloads with
  AES-128 in **ECB mode**, not a mode with a per-message nonce/IV (see
  [ADR-0001 §1](docs/adr/0001-charter.md), "Discrepancy on record"). ECB
  leaks whether two ciphertext blocks encode identical plaintext. This is a
  property of the MeshCore wire protocol itself, not a MeshCadet-specific
  weakness, and MeshCadet cannot change it without breaking interop with the
  rest of the mesh.
- **Physical USB possession is the sole provisioning authentication
  factor.** By design (see "Trust boundaries" above) — there is no
  secondary factor (password, hardware key, etc.) gating the host CLI's
  provisioning commands beyond having a cable plugged into the device.
- **No forward secrecy beyond what MeshCore itself provides.** Channel and
  DM key material is long-lived once provisioned; compromise of a channel
  secret compromises all messages encrypted under it, past and future, until
  it is rotated.

None of the example values in this repository's documentation, tests, or
source are real cryptographic material — provisioning examples in the
[README](README.md) use placeholder `<HEX64>` values, and in-tree test
fixtures use obvious dummy keys (e.g. `[0x6Du8; 32]` — `'m'` repeated, an
intentionally-recognizable non-random test constant, see
`protocol/src/codec.rs`), never real device identities, peer public keys, or
channel secrets. The `hil_keys.example.rs` template's committed placeholder
seed is an explicit all-zero non-identity (`[0u8; 32]`).

## Versions and maintenance

This project has **no** formal release, support, or maintenance cadence, and
none is promised. The author is under **no obligation** to investigate, fix,
patch, update, maintain, or support any version of MeshCadet, whether the
current default branch or any past or future revision. There is **no**
supported-version list and **no** guarantee that any security issue — known or
unknown — will ever be addressed. Any fix that does happen lands on the default
branch at the author's sole discretion; there is no maintained older-version
branch, and no backports are promised.

## Disclaimer — no warranty and no guarantee of safety

**MeshCadet is provided "AS IS" and "AS AVAILABLE", with all faults and
without warranty of any kind.** This applies with full force to everything in
this security document: the threat model, the asset list, and the known
limitations describe *design intent and best-effort risk reduction*, **not**
guarantees.

- **No warranty.** To the maximum extent permitted by applicable law, the
  author and contributors disclaim **all** warranties, express or implied,
  including the implied warranties of merchantability, fitness for a particular
  purpose, title, and non-infringement, and **specifically** any warranty of
  safety, security, reliability, or suitability for any particular use.
- **No guarantee of safety or security.** Nothing in this project guarantees
  that any policy or security feature — the allowlist, silent drop of
  non-allowlisted traffic, the PIN gate, the crypto, or any other measure —
  works, is effective, is free of defects, or protects any person or any data.
  The software may contain defects and **may fail**. A documented threat model
  does not mean the device is safe.
- **You assume all risk.** By downloading, building, flashing, using,
  conveying, or modifying MeshCadet, you assume **all risk** arising from that
  decision.
- **No support obligation.** As stated under "Versions and maintenance" above,
  there is no obligation to acknowledge reports, investigate, patch, update,
  maintain, or support any version, and no service-level commitment of any
  kind.
- **No liability.** To the maximum extent permitted by applicable law, in no
  event will the author or any contributor be liable for any damages, injury,
  harm, or loss of any kind arising out of or in connection with MeshCadet or
  its use.
- **Indemnification.** By using, conveying, or modifying MeshCadet, you agree
  to indemnify, defend, and hold harmless the author and all contributors from
  and against any and all claims, demands, damages, losses, liabilities, costs,
  and expenses (including reasonable legal fees) arising out of or related to
  your use, conveyance, or modification of the software.
- **RF / regulatory compliance.** This firmware transmits on LoRa in the
  ISM band. **You are solely responsible for operating this device in
  compliance with the RF regulations of your jurisdiction** (e.g. FCC Part 15
  / CE / regional ISM-band duty-cycle and spectrum rules), including
  frequency, power, and duty-cycle limits. The author and contributors make no
  representation that any particular configuration or build is compliant with
  any regulatory regime.

MeshCadet is licensed under **GPLv3**; its sections 15 and 16 already disclaim
warranty and limit liability, and the assumption-of-risk and indemnification
terms here are offered as **additional terms under GPLv3 section 7** (which
permits supplementing the license with further warranty/liability disclaimers
and an indemnification requirement). They are meant to be read consistently
with — not in contradiction of — the license; the controlling text is in
[`LICENSE`](LICENSE). See also the
[Disclaimer in the README](README.md#-disclaimer--no-warranty-no-guarantee-of-safety-use-at-your-own-risk).
