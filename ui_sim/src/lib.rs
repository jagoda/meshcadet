// SPDX-License-Identifier: GPL-3.0-only
//! Host-native render rig for this UI's outer-space theme's
//! image-asset pipeline.
//!
//! # Why this crate exists
//!
//! `firmware/` is a DETACHED Cargo workspace that cross-compiles for
//! `xtensa-esp32s3-espidf` under the `esp` toolchain (`esp-idf-svc`/
//! `esp-idf-hal` do not build for the host) — see `firmware/Cargo.toml`'s own
//! doc comment. That means the firmware crate itself can never be exercised
//! by a host-native `cargo test`, so there is no way to *see* whether a given
//! `slint::slint!{}` markup change actually renders correctly short of
//! flashing real hardware — exactly the on-hardware step this project's
//! design constraint forbids. This crate is the walking skeleton's
//! host-sim answer: it links against plain `slint` (software-renderer only,
//! no esp deps) and imports the REAL `firmware/src/ui/theme.slint` by
//! relative path (not a fork — single source of truth), so it can prove, on
//! the host, that:
//!
//! 1. **PRIMARY embed path** — `Image` + `@image-url(...)` referencing the
//!    real `firmware/assets/space/*.png` files, compiled to raw pixel data
//!    by `SLINT_EMBED_RESOURCES=embed-for-software-renderer`
//!    (`ui_sim/.cargo/config.toml`) — the SAME mechanism
//!    `firmware/src/ui/screens/unprovisioned.rs` ships in production
//!    (`firmware/.cargo/config.toml` sets the identical env var for the esp
//!    target build). Exercised here by the starfield + corner-planet images.
//! 2. **FALLBACK embed path** — a build.rs-generated RGB565 byte array
//!    (`build.rs` in this crate) fed to a runtime `SharedPixelBuffer<Rgb8Pixel>`
//!    + `slint::Image::from_rgb8`, set as an `in property <image>` — exercised
//!      here by the small "moon" swatch standing in for the mascot position.
//!
//! Both paths render into the SAME one-frame host-sim capture
//! (`render_host_sim_frame`), proving both compile AND actually paint pixels
//! (not just "compiles") before the motif-library + 7-screen
//! fan-out. `src/main.rs` writes that frame to
//! `docs/renders/unprovisioned-space-host-sim.png` — the human-visible
//! deliverable; `tests` below assert on the raw pixel buffer so a future
//! regression (e.g. an accidentally-blank image) fails `cargo test`, not
//! just a visual review.
//!
//! This is deliberately NOT a pixel-for-pixel mirror of
//! `unprovisioned.rs`'s full markup (it omits the mascot-bob/glow-in
//! animations and the wordmark/instruction text, which are pure-Slint
//! `Text`/`animate` features already proven by every other themed screen) —
//! it isolates exactly the one previously-unproven mechanism this crate
//! exists to de-risk: images through the inline macro, primary AND fallback.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, PlatformError, WindowAdapter};
use slint::{Image, PhysicalSize, Rgb8Pixel, SharedPixelBuffer};

mod fallback_image {
    include!(concat!(env!("OUT_DIR"), "/fallback_image.rs"));
}

/// Process-wide counting allocator for `ui_sim`'s Slint-rendering rigs —
/// the render-path allocation
/// hook for the UI perf-pass baseline. See `alloc_count.rs`'s module doc for why installing this here is
/// safe for every existing `ui_sim` binary (a transparent `System`
/// passthrough, only counted) and how it differs from `ui_perf`'s own
/// counting allocator (separate crate, separate process, separate subject).
pub mod alloc_count;

#[global_allocator]
static GLOBAL_ALLOC: alloc_count::CountingAllocator = alloc_count::CountingAllocator;

/// Host redraw-scope (dirty-region) rig. See its own module doc.
pub mod perf_profile;

/// M2 host-sim render rig — proves the
/// shared `firmware/src/ui/motifs.slint` asset+motion-helper library, as
/// opposed to this file's M1 walking-skeleton proof above. See that
/// module's own doc for why it is a separate component/render path rather
/// than an extension of `HostSimUi` below.
pub mod motif_library;

/// Host-sim render rig —
/// proves the `compose.rs` Send button's `star-gold` affordance +
/// `RocketOnSend` one-shot + auto-reset `Timer`. See that module's own doc
/// for why it is a separate, narrower component/render path.
pub mod compose_send;

/// Host-sim render rig —
/// proves `gps_status.rs`'s `StatusRow.icon-kind` selector correctly
/// switches between the `RingedPlanetCorner` ("location") and `Comet`
/// ("signal") motifs. See that module's own doc for why it is a separate,
/// narrower component/render path.
pub mod gps_status_rows;

