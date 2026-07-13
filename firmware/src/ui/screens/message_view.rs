// SPDX-License-Identifier: GPL-3.0-only
//! Message view screen — displays a conversation thread.
//!
//! Shows the message history for a single contact or channel.  Messages are
//! displayed in chronological order (oldest at top, newest at bottom).
//!
//! Each message bubble shows:
//! - Text with inline emoji rendered natively by Slint (UTF-8 code points)
//! - Sent/received indicator (alignment: right=ours, left=theirs)
//! - Timestamp (absolute or relative)
//! - ACK indicator (always a single ✓; grey when unacked, accent-blue when
//!   acked — color is the sole ack signal, there is no double-check glyph)
//!
//! The "📝 Write" button at the bottom navigates to [`ComposeScreen`].
//!
//! # Theme tokens + animation language
//!
//! Every color/font-size literal in this screen's `slint::slint!{}` block
//! (below) now reads from the shared `Theme` global (`ui/theme.slint`,
//! imported below) instead of a bare hex/px literal — a pixel-identical swap
//! (same values, same names, per `splash.rs`'s pilot precedent), with two
//! documented exceptions this screen was already carrying before the theme
//! pass and that the frozen token contract does not cover (see the
//! `mention_tier == 2` background, below, for both).
//!
//! Animation: `MessageViewScreen::new()` (`ui/mod.rs::navigate_to_message_view`)
//! constructs a FRESH `MessageViewScreenUi` every time the user navigates into
//! a thread, exactly like `ContactListScreen`'s own entry point — so this
//! screen gets the identical self-contained one-shot fade-in
//! (`content_opacity` + `init =>` + `animate`, no Rust wiring) as
//! `contact_list.rs`. Two more `animate` clauses apply the same "smooth
//! transition on an existing state-driven binding, never an infinite loop"
//! language `compose.rs` established, to this screen's own two live-state
//! transitions: the ack checkmark's grey→accent color flip (fires whenever a
//! DM's `acked` bit flips live, per `notification.rs`'s inbound-ACK wiring)
//! and the header/compose-button hover backgrounds. This is presentation-only:
//! no Rust wrapper method below was touched.
//!
//! # BUG FIX: header title off-center
//!
//! The header's `HorizontalLayout` places the back chevron (`HeaderIconButton`,
//! 44px wide) on the left and centers `contact_name`
//! (`horizontal-stretch: 1.0; horizontal-alignment: center`) in the space
//! beside it — with nothing balancing the chevron's width on the right, that
//! "remaining space" is itself off-center within the header, so the title's
//! visual center landed right of true screen center. Fixed the same way
//! `gps_status.rs`'s header already does it: a trailing
//! `Rectangle { width: 44px; height: 36px; }` spacer matching the chevron's
//! width, so the title centers across the FULL header width instead of the
//! space beside a single left-side element. The chevron itself is untouched
//! (still left-aligned, per the guard established when the compose
//! screen's back button and cursor were fixed). Applies to both DM and
//! channel views, since both render through this one `message_view.rs`.
//!
//! # Outer-space theme (per-screen spec row 4: "`nebula-violet`
//! own-message bubble tint" / "comet in header" / "comet-on-notify +
//! rocket-on-send")
//!
//! Three additive changes on top of the sections above, all still
//! presentation-only — no Rust wrapper *signature* changed, no handler logic
//! (send/receive/ack/navigation) touched:
//!
//! - **Own-bubble tint.** `MessageBubble`'s `is_ours` fill switches from
//!   `Theme.select` to the widened palette's `Theme.nebula-violet` (its
//!   `theme.slint` doc comment names this exact role: "own-message bubble
//!   tint"). The `mention_tier == 2` override (a literal, documented
//!   exception to the frozen token contract — see that binding's own doc
//!   comment above) still takes precedence over both; unchanged.
//! - **Comet in header.** A static `Comet` accent (`ui/motifs.slint`) sits in
//!   the header's dead space — the 44px zone the title-centering spacer
//!   Rectangle already reserves on the right — so it adds a persistent celestial
//!   motif without perturbing the back-chevron/centered-title/spacer
//!   HorizontalLayout that bug-fix established. `CometOnNotify`
//!   (same shared asset, animated) sweeps along the header's bottom 14px band
//!   — same position convention `contact_list.rs`'s header comet uses —
//!   whenever `notify_trigger` flips `false -> true`.
//! - **`notify_trigger` firing rule.** Mirrors `contact_list.rs`'s
//!   `notify_trigger`/`maybe_fire_notify` mechanism exactly, but keyed off a
//!   different quantity: the RECEIVED (`!is_ours`) message count in this
//!   thread, not an unread total (this screen has no unread concept of its
//!   own — the conversation is open). `MessageViewScreen::set_messages` (the
//!   one existing wrapper method every call site — initial navigate AND
//!   every live `refresh_message_view_for` — already funnels through)
//!   computes the received count and fires the comet iff it's a genuine
//!   increase over this SAME instance's previously-observed baseline
//!   (`None` on construction, so the initial populate never fires it, exactly
//!   like the contact-list precedent). Since `refresh_message_view_for` is
//!   only ever invoked from the `IncomingDm`/`IncomingGroupMsg`/`DmAcked`/
//!   `ChannelAcked`/`TelemetryResponse` branches of `UiRuntime::handle_event`
//!   (`ui/mod.rs`) — none of which this change touches — and only the first
//!   three of those five ever append a new `!is_ours` record (the Ack
//!   branches only flip an existing record's `acked` bit; `TelemetryResponse`
//!   injects a system-authored, received-style row), an Ack-only refresh
//!   changes nothing about the received count and correctly never re-fires
//!   the comet.
//! - **Rocket-on-send.** This screen has no send affordance of its own —
//!   composing and the actual Send tap both live on a *different* screen
//!   (`compose.rs`, which already carries its own `RocketOnSend` on its Send
//!   button). The "✏ Write" button here is this screen's one send-BOUND
//!   affordance (it's the sole door into composing a reply), so
//!   `RocketOnSend` fires inline from its own existing `clicked` handler —
//!   `root.compose_pressed(); root.rocket_trigger = true;` — the identical
//!   "flip a same-file `in-out property <bool>` inline in an existing
//!   callback, no new Rust wrapper method, no callback-signature change"
//!   pattern `compose.rs`'s own Send button already established, just
//!   attached to Write instead of Send since Write is the only button this
//!   screen has that is causally headed toward a send. A sibling `Timer`
//!   auto-resets `rocket_trigger` back to `false` ~100ms after the
//!   animation settles, mirroring `compose.rs`'s identical reset Timer.
//!
//! # Full-window starfield backdrop
//!
//! `SpaceBackdrop` is now the `Window`'s first (z-bottom) child, reversing
//! the earlier "scrolling-content screens excluded from backdrop" call.
//! The message list's `flick` Flickable carries no fill
//! of its own, so the backdrop already showed through between bubbles and
//! below the last message once added — only the bottom bar hosting the
//! "✏ Write" button needed its own fill changed, from opaque `Theme.bg-space`
//! to the same translucent `Theme.surface.with-alpha(0.55)` wash
//! `contact_list.rs`'s header uses, so the backdrop shows behind AND below
//! that button too, not just in the message list above it. The header and
//! message bubbles keep their existing opaque fills, unchanged.

