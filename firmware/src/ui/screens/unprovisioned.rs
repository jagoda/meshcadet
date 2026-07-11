// SPDX-License-Identifier: GPL-3.0-only
//! First-boot unprovisioned screen.
//!
//! Shown when the device has never been provisioned.  Nothing can be done on
//! the device here — the admin must connect via USB and run the host CLI.
//!
//! UI elements:
//! - A starfield header strip + corner ringed-planet accent (baked celestial
//!   scenery, RGB565 bitmaps) behind everything else.
//! - The "Cadet" mascot (idle pose) as the screen's hero bitmap, with a
//!   one-shot bob-in on mount.
//! - "MeshCadet" wordmark
//! - Subtitle: "Ask an admin to connect via USB to set up."
//! - The device's Ed25519 public key (hex, small font) — the admin needs
//!   this to register the device in their contacts.
//! - A one-shot glow-in on the logo border (see below — this used to be a
//!   dead infinite pulse).
//! - With `--features diagnostics`: live stdin RX byte counter.
//!
//! # Space asset pipeline pilot
//!
//! This screen is the walking-skeleton PILOT for this UI's
//! outer-space theme's image-asset pipeline: the
//! three bitmaps below (`cadet_idle.png`, `starfield.png`,
//! `planet_corner.png` — all under `firmware/assets/space/`, regenerated
//! reproducibly by that directory's `generate_assets.py`) are embedded via
//! Slint's `Image` + `@image-url(...)`, compiled to raw pixel data at build
//! time by `SLINT_EMBED_RESOURCES=embed-for-software-renderer`
//! (`firmware/.cargo/config.toml`'s `[env]`) — the PRIMARY path from the
//! design plan's asset-architecture options table. This is the mechanism
//! this pilot set out to de-risk before the motif-library + 7-screen
//! fan-out; the build.rs-generated RGB565 byte-array + runtime
//! `SharedPixelBuffer` FALLBACK path is proven separately in the host-native
//! `ui_sim` crate (repo root) rather than shipped here, since the primary
//! path built and rendered correctly in that same host-sim harness — see
//! `ui_sim/README.md` for the fallback exercise and the host-sim render this
//! screen delivers as evidence.
//!
//! # Theme tokens + one-shot animation language
//!
//! Every color/font-size literal in both `slint::slint!{}` blocks below (the
//! production variant and the `--features diagnostics` variant) now reads
//! from the shared `Theme` global (`ui/theme.slint`, imported below), same
//! pattern as every other themed screen. This screen also opts into the
//! widened space palette's `Theme.space-deep` backdrop (additive token; see
//! `ui/theme.slint`) as the deeper-than-`bg-space` canvas the baked starfield
//! sits on. Deliberate, documented deviations from a bare pixel-identical
//! swap — pre-existing, named defects earlier themeing/space work was
//! scoped to resolve alongside each re-skin:
//!
//! 1. **`glow_opacity` was dead code — fixed, not just re-skinned.** The
//!    pre-theme markup declared `glow_opacity` with `animate ... {
//!    iteration-count: -1; }`, which reads as "pulse forever", but
//!    `glow_opacity` was never mutated anywhere in this file — a Slint
//!    `animate` block only fires on a VALUE CHANGE (`splash.rs`'s module doc
//!    already calls out this exact property, by name, as the cautionary
//!    example of the trap), so the border sat at its static 0.4-alpha
//!    default forever; the "pulsing glow" never actually pulsed. Per this
//!    UI's animation-language rule ("never an infinite loop that a
//!    screen swap can cut off mid-cycle"), reinstating an infinite pulse
//!    isn't the right fix even once wired up correctly. Instead this now
//!    uses the same self-contained `init =>` one-shot mechanism every other
//!    themed screen uses: `glow_opacity` starts hidden (`0.0`); `init`
//!    writes it to the old static settled value (`0.4`), which is the value
//!    change that fires the `animate` transition. The border now glows in
//!    once when the screen mounts and then holds at exactly the alpha it
//!    silently sat at before (same rest-state look) — a REAL, finite
//!    animation instead of inert dead code. `reveal_opacity` is the separate,
//!    standard whole-screen entry fade every other themed screen already has
//!    (`UiRuntime::dismiss_splash` builds this screen fresh exactly once, at
//!    splash dismissal, so the same "init fires once per mount" reasoning
//!    other screens rely on applies here too — no Rust wiring needed for
//!    either property).
//! 2. **`📻` is retired outright, not just resized.** An earlier fix had
//!    already dropped it from 28px to `Theme.icon-lg` (20px) to dodge
//!    `gen_emoji_font.c`'s documented `EMOJI_SIZES` gap (28px was never
//!    registered for emoji, so it rasterised BLANK on real hardware). This
//!    change retires the glyph entirely instead of merely resizing it: the
//!    hero visual is now the `cadet_idle.png` RGB565 bitmap (see "Space asset
//!    pipeline pilot" above), so there is no emoji glyph at any size in this
//!    screen at all — not a font-size question anymore, a bitmap-vs-glyph
//!    one. The wordmark keeps its prior 28px via `Theme.size-hero` (plain
//!    Latin text, safe per `PIXEL_SIZES`, no emoji-blank risk). `xtask`'s
//!    `KNOWN_GAPS` allowlist entry for this file is now vacuously unused
//!    (the codepoint it names no longer appears here) rather than removed —
//!    left alone per the zero-shared-file-contention invariant this codebase
//!    maintains (`gen_emoji_font.c` and `xtask` are untouched by this change).

