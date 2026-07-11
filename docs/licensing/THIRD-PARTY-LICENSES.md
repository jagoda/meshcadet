# Third-party crate licenses (full dependency-graph snapshot)

This is the raw appendix backing `docs/licensing/DEPENDENCY-AUDIT.md`: every crate
(name, version, SPDX license expression) resolved by `cargo metadata` across MeshCadent's
two Cargo graphs — the root workspace (`protocol`, `host`, `xtask`, `ui_sim`, `ui_perf`)
and the detached `firmware/` workspace (cross-compiled for `xtensa-esp32s3-espidf`) —
deduplicated by `(name, version)`. **691 unique crate/version pairs, 0 with an unresolved
license field.**

Regenerate with:

```sh
cargo metadata --format-version 1 > /tmp/root_meta.json
(cd firmware && cargo metadata --format-version 1 > /tmp/fw_meta.json)
# then merge + dedupe the `.packages[].license` field by (name, version) from both files
```

See `DEPENDENCY-AUDIT.md` for the compatibility analysis (which license families appear,
why each is GPLv3-safe, and the entries that needed a closer look).

| Crate | Version | License |
|---|---|---|
| `adler2` | 2.0.1 | 0BSD OR MIT OR Apache-2.0 |
| `aes` | 0.8.4 | MIT OR Apache-2.0 |
| `aho-corasick` | 1.1.4 | Unlicense OR MIT |
| `aligned` | 0.4.3 | MIT OR Apache-2.0 |
| `aligned-vec` | 0.6.4 | MIT |
| `allocator-api2` | 0.2.21 | MIT OR Apache-2.0 |
| `android-activity` | 0.6.1 | MIT OR Apache-2.0 |
| `android-properties` | 0.2.2 | MIT |
| `android_system_properties` | 0.1.5 | MIT/Apache-2.0 |
| `annotate-snippets` | 0.12.16 | MIT OR Apache-2.0 |
| `anstream` | 1.0.0 | MIT OR Apache-2.0 |
| `anstyle` | 1.0.14 | MIT OR Apache-2.0 |
| `anstyle-parse` | 1.0.0 | MIT OR Apache-2.0 |
| `anstyle-query` | 1.1.5 | MIT OR Apache-2.0 |
| `anstyle-wincon` | 3.0.11 | MIT OR Apache-2.0 |
| `anyhow` | 1.0.102 | MIT OR Apache-2.0 |
| `arbitrary` | 1.4.2 | MIT OR Apache-2.0 |
| `arg_enum_proc_macro` | 0.3.4 | MIT |
| `arrayref` | 0.3.9 | BSD-2-Clause |
| `arrayvec` | 0.7.6 | MIT OR Apache-2.0 |
| `arrayvec` | 0.7.8 | MIT OR Apache-2.0 |
| `as-slice` | 0.2.1 | MIT OR Apache-2.0 |
| `async-broadcast` | 0.7.2 | MIT OR Apache-2.0 |
| `async-channel` | 2.5.0 | Apache-2.0 OR MIT |
| `async-executor` | 1.14.0 | Apache-2.0 OR MIT |
| `async-io` | 2.6.0 | Apache-2.0 OR MIT |
| `async-lock` | 3.4.2 | Apache-2.0 OR MIT |
| `async-process` | 2.5.0 | Apache-2.0 OR MIT |
| `async-recursion` | 1.1.1 | MIT OR Apache-2.0 |
| `async-signal` | 0.2.14 | Apache-2.0 OR MIT |
| `async-task` | 4.7.1 | Apache-2.0 OR MIT |
| `async-trait` | 0.1.89 | MIT OR Apache-2.0 |
| `atomic-waker` | 1.1.2 | Apache-2.0 OR MIT |
| `auto_enums` | 0.8.8 | Apache-2.0 OR MIT |
| `auto_enums` | 0.8.9 | Apache-2.0 OR MIT |
| `autocfg` | 1.5.1 | Apache-2.0 OR MIT |
| `av-scenechange` | 0.14.1 | MIT |
| `av1-grain` | 0.2.5 | BSD-2-Clause |
| `avif-serialize` | 0.8.9 | BSD-3-Clause |
| `az` | 1.2.1 | MIT/Apache-2.0 |
| `base64` | 0.22.1 | MIT OR Apache-2.0 |
| `bincode` | 2.0.1 | MIT |
| `bindgen` | 0.71.1 | BSD-3-Clause |
| `bindgen` | 0.72.1 | BSD-3-Clause |
| `bit_field` | 0.10.3 | Apache-2.0/MIT |
| `bitflags` | 1.3.2 | MIT/Apache-2.0 |
| `bitflags` | 2.13.0 | MIT OR Apache-2.0 |
| `bitstream-io` | 4.10.0 | MIT/Apache-2.0 |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 |
| `block2` | 0.5.1 | MIT |
| `block2` | 0.6.2 | MIT |
| `blocking` | 1.6.2 | Apache-2.0 OR MIT |
| `borsh` | 1.6.1 | MIT OR Apache-2.0 |
| `borsh` | 1.7.0 | MIT OR Apache-2.0 |
| `bstr` | 1.12.1 | MIT OR Apache-2.0 |
| `build-time` | 0.1.3 | MIT |
| `built` | 0.8.1 | MIT |
| `bumpalo` | 3.20.3 | MIT OR Apache-2.0 |
| `by_address` | 1.2.1 | MIT OR Apache-2.0 |
| `byte-slice-cast` | 1.2.3 | MIT |
| `bytemuck` | 1.25.0 | Zlib OR Apache-2.0 OR MIT |
| `bytemuck_derive` | 1.10.2 | Zlib OR Apache-2.0 OR MIT |
| `byteorder` | 1.5.0 | Unlicense OR MIT |
| `byteorder-lite` | 0.1.0 | Unlicense OR MIT |
| `bytes` | 1.11.1 | MIT |
| `bytes` | 1.12.0 | MIT |
| `calloop` | 0.13.0 | MIT |
| `calloop` | 0.14.4 | MIT |
| `camino` | 1.2.2 | MIT OR Apache-2.0 |
| `cargo-platform` | 0.1.9 | MIT OR Apache-2.0 |
| `cargo_metadata` | 0.18.1 | MIT |
| `cc` | 1.2.63 | MIT OR Apache-2.0 |
| `cc` | 1.2.65 | MIT OR Apache-2.0 |
| `cexpr` | 0.6.0 | Apache-2.0/MIT |
| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 |
| `cfg_aliases` | 0.2.1 | MIT |
| `cgl` | 0.3.2 | MIT / Apache-2.0 |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 |
| `cipher` | 0.4.4 | MIT OR Apache-2.0 |
| `clang-sys` | 1.8.1 | Apache-2.0 |
| `clap` | 4.6.1 | MIT OR Apache-2.0 |
| `clap_builder` | 4.6.0 | MIT OR Apache-2.0 |
| `clap_derive` | 4.6.1 | MIT OR Apache-2.0 |
| `clap_lex` | 1.1.0 | MIT OR Apache-2.0 |
| `clipboard-win` | 5.4.1 | BSL-1.0 |
| `clru` | 0.6.3 | MIT |
| `cmake` | 0.1.58 | MIT OR Apache-2.0 |
| `color_quant` | 1.1.0 | MIT |
| `colorchoice` | 1.0.5 | MIT OR Apache-2.0 |
| `combine` | 4.6.7 | MIT |
| `concurrent-queue` | 2.5.0 | Apache-2.0 OR MIT |
| `const-field-offset` | 0.2.0 | MIT OR Apache-2.0 |
| `const-field-offset-macro` | 0.2.0 | MIT OR Apache-2.0 |
| `const_format` | 0.2.36 | Zlib |
| `const_format_proc_macros` | 0.2.34 | Zlib |
| `convert_case` | 0.10.0 | MIT |
| `copypasta` | 0.10.2 | MIT / Apache-2.0 |
| `core-foundation` | 0.10.1 | MIT OR Apache-2.0 |
| `core-foundation` | 0.9.4 | MIT OR Apache-2.0 |
| `core-foundation-sys` | 0.8.7 | MIT OR Apache-2.0 |
| `core-graphics` | 0.23.2 | MIT OR Apache-2.0 |
| `core-graphics-types` | 0.1.3 | MIT OR Apache-2.0 |
| `core_maths` | 0.1.1 | MIT |
| `countme` | 3.0.1 | MIT OR Apache-2.0 |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 |
| `crc32fast` | 1.5.0 | MIT OR Apache-2.0 |
| `critical-section` | 1.2.0 | MIT OR Apache-2.0 |
| `crossbeam-channel` | 0.5.15 | MIT OR Apache-2.0 |
| `crossbeam-deque` | 0.8.6 | MIT OR Apache-2.0 |
| `crossbeam-epoch` | 0.9.18 | MIT OR Apache-2.0 |
| `crossbeam-utils` | 0.8.21 | MIT OR Apache-2.0 |
| `crunchy` | 0.2.4 | MIT |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 |
| `cursor-icon` | 1.2.0 | MIT OR Apache-2.0 OR Zlib |
| `curve25519-dalek` | 4.1.3 | BSD-3-Clause |
| `curve25519-dalek-derive` | 0.1.1 | MIT/Apache-2.0 |
| `cvt` | 0.1.2 | Apache-2.0 |
| `darling` | 0.21.3 | MIT |
| `darling_core` | 0.21.3 | MIT |
| `darling_macro` | 0.21.3 | MIT |
| `data-url` | 0.3.2 | MIT OR Apache-2.0 |
| `defmt` | 1.1.0 | MIT OR Apache-2.0 |
| `defmt-macros` | 1.1.0 | MIT OR Apache-2.0 |
| `defmt-parser` | 1.0.0 | MIT OR Apache-2.0 |
| `derive_more` | 2.1.1 | MIT |
| `derive_more-impl` | 2.1.1 | MIT |
| `derive_utils` | 0.15.1 | Apache-2.0 OR MIT |
| `digest` | 0.10.7 | MIT OR Apache-2.0 |
| `dispatch` | 0.2.0 | MIT |
| `dispatch2` | 0.3.1 | Zlib OR Apache-2.0 OR MIT |
| `display-interface` | 0.5.0 | MIT OR Apache-2.0 |
| `display-interface-spi` | 0.5.0 | MIT OR Apache-2.0 |
| `displaydoc` | 0.2.6 | MIT OR Apache-2.0 |
| `dlib` | 0.5.3 | MIT |
| `dpi` | 0.1.2 | Apache-2.0 AND MIT |
| `drm` | 0.14.1 | MIT |
| `drm-ffi` | 0.9.1 | MIT |
| `drm-fourcc` | 2.2.0 | MIT |
| `drm-sys` | 0.8.1 | MIT |
| `ed25519` | 2.2.3 | Apache-2.0 OR MIT |
| `ed25519-dalek` | 2.2.0 | BSD-3-Clause |
| `either` | 1.16.0 | MIT OR Apache-2.0 |
| `embassy-futures` | 0.1.2 | MIT OR Apache-2.0 |
| `embassy-sync` | 0.7.2 | MIT OR Apache-2.0 |
| `embedded-can` | 0.4.1 | MIT OR Apache-2.0 |
| `embedded-graphics` | 0.8.2 | MIT OR Apache-2.0 |
| `embedded-graphics-core` | 0.4.1 | MIT OR Apache-2.0 |
| `embedded-hal` | 0.2.7 | MIT OR Apache-2.0 |
| `embedded-hal` | 1.0.0 | MIT OR Apache-2.0 |
| `embedded-hal-async` | 1.0.0 | MIT OR Apache-2.0 |
| `embedded-hal-nb` | 1.0.0 | MIT OR Apache-2.0 |
| `embedded-io` | 0.6.1 | MIT OR Apache-2.0 |
| `embedded-io` | 0.7.1 | MIT OR Apache-2.0 |
| `embedded-io-async` | 0.6.1 | MIT OR Apache-2.0 |
| `embedded-io-async` | 0.7.0 | MIT OR Apache-2.0 |
| `embedded-svc` | 0.29.0 | MIT OR Apache-2.0 |
| `embuild` | 0.33.1 | MIT OR Apache-2.0 |
| `endi` | 1.1.1 | MIT |
| `enumflags2` | 0.7.12 | MIT OR Apache-2.0 |
| `enumflags2_derive` | 0.7.12 | MIT OR Apache-2.0 |
| `enumset` | 1.1.13 | MIT/Apache-2.0 |
| `enumset_derive` | 0.15.0 | MIT/Apache-2.0 |
| `envy` | 0.4.2 | MIT |
| `equator` | 0.4.2 | MIT |
| `equator-macro` | 0.4.2 | MIT |
| `equivalent` | 1.0.2 | Apache-2.0 OR MIT |
| `errno` | 0.3.14 | MIT OR Apache-2.0 |
| `error-code` | 3.3.2 | BSL-1.0 |
| `esp-idf-hal` | 0.46.2 | MIT OR Apache-2.0 |
| `esp-idf-svc` | 0.52.1 | MIT OR Apache-2.0 |
| `esp-idf-sys` | 0.37.2 | MIT OR Apache-2.0 |
| `euclid` | 0.22.14 | MIT OR Apache-2.0 |
| `event-listener` | 5.4.1 | Apache-2.0 OR MIT |
| `event-listener-strategy` | 0.5.4 | Apache-2.0 OR MIT |
| `exr` | 1.74.0 | BSD-3-Clause |
| `fastrand` | 2.4.1 | Apache-2.0 OR MIT |
| `fax` | 0.2.7 | MIT |
| `fdeflate` | 0.3.7 | MIT OR Apache-2.0 |
| `fiat-crypto` | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause |
| `field-offset` | 0.3.6 | MIT OR Apache-2.0 |
| `filetime` | 0.2.29 | MIT/Apache-2.0 |
| `find-msvc-tools` | 0.1.9 | MIT OR Apache-2.0 |
| `firmware` | 0.0.0 | MIT OR Apache-2.0 |
| `flate2` | 1.1.9 | MIT OR Apache-2.0 |
| `float-cmp` | 0.9.0 | MIT |
| `fnv` | 1.0.7 | Apache-2.0 / MIT |
| `foldhash` | 0.1.5 | Zlib |
| `foldhash` | 0.2.0 | Zlib |
| `font-types` | 0.11.3 | MIT OR Apache-2.0 |
| `fontdb` | 0.23.0 | MIT |
| `fontique` | 0.8.0 | Apache-2.0 OR MIT |
| `foreign-types` | 0.5.0 | MIT/Apache-2.0 |
| `foreign-types-macros` | 0.2.3 | MIT/Apache-2.0 |
| `foreign-types-shared` | 0.3.1 | MIT/Apache-2.0 |
| `form_urlencoded` | 1.2.2 | MIT OR Apache-2.0 |
| `fs_at` | 0.2.1 | Apache-2.0 |
| `futures` | 0.3.32 | MIT OR Apache-2.0 |
| `futures-channel` | 0.3.32 | MIT OR Apache-2.0 |
| `futures-core` | 0.3.32 | MIT OR Apache-2.0 |
| `futures-executor` | 0.3.32 | MIT OR Apache-2.0 |
| `futures-io` | 0.3.32 | MIT OR Apache-2.0 |
| `futures-lite` | 2.6.1 | Apache-2.0 OR MIT |
| `futures-macro` | 0.3.32 | MIT OR Apache-2.0 |
| `futures-sink` | 0.3.32 | MIT OR Apache-2.0 |
| `futures-task` | 0.3.32 | MIT OR Apache-2.0 |
| `futures-util` | 0.3.32 | MIT OR Apache-2.0 |
| `generic-array` | 0.14.7 | MIT |
| `getopts` | 0.2.24 | MIT OR Apache-2.0 |
| `getrandom` | 0.2.17 | MIT OR Apache-2.0 |
| `getrandom` | 0.3.4 | MIT OR Apache-2.0 |
| `getrandom` | 0.4.2 | MIT OR Apache-2.0 |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 |
| `gif` | 0.14.2 | MIT OR Apache-2.0 |
| `gl_generator` | 0.14.0 | Apache-2.0 |
| `glob` | 0.3.3 | MIT OR Apache-2.0 |
| `globset` | 0.4.18 | Unlicense OR MIT |
| `globwalk` | 0.8.1 | MIT |
| `glow` | 0.17.0 | MIT OR Apache-2.0 OR Zlib |
| `glutin` | 0.32.3 | Apache-2.0 |
| `glutin_egl_sys` | 0.7.1 | Apache-2.0 |
| `glutin_wgl_sys` | 0.6.1 | Apache-2.0 |
| `grid` | 1.0.1 | MIT |
| `half` | 2.7.1 | MIT OR Apache-2.0 |
| `harfrust` | 0.5.2 | MIT |
| `hash32` | 0.3.1 | MIT OR Apache-2.0 |
| `hashbrown` | 0.14.5 | MIT OR Apache-2.0 |
| `hashbrown` | 0.15.5 | MIT OR Apache-2.0 |
| `hashbrown` | 0.16.1 | MIT OR Apache-2.0 |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 |
| `heapless` | 0.8.0 | MIT OR Apache-2.0 |
| `heapless` | 0.9.3 | MIT OR Apache-2.0 |
| `heck` | 0.4.1 | MIT OR Apache-2.0 |
| `heck` | 0.5.0 | MIT OR Apache-2.0 |
| `hermit-abi` | 0.3.9 | MIT OR Apache-2.0 |
| `hermit-abi` | 0.5.2 | MIT OR Apache-2.0 |
| `hex` | 0.4.3 | MIT OR Apache-2.0 |
| `hex-literal` | 0.4.1 | MIT OR Apache-2.0 |
| `hmac` | 0.12.1 | MIT OR Apache-2.0 |
| `home` | 0.5.12 | MIT OR Apache-2.0 |
| `host` | 0.0.0 | MIT OR Apache-2.0 |
| `htmlparser` | 0.2.1 | MIT OR Apache-2.0 |
| `i-slint-backend-linuxkms` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `i-slint-backend-selector` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `i-slint-backend-winit` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `i-slint-common` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `i-slint-compiler` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `i-slint-core` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `i-slint-core-macros` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `i-slint-renderer-skia` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `i-slint-renderer-software` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `iana-time-zone` | 0.1.65 | MIT OR Apache-2.0 |
| `iana-time-zone-haiku` | 0.1.2 | MIT OR Apache-2.0 |
| `icu_collections` | 2.2.0 | Unicode-3.0 |
| `icu_locale` | 2.2.0 | Unicode-3.0 |
| `icu_locale_core` | 2.2.0 | Unicode-3.0 |
| `icu_locale_data` | 2.2.0 | Unicode-3.0 |
| `icu_normalizer` | 2.2.0 | Unicode-3.0 |
| `icu_normalizer_data` | 2.2.0 | Unicode-3.0 |
| `icu_properties` | 2.2.0 | Unicode-3.0 |
| `icu_properties_data` | 2.2.0 | Unicode-3.0 |
| `icu_provider` | 2.2.0 | Unicode-3.0 |
| `icu_segmenter` | 2.2.0 | Unicode-3.0 |
| `icu_segmenter_data` | 2.2.0 | Unicode-3.0 |
| `id-arena` | 2.3.0 | MIT/Apache-2.0 |
| `ident_case` | 1.0.1 | MIT/Apache-2.0 |
| `idna` | 1.1.0 | MIT OR Apache-2.0 |
| `idna_adapter` | 1.2.2 | Apache-2.0 OR MIT |
| `ignore` | 0.4.26 | Unlicense OR MIT |
| `image` | 0.25.10 | MIT OR Apache-2.0 |
| `image-webp` | 0.2.4 | MIT OR Apache-2.0 |
| `imagesize` | 0.14.0 | MIT |
| `imgref` | 1.12.2 | CC0-1.0 OR Apache-2.0 |
| `indexmap` | 2.14.0 | Apache-2.0 OR MIT |
| `inout` | 0.1.4 | MIT OR Apache-2.0 |
| `input` | 0.9.1 | MIT |
| `input-sys` | 1.19.0 | MIT |
| `integer-sqrt` | 0.1.5 | Apache-2.0/MIT |
| `interpolate_name` | 0.2.4 | MIT |
| `io-kit-sys` | 0.4.1 | MIT / Apache-2.0 |
| `io-lifetimes` | 1.0.11 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `is_terminal_polyfill` | 1.70.2 | MIT OR Apache-2.0 |
| `itertools` | 0.13.0 | MIT OR Apache-2.0 |
| `itertools` | 0.14.0 | MIT OR Apache-2.0 |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 |
| `jni` | 0.22.4 | MIT OR Apache-2.0 |
| `jni-macros` | 0.22.4 | MIT OR Apache-2.0 |
| `jni-sys` | 0.3.1 | MIT OR Apache-2.0 |
| `jni-sys` | 0.4.1 | MIT OR Apache-2.0 |
| `jni-sys-macros` | 0.4.1 | MIT OR Apache-2.0 |
| `jobserver` | 0.1.34 | MIT OR Apache-2.0 |
| `js-sys` | 0.3.103 | MIT OR Apache-2.0 |
| `js-sys` | 0.3.99 | MIT OR Apache-2.0 |
| `keyboard-types` | 0.7.0 | MIT OR Apache-2.0 |
| `khronos_api` | 3.1.0 | Apache-2.0 |
| `konst` | 0.2.20 | Zlib |
| `konst_macro_rules` | 0.2.19 | Zlib |
| `kurbo` | 0.13.1 | Apache-2.0 OR MIT |
| `lazy_static` | 1.5.0 | MIT OR Apache-2.0 |
| `leb128fmt` | 0.1.0 | MIT OR Apache-2.0 |
| `lebe` | 0.5.3 | BSD-3-Clause |
| `libc` | 0.2.186 | MIT OR Apache-2.0 |
| `libfuzzer-sys` | 0.4.13 | (MIT OR Apache-2.0) AND NCSA |
| `libloading` | 0.8.9 | ISC |
| `libm` | 0.2.16 | MIT |
| `libredox` | 0.1.17 | MIT |
| `libredox` | 0.1.18 | MIT |
| `libudev-sys` | 0.1.4 | MIT |
| `linebender_resource_handle` | 0.1.1 | Apache-2.0 OR MIT |
| `linked-hash-map` | 0.5.6 | MIT/Apache-2.0 |
| `linked_hash_set` | 0.1.6 | Apache-2.0 |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `linux-raw-sys` | 0.4.15 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `linux-raw-sys` | 0.9.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `litemap` | 0.8.2 | Unicode-3.0 |
| `log` | 0.4.32 | MIT OR Apache-2.0 |
| `log` | 0.4.33 | MIT OR Apache-2.0 |
| `loop9` | 0.1.5 | MIT |
| `lyon_algorithms` | 1.0.20 | MIT OR Apache-2.0 |
| `lyon_extra` | 1.1.0 | MIT OR Apache-2.0 |
| `lyon_geom` | 1.0.19 | MIT OR Apache-2.0 |
| `lyon_path` | 1.0.19 | MIT OR Apache-2.0 |
| `mach2` | 0.4.3 | BSD-2-Clause OR MIT OR Apache-2.0 |
| `maybe-rayon` | 0.1.1 | MIT |
| `memchr` | 2.8.1 | Unlicense OR MIT |
| `memchr` | 2.8.2 | Unlicense OR MIT |
| `memmap2` | 0.9.10 | MIT OR Apache-2.0 |
| `memmap2` | 0.9.11 | MIT OR Apache-2.0 |
| `memoffset` | 0.9.1 | MIT |
| `micromath` | 2.1.0 | Apache-2.0 OR MIT |
| `minimal-lexical` | 0.2.1 | MIT/Apache-2.0 |
| `miniz_oxide` | 0.8.9 | MIT OR Zlib OR Apache-2.0 |
| `mipidsi` | 0.8.0 | MIT |
| `moxcms` | 0.8.1 | BSD-3-Clause OR Apache-2.0 |
| `muda` | 0.18.0 | Apache-2.0 OR MIT |
| `natord` | 1.0.9 | MIT |
| `nb` | 0.1.3 | MIT OR Apache-2.0 |
| `nb` | 1.1.0 | MIT OR Apache-2.0 |
| `ndk` | 0.9.0 | MIT OR Apache-2.0 |
| `ndk-context` | 0.1.1 | MIT OR Apache-2.0 |
| `ndk-sys` | 0.6.0+11769913 | MIT OR Apache-2.0 |
| `new_debug_unreachable` | 1.0.6 | MIT |
| `nix` | 0.26.4 | MIT |
| `nix` | 0.29.0 | MIT |
| `nix` | 0.30.1 | MIT |
| `no_std_io2` | 0.9.4 | Apache-2.0 OR MIT |
| `nom` | 7.1.3 | MIT |
| `nom` | 8.0.0 | MIT |
| `noop_proc_macro` | 0.3.0 | MIT |
| `normpath` | 1.5.1 | MIT OR Apache-2.0 |
| `num-bigint` | 0.4.6 | MIT OR Apache-2.0 |
| `num-bigint` | 0.4.8 | MIT OR Apache-2.0 |
| `num-derive` | 0.4.2 | MIT OR Apache-2.0 |
| `num-integer` | 0.1.46 | MIT OR Apache-2.0 |
| `num-rational` | 0.4.2 | MIT OR Apache-2.0 |
| `num-traits` | 0.2.19 | MIT OR Apache-2.0 |
| `num_enum` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 |
| `num_enum_derive` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 |
| `objc-sys` | 0.3.5 | MIT |
| `objc2` | 0.5.2 | MIT |
| `objc2` | 0.6.4 | MIT |
| `objc2-app-kit` | 0.2.2 | MIT |
| `objc2-app-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-cloud-kit` | 0.2.2 | MIT |
| `objc2-cloud-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-contacts` | 0.2.2 | MIT |
| `objc2-core-data` | 0.2.2 | MIT |
| `objc2-core-data` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-core-foundation` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-core-graphics` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-core-image` | 0.2.2 | MIT |
| `objc2-core-image` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-core-location` | 0.2.2 | MIT |
| `objc2-core-text` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-core-video` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-encode` | 4.1.0 | MIT |
| `objc2-foundation` | 0.2.2 | MIT |
| `objc2-foundation` | 0.3.2 | MIT |
| `objc2-io-surface` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-link-presentation` | 0.2.2 | MIT |
| `objc2-metal` | 0.2.2 | MIT |
| `objc2-metal` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-quartz-core` | 0.2.2 | MIT |
| `objc2-quartz-core` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-symbols` | 0.2.2 | MIT |
| `objc2-ui-kit` | 0.2.2 | MIT |
| `objc2-ui-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT |
| `objc2-uniform-type-identifiers` | 0.2.2 | MIT |
| `objc2-user-notifications` | 0.2.2 | MIT |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 |
| `once_cell_polyfill` | 1.70.2 | MIT OR Apache-2.0 |
| `orbclient` | 0.3.55 | MIT |
| `ordered-stream` | 0.2.0 | MIT OR Apache-2.0 |
| `parking` | 2.2.1 | Apache-2.0 OR MIT |
| `parlance` | 0.1.0 | Apache-2.0 OR MIT |
| `parley` | 0.8.0 | Apache-2.0 OR MIT |
| `parley_data` | 0.8.0 | Apache-2.0 OR MIT |
| `paste` | 1.0.15 | MIT OR Apache-2.0 |
| `pastey` | 0.1.1 | MIT OR Apache-2.0 |
| `percent-encoding` | 2.3.2 | MIT OR Apache-2.0 |
| `pico-args` | 0.5.0 | MIT |
| `pin-project` | 1.1.13 | Apache-2.0 OR MIT |
| `pin-project-internal` | 1.1.13 | Apache-2.0 OR MIT |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT |
| `pin-utils` | 0.1.0 | MIT OR Apache-2.0 |
| `pin-weak` | 1.1.0 | MIT |
| `piper` | 0.2.5 | MIT OR Apache-2.0 |
| `pkg-config` | 0.3.33 | MIT OR Apache-2.0 |
| `plain` | 0.2.3 | MIT/Apache-2.0 |
| `png` | 0.17.16 | MIT OR Apache-2.0 |
| `png` | 0.18.1 | MIT OR Apache-2.0 |
| `polling` | 3.11.0 | Apache-2.0 OR MIT |
| `polycool` | 0.4.0 | MIT OR Apache-2.0 |
| `portable-atomic` | 1.13.1 | Apache-2.0 OR MIT |
| `potential_utf` | 0.1.5 | Unicode-3.0 |
| `ppv-lite86` | 0.2.21 | MIT OR Apache-2.0 |
| `prettyplease` | 0.2.37 | MIT OR Apache-2.0 |
| `proc-macro-crate` | 3.5.0 | MIT OR Apache-2.0 |
| `proc-macro-error-attr2` | 2.0.0 | MIT OR Apache-2.0 |
| `proc-macro-error2` | 2.0.1 | MIT OR Apache-2.0 |
| `proc-macro2` | 1.0.106 | MIT OR Apache-2.0 |
| `profiling` | 1.0.18 | MIT OR Apache-2.0 |
| `profiling-procmacros` | 1.0.18 | MIT OR Apache-2.0 |
| `protocol` | 0.0.0 | MIT OR Apache-2.0 |
| `pulldown-cmark` | 0.13.4 | MIT |
| `pulldown-cmark-escape` | 0.11.0 | MIT |
| `pxfm` | 0.1.29 | BSD-3-Clause OR Apache-2.0 |
| `qoi` | 0.4.1 | MIT/Apache-2.0 |
| `qrcode` | 0.14.1 | MIT OR Apache-2.0 |
| `quick-error` | 2.0.1 | MIT/Apache-2.0 |
| `quote` | 1.0.45 | MIT OR Apache-2.0 |
| `r-efi` | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| `r-efi` | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| `rand` | 0.8.6 | MIT OR Apache-2.0 |
| `rand` | 0.9.4 | MIT OR Apache-2.0 |
| `rand_chacha` | 0.3.1 | MIT OR Apache-2.0 |
| `rand_chacha` | 0.9.0 | MIT OR Apache-2.0 |
| `rand_core` | 0.6.4 | MIT OR Apache-2.0 |
| `rand_core` | 0.9.5 | MIT OR Apache-2.0 |
| `rav1e` | 0.8.1 | BSD-2-Clause |
| `ravif` | 0.13.0 | BSD-3-Clause |
| `raw-window-handle` | 0.6.2 | MIT OR Apache-2.0 OR Zlib |
| `raw-window-metal` | 1.1.0 | MIT OR Apache-2.0 |
| `rayon` | 1.12.0 | MIT OR Apache-2.0 |
| `rayon-core` | 1.13.0 | MIT OR Apache-2.0 |
| `read-fonts` | 0.37.0 | MIT OR Apache-2.0 |
| `read-fonts` | 0.39.2 | MIT OR Apache-2.0 |
| `redox_syscall` | 0.4.1 | MIT |
| `redox_syscall` | 0.5.18 | MIT |
| `redox_syscall` | 0.8.1 | MIT |
| `redox_syscall` | 0.9.0 | MIT |
| `regex` | 1.12.3 | MIT OR Apache-2.0 |
| `regex` | 1.12.4 | MIT OR Apache-2.0 |
| `regex-automata` | 0.4.14 | MIT OR Apache-2.0 |
| `regex-syntax` | 0.8.10 | MIT OR Apache-2.0 |
| `regex-syntax` | 0.8.11 | MIT OR Apache-2.0 |
| `remove_dir_all` | 0.8.4 | MIT OR Apache-2.0 |
| `resvg` | 0.47.0 | Apache-2.0 OR MIT |
| `rgb` | 0.8.53 | MIT |
| `rowan` | 0.16.1 | MIT OR Apache-2.0 |
| `roxmltree` | 0.21.1 | MIT OR Apache-2.0 |
| `rspolib` | 0.1.2 | MIT |
| `rustc-hash` | 1.1.0 | Apache-2.0/MIT |
| `rustc-hash` | 2.1.2 | Apache-2.0 OR MIT |
| `rustc-hash` | 2.1.3 | Apache-2.0 OR MIT |
| `rustc_version` | 0.4.1 | MIT OR Apache-2.0 |
| `rustix` | 0.38.44 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `rustix` | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `rustversion` | 1.0.22 | MIT OR Apache-2.0 |
| `rustybuzz` | 0.20.1 | MIT |
| `same-file` | 1.0.6 | Unlicense/MIT |
| `scoped-tls-hkt` | 0.1.5 | MIT/Apache-2.0 |
| `scopeguard` | 1.2.0 | MIT OR Apache-2.0 |
| `semver` | 1.0.28 | MIT OR Apache-2.0 |
| `serde` | 1.0.228 | MIT OR Apache-2.0 |
| `serde_core` | 1.0.228 | MIT OR Apache-2.0 |
| `serde_derive` | 1.0.228 | MIT OR Apache-2.0 |
| `serde_json` | 1.0.150 | MIT OR Apache-2.0 |
| `serde_repr` | 0.1.20 | MIT OR Apache-2.0 |
| `serde_spanned` | 1.1.1 | MIT OR Apache-2.0 |
| `serialport` | 4.9.0 | MPL-2.0 |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 |
| `shlex` | 1.3.0 | MIT OR Apache-2.0 |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 |
| `signal-hook-registry` | 1.4.8 | MIT OR Apache-2.0 |
| `signature` | 2.2.0 | Apache-2.0 OR MIT |
| `simd-adler32` | 0.3.9 | MIT |
| `simd_cesu8` | 1.1.1 | Apache-2.0 OR MIT |
| `simd_helpers` | 0.1.0 | MIT |
| `simdutf8` | 0.1.5 | MIT OR Apache-2.0 |
| `simplecss` | 0.2.2 | Apache-2.0 OR MIT |
| `siphasher` | 1.0.3 | MIT/Apache-2.0 |
| `skia-bindings` | 0.90.0 | MIT |
| `skia-safe` | 0.90.0 | MIT |
| `skrifa` | 0.40.0 | MIT OR Apache-2.0 |
| `skrifa` | 0.42.1 | MIT OR Apache-2.0 |
| `slab` | 0.4.12 | MIT |
| `slint` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `slint-build` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `slint-macros` | 1.16.1 | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| `slotmap` | 1.1.1 | Zlib |
| `smallvec` | 1.15.2 | MIT OR Apache-2.0 |
| `smol_str` | 0.2.2 | MIT OR Apache-2.0 |
| `smol_str` | 0.3.6 | MIT OR Apache-2.0 |
| `snafu` | 0.8.9 | MIT OR Apache-2.0 |
| `snafu-derive` | 0.8.9 | MIT OR Apache-2.0 |
| `softbuffer` | 0.4.8 | MIT OR Apache-2.0 |
| `spin_on` | 0.1.1 | Apache-2.0 OR MIT |
| `stable_deref_trait` | 1.2.1 | MIT OR Apache-2.0 |
| `strict-num` | 0.1.1 | MIT |
| `strsim` | 0.11.1 | MIT |
| `strum` | 0.24.1 | MIT |
| `strum` | 0.27.2 | MIT |
| `strum` | 0.28.0 | MIT |
| `strum_macros` | 0.24.3 | MIT |
| `strum_macros` | 0.27.2 | MIT |
| `strum_macros` | 0.28.0 | MIT |
| `subtle` | 2.6.1 | BSD-3-Clause |
| `svgtypes` | 0.16.1 | Apache-2.0 OR MIT |
| `swash` | 0.2.9 | Apache-2.0 OR MIT |
| `syn` | 1.0.109 | MIT OR Apache-2.0 |
| `syn` | 2.0.117 | MIT OR Apache-2.0 |
| `synstructure` | 0.13.2 | MIT |
| `sys-locale` | 0.3.2 | MIT OR Apache-2.0 |
| `taffy` | 0.9.2 | MIT |
| `tar` | 0.4.46 | MIT OR Apache-2.0 |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 |
| `text-size` | 1.1.1 | MIT OR Apache-2.0 |
| `thiserror` | 1.0.69 | MIT OR Apache-2.0 |
| `thiserror` | 2.0.18 | MIT OR Apache-2.0 |
| `thiserror-impl` | 1.0.69 | MIT OR Apache-2.0 |
| `thiserror-impl` | 2.0.18 | MIT OR Apache-2.0 |
| `tiff` | 0.11.3 | MIT |
| `tiny-skia` | 0.12.0 | BSD-3-Clause |
| `tiny-skia-path` | 0.12.0 | BSD-3-Clause |
| `tinystr` | 0.8.3 | Unicode-3.0 |
| `tinyvec` | 1.11.0 | Zlib OR Apache-2.0 OR MIT |
| `tinyvec_macros` | 0.1.1 | MIT OR Apache-2.0 OR Zlib |
| `toml` | 0.9.12+spec-1.1.0 | MIT OR Apache-2.0 |
| `toml_datetime` | 0.7.5+spec-1.1.0 | MIT OR Apache-2.0 |
| `toml_datetime` | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 |
| `toml_edit` | 0.25.12+spec-1.1.0 | MIT OR Apache-2.0 |
| `toml_parser` | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 |
| `toml_writer` | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 |
| `tracing` | 0.1.44 | MIT |
| `tracing-attributes` | 0.1.31 | MIT |
| `tracing-core` | 0.1.36 | MIT |
| `ttf-parser` | 0.25.1 | MIT OR Apache-2.0 |
| `typed-index-collections` | 3.5.0 | MIT OR Apache-2.0 |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 |
| `udev` | 0.9.3 | MIT |
| `uds_windows` | 1.2.1 | MIT |
| `ui_perf` | 0.0.0 | MIT OR Apache-2.0 |
| `ui_sim` | 0.0.0 | MIT OR Apache-2.0 |
| `uncased` | 0.9.10 | MIT OR Apache-2.0 |
| `unescaper` | 0.1.8 | GPL-3.0/MIT |
| `unicase` | 2.9.0 | MIT OR Apache-2.0 |
| `unicode-bidi` | 0.3.18 | MIT OR Apache-2.0 |
| `unicode-bidi-mirroring` | 0.4.0 | MIT/Apache-2.0 |
| `unicode-ccc` | 0.4.0 | MIT/Apache-2.0 |
| `unicode-ident` | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| `unicode-linebreak` | 0.1.5 | Apache-2.0 |
| `unicode-properties` | 0.1.4 | MIT/Apache-2.0 |
| `unicode-script` | 0.5.8 | MIT OR Apache-2.0 |
| `unicode-segmentation` | 1.13.3 | MIT OR Apache-2.0 |
| `unicode-vo` | 0.1.0 | MIT/Apache-2.0 |
| `unicode-width` | 0.2.2 | MIT OR Apache-2.0 |
| `unicode-xid` | 0.2.6 | MIT OR Apache-2.0 |
| `unty` | 0.0.4 | MIT OR Apache-2.0 |
| `url` | 2.5.8 | MIT OR Apache-2.0 |
| `usvg` | 0.47.0 | Apache-2.0 OR MIT |
| `utf8_iter` | 1.0.4 | Apache-2.0 OR MIT |
| `utf8parse` | 0.2.2 | Apache-2.0 OR MIT |
| `uuid` | 1.23.3 | Apache-2.0 OR MIT |
| `uuid` | 1.23.4 | Apache-2.0 OR MIT |
| `v_frame` | 0.3.9 | BSD-2-Clause |
| `version_check` | 0.9.5 | MIT/Apache-2.0 |
| `void` | 1.0.2 | MIT |
| `vtable` | 0.4.0 | MIT OR Apache-2.0 |
| `vtable-macro` | 0.4.0 | MIT OR Apache-2.0 |
| `walkdir` | 2.5.0 | Unlicense/MIT |
| `wasi` | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wasip2` | 1.0.3+wasi-0.2.9 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wasip2` | 1.0.4+wasi-0.2.12 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wasip3` | 0.4.0+wasi-0.3.0-rc-2026-01-06 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wasm-bindgen` | 0.2.122 | MIT OR Apache-2.0 |
| `wasm-bindgen` | 0.2.126 | MIT OR Apache-2.0 |
| `wasm-bindgen-futures` | 0.4.72 | MIT OR Apache-2.0 |
| `wasm-bindgen-futures` | 0.4.76 | MIT OR Apache-2.0 |
| `wasm-bindgen-macro` | 0.2.122 | MIT OR Apache-2.0 |
| `wasm-bindgen-macro` | 0.2.126 | MIT OR Apache-2.0 |
| `wasm-bindgen-macro-support` | 0.2.122 | MIT OR Apache-2.0 |
| `wasm-bindgen-macro-support` | 0.2.126 | MIT OR Apache-2.0 |
| `wasm-bindgen-shared` | 0.2.122 | MIT OR Apache-2.0 |
| `wasm-bindgen-shared` | 0.2.126 | MIT OR Apache-2.0 |
| `wasm-encoder` | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wasm-metadata` | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wasmparser` | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `web-sys` | 0.3.103 | MIT OR Apache-2.0 |
| `web-sys` | 0.3.99 | MIT OR Apache-2.0 |
| `web-time` | 1.1.0 | MIT OR Apache-2.0 |
| `webbrowser` | 1.2.1 | MIT OR Apache-2.0 |
| `weezl` | 0.1.12 | MIT OR Apache-2.0 |
| `which` | 4.4.2 | MIT |
| `winapi-util` | 0.1.11 | Unlicense OR MIT |
| `windows` | 0.62.2 | MIT OR Apache-2.0 |
| `windows-collections` | 0.3.2 | MIT OR Apache-2.0 |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 |
| `windows-implement` | 0.60.2 | MIT OR Apache-2.0 |
| `windows-interface` | 0.59.3 | MIT OR Apache-2.0 |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 |
| `windows-numerics` | 0.3.1 | MIT OR Apache-2.0 |
| `windows-result` | 0.4.1 | MIT OR Apache-2.0 |
| `windows-strings` | 0.5.1 | MIT OR Apache-2.0 |
| `windows-sys` | 0.48.0 | MIT OR Apache-2.0 |
| `windows-sys` | 0.52.0 | MIT OR Apache-2.0 |
| `windows-sys` | 0.59.0 | MIT OR Apache-2.0 |
| `windows-sys` | 0.60.2 | MIT OR Apache-2.0 |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 |
| `windows-targets` | 0.48.5 | MIT OR Apache-2.0 |
| `windows-targets` | 0.52.6 | MIT OR Apache-2.0 |
| `windows-targets` | 0.53.5 | MIT OR Apache-2.0 |
| `windows-threading` | 0.2.1 | MIT OR Apache-2.0 |
| `windows_aarch64_gnullvm` | 0.48.5 | MIT OR Apache-2.0 |
| `windows_aarch64_gnullvm` | 0.52.6 | MIT OR Apache-2.0 |
| `windows_aarch64_gnullvm` | 0.53.1 | MIT OR Apache-2.0 |
| `windows_aarch64_msvc` | 0.48.5 | MIT OR Apache-2.0 |
| `windows_aarch64_msvc` | 0.52.6 | MIT OR Apache-2.0 |
| `windows_aarch64_msvc` | 0.53.1 | MIT OR Apache-2.0 |
| `windows_i686_gnu` | 0.48.5 | MIT OR Apache-2.0 |
| `windows_i686_gnu` | 0.52.6 | MIT OR Apache-2.0 |
| `windows_i686_gnu` | 0.53.1 | MIT OR Apache-2.0 |
| `windows_i686_gnullvm` | 0.52.6 | MIT OR Apache-2.0 |
| `windows_i686_gnullvm` | 0.53.1 | MIT OR Apache-2.0 |
| `windows_i686_msvc` | 0.48.5 | MIT OR Apache-2.0 |
| `windows_i686_msvc` | 0.52.6 | MIT OR Apache-2.0 |
| `windows_i686_msvc` | 0.53.1 | MIT OR Apache-2.0 |
| `windows_x86_64_gnu` | 0.48.5 | MIT OR Apache-2.0 |
| `windows_x86_64_gnu` | 0.52.6 | MIT OR Apache-2.0 |
| `windows_x86_64_gnu` | 0.53.1 | MIT OR Apache-2.0 |
| `windows_x86_64_gnullvm` | 0.48.5 | MIT OR Apache-2.0 |
| `windows_x86_64_gnullvm` | 0.52.6 | MIT OR Apache-2.0 |
| `windows_x86_64_gnullvm` | 0.53.1 | MIT OR Apache-2.0 |
| `windows_x86_64_msvc` | 0.48.5 | MIT OR Apache-2.0 |
| `windows_x86_64_msvc` | 0.52.6 | MIT OR Apache-2.0 |
| `windows_x86_64_msvc` | 0.53.1 | MIT OR Apache-2.0 |
| `winit` | 0.30.13 | Apache-2.0 |
| `winnow` | 0.7.15 | MIT |
| `winnow` | 1.0.3 | MIT |
| `wit-bindgen` | 0.51.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wit-bindgen` | 0.57.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wit-bindgen-core` | 0.51.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wit-bindgen-rust` | 0.51.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wit-bindgen-rust-macro` | 0.51.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wit-component` | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `wit-parser` | 0.244.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| `write-fonts` | 0.45.0 | MIT OR Apache-2.0 |
| `writeable` | 0.6.3 | Unicode-3.0 |
| `x25519-dalek` | 2.0.1 | BSD-3-Clause |
| `xattr` | 1.6.1 | MIT OR Apache-2.0 |
| `xkbcommon` | 0.9.0 | MIT |
| `xkbcommon-dl` | 0.4.2 | MIT |
| `xkeysym` | 0.2.1 | MIT OR Apache-2.0 OR Zlib |
| `xml-rs` | 0.8.28 | MIT |
| `xmlwriter` | 0.1.0 | MIT |
| `xtask` | 0.0.0 | MIT OR Apache-2.0 |
| `y4m` | 0.8.0 | MIT |
| `yazi` | 0.2.1 | Apache-2.0 OR MIT |
| `yeslogic-fontconfig-sys` | 6.0.1 | MIT |
| `yoke` | 0.8.3 | Unicode-3.0 |
| `yoke-derive` | 0.8.2 | Unicode-3.0 |
| `zbus` | 5.16.0 | MIT |
| `zbus_macros` | 5.16.0 | MIT |
| `zbus_names` | 4.3.2 | MIT |
| `zeno` | 0.3.3 | Apache-2.0 OR MIT |
| `zerocopy` | 0.8.50 | BSD-2-Clause OR Apache-2.0 OR MIT |
| `zerocopy-derive` | 0.8.50 | BSD-2-Clause OR Apache-2.0 OR MIT |
| `zerofrom` | 0.1.8 | Unicode-3.0 |
| `zerofrom-derive` | 0.1.7 | Unicode-3.0 |
| `zeroize` | 1.8.2 | Apache-2.0 OR MIT |
| `zeroize_derive` | 1.4.3 | Apache-2.0 OR MIT |
| `zerotrie` | 0.2.4 | Unicode-3.0 |
| `zerovec` | 0.11.6 | Unicode-3.0 |
| `zerovec-derive` | 0.11.3 | Unicode-3.0 |
| `zmij` | 1.0.21 | MIT |
| `zune-core` | 0.5.1 | MIT OR Apache-2.0 OR Zlib |
| `zune-inflate` | 0.2.54 | MIT OR Apache-2.0 OR Zlib |
| `zune-jpeg` | 0.5.15 | MIT OR Apache-2.0 OR Zlib |
| `zvariant` | 5.12.0 | MIT |
| `zvariant_derive` | 5.12.0 | MIT |
| `zvariant_utils` | 3.4.0 | MIT |