slint::slint! {
    import { Theme } from "../theme.slint";
    import { Comet, CometOnNotify, RocketOnSend, SpaceBackdrop } from "../motifs.slint";
    import { SignalMeter } from "../signal_meter.slint";

    struct MessageEntry {
        text:         string,
        from_name:    string,
        time_str:     string,
        is_ours:      bool,
        acked:        bool,
        mention_tier: int,
    }

    component MessageBubble {
        in property <string>  text;
        // Sender name for a *received channel* message, sans the trailing
        // `": "` delimiter (Rust-side `build_message_items` parses it off
        // MeshCore's inline `"<name>: <msg>"` wire text — see that fn's
        // doc). Empty for DMs, sent messages, and channel messages with no
        // parseable prefix; emptiness is the signal this component uses to
        // fall back to plain single-run rendering, so the bold treatment
        // below is scoped to received channel messages only.
        in property <string>  from_name;
        in property <string>  time_str;
        in property <bool>    is_ours;
        in property <bool>    acked;
        // Highest `protocol::mention::MentionTier` found in `text` (Rust-side
        // `build_message_items`/`render_mentions`): 0 = no mention, 1 = an
        // other-node `@name` mention, 2 = a mention of THIS node's own name.
        // `text` itself already has `@[name]` wire brackets flattened to a
        // plain `@name` display string — this property exists purely to
        // drive the bubble tint below, self-mention more prominently than an
        // other-node mention (bubble-tier tint is the documented fallback
        // for Slint 1.16's lack of inline rich-text runs).
        in property <int>     mention_tier;

        height: content.preferred-height;

        content := HorizontalLayout {
            alignment: is_ours ? end : start;
            padding-left:  is_ours ? 48px : 8px;
            padding-right: is_ours ? 8px  : 48px;

            VerticalLayout {
                spacing: 2px;

                // Bubble + ack sit side by side in this row (not stacked),
                // so the ack glyph renders beside the message but as a
                // *sibling* of the colored Rectangle — outside its bounds —
                // instead of inside its padding box.
                //
                // `alignment: center` is what keeps short/single-glyph
                // messages from skewing left: with the default (Stretch)
                // box alignment, a row whose children all have a
                // stretch factor of 0 still gets force-grown to fill any
                // slack space (Slint's box-layout solver treats an
                // all-zero-stretch set as equal-weighted rather than
                // fixed-size — see i-slint-core's `adjust_items`
                // fallback), so a single character was being stretched
                // into a wide box and rendered flush left inside it.
                // Centering the row keeps the bubble (and the ack glyph
                // beside it) at natural size, centered as a group, whenever
                // there's slack to redistribute. Long/wrapped messages that
                // already consume all available width have no slack, so
                // they're unaffected. This centering lived one level deeper
                // (on the row *inside* the Rectangle) before the ack moved
                // out; it's needed here now because this row — not the
                // Rectangle's inner row — is the one that inherits the
                // slack from VerticalLayout.
                HorizontalLayout {
                    alignment: center;
                    spacing: 6px;

                    Rectangle {
                        // Mention tint (see `mention_tier`'s doc): tier 2
                        // (self-mention) fills the bubble with a stronger,
                        // brighter accent than the base bubble color and
                        // draws a matching 2px border — the "more prominent"
                        // half of the two-tier requirement. Tier 1
                        // (other-node mention) keeps the normal fill and
                        // adds a subtle 1px accent border only. Tier 0 is
                        // pixel-identical to pre-mention rendering (no
                        // border, unchanged fill) — the "existing render
                        // unchanged" acceptance criterion.
                        //
                        // `#0d3a52` (the tier-2 fill) is the one literal this
                        // theming pass leaves un-tokenized: the frozen
                        // `Theme` palette (`ui/theme.slint`) was named off
                        // the app's pre-mention color inventory and has no
                        // slot for this newer, mention-specific accent — it
                        // isn't a near-duplicate of any of the twelve tokens
                        // (unlike `#1a4a6b` just below, which the theme
                        // pass's own fold table names as a `select` duplicate).
                        // Inventing a same-value token for a single call site
                        // would misrepresent the contract as covering this
                        // color when it doesn't; kept literal and named here
                        // instead, flagged as a contract-amendment candidate
                        // for a later acceptance sweep.
                        //
                        // The `is_ours` base fill (mention_tier != 2) is
                        // `Theme.nebula-violet` — that token's own
                        // `theme.slint` doc comment names this exact role
                        // ("own-message bubble tint"), replacing the prior
                        // `Theme.select` fill. Received messages keep
                        // `Theme.surface-raised`, unchanged.
                        background: mention_tier == 2
                            ? #0d3a52
                            : (is_ours ? Theme.nebula-violet : Theme.surface-raised);
                        border-radius: 10px;
                        border-width: mention_tier == 0 ? 0px : (mention_tier == 2 ? 2px : 1px);
                        border-color: Theme.brand-signal;

                        // Padding properties only take effect on layout
                        // elements (HorizontalLayout/VerticalLayout/
                        // GridLayout) — Slint silently ignores them
                        // (compiler emits a warning, not an error) when set
                        // directly on a Rectangle, which is why the prior
                        // 16px -> 20px bump never showed up on-device: it
                        // was set on this Rectangle, not on a layout. Moving
                        // it onto the inner HorizontalLayout (which bounds
                        // the Text) is what actually constrains/insets the
                        // text width.
                        HorizontalLayout {
                            padding-left: 20px;
                            padding-right: 20px;
                            padding-top: 6px;
                            padding-bottom: 6px;

                            // Received channel messages carry a parsed
                            // `from_name` (see `MessageEntry`'s doc above);
                            // render the "<name>:" prefix bold and keep the
                            // body at normal weight, side by side so the
                            // name reads inline with the start of the
                            // message. Everything else (DMs, sent messages,
                            // channel messages with no parseable prefix)
                            // falls back to the single plain-weight Text this
                            // replaced — `from_name` is empty in all of
                            // those cases.
                            if from_name == "" : Text {
                                text: text;
                                font-size: Theme.size-body;
                                color: Theme.text-primary;
                                wrap: word-wrap;
                            }
                            if from_name != "" : HorizontalLayout {
                                spacing: 4px;

                                Text {
                                    text: from_name + ":";
                                    font-size: Theme.size-body;
                                    font-weight: 700;
                                    color: Theme.text-primary;
                                }

                                Text {
                                    text: text;
                                    font-size: Theme.size-body;
                                    color: Theme.text-primary;
                                    wrap: word-wrap;
                                    horizontal-stretch: 1;
                                }
                            }
                        }
                    }

                    // Ack checkmark sits to the right of the bubble, as a
                    // sibling of the Rectangle — outside the colored box —
                    // in the same row as the bubble, so it adds no extra
                    // height and stays beside (not below) the message. The
                    // glyph is always a single check — color is the sole ack
                    // signal (grey unacked -> accent-blue acked), there is no
                    // double-check state.
                    if is_ours : Text {
                        text: "✓";
                        font-size: Theme.size-caption;
                        color: acked ? Theme.brand-signal : Theme.text-secondary;
                        // Smooth grey->accent transition when a live inbound
                        // ACK flips this bit (see `notification.rs`), instead
                        // of an instant color snap — the theme's "animate an
                        // existing state-driven binding" language
                        // (`compose.rs`'s send-button precedent), not a
                        // screen-entry animation.
                        animate color { duration: 150ms; easing: ease-out; }
                    }
                }

                // Timestamp caption lives OUTSIDE the bubble's padding box —
                // it doesn't inflate the colored Rectangle's height. It sits
                // tight below the bubble+ack row (2px spacing above,
                // matching the layout's own spacing). The ack indicator
                // doesn't live here (see above); this row is just the time.
                Text {
                    text: time_str;
                    font-size: Theme.size-caption;
                    color: Theme.text-secondary;
                    horizontal-alignment: right;
                }
            }
        }
    }

    // Header icon button — forwarded `clicked` on a TouchArea that fills its
    // parent (mirrors the proven ContactRow pattern).  Replaces the inline raw
    // header TouchArea whose click did not complete on hardware.
    component HeaderIconButton {
        in property <string> icon;
        in property <color>  icon_color: Theme.text-secondary;
        in property <length> icon_size: Theme.size-title;
        callback clicked;

        Rectangle {
            background: touch.has-hover ? Theme.surface-raised : transparent;
            animate background { duration: 120ms; easing: ease-out; }
            Text {
                text: icon;
                font-size: icon_size;
                color: icon_color;
                horizontal-alignment: center;
                vertical-alignment: center;
            }
            touch := TouchArea {
                width: parent.width;
                height: parent.height;
                clicked => { root.clicked(); }
            }
        }
    }

    export component MessageViewScreenUi inherits Window {
        width: 320px;
        height: 240px;
        background: Theme.bg-space;

        // ── One-shot screen-entry animation ──
        // `MessageViewScreen::new()` (`ui/mod.rs::navigate_to_message_view`)
        // constructs a FRESH `MessageViewScreenUi` every time the user
        // (re-)enters a thread, so this `init =>` fade-in fires exactly once
        // per visit — same mechanism as `contact_list.rs`'s `content_opacity`
        // (see that file's doc). `PropertyAnimation::iteration-count` defaults
        // to 1 (not infinite), so it plays once and holds its end state.
        in-out property <float> content_opacity: 0;
        animate content_opacity { duration: 200ms; easing: ease-out; }
        init => { content_opacity = 1; }

        // ── Comet-on-notify trigger ── Retriggerable one-shot, same `play`-property
        // contract as every other `motifs.slint` motion helper. Rust flips
        // this `false -> true` from `MessageViewScreen::set_messages` the
        // first time this thread's received-message count genuinely
        // increases while this screen instance is showing — see this file's
        // module doc, "Outer-space theme" section, for the full firing rule.
        // Deliberately never reset back to `false` from Rust within one
        // instance: `MessageViewScreen::new()` (`ui/mod.rs::
        // navigate_to_message_view`) builds a FRESH `MessageViewScreenUi`
        // every time the user (re-)enters a thread (same fact
        // `content_opacity` above already relies on), so the property's
        // declared `false` default IS the re-arm for the next visit.
        in-out property <bool> notify_trigger: false;

        // Drives the Write button's `RocketOnSend` one-shot (see module
        // doc's "Outer-space theme" section for why this fires from Write,
        // not a screen-local Send) — flipped `true` inline in the Write
        // button's own `clicked` handler below, and auto-reset back to
        // `false` by the sibling `Timer` once the fired-state animation has
        // settled (mirrors `compose.rs`'s identical reset Timer).
        in-out property <bool> rocket_trigger: false;

        in property <string>          contact_name;
        in property <[MessageEntry]>  messages;
        // Repeater signal-meter reading (ADR-0010): 0 = direct-only,
        // 1..=5 = bars. Pushed by `MessageViewScreen::set_signal_level`; see
        // `SignalMeter`'s embedding below.
        in property <int>             signal_level: 0;

        callback back_pressed;
        callback compose_pressed;

        // Auto-reset for `rocket_trigger` — identical mechanism and duration
        // to `compose.rs`'s Send-button Timer (see that file's doc for the
        // "self-disabling one-shot" rationale): `running` tracks the trigger
        // itself, so arming it starts the timer and the instant it fires and
        // clears the trigger back to `false`, `running` follows it back down
        // — no separate "armed" bookkeeping needed.
        Timer {
            interval: 500ms;
            running: root.rocket_trigger;
            triggered => { root.rocket_trigger = false; }
        }

        // Pin the message list to the newest entry.  `flick.viewport-height`
        // auto-binds to the content VerticalLayout's min-height (Slint's
        // Flickable pass), so once messages/layout have settled this clamps
        // viewport-y to the bottom-most scroll offset.  Called from Rust right
        // after `set_messages()` — i.e. on every new-message arrival and on
        // view entry — so the newest message is always in view without
        // requiring a manual scroll.
        public function scroll_to_bottom() {
            flick.viewport-y = min(0px, flick.height - flick.viewport-height);
        }

        // Trackball roll: nudge the viewport by `delta_px` (positive = toward
        // the top/older messages, negative = toward the bottom/newer),
        // clamped to the same bounds `scroll_to_bottom` respects so a roll can
        // never scroll past either end of the thread.
        public function scroll_by(delta_px: int) {
            flick.viewport-y = max(
                min(0px, flick.height - flick.viewport-height),
                min(0px, flick.viewport-y + delta_px * 1px),
            );
        }

        // Full-window dim starfield backdrop — same z-bottom placement
        // `admin_menu.rs`'s `SpaceBackdrop` doc establishes, declared first
        // so it paints behind the header, the message list, and the
        // bottom Write-button bar below. The message list's `flick`
        // Flickable carries no fill of its own (only individual
        // `MessageBubble` rectangles paint), so the backdrop shows through
        // between bubbles and in the blank area below the last message; the
        // bottom bar's own background is switched to a translucent wash
        // (see that Rectangle below) so the backdrop shows behind AND below
        // the Write button too, by deliberate design.
        SpaceBackdrop {}

        VerticalLayout {
            // Screen-entry one-shot fade-in — see `content_opacity`'s doc above.
            opacity: content_opacity;

            // ── Header bar ──────────────────────────────────────────────────
            Rectangle {
                height: 36px;
                background: Theme.surface;

                HorizontalLayout {
                    padding-left: 4px;
                    padding-right: 8px;
                    spacing: 4px;

                    // Back button
                    HeaderIconButton {
                        width: 44px;
                        icon: "‹";
                        icon_size: Theme.size-display;
                        icon_color: Theme.brand-signal;
                        clicked => { root.back_pressed(); }
                    }

                    Text {
                        text: contact_name;
                        font-size: Theme.size-body-lg;
                        font-weight: 600;
                        color: Theme.text-primary;
                        horizontal-stretch: 1.0;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }

                    // Balance the back button's width so the title stays
                    // centered on the full header. Without this,
                    // the title only centers in the space beside the back
                    // button, landing right of true screen center — the
                    // same fix `gps_status.rs`'s header already carries.
                    //
                    // The `SignalMeter` (ADR-0010) nests INSIDE this spacer
                    // (same "don't touch the spacer's own reserved width"
                    // reasoning as `gps_status.rs`'s identical placement),
                    // but — unlike that screen — this header's top-right
                    // corner is already occupied by the static `Comet` motif
                    // floating just below, at this same Rectangle's
                    // `parent.width - 34px .. - 6px`, `y: 4px..18px` box (see
                    // that `Comet` instance's own comment). So the meter is
                    // pinned to the LEFT edge of this spacer instead of the
                    // right, at `x: 1px..17px, y: 3px..17px` — clear of the
                    // Comet's box AND the `CometOnNotify` sweep band
                    // (`y: parent.height - 14px..`, i.e. `22px..36px`) below.
                    Rectangle {
                        width: 44px; height: 36px;
                        SignalMeter {
                            signal-level: root.signal_level;
                            width: 16px;
                            height: 14px;
                            x: 1px;
                            y: 3px;
                        }
                    }
                }

                // Static comet motif — declared after the HorizontalLayout so it
                // paints on top, but positioned with an explicit x/y (not a
                // layout child) inside the dead space the balancing spacer
                // Rectangle above already reserves on the right. This keeps
                // the back-chevron/centered-title/spacer arrangement
                // completely
                // undisturbed while still giving this header a persistent
                // celestial accent, per this screen's per-screen spec row
                // ("comet in header").
                Comet {
                    x: parent.width - 34px;
                    y: 4px;
                }

                // Comet-on-notify sweep — same shared asset, animated,
                // sweeping the header's bottom 14px band exactly like
                // `contact_list.rs`'s header comet. See `notify_trigger`'s
                // doc above for the firing rule.
                CometOnNotify {
                    x: 0px;
                    y: parent.height - 14px;
                    play: root.notify_trigger;
                }
            }

            // ── Message list ──────────────────────────────────────────────────
            flick := Flickable {
                vertical-stretch: 1.0;
                VerticalLayout {
                    padding: 4px;
                    for m[i] in messages : MessageBubble {
                        text:         m.text;
                        from_name:    m.from_name;
                        time_str:     m.time_str;
                        is_ours:      m.is_ours;
                        acked:        m.acked;
                        mention_tier: m.mention_tier;
                    }
                }
            }

            // ── Compose button ────────────────────────────────────────────────
            // Bar fill switched from opaque `Theme.bg-space` to the same
            // translucent `Theme.surface` wash `contact_list.rs`'s header
            // uses — so the full-window `SpaceBackdrop` above shows behind
            // AND below the "✏ Write" button, not just in the message list
            // above it. The button itself keeps its own opaque
            // `brand-signal`/`brand-signal-bright` pill fill, unchanged, so
            // tap-target contrast is unaffected.
            Rectangle {
                height: 40px;
                background: Theme.surface.with-alpha(0.55);

                // Top separator (per-side borders aren't a Slint property)
                Rectangle {
                    y: 0px;
                    height: 1px;
                    width: parent.width;
                    background: Theme.surface-raised;
                }

                HorizontalLayout {
                    alignment: center;
                    padding: 6px;

                    Rectangle {
                        width: 120px;
                        height: 28px;
                        background: write_touch.has-hover ? Theme.brand-signal-bright : Theme.brand-signal;
                        animate background { duration: 120ms; easing: ease-out; }
                        border-radius: 14px;
                        Text {
                            text: "✏ Write";
                            font-size: Theme.size-body;
                            font-weight: 600;
                            color: Theme.bg-space;
                            horizontal-alignment: center;
                            vertical-alignment: center;
                        }
                        write_touch := TouchArea {
                            width: parent.width;
                            height: parent.height;
                            clicked => {
                                root.compose_pressed();
                                root.rocket_trigger = true;
                            }
                        }

                        // Floats above the button on its own explicit x/y —
                        // does not perturb this Rectangle's fixed 120x28 size
                        // or the action bar's HorizontalLayout flow (mirrors
                        // `compose.rs`'s identical Send-button placement).
                        RocketOnSend {
                            x: parent.width / 2 - self.width / 2;
                            y: -20px;
                            play: root.rocket_trigger;
                        }
                    }
                }
            }
        }
    }
}