// Production build: no diagnostic counter property or UI element.
#[cfg(not(feature = "diagnostics"))]
slint::slint! {
    import { Theme } from "../theme.slint";

    export component UnprovisionedScreenUi inherits Window {
        // ── Layout ──────────────────────────────────────────────────────────
        width: 320px;
        height: 240px;
        background: Theme.space-deep; // widened-palette opt-in — see module doc

        // One-shot logo-border glow-in — see module doc point 1. Starts
        // hidden; `init` below writes it to its old static settled alpha,
        // which is the value change that fires this `animate`.
        in-out property <float> glow_opacity: 0.0;
        animate glow_opacity {
            duration: 1200ms;
            easing: ease-in-out;
        }

        // Whole-screen one-shot entry fade — same pattern as every other
        // themed screen (see module doc point 1).
        in-out property <float> reveal_opacity: 0;
        animate reveal_opacity { duration: 200ms; easing: ease-out; }

        // One-shot "mascot-bob" (this UI's motion-language vocabulary): the
        // Cadet hero bitmap settles in from a small upward offset instead of
        // popping in flat-footed. Same init-driven value-change mechanism as
        // glow_opacity/reveal_opacity above — never an infinite loop.
        in-out property <length> mascot_bob_y: -10px;
        animate mascot_bob_y { duration: 450ms; easing: ease-out; }

        init => {
            self.reveal_opacity = 1.0;
            self.glow_opacity = 0.4;
            self.mascot_bob_y = 0px;
        }

        // Properties set from Rust
        in property <string> pubkey_hex;

        // ── Baked celestial scenery (RGB565 bitmaps, primary embed path:
        //    Image + @image-url, compiled by
        //    SLINT_EMBED_RESOURCES=embed-for-software-renderer — see module
        //    doc). Declared first so they paint BEHIND the foreground
        //    VerticalLayout (Slint z-orders by declaration order). ─────────
        Image {
            source: @image-url("../../../assets/space/starfield.png");
            x: 0px;
            y: 0px;
            width: 320px;
            height: 40px;
        }
        Image {
            source: @image-url("../../../assets/space/planet_corner.png");
            x: 320px - 40px - 8px;
            y: 8px;
            width: 40px;
            height: 40px;
        }

        VerticalLayout {
            width: 100%;
            height: 100%;
            alignment: center;
            padding-top: 12px;
            spacing: 6px;
            opacity: reveal_opacity;

            // Cadet mascot — hero bitmap. Retires the old 📻 glyph outright
            // (see module doc point 2): the hero visual is a bitmap, not a
            // Text glyph run.
            Rectangle {
                height: 64px;
                Image {
                    source: @image-url("../../../assets/space/cadet_idle.png");
                    width: 64px;
                    height: 64px;
                    y: mascot_bob_y;
                }
            }

            // Wordmark
            Rectangle {
                height: 40px;
                background: transparent;
                border-radius: 12px;
                border-width: 2px;
                border-color: Theme.brand-signal.with-alpha(glow_opacity);

                Text {
                    text: "MeshCadet";
                    font-size: Theme.size-hero; // 28px — see module doc
                    font-weight: 700;
                    color: Theme.brand-signal;
                    horizontal-alignment: center;
                    vertical-alignment: center;
                }
            }

            // Instruction
            Text {
                text: "Ask an admin to connect\nvia USB to set up.";
                font-size: Theme.size-body-lg; // 14px
                color: Theme.text-secondary;
                horizontal-alignment: center;
                wrap: word-wrap;
            }

            // Spacer
            Rectangle { height: 4px; }

            // Public key (admin needs this to add device as a contact)
            Text {
                text: "Device key:";
                font-size: Theme.size-meta; // 10px
                color: Theme.text-muted;
                horizontal-alignment: center;
            }
            Text {
                text: pubkey_hex;
                font-size: Theme.size-badge; // 8px
                color: Theme.text-muted; // folds the old #404850 — see ui/theme.slint
                horizontal-alignment: center;
                wrap: word-wrap;
            }
        }
    }
}

