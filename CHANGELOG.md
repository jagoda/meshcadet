# Changelog

All notable changes to MeshCadet are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning is managed by [release-please](https://github.com/googleapis/release-please)
— see `release-please-config.json` and `docs/adr/0004-release-architecture.md`.
The entry below documents everything landed before release-please's first
`chore(release): vX.Y.Z` PR.

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