// `received_total_increased` is pure Rust with no Slint dependency — it now
// lives in `firmware_core::ui::message_view` so its tests execute under
// `cargo test --workspace` (this crate is a detached, cross-compiled
// workspace — see `Cargo.toml`'s doc comment — so a `#[cfg(test)]` block
// written here would type-check but never run). `MessageItem` (plain data,
// no Slint dependency) moved alongside it, together with
// `build_message_items`/`render_mentions`/`wrap_outgoing_mentions` (the
// `UiRuntime` model-builder and its @mention glue that construct/consume
// it — see `firmware/src/ui/mod.rs`'s `refresh_message_view_for`/
// `navigate_to_message_view`/`on_send_message` call sites). `render_mentions`
// and `incoming_message_is_unread` are consumed only inside
// `firmware_core::ui::message_view` (the former) or imported directly from
// there by `ui/mod.rs` (the latter) — re-exported here as `build_message_items`/
// `wrap_outgoing_mentions`/`MessageItem` only, since `firmware`'s single
// `[[bin]]` target has no external re-export surface to keep an unused `pub
// use` alive (`cargo check` warns on it as dead code, unlike a library
// crate). Only this Slint-backed view wrapper stays. See
// `docs/adr/0005-firmware-core-extraction.md`.
use firmware_core::ui::message_view::received_total_increased;
pub use firmware_core::ui::message_view::{build_message_items, wrap_outgoing_mentions, MessageItem};