// Diagnostics build: adds live RX byte counter (--features diagnostics).
#[cfg(feature = "diagnostics")]
slint::slint! {
    import { Theme } from "../theme.slint";

    export component UnprovisionedScreenUi inherits Window {
        // ── Layout ──────────────────────────────────────────────────────────
        width: 320px;
        height: 240px;
        background: Theme.space-deep; // widened-palette opt-in — see module doc

        // One-shot logo-border glow-in — see module doc point 1. Starts
        // hidden; `init` below writes it to its old static settled alpha,
        // which is the value change that fires this `animate`.
        in-out property <float> glow_opacity: 0.0;
        animate glow_opacity {
            duration: 1200ms;
            easing: ease-in-out;
        }

        // Whole-screen one-shot entry fade — same pattern as every other
        // themed screen (see module doc point 1).
        in-out property <float> reveal_opacity: 0;
        animate reveal_opacity { duration: 200ms; easing: ease-out; }

        // One-shot "mascot-bob" — see the production variant above for the
        // full rationale; same init-driven value-change mechanism.
        in-out property <length> mascot_bob_y: -10px;
        animate mascot_bob_y { duration: 450ms; easing: ease-out; }

        init => {
            self.reveal_opacity = 1.0;
            self.glow_opacity = 0.4;
            self.mascot_bob_y = 0px;
        }

        // Properties set from Rust
        in property <string> pubkey_hex;

        // Cumulative stdin RX byte counter (diagnostics build only).
        in property <string> rx_bytes_str: "RX: — bytes";

        // ── Baked celestial scenery — see production variant above ─────────
        Image {
            source: @image-url("../../../assets/space/starfield.png");
            x: 0px;
            y: 0px;
            width: 320px;
            height: 40px;
        }
        Image {
            source: @image-url("../../../assets/space/planet_corner.png");
            x: 320px - 40px - 8px;
            y: 8px;
            width: 40px;
            height: 40px;
        }

        VerticalLayout {
            width: 100%;
            height: 100%;
            alignment: center;
            padding-top: 12px;
            spacing: 6px;
            opacity: reveal_opacity;

            // Cadet mascot — hero bitmap. See production variant above.
            Rectangle {
                height: 64px;
                Image {
                    source: @image-url("../../../assets/space/cadet_idle.png");
                    width: 64px;
                    height: 64px;
                    y: mascot_bob_y;
                }
            }

            // Wordmark
            Rectangle {
                height: 40px;
                background: transparent;
                border-radius: 12px;
                border-width: 2px;
                border-color: Theme.brand-signal.with-alpha(glow_opacity);

                Text {
                    text: "MeshCadet";
                    font-size: Theme.size-hero; // 28px — see module doc
                    font-weight: 700;
                    color: Theme.brand-signal;
                    horizontal-alignment: center;
                    vertical-alignment: center;
                }
            }

            // Instruction
            Text {
                text: "Ask an admin to connect\nvia USB to set up.";
                font-size: Theme.size-body-lg; // 14px
                color: Theme.text-secondary;
                horizontal-alignment: center;
                wrap: word-wrap;
            }

            // Spacer
            Rectangle { height: 4px; }

            // Public key (admin needs this to add device as a contact)
            Text {
                text: "Device key:";
                font-size: Theme.size-meta; // 10px
                color: Theme.text-muted;
                horizontal-alignment: center;
            }
            Text {
                text: pubkey_hex;
                font-size: Theme.size-badge; // 8px
                color: Theme.text-muted; // folds the old #404850 — see ui/theme.slint
                horizontal-alignment: center;
                wrap: word-wrap;
            }

            // Live RX counter — visible only in diagnostics builds.
            Text {
                text: rx_bytes_str;
                font-size: Theme.size-caption; // 9px
                color: Theme.ok;
                horizontal-alignment: center;
            }
        }
    }
}

/// Rust-side wrapper for the unprovisioned screen component.
pub struct UnprovisionedScreen {
    component: self::UnprovisionedScreenUi,
}

impl UnprovisionedScreen {
    pub fn new() -> anyhow::Result<Self> {
        let component = self::UnprovisionedScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;
        Ok(UnprovisionedScreen { component })
    }

    /// Update the displayed public key (called once after identity is known).
    pub fn set_pubkey_hex(&self, hex: &str) {
        self.component.set_pubkey_hex(hex.into());
    }

    /// Update the live stdin RX byte counter on the screen.
    ///
    /// Available only with `--features diagnostics`; compiled out of production.
    #[cfg(feature = "diagnostics")]
    pub fn set_rx_bytes(&self, n: u32) {
        self.component.set_rx_bytes_str(format!("RX: {} bytes", n).into());
    }

    pub fn hide(&self) {
        self.component.hide().ok();
    }
}
