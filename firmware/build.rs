// SPDX-License-Identifier: GPL-3.0-only
//! esp-idf build system integration + build-time emoji font generation.
//!
//! `embuild::espidf::sysenv::output()` propagates the ESP-IDF CMake environment
//! variables (IDF_PATH, IDF_TARGET, linker flags) collected by `cargo-pio` /
//! `embuild` into the Rust build environment so the linker can find libc,
//! freertos, and the esp-idf component archives.
//!
//! `build_emoji_font()` compiles `gen_emoji_font.c` (a host-side C program that
//! uses FreeType) and runs it to generate `$OUT_DIR/emoji_font.rs` — static
//! `BitmapGlyph` arrays for ASCII + UI symbols + 40 curated emoji at every
//! font-size the UI uses (8..28 px; emoji only at the subset where they appear).
//! This file is `include!`-d into `src/ui/platform.rs` at compile time.
//!
//! # Prerequisites (build machine)
//! - `gcc` in PATH
//! - `libfreetype6-dev` installed (provides freetype2 headers + pkg-config)
//! - `pkg-config` in PATH
//! - DejaVu Sans at `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf`
//!   (package `fonts-dejavu-core` on Debian/Ubuntu)

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// True when `output` exists and is at least as new as every `input`.
/// A missing output, or any input we cannot stat, forces regeneration
/// (fail-safe: we would rather rebuild than serve a stale artifact).
fn is_up_to_date(output: &Path, inputs: &[&Path]) -> bool {
    let out_mtime: SystemTime = match std::fs::metadata(output).and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return false,
    };
    for input in inputs {
        match std::fs::metadata(input).and_then(|m| m.modified()) {
            Ok(t) if t > out_mtime => return false,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    true
}

/// Emit the firmware's own build identity as a compile-time env var.
///
/// `main.rs` logs this at boot as the authoritative "which build am I running"
/// signal. It is deliberately firmware-owned and NOT the esp-idf `esp_app_desc`
/// "App version" tag: the latter is generated inside esp-idf-sys's CMake/ninja
/// build, which is not re-invoked on an incremental `cargo run`, so it freezes
/// at the last full esp-idf build's `git describe` while the Rust app relinks
/// fresh. Because this build
/// script no longer emits per-file `rerun-if-changed` (see `main()`), Cargo
/// re-runs it whenever any package file changes, so this value tracks the
/// actual flashed app on every incremental build — including uncommitted edits,
/// which surface as the `-dirty` suffix.
fn emit_build_version() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let version = Command::new("git")
        .args(["describe", "--always", "--dirty", "--tags"])
        .current_dir(&manifest_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=MESHCADET_BUILD_VERSION={version}");
}

fn build_emoji_font() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    let c_src    = manifest_dir.join("gen_emoji_font.c");
    let gen_exe  = out_dir.join("gen_emoji_font");
    let out_rs   = out_dir.join("emoji_font.rs");
    let emoji_ttf = manifest_dir.join("assets/NotoEmoji-Regular.ttf");
    let latin_ttf  = PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf");

    // ── Incremental guard ────────────────────────────────────────────────────
    // This script re-runs on every crate rebuild (it emits no `rerun-if-changed`
    // so MESHCADET_BUILD_VERSION stays fresh — see `main()`). The generated font
    // only depends on the C generator and the bundled emoji TTF, so skip the
    // (multi-second) gcc-compile + run when emoji_font.rs is already newer than
    // both. DejaVu Sans is a stable system font and intentionally excluded, as
    // it was from the previous `rerun-if-changed` set.
    if is_up_to_date(&out_rs, &[&c_src, &emoji_ttf]) {
        return;
    }

    // ── Resolve FreeType compiler/linker flags via pkg-config ────────────────
    let ft_cflags_out = Command::new("pkg-config")
        .args(["--cflags", "freetype2"])
        .output()
        .expect("pkg-config failed — install libfreetype6-dev");
    let ft_libs_out = Command::new("pkg-config")
        .args(["--libs", "freetype2"])
        .output()
        .expect("pkg-config failed — install libfreetype6-dev");

    let ft_cflags: Vec<String> =
        String::from_utf8(ft_cflags_out.stdout)
            .unwrap()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
    let ft_libs: Vec<String> =
        String::from_utf8(ft_libs_out.stdout)
            .unwrap()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

    // ── Compile the C host tool ───────────────────────────────────────────────
    let gcc_status = Command::new("gcc")
        .arg("-O2")
        .args(&ft_cflags)
        .arg(&c_src)
        .args(&ft_libs)
        .arg("-o")
        .arg(&gen_exe)
        .status()
        .expect("gcc not found — install build-essential");

    assert!(
        gcc_status.success(),
        "Failed to compile gen_emoji_font.c (exit {:?})",
        gcc_status.code()
    );

    // ── Run the generator to produce emoji_font.rs ───────────────────────────
    let run_status = Command::new(&gen_exe)
        .arg(&latin_ttf)
        .arg(&emoji_ttf)
        .arg(&out_rs)
        .status()
        .expect("Failed to spawn gen_emoji_font");

    assert!(
        run_status.success(),
        "gen_emoji_font exited with error (exit {:?})",
        run_status.code()
    );
}

fn main() {
    // NOTE: this build script intentionally emits NO `cargo:rerun-if-changed`
    // directives. Cargo therefore re-runs it whenever any file in the firmware
    // package changes, which keeps MESHCADET_BUILD_VERSION in sync with the
    // actual flashed app on every incremental build (an app-source edit alone
    // used to leave the boot version tag frozen — see `emit_build_version`).
    // The emoji-font generation stays incremental via its own mtime guard
    // (`is_up_to_date`), so this broader re-run does not regenerate the font
    // unless its inputs actually changed.
    embuild::espidf::sysenv::output();
    emit_build_version();
    build_emoji_font();
}