/// Rust-side wrapper.
pub struct MessageViewScreen {
    component: self::MessageViewScreenUi,
    // Baseline for the comet-on-notify trigger (see `notify_trigger`'s doc in
    // the markup above and `received_total_increased` above) — `None` until
    // the first `set_messages` call on THIS instance records one. `Cell`, not
    // a plain field, because every method here takes `&self` (mirrors
    // `ContactListScreen`'s identical need for its own notify baselines).
    prev_received_count: std::cell::Cell<Option<i32>>,
    // PERSISTENT message model, reconciled in place by `set_messages` rather
    // than replaced wholesale.
    //
    // # Why persistent (repaint-scope optimization)
    //
    // The Slint `SoftwareRenderer` runs in `RepaintBufferType::ReusedBuffer`
    // (dirty-region) mode: `ui/platform.rs::render_if_needed` flushes only the
    // renderer's per-frame dirty region over the shared SPI2 bus. REPLACING a
    // model (`set_messages(ModelRc::new(fresh_vec))`, the previous behavior)
    // makes the renderer conservatively invalidate the WHOLE 320x240 window —
    // every scanline re-flushed — because it cannot diff the old (dropped)
    // repeater instances against the new ones. Updating the SAME model
    // in place (push a newly-arrived row, `set_row_data` a row whose ack state
    // flipped, leave unchanged rows untouched) invalidates ONLY the rows that
    // actually changed, so the static space backdrop + header strip are not
    // re-flushed on every incoming message. Measured on host
    // (`ui_perf/tests/model_update_repaint.rs`): a single new message goes from
    // a 240-line full-window flush to a ~22-line scoped flush — pixel-identical
    // output, far fewer SPI holds competing with the radio during message
    // traffic. `Rc<VecModel>` so this wrapper keeps a live handle while the
    // component owns the same model via `ModelRc`.
    messages_model: std::rc::Rc<slint::VecModel<MessageEntry>>,
}