/// Diagnostic render rig proving the `ContactRow`
/// per-row badge and tab-bar aggregate badge from `contact_list.rs` actually
/// paint a visible badge over their surrounding backgrounds. See that
/// module's own doc for why it is a separate, narrower component/render
/// path.
pub mod contact_badges;

/// Host-sim render
/// rig — verbatim copy of `splash.rs`'s markup, proving the full-window
/// `SpaceBackdrop` + bottom-anchored `PlanetHorizon` line art does not
/// overlap/obscure the wordmark or version string (this check's abort
/// condition). See that module's own doc for why it is a separate, narrower
/// component/render path.
pub mod splash_lineart;

/// Host-sim render rig —
/// proves the full-window `SpaceBackdrop` composites correctly underneath a
/// translucent list row, a transparent content pane, and a translucent
/// bottom action bar with an opaque button pill — the mechanism
/// `contact_list.rs`/`message_view.rs`/`compose.rs` all newly rely on. See
/// that module's own doc for why it is a separate, narrower component/render
/// path.
pub mod list_pane_backdrop;

/// Host-sim render rig —
/// proves the standalone `SignalMeter` widget (`ui/signal_meter.slint`,
/// ADR-0010 / `meshcadet-signal-meter` campaign) renders its direct-only
/// ring at level 0 and the correct filled-bar count at levels 1..=5. See
/// that module's own doc for why it is a separate, narrower component/render
/// path.
pub mod signal_meter;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 240;

slint::slint! {
    import { Theme } from "../../firmware/src/ui/theme.slint";

    export component HostSimUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.space-deep;

        // FALLBACK path: fed at runtime from the build.rs-generated RGB565
        // byte array (see fallback_image.rs / lib.rs::render_host_sim_frame).
        in property <image> fallback_mascot;

        // PRIMARY path: @image-url + SLINT_EMBED_RESOURCES=embed-for-
        // software-renderer — identical mechanism to unprovisioned.rs.
        Image {
            source: @image-url("../../firmware/assets/space/starfield.png");
            x: 0px;
            y: 0px;
            width: 320px;
            height: 40px;
        }
        Image {
            source: @image-url("../../firmware/assets/space/planet_corner.png");
            x: 320px - 40px - 8px;
            y: 8px;
            width: 40px;
            height: 40px;
        }

        // FALLBACK path render target, standing in for the mascot position.
        Image {
            source: fallback_mascot;
            x: 320px / 2 - 12px;
            y: 100px;
            width: 24px;
            height: 24px;
        }
    }
}

struct HostSimPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl Platform for HostSimPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Unpack the build.rs-generated packed-RGB565 fallback array into a
/// `SharedPixelBuffer<Rgb8Pixel>` — the runtime half of the FALLBACK path
/// ("fed to Image::from_rgb8 / SharedPixelBuffer at runtime", per the
/// design plan).
fn fallback_image_rgb8() -> SharedPixelBuffer<Rgb8Pixel> {
    let w = fallback_image::FALLBACK_WIDTH as u32;
    let h = fallback_image::FALLBACK_HEIGHT as u32;
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    let bytes = buf.make_mut_bytes();
    for (i, word) in fallback_image::FALLBACK_RGB565.iter().enumerate() {
        let r5 = (word >> 11) & 0x1F;
        let g6 = (word >> 5) & 0x3F;
        let b5 = word & 0x1F;
        // Expand 5/6/5-bit channels back to 8-bit (matches the inverse of
        // ui/theme.rs::rgb565's truncating pack).
        let r8 = ((r5 << 3) | (r5 >> 2)) as u8;
        let g8 = ((g6 << 2) | (g6 >> 4)) as u8;
        let b8 = ((b5 << 3) | (b5 >> 2)) as u8;
        bytes[i * 3] = r8;
        bytes[i * 3 + 1] = g8;
        bytes[i * 3 + 2] = b8;
    }
    buf
}

/// One fully-rendered host-sim frame: `WIDTH * HEIGHT` RGB565 pixels,
/// row-major. Exercises BOTH the primary (`@image-url`) and fallback
/// (build.rs byte array) embed paths in a single component instance.
///
/// # Panics
/// Panics if a Slint platform is already installed in this process (Slint
/// enforces a process-wide singleton) — callers must ensure this runs at
/// most once per process (see module doc: the `cargo test` harness calls
/// this from exactly one `#[test]`).
pub fn render_host_sim_frame() -> Vec<Rgb565Pixel> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    slint::platform::set_platform(Box::new(HostSimPlatform {
        window: window.clone(),
        start: Instant::now(),
    }))
    .expect("Slint platform already set in this process");

    let ui = HostSimUi::new().expect("HostSimUi::new");
    ui.set_fallback_mascot(Image::from_rgb8(fallback_image_rgb8()));
    ui.show().expect("HostSimUi::show");

    slint::platform::update_timers_and_animations();
    window.request_redraw();

    let mut framebuffer = vec![Rgb565Pixel(0); (WIDTH * HEIGHT) as usize];
    let rendered = window.draw_if_needed(|renderer| {
        renderer.render(&mut framebuffer, WIDTH as usize);
    });
    assert!(
        rendered,
        "host-sim frame was not dirty on first render — nothing painted"
    );

    framebuffer
}

