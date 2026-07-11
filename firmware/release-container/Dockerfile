# SPDX-License-Identifier: GPL-3.0-only
#
# Pinned build container for MeshCadet's tag-fired production firmware release
# (.github/workflows/release.yml). See docs/release-reproducibility.md for the
# full rebuild + verify recipe this image is one half of.
#
# WHY A CONTAINER, NOT JUST ENV VARS (do not simplify this away):
#
# firmware/build.rs's build_emoji_font() shells out to the HOST'S OWN `gcc` +
# `libfreetype2` (via pkg-config) against the HOST'S OWN
# /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf to rasterize the bundled
# emoji/UI font at compile time (see that file's own header comment). FreeType's
# hinting/rasterization output — and therefore the exact glyph bitmap bytes
# baked into the firmware image — can change across FreeType or DejaVu Sans
# versions, even with byte-identical source, `SOURCE_DATE_EPOCH`, and
# `--remap-path-prefix` all held constant. Pinning `libfreetype6-dev` and
# `fonts-dejavu-core` to exact package versions closes that hole. Everything
# else this Dockerfile installs (ESP-IDF, the `esp` Rust toolchain fork) is
# ALREADY pinned by version, independent of the container — see
# `firmware/.cargo/config.toml`'s `ESP_IDF_VERSION` and the
# `ESP_TOOLCHAIN_VERSION` build-arg below — so the container's job is
# specifically to also pin the two packages that gate byte-reproducibility,
# not to re-pin things already pinned elsewhere.
#
# BASE IMAGE: pinned by digest (not just tag) so the underlying rootfs never
# drifts under this Dockerfile. Digest resolved from `docker pull ubuntu:22.04`
# on 2026-07-11 — see docs/release-reproducibility.md for the refresh
# procedure if this ever needs to move (e.g. Ubuntu 22.04's apt archive prunes
# the pinned freetype/dejavu package versions below; see that doc's fallback
# via snapshot.ubuntu.com).
FROM ubuntu:22.04@sha256:0e0a0fc6d18feda9db1590da249ac93e8d5abfea8f4c3c0c849ce512b5ef8982

# Fail the build immediately if any command in a pipeline fails (not just the
# last one) — e.g. the `curl | sh` rustup install below silently "succeeds"
# with an empty rustup-init script if the curl half fails, without this.
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# Pinned exact package versions for the byte-reproducibility-critical pair
# (see header comment). Versions current in the Ubuntu 22.04 (jammy) archive
# as of 2026-07-11 — verified via `rmadison` / Launchpad prior to landing this
# file. `apt-get install pkg=version` fails loudly (not silently upgrades) if
# either version is ever pruned from the archive, which is the correct
# failure mode for a reproducibility-critical pin: see
# docs/release-reproducibility.md for the recovery procedure.
ARG FREETYPE_DEV_VERSION=2.11.1+dfsg-1ubuntu0.3
ARG DEJAVU_CORE_VERSION=2.37-2build1

# `esp` Rust toolchain fork version (espup --toolchain-version). Independent
# pin from ESP_IDF_VERSION (firmware/.cargo/config.toml) — the Xtensa rustc
# fork and the ESP-IDF C SDK version separately. Bump deliberately, in its own
# PR, if this ever needs to move.
ARG ESP_TOOLCHAIN_VERSION=1.86.0.0

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_HOME=/opt/cargo \
    RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:/root/.cargo/bin:$PATH

# ── ESP-IDF v5.2.2 native build prerequisites ────────────────────────────────
# Per Espressif's documented Linux prerequisite list for this ESP-IDF release
# (git/wget/flex/bison/gperf/python3/cmake/ninja-build/ccache/libffi-dev/
# libssl-dev/dfu-util/libusb-1.0-0), plus build-essential/pkg-config/curl/
# unzip/ca-certificates for espup + our own build.rs steps, plus the two
# byte-reproducibility-pinned packages from the header comment.
#
# Only libfreetype6-dev/fonts-dejavu-core need an exact-version pin (see
# header comment: they're the one lever that reaches build.rs's
# byte-reproducibility-critical emoji-font generation). Pinning every
# transitive apt package here would be brittle (archive-prune risk
# multiplied across a dozen packages) for no reproducibility benefit, since
# none of the others' versions can change the firmware image's bytes: the
# actual cross-compiler is the separately-pinned `esp` toolchain fork +
# ESP_IDF_VERSION, not this container's host gcc/cmake/ninja.
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config curl unzip ca-certificates git \
      wget flex bison gperf python3 python3-pip python3-venv \
      cmake ninja-build ccache libffi-dev libssl-dev dfu-util libusb-1.0-0 \
      "libfreetype6-dev=${FREETYPE_DEV_VERSION}" \
      "fonts-dejavu-core=${DEJAVU_CORE_VERSION}" \
    && apt-mark hold libfreetype6-dev fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*

# ── Rust: rustup + the `esp` Xtensa toolchain fork (espup) + ldproxy ────────
# Mirrors ci.yml's `firmware` job (same esp-rs/xtensa-toolchain action, same
# ldproxy install) so this container's toolchain provenance matches CI's,
# just pinned to an exact version here instead of "latest".
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
      sh -s -- --profile minimal --default-toolchain stable -y && \
    curl -LO "https://github.com/esp-rs/espup/releases/latest/download/espup-x86_64-unknown-linux-gnu.zip" && \
    unzip -o espup-x86_64-unknown-linux-gnu.zip -d /opt/cargo/bin && \
    chmod +x /opt/cargo/bin/espup && \
    rm espup-x86_64-unknown-linux-gnu.zip && \
    espup install -l debug --targets esp32s3 --toolchain-version "${ESP_TOOLCHAIN_VERSION}" --export-file /opt/export-esp.sh && \
    cargo install ldproxy --locked

# `espup install` writes rustup's `esp` toolchain plus a shell snippet
# (/opt/export-esp.sh) exporting LIBCLANG_PATH/CLANG_PATH/PATH for it. That
# snippet must be sourced in every RUN/entrypoint step that builds the
# firmware crate (firmware/rust-toolchain.toml pins channel "esp", which only
# exists once this snippet's PATH additions make `rustup`'s `esp` toolchain
# reachable) — release-container/build.sh (the container's ENTRYPOINT) does
# this first.
COPY release-container/build.sh /opt/build.sh
RUN chmod +x /opt/build.sh

WORKDIR /build
ENTRYPOINT ["/opt/build.sh"]