impl MessageViewScreen {
    pub fn new() -> anyhow::Result<Self> {
        let component = self::MessageViewScreenUi::new()
            .map_err(|e| anyhow::anyhow!("slint component init: {:?}", e))?;
        component.show()
            .map_err(|e| anyhow::anyhow!("slint window show: {:?}", e))?;
        // Install ONE persistent, initially empty model on the component; every
        // `set_messages` call reconciles this same instance in place (see the
        // field doc for the repaint-scope rationale).
        let messages_model = std::rc::Rc::new(slint::VecModel::<MessageEntry>::default());
        component.set_messages(slint::ModelRc::from(messages_model.clone()));
        Ok(MessageViewScreen {
            component,
            prev_received_count: std::cell::Cell::new(None),
            messages_model,
        })
    }

    pub fn set_contact_name(&self, name: &str) {
        self.component.set_contact_name(name.into());
    }

    /// Push a fresh repeater signal-meter reading (ADR-0010) into the
    /// header's `SignalMeter` — see `GpsStatusScreen::set_signal_level`'s
    /// identical doc for the `bars` contract.
    pub fn set_signal_level(&self, bars: i32) {
        self.component.set_signal_level(bars);
    }

    pub fn set_messages(&self, messages: &[MessageItem]) {
        use slint::Model as _;
        // Reconcile the PERSISTENT model in place instead of replacing it, so a
        // new-message refresh dirties only the changed rows and leaves the
        // static backdrop/header un-reflushed (see `messages_model`'s field doc
        // for the full repaint-scope rationale). The final model content is
        // byte-identical to what the old wholesale-rebuild produced — this is a
        // pure flush-scope change, not a content/order/behavior change.
        let model = &*self.messages_model;
        let mut received_count: i32 = 0;
        let old_len = model.row_count();
        for (i, m) in messages.iter().enumerate() {
            if !m.is_ours {
                received_count += 1;
            }
            let entry = MessageEntry {
                text:         m.text.clone().into(),
                from_name:    m.from_name.clone().into(),
                time_str:     m.time_str.clone().into(),
                is_ours:      m.is_ours,
                acked:        m.acked,
                mention_tier: m.mention_tier,
            };
            if i < old_len {
                // Only write (and thus dirty) a row whose content actually
                // changed — an unchanged row is skipped so it isn't re-flushed.
                // The common live-refresh case (one appended message, earlier
                // rows unchanged) writes nothing here and pushes once below.
                if model.row_data(i).as_ref() != Some(&entry) {
                    model.set_row_data(i, entry);
                }
            } else {
                model.push(entry);
            }
        }
        // Drop any surplus tail rows if the new list is shorter than the old
        // (not expected on the append-only live path, but keeps the model an
        // exact mirror of `messages` for correctness).
        while model.row_count() > messages.len() {
            model.remove(model.row_count() - 1);
        }
        // Layout (and thus `flick.viewport-height`) settles synchronously off
        // the model update above, so it's safe to clamp the scroll position
        // to the bottom in the same call — covers both new-message arrival
        // and initial view entry (both funnel through `set_messages`).
        self.component.invoke_scroll_to_bottom();
        // Comet-on-notify — see `received_total_increased`'s doc. Fires iff
        // this call's received count is a genuine increase over this
        // instance's previously-observed baseline; the baseline is then
        // updated unconditionally (same "compare-then-unconditionally-update"
        // shape as `contact_list.rs`'s `maybe_fire_notify`).
        if received_total_increased(self.prev_received_count.get(), received_count) {
            self.component.set_notify_trigger(true);
        }
        self.prev_received_count.set(Some(received_count));
    }