/// Convert a rendered RGB565 framebuffer to an `image::RgbImage` (RGB8) for
/// PNG export — the host-sim render deliverable.
pub fn framebuffer_to_rgb_image(
    framebuffer: &[Rgb565Pixel],
    width: u32,
    height: u32,
) -> image::RgbImage {
    let mut img = image::RgbImage::new(width, height);
    for (i, px) in framebuffer.iter().enumerate() {
        let r5 = (px.0 >> 11) & 0x1F;
        let g6 = (px.0 >> 5) & 0x3F;
        let b5 = px.0 & 0x1F;
        let r8 = ((r5 << 3) | (r5 >> 2)) as u8;
        let g8 = ((g6 << 2) | (g6 >> 4)) as u8;
        let b8 = ((b5 << 3) | (b5 >> 2)) as u8;
        let x = (i as u32) % width;
        let y = (i as u32) / width;
        img.put_pixel(x, y, image::Rgb([r8, g8, b8]));
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb_at(fb: &[Rgb565Pixel], x: u32, y: u32) -> (u8, u8, u8) {
        let px = fb[(y * WIDTH + x) as usize];
        let r5 = (px.0 >> 11) & 0x1F;
        let g6 = (px.0 >> 5) & 0x3F;
        let b5 = px.0 & 0x1F;
        (
            ((r5 << 3) | (r5 >> 2)) as u8,
            ((g6 << 2) | (g6 >> 4)) as u8,
            ((b5 << 3) | (b5 >> 2)) as u8,
        )
    }

    /// RGB565 is lossy (5/6/5 bits per channel): round an 8-bit-per-channel
    /// hex color through the SAME pack/expand path the renderer itself uses
    /// so expected-value comparisons below match what actually lands in the
    /// framebuffer, not the pre-quantization hex literal.
    fn quantize565(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let r5 = r >> 3;
        let g6 = g >> 2;
        let b5 = b >> 3;
        (
            ((r5 << 3) | (r5 >> 2)),
            ((g6 << 2) | (g6 >> 4)),
            ((b5 << 3) | (b5 >> 2)),
        )
    }

    /// Single test (see module doc + `render_host_sim_frame`'s panic note):
    /// Slint enforces one platform per process, and `cargo test` runs
    /// `#[test]` functions as threads in ONE process, so exactly one test
    /// function may call the render pipeline.
    #[test]
    fn host_sim_frame_exercises_both_embed_paths_and_paints_pixels() {
        let fb = render_host_sim_frame();
        assert_eq!(fb.len(), (WIDTH * HEIGHT) as usize);

        let space_deep = quantize565(0x07, 0x0a, 0x12);

        // Background (space-deep) must be visible somewhere the images
        // don't cover, e.g. bottom-left corner.
        let bg = rgb_at(&fb, 4, HEIGHT - 4);
        assert_eq!(
            bg,
            space_deep,
            "space-deep background did not render at (4, {})",
            HEIGHT - 4
        );

        // PRIMARY path #1 — starfield header strip must contain at least one
        // non-background (star) pixel: proves the @image-url PNG actually
        // decoded and painted, not just "compiled to a blank/placeholder".
        let starfield_has_content =
            (0..320u32).any(|x| (0..40u32).any(|y| rgb_at(&fb, x, y) != space_deep));
        assert!(
            starfield_has_content,
            "starfield.png region is entirely blank/background"
        );

        // PRIMARY path #2 — corner planet region must contain its
        // planet-warm body color somewhere near its center.
        let planet_center = rgb_at(&fb, 320 - 40 - 8 + 20, 8 + 20);
        assert_ne!(
            planet_center, space_deep,
            "planet_corner.png did not paint over the backdrop"
        );

        // FALLBACK path — the build.rs-generated moon-silver swatch must be
        // visible at its known center (proves SharedPixelBuffer::from_rgb8
        // fed from a build-time byte array actually renders).
        let fallback_center = rgb_at(&fb, WIDTH / 2, 112);
        assert_eq!(
            fallback_center,
            quantize565(0xc8, 0xd0, 0xe0),
            "fallback moon-silver swatch (build.rs byte-array path) did not render at its expected center"
        );
    }
}
