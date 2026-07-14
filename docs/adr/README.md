# Architecture Decision Records

| ADR | Title | Status |
|-----|-------|--------|
| [0001](0001-charter.md) | MeshCadet Charter | Accepted (2026-06-07) |
| [0002](0002-provisioning-wire-format.md) | Provisioning Wire Format | Accepted (2026-06-13) |
| [0003](0003-ui-toolkit.md) | UI Toolkit: Slint with SoftwareRenderer | Accepted (2026-06-13) |
| [0004](0004-release-architecture.md) | Release Architecture | Accepted (2026-07-11) |
| [0005](0005-firmware-core-extraction.md) | `firmware-core` Extraction | Accepted (2026-07-11) |
| [0006](0006-web-flasher.md) | Web Flasher: Version Selector Over a Same-Origin Mirror | Accepted (2026-07-11) |
| [0007](0007-provisioner-codec.md) | Web Provisioner Codec: Pure JS + Golden-Vector CI Guard | Accepted (2026-07-11) |
| [0008](0008-nondestructive-update-artifacts.md) | Non-Destructive Update Artifacts: App-Only Image + Layout Compatibility Gate | Accepted (2026-07-13) |
| [0009](0009-two-path-flasher.md) | Two-Path Web Flasher: Fresh Install vs Non-Destructive Upgrade | Accepted (2026-07-13) |
| [0010](0010-signal-meter.md) | Repeater Signal Meter: Hop-Gated RSSI, Max-With-Decay, Per-Screen Placement | Accepted (2026-07-13) |
| [0011](0011-unified-esptool-js-flasher.md) | Unified esptool-js Flasher: Fresh Install Drops `<esp-web-install-button>` | Accepted (2026-07-14) |

ADR-0001 is the project's founding charter: the design decisions made before
any code was written. Subsequent ADRs (UI toolkit choice, provisioning wire
format, storage layout, etc.) record later decisions as they were made.