    pub fn on_back_pressed(&self, cb: impl Fn() + 'static) {
        self.component.on_back_pressed(cb);
    }

    pub fn on_compose_pressed(&self, cb: impl Fn() + 'static) {
        self.component.on_compose_pressed(cb);
    }

    /// Fire `back_pressed` exactly as the header's back button would — used
    /// by the trackball's roll-Left handler so both input paths funnel
    /// through the same navigation logic.
    pub fn invoke_back_pressed(&self) {
        self.component.invoke_back_pressed();
    }

    /// Fire `compose_pressed` exactly as the "✏ Write" button would — used by
    /// the trackball's Click handler.
    pub fn invoke_compose_pressed(&self) {
        self.component.invoke_compose_pressed();
    }

    /// Scroll the thread by `delta_px` (positive = up/older, negative =
    /// down/newer) — see the Slint `scroll_by` function's doc for the clamp.
    /// Used by the trackball's roll-Up/Down handler.
    pub fn scroll_by(&self, delta_px: i32) {
        self.component.invoke_scroll_by(delta_px);
    }

    pub fn hide(&self) { self.component.hide().ok(); }
}

// `received_total_increased`'s tests moved to
// `firmware-core/src/ui/message_view.rs` alongside the function — see this
// file's module-level move note above.
