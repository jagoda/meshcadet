// SPDX-License-Identifier: GPL-3.0-only
//! The REAL on-device `MeshCadetEmoji` bitmap font, generated the SAME way
//! `firmware/src/ui/platform.rs`'s identical `emoji_font` module is: by
//! running `firmware/gen_emoji_font.c` (a host-side FreeType tool) against
//! `firmware/assets/NotoEmoji-Regular.ttf` — see `build.rs::build_emoji_font`
//! for the generation step and its own doc comment for why this crate now
//! reuses that SAME generator/asset (relative path, not a fork) rather than
//! relying solely on `SLINT_EMBED_TEXTURES`'s compile-time host-fontconfig
//! bake.
//!
//! `register_device_font` (`lib.rs`) is what actually registers this font on
//! a render rig's window — this module only exposes `emoji_bitmap_font()`.
//!
//! See `build.rs::lint_font_provisioning`'s doc comment for the hazard this
//! exists to close and the build-time enforcement mechanism.

use i_slint_core::graphics::{BitmapFont, BitmapGlyph, BitmapGlyphs, CharacterMapEntry};
use i_slint_core::slice::Slice;

include!(concat!(env!("OUT_DIR"), "/emoji_font.rs"));
