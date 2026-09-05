# Changelog

All notable changes to MeshCadet are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning is managed by [release-please](https://github.com/googleapis/release-please)
— see `release-please-config.json` and `docs/adr/0004-release-architecture.md`.
The entry below documents everything landed before release-please's first
`chore(release): vX.Y.Z` PR.

## [0.8.0](https://github.com/jagoda/meshcadet/compare/v0.7.0...v0.8.0) (2026-09-05)


### Added

* **firmware-core:** land the screen-lock core-state decisions ([1baae58](https://github.com/jagoda/meshcadet/commit/1baae580421e6bd77760674c12695a06e7f63557))
* **firmware-core:** land the screen-lock core-state decisions ([346e28f](https://github.com/jagoda/meshcadet/commit/346e28f7459aed734aeb8b1cae154b286a87d911))
* **firmware-core:** lock PIN comparison + admin-menu timeout formatter ([b54aa8d](https://github.com/jagoda/meshcadet/commit/b54aa8d3f203d8bbb820740fa7e05cc3152c3eaa))
* **firmware:** land the screen-lock firmware transport ([db20d85](https://github.com/jagoda/meshcadet/commit/db20d85d80e6fc72965cc40751eccc625dba78af))
* **firmware:** land the screen-lock firmware transport (NVS, admin frames, boot seed) ([56edde5](https://github.com/jagoda/meshcadet/commit/56edde5c63f8de2f75ac5cb2b8b6cc882cabb111))
* **firmware:** land the screen-lock overlay, lock screen, and admin-menu controls ([a295699](https://github.com/jagoda/meshcadet/commit/a295699121c53ff8e21f1fec940392137f29a626))
* **firmware:** land the screen-lock overlay, lock screen, and admin-menu controls ([37650f4](https://github.com/jagoda/meshcadet/commit/37650f424c27ac2557fddd8538daaf7605cad6c1))
* **firmware:** register the lock screen's three new glyphs in gen_emoji_font.c ([13eebbf](https://github.com/jagoda/meshcadet/commit/13eebbf2d4d970f783aa49309c241d991aac53c3))
* **host:** land the screen-lock host CLI surface ([f195702](https://github.com/jagoda/meshcadet/commit/f19570212bd4ec688cb82c6e55e1dd380463fe99))
* **host:** land the screen-lock host CLI surface ([1144b14](https://github.com/jagoda/meshcadet/commit/1144b14f04f6397324af7789acd30cc69d13df5c))
* **power:** backlight brightness control, admin-menu stepper ([7fe015a](https://github.com/jagoda/meshcadet/commit/7fe015ab7d0815d9500bf81ed8f2d7ebc40e7efa))
* **power:** backlight brightness control, admin-menu stepper, provenance-guard hardening ([9cefba8](https://github.com/jagoda/meshcadet/commit/9cefba8b22df09345957d9d4c3a13fa1d75eb0a8))
* **power:** ESP-IDF dynamic frequency scaling (DFS only) ([432e456](https://github.com/jagoda/meshcadet/commit/432e456d86b519327f110c9766f02237d426dae7))
* **power:** ESP-IDF dynamic frequency scaling (DFS only) ([f433311](https://github.com/jagoda/meshcadet/commit/f43331141021fc0c86aa559adec12759458df213))
* **power:** idle-screen — ST7789 sleep, render-skip, adaptive tick, dim-before-sleep ([d96b4ac](https://github.com/jagoda/meshcadet/commit/d96b4ac7c64370e17a3c444d8720317c76f04982))
* **power:** idle-screen enabler — ST7789 sleep, render-skip, adaptive tick, dim-before-sleep ([0ce0f61](https://github.com/jagoda/meshcadet/commit/0ce0f61e4c73c8b045e88cdc49f52ad9e665382f))
* **protocol:** land the screen-lock wire contract (Rust + JS) ([bf679df](https://github.com/jagoda/meshcadet/commit/bf679df89f47cef4676a04260e6e4565bb9c4628))
* **protocol:** land the screen-lock wire contract (Rust + JS) ([9f0a2d2](https://github.com/jagoda/meshcadet/commit/9f0a2d2864299697daa301791f0ff49b48ab0374))
* **provisioner:** land the screen-lock web-provisioner surface ([3873c33](https://github.com/jagoda/meshcadet/commit/3873c33aeaab84be890ec9044d9716a745d76db2))
* **provisioner:** screen-lock web-provisioner surface ([141bc5a](https://github.com/jagoda/meshcadet/commit/141bc5a744eda715dba208d22fb4f239271cda10))
* **ui_sim:** register the real on-device font on promo/host-sim rigs, lint enforces it ([93dced1](https://github.com/jagoda/meshcadet/commit/93dced115679b8a03c7baaa536f6654815e78e31))
* **ui_sim:** register the real on-device font on promo/host-sim rigs, lint enforces it ([56a20e0](https://github.com/jagoda/meshcadet/commit/56a20e0f769cbead9e05d6788b3bb448e9881bf4))


### Fixed

* **battery:** rebuild display pipeline as a three-state voltage-domain bucket ([1f11774](https://github.com/jagoda/meshcadet/commit/1f11774a98bd116adde5493e8115b39f0e051e8f))
* **battery:** rebuild display pipeline as a three-state voltage-domain bucket ([6ff89a7](https://github.com/jagoda/meshcadet/commit/6ff89a74ea5135fe0ca97dc3af9c95e042bfc895))
* **ci:** make the paths-filter table total, not a list of known lanes ([9026390](https://github.com/jagoda/meshcadet/commit/9026390a4c1cea5bf995160d5f7430e3529f4978))
* **ci:** make the paths-filter table total, not a list of known lanes ([7d43fa0](https://github.com/jagoda/meshcadet/commit/7d43fa07db79167124dc5566b01a97ed7e20cbaa))
* **ci:** run xtask's static guard battery on firmware-only PRs ([c648d6f](https://github.com/jagoda/meshcadet/commit/c648d6f98a2f3bc47393536eda32e9acde49d3f7))
* **ci:** run xtask's static guard battery on firmware-only PRs ([843af65](https://github.com/jagoda/meshcadet/commit/843af65d8fd923b2c1492857656982237236e32c))
* **ci:** scrub internal doc-path leaks from ui_sim font-provisioning docs ([ceac521](https://github.com/jagoda/meshcadet/commit/ceac5215114234b9a36f3ff15aaa33537f92d4ee))
* **ci:** widen local vocabulary guard from commit-subjects to tracked file content ([c30d00e](https://github.com/jagoda/meshcadet/commit/c30d00e53152b6bde2dc2b6f153cd5a3962fcb76))
* **ci:** widen local vocabulary guard from commit-subjects to tracked file content ([6e4684b](https://github.com/jagoda/meshcadet/commit/6e4684b5541a4d87af5836cdb6988e065dc57cb9))
* **docs:** ADR-0014 provenance tags — mechanize D2's estimate-labelling rule ([4f8c4d2](https://github.com/jagoda/meshcadet/commit/4f8c4d2b6f13853afdbd54030819e6fe219fb97d))
* **docs:** correct Phase 7 record-accuracy gaps (deep-review pass 1) ([df89f6d](https://github.com/jagoda/meshcadet/commit/df89f6d444297a2aa019f38d720b67d3a348e2ef))
* **docs:** correct Phase 7 record-accuracy gaps left open by deep-review pass 1 ([fb86c76](https://github.com/jagoda/meshcadet/commit/fb86c765883e61ad9142c70a4af57819cd55e343))
* **docs:** re-pin ADR-0014's gps.rs line citation after the record-corrections shift ([98cdde4](https://github.com/jagoda/meshcadet/commit/98cdde4a9ef35943dcc370e53f39dfcb569db407))
* **docs:** rewrite internal-ops path leaks to public ADR citations ([f698bab](https://github.com/jagoda/meshcadet/commit/f698bab3b431e526963479419d11f62bf4b84628))
* **docs:** rewrite internal-ops vocabulary leaks in lock docs ([b1fa575](https://github.com/jagoda/meshcadet/commit/b1fa575011afc96ff0ca9290c8aaa1aaab72c152))
* **docs:** tag ADR-0014 D5's bare power figures, give row 9/D4 inline arithmetic, mechanize D2 ([3aaf95c](https://github.com/jagoda/meshcadet/commit/3aaf95c016963e97b9642a090596da2f28a794e4))
* **docs:** update README's stale 'no CI yet' claim ([c59d620](https://github.com/jagoda/meshcadet/commit/c59d620bed5b3ce75c92bac11ac0ba5f71dc0ef7))
* **docs:** update README's stale 'no CI yet' claim to match ci.yml ([30c793c](https://github.com/jagoda/meshcadet/commit/30c793c0af80b6ecc89c2289b11ed7bb3ffbd354))
* **firmware:** dedup the lock backoff countdown's per-tick Slint push ([240729a](https://github.com/jagoda/meshcadet/commit/240729adc8c8fed8f9642fa443bf7d63110d5a55))
* **firmware:** keep the lock screen's D5 badge live while locked ([8d0f55f](https://github.com/jagoda/meshcadet/commit/8d0f55f820dfcebbd5477b13e106275eb5e8df27))
* **firmware:** rephrase admin_server comment to avoid banned internal-ops term ([1f160f9](https://github.com/jagoda/meshcadet/commit/1f160f921e5ad3b8211c847971150ed23e122fa3))
* **firmware:** screen-lock integrity fixes from deep-review pass 1 ([9d4a376](https://github.com/jagoda/meshcadet/commit/9d4a376dd47c50765bcf26246994f534507b0533))
* **firmware:** screen-lock integrity fixes from deep-review pass 1 ([77401e5](https://github.com/jagoda/meshcadet/commit/77401e5c7211fd08dcb1e6b04afc4524ab797bf1))
* **power:** bound ASLEEP_IDLE_TICK_MS by GT911 tap-loss, not just wake-latency ([901dfcd](https://github.com/jagoda/meshcadet/commit/901dfcde65d5877313efa7b05471cb509be08bb0))
* **power:** bound ASLEEP_IDLE_TICK_MS by GT911 tap-loss, not just wake-latency ([e7f9c9a](https://github.com/jagoda/meshcadet/commit/e7f9c9a45f29794af70c56e58eff60593b523a51))
* **power:** const-block the GT911 tap-loss bound assertion for clippy ([dbfff83](https://github.com/jagoda/meshcadet/commit/dbfff83e2d5b151797a84bc34f50fdb3b9eadb16))
* **power:** guard power_provenance's new bidirectional window against UTF-8 mid-char slicing ([5284722](https://github.com/jagoda/meshcadet/commit/528472245393f2c45184a6df4b728065bfa6cfb1))
* **protocol:** dedup a transport-coded frame's payload correctly ([924c471](https://github.com/jagoda/meshcadet/commit/924c4715cc7410ff0e973e18e0126bf3ebaee452))
* **protocol:** reject malformed inner PATH-return path_len instead of panicking ([5863e02](https://github.com/jagoda/meshcadet/commit/5863e026935aa3c91213830231b65758f94d9d5e))
* transport-coded dedup mis-slicing and inner PATH-return path_len panic ([54dfdf0](https://github.com/jagoda/meshcadet/commit/54dfdf0cbae95e784465ff591e1ccd65293bc810))
* **ui_sim:** derive splash promo version from workspace, regenerate PNG ([2a63a5e](https://github.com/jagoda/meshcadet/commit/2a63a5ebabd485ec4040a69d9c6008aaf0fe9b20))
* **ui_sim:** derive splash promo version from workspace, regenerate PNG ([f3d779f](https://github.com/jagoda/meshcadet/commit/f3d779ff848b09d45d39e62f22b551564f551726))
* **ui_sim:** reconcile promo-screenshot rig drift, regenerate PNGs ([8e892b9](https://github.com/jagoda/meshcadet/commit/8e892b97c2c91c4b69a0d1a82a85f7cb048665c7))
* **ui_sim:** reconcile promo-screenshot rig drift, regenerate PNGs ([561cb40](https://github.com/jagoda/meshcadet/commit/561cb403d84f769291a4b74e03378bdec9d6cf62))
* v1.17 currency bump, group-sender-name disclosure, transport-code RX fix ([465036d](https://github.com/jagoda/meshcadet/commit/465036df5165da1c717e3c425b492d5a7726133a))
* v1.17 currency bump, group-sender-name disclosure, transport-code RX fix ([420f084](https://github.com/jagoda/meshcadet/commit/420f084c20b6d5e3e5ee7bf5bbe235020eb23a5d))
* **xtask:** wire slint_thread_affinity into default guard battery ([84c174e](https://github.com/jagoda/meshcadet/commit/84c174e46ab2b45345cbec4d17c440c3e0aea6b6))


### Documentation

* **adr:** strip internal-ops path reference from ADR-0014 ([55c7528](https://github.com/jagoda/meshcadet/commit/55c7528d28668b21018b8d1f6335afc4a0879934))
* **firmware:** correct the RESPONSE-arm's stale justification comment ([b85fd82](https://github.com/jagoda/meshcadet/commit/b85fd82313ad979d7502be5a07e19154596a6b06))
* **lock:** screen-lock README section, ADR-0013, and bench procedure ([ce31718](https://github.com/jagoda/meshcadet/commit/ce3171840c08ed3dc63c9e9a077bd118c99fa304))
* **lock:** screen-lock README, ADR-0013, and bench procedure ([d117b73](https://github.com/jagoda/meshcadet/commit/d117b73ab63cfa56130d72a2fbf351bf41596675))
* **power:** ADR-0014 power policy — invariants, estimate labelling, light-sleep contract ([f165d99](https://github.com/jagoda/meshcadet/commit/f165d993f02820461f6dce6f1d2cb95743e723a5))
* **power:** ADR-0014 power policy — invariants, estimate labelling, light-sleep contract ([2c0957e](https://github.com/jagoda/meshcadet/commit/2c0957e8382328ada4d03866daec9e2c98c5ce9b))
* **power:** document GPS standby's negative result — no ASCII/checksum lever exists for either GNSS variant ([e22e95c](https://github.com/jagoda/meshcadet/commit/e22e95c3e39ad1c0faa651a84db986cff659563c))
* **power:** GPS standby — documented negative result, both GNSS variants ([ea5b729](https://github.com/jagoda/meshcadet/commit/ea5b72970b4c5b064d1b34b9b429beb0b181fabe))

## [0.7.0](https://github.com/jagoda/meshcadet/compare/v0.6.0...v0.7.0) (2026-08-22)


### Added

* **firmware:** auto-retry unacked DM/room-post sends with backoff cap ([d6eb9d0](https://github.com/jagoda/meshcadet/commit/d6eb9d02a7463dfc6a91e937668f7353b6869746))
* **firmware:** auto-retry unacked DM/room-post sends with backoff cap ([5da1534](https://github.com/jagoda/meshcadet/commit/5da153439cfd6b793e771b2f82ebf3da414f9ccc))
* **firmware:** stabilize battery SoC — NVS persistence, peak sampling, slew-limit + buckets ([2508553](https://github.com/jagoda/meshcadet/commit/250855365451d1279c19c942b6de46bfb9502ca1))
* **firmware:** tri-state delivery-state model for DM/room-post ACKs ([0315458](https://github.com/jagoda/meshcadet/commit/0315458adae9e07adc2a743f66821c97b39ee7ce))
* **firmware:** tri-state delivery-state model for DM/room-post ACKs ([8cfa860](https://github.com/jagoda/meshcadet/commit/8cfa860188b832b4ed786323fc5d29b98b79b4fb))
* **protocol:** send raw battery millivolts in the telemetry RESPONSE ([208f45f](https://github.com/jagoda/meshcadet/commit/208f45f6dccee3b398c1942b2bc2450433fb1fcc))
* **protocol:** send raw battery millivolts in the telemetry RESPONSE ([1a8c1c6](https://github.com/jagoda/meshcadet/commit/1a8c1c64292779e49dbd64271d372a9a36b9346b))
* **ui:** add a glanceable battery indicator beside the header signal meter ([e6a0019](https://github.com/jagoda/meshcadet/commit/e6a0019c4a9b04c41d46ff1266133c3dd597f187))
* **ui:** add a glanceable battery indicator beside the header SignalMeter ([8d630a7](https://github.com/jagoda/meshcadet/commit/8d630a7a4c077892c2dd315dd4ad0530e1b0b238))


### Fixed

* **ci:** close the vocabulary-leak recurrence path at commit-subject time ([ba7a834](https://github.com/jagoda/meshcadet/commit/ba7a834220ce413cc0ef3d852e6e9a4751196a80))
* **ci:** scrub internal doc-path leak from alloc_tick_dedup.rs ([706bb2b](https://github.com/jagoda/meshcadet/commit/706bb2bf5cdce2ad3af14a833e3884d8a051959f))
* **ci:** scrub internal doc-path leaks from battery.rs comments ([aa86dee](https://github.com/jagoda/meshcadet/commit/aa86dee5b84cfadbf032106d1330f05c3d08dc62))
* **ci:** scrub internal-ops vocabulary leak from ACK-matcher doc comments ([a56bd88](https://github.com/jagoda/meshcadet/commit/a56bd88fb99fefbd2bce18dbf067e0e1949212ff))
* **ci:** scrub internal-ops vocabulary leak from battery.rs doc comment ([712389b](https://github.com/jagoda/meshcadet/commit/712389b2d00060547d61c12a5dcc9d0730afa432))
* **firmware:** accept pre-fix GPS RMC sync from the GNSS module's backup RTC ([f07eded](https://github.com/jagoda/meshcadet/commit/f07eded51db10edb5d3ff49137b98429ea5ae7cd))
* **firmware:** battery indicator no longer crawls at ~2%/30s off a contaminated boot-time guess ([aa6dbc0](https://github.com/jagoda/meshcadet/commit/aa6dbc034d9860098dd097f2973bc0ab461c842c))
* **firmware:** honest clock-source provenance and a real sync-age on the GPS status screen ([a3bb136](https://github.com/jagoda/meshcadet/commit/a3bb136c4e5830dbb6dd29ecda11137733e5663c))
* **firmware:** honest clock-source provenance and a real sync-age on the GPS status screen ([7962350](https://github.com/jagoda/meshcadet/commit/7962350a22e259908b35dab7bf8e9c17fbf5b0a7))
* **firmware:** skip the discharge slew limit on the first confirmed percent basis ([edff082](https://github.com/jagoda/meshcadet/commit/edff082aa6a6794f8bdd7db2cd4c10ef68b84d07))
* **ui:** align header icon clusters to the header's outer edges ([d6585ae](https://github.com/jagoda/meshcadet/commit/d6585ae04d125cdde99c054f82beb914991d1c00))
* **ui:** align header icon clusters to the header's outer edges ([87719fd](https://github.com/jagoda/meshcadet/commit/87719fdb37b525ee7b4b616778b666c85a8e7b2b))
* **ui:** center the messaging view's header status icons vertically ([60abad2](https://github.com/jagoda/meshcadet/commit/60abad28132a8d1dc810df28fec67aef9172cd96))
* **ui:** center the messaging view's header status icons vertically ([985f07d](https://github.com/jagoda/meshcadet/commit/985f07dd10dd0f5e1ff4dd50be8a0e4ea24c2b00))
* **ui:** drop the static comet accent from the message-view header ([3ce2ea2](https://github.com/jagoda/meshcadet/commit/3ce2ea24e8fe6c9efd62f6e0a91de2e4ac378f8d))
* **ui:** drop the static comet accent from the message-view header ([8e5136f](https://github.com/jagoda/meshcadet/commit/8e5136fd4fe1a0ad02c642769078c346f4147807))

## [0.6.0](https://github.com/jagoda/meshcadet/compare/v0.5.0...v0.6.0) (2026-08-04)


### Added

* **firmware:** add diagnostics-gated on-device perf instrumentation ([764ac91](https://github.com/jagoda/meshcadet/commit/764ac91c530dcd8e96d6d179511a429c94711038))
* **firmware:** add diagnostics-gated on-device perf instrumentation ([d1db0ec](https://github.com/jagoda/meshcadet/commit/d1db0ec7735d920c7160836067a9e878f64613d0))
* **firmware:** curate RENDER_EXTRA_CPS to the campaign's 600-entry target ([1fff506](https://github.com/jagoda/meshcadet/commit/1fff50622a3aa5398075f1e5d7d7238590f6bf13))
* **firmware:** curate RENDER_EXTRA_CPS to the campaign's 600-entry target ([82dbdaf](https://github.com/jagoda/meshcadet/commit/82dbdaf5680e3fb1d30508772df117504a80b7da))
* **firmware:** grow emoji picker to 96 entries with category tabs ([8fdc078](https://github.com/jagoda/meshcadet/commit/8fdc078e931a92776a82792a14aa8a925ce2ee1b))
* **firmware:** grow emoji picker to 96 entries with category tabs ([e04da64](https://github.com/jagoda/meshcadet/commit/e04da64eee4c492a7fdb8d7107a1c1396a63045a))
* **firmware:** implement ADR-0012 dispatcher/UI task split (M1) ([713b3dc](https://github.com/jagoda/meshcadet/commit/713b3dc6f2010d889022630dda62dcfbbf4cbf44))
* **firmware:** implement ADR-0012 dispatcher/UI task split (M1) ([af448df](https://github.com/jagoda/meshcadet/commit/af448df4a14cea149c4461991f9c8f8522a7dcc3))
* **firmware:** introduce RENDER_EXTRA_CPS render-only codepoint table ([669dd2f](https://github.com/jagoda/meshcadet/commit/669dd2fefd5832f1abd4c52437ad6ae6e8e43f00))
* **firmware:** introduce render-only emoji tier with EMOJI_TABLE subset invariant ([07116d2](https://github.com/jagoda/meshcadet/commit/07116d2c6af55d924c348e01483022319a2a959d))
* **firmware:** recompute app-image size against a committed baseline ([cd08085](https://github.com/jagoda/meshcadet/commit/cd080851235442bb94095ff292ad93bbc2bdb34d))
* **firmware:** recompute app-image size against a committed baseline ([ca0a4bf](https://github.com/jagoda/meshcadet/commit/ca0a4bf257d206dbe71fe075637a41dcf32accd9))
* **firmware:** restore ui_step/ui-starvation instrumentation on ui_task ([148fcc6](https://github.com/jagoda/meshcadet/commit/148fcc69287acd83c3537cd1d1b86d379d1006e9))
* **firmware:** restore ui_step/ui-starvation instrumentation on ui_task ([231ad56](https://github.com/jagoda/meshcadet/commit/231ad56fbf11c9c6bf593ba42a7bf594e89eeb16))
* **perf-device-report:** add the collection-kit ingest path ([e04f69d](https://github.com/jagoda/meshcadet/commit/e04f69d69e0879fc3da2108bb3071b642c2204b1))
* **perf-loop-model:** expose device-report re-calibration hook ([3e47008](https://github.com/jagoda/meshcadet/commit/3e47008fdafcbb0131216ed59286072c3a764409))
* **perf:** add host discrete-event model of the dispatcher superloop ([19626df](https://github.com/jagoda/meshcadet/commit/19626dfbddf6cac8f9a071dc71cd9b324d740fa7))
* **perf:** host discrete-event model of the dispatcher superloop ([8f37a4d](https://github.com/jagoda/meshcadet/commit/8f37a4d2f31e8744a6d5bef8e4b2494992c82fa3))
* **perf:** log free internal-heap headroom in the 30s diagnostics rollup ([65dbdd9](https://github.com/jagoda/meshcadet/commit/65dbdd995ac0f6ce9edb22fb4cf29e3e4408102b))
* **perf:** M1 task-split host validation — as-built loop model, parity matrix, kit regen ([0c72fe9](https://github.com/jagoda/meshcadet/commit/0c72fe987517a5e888c65ffc8bf2648cf0dc5a59))
* **protocol/firmware-core/ui_sim:** normalize inbound emoji so VS16/skin-tone/ZWJ no longer render blank cells ([29cec59](https://github.com/jagoda/meshcadet/commit/29cec59a0dbaf915110379359d1ccd82806f9e05))
* **protocol/firmware-core/ui_sim:** normalize inbound emoji so VS16/skin-tone/ZWJ no longer render blank cells ([1abad3e](https://github.com/jagoda/meshcadet/commit/1abad3e8e419d6aec1c9871091d2c3a03f025ef6))
* **radio:** land D9/D11 SPI2 bus-hold GPIO-toggle probe in radio.rs ([7e8dfc8](https://github.com/jagoda/meshcadet/commit/7e8dfc84d8b0e2e7bc05b7e1fe43fcfeba81dcb9))
* **ui:** render the emoji picker's 96 grid cells in full color ([9bf1a82](https://github.com/jagoda/meshcadet/commit/9bf1a8234c408e157311e83d96c86bb41a7c133a))
* **ui:** render the emoji picker's 96 grid cells in full color ([8684097](https://github.com/jagoda/meshcadet/commit/8684097c44a1e9ada2808c0672178adfad458863))
* **xtask:** add static guard for ADR-0012 R8 Slint thread-affinity barrier ([b4af00c](https://github.com/jagoda/meshcadet/commit/b4af00cd9f37ee4833beedbfca91543cac2eaba7))
* **xtask:** make ADR-0012 R8's Slint thread-affinity barrier mechanical ([e007445](https://github.com/jagoda/meshcadet/commit/e007445e3087a76cc9ecd2307c4165fcbf76020b))
* **xtask:** rewrite the picker/render sync guard from equality to subset ([aa550af](https://github.com/jagoda/meshcadet/commit/aa550afa122e5499b37a9daac5385be20e1e68f2))


### Fixed

* **ci:** run cargo fmt on splash_backdrop_steal_probe.rs ([2f02b3e](https://github.com/jagoda/meshcadet/commit/2f02b3e254d4b2783e52391272cd4715910d6015))
* **ci:** scrub internal doc-path leak and apply cargo fmt ([f9fd86a](https://github.com/jagoda/meshcadet/commit/f9fd86a4d86fdbfd0939922bde58e0931c952079))
* **docs:** drop internal-ops term from ui-residual-opt-r1.md ([d4b23d5](https://github.com/jagoda/meshcadet/commit/d4b23d52175cec04c62fe5730b7b4de7fec72b50))
* **firmware:** add missing PerfRollup.ui_step field; scrub vocabulary leak ([aa0a1a4](https://github.com/jagoda/meshcadet/commit/aa0a1a4b60f76a594d8cf19bc44ffdae4d075473))
* **firmware:** bundle ui_task::spawn's peripheral params behind one Box ([8df94bd](https://github.com/jagoda/meshcadet/commit/8df94bd52b5030f222bcffa7de9049b05be207a7))
* **firmware:** bundle ui_task::spawn's peripheral params behind one Box ([5070c35](https://github.com/jagoda/meshcadet/commit/5070c354c789443c541060aed26deda149605c1c))
* **firmware:** derive Clone for BootSeed ([af86319](https://github.com/jagoda/meshcadet/commit/af863193304b85a2275678b7d6688a0943b1e241))
* **firmware:** drive emoji wght axis + alpha gamma boost for crisp mono glyphs ([d2ce8ea](https://github.com/jagoda/meshcadet/commit/d2ce8ea0d23db767d271125039c4850afe1213fc))
* **firmware:** drive emoji wght axis + alpha gamma boost for crisp mono glyphs ([3f2400d](https://github.com/jagoda/meshcadet/commit/3f2400d9da27c86382973de21d44ff6f7ad21427))
* **firmware:** recompute app-image baseline against main, correct its method provenance ([dcc8242](https://github.com/jagoda/meshcadet/commit/dcc82429c2bbcdccc7bc909341cff94c9f858864))
* **firmware:** recompute app-image baseline against main, correct its method provenance ([c0a45c0](https://github.com/jagoda/meshcadet/commit/c0a45c047a2340158a12e20cfc20385dcd72575c))
* **protocol:** drop internal-ops term from emoji.rs doc comment ([2b2cf8e](https://github.com/jagoda/meshcadet/commit/2b2cf8eef370df9b867c94ad3bc3c7d9fbaef1cd))
* **provisioner:** make generated channel secrets recoverable before add ([d94c84e](https://github.com/jagoda/meshcadet/commit/d94c84e56053f66670d712caa44b573a9fc45e7f))
* **provisioner:** make generated channel secrets recoverable before add ([9e3f66e](https://github.com/jagoda/meshcadet/commit/9e3f66e8783f9294638eae09c0bcd07bf1ae1f12))
* **radio:** re-check DIO1 level after a wake before reporting Asserted ([5bcb3d8](https://github.com/jagoda/meshcadet/commit/5bcb3d8921eab5b0c7e1b5341d01ad93138aa499))
* **radio:** re-check DIO1 level after a wake before reporting Asserted ([251e53b](https://github.com/jagoda/meshcadet/commit/251e53bedbc92ce221c7aa9f38b1c5bd29099a31))
* **release:** parse a multi-line members = [...] array in sync-cargo-lock-versions.sh ([5d97014](https://github.com/jagoda/meshcadet/commit/5d970144ce7aa3cc8f9a9be085e1e8b4a29296ae))
* **release:** parse a multi-line members array in sync-cargo-lock-versions.sh ([a3574e1](https://github.com/jagoda/meshcadet/commit/a3574e1fc010e1f33ffa7401182b5fdbccb9ba65))
* **ui:** dedupe the 7x embedded starfield backdrop texture ([9637838](https://github.com/jagoda/meshcadet/commit/9637838021a2678fc5426962e4092a8f381fdc38))
* **ui:** dedupe the 7x embedded starfield backdrop texture ([7d73a41](https://github.com/jagoda/meshcadet/commit/7d73a416fef949b534404cde93418079dc307f3b))
* **ui:** guard render_if_needed against Slint's unset-component boot panic ([9e20f11](https://github.com/jagoda/meshcadet/commit/9e20f112a26366097f8cfaf3b61b20502e95aa84))
* **ui:** guard render_if_needed against Slint's unset-component panic ([689691d](https://github.com/jagoda/meshcadet/commit/689691d911701ea0f0d62ec7b18fb1331046d53d))
* **ui:** warm the shared backdrop-image cache before the boot splash's own component ([e7f7235](https://github.com/jagoda/meshcadet/commit/e7f7235d0cd31d94614e481a124824e7618922bc))
* **ui:** warm the shared backdrop-image cache before the boot splash's own component ([4f5f330](https://github.com/jagoda/meshcadet/commit/4f5f330ab9995ea403766bf86c306b4c4479f7fc))
* **xtask:** drop inherited RUSTUP_TOOLCHAIN before nested firmware release build ([7600fcb](https://github.com/jagoda/meshcadet/commit/7600fcbf06ac0d2afa1bcdaf94d3598d4c556f2c))
* **xtask:** fail loud instead of silently skipping an unreadable directory in the Slint-affinity guard ([1a3084c](https://github.com/jagoda/meshcadet/commit/1a3084cbdcc74d1de4dd179e9fc6f7bd2e8f44d9))


### Performance

* **device-report-ingest:** build the collection-kit ingest path (no data yet) ([9be53e1](https://github.com/jagoda/meshcadet/commit/9be53e1b21eeb435e679752e864ea03d8b604d62))
* **diagnostics:** log free internal-heap headroom (closes ADR-0012 D-H) ([5ddafdd](https://github.com/jagoda/meshcadet/commit/5ddafdd3cfa10eead61a8b2dc368d4b00c818200))
* land D9/D11 SPI2 GPIO-toggle probe in radio.rs ([d1654b0](https://github.com/jagoda/meshcadet/commit/d1654b0eae505881af1bf454dd2d6541313dfbab))
* M1 task-split host validation — as-built loop model, parity matrix, kit regen ([f278ed0](https://github.com/jagoda/meshcadet/commit/f278ed0a2dca81e71417746da6506396eebe6304))
* **M3:** re-rank the residual UI-side items post-split — both closed/demoted, no optimization landed ([c58d617](https://github.com/jagoda/meshcadet/commit/c58d617b99263323c9bdd735b6d0132751e29b02))
* **radio:** interrupt/notification-driven DIO1 waits (M2) ([e11c4a0](https://github.com/jagoda/meshcadet/commit/e11c4a073578f239ab97ef7d874b4af5008e9976))
* **radio:** M2 host validation — DIO1 wait quantization, wait-abstraction state machine, ISR-safety audit ([f7a8c10](https://github.com/jagoda/meshcadet/commit/f7a8c1029fcf26edb93a5ad31b872de617bab916))
* **radio:** M2 host validation — DIO1 wait quantization, wait-abstraction state machine, ISR-safety audit ([896aa7c](https://github.com/jagoda/meshcadet/commit/896aa7c06ab3493da0339a07dccaae7a2bfc7638))
* **radio:** post-green review — observability log + stale-doc corrections ([458441c](https://github.com/jagoda/meshcadet/commit/458441c40dcf2fb454f554e02d4453edc802845b))
* **radio:** replace DIO1 spin-polls with interrupt/notification-driven waits ([7c3ee48](https://github.com/jagoda/meshcadet/commit/7c3ee48c9ddcd9b5bf22212871cc37367a3bb766))


### Changed

* **perf-device-report:** dedupe payload_bytes field formatting ([532141a](https://github.com/jagoda/meshcadet/commit/532141afa2eaa0bbf075bd894c73b97c3a5a1d37))
* **perf:** tighten loop-model report + document backoff simplification ([6aa7290](https://github.com/jagoda/meshcadet/commit/6aa72900ead6cd91bf1683aaa913270b35abf7b3))


### Documentation

* **adr:** ADR-0012 dispatcher/UI task split design ([2a64163](https://github.com/jagoda/meshcadet/commit/2a64163eaff6e14e87ad0df73efd0b94eea256b1))
* **adr:** ADR-0012 dispatcher/UI task split design ([5a878ca](https://github.com/jagoda/meshcadet/commit/5a878ca5e25a5e87d5abd1991abe756f3698adb4))
* **adr:** sharpen ADR-0012's Sync auto-derivation argument ([d5e39e4](https://github.com/jagoda/meshcadet/commit/d5e39e4b1ad029894519772bd734fedb444c816c))
* **firmware:** correct ADR-0012 R8's mechanical-enforcement claim and stale SPI2 comment ([ca4d8bf](https://github.com/jagoda/meshcadet/commit/ca4d8bf7da9288dd14a7024464301eabb25ed1df))
* **firmware:** fix stale equality claim in EMOJI_CPS's own local comment ([ddaf1b9](https://github.com/jagoda/meshcadet/commit/ddaf1b9550f1ad2c7881d7785cb4f337c3c66b79))
* **firmware:** note expected run-to-run noise in the app-image baseline ([54909c9](https://github.com/jagoda/meshcadet/commit/54909c967ad6f116968a1b820bca532e276219c5))
* **perf:** author on-device performance collection kit (M0) ([710a7d1](https://github.com/jagoda/meshcadet/commit/710a7d1e472d02d88cea441aabd72e4a88a1927c))
* **perf:** author the on-device performance collection kit (M0) ([1e75ee8](https://github.com/jagoda/meshcadet/commit/1e75ee83c4929cbd8f47fb15cd9f8b6a87e45964))
* **perf:** consolidate ui-perf-baseline into one provenance-tagged record ([d7d6bc2](https://github.com/jagoda/meshcadet/commit/d7d6bc282daeb6fce0f05f178195f9d7ebbf6573))
* **perf:** consolidate ui-perf-baseline into one provenance-tagged record ([493ced6](https://github.com/jagoda/meshcadet/commit/493ced604f8962c03286245974b9ae3ab30c42e5))
* **perf:** correct display SPI-line-floor comment and re-anchor ui_step ([d5069cc](https://github.com/jagoda/meshcadet/commit/d5069ccdf58a6a16916a20d0ccb92cbe1f8dfbef))
* **perf:** correct the 10x-low display SPI floor and its derived numbers ([b5f12af](https://github.com/jagoda/meshcadet/commit/b5f12af319a81708738c6489b7419b615f0029a4))
* **perf:** document the device-report archive schema ([ac86d9b](https://github.com/jagoda/meshcadet/commit/ac86d9b3a10201ffc0b8673b182c6a8a50f76f84))
* **perf:** drop internal-ops vocabulary from device-report ingest docs ([fa324b7](https://github.com/jagoda/meshcadet/commit/fa324b75b008635564827d77e424e35ae9c609a2))
* **perf:** drop internal-ops vocabulary from SPI2 arbitration doc ([5587936](https://github.com/jagoda/meshcadet/commit/55879362546e716b506dac50e32b07c4838eba81))
* **perf:** drop internal-ops vocabulary from task-split host validation doc ([501df74](https://github.com/jagoda/meshcadet/commit/501df74a192bcd2da6da3d3b85f8b701941aa098))
* **perf:** fix collection-kit.md ref check, CLI naming, and port fallback ([5259e81](https://github.com/jagoda/meshcadet/commit/5259e81b61847f828672413b89cb98d7a1ea2f07))
* **perf:** fix collection-kit.md's dead ref-check grep, vague CLI ref, and broken --port fallback ([8199b5d](https://github.com/jagoda/meshcadet/commit/8199b5ddec6d60307cb1af03981646ed20d5e6a8))
* **perf:** land the performance review — one state-of-record document ([3fa905f](https://github.com/jagoda/meshcadet/commit/3fa905fe4b665d13489a05ade408bd29cfd43cf5))
* **perf:** M3 re-ranking — both residual UI items closed/demoted, no optimization landed ([ec2d5e4](https://github.com/jagoda/meshcadet/commit/ec2d5e492d14fb0788c2b3be2bffdfcffd29a469))
* **perf:** recompute ui-perf-baseline.md SPI-floor table and derived claims ([8ee62aa](https://github.com/jagoda/meshcadet/commit/8ee62aa7b96c698161679953e1354c76a5dc2a3f))
* **perf:** record the UI_TICK_MS / RENDER_MIN_INTERVAL_MS coupling at both constants ([738b7e0](https://github.com/jagoda/meshcadet/commit/738b7e0a7dae9054f8c2f4c967e16bcb91daa52d))
* **perf:** regenerate perf-loop-model-baseline.md report with corrected ui_step ([efd7cf9](https://github.com/jagoda/meshcadet/commit/efd7cf9b808e87a394db5f4968ea2b15a0915883))
* **perf:** repoint code comments at the renumbered baseline sections ([3db0f38](https://github.com/jagoda/meshcadet/commit/3db0f38b1c8f8d519a260dc8641dadfbb7ac4a11))
* **perf:** resolve ADR-0012's D-A..D-H labels into the one register ([757b36f](https://github.com/jagoda/meshcadet/commit/757b36fcd4fc2b787b6d30b877447bd117aa8d16))
* **perf:** resolve M0 checkpoint's R2 doc-consistency findings ([5f89284](https://github.com/jagoda/meshcadet/commit/5f89284c2a2dc0598b8c27a887e7baf7879fb445))
* **perf:** resolve M0 checkpoint's R2 doc-consistency findings ([fd6c1cc](https://github.com/jagoda/meshcadet/commit/fd6c1cc73ab77ab6ef2ea82bdd80e0ae3158ee65))
* **perf:** rewrite the performance record as the post-review state of record ([68c520b](https://github.com/jagoda/meshcadet/commit/68c520b8f921d9a11889375fa38158207bf76a1d))
* **perf:** settle R1 (SPI2 bus arbitration) by static analysis ([bed3bc3](https://github.com/jagoda/meshcadet/commit/bed3bc3fb396bccf44dc398a18495e4e9e0ad4d9))
* **perf:** settle R1 (SPI2 bus arbitration) by static analysis ([306b556](https://github.com/jagoda/meshcadet/commit/306b55629b4cfa9cc7cbc3dc734a8ea004e4ad1e))
* **perf:** strip internal-ops vocabulary from radio host-validation doc ([085307e](https://github.com/jagoda/meshcadet/commit/085307ed6ba0be777e98ef3b973af0b2ec55cb8b))
* **perf:** unblock D9/D11 now that the SPI2 GPIO-toggle probe exists ([94759a5](https://github.com/jagoda/meshcadet/commit/94759a5f472ed8f2e045896e899dad490f2f42bd))
* **radio:** rewrite internal-ops term in DIO1 wait-safety doc comment ([8269bd0](https://github.com/jagoda/meshcadet/commit/8269bd0ed48dba33af2bca1acd9fc4f19d8718c8))

## [0.5.0](https://github.com/jagoda/meshcadet/compare/v0.4.0...v0.5.0) (2026-08-01)


### Added

* **firmware-core:** room-client session state machine + ui_sim proofs ([fa5f44c](https://github.com/jagoda/meshcadet/commit/fa5f44ca8f93343cf5ba7aaa8928053ea0330199))
* **firmware-core:** room-session logic for post/keep-alive/notification phases ([209bc44](https://github.com/jagoda/meshcadet/commit/209bc4470927a233f5b0dd7ffdc8ad6d4adf7c28))
* **firmware:** rename Messages/Channels tabs to Contacts/Groups ([8417a73](https://github.com/jagoda/meshcadet/commit/8417a731aade0ff48926f769e286671ffbfcbf8b))
* **firmware:** rename Messages/Channels tabs to Contacts/Groups ([504782f](https://github.com/jagoda/meshcadet/commit/504782f304352b98442e6fcc7fd3f701fdd6d15a))
* **firmware:** rename Messages/Channels tabs to Contacts/Groups ([4d982dc](https://github.com/jagoda/meshcadet/commit/4d982dcafe2deaa296414213446fc3ae83bc9c06))
* **firmware:** room post, permission-gated compose, keep-alive, notification suppression ([75ff5f8](https://github.com/jagoda/meshcadet/commit/75ff5f8791acde16b8681237422428c399fd16a9))
* **firmware:** wire device-side handlers for room provisioning frames ([be228fa](https://github.com/jagoda/meshcadet/commit/be228fa6347665c48394c75ff6997efbe10f58a2))
* **firmware:** wire device-side handlers for room provisioning frames ([1611a78](https://github.com/jagoda/meshcadet/commit/1611a7863f2468f59ee99ec88774bd2efd097182))
* **firmware:** wire post/keep-alive/notification-phase logic into runtime ([9a74d4f](https://github.com/jagoda/meshcadet/commit/9a74d4f7541a1938bab0466ff066ee807077260f))
* **firmware:** wire the room-client login/read session state machine ([f9796fa](https://github.com/jagoda/meshcadet/commit/f9796fabedc5e023f4ef426cc31632da0ec6c560))
* **firmware:** wire the room-client session state machine into main.rs ([a0ee754](https://github.com/jagoda/meshcadet/commit/a0ee754c83d2b0713435084f9f4047601a0bb866))
* **host:** add room URI builder and round-trip parser ([6f130f4](https://github.com/jagoda/meshcadet/commit/6f130f451aef109951abe661d111424979c5eeff))
* **host:** add room-server provisioning verbs ([8ea4fc5](https://github.com/jagoda/meshcadet/commit/8ea4fc568a7faa64481152cf43b1490768df9ea7))
* **host:** add room-server provisioning verbs to admin CLI ([82223ce](https://github.com/jagoda/meshcadet/commit/82223ce86ad9708a88bec3cfb8e694f1e364679f))
* **protocol,firmware,host,site:** room provisioning storage+wire+URI contract ([78b3a2b](https://github.com/jagoda/meshcadet/commit/78b3a2b448803b3f98f4fc5aab40b8627a96c663))
* **protocol:** add room provisioning frames ([d37e20d](https://github.com/jagoda/meshcadet/commit/d37e20d828dcf9dbf3c22370cae166a3b40b68fc))
* **protocol:** add room-server client codec ([c08a45a](https://github.com/jagoda/meshcadet/commit/c08a45a39179b80424df1db6a1682defb0f30082))
* **protocol:** add room-server client codec ([25916d0](https://github.com/jagoda/meshcadet/commit/25916d0e600c4fa1a1bb182221fc014512674d90))
* **room:** adopt room server's clock as a trusted wall clock ([1d89529](https://github.com/jagoda/meshcadet/commit/1d89529fbf3b9132f21ebad77ab53b912c2b7553))
* **room:** adopt room server's clock as a trusted wall clock ([cd5d462](https://github.com/jagoda/meshcadet/commit/cd5d4621b36f12751fc10cdc1a2e4601939c8354))
* **site:** mirror room provisioning frames in the JS codec ([857b604](https://github.com/jagoda/meshcadet/commit/857b604caa60eb6ef9cc276b05dddd555148bc46))
* **site:** mirror room provisioning into the browser provisioner + admin-PIN hygiene backfill ([12cd99f](https://github.com/jagoda/meshcadet/commit/12cd99f7d04069be883eb8c3379f50e365986d0c))
* **site:** mirror room-server provisioning into the browser provisioner ([57d31e7](https://github.com/jagoda/meshcadet/commit/57d31e7ee33b53e9bfca7266cc1ad7d987bbfb91))


### Fixed

* **ci:** mirror dispatched releases onto the Pages site ([99827b7](https://github.com/jagoda/meshcadet/commit/99827b7cd9cebd3d365eea6f149e9f3245faf0fe))
* **ci:** mirror dispatched releases onto the Pages site ([8dc2fe1](https://github.com/jagoda/meshcadet/commit/8dc2fe150297023c24f13e29bfbc2c143eb9a6e2))
* **ci:** scrub internal-ops doc-path references from public code comments ([9fd4d7b](https://github.com/jagoda/meshcadet/commit/9fd4d7b9acd63cb17fe13b35e57c45f3dbddd6e3))
* **firmware:** add persisted per-contact inbound replay guard for handle_dm ([b04e53b](https://github.com/jagoda/meshcadet/commit/b04e53bf5a748744afd943b4026581c7cb0bf98d))
* **firmware:** add persisted per-contact inbound replay guard for handle_dm ([7a27918](https://github.com/jagoda/meshcadet/commit/7a27918b179c8e4168f900e0afdc134c9e5f6b08))
* **firmware:** add room-UI glyphs to bundled font coverage ([1649368](https://github.com/jagoda/meshcadet/commit/164936889663b57586b596d3e16decb77e3252f5))
* **firmware:** bound room drain window with a stall timeout ([ebde9d9](https://github.com/jagoda/meshcadet/commit/ebde9d91d443a00f54eae52268f7112c3f2b8d12))
* **firmware:** bound room drain window with a stall timeout ([9d847ba](https://github.com/jagoda/meshcadet/commit/9d847ba2e3af7c83c023ac5e73e21eea4ecc2102))
* **firmware:** close zero-channel GRP_TXT RX/TX hole ([e1f900a](https://github.com/jagoda/meshcadet/commit/e1f900aeb6bfe8f269d42d24d88332d378d23802))
* **firmware:** close zero-channel GRP_TXT RX/TX hole ([3fd28e3](https://github.com/jagoda/meshcadet/commit/3fd28e38dc7ad1988a1a7a7adb60641d65823409))
* **firmware:** decode keep-alive ACK through the validated decoder ([9e883ba](https://github.com/jagoda/meshcadet/commit/9e883bab73758e37ec946b47bb4dbfab49dae9a9))
* **firmware:** decouple room re-flood-login cadence from the drain cadence ([0b48392](https://github.com/jagoda/meshcadet/commit/0b483920d49d03f47bc44e9c5872195328c66917))
* **firmware:** decouple room re-flood-login cadence from the drain cadence ([8d02c5b](https://github.com/jagoda/meshcadet/commit/8d02c5b6973c6a1c653e92f578b210a2c903219f))
* **firmware:** durably erase the room session store on DEL_ROOM/ADD_ROOM ([6fb39ec](https://github.com/jagoda/meshcadet/commit/6fb39ec8b1f02a2ad3fe36e9c3224d11e4a2e65d))
* **firmware:** durably erase the room session store on DEL_ROOM/ADD_ROOM ([82acdc5](https://github.com/jagoda/meshcadet/commit/82acdc544c63f1b22c0be1bd10f567b684cc5d5a))
* **firmware:** erase room session store on ADD/DEL_ROOM, document DEL_ROOM's reboot-required posture ([0755231](https://github.com/jagoda/meshcadet/commit/07552315a4eaa3cb93e71f775847478b5099fdcf))
* **firmware:** erase the room session store on ADD/DEL_ROOM, document DEL_ROOM's reboot-required posture ([55940f5](https://github.com/jagoda/meshcadet/commit/55940f5637cc4ba2a1ec4d28f16835d572a661c9))
* **firmware:** heap-allocate ProvisionedConfig off the admin_server/prov_server thread stacks ([36f26e9](https://github.com/jagoda/meshcadet/commit/36f26e9c70fb583ab49c08ffd7b7dd7d992f1bf5))
* **firmware:** heap-allocate ProvisionedConfig off the admin_server/prov_server thread stacks ([b459e87](https://github.com/jagoda/meshcadet/commit/b459e87c35fa074f66561c1e68a95a3f9fab8a4a))
* **firmware:** keep decode_path_return's PathExtra match exhaustive ([763f6a4](https://github.com/jagoda/meshcadet/commit/763f6a40f0e7ffebaca7e529b9e7a1da223ef288))
* **firmware:** log TX-queue eviction, keep sync_since monotonic, and fix room-push DM fallthrough ([73fcde0](https://github.com/jagoda/meshcadet/commit/73fcde00d904a0be5c76ae6efe289d6466e7a7b0))
* **firmware:** persist room TX watermark on every advancing send ([350cc6f](https://github.com/jagoda/meshcadet/commit/350cc6fd4c079ad16dc4f0e4361eff62922fcf37))
* **firmware:** persist room TX watermark on every advancing send ([a56c7b7](https://github.com/jagoda/meshcadet/commit/a56c7b7a586b7de539c95513e167ff6d0b68a101))
* **firmware:** propagate runtime-learned room session state to the UI and keep-alive scheduler ([b2ecc9b](https://github.com/jagoda/meshcadet/commit/b2ecc9b132661e540e89377e34cc31b3d4a6075c))
* **firmware:** propagate runtime-learned room session state to the UI and keep-alive scheduler ([89d4deb](https://github.com/jagoda/meshcadet/commit/89d4deb59c64716eaae9a9918818c322a841e618))
* **firmware:** re-stamp room posts with the server's own clock, not the sender's ([1f47f6b](https://github.com/jagoda/meshcadet/commit/1f47f6beb7d50b7b3cd9a2ddfd6b19dcd3ef78b1))
* **firmware:** re-stamp room posts with the server's own clock, not the sender's ([46f103f](https://github.com/jagoda/meshcadet/commit/46f103f80421badea2121d8d9476c18e8c5b3aef))
* **firmware:** reconnect-stall detector zeroes out_path to relearn a changed route ([33c048b](https://github.com/jagoda/meshcadet/commit/33c048beadd36729f2e8fe3e24bdb002e92cb769))
* **firmware:** relearn room out_path after keep-alive stall so reconnect recovers without reboot ([3bbf63d](https://github.com/jagoda/meshcadet/commit/3bbf63db87f5f0f3b9155b368a5ae3be5a62b8e7))
* **firmware:** render room-post bubble only on confirmed send; surface refusals ([d6abd3e](https://github.com/jagoda/meshcadet/commit/d6abd3e966a515c86697fc4bcf8154a14fac6a4a))
* **firmware:** render room-post bubble only on confirmed send; surface refusals ([cdde8cf](https://github.com/jagoda/meshcadet/commit/cdde8cfff0ba7e99b5206206bca3d44ed4f09e77))
* **firmware:** render room-post senders like channels, and stop clock-sync poisoning from bricking room posts ([e1edc10](https://github.com/jagoda/meshcadet/commit/e1edc1064642fbcc1ed88d4b2e0f1c86b031a8b0))
* **firmware:** render room-post senders like channels, stop clock-sync poisoning ([390af99](https://github.com/jagoda/meshcadet/commit/390af99d4f927e14273976cddf8291b20db5e163))
* **firmware:** room-scoped monotonic TX timestamp, never random ([b3eda3c](https://github.com/jagoda/meshcadet/commit/b3eda3c47558d32efaf215f292709d8da728739d))
* **firmware:** room-scoped monotonic TX timestamp, never random ([db5a579](https://github.com/jagoda/meshcadet/commit/db5a5793226e40ab6c5e8b9c92e71ba836ea58d8))
* **firmware:** room-session observability and watermark robustness fixes ([994d59e](https://github.com/jagoda/meshcadet/commit/994d59e58a6c30f1f697275770f3af57d348afca))
* **firmware:** surface RoomPostRefused when the read-only recheck blocks a send ([e9a7536](https://github.com/jagoda/meshcadet/commit/e9a7536bfd262491e6ee7686a1cdc48985c200c9))
* **firmware:** surface RoomPostRefused when the read-only recheck blocks a send ([b9096ea](https://github.com/jagoda/meshcadet/commit/b9096ea1dd65e0ca2cfd86efbbd9bf864dd2696e))
* **firmware:** update handle_ack test call site for new room_runtime param ([68264b1](https://github.com/jagoda/meshcadet/commit/68264b1c4cb2a1a38bf31aa6ba6b79f2dffe8f04))
* **history:** port TIMESTAMP_UNKNOWN sentinel to the JS transcript renderer ([1e62b2c](https://github.com/jagoda/meshcadet/commit/1e62b2cefef4d92fa74a23cbb453aff44ae0d0bd))
* **history:** port TIMESTAMP_UNKNOWN sentinel to the JS transcript renderer ([a773d6b](https://github.com/jagoda/meshcadet/commit/a773d6bad917d1938fe37510a786487ae8ad1a6c))
* **host:** add gen-channel-secret CSPRNG generator + weak-pattern warning ([096da43](https://github.com/jagoda/meshcadet/commit/096da43be71086b7b783e153dcbd3fe829e7c5f2))
* **host:** gen-channel-secret CSPRNG generator + stdin-only composition ([9af8e70](https://github.com/jagoda/meshcadet/commit/9af8e7041a05f11a2fb3edbb69740d310f3461b6))
* **host:** move channel-secret and admin-PIN CLI args off argv ([a8a55cf](https://github.com/jagoda/meshcadet/commit/a8a55cf611d359ffd74954c77482fda3d6ad27b6))
* **host:** move channel-secret and admin-PIN CLI args off argv ([35fca60](https://github.com/jagoda/meshcadet/commit/35fca608f6a15999d99076331545a437ac88e8cf))
* **host:** repoint gen-channel-secret composition at --secret-stdin, not argv ([50c7d69](https://github.com/jagoda/meshcadet/commit/50c7d6942317e38b72adb07c037c34d4563bf038))
* **room:** bound room-clock plausibility at adoption, not repair-at-load ([0674b55](https://github.com/jagoda/meshcadet/commit/0674b55762d2edff1877435f5cfbf050a615ca3f))
* **room:** bound room-clock plausibility at adoption, not repair-at-load ([79fe3d0](https://github.com/jagoda/meshcadet/commit/79fe3d089cf9772cabae846b5271eda08eefcfe0))
* **room:** don't overwrite a still-outstanding keep-alive ack before its reply window closes ([25dae32](https://github.com/jagoda/meshcadet/commit/25dae328c51dbf1e58fc96bf429f5dcdbba6dc47))
* **room:** don't overwrite a still-outstanding keep-alive ack before its reply window closes ([174a2d8](https://github.com/jagoda/meshcadet/commit/174a2d8482ee3f28a1693d4d1a9513787cc332af))
* **room:** flush a stalled drain window on the scheduler's own periodic tick ([f2963e1](https://github.com/jagoda/meshcadet/commit/f2963e1c03e6a26451703a33094e38680e729e19))
* **room:** flush a stalled drain window on the scheduler's own periodic tick ([a5ef478](https://github.com/jagoda/meshcadet/commit/a5ef4788c44d13fe743b778ae47a9c171df0783f))
* **room:** flush an already-absorbed drain backlog immediately on closer failure ([8ae4675](https://github.com/jagoda/meshcadet/commit/8ae46753e7881ccb71bbe7815f7f6a9f35e92d5d))
* **room:** flush an already-absorbed drain backlog on closer failure ([7059ec7](https://github.com/jagoda/meshcadet/commit/7059ec73e344a25fd884cdedc5109300aaab3873))
* **room:** gate reflood backoff reset on an actually-learned route ([6c92c5c](https://github.com/jagoda/meshcadet/commit/6c92c5ccc52427fe56b87e5c83f78f39ace93af2))
* **room:** gate reflood backoff reset on an actually-learned route ([2c1a4d2](https://github.com/jagoda/meshcadet/commit/2c1a4d2c248534d4c76fe9660a7045765f9c2f54))
* **room:** gate reflood-backoff reset on has_route(), not out_path_len ([648b2c8](https://github.com/jagoda/meshcadet/commit/648b2c81a526171d3170b848d7995791487dfb62))
* **room:** gate the reflood-backoff reset on has_route(), not out_path_len ([5fb62ce](https://github.com/jagoda/meshcadet/commit/5fb62cebcd276dde2579df8efd07be57e0e188f9))
* **room:** match a room post's ACK when bundled in a PATH-return ([cdbb443](https://github.com/jagoda/meshcadet/commit/cdbb4432dd8aecd189ec7c19cc869105298d1cb4))
* **room:** match a room post's ACK when it arrives bundled in a PATH-return ([b05007a](https://github.com/jagoda/meshcadet/commit/b05007ab333170af50d5410a95d6694fd0475306))
* **room:** notify for a post whose drain window has no working closer ([82f03fb](https://github.com/jagoda/meshcadet/commit/82f03fb8303b467dd794ee4cbf19d4199c2e1daa))
* **room:** notify for a post whose drain window has no working closer ([0c2652c](https://github.com/jagoda/meshcadet/commit/0c2652c2822f7964d66bed7787a719bb54b5f421))
* **room:** notify on a post-triggered drain-window close, and short-circuit the blind stall bound ([debeb0d](https://github.com/jagoda/meshcadet/commit/debeb0d717a098f9266b9da9cd8139f482ad0d66))
* **room:** notify on a post-triggered drain-window close, and short-circuit the blind stall bound ([57e5677](https://github.com/jagoda/meshcadet/commit/57e567704b94e9d5acd251ade1546b36432b48f6))
* **room:** persist the room-post watermark advance, add a shape-based guard ([9d0866f](https://github.com/jagoda/meshcadet/commit/9d0866f56aed11f9fa274a706c84e87a6f9356cd))
* **room:** persist the room-post watermark advance, add a shape-based guard ([1d141b2](https://github.com/jagoda/meshcadet/commit/1d141b206b4fc9ad371bb7ffe88eb2937a11b615))
* **room:** pre-filter room push/login-reply frames on dest_hash ([3464718](https://github.com/jagoda/meshcadet/commit/34647182c8557cefc519c01bfd21279cc2afbaca))
* **room:** pre-filter room push/login-reply frames on dest_hash ([58ad169](https://github.com/jagoda/meshcadet/commit/58ad16951b7a114fd716b4dd89d79020e99c839b))
* **room:** reopen drain window on relogin; rewind a starved sync_since ([477a129](https://github.com/jagoda/meshcadet/commit/477a1293880b3faa2ebbc261a9027c9e851f9cb8))
* **room:** reopen the drain window on relogin; rewind a starved sync_since ([f37f74a](https://github.com/jagoda/meshcadet/commit/f37f74afe023711b32f610326784cec7264eca69))
* **room:** separate the room-post wire nonce from display time ([599f294](https://github.com/jagoda/meshcadet/commit/599f294fe7d4bcaff626a9146e027553187813cd))
* **room:** separate the room-post wire nonce from display time ([183fb8e](https://github.com/jagoda/meshcadet/commit/183fb8ea5dcfd9ec67ec2d177306a4b74e54fde4))
* **security:** stop echoing channel-secret bytes in host/firmware logs ([9b30310](https://github.com/jagoda/meshcadet/commit/9b3031070167b0fd0a3ed00e879a1d8a69635bed))
* **security:** stop echoing channel-secret bytes in host/firmware logs ([64cb1f1](https://github.com/jagoda/meshcadet/commit/64cb1f15c13bb05d9f1c808567f448d199a4cf9c))
* **site:** bring channel-secret fields up to the PIN/guest-password hygiene contract ([c0760d7](https://github.com/jagoda/meshcadet/commit/c0760d7169dc2092eb32aa37788c8f0bd41a8041))
* **site:** bring channel-secret fields up to the PIN/guest-password hygiene contract ([9661c25](https://github.com/jagoda/meshcadet/commit/9661c25ee26565f3e798aad9e6be2fe062f61405))
* **site:** make the whole provisioner test suite runnable in one command ([cab3f64](https://github.com/jagoda/meshcadet/commit/cab3f64727bda93b7c2ab264d3790a52413718cc))
* **site:** vendor esptool-js instead of CDN-importing it into the flasher ([116e947](https://github.com/jagoda/meshcadet/commit/116e947e50f437e08ab610b174cba51f53a13343))
* **site:** vendor esptool-js instead of CDN-importing it into the flasher ([ae77aa9](https://github.com/jagoda/meshcadet/commit/ae77aa914b6fe05475cac9a517b829c54603a1fd))
* **site:** vendor the QR library and add a Content-Security-Policy to every page ([4afa843](https://github.com/jagoda/meshcadet/commit/4afa843d1754f8fbf82e1aab227bb9dc55c3ba30))
* **site:** vendor the QR library and add a Content-Security-Policy to every page ([22036a4](https://github.com/jagoda/meshcadet/commit/22036a41013b8664492bdecec7beb82eae55c590))
* **ui_sim:** move Channels-tab badge probe pixel off the unread-count glyph ([f9f3dbb](https://github.com/jagoda/meshcadet/commit/f9f3dbb676bcea16744da559b6e92fb24e89765f))
* **ui_sim:** move Channels-tab badge sample off the unread-glyph column ([1aefa70](https://github.com/jagoda/meshcadet/commit/1aefa70f6f7568a0d1d381f082caa317c05370b4))
* **ui:** make room-post ACKs carry is_channel so the live view redraws ([515c285](https://github.com/jagoda/meshcadet/commit/515c285cbedfda564fdd5130bdf17946cffc4cd6))
* **ui:** room-post ACKs carry is_channel so the live view redraws ([f12cab1](https://github.com/jagoda/meshcadet/commit/f12cab14594ae746f701f9be7ef126447221c442))


### Changed

* **firmware-core:** extract config_store codec ([f119f41](https://github.com/jagoda/meshcadet/commit/f119f41ac48caacf6ef4f9e6044d2649354253f6))
* **firmware:** thin config_store.rs to a firmware-core shim ([171f7bb](https://github.com/jagoda/meshcadet/commit/171f7bbd7a20732988fd0a937cdc1aae41b415c6))
* **host:** extract password-truncation warning into a testable helper ([14cc174](https://github.com/jagoda/meshcadet/commit/14cc1742c11d8f66cc1d6bb5e715aea988d5d695))
* **xtask:** surface the parity check in the xtask binary, drop a dead wrapper ([61532ec](https://github.com/jagoda/meshcadet/commit/61532ec8ce3f49b9d5a00d5496178280347d3a63))


### Documentation

* **adr:** bump ADR-0001 protocol target to v1.16, admit room-server role ([6119d31](https://github.com/jagoda/meshcadet/commit/6119d3190744f91cc08176056e63ff2564eebad9))
* **adr:** bump ADR-0001 protocol target to v1.16, admit room-server role ([15d7f18](https://github.com/jagoda/meshcadet/commit/15d7f18e73ceaa2f2f79c08dae00cb8ffad77fd7))
* **adr:** strip internal ops-vocabulary leaks, add CI regression guard ([ab43cd8](https://github.com/jagoda/meshcadet/commit/ab43cd8d296c82524d2e01441a72b830682dc46a))
* **adr:** strip internal ops-vocabulary leaks, add CI regression guard ([ecc1ca0](https://github.com/jagoda/meshcadet/commit/ecc1ca0e8ea960adff028ed85621c2f7a9f7257d))
* correct room-discoverability and truncated-HMAC claims; fix MAC compare ([eea17c9](https://github.com/jagoda/meshcadet/commit/eea17c993feac1f304f62c49831fbb561d58ec43))
* **firmware:** update FINDING G doc to name the new save_room_session call sites ([bf46f27](https://github.com/jagoda/meshcadet/commit/bf46f274c06a9724a74f9a090dc42c9a9e11742f))
* qualify room-discoverability and truncated-HMAC claims ([4afa3e0](https://github.com/jagoda/meshcadet/commit/4afa3e03cb9d94a24ae392f277731620a1291f08))
* record room-provisioning wire format decision in ADR-0002/0005 ([d27eb0d](https://github.com/jagoda/meshcadet/commit/d27eb0d378f1eff4d8daae5fd6c1c3189c6c2eca))
* **site:** document the renamed Contacts/Groups UI and the room-server client flow ([0b0d997](https://github.com/jagoda/meshcadet/commit/0b0d997a663def548886d2cba726717bda8f1760))
* **site:** rename Contacts/Groups + document room-server client flow ([2232227](https://github.com/jagoda/meshcadet/commit/223222749935bf3eea67bc4180d925b4e28e4849))

## [0.4.0](https://github.com/jagoda/meshcadet/compare/v0.3.2...v0.4.0) (2026-07-15)


### Added

* **firmware:** generate signed self-advert card on-device, USB-only ([1ab0346](https://github.com/jagoda/meshcadet/commit/1ab03468f3970a41ac28504fa4f74543e9276667))
* **firmware:** generate signed self-advert card on-device, USB-only ([410c9d3](https://github.com/jagoda/meshcadet/commit/410c9d3ca992d66b441c773939db669454c3c94f))
* **host:** surface both advert-card contact URLs ([42f3746](https://github.com/jagoda/meshcadet/commit/42f37467974e59d3b74448c073f94cf0fb5f823b))
* **host:** surface both contact URLs in CLI via signed self-advert card ([59f9e51](https://github.com/jagoda/meshcadet/commit/59f9e51dc6a88e2b5baf0e29a239049579c7d062))
* **protocol:** add signed self-advert card + wire frames ([3e93a57](https://github.com/jagoda/meshcadet/commit/3e93a57c819fdbfaab0d5ab4962a0a8d0a209693))
* **protocol:** add signed self-advert card wire protocol ([d92760d](https://github.com/jagoda/meshcadet/commit/d92760d65d8d4c0de2c38670ca68b264fa6c0234))
* **site:** show Format B (meshcore-cli card URI) in the web provisioner ([92ab2d5](https://github.com/jagoda/meshcadet/commit/92ab2d5c7a473dae7487a2aaa8cef6873eca1cba))
* **site:** show Format B (meshcore-cli card URI) in the web provisioner ([c72fd06](https://github.com/jagoda/meshcadet/commit/c72fd06089e25c724fcc5d3a80247141ba26e646))


### Fixed

* correct stale v1.16 ACK breaking-change claim ([52d73a9](https://github.com/jagoda/meshcadet/commit/52d73a98452bc847fa58ac38707ae8cfe4abc6e0))
* correct stale v1.16 ACK breaking-change claim ([3689db5](https://github.com/jagoda/meshcadet/commit/3689db55b76fa1cef0322b5ddad53ad2f8f257da))
* discard stale response residue at each command's start ([99a4fb8](https://github.com/jagoda/meshcadet/commit/99a4fb81cae6cea2cd75d3d682edb4ad39593fae))
* discard stale response residue at each command's start ([7b2aada](https://github.com/jagoda/meshcadet/commit/7b2aada77bde981c738a7cd5da701c48a9bb8a92))
* **firmware:** repoint app timestamps at GPS-synced wall-clock time ([ff29cec](https://github.com/jagoda/meshcadet/commit/ff29cecfc9f3b905810f188e99516d1bbb78f873))
* **firmware:** repoint app timestamps at GPS-synced wall-clock time ([4d03327](https://github.com/jagoda/meshcadet/commit/4d0332710e340a61a255962328c8567b4a10710d))
* **firmware:** satisfy fmt and clippy on GPS-sync test code ([04bda8a](https://github.com/jagoda/meshcadet/commit/04bda8a33dea7c572c1253151b48e73324c8c256))
* **firmware:** split GPS time-sync row into two lines ([2b9434b](https://github.com/jagoda/meshcadet/commit/2b9434b03db7b211445955971d4a26314da51e60))
* **firmware:** split GPS time-sync row into two lines ([f7cb0aa](https://github.com/jagoda/meshcadet/commit/f7cb0aa8938c541888dbc22617767baecd2be674))
* **host:** correct meshcore-cli import command spelling in user-facing text ([7471a00](https://github.com/jagoda/meshcadet/commit/7471a002bdf57fd5e4b735c9423ed5107d2450af))
* **host:** correct meshcore-cli import command spelling in user-facing text ([cf72f30](https://github.com/jagoda/meshcadet/commit/cf72f300e90e76bd9ef0ea05d5a65726eed95043))
* keep the contacts-list Remove button label on one line ([e777b2e](https://github.com/jagoda/meshcadet/commit/e777b2e02b1d98db2b2bfc063fd238af1606277a))
* keep the contacts-list Remove button label on one line ([c5d9303](https://github.com/jagoda/meshcadet/commit/c5d930356a2602829cb24f505b11e2d2b7a13c23))
* **release:** dispatch release.yml explicitly so tags actually build ([9af9257](https://github.com/jagoda/meshcadet/commit/9af9257fc9da6ba2bf4c4c5da523d26a6e648780))
* **release:** dispatch release.yml explicitly so tags actually build ([cd6660e](https://github.com/jagoda/meshcadet/commit/cd6660ea67c1f01b8f863c4a824b7f69da72135a))
* **site:** discard mid-wait stray frames so advert query can't desync the read chain ([6db9222](https://github.com/jagoda/meshcadet/commit/6db922241b8587a9d564583a2ccbb690582fd65e))
* **site:** discard mid-wait stray frames so advert query can't desync the read chain ([44b87c7](https://github.com/jagoda/meshcadet/commit/44b87c7038b009f421da525b4040594b4eb8ebd8))

## [0.3.2](https://github.com/jagoda/meshcadet/compare/v0.3.1...v0.3.2) (2026-07-14)


### Fixed

* **release:** let release-please complete its tag+label hand-off ([80b9cff](https://github.com/jagoda/meshcadet/commit/80b9cffeb79f2d713c2000daf5c81a88f5c54e6f))
* **release:** let release-please complete its tag+label hand-off ([7065f88](https://github.com/jagoda/meshcadet/commit/7065f88cdd77fde230433da664869278fa691c1a))

## [0.3.1](https://github.com/jagoda/meshcadet/compare/v0.3.0...v0.3.1) (2026-07-14)


### Fixed

* **release:** re-seed layout-baseline.txt to match generate-update-meta.sh's real hashing convention ([c104e9c](https://github.com/jagoda/meshcadet/commit/c104e9c1204e4228a31671d16264099275a9586f))
* **release:** re-seed layout-baseline.txt to match generate-update-meta.sh's real hashing convention ([69c95e9](https://github.com/jagoda/meshcadet/commit/69c95e96338eac46d50e5f05bd748340ef4973a2))
* **site:** convert flasher images to binary strings before esptool-js writeFlash ([d678abc](https://github.com/jagoda/meshcadet/commit/d678abc99b0a1653ea5f9a71143bb5bbb0697271))
* **site:** convert flasher images to binary strings before esptool-js writeFlash ([bfda9d4](https://github.com/jagoda/meshcadet/commit/bfda9d47f31a649eadefa459cd6f16ab322d101d))
* **site:** mirror release assets promptly + fix the web flasher's Fresh-install flow ([abbb8ea](https://github.com/jagoda/meshcadet/commit/abbb8eaf138b9e1d70c42a70274bd199e95019ca))
* **site:** mirror release assets promptly + fix the web flasher's Fresh-install flow ([53f917c](https://github.com/jagoda/meshcadet/commit/53f917cac4de831e65e302834092f45b8c997413))

## [0.3.0](https://github.com/jagoda/meshcadet/compare/v0.2.0...v0.3.0) (2026-07-14)


### Added

* **firmware-core:** add repeater signal-strength tracker ([474c4cf](https://github.com/jagoda/meshcadet/commit/474c4cfc3f16fda4e6b927f358d320382aa0d99e))
* **firmware-core:** repeater signal-strength tracker + ADR-0010 ([9e046df](https://github.com/jagoda/meshcadet/commit/9e046dff0e483e0c28fdcf1819a3e121c7b47cbf))
* **firmware:** wire the repeater signal meter into the rx path and UI ([4001c2c](https://github.com/jagoda/meshcadet/commit/4001c2c2e7ac95898551a24bd16e790fc9b806f9))
* **firmware:** wire the repeater signal meter into the rx path and UI ([35c39ec](https://github.com/jagoda/meshcadet/commit/35c39ecebe694c451097ec9519b9cef693429ab1))
* **release:** publish an app-only update artifact + layout compatibility gate ([75b45e2](https://github.com/jagoda/meshcadet/commit/75b45e2d6b6b3fcd35f9ced11eeb4aba6a2e2230))
* **release:** publish non-destructive app-only update artifacts ([2b585b3](https://github.com/jagoda/meshcadet/commit/2b585b31392a0f1ad582e0283503e163527eb4df))
* **site:** add a Getting Started section to the landing page ([62f6c57](https://github.com/jagoda/meshcadet/commit/62f6c57d9b217275e3dc2486b5908b9f1571b533))
* **site:** add a Getting Started section to the landing page ([b4fc37e](https://github.com/jagoda/meshcadet/commit/b4fc37ee19075dc63347a5435845492bd328762d))
* **site:** add promotional UI screenshots to the landing page ([b8f5bb9](https://github.com/jagoda/meshcadet/commit/b8f5bb95a4722a1c69c73ee094fc10ea311cc532))
* **site:** harden the Upgrade path per post-green + hardware-safety review ([ea3c972](https://github.com/jagoda/meshcadet/commit/ea3c97264564454d297e3dcdff52d84faa5939ff))
* **site:** two-path web flasher — Fresh install vs non-destructive Upgrade ([f74ba05](https://github.com/jagoda/meshcadet/commit/f74ba05db2240d4da109d6db52d2af9a9f4e6d19))
* **site:** two-path web flasher — Fresh install vs non-destructive Upgrade ([a6c1672](https://github.com/jagoda/meshcadet/commit/a6c16722463b9c90ba32aae8f1f1801beedbc5df))
* **site:** wire the four promo screenshots into the landing page gallery ([0cf3c0c](https://github.com/jagoda/meshcadet/commit/0cf3c0cba2485d68892e5f2469280cb28620aaee))
* **ui_sim:** add promo screenshot render rigs for four production screens ([91cbd16](https://github.com/jagoda/meshcadet/commit/91cbd16353a4b377fd72eca030e872281ac5860d))


### Fixed

* **firmware-core:** silence clippy on decay boundary test ([3876f6f](https://github.com/jagoda/meshcadet/commit/3876f6f3f78a748f0568ad0b670b56269acbf480))
* **release:** stop crashing the lockfile-sync commit step on every tag-only run ([1cc3c8c](https://github.com/jagoda/meshcadet/commit/1cc3c8c78011c1e13e6fbb49e5789a9946feae17))
* **release:** stop crashing the lockfile-sync commit step on every tag-only run ([cd63409](https://github.com/jagoda/meshcadet/commit/cd63409293f74b7c14c4e0aeb620ae1fd09174c1))
* **ui_sim:** sync promo screen markup with the merged signal-meter widget ([20c4493](https://github.com/jagoda/meshcadet/commit/20c44933e6d7aacdb32075f6eb31c5dab4058a0a))
* **ui:** move contact/channel-list signal meter to the right of the gear ([71d31c6](https://github.com/jagoda/meshcadet/commit/71d31c6e6b93e82d82a45c4d793b820a9dc171e8))
* **ui:** move contact/channel-list signal meter to the right of the gear ([7598a44](https://github.com/jagoda/meshcadet/commit/7598a445793fb6c9e5e7e3d20e2ae0eb40aa8013))


### Changed

* **release:** extract the layout-compatibility gate into a tested script ([e9d1053](https://github.com/jagoda/meshcadet/commit/e9d105384aebc83c3d0c569ed2cdb91f9b157d52))
* **site:** reorder landing page and unify navigation ([a2856fa](https://github.com/jagoda/meshcadet/commit/a2856faf5f0c00e81d1863e70fe22c43f79e685f))
* **site:** reorder landing page and unify navigation ([75df9b1](https://github.com/jagoda/meshcadet/commit/75df9b1a0f8a23d3c1abcfd58789d527de952449))


### Documentation

* **adr:** add ADR-0008 for non-destructive update artifacts ([5603611](https://github.com/jagoda/meshcadet/commit/5603611a9290463930b2dfbbe85b68f2818f3080))
* **adr:** add ADR-0010 for the repeater signal meter design ([399729a](https://github.com/jagoda/meshcadet/commit/399729aec113dcc278a0ceebad29e44e4e9c0715))
* **adr:** note the extracted gate script + verified site-mirror compatibility ([cec144a](https://github.com/jagoda/meshcadet/commit/cec144a6ee698a55bab9556b302d41195b17d68d))

## [0.2.0](https://github.com/jagoda/meshcadet/compare/v0.1.0...v0.2.0) (2026-07-13)


### Added

* **release:** add sync-cargo-lock-versions.sh + smoke test ([e386cb4](https://github.com/jagoda/meshcadet/commit/e386cb42a16ac2f0a67f0ba1f81fcea5d3869df7))


### Fixed

* **release:** post-green hardening for the Cargo.lock sync script ([df9f7e9](https://github.com/jagoda/meshcadet/commit/df9f7e9a184b317bc3fe703ffe9da00b7dcdd4c6))
* **release:** replace release-plz with release-please ([2cd1802](https://github.com/jagoda/meshcadet/commit/2cd1802bd8dc0a00998e22d49b981309be3750b1))
* **release:** replace release-plz with release-please ([9bdc928](https://github.com/jagoda/meshcadet/commit/9bdc92862bd59a5821e5fc0345b2d3c4c0d33872))
* **release:** sign the Cargo.lock sync commit via the GitHub API ([5302d79](https://github.com/jagoda/meshcadet/commit/5302d79739e5a400ebd4cb41c66c2039f8b3c73a))
* **release:** sign the Cargo.lock sync commit via the GitHub API ([c9a3f04](https://github.com/jagoda/meshcadet/commit/c9a3f04f0c582271cfd87fe6143e348ccbfa64c3))
* **release:** sync Cargo.lock/firmware/Cargo.lock on every release PR ([ecd1f18](https://github.com/jagoda/meshcadet/commit/ecd1f18410cbcad72d466dfbd4be0d8d9a49f77e))


### Documentation

* **release:** correct ADR-0004 §5's squash-merge premise ([35ab4ea](https://github.com/jagoda/meshcadet/commit/35ab4eafd0146ab54f98f1bf3d1f64cde08ad3f4))

## [0.1.0] - 2026-07-12

### MeshCadet

- Mesh-radio messaging firmware for the LilyGO T-Deck Plus


The first public release of MeshCadet: a deliberately-limited, MeshCore-interop
firmware for the LilyGo T-Deck Plus. Its limits are design choices for a
controlled, minimal comms device — MeshCadet is provided "as is" with no
warranty and no guarantee of safety or security; see the Disclaimer in
[`README.md`](README.md) and [`SECURITY.md`](SECURITY.md).

### Added

- **Protocol interop (`protocol/`)**: byte-exact Rust port of the MeshCore
  v1.15.0 wire protocol — packet framing, Ed25519/X25519 identity and ECDH,
  AES-128-ECB + HMAC-SHA256 DM/channel encryption, ACK codec, and routing.
- **Firmware (`firmware/`)**: ESP32-S3 device app for the T-Deck Plus —
  LoRa radio (SX1262) send/receive, GPS-backed pull-only telemetry, a
  touch-screen UI (Slint) for contacts/conversations/composing with a
  curated emoji set, on-device history storage, and a PIN-gated admin menu.
- **Allowlist policy layer**: allowlist-only contacts and channels, no
  device-initiated advertising, silent drop of all non-allowlisted traffic,
  pull-only (never push) location telemetry.
- **Admin host CLI (`host/`)**: USB-serial provisioning tool (`meshcadet`)
  for registering contacts/channels, setting notification defaults and a
  PIN, exporting history, and resetting a forgotten PIN.
- **Development tooling**: `xtask` (host-side glyph-coverage verification for
  the emoji/icon font pipeline), `ui_sim` (host-native Slint render rig for
  UI/asset verification without hardware), `ui_perf` (host-native UI
  performance measurement harness).
- Design record in `docs/adr/` (protocol/policy charter, provisioning
  wire format, UI toolkit choice) and a manual hardware verification
  checklist in `docs/hil-real-mesh-procedure.md`.
- GPLv3 licensing, upstream attribution (`NOTICE`), and a full third-party
  dependency license audit (`docs/licensing/`).

### Known limitations

See [`SECURITY.md`](SECURITY.md) and the README's
["Status and known limitations"](README.md#status-and-known-limitations)
section — notably: no at-rest encryption of provisioned data, no PIN
attempt lockout, and inherited AES-128-ECB from the MeshCore wire protocol.
